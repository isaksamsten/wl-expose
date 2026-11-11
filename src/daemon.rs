//! `--daemon`: keep the Wayland connection, window list, config and
//! wallpaper ready, and show the overlay for clients on a Unix socket (see
//! ipc.rs). Captures still start fresh on every show.
//!
//! One show at a time: a request while the overlay is open cancels it (and
//! is answered with `cancelled`), so binding wl-expose to a key toggles it.
//! A client that disconnects cancels its show.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::SystemTime;

use smithay_client_toolkit::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
use wayland_client::QueueHandle;

use crate::app::{App, Outcome};
use crate::config::Config;
use crate::globals::Globals;
use crate::ipc::{self, Reply, Request};
use crate::options::Options;
use crate::overlay::Wallpaper;
use crate::{Error, Wayland};

/// Requests are tiny; anything bigger is not a client.
const MAX_REQUEST: usize = 1 << 20;

/// Client events, queued by the socket sources for the daemon loop.
pub enum Incoming {
    /// A complete request; `stream` is for the replies.
    Request {
        client: u64,
        stream: UnixStream,
        req: Request,
    },
    /// The client closed its connection.
    Gone(u64),
}

/// Hand `req` to a running daemon and relay its answer. None: no daemon is
/// running (or it went away before taking the request).
pub fn client(req: &Request) -> Option<ExitCode> {
    let path = ipc::socket_path()?;
    let mut stream = UnixStream::connect(path).ok()?;
    stream.write_all(req.encode().as_bytes()).ok()?;
    // Keep the connection open while waiting: closing it cancels.
    let mut lines = BufReader::new(stream).lines();
    loop {
        let reply = match lines.next() {
            Some(Ok(line)) => Reply::decode(&line),
            Some(Err(e)) => Err(e.to_string()),
            None => Err("connection closed".into()),
        };
        match reply {
            Ok(Reply::Warn(m)) => eprintln!("wl-expose: {m}"),
            Ok(Reply::Picked(id)) => {
                println!("{id}");
                return Some(ExitCode::SUCCESS);
            }
            Ok(Reply::Cancelled) => return Some(ExitCode::FAILURE),
            Ok(Reply::Error(m)) => {
                eprintln!("wl-expose: {m}");
                return Some(ExitCode::FAILURE);
            }
            Err(e) => {
                eprintln!("wl-expose: daemon: {e}");
                return Some(ExitCode::FAILURE);
            }
        }
    }
}

