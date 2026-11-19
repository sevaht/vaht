//! Noticing that a configuration file changed, without polling (02 §2).
//!
//! One inotify instance watches the configuration directory, and its
//! `styles`, `layouts` and `options` directories (and the directories in
//! `options` and `styles`) when there are any, and is registered with the event
//! loop. Directories are watched rather than files, because editors save by
//! writing a new file and renaming it over the old one, which a watch on
//! the old file would never see.

use std::path::{Path, PathBuf};

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use super::ConfigFile;
use super::library::Library;
use super::options;
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
        // Without a directory of styles or layouts yet, its creation is a
        // change like any other, and the next reload watches it.
        // The directories of styles and layouts, the options' (a directory of
        // choices for each option), and any directory inside `styles`, where a
        // style keeps the choice files it names.
        let mut watched: Vec<PathBuf> = Library::ALL
            .iter()
            .map(|library| directory.join(library.directory))
            .collect();
        watched.push(directory.join(options::DIRECTORY));
        for parent in [
            directory.join(options::DIRECTORY),
            directory.join(Library::STYLES.directory),
        ] {
            watched.extend(
                std::fs::read_dir(parent)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir()),
            );
        }
        let libraries: Vec<_> = watched
            .iter()
            .filter_map(|path| inotify.add_watch(path, Self::EVENTS).ok())
            .collect();
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
                    if libraries.contains(&event.wd) {
                        // A file of the library, or a directory in one (a
                        // new option's, or a style's own).
                        event.mask.contains(AddWatchFlags::IN_ISDIR)
                            || name.is_some_and(|name| {
                                Path::new(name).extension() == Some("toml".as_ref())
                            })
                    } else {
                        name.is_some_and(|name| {
                            ConfigFile::is_named(name)
                                || name == options::DIRECTORY
                                || Library::ALL.iter().any(|library| name == library.directory)
                        })
                    }
                });
                // A directory of styles or layouts appearing or going needs a
                // new watch.
                let libraries_moved = events.iter().any(|event| {
                    if libraries.contains(&event.wd) {
                        event.mask.contains(AddWatchFlags::IN_ISDIR)
                    } else {
                        Library::ALL
                            .iter()
                            .map(|library| library.directory)
                            .chain([options::DIRECTORY])
                            .any(|name| event.name.as_deref() == Some(name.as_ref()))
                    }
                });
                if directory_gone || file_changed {
                    compositor.config_source.note_change(
                        directory_gone || libraries_moved,
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
