//! Full-screen layer surface with one subsurface per window thumbnail.
//!
//! The background, highlight and placeholders are single-pixel buffers (or
//! 1x1 shm buffers if the compositor lacks wp_single_pixel_buffer_manager_v1)
//! scaled with viewports; thumbnails attach the capture buffers directly and
//! let the compositor scale them. With a background image, the image is the
//! main surface's buffer (cropped to cover it) and the background color is a
//! tint subsurface above it. Shadows are up to four shm strips around each
//! thumbnail, below every thumbnail. App icons (shm, one buffer per app)
//! sit on the thumbnails' bottom edges, above every thumbnail, and the
//! selected window's title pill is above everything. Icons and pills are
//! rendered by the worker thread and attached when they arrive. All
//! subsurfaces have an empty input region, so pointer events arrive on the
//! main surface and are hit-tested here.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Instant;

use smithay_client_toolkit::{
    compositor::Region,
    dispatch2::Dispatch2,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface},
    },
    shm::raw::RawPool,
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
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
use crate::icons;
use crate::ipc::Request;
use crate::layout::{self, Dir};
use crate::shadow;
use crate::text;
use crate::toplevel::Toplevel;
use crate::worker::{Assets, PillRequest};

/// Layout size of a window whose capture size is not known yet.
const DEFAULT_SIZE: (f64, f64) = (800.0, 500.0);

/// Space between the icon and the title pill, logical pixels.
const PILL_GAP: i32 = 6;

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

/// A window's shadow: top, bottom, left and right strips.
struct Shadow {
    strips: [Sub; 4],
    /// The attached buffers.
    buffers: [Option<WlBuffer>; 4],
    /// Strip rects relative to the thumbnail.
    rects: [Option<shadow::Rect>; 4],
    /// Thumbnail size and scale the buffers were rendered for.
    rendered: Option<(i32, i32, i32)>,
}

impl Shadow {
    fn new(g: &Globals, parent: &WlSurface, no_input: &Region, qh: &QueueHandle<App>) -> Self {
        Self {
            strips: std::array::from_fn(|_| Sub::new(g, parent, no_input, qh)),
            buffers: Default::default(),
            rects: [None; 4],
            rendered: None,
        }
    }

    /// Follow the thumbnail at `r`; re-render only if its size or the scale
    /// changed.
    fn update(
        &mut self,
        g: &Globals,
        settings: &shadow::Settings,
        r: IRect,
        scale: i32,
        qh: &QueueHandle<App>,
    ) {
        let key = (r.w, r.h, scale);
        if self.rendered != Some(key) {
            self.rendered = Some(key);
            let strips = shadow::render(r.w, r.h, settings, scale);
            for (k, strip) in strips.into_iter().enumerate() {
                if let Some(old) = self.buffers[k].take() {
                    retire(&old);
                }
                let buffer = strip.as_ref().and_then(|s| match strip_buffer(g, s, qh) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        eprintln!("wl-expose: shadow buffer: {e}");
                        None
                    }
                });
                let sub = &self.strips[k];
                match &buffer {
                    Some(b) => sub.attach(b),
                    None => sub.surface.attach(None, 0, 0),
                }
                self.rects[k] = buffer.is_some().then(|| strip.unwrap().rect);
                self.buffers[k] = buffer;
            }
        }
        for (sub, rect) in self.strips.iter().zip(self.rects) {
            if let Some(s) = rect {
                sub.place(IRect {
                    x: r.x + s.x,
                    y: r.y + s.y,
                    w: s.w,
                    h: s.h,
                });
            }
            sub.surface.commit();
        }
    }
}

impl Drop for Shadow {
    fn drop(&mut self) {
        // Like capture buffers, attached ones may be destroyed right away.
        for buffer in self.buffers.iter().flatten() {
            buffer.destroy();
        }
    }
}

/// An shm buffer holding premultiplied ARGB8888 `pixels`. The pool is gone
/// once this returns; the buffer keeps the memory alive.
fn shm_buffer<U>(
    g: &Globals,
    width: u32,
    height: u32,
    pixels: &[u8],
    data: U,
    qh: &QueueHandle<App>,
) -> std::io::Result<WlBuffer>
where
    App: wayland_client::Dispatch<WlBuffer, U>,
    U: Send + Sync + 'static,
{
    let len = pixels.len();
    if len > i32::MAX as usize || len != width as usize * height as usize * 4 || len == 0 {
        return Err(std::io::Error::other("bad buffer size"));
    }
    let mut pool = RawPool::new(len, &g.shm).map_err(std::io::Error::other)?;
    pool.mmap()[..len].copy_from_slice(pixels);
    Ok(pool.create_buffer(
        0,
        width as i32,
        height as i32,
        width as i32 * 4,
        wl_shm::Format::Argb8888,
        data,
        qh,
    ))
}

