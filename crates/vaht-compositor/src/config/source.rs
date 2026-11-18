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

    /// Writes each of `settings`, a key's parts and a TOML value, into
    /// `settings.toml`, keeping the rest of the file as it is; the watch
    /// then reloads the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `settings.toml` cannot be read, parsed or
    /// written.
    pub(crate) fn write_settings(
        &self,
        settings: &[(&[String], Option<&str>)],
    ) -> Result<(), ConfigError> {
        let path = self.directory.join("settings.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(ConfigError::Read { path, error }),
        };
        let invalid = |error: &toml_edit::TomlError, text: &str| {
            let (line, column) = error
                .span()
                .map_or((1, 1), |span| Self::line_and_column(text, span.start));
            ConfigError::Invalid {
                path: path.clone(),
                line,
                column,
                message: error.message().to_owned(),
            }
        };
        let mut document: toml_edit::DocumentMut =
            text.parse().map_err(|error| invalid(&error, &text))?;
        for (key, value) in settings {
            match value {
                Some(value) => {
                    let value: toml_edit::Value =
                        value.parse().map_err(|error| invalid(&error, value))?;
                    Self::write_setting(&mut document, key, value);
                }
                None => Self::remove_setting(&mut document, key),
            }
        }
        // Written beside and renamed over, so a reader never sees half.
        let temporary = path.with_extension("toml.vaht-new");
        std::fs::write(&temporary, document.to_string())
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|error| ConfigError::Write { path, error })
    }

    /// Takes `key`, its parts, out of `document`, if it is there: the setting
    /// is back to its default.
    fn remove_setting(document: &mut toml_edit::DocumentMut, key: &[String]) {
        let Some((last, path)) = key.split_last() else {
            return;
        };
        let mut table: Option<&mut dyn toml_edit::TableLike> = Some(document.as_table_mut());
        for part in path {
            table = table.and_then(|table| table.get_mut(part)?.as_table_like_mut());
        }
        if let Some(table) = table {
            table.remove(last);
        }
    }

    /// Sets `key`, its parts, to `value` in `document`, making the tables
    /// on the way.
    fn write_setting(
        document: &mut toml_edit::DocumentMut,
        key: &[String],
        value: toml_edit::Value,
    ) {
        let Some((last, path)) = key.split_last() else {
            return;
        };
        if path.is_empty() {
            let is_new = !document.contains_key(last);
            Self::insert_keeping_comment(document.as_table_mut(), last, value);
            if is_new {
                Self::keep_leading_comment_first(document, last);
            }
            return;
        }
        let mut table: &mut dyn toml_edit::TableLike = document.as_table_mut();
        for part in path {
            let item = table.entry(part).or_insert_with(|| {
                // Shown only once it holds a value itself.
                let mut new = toml_edit::Table::new();
                new.set_implicit(true);
                toml_edit::Item::Table(new)
            });
            if item.as_table_like().is_none() {
                *item = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let Some(next) = item.as_table_like_mut() else {
                return;
            };
            table = next;
        }
        Self::insert_keeping_comment(table, last, value);
    }

    /// Sets `name` in `table` to `value`, keeping the comment beside the
    /// value it replaces.
    fn insert_keeping_comment(
        table: &mut dyn toml_edit::TableLike,
        name: &str,
        mut value: toml_edit::Value,
    ) {
        if let Some(replaced) = table.get(name).and_then(toml_edit::Item::as_value) {
            *value.decor_mut() = replaced.decor().clone();
        }
        table.insert(name, toml_edit::Item::Value(value));
    }

    /// Moves the file's opening comment, which belongs to its first table,
    /// onto the top-level key `name` just added: a key is written above
    /// every table, and the comment would otherwise follow it.
    fn keep_leading_comment_first(document: &mut toml_edit::DocumentMut, name: &str) {
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
        if let Some(mut key) = document.key_mut(name) {
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

    /// Reads `settings.toml` as a table, or its built-in version.
    ///
    /// # Errors
    ///
    /// Returns an error if it cannot be read or is not TOML.
    pub(crate) fn settings_table(&self) -> Result<toml::Table, ConfigError> {
        let [_, settings, ..] = Config::FILES;
        settings.read_table(&self.directory)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_written_into_their_tables_keeping_the_rest() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "# Mine.\n[focus]\nmodel = \"click\" # how\n").expect("written");
        let source = ConfigSource::new(directory.path().to_path_buf());
        let key = |text: &str| text.split('.').map(str::to_owned).collect::<Vec<_>>();
        let (theme, model, delay, speed) = (
            key("theme"),
            key("focus.model"),
            key("taskbar.auto_hide_delay_ms"),
            vec![
                "input".to_owned(),
                "device".to_owned(),
                "My Mouse".to_owned(),
                "accel_speed".to_owned(),
            ],
        );
        source
            .write_settings(&[
                (&theme, Some("\"mine\"")),
                (&model, Some("\"sloppy\"")),
                (&delay, Some("250")),
                (&speed, Some("0.5")),
            ])
            .expect("written");
        let written = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            written,
            "# Mine.\ntheme = \"mine\"\n\n[focus]\nmodel = \"sloppy\" # how\n\n[taskbar]\n\
             auto_hide_delay_ms = 250\n\n[input.device.\"My Mouse\"]\naccel_speed = 0.5\n"
        );
    }

    #[test]
    fn a_setting_put_back_to_its_default_leaves_the_file_but_not_its_comments() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("settings.toml");
        std::fs::write(
            &path,
            "# Mine.\n[focus]\nmodel = \"click\" # how\nauto_raise = true # why\n\n[blur]\npasses = 5\n",
        )
        .expect("written");
        let source = ConfigSource::new(directory.path().to_path_buf());
        let key = |text: &str| text.split('.').map(str::to_owned).collect::<Vec<_>>();
        let (model, passes, missing) =
            (key("focus.model"), key("blur.passes"), key("taskbar.rows"));
        source
            .write_settings(&[(&model, None), (&passes, None), (&missing, None)])
            .expect("written");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "# Mine.\n[focus]\nauto_raise = true # why\n\n[blur]\n",
            "the keys are gone; the comment of the one kept, and the file's own, stay"
        );
    }
}
