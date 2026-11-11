use std::process::ExitCode;

use clap::Parser;

/// Show the given windows (ext toplevel identifiers; default: all) side by
/// side, let the user pick one, print its ID, and exit 0. Escape or a click on
/// the background exits 1.
#[derive(Parser, Debug)]
#[command(version)]
pub struct Options {
    /// Ext toplevel identifiers of the windows to show
    #[arg(value_name = "ID")]
    pub ids: Vec<String>,

    /// Print identifier<TAB>app_id<TAB>title per window
    #[arg(short, long)]
    pub list: bool,

    /// Start with this window selected (default: first ID)
    #[arg(short, long, value_name = "ID")]
    pub select: Option<String>,

    /// Capture one frame per window (default: live)
    #[arg(long = "static")]
    pub static_: bool,

    /// Paint cursors into thumbnails
    #[arg(short, long)]
    pub cursors: bool,

    /// Max wait for first frames before showing
    #[arg(short, long, value_name = "MS", default_value = "500", value_parser = parse_px)]
    pub timeout: u32,

    /// Overlay color, RRGGBB[AA]
    #[arg(long, value_name = "COLOR", default_value = "000000b3", value_parser = parse_color)]
    pub background: u32,

    /// Selection border color
    #[arg(long, value_name = "COLOR", default_value = "4c8dffff", value_parser = parse_color)]
    pub highlight: u32,

    /// Color of windows without a thumbnail
    #[arg(long, value_name = "COLOR", default_value = "3a3a3aff", value_parser = parse_color)]
    pub placeholder: u32,

    /// Minimum space between thumbnails
    #[arg(long, value_name = "PX", default_value = "24", value_parser = parse_px)]
    pub gap: u32,

    /// Space around the layout
    #[arg(long, value_name = "PX", default_value = "48", value_parser = parse_px)]
    pub margin: u32,

    /// Selection border width
    #[arg(long, value_name = "PX", default_value = "4", value_parser = parse_px)]
    pub border: u32,

    /// Max thumbnail scale
    #[arg(long, value_name = "F", default_value = "1.0", value_parser = parse_scale)]
    pub max_scale: f64,
}

impl Options {
    /// Parse the command line. On `--help`, `--version` or an error, prints
    /// and returns the exit code (0 or 1).
    pub fn parse_args() -> Result<Self, ExitCode> {
        Self::try_parse().map_err(|e| {
            let _ = e.print();
            if e.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        })
    }

    /// Keep capturing after the first frame.
    pub fn live(&self) -> bool {
        !self.static_
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

fn parse_px(s: &str) -> Result<u32, String> {
    s.parse()
        .ok()
        .filter(|&v| v <= 100_000)
        .ok_or_else(|| format!("expected 0-100000, got '{s}'"))
}

fn parse_scale(s: &str) -> Result<f64, String> {
    s.parse()
        .ok()
        .filter(|&v: &f64| v > 0.0)
        .ok_or_else(|| format!("expected a positive number, got '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn colors() {
        assert_eq!(parse_color("000000b3"), Ok(0x000000b3));
        assert_eq!(parse_color("#4c8dff"), Ok(0x4c8dffff));
        assert!(parse_color("+4c8dff").is_err());
        assert!(parse_color("4c8df").is_err());
        assert!(parse_color("4c8dffff00").is_err());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_px("24"), Ok(24));
        assert!(parse_px("-1").is_err());
        assert!(parse_px("100001").is_err());
        assert_eq!(parse_scale("0.5"), Ok(0.5));
        assert!(parse_scale("0").is_err());
    }

    #[test]
    fn cli() {
        Options::command().debug_assert();
        let o = Options::try_parse_from(["wl-expose", "--static", "-s", "2", "1", "2"]).unwrap();
        assert!(!o.live());
        assert_eq!(o.select.as_deref(), Some("2"));
        assert_eq!(o.ids, ["1", "2"]);
        assert_eq!(o.background, 0x000000b3);
    }
}
