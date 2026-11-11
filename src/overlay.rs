//! Full-screen layer surface with one subsurface per window thumbnail.
//!
//! The background, highlight and placeholders are single-pixel buffers (or
//! 1x1 shm buffers if the compositor lacks wp_single_pixel_buffer_manager_v1)
//! scaled with viewports; thumbnails attach the capture buffers directly and
//! let the compositor scale them. With a background image, the image is the
//! main surface's buffer (cropped to cover it) and the background color is a
//! tint subsurface above it. All subsurfaces have an empty input region, so
//! pointer events arrive on the main surface and are hit-tested here.

use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use smithay_client_toolkit::{
    compositor::Region,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface},
    },
    shm::raw::RawPool,
};
use wayland_client::{
    QueueHandle,
    protocol::{
        wl_buffer::WlBuffer, wl_output::Transform, wl_shm, wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::{
    ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    wp::viewporter::client::wp_viewport::WpViewport,
};

use crate::app::App;
use crate::background::{self, Image};
use crate::capture::{Capture, CaptureEvent, Frame};
use crate::config::Config;
use crate::globals::{Globals, Ignore};
use crate::ipc::Request;
use crate::layout::{self, Dir};

/// Layout size of a window whose capture size is not known yet.
const DEFAULT_SIZE: (f64, f64) = (800.0, 500.0);

static NEXT_WINDOW_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId(u32);

/// A layout rect in surface-local logical pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64
            && x < (self.x + self.w) as f64
            && y >= self.y as f64
            && y < (self.y + self.h) as f64
    }
}

/// A desync subsurface scaled with a viewport, without input.
struct Sub {
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
}

impl Sub {
    fn new(g: &Globals, parent: &WlSurface, no_input: &Region, qh: &QueueHandle<App>) -> Self {
        let (subsurface, surface) = g.subcompositor.create_subsurface(parent.clone(), qh);
        subsurface.set_desync();
        let viewport = g.viewporter.get_viewport(&surface, qh, Ignore);
        surface.set_input_region(Some(no_input.wl_region()));
        Self {
            surface,
            subsurface,
            viewport,
        }
    }

    fn attach(&self, buffer: &WlBuffer) {
        self.surface.attach(Some(buffer), 0, 0);
        self.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
    }

    fn place(&self, r: IRect) {
        self.subsurface.set_position(r.x, r.y);
        self.viewport.set_destination(r.w, r.h);
    }
}

impl Drop for Sub {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
    }
}

/// What a thumbnail currently shows.
#[derive(Clone, Copy, PartialEq)]
struct Shown {
    width: u32,
    height: u32,
    transform: Transform,
}

fn transform_swaps(t: Transform) -> bool {
    u32::from(t) & 1 != 0 // 90 and 270, flipped or not
}

pub struct Window {
    pub id: WindowId,
    pub handle: ExtForeignToplevelHandleV1,
    pub identifier: String,
    capture: Capture,
    sub: Sub,
    pub rect: IRect,
    /// The capture frame attached to the thumbnail; None: the placeholder.
    shown: Option<Shown>,
}

impl Window {
    /// Logical size of the window's content, for the layout.
    fn size(&self, scale: i32) -> layout::Item {
        let (w, h, transform) = match (self.shown, self.capture.size()) {
            (Some(s), _) => (s.width, s.height, s.transform),
            (None, Some((w, h))) => (w, h, Transform::Normal),
            (None, None) => {
                return layout::Item {
                    w: DEFAULT_SIZE.0,
                    h: DEFAULT_SIZE.1,
                };
            }
        };
        let (w, h) = if transform_swaps(transform) {
            (h, w)
        } else {
            (w, h)
        };
        // Capture sizes are in buffer pixels; never show windows larger than
        // their real size on a scaled output.
        let scale = scale as f64;
        layout::Item {
            w: w as f64 / scale,
            h: h as f64 / scale,
        }
    }
}

struct Style {
    gap: f64,
    margin: f64,
    border: i32,
    max_scale: f64,
}

/// A background image in an shm buffer, shared by every overlay of a daemon.
pub struct Wallpaper {
    buffer: WlBuffer,
    width: u32,
    height: u32,
}

