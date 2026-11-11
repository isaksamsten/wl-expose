//! Seat handling: keyboard (xkbcommon, with key repeat) and pointer.

use smithay_client_toolkit::seat::{
    Capability, SeatHandler, SeatState,
    keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    pointer::{PointerEvent, PointerEventKind, PointerHandler},
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    protocol::{
        wl_keyboard::WlKeyboard, wl_pointer::WlPointer, wl_seat::WlSeat, wl_surface::WlSurface,
    },
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};

use crate::app::{App, Outcome};
use crate::layout::Dir;

/// linux/input-event-codes.h
const BTN_LEFT: u32 = 0x110;

#[derive(Default)]
pub struct Input {
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    cursor_shape: Option<WpCursorShapeDeviceV1>,
    /// Last pointer position on the overlay surface.
    pointer_pos: (f64, f64),
    pointer_inside: bool,
    shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Cancel,
    PickSelected,
    /// Pick the n-th window in input order.
    Pick(usize),
    Move(Dir),
    Cycle(isize),
}

impl Action {
    fn from_key(sym: Keysym, shift: bool) -> Option<Self> {
        Some(match sym {
            Keysym::Escape => Action::Cancel,
            Keysym::Return | Keysym::KP_Enter => Action::PickSelected,
            Keysym::Left | Keysym::h => Action::Move(Dir::Left),
            Keysym::Right | Keysym::l => Action::Move(Dir::Right),
            Keysym::Up | Keysym::k => Action::Move(Dir::Up),
            Keysym::Down | Keysym::j => Action::Move(Dir::Down),
            Keysym::Tab => Action::Cycle(if shift { -1 } else { 1 }),
            Keysym::ISO_Left_Tab => Action::Cycle(-1),
            _ => {
                let n = sym.raw().checked_sub(Keysym::_1.raw()).filter(|&n| n < 9)?;
                Action::Pick(n as usize)
            }
        })
    }

    /// Navigation repeats while the key is held; picking doesn't.
    fn repeats(self) -> bool {
        matches!(self, Action::Move(_) | Action::Cycle(_))
    }
}

impl App {
    fn key(&mut self, sym: Keysym, repeat: bool) {
        let Some(action) = Action::from_key(sym, self.input.shift) else {
            return;
        };
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if repeat && !action.repeats() {
            return;
        }
        match action {
            Action::Cancel => self.finish(Outcome::Cancelled),
            Action::PickSelected => {
                if let Some(i) = overlay.selected {
                    self.pick(i);
                }
            }
            Action::Pick(i) => self.pick(i),
            Action::Move(dir) => overlay.move_selection(dir),
            Action::Cycle(step) => overlay.cycle(step),
        }
    }

    fn release_keyboard(&mut self) {
        if let Some(kb) = self.input.keyboard.take()
            && kb.version() >= 3
        {
            kb.release();
        }
    }

    fn release_pointer(&mut self) {
        if let Some(device) = self.input.cursor_shape.take() {
            device.destroy();
        }
        if let Some(pointer) = self.input.pointer.take()
            && pointer.version() >= 3
        {
            pointer.release();
        }
        self.input.pointer_inside = false;
    }

    pub fn release_input(&mut self) {
        self.release_keyboard();
        self.release_pointer();
    }
}

impl Input {
    /// The overlay surface is gone; a new one gets a fresh enter.
    pub fn reset_pointer(&mut self) {
        self.pointer_inside = false;
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        cap: Capability,
    ) {
        match cap {
            Capability::Keyboard if self.input.keyboard.is_none() => {
                let repeat = Box::new(|app: &mut App, _: &WlKeyboard, event: KeyEvent| {
                    app.key(event.keysym, true);
                });
                let loop_handle = self.loop_handle.clone();
                match self
                    .seat_state
                    .get_keyboard_with_repeat(qh, &seat, None, loop_handle, repeat)
                {
                    Ok(kb) => self.input.keyboard = Some(kb),
                    Err(e) => eprintln!("wl-expose: keyboard unavailable: {e}"),
                }
            }
            Capability::Pointer if self.input.pointer.is_none() => {
                match self.seat_state.get_pointer(qh, &seat) {
                    Ok(pointer) => {
                        self.input.cursor_shape = self
                            .cursor_shape
                            .as_ref()
                            .map(|m| m.get_shape_device(&pointer, qh));
                        self.input.pointer = Some(pointer);
                    }
                    Err(e) => eprintln!("wl-expose: pointer unavailable: {e}"),
                }
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        cap: Capability,
    ) {
        match cap {
            Capability::Keyboard => self.release_keyboard(),
            Capability::Pointer => self.release_pointer(),
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event.keysym, false);
    }

    /// Compositor-side repeat (wl_keyboard v10).
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event.keysym, true);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.input.shift = modifiers.shift;
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(overlay) = &mut self.overlay else {
                return;
            };
            if &event.surface != overlay.surface() {
                continue;
            }
            let (x, y) = event.position;
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    // Don't select on enter: the pointer may just happen to
                    // rest on a thumbnail when the overlay appears.
                    self.input.pointer_pos = (x, y);
                    self.input.pointer_inside = true;
                    if let Some(device) = &self.input.cursor_shape {
                        device.set_shape(serial, Shape::Default);
                    }
                }
                PointerEventKind::Leave { .. } => self.input.pointer_inside = false,
                PointerEventKind::Motion { .. } => {
                    self.input.pointer_pos = (x, y);
                    if let Some(i) = overlay.hit(x, y) {
                        overlay.select(i);
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    if !overlay.mapped() || !self.input.pointer_inside {
                        continue;
                    }
                    let (px, py) = self.input.pointer_pos;
                    match overlay.hit(px, py) {
                        None => self.finish(Outcome::Cancelled), // any click on the background
                        Some(i) if button == BTN_LEFT => self.pick(i),
                        Some(_) => {}
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_actions() {
        assert_eq!(
            Action::from_key(Keysym::Escape, false),
            Some(Action::Cancel)
        );
        assert_eq!(
            Action::from_key(Keysym::l, false),
            Some(Action::Move(Dir::Right))
        );
        assert_eq!(Action::from_key(Keysym::Tab, true), Some(Action::Cycle(-1)));
        assert_eq!(Action::from_key(Keysym::_1, false), Some(Action::Pick(0)));
        assert_eq!(Action::from_key(Keysym::_9, false), Some(Action::Pick(8)));
        assert_eq!(Action::from_key(Keysym::_0, false), None);
        assert_eq!(Action::from_key(Keysym::a, false), None);
        assert!(!Action::Pick(0).repeats() && Action::Cycle(1).repeats());
    }
}
