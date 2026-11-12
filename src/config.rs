//! Settings from the config file (TOML).
//!
//! Looked up at `$XDG_CONFIG_HOME/wl-expose/config.toml` (default
//! `~/.config/wl-expose/config.toml`) unless `--config` names a file. A
//! missing default file means the defaults; every key is optional.
//!
//! Colors and backgrounds live in `[appearance]`; `[appearance.dark]` and
//! `[appearance.light]` override its keys in the desktop's dark or light
//! mode (or the one `mode` forces).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Deserializer, de::Error as _};

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Opening transition duration in milliseconds; 0: none.
    #[serde(deserialize_with = "duration")]
    pub open_duration: u32,
    /// Card size relative to the output.
    #[serde(deserialize_with = "card_scale")]
    pub card_scale: f64,
    /// Minimum space between thumbnails.
    #[serde(deserialize_with = "px")]
    pub gap: u32,
    /// Space between the card edges and the layout; negative lets
    /// thumbnails extend past the card (but never off the output).
    #[serde(deserialize_with = "signed_px")]
    pub margin: i32,
    /// Zoom factor of the selected thumbnail; 1 disables zooming.
    #[serde(deserialize_with = "zoom")]
    pub zoom: f64,
    /// Zoom animation duration in milliseconds; 0: instant.
    #[serde(deserialize_with = "duration")]
    pub zoom_duration: u32,
    /// Max thumbnail scale.
    #[serde(deserialize_with = "scale")]
    pub max_scale: f64,
    /// App icon size; 0 disables icons.
    #[serde(deserialize_with = "icon_size")]
    pub icon_size: u32,
    /// Show the selected window's title in a pill below it.
    pub titles: bool,
    /// Font family for titles (and letter icons).
    pub font: String,
    /// Title font size.
    #[serde(deserialize_with = "font_size")]
    pub font_size: u32,
    pub appearance: Appearance,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            open_duration: 400,
            card_scale: 0.9,
            gap: 24,
            margin: 48,
            zoom: 1.08,
            zoom_duration: 150,
            max_scale: 1.0,
            icon_size: 48,
            titles: true,
            font: "sans-serif".into(),
            font_size: 14,
            appearance: Appearance::default(),
        }
    }
}

/// Colors and backgrounds: the keys of `[appearance]`, and of its `dark`
/// and `light` tables, which override them.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Theme {
    /// Color outside the card, made opaque over black. With a background
    /// image, the tint over its blurred copy instead.
    #[serde(deserialize_with = "color")]
    pub background: u32,
    /// Image scaled to cover the output, shown on the card.
    /// `~/` is expanded; relative paths are relative to the config file.
    pub background_image: Option<PathBuf>,
    /// Blur of the background image outside the card; 0: none.
    #[serde(deserialize_with = "px")]
    pub background_blur: u32,
    /// Card color without a background image.
    #[serde(deserialize_with = "color")]
    pub card_color: u32,
    /// Color of windows without a thumbnail.
    #[serde(deserialize_with = "color")]
    pub placeholder: u32,
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
    /// Icon theme (a directory name); default: gsettings, then hicolor.
    pub icon_theme: Option<String>,
    #[serde(deserialize_with = "color")]
    pub title_color: u32,
    #[serde(deserialize_with = "color")]
    pub title_background: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: 0x000000b3,
            background_image: None,
            background_blur: 0,
            card_color: 0x303030ff,
            placeholder: 0x3a3a3aff,
            shadow_color: 0x00000077,
            shadow_softness: 30,
            shadow_spread: 5,
            shadow_offset: [0, 5],
            icon_theme: None,
            title_color: 0xffffffff,
            title_background: 0x000000b3,
        }
    }
}

/// Which of `[appearance.dark]` and `[appearance.light]` applies.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Follow the desktop's color scheme.
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scheme {
    #[default]
    Dark,
    Light,
}