impl Wallpaper {
    pub fn new(g: &Globals, img: &Image, qh: &QueueHandle<App>) -> std::io::Result<Self> {
        let mut pool = RawPool::new(img.data.len(), &g.shm).map_err(std::io::Error::other)?;
        pool.mmap()[..img.data.len()].copy_from_slice(&img.data);
        let (w, h) = (img.width as i32, img.height as i32);
        // The buffer stays valid after the pool is destroyed (on drop).
        let buffer =
            pool.create_buffer(0, w, h, img.stride(), wl_shm::Format::Argb8888, Ignore, qh);
        Ok(Self {
            buffer,
            width: img.width,
            height: img.height,
        })
    }
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

pub struct Overlay {
    // Field order is drop order: children before their parent surface.
    pub windows: Vec<Window>,
    pub selected: Option<usize>,
    highlight_sub: Sub,
    /// The background color over the wallpaper; None without one.
    tint_sub: Option<Sub>,
    viewport: WpViewport,
    layer: LayerSurface,
    background: WlBuffer,
    wallpaper: Option<Rc<Wallpaper>>,
    highlight: WlBuffer,
    placeholder: WlBuffer,
    g: Rc<Globals>,
    style: Style,

    width: u32,
    height: u32,
    scale: i32,
    configured: bool,
    mapped: bool,
    /// The first-frame timeout expired: map even without all frames.
    deadline_passed: bool,
    /// With WL_EXPOSE_TRACE set: when the overlay was created.
    trace: Option<Instant>,
}

impl Overlay {
    /// Create the overlay for `toplevels` (handle and identifier each), in
    /// layout order, and start capturing them.
    pub fn new(
        g: Rc<Globals>,
        toplevels: Vec<(ExtForeignToplevelHandleV1, String)>,
        req: &Request,
        config: &Config,
        wallpaper: Option<Rc<Wallpaper>>,
        qh: &QueueHandle<App>,
    ) -> std::io::Result<Self> {
        let background = color_buffer(&g, config.background, qh)?;
        let highlight = color_buffer(&g, config.highlight, qh)?;
        let placeholder = color_buffer(&g, config.placeholder, qh)?;
        // Copied by set_input_region, so it is destroyed at the end of new().
        let no_input = Region::new(&g.compositor).map_err(std::io::Error::other)?;

        let surface = g.compositor.create_surface(qh);
        let viewport = g.viewporter.get_viewport(&surface, qh, Ignore);
        // Output None: the compositor picks the focused output.
        let layer = g.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("wl-expose"),
            None,
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);

        // Created before the thumbnails, so they start below all of them,
        // the tint lowest.
        let tint_sub = (wallpaper.is_some() && config.background & 0xff != 0).then(|| {
            let sub = Sub::new(&g, layer.wl_surface(), &no_input, qh);
            sub.attach(&background);
            sub
        });
        let highlight_sub = Sub::new(&g, layer.wl_surface(), &no_input, qh);

        let windows = toplevels
            .into_iter()
            .map(|(handle, identifier)| {
                // Unique across overlays, so late events for a previous
                // overlay's captures never reach this one's.
                let id = WindowId(NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed));
                Window {
                    id,
                    capture: Capture::new(id, &handle, &g, req.live, req.cursors, qh),
                    sub: Sub::new(&g, layer.wl_surface(), &no_input, qh),
                    handle,
                    identifier,
                    rect: IRect::default(),
                    shown: None,
                }
            })
            .collect();

        // Initial commit without a buffer; the map happens once frames are in.
        layer.commit();