fn strip_buffer(
    g: &Globals,
    strip: &shadow::Strip,
    qh: &QueueHandle<App>,
) -> std::io::Result<WlBuffer> {
    let data = OnceBuffer::default();
    shm_buffer(g, strip.width, strip.height, &strip.pixels, data, qh)
}

/// User data of a buffer that is attached once and replaced later (shadow
/// strips, title pills): it is destroyed once it is both replaced (retired)
/// and released by the compositor, whichever comes last.
#[derive(Default)]
struct OnceBuffer(AtomicU8);

const RELEASED: u8 = 1;
const RETIRED: u8 = 2;

fn retire(buffer: &WlBuffer) {
    if let Some(data) = buffer.data::<OnceBuffer>()
        && data.0.fetch_or(RETIRED, Ordering::Relaxed) & RELEASED != 0
    {
        buffer.destroy();
    }
}

impl Dispatch2<WlBuffer, App> for OnceBuffer {
    fn event(
        &self,
        _: &mut App,
        buffer: &WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        // The only event is release. Each buffer is attached exactly once.
        if self.0.fetch_or(RELEASED, Ordering::Relaxed) & RETIRED != 0 {
            buffer.destroy();
        }
    }
}

pub struct Window {
    pub id: WindowId,
    pub handle: ExtForeignToplevelHandleV1,
    pub identifier: String,
    app_id: String,
    title: String,
    capture: Capture,
    sub: Sub,
    /// None with shadows disabled.
    shadow: Option<Shadow>,
    /// None with icons disabled.
    icon: Option<Icon>,
    pub rect: IRect,
    /// The capture frame attached to the thumbnail; None: the placeholder.
    shown: Option<Shown>,
}

/// A window's app icon, centered on the thumbnail's bottom edge.
struct Icon {
    sub: Sub,
    rect: IRect,
    /// The icon attached, if any.
    attached: Option<icons::Key>,
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
    /// Highlight corner radius; 0: the 1x1 color buffer is scaled instead.
    border_radius: i32,
    highlight_color: u32,
    max_scale: f64,
    shadow: Option<shadow::Settings>,
    /// Logical pixels; 0: no icons.
    icon_size: i32,
    icon_theme: Option<String>,
    /// For letter icons.
    font: String,
    /// None: no title pill.
    title: Option<text::Style>,
}

impl Style {
    /// How far the icon reaches below the thumbnail: the lower half.
    fn icon_below(&self) -> i32 {
        self.icon_size - self.icon_size / 2
    }

    /// How far below the thumbnail the title pill starts.
    fn pill_offset(&self) -> i32 {
        self.icon_below() + PILL_GAP
    }

    /// Space the layout keeps free below each thumbnail: the lower half of
    /// the icon, then the title pill.
    fn below(&self) -> f64 {
        match &self.title {
            Some(t) => (self.pill_offset() + text::pill_height(t.size)) as f64,
            None => self.icon_below() as f64,
        }
    }
}