/// The `[appearance]` table, resolved for both schemes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Appearance {
    pub mode: Mode,
    /// `[appearance]` with `[appearance.dark]` over it.
    pub dark: Theme,
    /// `[appearance]` with `[appearance.light]` over it.
    pub light: Theme,
    /// The scheme in use; set by `detect`, not read from the file.
    pub scheme: Scheme,
}

impl Appearance {
    /// The theme of the scheme in use.
    pub fn theme(&self) -> &Theme {
        match self.scheme {
            Scheme::Dark => &self.dark,
            Scheme::Light => &self.light,
        }
    }

    /// Pick the scheme: `mode`, or with `auto`, the desktop's (only asked
    /// if the themes differ).
    pub fn detect(&mut self) {
        self.apply(desktop_scheme);
    }

    /// Like `detect`, with `desktop` giving the desktop's scheme.
    pub fn apply(&mut self, desktop: impl FnOnce() -> Scheme) {
        self.scheme = match self.mode {
            Mode::Dark => Scheme::Dark,
            Mode::Light => Scheme::Light,
            Mode::Auto if self.dark == self.light => Scheme::Dark,
            Mode::Auto => desktop(),
        };
    }
}

impl Scheme {
    pub fn new(prefers_dark: bool) -> Self {
        if prefers_dark {
            Scheme::Dark
        } else {
            Scheme::Light
        }
    }
}

impl<'de> Deserialize<'de> for Appearance {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut base = toml::Table::deserialize(d)?;
        let mode = match base.remove("mode") {
            Some(v) => v
                .try_into()
                .map_err(|e| D::Error::custom(format!("mode: {e}")))?,
            None => Mode::default(),
        };
        let mut overrides = |name: &str| match base.remove(name) {
            None => Ok(toml::Table::new()),
            Some(toml::Value::Table(t)) => Ok(t),
            Some(_) => Err(D::Error::custom(format!("{name}: expected a table"))),
        };
        let (dark, light) = (overrides("dark")?, overrides("light")?);
        // Report errors in the shared keys once, without a table name.
        Theme::deserialize(base.clone()).map_err(D::Error::custom)?;
        let theme = |name: &str, overrides: toml::Table| {
            let mut table = base.clone();
            table.extend(overrides);
            Theme::deserialize(table).map_err(|e| D::Error::custom(format!("{name}: {e}")))
        };
        Ok(Self {
            mode,
            dark: theme("dark", dark)?,
            light: theme("light", light)?,
            scheme: Scheme::default(),
        })
    }
}

/// Dark if the desktop prefers dark (per the settings portal, else
/// gsettings), light otherwise.
pub fn desktop_scheme() -> Scheme {
    let dark = portal_prefers_dark().or_else(gsettings_prefers_dark);
    Scheme::new(dark.unwrap_or(false))
}

/// Run `cmd` with `args`; its stdout if it succeeds.
fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8(out.stdout).ok()
}

/// `org.freedesktop.appearance color-scheme`: 0 no preference, 1 dark, 2 light.
pub fn portal_prefers_dark() -> Option<bool> {
    let out = output(
        "gdbus",
        &[
            "call",
            "--session",
            "--timeout=1",
            "--dest=org.freedesktop.portal.Desktop",
            "--object-path=/org/freedesktop/portal/desktop",
            "--method=org.freedesktop.portal.Settings.ReadOne",
            "org.freedesktop.appearance",
            "color-scheme",
        ],
    )?;
    parse_portal(&out)
}

/// `(<uint32 1>,)`, or `(<<uint32 1>>,)` from the older `Read`; also the
/// `SettingChanged` lines of `gdbus monitor`.
pub fn parse_portal(out: &str) -> Option<bool> {
    let rest = &out[out.find("uint32 ")? + "uint32 ".len()..];
    let n: u32 = rest[..rest.find(|c: char| !c.is_ascii_digit())?]
        .parse()
        .ok()?;
    Some(n == 1)
}

pub fn gsettings_prefers_dark() -> Option<bool> {
    let out = output(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    )?;
    Some(parse_gsettings(&out))
}

