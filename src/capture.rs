//! Per-window capture: one ext_image_copy_capture session, two shm buffers.
//!
//! Frames are captured into the back buffer; on `ready` the buffer is handed
//! to the overlay (attached to the thumbnail subsurface) and the buffers swap.
//! In live mode the next frame is requested right away; the compositor only
//! answers once the window has new content.

use smithay_client_toolkit::{dispatch2::Dispatch2, shm::raw::RawPool};
use wayland_client::{
    Connection, QueueHandle, WEnum,
    protocol::{wl_buffer::WlBuffer, wl_output::Transform, wl_shm},
};
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
        ext_image_copy_capture_manager_v1::Options,
        ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
    },
};

use crate::app::App;
use crate::globals::{Globals, Ignore, Shm};
use crate::overlay::WindowId;

const MAX_FAILURES: u32 = 3;

/// What the overlay needs to know about.
pub enum CaptureEvent {
    /// A new frame to show. Its buffer stays busy until the compositor
    /// releases it.
    Frame(Frame),
    /// Capturing gave up before any frame arrived.
    Failed(&'static str),
}

pub struct Frame {
    pub buffer: WlBuffer,
    pub width: u32,
    pub height: u32,
    pub transform: Transform,
}

enum State {
    /// Waiting for the first buffer constraints.
    Starting,
    /// Buffers are allocated; a frame is requested once the back buffer is free.
    Idle,
    /// A frame is in flight. `realloc`: new constraints arrived meanwhile and
    /// are applied when it finishes.
    InFlight {
        frame: ExtImageCopyCaptureFrameV1,
        realloc: bool,
    },
    /// No further frames will be requested.
    Done,
}

/// The latest buffer constraints from the session.
#[derive(Default)]
struct Constraints {
    width: u32,
    height: u32,
    shm_formats: Vec<wl_shm::Format>,
    /// A `done` ended the batch; the next constraint event starts a new one.
    complete: bool,
}

impl Constraints {
    fn begin_event(&mut self) {
        if self.complete {
            self.shm_formats.clear();
            self.complete = false;
        }
    }

    fn shm_format(&self) -> Option<wl_shm::Format> {
        use wl_shm::Format::*;
        self.shm_formats
            .iter()
            .copied()
            .find(|f| matches!(f, Argb8888 | Xrgb8888 | Abgr8888 | Xbgr8888))
    }
}

pub struct Capture {
    id: WindowId,
    live: bool,
    source: ExtImageCaptureSourceV1,
    session: Option<ExtImageCopyCaptureSessionV1>,
    constraints: Constraints,
    buffers: Option<Buffers>,
    state: State,
    transform: Transform,
    /// Consecutive failed frames.
    failures: u32,
    /// At least one frame was handed to the overlay.
    have_frame: bool,
}

impl Capture {
    pub fn new(
        id: WindowId,
        toplevel: &ExtForeignToplevelHandleV1,
        g: &Globals,
        live: bool,
        cursors: bool,
        qh: &QueueHandle<App>,
    ) -> Self {
        let source = g.source_manager.create_source(toplevel, qh, Ignore);
        let options = if cursors {
            Options::PaintCursors
        } else {
            Options::empty()
        };
        let session = g
            .copy_manager
            .create_session(&source, options, qh, SessionData(id));
        Self {
            id,
            live,
            source,
            session: Some(session),
            constraints: Constraints::default(),
            buffers: None,
            state: State::Starting,
            transform: Transform::Normal,
            failures: 0,
            have_frame: false,
        }
    }

    /// Buffer size announced by the compositor, if any.
    pub fn size(&self) -> Option<(u32, u32)> {
        let Constraints { width, height, .. } = self.constraints;
        (width > 0 && height > 0).then_some((width, height))
    }

    /// The first frame arrived or capturing gave up.
    pub fn settled(&self) -> bool {
        self.have_frame || matches!(self.state, State::Done)
    }

    fn give_up(&mut self, why: &'static str) -> Option<CaptureEvent> {
        if let State::InFlight { frame, .. } = std::mem::replace(&mut self.state, State::Done) {
            frame.destroy();
        }
        (!self.have_frame).then_some(CaptureEvent::Failed(why))
    }

