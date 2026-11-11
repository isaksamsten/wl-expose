//! Appearance settings from the config file (TOML).
//!
//! Looked up at `$XDG_CONFIG_HOME/wl-expose/config.toml` (default
//! `~/.config/wl-expose/config.toml`) unless `--config` names a file. A
//! missing default file means the defaults; every key is optional.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, de::Error as _};

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Overlay color, drawn over the background image if there is one.
    #[serde(deserialize_with = "color")]
    pub background: u32,
    /// Image scaled to cover the output, below the background color.
    /// `~/` is expanded; relative paths are relative to the config file.
    pub background_image: Option<PathBuf>,
    /// Selection border color.
    #[serde(deserialize_with = "color")]
    pub highlight: u32,
    /// Color of windows without a thumbnail.
    #[serde(deserialize_with = "color")]
    pub placeholder: u32,
    /// Minimum space between thumbnails.
    #[serde(deserialize_with = "px")]
    pub gap: u32,
    /// Space around the layout.
    #[serde(deserialize_with = "px")]
    pub margin: u32,
    /// Selection border width.
    #[serde(deserialize_with = "px")]
    pub border: u32,
    /// Max thumbnail scale.
    #[serde(deserialize_with = "scale")]
    pub max_scale: f64,
    /// Shadow color; alpha 0 disables shadows.
    #[serde(deserialize_with = "color")]
    pub shadow_color: u32,
    /// Shadow blur radius.
    #[serde(deserialize_with = "px")]
    pub shadow_softness: u32,
    /// How much the shadow box is grown (or shrunk, if negative).
    #[serde(deserialize_with = "signed_px")]
    pub shadow_spread: i32,
    /// How far the shadow is moved, `[x, y]`.
    #[serde(deserialize_with = "offset")]
    pub shadow_offset: [i32; 2],
    /// App icon size; 0 disables icons.
    #[serde(deserialize_with = "icon_size")]
    pub icon_size: u32,
    /// Icon theme (a directory name); default: gsettings, then hicolor.
    pub icon_theme: Option<String>,
    /// Show the selected window's title in a pill below it.
    pub titles: bool,
    /// Font family for titles (and letter icons).
    pub font: String,
    /// Title font size.
    #[serde(deserialize_with = "font_size")]
    pub font_size: u32,
    #[serde(deserialize_with = "color")]
    pub title_color: u32,
    #[serde(deserialize_with = "color")]
    pub title_background: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            background: 0x000000b3,
            background_image: None,
            highlight: 0x4c8dffff,
            placeholder: 0x3a3a3aff,
            gap: 24,
            margin: 48,
            border: 4,
            max_scale: 1.0,
            shadow_color: 0x00000077,
            shadow_softness: 30,
            shadow_spread: 5,
            shadow_offset: [0, 5],
            icon_size: 48,
            icon_theme: None,
            titles: true,
            font: "sans-serif".into(),
            font_size: 14,
            title_color: 0xffffffff,
            title_background: 0x000000b3,
        }
    }
}

impl Config {
    /// The shadow settings; None if shadows are disabled.
    pub fn shadow(&self) -> Option<crate::shadow::Settings> {
        (self.shadow_color & 0xff != 0).then(|| crate::shadow::Settings {
            color: self.shadow_color,
            softness: self.shadow_softness as f64,
            spread: self.shadow_spread,
            offset: (self.shadow_offset[0], self.shadow_offset[1]),
        })
    }

    /// The title pill style; None if titles are disabled.
    pub fn title_style(&self) -> Option<crate::text::Style> {
        self.titles.then(|| crate::text::Style {
            font: self.font.clone(),
            size: self.font_size as f64,
            color: self.title_color,
            background: self.title_background,
        })
    }

    /// The file `load` reads: `explicit`, or the default config file.
    pub fn path(explicit: Option<&Path>) -> Option<PathBuf> {
        explicit.map(Path::to_path_buf).or_else(default_path)
    }

    /// Load `explicit`, or the default config file if None (missing is fine).
    pub fn load(explicit: Option<&Path>) -> Result<Self, String> {
        let Some(path) = Self::path(explicit) else {
            return Ok(Self::default());
        };
        let required = explicit.is_some();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut config =
            Self::parse(&text).map_err(|e| format!("{}: {}", path.display(), e.trim_end()))?;
        if let Some(image) = &mut config.background_image {
            *image = resolve(image, path.parent(), std::env::var_os("HOME").as_deref());
        }
        Ok(config)
    }

    fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

/// `$XDG_CONFIG_HOME/wl-expose/config.toml`, falling back to `~/.config`.
fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("wl-expose").join("config.toml"))
}

/// Expand a leading `~/` and make relative paths relative to `dir`.
fn resolve(path: &Path, dir: Option<&Path>, home: Option<&std::ffi::OsStr>) -> PathBuf {
    if let (Ok(rest), Some(home)) = (path.strip_prefix("~"), home) {
        return Path::new(home).join(rest);
    }
    match dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path.to_path_buf(),
    }
}

/// RRGGBB or RRGGBBAA, optionally with a leading '#', as 0xRRGGBBAA.
fn parse_color(s: &str) -> Result<u32, String> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let invalid = || format!("expected RRGGBB or RRGGBBAA, got '{s}'");
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let v = u32::from_str_radix(hex, 16).map_err(|_| invalid())?;
    match hex.len() {
        6 => Ok(v << 8 | 0xff),
        8 => Ok(v),
        _ => Err(invalid()),
    }
}