        Ok(Self {
            windows,
            selected: None,
            highlight_sub,
            tint_sub,
            viewport,
            layer,
            background,
            wallpaper,
            highlight,
            placeholder,
            g,
            style: Style {
                gap: config.gap as f64,
                margin: config.margin as f64,
                border: config.border as i32,
                max_scale: config.max_scale,
            },
            width: 0,
            height: 0,
            scale: 1,
            configured: false,
            mapped: false,
            deadline_passed: false,
            trace: std::env::var_os("WL_EXPOSE_TRACE").map(|_| Instant::now()),
        })
    }

    pub fn surface(&self) -> &WlSurface {
        self.layer.wl_surface()
    }

    pub fn mapped(&self) -> bool {
        self.mapped
    }

    pub fn index_of(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    /// Subsurface positions and stacking are applied on the parent commit.
    /// Before mapping, the map commit takes care of that.
    fn commit_parent(&self) {
        if self.mapped {
            self.layer.commit();
        }
    }

    pub fn configure(&mut self, width: u32, height: u32) {
        let changed = !self.configured || (width, height) != (self.width, self.height);
        if let Some(t0) = self.trace
            && !self.configured
        {
            trace(t0, format_args!("configured {width}x{height}"));
        }
        self.configured = true;
        self.width = width;
        self.height = height;
        if width > 0 && height > 0 {
            self.viewport.set_destination(width as i32, height as i32);
            if let Some(wp) = &self.wallpaper {
                let (x, y, w, h) = background::cover_source(wp.width, wp.height, width, height);
                self.viewport.set_source(x, y, w, h);
            }
            if let Some(tint) = &self.tint_sub {
                tint.place(IRect {
                    x: 0,
                    y: 0,
                    w: width as i32,
                    h: height as i32,
                });
                tint.surface.commit();
            }
        }
        if !self.mapped {
            return;
        }
        if changed {
            self.relayout();
        } else {
            self.layer.commit();
        }
    }

    pub fn set_scale(&mut self, scale: i32) {
        if scale < 1 || scale == self.scale {
            return;
        }
        self.scale = scale;
        if self.mapped {
            self.relayout();
        }
    }

    pub fn relayout(&mut self) {
        if !self.configured || self.width == 0 || self.height == 0 || self.windows.is_empty() {
            self.commit_parent();
            return;
        }
        let items: Vec<layout::Item> = self.windows.iter().map(|w| w.size(self.scale)).collect();
        let (width, height) = (self.width as f64, self.height as f64);
        let m = self.style.margin;
        let mut area = layout::Rect::new(m, m, width - 2.0 * m, height - 2.0 * m);
        if area.w <= 0.0 || area.h <= 0.0 {
            area = layout::Rect::new(0.0, 0.0, width, height);
        }
        let params = layout::Params {
            spacing: self.style.gap,
            max_scale: self.style.max_scale,
        };
        let rects = layout::compute(&items, area, &params);

        for (win, r) in self.windows.iter_mut().zip(rects) {
            // Round edges rather than sizes so neighbors never overlap.
            let (x0, y0) = (r.x.round() as i32, r.y.round() as i32);
            let (x1, y1) = ((r.x + r.w).round() as i32, (r.y + r.h).round() as i32);
            win.rect = IRect {
                x: x0,
                y: y0,
                w: (x1 - x0).max(1),
                h: (y1 - y0).max(1),
            };
            win.sub.place(win.rect);
            if win.shown.is_none() {
                win.sub.attach(&self.placeholder);
            }
            win.sub.surface.commit();
        }
        self.update_highlight(); // commits the parent
    }

    /// Run `f` on a window's capture and apply what it reports.
    pub fn capture_update(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut Capture, &Globals) -> Option<CaptureEvent>,
    ) {
        let Some(i) = self.index_of(id) else { return };
        let had_size = self.windows[i].capture.size().is_some();
        let had_frame = self.windows[i].shown.is_some();
        let event = f(&mut self.windows[i].capture, &self.g);
        if let Some(t0) = self.trace {
            let win = &self.windows[i];
            if !had_size && let Some((w, h)) = win.capture.size() {
                trace(t0, format_args!("'{}': size {w}x{h}", win.identifier));
            }
            if !had_frame && matches!(event, Some(CaptureEvent::Frame(_))) {
                let kind = win.capture.buffer_kind().unwrap_or("?");
                trace(
                    t0,
                    format_args!("'{}': first frame ({kind})", win.identifier),
                );
            }
        }
        match event {
            Some(CaptureEvent::Frame(frame)) => self.show_frame(i, frame),
            Some(CaptureEvent::Failed(why)) => {
                let win = &self.windows[i];
                eprintln!("wl-expose: capture of '{}' failed: {why}", win.identifier);
                // Without configure, the placeholder is attached on the next relayout.
                if win.shown.is_none() && self.configured {
                    win.sub.attach(&self.placeholder);
                    win.sub.surface.commit();
                }
            }
            None => {}
        }
    }

    fn show_frame(&mut self, i: usize, frame: Frame) {
        let scale = self.scale;
        let win = &mut self.windows[i];
        // The layout used the announced capture size until now; only a
        // different size (or a rotation) moves anything.
        let laid_out = win.size(scale);
        win.shown = Some(Shown {
            width: frame.width,
            height: frame.height,
            transform: frame.transform,
        });
        let resized = win.size(scale) != laid_out;
        win.sub.surface.set_buffer_transform(frame.transform);
        win.sub.attach(&frame.buffer);
        if resized && self.mapped {
            self.relayout(); // commits every thumbnail
        } else {
            win.sub.surface.commit();
        }
    }

    pub fn update_highlight(&self) {
        let b = self.style.border;
        let sub = &self.highlight_sub;
        match self.selected.map(|i| &self.windows[i]) {
            Some(sel) if sel.rect.w > 0 && sel.rect.h > 0 && b > 0 => {
                let r = sel.rect;
                sub.place(IRect {
                    x: r.x - b,
                    y: r.y - b,
                    w: r.w + 2 * b,
                    h: r.h + 2 * b,
                });
                sub.subsurface.place_below(&sel.sub.surface);
                sub.attach(&self.highlight);
            }
            _ => sub.surface.attach(None, 0, 0),
        }
        sub.surface.commit();
        self.commit_parent();
    }

    /// The first-frame timeout expired.
    pub fn deadline_passed(&mut self) {
        self.deadline_passed = true;
        self.maybe_map();
    }

    /// Map once configured and every capture knows its size (frames fill in
    /// as they arrive), or the deadline passed.
    pub fn maybe_map(&mut self) {
        if self.mapped || !self.configured {
            return;
        }
        if !self.deadline_passed && !self.windows.iter().all(|w| w.capture.sized()) {
            return;
        }
        self.mapped = true;
        if let Some(t0) = self.trace {
            let why = if self.deadline_passed {
                "deadline"
            } else {
                "all sized"
            };
            trace(t0, format_args!("mapped ({why})"));
        }
        let surface = self.layer.wl_surface();
        let buffer = match &self.wallpaper {
            Some(wp) => &wp.buffer,
            None => &self.background,
        };
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        self.relayout(); // commits the parent, mapping it
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<usize> {
        if !self.mapped {
            return None;
        }
        self.windows.iter().position(|w| w.rect.contains(x, y))
    }

    pub fn select(&mut self, i: usize) {
        if i < self.windows.len() && self.selected != Some(i) {
            self.selected = Some(i);
            self.update_highlight();
        }
    }

    /// Move to the nearest window in `dir`.
    pub fn move_selection(&mut self, dir: Dir) {
        let Some(cur) = self.selected else {
            self.select(0);
            return;
        };
        let rects: Vec<layout::Rect> = self
            .windows
            .iter()
            .map(|w| {
                let r = w.rect;
                layout::Rect::new(r.x as f64, r.y as f64, r.w as f64, r.h as f64)
            })
            .collect();
        if let Some(next) = layout::neighbor(&rects, cur, dir) {
            self.select(next);
        }
    }

    /// Cycle in input order.
    pub fn cycle(&mut self, step: isize) {
        let n = self.windows.len();
        if n == 0 {
            return;
        }
        let next = match self.selected {
            Some(cur) => (cur as isize + step).rem_euclid(n as isize) as usize,
            None if step > 0 => 0,
            None => n - 1,
        };
        self.select(next);
    }

    /// Remove a closed window. The selection moves to its successor.
    pub fn remove(&mut self, i: usize) {
        self.windows.remove(i);
        let n = self.windows.len();
        self.selected = match self.selected {
            _ if n == 0 => None,
            Some(s) if s > i => Some(s - 1),
            Some(s) if s == i => Some(i.min(n - 1)),
            other => other,
        };
        if n > 0 {
            self.relayout();
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.background.destroy();
        self.highlight.destroy();
        self.placeholder.destroy();
    }
}

fn trace(t0: Instant, what: std::fmt::Arguments) {
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("wl-expose: trace {ms:7.1} ms  {what}");
}

/// A 1x1 buffer of an 0xRRGGBBAA color, for scaling with a viewport.
fn color_buffer(g: &Globals, rgba: u32, qh: &QueueHandle<App>) -> std::io::Result<WlBuffer> {
    // Both buffer kinds take premultiplied alpha.
    let [r, gr, b, a] = rgba.to_be_bytes().map(|c| c as f64 / 255.0);
    let [r, gr, b] = [r, gr, b].map(|c| c * a);
    if let Some(single_pixel) = &g.single_pixel {
        let c = |v: f64| (v * u32::MAX as f64) as u32;
        return Ok(single_pixel.create_u32_rgba_buffer(c(r), c(gr), c(b), c(a), qh, Ignore));
    }
    let mut pool = RawPool::new(4, &g.shm).map_err(std::io::Error::other)?;
    let c = |v: f64| (v * 255.0).round() as u32;
    let pixel = c(a) << 24 | c(r) << 16 | c(gr) << 8 | c(b);
    pool.mmap()[..4].copy_from_slice(&pixel.to_le_bytes());
    Ok(pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, Ignore, qh))
}