pub fn run(opts: &Options) -> Result<ExitCode, Error> {
    let path = ipc::socket_path().ok_or("XDG_RUNTIME_DIR is not set (or not absolute)")?;
    let mut appearance =
        Appearance::load(opts.config.clone()).map_err(|e| format!("config: {e}"))?;
    let Wayland {
        conn,
        qh,
        mut event_loop,
        mut app,
        g,
    } = crate::connect(true)?;
    let g = g.expect("overlay globals were requested");
    appearance.refresh(&g, &qh, &mut |m| eprintln!("wl-expose: {m}"));
    // Start the worker now, so fonts and icons are ready for the first show.
    app.prefetch_icons(&appearance.config);

    let listener = bind(&path)?;
    let _socket = SocketFile(path.clone());
    listener.set_nonblocking(true)?;
    let mut next_client = 0;
    event_loop.handle().insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        move |_, listener, app| {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        next_client += 1;
                        if let Err(e) = watch_client(app, stream, next_client) {
                            eprintln!("wl-expose: client: {e}");
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(e) => {
                        eprintln!("wl-expose: accept: {e}");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        },
    )?;
    eprintln!("wl-expose: daemon listening on {}", path.display());

    // The client of the current show.
    let mut active: Option<(u64, UnixStream)> = None;
    loop {
        event_loop
            .dispatch(None, &mut app)
            .map_err(|e| format!("wayland connection error: {e}"))?;
        app.after_dispatch();

        for incoming in std::mem::take(&mut app.inbox) {
            match incoming {
                Incoming::Request { stream, .. } if active.is_some() => {
                    // Toggle: close the open overlay instead of queueing.
                    app.finish(Outcome::Cancelled);
                    answer(stream, Reply::Cancelled);
                }
                Incoming::Request {
                    client,
                    stream,
                    req,
                } => {
                    let mut warn = |m: String| send(&stream, &Reply::Warn(m));
                    if appearance.refresh(&g, &qh, &mut warn) {
                        // Icon size, theme or font may differ.
                        app.clear_icons();
                    }
                    let wallpaper = appearance.wallpaper.clone();
                    match app.show(&g, &req, &appearance.config, wallpaper, &qh, &mut warn) {
                        Ok(()) => active = Some((client, stream)),
                        Err(e) => answer(stream, Reply::Error(e)),
                    }
                }
                Incoming::Gone(client) => {
                    if active.as_ref().is_some_and(|(c, _)| *c == client) {
                        app.finish(Outcome::Cancelled);
                    }
                }
            }
        }

        if let Some(outcome) = app.outcome.take() {
            app.end_show();
            // Make sure the overlay is gone before the client acts on the result.
            conn.roundtrip()?;
            if let Some((_, stream)) = active.take() {
                answer(
                    stream,
                    match outcome {
                        Outcome::Picked(id) => Reply::Picked(id),
                        Outcome::Cancelled => Reply::Cancelled,
                    },
                );
            }
            // Windows opened since are ready next time.
            app.prefetch_icons(&appearance.config);
        }
    }
}

/// Read the client's request, then watch for it hanging up.
fn watch_client(app: &mut App, stream: UnixStream, client: u64) -> std::io::Result<()> {
    stream.set_nonblocking(true)?;
    // Some(..) until the request is complete.
    let mut reply_stream = Some(stream.try_clone()?);
    let mut buf = Vec::new();
    let source = Generic::new(stream, Interest::READ, Mode::Level);
    let result = app
        .loop_handle
        .insert_source(source, move |_, stream, app| {
            let mut chunk = [0; 4096];
            loop {
                match (&**stream).read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) if reply_stream.is_some() => buf.extend_from_slice(&chunk[..n]),
                    Ok(_) => {} // nothing is expected after the request
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        if let Some(reply) = reply_stream.take() {
                            reply_stream = take_request(app, client, reply, &mut buf);
                        }
                        return Ok(PostAction::Continue);
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
                if buf.len() > MAX_REQUEST {
                    if let Some(reply) = reply_stream.take() {
                        answer(reply, Reply::Error("request too large".into()));
                    }
                    return Ok(PostAction::Remove);
                }
            }
            // End of stream (or an error): the client is gone.
            if reply_stream.is_none() {
                app.inbox.push(Incoming::Gone(client));
            }
            Ok(PostAction::Remove)
        });
    result
        .map(|_| ())
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Queue the request in `buf` if complete. Gives `reply` back if not.
fn take_request(
    app: &mut App,
    client: u64,
    reply: UnixStream,
    buf: &mut Vec<u8>,
) -> Option<UnixStream> {
    match Request::decode(buf) {
        Ok(Some(req)) => {
            buf.clear();
            app.inbox.push(Incoming::Request {
                client,
                stream: reply,
                req,
            });
            None
        }
        Ok(None) => Some(reply),
        Err(e) => {
            answer(reply, Reply::Error(format!("bad request: {e}")));
            None
        }
    }
}

/// Best effort: replies are tiny, and a client that went away needs none.
fn send(stream: &UnixStream, reply: &Reply) {
    let _ = (&*stream).write_all(reply.encode().as_bytes());
}

/// Send the final reply and close, which also ends the client's watch.
fn answer(stream: UnixStream, reply: Reply) {
    send(&stream, &reply);
    let _ = stream.shutdown(Shutdown::Both);
}

/// Bind the socket, replacing a stale one left by a daemon that died.
fn bind(path: &Path) -> Result<UnixListener, Error> {
    match UnixListener::bind(path) {
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Err(format!("a daemon is already listening on {}", path.display()).into());
            }
            std::fs::remove_file(path)?;
            Ok(UnixListener::bind(path)?)
        }
        other => Ok(other?),
    }
}

/// Removes the socket on exit.
struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The config and wallpaper, reloaded when their files change.
struct Appearance {
    /// `--config`, if given.
    explicit: Option<PathBuf>,
    config: Config,
    config_mtime: Option<SystemTime>,
    wallpaper: Option<Rc<Wallpaper>>,
    /// The image path and mtime `wallpaper` was loaded from (or failed to).
    wallpaper_source: Option<(PathBuf, Option<SystemTime>)>,
}

impl Appearance {
    fn load(explicit: Option<PathBuf>) -> Result<Self, String> {
        let config = Config::load(explicit.as_deref())?;
        let config_mtime = Config::path(explicit.as_deref()).as_deref().and_then(mtime);
        Ok(Self {
            explicit,
            config,
            config_mtime,
            wallpaper: None,
            wallpaper_source: None,
        })
    }

    /// Reload whatever changed. Errors keep the previous config, or show no
    /// image, and are reported to `warn`. Returns whether the config changed.
    fn refresh(
        &mut self,
        g: &Globals,
        qh: &QueueHandle<App>,
        warn: &mut dyn FnMut(String),
    ) -> bool {
        let config_mtime = Config::path(self.explicit.as_deref())
            .as_deref()
            .and_then(mtime);
        let mut changed = false;
        if config_mtime != self.config_mtime {
            match Config::load(self.explicit.as_deref()) {
                Ok(config) => {
                    changed = config != self.config;
                    self.config = config;
                    self.config_mtime = config_mtime;
                }
                Err(e) => warn(format!("config: {e} (keeping the previous config)")),
            }
        }

        let source = self.config.background_image.clone().map(|path| {
            let mtime = mtime(&path);
            (path, mtime)
        });
        if source != self.wallpaper_source {
            // Free the old image before decoding the new one.
            self.wallpaper = None;
            self.wallpaper_source = source;
            if let Some((path, _)) = &self.wallpaper_source {
                match crate::load_wallpaper(path, g, qh) {
                    Ok(wallpaper) => self.wallpaper = Some(wallpaper),
                    Err(e) => warn(e),
                }
            }
        }
        changed
    }
}
