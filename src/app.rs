//! Application state and the SCTK handlers that don't belong to input.

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_dispatch2, delegate_registry,
    foreign_toplevel_list::{ForeignToplevelList, ForeignToplevelListHandler},
    output::{OutputHandler, OutputState},
    reexports::calloop::LoopHandle,
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
use crate::globals::Shm;
use crate::input::Input;
use crate::overlay::{Overlay, WindowId};
use crate::toplevel::Toplevel;

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
    /// Created once the windows to show are known; None with `--list`.
    pub overlay: Option<Overlay>,
    /// Set once the user picked or cancelled.
    pub outcome: Option<Outcome>,
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
            outcome: None,
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
        f: impl FnOnce(&mut Capture, &Shm) -> Option<CaptureEvent>,
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
