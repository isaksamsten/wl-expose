//! Application state and the SCTK handlers that don't belong to input.

use std::rc::Rc;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_dispatch2, delegate_registry,
    foreign_toplevel_list::{ForeignToplevelList, ForeignToplevelListHandler},
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        LoopHandle, RegistrationToken,
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
use crate::input::Input;
use crate::ipc::Request;
use crate::overlay::{Overlay, Wallpaper, WindowId};
use crate::toplevel::{self, Toplevel};

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
            overlay: None,
            deadline: None,
            outcome: None,
            inbox: Vec::new(),
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
            .map(|&i| (handles[i].clone(), toplevels[i].identifier.clone()))
            .collect();
        let mut overlay = Overlay::new(g.clone(), windows, req, config, wallpaper, qh)
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

    fn update_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: ExtForeignToplevelHandleV1,
    ) {
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

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}

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
