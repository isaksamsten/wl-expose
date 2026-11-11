//! Application state and the SCTK handlers that don't belong to input.

use std::rc::Rc;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_dispatch2, delegate_registry,
    foreign_toplevel_list::{ForeignToplevelList, ForeignToplevelListHandler},
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        LoopHandle, RegistrationToken, channel,
        timer::{TimeoutAction, Timer},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{SeatState, pointer::cursor_shape::CursorShapeManager},
    shell::wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::GlobalList,
    protocol::{wl_output, wl_surface::WlSurface},
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;

use crate::capture::{Capture, CaptureEvent};
use crate::config::Config;
use crate::daemon::Incoming;
use crate::globals::Globals;
use crate::icons;
use crate::input::Input;
use crate::ipc::Request;
use crate::overlay::{Overlay, Wallpaper, WindowId};
use crate::toplevel::{self, Toplevel};
use crate::worker::{Assets, Done};

pub enum Outcome {
    /// The identifier of the picked window.
    Picked(String),
    Cancelled,
}

pub struct App {
    registry: RegistryState,
    pub(crate) seat_state: SeatState,
    output_state: OutputState,
    pub toplevels: ForeignToplevelList,
    pub(crate) cursor_shape: Option<CursorShapeManager>,
    pub(crate) loop_handle: LoopHandle<'static, App>,
    pub(crate) input: Input,
    /// The worker thread for icons and titles, started with the first show.
    assets: Option<Rc<Assets>>,
    /// The current overlay, while a show is in progress.
    pub overlay: Option<Overlay>,
    /// The first-frame timer of the current show, until it fires.
    deadline: Option<RegistrationToken>,
    /// Set once the user picked or cancelled.
    pub outcome: Option<Outcome>,
    /// Client events for the daemon loop.
    pub(crate) inbox: Vec<Incoming>,
}

impl App {
    pub fn new(
        globals: &GlobalList,
        qh: &QueueHandle<App>,
        loop_handle: LoopHandle<'static, App>,
    ) -> Self {
        Self {
            registry: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            toplevels: ForeignToplevelList::new(globals, qh),
            cursor_shape: CursorShapeManager::bind(globals, qh).ok(),
            loop_handle,
            input: Input::default(),
            assets: None,
            overlay: None,
            deadline: None,
            outcome: None,
            inbox: Vec::new(),
        }
    }

    /// The worker, started on first use; its results go to the overlay.
    fn assets(&mut self) -> Result<Rc<Assets>, String> {
        if let Some(assets) = &self.assets {
            // A daemon's entries may have changed since the last show.
            assets.refresh();
            return Ok(assets.clone());
        }
        let (assets, results) =
            Assets::spawn().map_err(|e| format!("failed to start the worker: {e}"))?;
        self.loop_handle
            .insert_source(results, |event, _, app| {
                if let channel::Event::Msg(done) = event {
                    app.worker_done(done);
                }
            })
            .map_err(|e| format!("failed to watch the worker: {e}"))?;
        let assets = Rc::new(assets);
        self.assets = Some(assets.clone());
        Ok(assets)
    }

    fn worker_done(&mut self, done: Done) {
        if let Some(assets) = &self.assets {
            assets.received(&done);
        }
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        match done {
            Done::Icon { key, .. } => overlay.icon_ready(&key),
            Done::IconsReloaded => overlay.icons_reloaded(),
            Done::Pill {
                window,
                request,
                pill,
            } => overlay.pill_ready(window, request, pill),
        }
    }

    /// Forget cached icons, e.g. because the config changed.
    pub fn clear_icons(&self) {
        if let Some(assets) = &self.assets {
            assets.clear_icons();
        }
    }

    /// Have the worker load the icons of every open window at every output
    /// scale, so the daemon's next show has them on its first frame.
    pub fn prefetch_icons(&mut self, config: &Config) {
        if config.icon_size == 0 {
            return;
        }
        let assets = match self.assets() {
            Ok(assets) => assets,
            Err(e) => return eprintln!("wl-expose: {e}"),
        };
        let mut scales: Vec<i32> = self
            .output_state
            .outputs()
            .filter_map(|o| self.output_state.info(&o))
            .map(|info| info.scale_factor.max(1))
            .collect();
        scales.sort_unstable();
        scales.dedup();
        if scales.is_empty() {
            scales.push(1);
        }
        for (_, toplevel) in self.toplevels() {
            for &scale in &scales {
                let key = icons::Key::new(&toplevel.app_id, config, scale);
                assets.icon(&key, &config.font);
            }
        }
    }

