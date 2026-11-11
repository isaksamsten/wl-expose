//! wl-expose: pick a window from an exposé-style overlay and print its ID.

mod app;
mod capture;
mod globals;
mod input;
mod layout;
mod options;
mod overlay;
mod toplevel;

use std::process::ExitCode;
use std::time::Duration;

use smithay_client_toolkit::reexports::{
    calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    },
    calloop_wayland_source::WaylandSource,
};
use wayland_client::{Connection, Proxy, globals::registry_queue_init};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;

use crate::app::{App, Outcome};
use crate::globals::Globals;
use crate::options::Options;
use crate::overlay::Overlay;

fn main() -> ExitCode {
    let opts = match Options::parse_args() {
        Ok(opts) => opts,
        Err(code) => return code,
    };
    match run(&opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("wl-expose: {e}");
            ExitCode::FAILURE
        }
    }
}

type Error = Box<dyn std::error::Error>;

fn run(opts: &Options) -> Result<ExitCode, Error> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("failed to connect to wayland display: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();

    let toplevel_list = ExtForeignToplevelListV1::interface().name;
    if !globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == toplevel_list))
    {
        eprintln!("wl-expose: missing global: {toplevel_list}");
        return Ok(ExitCode::FAILURE);
    }
    let globals_overlay = if opts.list {
        None
    } else {
        match Globals::bind(&globals, &qh) {
            Some(g) => Some(g),
            None => return Ok(ExitCode::FAILURE),
        }
    };

    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let mut app = App::new(&globals, &qh, event_loop.handle());
    // The first roundtrip delivers the handles, the second their state.
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;

    let (handles, toplevels): (Vec<_>, Vec<_>) = app.toplevels().into_iter().unzip();
    let Some(g) = globals_overlay else {
        print!("{}", toplevel::format_list(&toplevels));
        return Ok(ExitCode::SUCCESS);
    };

    let shown = toplevel::select(&toplevels, &opts.ids, |id| {
        eprintln!("wl-expose: no window with ID '{id}'");
    });
    if shown.is_empty() {
        eprintln!("wl-expose: no windows to show");
        return Ok(ExitCode::FAILURE);
    }
    let windows: Vec<_> = shown
        .iter()
        .map(|&i| (handles[i].clone(), toplevels[i].identifier.clone()))
        .collect();

    let mut overlay = Overlay::new(g, windows, opts, &qh)?;
    // The first window passed in starts selected, unless --select says otherwise.
    let mut selected = 0;
    if let Some(id) = &opts.select {
        match overlay.windows.iter().position(|w| w.identifier == *id) {
            Some(i) => selected = i,
            None => eprintln!("wl-expose: --select: no shown window with ID '{id}'"),
        }
    }
    overlay.selected = Some(selected);
    app.overlay = Some(overlay);

    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue).insert(handle.clone())?;
    let timeout = Timer::from_duration(Duration::from_millis(opts.timeout.into()));
    handle.insert_source(timeout, |_, _, app| {
        if let Some(overlay) = &mut app.overlay {
            overlay.deadline_passed();
        }
        TimeoutAction::Drop
    })?;

    while app.outcome.is_none() {
        event_loop
            .dispatch(None, &mut app)
            .map_err(|e| format!("wayland connection error: {e}"))?;
        if let Some(overlay) = &mut app.overlay {
            overlay.maybe_map();
        }
    }

    app.release_input();
    app.overlay = None;
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