    /// (Re)allocate buffers for the latest constraints and request a frame.
    fn apply_constraints(&mut self, shm: &Shm, qh: &QueueHandle<App>) -> Option<CaptureEvent> {
        let Some(format) = self.constraints.shm_format() else {
            return self.give_up("no supported shm format");
        };
        let Some((width, height)) = self.size() else {
            return self.give_up("zero buffer size");
        };
        if !self
            .buffers
            .as_ref()
            .is_some_and(|b| b.matches(width, height, format))
        {
            self.buffers = None;
            match Buffers::new(shm, self.id, width, height, format, qh) {
                Ok(b) => self.buffers = Some(b),
                Err(e) => {
                    eprintln!("wl-expose: shm allocation failed: {e}");
                    return self.give_up("shm allocation failed");
                }
            }
        }
        self.state = State::Idle;
        self.request_frame(qh);
        None
    }

    fn request_frame(&mut self, qh: &QueueHandle<App>) {
        let (State::Idle, Some(session), Some(bufs)) = (&self.state, &self.session, &self.buffers)
        else {
            return;
        };
        if bufs.busy[bufs.back] {
            return; // resumed from buffer_released
        }
        self.transform = Transform::Normal;
        let frame = session.create_frame(qh, FrameData(self.id));
        frame.attach_buffer(&bufs.slots[bufs.back]);
        frame.damage_buffer(0, 0, bufs.width as i32, bufs.height as i32);
        frame.capture();
        self.state = State::InFlight {
            frame,
            realloc: false,
        };
    }

    /// Takes the finished in-flight frame, leaving the capture idle.
    fn finish_frame(&mut self) -> Option<bool> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::InFlight { frame, realloc } => {
                frame.destroy();
                Some(realloc)
            }
            other => {
                self.state = other;
                None
            }
        }
    }

    fn next_frame(
        &mut self,
        realloc: bool,
        shm: &Shm,
        qh: &QueueHandle<App>,
    ) -> Option<CaptureEvent> {
        if realloc {
            self.apply_constraints(shm, qh)
        } else {
            self.request_frame(qh);
            None
        }
    }

    pub fn session_event(
        &mut self,
        event: ext_image_copy_capture_session_v1::Event,
        shm: &Shm,
        qh: &QueueHandle<App>,
    ) -> Option<CaptureEvent> {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => {
                self.constraints.begin_event();
                self.constraints.width = width;
                self.constraints.height = height;
            }
            Event::ShmFormat { format } => {
                self.constraints.begin_event();
                if let WEnum::Value(format) = format {
                    self.constraints.shm_formats.push(format);
                }
            }
            Event::DmabufDevice { .. } | Event::DmabufFormat { .. } => {
                self.constraints.begin_event();
            }
            Event::Done => {
                self.constraints.complete = true;
                match &mut self.state {
                    State::Done => {}
                    // The in-flight frame will fail or complete; reallocate after it.
                    State::InFlight { realloc, .. } => *realloc = true,
                    State::Starting | State::Idle => return self.apply_constraints(shm, qh),
                }
            }
            Event::Stopped => {
                let event = self.give_up("session stopped");
                if let Some(session) = self.session.take() {
                    session.destroy();
                }
                return event;
            }
            _ => {}
        }
        None
    }

    pub fn frame_event(
        &mut self,
        event: ext_image_copy_capture_frame_v1::Event,
        shm: &Shm,
        qh: &QueueHandle<App>,
    ) -> Option<CaptureEvent> {
        use ext_image_copy_capture_frame_v1::Event;
        match event {
            Event::Transform {
                transform: WEnum::Value(transform),
            } => self.transform = transform,
            Event::Ready => {
                let realloc = self.finish_frame()?;
                self.failures = 0;
                self.have_frame = true;
                let bufs = self.buffers.as_mut()?;
                let slot = bufs.back;
                bufs.back ^= 1;
                bufs.busy[slot] = true; // attached by the overlay
                let frame = Frame {
                    buffer: bufs.slots[slot].clone(),
                    width: bufs.width,
                    height: bufs.height,
                    transform: self.transform,
                };
                if self.live {
                    // Can only fail after the first frame, so nothing to report.
                    let _ = self.next_frame(realloc, shm, qh);
                } else {
                    self.state = State::Done;
                }
                return Some(CaptureEvent::Frame(frame));
            }
            Event::Failed { reason } => {
                let realloc = self.finish_frame()?;
                if reason == WEnum::Value(FailureReason::Stopped) {
                    return self.give_up("stopped");
                }
                self.failures += 1;
                if self.failures > MAX_FAILURES {
                    return self.give_up(match reason {
                        WEnum::Value(FailureReason::BufferConstraints) => "buffer constraints",
                        _ => "unknown",
                    });
                }
                // On buffer_constraints the session sends new constraints; if
                // they arrived already, this reallocates. Otherwise retry.
                return self.next_frame(realloc, shm, qh);
            }
            _ => {}
        }
        None
    }

    pub fn buffer_released(&mut self, slot: usize, qh: &QueueHandle<App>) {
        if let Some(bufs) = &mut self.buffers {
            bufs.busy[slot] = false;
        }
        self.request_frame(qh); // in case the capture was waiting for this buffer
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let State::InFlight { frame, .. } = &self.state {
            frame.destroy();
        }
        if let Some(session) = &self.session {
            session.destroy();
        }
        self.source.destroy();
    }
}

