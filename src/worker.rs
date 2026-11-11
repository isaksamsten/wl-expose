//! A worker thread for everything slow that the overlay doesn't wait for:
//! loading fonts, desktop entries and icons, and rendering title pills and
//! letter tiles. Results come back through a calloop channel, and the main
//! thread turns them into shm buffers.
//!
//! The worker lives as long as the process, so in the daemon its caches (font
//! system, desktop entries, decoded icons) are warm for every show. The main
//! thread keeps the icons it received too, so a show can attach them before
//! its first frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use smithay_client_toolkit::reexports::calloop::channel;

use crate::background::Image;
use crate::icons::{self, Icons};
use crate::overlay::WindowId;
use crate::text::{self, Text};

enum Job {
    /// The icon for `key`; a letter tile in `font` if there is none.
    Icon { key: icons::Key, font: String },
    /// A title pill for a window.
    Pill {
        window: WindowId,
        request: PillRequest,
    },
    /// Reload the desktop entries if they changed.
    Refresh,
    /// Forget decoded icons, e.g. because the config changed.
    ClearIcons,
}

/// What a pill is rendered for. A result is only used while it is still
/// what the overlay wants.
#[derive(Clone, Debug, PartialEq)]
pub struct PillRequest {
    pub title: String,
    /// Logical pixels.
    pub max_width: i32,
    pub scale: i32,
    pub style: text::Style,
}

pub enum Done {
    Icon {
        key: icons::Key,
        image: Arc<Image>,
    },
    /// The desktop entries changed, so cached icons may be stale.
    IconsReloaded,
    Pill {
        window: WindowId,
        request: PillRequest,
        pill: text::Pill,
    },
}

/// The worker and the icons received from it, shared by the app and its
/// overlays.
pub struct Assets {
    jobs: mpsc::Sender<Job>,
    /// None: requested, not received yet.
    icons: RefCell<HashMap<icons::Key, Option<Arc<Image>>>>,
}

impl Assets {
    /// Start the worker thread; it loads the fonts when idle. Insert the
    /// returned channel into the event loop and pass its results to
    /// `received`.
    pub fn spawn() -> std::io::Result<(Self, channel::Channel<Done>)> {
        let (jobs, inbox) = mpsc::channel();
        let (done, results) = channel::channel();
        std::thread::Builder::new()
            .name("wl-expose-worker".into())
            .spawn(move || run(inbox, done))?;
        let assets = Self {
            jobs,
            icons: RefCell::new(HashMap::new()),
        };
        Ok((assets, results))
    }

    /// Fire and forget: if the worker died, the overlay lacks icons and titles.
    fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// The icon for `key` if received; otherwise it is requested (once) and
    /// arrives as `Done::Icon`.
    pub fn icon(&self, key: &icons::Key, font: &str) -> Option<Arc<Image>> {
        let mut icons = self.icons.borrow_mut();
        if let Some(entry) = icons.get(key) {
            return entry.clone();
        }
        icons.insert(key.clone(), None);
        self.send(Job::Icon {
            key: key.clone(),
            font: font.to_string(),
        });
        None
    }

    pub fn pill(&self, window: WindowId, request: PillRequest) {
        self.send(Job::Pill { window, request });
    }

    /// Check for changed desktop entries (answered with `IconsReloaded` if so).
    pub fn refresh(&self) {
        self.send(Job::Refresh);
    }

    /// Forget all icons, here and in the worker.
    pub fn clear_icons(&self) {
        self.icons.borrow_mut().clear();
        self.send(Job::ClearIcons);
    }

    /// Book-keeping for a result from the worker.
    pub fn received(&self, done: &Done) {
        match done {
            Done::Icon { key, image } => {
                let mut icons = self.icons.borrow_mut();
                // Unless cleared since it was requested.
                if let Some(entry) = icons.get_mut(key) {
                    *entry = Some(image.clone());
                }
            }
            // The worker already dropped its stale icons.
            Done::IconsReloaded => self.icons.borrow_mut().clear(),
            Done::Pill { .. } => {}
        }
    }
}

fn run(inbox: mpsc::Receiver<Job>, done: channel::Sender<Done>) {
    // Loading the system fonts takes a while: do it when first needed, or
    // as soon as there is nothing else to do, so icons don't wait for it.
    let mut text: Option<Text> = None;
    let mut icons = Icons::new();
    // Letter tiles for apps without an icon, by icon key and font.
    let mut letters: HashMap<(icons::Key, String), Arc<Image>> = HashMap::new();
    loop {
        let job = match inbox.try_recv() {
            Ok(job) => job,
            Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {
                text.get_or_insert_with(Text::new);
                match inbox.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            }
        };
        let result = match job {
            Job::Icon { key, font } => {
                icons.load_entries();
                let image = match icons.get(&key) {
                    Some(image) => image,
                    None => letters
                        .entry((key.clone(), font))
                        .or_insert_with_key(|(key, font)| {
                            let text = text.get_or_insert_with(Text::new);
                            Arc::new(text.letter(&key.app_id, key.size, font))
                        })
                        .clone(),
                };
                Done::Icon { key, image }
            }
            Job::Pill { window, request } => {
                let pill = text.get_or_insert_with(Text::new).pill(
                    &request.title,
                    &request.style,
                    request.max_width,
                    request.scale,
                );
                Done::Pill {
                    window,
                    request,
                    pill,
                }
            }
            Job::Refresh => {
                if !icons.refresh() {
                    continue;
                }
                letters.clear();
                Done::IconsReloaded
            }
            Job::ClearIcons => {
                icons.clear();
                letters.clear();
                continue;
            }
        };
        if done.send(result).is_err() {
            return;
        }
    }
}