/// `'prefer-dark'`, from `gsettings get`.
pub fn parse_gsettings(value: &str) -> bool {
    value.trim().trim_matches('\'') == "prefer-dark"
}

impl Config {
    /// The colors and backgrounds in use.
    pub fn theme(&self) -> &Theme {
        self.appearance.theme()
    }

    /// The shadow settings; None if shadows are disabled.
    pub fn shadow(&self) -> Option<crate::shadow::Settings> {
        let t = self.theme();
        (t.shadow_color & 0xff != 0).then(|| crate::shadow::Settings {
            color: t.shadow_color,
            softness: t.shadow_softness as f64,
            spread: t.shadow_spread,
            offset: (t.shadow_offset[0], t.shadow_offset[1]),
        })
    }

    /// The title pill style; None if titles are disabled.
    pub fn title_style(&self) -> Option<crate::text::Style> {
        self.titles.then(|| crate::text::Style {
            font: self.font.clone(),
            size: self.font_size as f64,
            color: self.theme().title_color,
            background: self.theme().title_background,
        })
    }

    /// The file `load` reads: `explicit`, or the default config file.
    pub fn path(explicit: Option<&Path>) -> Option<PathBuf> {
        explicit.map(Path::to_path_buf).or_else(default_path)
    }

    /// Load `explicit`, or the default config file if None (missing is
    /// fine), and detect the scheme.
    pub fn load(explicit: Option<&Path>) -> Result<Self, String> {
        let mut config = Self::read(explicit)?;
        config.appearance.detect();
        Ok(config)
    }

    /// Like `load`, but leaves the scheme for the caller to `apply`.
    pub fn read(explicit: Option<&Path>) -> Result<Self, String> {
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
        let home = std::env::var_os("HOME");
        let appearance = &mut config.appearance;
        for theme in [&mut appearance.dark, &mut appearance.light] {
            if let Some(image) = &mut theme.background_image {
                *image = resolve(image, path.parent(), home.as_deref());
            }
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

fn duration<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    bounded(d, 0, 10_000)
}

fn card_scale<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = f64::deserialize(d)?;
    if v > 0.0 && v <= 1.0 {
        Ok(v)
    } else {
        Err(D::Error::custom(format!(
            "expected more than 0, at most 1, got {v}"
        )))
    }
}

fn zoom<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = f64::deserialize(d)?;
    if (1.0..=2.0).contains(&v) {
        Ok(v)
    } else {
        Err(D::Error::custom(format!("expected 1-2, got {v}")))
    }
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
    fn readme_example() {
        let readme = include_str!("../README.md");
        let start = readme.find("```toml\n").unwrap() + "```toml\n".len();
        let len = readme[start..].find("```").unwrap();
        let c = Config::parse(&readme[start..start + len]).unwrap();
        assert_eq!(c.appearance.dark, Theme::default());
        assert_eq!(c.appearance.light.card_color, 0xf0f0f0ff);
    }

    #[test]
    fn full() {
        let c = Config::parse(
            r##"
            open-duration = 0
            card-scale = 0.8
            gap = 10
            margin = -12
            max-scale = 2
            zoom = 1.1
            zoom-duration = 0
            icon-size = 0
            titles = false
            font = "Inter"
            font-size = 12

            [appearance]
            mode = "light"
            background = "#00000080"
            background-image = "wall.png"
            background-blur = 40
            card-color = "101010"
            placeholder = "#222222"
            shadow-color = "ff000080"
            shadow-softness = 0
            shadow-spread = -3
            shadow-offset = [2, -4]
            icon-theme = "Papirus"
            title-color = "eeeeee"
            title-background = "00000080"
            "##,
        )
        .unwrap();
        assert_eq!(c.open_duration, 0);
        assert_eq!(c.card_scale, 0.8);
        assert_eq!((c.gap, c.margin), (10, -12));
        assert_eq!((c.zoom, c.zoom_duration), (1.1, 0));
        assert_eq!(c.max_scale, 2.0);
        assert_eq!(c.icon_size, 0);
        assert!(!c.titles && c.title_style().is_none());
        assert_eq!((c.font.as_str(), c.font_size), ("Inter", 12));
        assert_eq!(c.appearance.mode, Mode::Light);
        // Without overrides, both schemes are the same.
        assert_eq!(c.appearance.dark, c.appearance.light);
        let t = c.theme();
        assert_eq!(t.background, 0x00000080);
        assert_eq!(t.background_image.as_deref(), Some(Path::new("wall.png")));
        assert_eq!(t.background_blur, 40);
        assert_eq!(t.card_color, 0x101010ff);
        assert_eq!(t.placeholder, 0x222222ff);
        assert_eq!(t.shadow_color, 0xff000080);
        assert_eq!(t.shadow_softness, 0);
        assert_eq!(t.shadow_spread, -3);
        assert_eq!(t.shadow_offset, [2, -4]);
        assert_eq!(t.icon_theme.as_deref(), Some("Papirus"));
        assert_eq!(
            (t.title_color, t.title_background),
            (0xeeeeeeff, 0x00000080)
        );
    }

