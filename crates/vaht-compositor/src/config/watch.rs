//! Noticing that a configuration file changed, without polling (02 §2).
//!
//! One inotify instance watches the configuration directory, and its
//! `themes` directory when there is one, and is registered with the event
//! loop. Directories are watched rather than files, because editors save by
//! writing a new file and renaming it over the old one, which a watch on
//! the old file would never see.

use std::path::Path;

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use super::{ConfigFile, ThemeFile};
use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// A watch on the configuration directory; dropping it stops watching.
#[derive(Debug)]
pub(crate) struct ConfigWatch {
    /// The inotify descriptor, registered with the event loop.
    _source: EventSource<Compositor>,
}

impl ConfigWatch {
    /// What happens to a file that may change the configuration: written,
    /// renamed into or out of the directory, created or deleted.
    const EVENTS: AddWatchFlags = AddWatchFlags::IN_CLOSE_WRITE
        .union(AddWatchFlags::IN_MOVED_TO)
        .union(AddWatchFlags::IN_MOVED_FROM)
        .union(AddWatchFlags::IN_CREATE)
        .union(AddWatchFlags::IN_DELETE)
        .union(AddWatchFlags::IN_DELETE_SELF)
        .union(AddWatchFlags::IN_MOVE_SELF);

    /// Starts watching `directory`; the compositor hears of every change to
    /// a configuration file in it.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be watched, for example
    /// because it does not exist.
    pub(crate) fn start(
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
        directory: &Path,
    ) -> Result<Self, WatchError> {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .map_err(WatchError::Create)?;
        inotify
            .add_watch(directory, Self::EVENTS)
            .map_err(WatchError::Watch)?;
        // Without a themes directory yet, its creation is a change like any
        // other, and the next reload watches it.
        let themes = inotify
            .add_watch(&directory.join(ThemeFile::DIRECTORY), Self::EVENTS)
            .ok();
        let source = EventSource::insert(
            handle,
            ledger,
            Generic::new(inotify, Interest::READ, Mode::Level),
            move |_, inotify, compositor| {
                let events = match inotify.read_events() {
                    Ok(events) => events,
                    Err(nix::errno::Errno::EAGAIN) => return Ok(PostAction::Continue),
                    Err(error) => {
                        tracing::warn!(%error, "could not read configuration changes");
                        return Ok(PostAction::Continue);
                    }
                };
                let directory_gone = events.iter().any(|event| {
                    event
                        .mask
                        .intersects(AddWatchFlags::IN_DELETE_SELF | AddWatchFlags::IN_MOVE_SELF)
                });
                let file_changed = events.iter().any(|event| {
                    let name = event.name.as_deref();
                    if Some(event.wd) == themes {
                        name.is_some_and(|name| {
                            Path::new(name).extension() == Some("toml".as_ref())
                        })
                    } else {
                        name.is_some_and(|name| {
                            ConfigFile::is_named(name) || name == ThemeFile::DIRECTORY
                        })
                    }
                });
                // A themes directory appearing or going needs a new watch.
                let themes_moved = events.iter().any(|event| {
                    Some(event.wd) != themes
                        && event.name.as_deref() == Some(ThemeFile::DIRECTORY.as_ref())
                });
                if directory_gone || file_changed {
                    compositor.config_source.note_change(
                        directory_gone || themes_moved,
                        &compositor.loop_handle,
                        &compositor.ledger,
                    );
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| WatchError::Register(error.error))?;
        Ok(Self { _source: source })
    }
}

/// Why the configuration directory cannot be watched.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WatchError {
    /// No inotify instance could be created.
    #[error("could not create an inotify instance")]
    Create(#[source] nix::Error),
    /// The directory could not be watched.
    #[error("could not watch the directory")]
    Watch(#[source] nix::Error),
    /// The watch could not be registered with the event loop.
    #[error("could not register the watch with the event loop")]
    Register(#[source] calloop::Error),
}