/// The title pill on screen: what it shows and where.
struct ShownPill {
    window: WindowId,
    request: PillRequest,
    buffer: WlBuffer,
    width: i32,
    height: i32,
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
    /// The selected window's title; None with titles disabled.
    pill_sub: Option<Sub>,
    highlight_sub: Sub,
    /// The background color over the wallpaper; None without one.
    tint_sub: Option<Sub>,
    viewport: WpViewport,
    layer: LayerSurface,
    background: WlBuffer,
    wallpaper: Option<Rc<Wallpaper>>,
    highlight: WlBuffer,
    /// With a border radius: the attached highlight, and the size and scale
    /// it was rendered for.
    rounded_highlight: Option<(WlBuffer, (i32, i32, i32))>,
    placeholder: WlBuffer,
    /// Icon buffers, shared by the windows of an app.
    icon_buffers: HashMap<icons::Key, WlBuffer>,
    /// Replaced icon buffers, possibly still attached.
    stale_buffers: Vec<WlBuffer>,
    /// The pill attached to `pill_sub`.
    pill: Option<ShownPill>,
    /// The pill requested from the worker, until it arrives.
    pill_wanted: Option<(WindowId, PillRequest)>,
    assets: Rc<Assets>,
    g: Rc<Globals>,
    qh: QueueHandle<App>,
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
    /// Create the overlay for `toplevels`, in layout order, and start
    /// capturing them. Icons and titles come from `assets`.
    pub fn new(
        g: Rc<Globals>,
        toplevels: Vec<(ExtForeignToplevelHandleV1, Toplevel)>,
        req: &Request,
        config: &Config,
        wallpaper: Option<Rc<Wallpaper>>,
        assets: Rc<Assets>,
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

        // Created before the thumbnails, so they start below all of them:
        // the tint lowest, then every shadow, then the highlight.
        let tint_sub = (wallpaper.is_some() && config.background & 0xff != 0).then(|| {
            let sub = Sub::new(&g, layer.wl_surface(), &no_input, qh);
            sub.attach(&background);
            sub
        });
        let shadow = config.shadow();
        let shadows: Vec<Option<Shadow>> = toplevels
            .iter()
            .map(|_| {
                shadow
                    .is_some()
                    .then(|| Shadow::new(&g, layer.wl_surface(), &no_input, qh))
            })
            .collect();
        let highlight_sub = Sub::new(&g, layer.wl_surface(), &no_input, qh);

        let mut windows: Vec<Window> = toplevels
            .into_iter()
            .zip(shadows)
            .map(|((handle, toplevel), shadow)| {
                // Unique across overlays, so late events for a previous
                // overlay's captures never reach this one's.
                let id = WindowId(NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed));
                Window {
                    id,
                    capture: Capture::new(id, &handle, &g, req.live, req.cursors, qh),
                    sub: Sub::new(&g, layer.wl_surface(), &no_input, qh),
                    shadow,
                    icon: None,
                    handle,
                    identifier: toplevel.identifier,
                    app_id: toplevel.app_id,
                    title: toplevel.title,
                    rect: IRect::default(),
                    shown: None,
                }
            })
            .collect();
        // Above every thumbnail, then the pill above everything.
        if config.icon_size > 0 {
            for win in &mut windows {
                win.icon = Some(Icon {
                    sub: Sub::new(&g, layer.wl_surface(), &no_input, qh),
                    rect: IRect::default(),
                    attached: None,
                });
            }
        }
        let title = config.title_style();
        let pill_sub = title
            .is_some()
            .then(|| Sub::new(&g, layer.wl_surface(), &no_input, qh));

        // Initial commit without a buffer; the map happens once frames are in.
        layer.commit();