    #[test]
    fn schemes() {
        let mut c = Config::parse(
            r##"
            [appearance]
            card-color = "101010"
            placeholder = "222222"

            [appearance.dark]
            background-image = "dark.png"

            [appearance.light]
            card-color = "f0f0f0"
            "##,
        )
        .unwrap();
        let a = &c.appearance;
        assert_eq!(a.mode, Mode::Auto);
        assert_eq!(a.dark.card_color, 0x101010ff);
        assert_eq!(a.light.card_color, 0xf0f0f0ff);
        assert_eq!(
            (a.dark.placeholder, a.light.placeholder),
            (0x222222ff, 0x222222ff)
        );
        assert_eq!(
            a.dark.background_image.as_deref(),
            Some(Path::new("dark.png"))
        );
        assert_eq!(a.light.background_image, None);

        c.appearance.mode = Mode::Light;
        c.appearance.detect();
        assert_eq!(c.theme().card_color, 0xf0f0f0ff);
        c.appearance.mode = Mode::Dark;
        c.appearance.detect();
        assert_eq!(c.theme().card_color, 0x101010ff);
    }

    #[test]
    fn portal() {
        assert_eq!(parse_portal("(<uint32 1>,)\n"), Some(true));
        assert_eq!(parse_portal("(<<uint32 2>>,)\n"), Some(false));
        assert_eq!(parse_portal("(<uint32 0>,)\n"), Some(false));
        assert_eq!(parse_portal("()\n"), None);
    }

    #[test]
    fn invalid() {
        let err = |s: &str| Config::parse(s).is_err();
        assert!(err("gap = -1"));
        assert!(err("margin = -100001"));
        assert!(err("gap = 100001"));
        assert!(err("max-scale = 0"));
        assert!(err("zoom = 0.9"));
        assert!(err("zoom = 3"));
        assert!(err("zoom-duration = -1"));
        assert!(err("card-scale = 0"));
        assert!(err("card-scale = 1.5"));
        assert!(err("open-duration = 10001"));
        assert!(err("unknown = 1"));
        assert!(err("font-size = 0"));
        assert!(err("icon-size = 2000"));
        assert!(err("titles = 1"));
        // Appearance keys only go in [appearance].
        assert!(err("placeholder = \"222222\""));
        assert!(err("[appearance]\ngap = 1"));
        assert!(err("[appearance]\nplaceholder = \"red\""));
        assert!(err("[appearance]\nshadow-spread = 100001"));
        assert!(err("[appearance]\nshadow-offset = [1]"));
        assert!(err("[appearance]\nshadow-softness = -1"));
        assert!(err("[appearance]\nmode = \"dim\""));
        assert!(err("[appearance]\ndark = 1"));
        assert!(err("[appearance.dark]\nplaceholder = \"red\""));
        assert!(err("[appearance.light]\nunknown = 1"));
        assert!(err("[appearance.light.dark]"));
        assert!(
            Config::parse("[appearance]\nshadow-color = \"00000000\"")
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
