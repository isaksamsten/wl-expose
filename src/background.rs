//! Background image: decoding into shm-ready pixels, and the viewport crop
//! that scales it to cover the overlay.

use std::path::Path;

/// A decoded image as premultiplied ARGB8888 (little endian: B, G, R, A).
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Image {
    pub fn load(path: &Path) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let rgba = image::ImageReader::open(path)
            .map_err(|e| err(&e))?
            .with_guessed_format()
            .map_err(|e| err(&e))?
            .decode()
            .map_err(|e| err(&e))?
            .into_rgba8();
        let (width, height) = rgba.dimensions();
        if width == 0 || height == 0 || width > i32::MAX as u32 / 4 || height > i32::MAX as u32 / 4
        {
            return Err(err(&format!("unsupported size {width}x{height}")));
        }
        Ok(Self {
            width,
            height,
            data: to_argb8888(rgba.as_raw()),
        })
    }

    pub fn stride(&self) -> i32 {
        self.width as i32 * 4
    }
}

/// Straight RGBA8 to premultiplied ARGB8888 in memory order.
fn to_argb8888(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
        let pre = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
        out.extend_from_slice(&[pre(b), pre(g), pre(r), a]);
    }
    out
}

/// The source rect (x, y, w, h in buffer pixels) of an `iw`x`ih` image that,
/// scaled to `w`x`h`, covers it without distortion: centered, cropping the
/// overflowing axis.
pub fn cover_source(iw: u32, ih: u32, w: u32, h: u32) -> (f64, f64, f64, f64) {
    let (iw, ih, w, h) = (iw as f64, ih as f64, w as f64, h as f64);
    let scale = (w / iw).max(h / ih);
    let (sw, sh) = ((w / scale).min(iw), (h / scale).min(ih));
    // wl_fixed has 1/256 precision; keep the rect inside the buffer after
    // rounding, or the compositor raises a protocol error.
    let floor = |v: f64| (v * 256.0).floor() / 256.0;
    let min = 1.0 / 256.0;
    let (sw, sh) = (floor(sw).max(min), floor(sh).max(min));
    (floor((iw - sw) / 2.0), floor((ih - sh) / 2.0), sw, sh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply() {
        assert_eq!(to_argb8888(&[255, 128, 0, 255]), [0, 128, 255, 255]);
        assert_eq!(to_argb8888(&[255, 255, 255, 0]), [0, 0, 0, 0]);
        assert_eq!(to_argb8888(&[255, 0, 0, 128]), [0, 0, 128, 128]);
    }

    #[test]
    fn cover() {
        // Same aspect: everything.
        assert_eq!(
            cover_source(1920, 1080, 3840, 2160),
            (0.0, 0.0, 1920.0, 1080.0)
        );
        // Wider image: crop the sides.
        assert_eq!(
            cover_source(2000, 1000, 1000, 1000),
            (500.0, 0.0, 1000.0, 1000.0)
        );
        // Taller image: crop top and bottom.
        assert_eq!(
            cover_source(1000, 2000, 1000, 1000),
            (0.0, 500.0, 1000.0, 1000.0)
        );
        // Never outside the buffer.
        for (iw, ih, w, h) in [(1366, 768, 1920, 1200), (3, 7, 2560, 1440), (1, 1, 5, 3)] {
            let (x, y, sw, sh) = cover_source(iw, ih, w, h);
            assert!(x >= 0.0 && y >= 0.0 && sw > 0.0 && sh > 0.0);
            assert!(x + sw <= iw as f64 && y + sh <= ih as f64);
        }
    }
}