        Ok(Self {
            windows,
            selected: None,
            pill_sub,
            highlight_sub,
            tint_sub,
            viewport,
            layer,
            background,
            wallpaper,
            highlight,
            rounded_highlight: None,
            placeholder,
            icon_buffers: HashMap::new(),
            stale_buffers: Vec::new(),
            pill: None,
            pill_wanted: None,
            assets,
            g,
            qh: qh.clone(),
            style: Style {
                gap: config.gap as f64,
                margin: config.margin as f64,
                border: config.border as i32,
                border_radius: config.border_radius as i32,
                highlight_color: config.highlight,
                max_scale: config.max_scale,
                shadow,
                icon_size: config.icon_size as i32,
                icon_theme: config.icon_theme.clone(),
                font: config.font.clone(),
                title,
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
            below: self.style.below(),
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
            if let (Some(sh), Some(settings)) = (&mut win.shadow, &self.style.shadow) {
                sh.update(&self.g, settings, win.rect, self.scale, &self.qh);
            }
            if let Some(icon) = &mut win.icon {
                let (r, s) = (win.rect, self.style.icon_size);
                icon.rect = IRect {
                    x: r.x + (r.w - s) / 2,
                    y: r.y + r.h - s / 2,
                    w: s,
                    h: s,
                };
                icon.sub.place(icon.rect);
            }
        }
        self.update_icons(); // commits every icon
        self.update_highlight(); // commits the parent
    }

    /// The icon key of `app_id` at the current scale.
    fn icon_key(&self, app_id: &str) -> icons::Key {
        icons::Key {
            app_id: app_id.to_string(),
            size: (self.style.icon_size * self.scale) as u32,
            theme: self.style.icon_theme.clone(),
        }
    }

    /// The icon buffer for `key`, made from the received image. None if the
    /// image isn't there yet; it is requested then.
    fn icon_buffer(&mut self, key: &icons::Key) -> Option<WlBuffer> {
        if let Some(buffer) = self.icon_buffers.get(key) {
            return Some(buffer.clone());
        }
        let img = self.assets.icon(key, &self.style.font)?;
        match shm_buffer(&self.g, img.width, img.height, &img.data, Ignore, &self.qh) {
            Ok(buffer) => {
                self.icon_buffers.insert(key.clone(), buffer.clone());
                Some(buffer)
            }
            Err(e) => {
                eprintln!("wl-expose: icon buffer: {e}");
                None
            }
        }
    }

    /// Attach every window's current icon that is available, request the
    /// others, and commit the icons.
    fn update_icons(&mut self) {
        for i in 0..self.windows.len() {
            let Some(icon) = &self.windows[i].icon else {
                continue;
            };
            let key = self.icon_key(&self.windows[i].app_id);
            if icon.attached.as_ref() != Some(&key)
                && let Some(buffer) = self.icon_buffer(&key)
                && let Some(icon) = &mut self.windows[i].icon
            {
                icon.sub.attach(&buffer);
                icon.attached = Some(key);
            }
            if let Some(icon) = &self.windows[i].icon {
                icon.sub.surface.commit();
            }
        }
    }

    /// An icon arrived from the worker.
    pub fn icon_ready(&mut self, key: &icons::Key) {
        let wanted = self
            .windows
            .iter()
            .any(|w| w.icon.is_some() && self.icon_key(&w.app_id) == *key);
        // Before mapping, the map's relayout attaches it.
        if wanted && self.mapped {
            self.update_icons();
        }
    }

    /// The desktop entries changed: get every icon again.
    pub fn icons_reloaded(&mut self) {
        // Attached buffers may be in use; keep them until the overlay goes.
        self.stale_buffers
            .extend(self.icon_buffers.drain().map(|(_, buffer)| buffer));
        for icon in self.windows.iter_mut().filter_map(|w| w.icon.as_mut()) {
            icon.attached = None;
        }
        if self.mapped {
            self.update_icons();
        }
    }

    /// The title or app_id of a shown window changed.
    pub fn update_toplevel(&mut self, handle: &ExtForeignToplevelHandleV1, toplevel: &Toplevel) {
        let Some(i) = self.windows.iter().position(|w| w.handle == *handle) else {
            return;
        };
        let win = &mut self.windows[i];
        let app_changed = win.app_id != toplevel.app_id;
        let title_changed = win.title != toplevel.title;
        win.app_id.clone_from(&toplevel.app_id);
        win.title.clone_from(&toplevel.title);
        if app_changed && self.mapped {
            self.update_icons();
        }
        if title_changed && self.selected == Some(i) {
            self.update_pill();
            self.commit_parent();
        }
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

    /// Move the highlight and the title pill to the selection.
    pub fn update_highlight(&mut self) {
        let b = self.style.border;
        let target = (self.selected.map(|i| &self.windows[i]))
            .filter(|w| w.rect.w > 0 && w.rect.h > 0 && b > 0)
            .map(|w| (w.rect, w.sub.surface.clone()));
        match target {
            Some((r, above)) => {
                let rect = IRect {
                    x: r.x - b,
                    y: r.y - b,
                    w: r.w + 2 * b,
                    h: r.h + 2 * b,
                };
                if self.style.border_radius > 0 {
                    self.attach_rounded_highlight(rect.w, rect.h);
                } else {
                    self.highlight_sub.attach(&self.highlight);
                }
                let sub = &self.highlight_sub;
                sub.place(rect);
                sub.subsurface.place_below(&above);
            }
            None => {
                self.highlight_sub.surface.attach(None, 0, 0);
                if let Some((old, _)) = self.rounded_highlight.take() {
                    retire(&old);
                }
            }
        }
        self.highlight_sub.surface.commit();
        self.update_pill();
        self.commit_parent();
    }

    /// Attach a rounded highlight of `w`x`h` logical pixels, unless the
    /// attached one already is. Each buffer is attached once (OnceBuffer).
    fn attach_rounded_highlight(&mut self, w: i32, h: i32) {
        let s = self.scale;
        let key = (w, h, s);
        if self.rounded_highlight.as_ref().is_some_and(|(_, k)| *k == key) {
            return;
        }
        let r = (self.style.border_radius * s) as f64;
        let img = text::rounded_rect(w * s, h * s, r, self.style.highlight_color);
        let data = OnceBuffer::default();
        match shm_buffer(&self.g, img.width, img.height, &img.data, data, &self.qh) {
            Ok(buffer) => {
                self.highlight_sub.attach(&buffer);
                if let Some((old, _)) = self.rounded_highlight.replace((buffer, key)) {
                    retire(&old);
                }
            }
            Err(e) => {
                eprintln!("wl-expose: highlight buffer: {e}");
                // Square rather than nothing.
                self.highlight_sub.attach(&self.highlight);
                if let Some((old, _)) = self.rounded_highlight.take() {
                    retire(&old);
                }
            }
        }
    }

    /// The selected window, once laid out.
    fn selected_window(&self) -> Option<&Window> {
        let win = self.windows.get(self.selected?)?;
        (win.rect.w > 0 && win.rect.h > 0).then_some(win)
    }

    /// Show the selected window's title: keep, move or replace the pill, or
    /// request a new one from the worker. The parent commit is the caller's.
    fn update_pill(&mut self) {
        let (Some(sub), Some(style)) = (&self.pill_sub, &self.style.title) else {
            return;
        };
        let wanted = self
            .selected_window()
            .filter(|w| !w.title.trim().is_empty())
            .map(|w| {
                let max_width = w.rect.w.max(text::MIN_PILL_WIDTH).min(self.width as i32);
                let request = PillRequest {
                    title: w.title.clone(),
                    max_width,
                    scale: self.scale,
                    style: style.clone(),
                };
                (w.id, request)
            });
        // A pill of another window goes right away; an outdated one of the
        // same window stays until its replacement arrives.
        let keep = matches!((&self.pill, &wanted), (Some(p), Some((id, _))) if p.window == *id);
        if !keep && let Some(old) = self.pill.take() {
            retire(&old.buffer);
            sub.surface.attach(None, 0, 0);
            sub.surface.commit();
        }
        let current = self.pill.as_ref().map(|p| (p.window, &p.request));
        match wanted {
            Some((id, request)) if current != Some((id, &request)) => {
                if self.pill_wanted.as_ref() != Some(&(id, request.clone())) {
                    self.assets.pill(id, request.clone());
                    self.pill_wanted = Some((id, request));
                }
            }
            _ => self.pill_wanted = None,
        }
        self.place_pill();
    }

    /// Put the pill below its window's icon, centered and inside the output.
    fn place_pill(&self) {
        let (Some(sub), Some(pill)) = (&self.pill_sub, &self.pill) else {
            return;
        };
        let Some(win) = self.windows.iter().find(|w| w.id == pill.window) else {
            return;
        };
        let r = win.rect;
        let max_x = (self.width as i32 - pill.width).max(0);
        sub.place(IRect {
            x: (r.x + (r.w - pill.width) / 2).clamp(0, max_x),
            y: r.y + r.h + self.style.pill_offset(),
            w: pill.width,
            h: pill.height,
        });
        sub.surface.commit();
    }

    /// A pill arrived from the worker.
    pub fn pill_ready(&mut self, window: WindowId, request: PillRequest, pill: text::Pill) {
        if self.pill_wanted.as_ref() != Some(&(window, request.clone())) {
            return; // the selection or title changed meanwhile
        }
        self.pill_wanted = None;
        let Some(sub) = &self.pill_sub else { return };
        let img = &pill.image;
        let buffer = match shm_buffer(
            &self.g,
            img.width,
            img.height,
            &img.data,
            OnceBuffer::default(),
            &self.qh,
        ) {
            Ok(buffer) => buffer,
            Err(e) => {
                eprintln!("wl-expose: title buffer: {e}");
                return;
            }
        };
        sub.attach(&buffer);
        if let Some(old) = self.pill.take() {
            retire(&old.buffer);
        }
        self.pill = Some(ShownPill {
            window,
            request,
            buffer,
            width: pill.width,
            height: pill.height,
        });
        self.place_pill();
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

    /// The window whose thumbnail or icon is at (`x`, `y`).
    pub fn hit(&self, x: f64, y: f64) -> Option<usize> {
        if !self.mapped {
            return None;
        }
        // Icons are on top, so they win where they cover a thumbnail.
        let on_icon = |w: &Window| w.icon.as_ref().is_some_and(|i| i.rect.contains(x, y));
        (self.windows.iter().position(on_icon))
            .or_else(|| self.windows.iter().position(|w| w.rect.contains(x, y)))
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
        if let Some((buffer, _)) = &self.rounded_highlight {
            buffer.destroy();
        }
        self.placeholder.destroy();
        // Like capture buffers, attached ones may be destroyed right away.
        for buffer in self.icon_buffers.values().chain(&self.stale_buffers) {
            buffer.destroy();
        }
        if let Some(pill) = &self.pill {
            pill.buffer.destroy();
        }
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
