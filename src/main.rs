//! wl-expose: pick a window from an exposé-style overlay and print its ID.

mod app;
mod background;
mod capture;
mod config;
mod daemon;
mod dmabuf;
mod globals;
mod icons;
mod input;
mod ipc;
mod layout;
mod options;
mod overlay;
mod shadow;
mod text;
mod toplevel;
mod worker;

use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;

use smithay_client_toolkit::reexports::{
    calloop::EventLoop, calloop_wayland_source::WaylandSource,
};
use wayland_client::{Connection, Proxy, QueueHandle, globals::registry_queue_init};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;

use crate::app::{App, Outcome};
use crate::background::Image;
use crate::config::Config;
use crate::globals::Globals;
use crate::options::Options;
use crate::overlay::Wallpaper;

fn main() -> ExitCode {
    let opts = match Options::parse_args() {
        Ok(opts) => opts,
        Err(code) => return code,
    };
    if opts.use_daemon()
        && let Some(code) = daemon::client(&opts.request())
    {
        return code;
    }
    let result = if opts.daemon {
        daemon::run(&opts)
    } else {
        run(&opts)
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("wl-expose: {e}");
            ExitCode::FAILURE
        }
    }
}

type Error = Box<dyn std::error::Error>;

/// A Wayland connection with the window list received.
pub struct Wayland {
    pub conn: Connection,
    pub qh: QueueHandle<App>,
    pub event_loop: EventLoop<'static, App>,
    pub app: App,
    /// What the overlay needs; None if not requested.
    pub g: Option<Rc<Globals>>,
}

/// Connect, bind the globals (those of the overlay too if `overlay`), and
/// receive the window list.
pub fn connect(overlay: bool) -> Result<Wayland, Error> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("failed to connect to wayland display: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();

    let toplevel_list = ExtForeignToplevelListV1::interface().name;
    if !globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == toplevel_list))
    {
        return Err(format!("missing global: {toplevel_list}").into());
    }
    let g = if overlay {
        // Globals::bind reports each missing global.
        let g = Globals::bind(&globals, &qh).ok_or("required globals missing")?;
        Some(Rc::new(g))
    } else {
        None
    };

    let event_loop: EventLoop<App> = EventLoop::try_new()?;
    let mut app = App::new(&globals, &qh, event_loop.handle());
    // The first roundtrip delivers the handles, the second their state.
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle())?;
    Ok(Wayland {
        conn,
        qh,
        event_loop,
        app,
        g,
    })
}

/// Decode `path` into an shm buffer.
pub fn load_wallpaper(
    path: &Path,
    g: &Globals,
    qh: &QueueHandle<App>,
) -> Result<Rc<Wallpaper>, String> {
    let img = Image::load(path)?;
    let wallpaper = Wallpaper::new(g, &img, qh).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Rc::new(wallpaper))
}

/// Without a daemon: show the overlay from this process.
fn run(opts: &Options) -> Result<ExitCode, Error> {
    let config = if opts.list {
        None
    } else {
        Some(Config::load(opts.config.as_deref()).map_err(|e| format!("config: {e}"))?)
    };
    let Wayland {
        conn,
        qh,
        mut event_loop,
        mut app,
        g,
    } = connect(!opts.list)?;
    let (Some(g), Some(config)) = (g, config) else {
        let toplevels: Vec<_> = app.toplevels().into_iter().map(|(_, t)| t).collect();
        print!("{}", toplevel::format_list(&toplevels));
        return Ok(ExitCode::SUCCESS);
    };

    // A broken image is not worth failing over: fall back to the color.
    let wallpaper = config.background_image.as_deref().and_then(|path| {
        load_wallpaper(path, &g, &qh)
            .inspect_err(|e| eprintln!("wl-expose: {e}"))
            .ok()
    });
    let mut warn = |m: String| eprintln!("wl-expose: {m}");
    if let Err(e) = app.show(&g, &opts.request(), &config, wallpaper, &qh, &mut warn) {
        eprintln!("wl-expose: {e}");
        return Ok(ExitCode::FAILURE);
    }

    while app.outcome.is_none() {
        event_loop
            .dispatch(None, &mut app)
            .map_err(|e| format!("wayland connection error: {e}"))?;
        app.after_dispatch();
    }

    app.end_show();
    app.release_input();
    app.toplevels.stop();
    // Make sure the overlay is gone before the caller acts on the result.
    conn.roundtrip()?;

    Ok(match app.outcome {
        Some(Outcome::Picked(id)) => {
            println!("{id}");
            ExitCode::SUCCESS
        }
        _ => ExitCode::FAILURE,
    })
}