    /// Open toplevels with their state, in announcement order.
    pub fn toplevels(&self) -> Vec<(ExtForeignToplevelHandleV1, Toplevel)> {
        self.toplevels
            .toplevels()
            .iter()
            .filter_map(|handle| {
                let info = self.toplevels.info(handle)?;
                let toplevel = Toplevel {
                    identifier: info.identifier,
                    app_id: info.app_id,
                    title: info.title,
                };
                Some((handle.clone(), toplevel))
            })
            .collect()
    }

    /// Open the overlay for `req` and start its first-frame timer. Warnings
    /// about the request go to `warn`.
    pub fn show(
        &mut self,
        g: &Rc<Globals>,
        req: &Request,
        config: &Config,
        wallpaper: Option<Rc<Wallpaper>>,
        qh: &QueueHandle<App>,
        warn: &mut dyn FnMut(String),
    ) -> Result<(), String> {
        let (handles, toplevels): (Vec<_>, Vec<_>) = self.toplevels().into_iter().unzip();
        let shown = toplevel::select(&toplevels, &req.ids, |id| {
            warn(format!("no window with ID '{id}'"));
        });
        if shown.is_empty() {
            return Err("no windows to show".into());
        }
        let windows: Vec<_> = shown
            .iter()
            .map(|&i| (handles[i].clone(), toplevels[i].clone()))
            .collect();
        let assets = self.assets()?;
        let mut overlay = Overlay::new(g.clone(), windows, req, config, wallpaper, assets, qh)
            .map_err(|e| format!("failed to create the overlay: {e}"))?;
        // The first window passed in starts selected, unless --select says otherwise.
        let mut selected = 0;
        if let Some(id) = &req.select {
            match overlay.windows.iter().position(|w| w.identifier == *id) {
                Some(i) => selected = i,
                None => warn(format!("--select: no shown window with ID '{id}'")),
            }
        }
        overlay.selected = Some(selected);
        self.overlay = Some(overlay);
        self.outcome = None;

        let timer = Timer::from_duration(Duration::from_millis(req.timeout_ms.into()));
        let token = self
            .loop_handle
            .insert_source(timer, |_, _, app| {
                app.deadline = None;
                if let Some(overlay) = &mut app.overlay {
                    overlay.deadline_passed();
                }
                TimeoutAction::Drop
            })
            .map_err(|e| format!("failed to start the timer: {e}"))?;
        self.deadline = Some(token);
        Ok(())
    }

    /// After each event loop dispatch: map once ready.
    pub fn after_dispatch(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.maybe_map();
        }
    }

    /// Destroy the overlay. The caller should roundtrip before acting on the
    /// outcome, so the overlay is gone by then.
    pub fn end_show(&mut self) {
        self.overlay = None;
        if let Some(token) = self.deadline.take() {
            self.loop_handle.remove(token);
        }
        self.input.reset_pointer();
    }
    /// The first outcome wins.
    pub fn finish(&mut self, outcome: Outcome) {
        self.outcome.get_or_insert(outcome);
    }

    pub fn pick(&mut self, i: usize) {
        let Some(win) = self.overlay.as_ref().and_then(|o| o.windows.get(i)) else {
            return;
        };
        self.finish(Outcome::Picked(win.identifier.clone()));
    }

    pub fn capture_update(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut Capture, &Globals) -> Option<CaptureEvent>,
    ) {
        if let Some(overlay) = &mut self.overlay {
            overlay.capture_update(id, f);
        }
    }
}

impl ForeignToplevelListHandler for App {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelList {
        &mut self.toplevels
    }

    fn new_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: ExtForeignToplevelHandleV1,
    ) {
    }

    /// Titles (and app_ids) update live.
    fn update_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        handle: ExtForeignToplevelHandleV1,
    ) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        let Some(info) = self.toplevels.info(&handle) else {
            return;
        };
        let toplevel = Toplevel {
            identifier: info.identifier,
            app_id: info.app_id,
            title: info.title,
        };
        overlay.update_toplevel(&handle, &toplevel);
    }

    fn toplevel_closed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        handle: ExtForeignToplevelHandleV1,
    ) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        let Some(i) = overlay.windows.iter().position(|w| w.handle == handle) else {
            return;
        };
        overlay.remove(i);
        if overlay.windows.is_empty() {
            eprintln!("wl-expose: all windows closed");
            self.finish(Outcome::Cancelled);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.finish(Outcome::Cancelled);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(overlay) = &mut self.overlay {
            let (width, height) = configure.new_size;
            overlay.configure(width, height);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        if let Some(overlay) = &mut self.overlay
            && overlay.surface() == surface
        {
            overlay.set_scale(factor);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if let Some(overlay) = &mut self.overlay
            && overlay.surface() == surface
        {
            overlay.frame_done();
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
delegate_dispatch2!(App);