fn color<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    parse_color(&String::deserialize(d)?).map_err(D::Error::custom)
}

fn px<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = i64::deserialize(d)?;
    u32::try_from(v)
        .ok()
        .filter(|&v| v <= 100_000)
        .ok_or_else(|| D::Error::custom(format!("expected 0-100000, got {v}")))
}

fn check_signed_px<E: serde::de::Error>(v: i64) -> Result<i32, E> {
    i32::try_from(v)
        .ok()
        .filter(|v| v.abs() <= 100_000)
        .ok_or_else(|| E::custom(format!("expected -100000-100000, got {v}")))
}

fn signed_px<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    check_signed_px(i64::deserialize(d)?)
}

fn offset<'de, D: Deserializer<'de>>(d: D) -> Result<[i32; 2], D::Error> {
    let [x, y] = <[i64; 2]>::deserialize(d)?;
    Ok([check_signed_px(x)?, check_signed_px(y)?])
}

fn bounded<'de, D: Deserializer<'de>>(d: D, min: u32, max: u32) -> Result<u32, D::Error> {
    let v = i64::deserialize(d)?;
    u32::try_from(v)
        .ok()
        .filter(|v| (min..=max).contains(v))
        .ok_or_else(|| D::Error::custom(format!("expected {min}-{max}, got {v}")))
}

fn font_size<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    bounded(d, 1, 1000)
}

fn icon_size<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    bounded(d, 0, 1024)
}

fn scale<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = f64::deserialize(d)?;
    if v > 0.0 && v.is_finite() {
        Ok(v)
    } else {
        Err(D::Error::custom(format!(
            "expected a positive number, got {v}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("000000b3"), Ok(0x000000b3));
        assert_eq!(parse_color("#4c8dff"), Ok(0x4c8dffff));
        assert!(parse_color("+4c8dff").is_err());
        assert!(parse_color("4c8df").is_err());
        assert!(parse_color("4c8dffff00").is_err());
    }

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse(""), Ok(Config::default()));
    }

    #[test]
    fn full() {
        let c = Config::parse(
            r##"
            background = "#00000080"
            background-image = "wall.png"
            highlight = "ff0000"
            placeholder = "#222222"
            gap = 10
            margin = 0
            border = 2
            max-scale = 2
            shadow-color = "ff000080"
            shadow-softness = 0
            shadow-spread = -3
            shadow-offset = [2, -4]
            icon-size = 0
            icon-theme = "Papirus"
            titles = false
            font = "Inter"
            font-size = 12
            title-color = "eeeeee"
            title-background = "00000080"
            "##,
        )
        .unwrap();
        assert_eq!(c.background, 0x00000080);
        assert_eq!(c.background_image.as_deref(), Some(Path::new("wall.png")));
        assert_eq!(c.highlight, 0xff0000ff);
        assert_eq!(c.placeholder, 0x222222ff);
        assert_eq!((c.gap, c.margin, c.border), (10, 0, 2));
        assert_eq!(c.max_scale, 2.0);
        assert_eq!(c.shadow_color, 0xff000080);
        assert_eq!(c.shadow_softness, 0);
        assert_eq!(c.shadow_spread, -3);
        assert_eq!(c.shadow_offset, [2, -4]);
        assert_eq!(c.icon_size, 0);
        assert_eq!(c.icon_theme.as_deref(), Some("Papirus"));
        assert!(!c.titles && c.title_style().is_none());
        assert_eq!((c.font.as_str(), c.font_size), ("Inter", 12));
        assert_eq!(
            (c.title_color, c.title_background),
            (0xeeeeeeff, 0x00000080)
        );
    }

    #[test]
    fn invalid() {
        assert!(Config::parse("gap = -1").is_err());
        assert!(Config::parse("gap = 100001").is_err());
        assert!(Config::parse("max-scale = 0").is_err());
        assert!(Config::parse("highlight = \"red\"").is_err());
        assert!(Config::parse("unknown = 1").is_err());
        assert!(Config::parse("shadow-spread = 100001").is_err());
        assert!(Config::parse("shadow-offset = [1]").is_err());
        assert!(Config::parse("shadow-softness = -1").is_err());
        assert!(Config::parse("font-size = 0").is_err());
        assert!(Config::parse("icon-size = 2000").is_err());
        assert!(Config::parse("titles = 1").is_err());
        assert!(
            Config::parse("shadow-color = \"00000000\"")
                .unwrap()
                .shadow()
                .is_none()
        );
    }

    #[test]
    fn paths() {
        let home = Some(std::ffi::OsStr::new("/home/u"));
        let dir = Some(Path::new("/cfg"));
        assert_eq!(
            resolve(Path::new("~/a.png"), dir, home),
            Path::new("/home/u/a.png")
        );
        assert_eq!(
            resolve(Path::new("a.png"), dir, home),
            Path::new("/cfg/a.png")
        );
        assert_eq!(resolve(Path::new("/a.png"), dir, home), Path::new("/a.png"));
        assert_eq!(
            resolve(Path::new("~/a.png"), dir, None),
            Path::new("/cfg/~/a.png")
        );
    }
}
