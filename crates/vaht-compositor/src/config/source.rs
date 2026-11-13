//! Where the configuration comes from, and when it is read again (02 §2).
//!
//! [`ConfigSource`] owns the configuration directory and its watch. A
//! change to a configuration file starts a short timer; once the burst of
//! changes an editor's save makes has settled, the compositor reloads.

use std::path::{Path, PathBuf};
use std::time::Duration;

use smithay::reexports::calloop::LoopHandle;

use super::watch::ConfigWatch;
use super::{Config, ConfigError};
use crate::Compositor;
use crate::event_source::Timer;
use crate::resources::ResourceLedger;

/// The configuration directory, watched for changes while it exists.
#[derive(Debug)]
pub(crate) struct ConfigSource {
    /// Where the configuration files are.
    directory: PathBuf,
    /// Watches the directory, while it exists.
    watch: Option<ConfigWatch>,
    /// Whether the directory was removed or renamed, so the watch sees
    /// nothing any more and must be renewed.
    is_watch_stale: bool,
    /// Reloads the configuration once a burst of file changes settles.
    reload_timer: Option<Timer<Compositor>>,
}

impl ConfigSource {
    /// How long the configuration files must be left alone after a change
    /// before they are read: an editor's save is several changes.
    const RELOAD_DELAY: Duration = Duration::from_millis(200);

    /// The configuration in `directory`, not watched yet.
    pub(crate) const fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            watch: None,
            is_watch_stale: false,
            reload_timer: None,
        }
    }

    /// Returns where the configuration files are.
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Reads and validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns the first problem found, as [`Config::load`] does.
    pub(crate) fn load(&self) -> Result<Config, ConfigError> {
        Config::load(&self.directory)
    }

    /// Watches the directory, unless it already is or the directory does
    /// not exist; a reload tries again.
    pub(crate) fn watch(
        &mut self,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) {
        if self.watch.is_some() && !self.is_watch_stale {
            return;
        }
        match ConfigWatch::start(handle, ledger, &self.directory) {
            Ok(watch) => {
                self.watch = Some(watch);
                self.is_watch_stale = false;
                tracing::debug!(directory = %self.directory.display(), "watching the configuration");
            }
            Err(error) => tracing::debug!(
                error = &error as &dyn std::error::Error,
                directory = %self.directory.display(),
                "not watching the configuration; it reloads on SIGHUP or the reload action"
            ),
        }
    }

    /// Notes that a configuration file changed, or with `is_directory_gone`
    /// that the directory itself went away, and schedules a reload once
    /// things settle.
    pub(crate) fn note_change(
        &mut self,
        is_directory_gone: bool,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) {
        self.is_watch_stale |= is_directory_gone;
        match Timer::once(
            handle,
            ledger,
            Self::RELOAD_DELAY,
            Compositor::reload_configuration,
        ) {
            // Replacing a pending timer cancels it: the burst goes on.
            Ok(timer) => self.reload_timer = Some(timer),
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not schedule a configuration reload"
            ),
        }
    }
}
