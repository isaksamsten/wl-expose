use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::ipc::Request;

/// Show the given windows (ext toplevel identifiers; default: all) side by
/// side, let the user pick one, print its ID, and exit 0. Escape or a click on
/// the background exits 1. Appearance is set in the config file
/// ($XDG_CONFIG_HOME/wl-expose/config.toml). If a daemon (--daemon) is
/// running, it shows the overlay; otherwise this process does.
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

    /// Max wait for window sizes before showing
    #[arg(short, long, value_name = "MS", default_value = "500", value_parser = parse_ms)]
    pub timeout: u32,

    /// Output to show on (a wl_output name, e.g. DP-1); default: the only
    /// output, if there is just one
    #[arg(short, long, value_name = "NAME")]
    pub output: Option<String>,

    /// Config file (default: $XDG_CONFIG_HOME/wl-expose/config.toml). Without
    /// --daemon, implies --no-daemon
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Run in the background and show the overlay for later invocations
    #[arg(long, conflicts_with_all = ["ids", "list", "select", "static_", "cursors", "timeout", "output", "no_daemon"])]
    pub daemon: bool,

    /// Show the overlay from this process even if a daemon is running
    #[arg(long)]
    pub no_daemon: bool,
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

    /// What to show, for this process or the daemon.
    pub fn request(&self) -> Request {
        Request {
            ids: self.ids.clone(),
            select: self.select.clone(),
            live: !self.static_,
            cursors: self.cursors,
            timeout_ms: self.timeout,
            output: self.output.clone(),
        }
    }

    /// Hand the request to a running daemon, if any.
    pub fn use_daemon(&self) -> bool {
        !self.daemon && !self.list && !self.no_daemon && self.config.is_none()
    }
}

fn parse_ms(s: &str) -> Result<u32, String> {
    s.parse()
        .ok()
        .filter(|&v| v <= 100_000)
        .ok_or_else(|| format!("expected 0-100000, got '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn numbers() {
        assert_eq!(parse_ms("24"), Ok(24));
        assert!(parse_ms("-1").is_err());
        assert!(parse_ms("100001").is_err());
    }

    #[test]
    fn cli() {
        Options::command().debug_assert();
        let o =
            Options::try_parse_from(["wl-expose", "--static", "-s", "2", "-o", "DP-1", "1", "2"])
                .unwrap();
        assert!(!o.request().live);
        assert_eq!(o.select.as_deref(), Some("2"));
        assert_eq!(o.request().output.as_deref(), Some("DP-1"));
        assert_eq!(o.ids, ["1", "2"]);
        assert_eq!(o.timeout, 500);
        assert!(o.config.is_none());
        assert!(o.use_daemon());
    }

    #[test]
    fn daemon() {
        let parse = |args: &[&str]| Options::try_parse_from([&["wl-expose"], args].concat());
        assert!(parse(&["--daemon", "--config", "c.toml"]).unwrap().daemon);
        assert!(parse(&["--daemon", "1"]).is_err());
        assert!(parse(&["--daemon", "--static"]).is_err());
        assert!(parse(&["--daemon", "-t", "100"]).is_err());
        assert!(parse(&["--daemon", "-o", "DP-1"]).is_err());
        assert!(!parse(&["--config", "c.toml"]).unwrap().use_daemon());
        assert!(!parse(&["--no-daemon"]).unwrap().use_daemon());
        assert!(!parse(&["--list"]).unwrap().use_daemon());
    }
}
