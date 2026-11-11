//! Drop shadows for thumbnails, rendered on the CPU.
//!
//! The same closed-form Gaussian box shadow niri and umbriel evaluate in
//! their shaders (<https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/>,
//! CC0), with CSS `box-shadow` semantics: the shadow box is the thumbnail
//! grown by `spread` and moved by `offset`, blurred with sigma = softness / 2.
//! Thumbnails have square corners, so the shadow is separable into a column
//! and a row profile.
//!
//! The shadow is not drawn behind the thumbnail. What is left is split into
//! up to four strips (top and bottom full width, left and right between
//! them), so memory grows with the perimeter rather than the area.

/// Shadow settings, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// 0xRRGGBBAA, not premultiplied.
    pub color: u32,
    /// Blur radius; sigma = softness / 2.
    pub softness: f64,
    pub spread: i32,
    pub offset: (i32, i32),
}

impl Settings {
    fn sigma(&self) -> f64 {
        self.softness.max(0.0) / 2.0
    }

    /// How far the shadow is drawn beyond its box.
    fn extent(&self) -> i32 {
        (3.0 * self.sigma()).ceil() as i32
    }
}

/// A rect in logical pixels, relative to the thumbnail's top left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn from_edges(x0: i32, y0: i32, x1: i32, y1: i32) -> Option<Self> {
        (x1 > x0 && y1 > y0).then_some(Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// One strip of a shadow: where it goes and its pixels.
pub struct Strip {
    pub rect: Rect,
    /// Buffer size: the rect times the scale.
    pub width: u32,
    pub height: u32,
    /// Premultiplied ARGB8888 (little-endian, so B G R A bytes), packed rows.
    pub pixels: Vec<u8>,
}

impl Strip {
    pub fn stride(&self) -> i32 {
        self.width as i32 * 4
    }
}

/// The shadow box: the thumbnail grown by `spread` and moved by `offset`, as
/// edges. None if a negative spread leaves nothing.
fn shadow_box(w: i32, h: i32, s: &Settings) -> Option<[i32; 4]> {
    let (ox, oy) = s.offset;
    let b = [
        -s.spread + ox,
        -s.spread + oy,
        w + s.spread + ox,
        h + s.spread + oy,
    ];
    (b[2] > b[0] && b[3] > b[1]).then_some(b)
}

/// The strips covering the shadow box grown by its extent, minus the
/// `w`x`h` thumbnail: top, bottom, left, right; None where empty.
pub fn strip_rects(w: i32, h: i32, s: &Settings) -> [Option<Rect>; 4] {
    let Some([sx0, sy0, sx1, sy1]) = shadow_box(w, h, s) else {
        return [None; 4];
    };
    let e = s.extent();
    let (x0, y0, x1, y1) = (sx0 - e, sy0 - e, sx1 + e, sy1 + e);
    // The thumbnail is (0, 0) to (w, h); clamp its edges into the region.
    let (ty0, ty1) = (0.clamp(y0, y1), h.clamp(y0, y1));
    let (tx0, tx1) = (0.clamp(x0, x1), w.clamp(x0, x1));
    [
        Rect::from_edges(x0, y0, x1, ty0),
        Rect::from_edges(x0, ty1, x1, y1),
        Rect::from_edges(x0, ty0, tx0, ty1),
        Rect::from_edges(tx1, ty0, x1, ty1),
    ]
}

/// The shadow of a `w`x`h` thumbnail at buffer `scale`.
pub fn render(w: i32, h: i32, s: &Settings, scale: i32) -> [Option<Strip>; 4] {
    let rects = strip_rects(w, h, s);
    let Some([sx0, sy0, sx1, sy1]) = shadow_box(w, h, s) else {
        return [None, None, None, None];
    };
    let sigma = s.sigma();
    let scale = scale.max(1);
    let [r, g, b, a] = s.color.to_be_bytes().map(|c| c as f64 / 255.0);
    rects.map(|rect| {
        let rect = rect?;
        let (width, height) = ((rect.w * scale) as u32, (rect.h * scale) as u32);
        // Buffer pixel centers in thumbnail-relative logical pixels.
        let center = |origin: i32, i: u32| origin as f64 + (i as f64 + 0.5) / scale as f64;
        let xs: Vec<f64> = (0..width)
            .map(|i| profile(center(rect.x, i), sx0 as f64, sx1 as f64, sigma))
            .collect();
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for j in 0..height {
            let y = a * profile(center(rect.y, j), sy0 as f64, sy1 as f64, sigma);
            for &x in &xs {
                let alpha = x * y;
                let c = |v: f64| (v * alpha * 255.0).round() as u8;
                pixels.extend_from_slice(&[c(b), c(g), c(r), c(1.0)]);
            }
        }
        Some(Strip {
            rect,
            width,
            height,
            pixels,
        })
    })
}

/// Coverage of a Gaussian-blurred interval [`lo`, `hi`] at `x`.
fn profile(x: f64, lo: f64, hi: f64, sigma: f64) -> f64 {
    if sigma <= 0.0 {
        return if x >= lo && x < hi { 1.0 } else { 0.0 };
    }
    let k = std::f64::consts::FRAC_1_SQRT_2 / sigma;
    ((erf((x - lo) * k) - erf((x - hi) * k)) / 2.0).clamp(0.0, 1.0)
}

/// The shader's erf approximation (Abramowitz and Stegun 7.1.27).
fn erf(x: f64) -> f64 {
    let a = x.abs();
    let p = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
    let p2 = p * p;
    x.signum() * (1.0 - 1.0 / (p2 * p2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(softness: f64, spread: i32, offset: (i32, i32)) -> Settings {
        Settings {
            color: 0x000000ff,
            softness,
            spread,
            offset,
        }
    }

    fn alpha(strip: &Strip, x: u32, y: u32) -> u8 {
        strip.pixels[((y * strip.width + x) * 4 + 3) as usize]
    }

    #[test]
    fn strips_cover_region_minus_thumbnail() {
        let cases = [
            (100, 60, settings(30.0, 5, (0, 5))),
            (100, 60, settings(0.0, 0, (0, 0))),
            (100, 60, settings(10.0, -20, (0, 0))),
            (100, 60, settings(4.0, -40, (300, 0))),
            (100, 60, settings(4.0, 0, (-90, 50))),
            (10, 10, settings(7.0, 3, (-2, 4))),
            (100, 60, settings(10.0, -40, (0, 0))),
        ];
        for (w, h, s) in cases {
            let rects = strip_rects(w, h, &s);
            let region = shadow_box(w, h, &s).map(|[x0, y0, x1, y1]| {
                let e = s.extent();
                (x0 - e, y0 - e, x1 + e, y1 + e)
            });
            for y in -400..400 {
                for x in -400..400 {
                    let in_region = region
                        .is_some_and(|(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1);
                    let in_thumb = x >= 0 && x < w && y >= 0 && y < h;
                    let hits = rects
                        .iter()
                        .flatten()
                        .filter(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
                        .count();
                    let want = usize::from(in_region && !in_thumb);
                    assert_eq!(hits, want, "{w}x{h} {s:?} at ({x}, {y}): {rects:?}");
                }
            }
        }
    }

    #[test]
    fn symmetric() {
        let s = settings(30.0, 5, (0, 0));
        let [top, bottom, left, right] = render(100, 60, &s, 1).map(Option::unwrap);
        assert_eq!(top.width, bottom.width);
        assert_eq!(left.height, right.height);
        for y in 0..top.height {
            for x in 0..top.width {
                let a = alpha(&top, x, y);
                assert_eq!(a, alpha(&top, top.width - 1 - x, y));
                assert_eq!(a, alpha(&bottom, x, bottom.height - 1 - y));
            }
        }
        for y in 0..left.height {
            for x in 0..left.width {
                assert_eq!(alpha(&left, x, y), alpha(&right, right.width - 1 - x, y));
            }
        }
    }

    #[test]
    fn edge_is_half_and_extent_is_clear() {
        // Spread 0, no offset: the shadow box is the thumbnail. At scale 2,
        // the pixels on either side of the left edge average to the edge.
        let s = settings(30.0, 0, (0, 0));
        let e = s.extent() as u32;
        let left = render(400, 400, &s, 2)[2].take().unwrap();
        assert_eq!(left.width, 2 * e);
        let y = left.height / 2;
        let edge = alpha(&left, left.width - 1, y) as f64;
        assert!((edge - 127.5).abs() < 3.0, "edge alpha {edge}");
        assert!(alpha(&left, 0, y) <= 1);
        // Monotone towards the thumbnail.
        for x in 1..left.width {
            assert!(alpha(&left, x, y) >= alpha(&left, x - 1, y));
        }
    }

    #[test]
    fn color_alpha_scales() {
        let mut s = settings(30.0, 0, (0, 0));
        s.color = 0xff000080;
        let left = render(400, 400, &s, 1)[2].take().unwrap();
        let px =
            &left.pixels[((left.height / 2 * left.width + left.width - 1) * 4) as usize..][..4];
        // Premultiplied red at about half of 0x80.
        assert!((px[3] as i32 - 64).abs() <= 3, "{px:?}");
        assert_eq!(px[2], px[3]);
        assert_eq!((px[0], px[1]), (0, 0));
    }

    #[test]
    fn hard_edge() {
        let s = settings(0.0, 5, (0, 0));
        let strips = render(100, 60, &s, 1);
        for strip in strips.iter().flatten() {
            assert!(strip.pixels.chunks(4).all(|p| p[3] == 255));
        }
        let [top, _, left, _] = strip_rects(100, 60, &s);
        assert_eq!(
            top,
            Some(Rect {
                x: -5,
                y: -5,
                w: 110,
                h: 5
            })
        );
        assert_eq!(
            left,
            Some(Rect {
                x: -5,
                y: 0,
                w: 5,
                h: 60
            })
        );
    }

    #[test]
    fn scaled_buffer_size() {
        let s = settings(10.0, 5, (0, 5));
        for strip in render(100, 60, &s, 3).iter().flatten() {
            assert_eq!(strip.width, strip.rect.w as u32 * 3);
            assert_eq!(strip.height, strip.rect.h as u32 * 3);
            assert_eq!(
                strip.pixels.len(),
                (strip.width * strip.height * 4) as usize
            );
        }
    }

    #[test]
    fn negative_spread_can_remove_shadow() {
        let s = settings(10.0, -50, (0, 0));
        assert_eq!(strip_rects(100, 60, &s), [None; 4]);
    }
}
