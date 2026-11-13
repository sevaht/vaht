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
    /// Whether the directory, or its themes directory, came or went since
    /// the watch began, so the watch must be renewed.
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

    /// Writes `theme = "<name>"` into `settings.toml`, keeping the rest of
    /// the file as it is; the watch then reloads the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `settings.toml` cannot be read, parsed or
    /// written.
    pub(crate) fn write_theme(&self, name: &str) -> Result<(), ConfigError> {
        let path = self.directory.join("settings.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(ConfigError::Read { path, error }),
        };
        let mut document: toml_edit::DocumentMut =
            text.parse().map_err(|error: toml_edit::TomlError| {
                let (line, column) = error
                    .span()
                    .map_or((1, 1), |span| Self::line_and_column(&text, span.start));
                ConfigError::Invalid {
                    path: path.clone(),
                    line,
                    column,
                    message: error.message().to_owned(),
                }
            })?;
        let is_new = !document.contains_key("theme");
        document.insert("theme", toml_edit::value(name));
        if is_new {
            Self::keep_leading_comment_first(&mut document);
        }
        // Written beside and renamed over, so a reader never sees half.
        let temporary = path.with_extension("toml.vaht-new");
        std::fs::write(&temporary, document.to_string())
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|error| ConfigError::Write { path, error })
    }

    /// Moves the file's opening comment, which belongs to its first table,
    /// onto the `theme` key just added: a key is written above every table,
    /// and the comment would otherwise follow it.
    fn keep_leading_comment_first(document: &mut toml_edit::DocumentMut) {
        let first_table = document
            .iter_mut()
            .filter_map(|(_, item)| item.as_table_mut())
            .min_by_key(|table| table.position().unwrap_or(isize::MAX));
        let Some(table) = first_table else {
            return;
        };
        let leading = table
            .decor()
            .prefix()
            .and_then(|prefix| prefix.as_str())
            .unwrap_or_default()
            .to_owned();
        if !leading.trim_start().starts_with('#') {
            return;
        }
        table.decor_mut().set_prefix("\n");
        if let Some(mut key) = document.key_mut("theme") {
            key.leaf_decor_mut().set_prefix(leading);
        }
    }

    /// Returns the line and column, from 1, of byte `offset` in `text`.
    fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
        let before = &text[..offset.min(text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before
            .rfind('\n')
            .map_or(before.len(), |newline| before.len() - newline - 1)
            + 1;
        (line, column)
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

    /// Notes that a configuration or theme file changed, and with
    /// `needs_new_watch` that the directory or its themes directory came or
    /// went, and schedules a reload once things settle.
    pub(crate) fn note_change(
        &mut self,
        needs_new_watch: bool,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) {
        self.is_watch_stale |= needs_new_watch;
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