/// Two equally sized buffers in one shm pool.
struct Buffers {
    _pool: RawPool,
    slots: [WlBuffer; 2],
    busy: [bool; 2],
    /// The slot the next frame is captured into.
    back: usize,
    width: u32,
    height: u32,
    format: wl_shm::Format,
}

impl Buffers {
    fn new(
        shm: &Shm,
        window: WindowId,
        width: u32,
        height: u32,
        format: wl_shm::Format,
        qh: &QueueHandle<App>,
    ) -> std::io::Result<Self> {
        let too_large = || std::io::Error::other("buffer too large");
        let stride = width.checked_mul(4).ok_or_else(too_large)?;
        let size = stride.checked_mul(height).ok_or_else(too_large)?;
        let pool_size = size
            .checked_mul(2)
            .filter(|&s| s <= i32::MAX as u32)
            .ok_or_else(too_large)?;
        let mut pool = RawPool::new(pool_size as usize, shm).map_err(std::io::Error::other)?;
        let slots = [0, 1].map(|slot| {
            let offset = (size * slot as u32) as i32;
            let data = BufferData { window, slot };
            pool.create_buffer(
                offset,
                width as i32,
                height as i32,
                stride as i32,
                format,
                data,
                qh,
            )
        });
        Ok(Self {
            _pool: pool,
            slots,
            busy: [false; 2],
            back: 0,
            width,
            height,
            format,
        })
    }

    fn matches(&self, width: u32, height: u32, format: wl_shm::Format) -> bool {
        (self.width, self.height, self.format) == (width, height, format)
    }
}

impl Drop for Buffers {
    fn drop(&mut self) {
        // Destroying a buffer that is still attached is fine: the compositor
        // keeps showing its last contents until the next attach.
        for buffer in &self.slots {
            buffer.destroy();
        }
    }
}

// ---------------------------------------------------------------------------
// protocol glue: route events to the window's capture

pub struct SessionData(WindowId);
pub struct FrameData(WindowId);
pub struct BufferData {
    window: WindowId,
    slot: usize,
}

impl Dispatch2<ExtImageCopyCaptureSessionV1, App> for SessionData {
    fn event(
        &self,
        app: &mut App,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &Connection,
        qh: &QueueHandle<App>,
    ) {
        app.capture_update(self.0, |cap, shm| cap.session_event(event, shm, qh));
    }
}

impl Dispatch2<ExtImageCopyCaptureFrameV1, App> for FrameData {
    fn event(
        &self,
        app: &mut App,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &Connection,
        qh: &QueueHandle<App>,
    ) {
        app.capture_update(self.0, |cap, shm| cap.frame_event(event, shm, qh));
    }
}

impl Dispatch2<WlBuffer, App> for BufferData {
    fn event(
        &self,
        app: &mut App,
        _: &WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        _: &Connection,
        qh: &QueueHandle<App>,
    ) {
        // The only event is release.
        app.capture_update(self.window, |cap, _| {
            cap.buffer_released(self.slot, qh);
            None
        });
    }
}
