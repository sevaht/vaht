//! Noticing that a desktop entry came, changed or went, without polling.
//!
//! The icon store reads every desktop entry once and remembers what it found
//! (and what it did not), so an application installed, or an entry edited,
//! while vaht runs would stay unknown until a reload. One inotify instance
//! watches the `applications` directory of each data directory (and the
//! directories inside it, as the store reads them); when a `.desktop` file
//! there is written, moved, created or deleted, the store forgets what it
//! knows, once things settle, and looks again the next time an icon is wanted.
//!
//! A package update or Steam installing games changes hundreds of files at
//! once; each change pushes the forgetting back, so a burst costs one.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify, WatchDescriptor};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::Compositor;
use crate::event_source::{EventSource, Timer};
use crate::resources::ResourceLedger;

/// The directory of desktop entries in a data directory.
const APPLICATIONS: &str = "applications";

/// How many directories deep inside `applications` entries are read (and so
/// watched): the icon store's own limit.
const DEPTH: u32 = 3;

/// How long after the last change the store forgets what it knows.
const DELAY: Duration = Duration::from_millis(500);

/// What happens to a file that may be a desktop entry: written, moved into
/// or out of the directory, created (a link a package manager makes) or
/// deleted.
const EVENTS: AddWatchFlags = AddWatchFlags::IN_CLOSE_WRITE
    .union(AddWatchFlags::IN_MOVED_TO)
    .union(AddWatchFlags::IN_MOVED_FROM)
    .union(AddWatchFlags::IN_CREATE)
    .union(AddWatchFlags::IN_DELETE);

/// A watch on the directories of desktop entries; dropping it stops watching.
#[derive(Debug)]
pub(crate) struct EntryWatch {
    /// The inotify descriptor, registered with the event loop.
    _source: EventSource<Compositor>,
}

/// What a watched directory is, which says what a change in it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A data directory, watched for its `applications` appearing.
    Data,
    /// `applications`, or a directory inside it.
    Entries,
}

/// The directories being watched.
type Watched = HashMap<WatchDescriptor, (PathBuf, Kind)>;

impl EntryWatch {
    /// Starts watching the `applications` directories of `data_directories`;
    /// the compositor's icons are forgotten, after a pause, whenever a desktop
    /// entry changes in one.
    ///
    /// # Errors
    ///
    /// Returns an error if no inotify instance can be made or registered; a
    /// directory that cannot be watched is left out.
    pub(crate) fn start(
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
        data_directories: &[PathBuf],
    ) -> Result<Self, WatchError> {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .map_err(WatchError::Create)?;
        let mut watched = Watched::new();
        for directory in data_directories {
            // A data directory is watched for its `applications` appearing,
            // and `applications` itself if it is there.
            Self::watch(
                &inotify,
                directory,
                Kind::Data,
                AddWatchFlags::IN_CREATE | AddWatchFlags::IN_MOVED_TO,
                &mut watched,
            );
            Self::watch_entries(&inotify, &directory.join(APPLICATIONS), DEPTH, &mut watched);
        }
        let source = EventSource::insert(
            handle,
            ledger,
            Generic::new(inotify, Interest::READ, Mode::Level),
            move |_, inotify, compositor| {
                let events = match inotify.read_events() {
                    Ok(events) => events,
                    Err(nix::errno::Errno::EAGAIN) => return Ok(PostAction::Continue),
                    Err(error) => {
                        tracing::warn!(%error, "could not read desktop entry changes");
                        return Ok(PostAction::Continue);
                    }
                };
                let mut is_changed = false;
                for event in events {
                    if event.mask.contains(AddWatchFlags::IN_IGNORED) {
                        // The directory is gone, and its watch with it.
                        watched.remove(&event.wd);
                        continue;
                    }
                    let Some((directory, kind)) = watched.get(&event.wd).cloned() else {
                        continue;
                    };
                    let name = event.name.as_deref();
                    let is_directory = event.mask.contains(AddWatchFlags::IN_ISDIR);
                    match (kind, is_directory) {
                        // `applications` appeared in a data directory.
                        (Kind::Data, true) if name == Some(OsStr::new(APPLICATIONS)) => {
                            Self::watch_entries(
                                inotify,
                                &directory.join(APPLICATIONS),
                                DEPTH,
                                &mut watched,
                            );
                            is_changed = true;
                        }
                        (Kind::Data, _) => {}
                        // A directory of entries appeared: watch it too, and
                        // look again at what is in it.
                        (Kind::Entries, true) => {
                            if let Some(name) = name {
                                let new = directory.join(name);
                                let depth = Self::depth_below(&watched, &new);
                                Self::watch_entries(inotify, &new, depth, &mut watched);
                            }
                            is_changed = true;
                        }
                        (Kind::Entries, false) => {
                            is_changed |= name.is_some_and(|name| {
                                Path::new(name).extension() == Some("desktop".as_ref())
                            });
                        }
                    }
                }
                if is_changed {
                    // Another change pushes this back: the burst goes on.
                    match Timer::once(
                        &compositor.loop_handle,
                        &compositor.ledger,
                        DELAY,
                        Compositor::forget_desktop_entries,
                    ) {
                        Ok(timer) => compositor.entry_refresh = Some(timer),
                        Err(error) => tracing::warn!(
                            error = &error as &dyn std::error::Error,
                            "could not schedule a look at the desktop entries"
                        ),
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| WatchError::Register(error.error))?;
        Ok(Self { _source: source })
    }

    /// Watches `directory` as `kind`, if it can be, and notes it in `watched`.
    fn watch(
        inotify: &Inotify,
        directory: &Path,
        kind: Kind,
        events: AddWatchFlags,
        watched: &mut Watched,
    ) {
        if let Ok(descriptor) = inotify.add_watch(directory, events) {
            watched.insert(descriptor, (directory.to_path_buf(), kind));
        }
    }

    /// Watches `directory` and the directories in it, `depth` levels down.
    fn watch_entries(inotify: &Inotify, directory: &Path, depth: u32, watched: &mut Watched) {
        Self::watch(inotify, directory, Kind::Entries, EVENTS, watched);
        if depth == 0 {
            return;
        }
        let Ok(listing) = std::fs::read_dir(directory) else {
            return;
        };
        for path in listing.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                Self::watch_entries(inotify, &path, depth - 1, watched);
            }
        }
    }

    /// Returns how many more levels of directories inside `directory`, a new
    /// one in a watched one, are watched: the depth less how far down it is.
    fn depth_below(watched: &Watched, directory: &Path) -> u32 {
        let top = watched
            .values()
            .filter(|(_, kind)| *kind == Kind::Entries)
            .map(|(path, _)| path)
            .filter(|path| directory.starts_with(path))
            .min_by_key(|path| path.components().count());
        let below = top.map_or(0, |top| {
            directory
                .strip_prefix(top)
                .map_or(0, |rest| rest.components().count())
        });
        DEPTH.saturating_sub(u32::try_from(below).unwrap_or(DEPTH))
    }
}

/// Why the desktop entries cannot be watched.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WatchError {
    /// No inotify instance could be created.
    #[error("could not create an inotify instance")]
    Create(#[source] nix::Error),
    /// The watch could not be registered with the event loop.
    #[error("could not register the watch with the event loop")]
    Register(#[source] calloop::Error),
}
