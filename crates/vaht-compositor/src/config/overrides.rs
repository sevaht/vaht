//! Settings changed while vaht runs (`set-theme`), kept apart from the
//! files until `save-settings` writes them there (00 §3, `SaveSettings`).
//!
//! A change lasts across reloads, so editing another file does not undo
//! it; but if the user edits the same setting in its file, the file wins
//! and the change is dropped.

use std::path::Path;

use super::{Config, ConfigError, ConfigSource, ThemeFile};

/// The settings changed in place, not (yet) in the files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SettingOverrides {
    /// The theme chosen with `set-theme`, and what `settings.toml` said
    /// when it was chosen.
    theme: Option<ThemeOverride>,
}

/// A theme chosen in place.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeOverride {
    /// The theme chosen.
    name: String,
    /// The theme `settings.toml` named then; when the file names another,
    /// the user changed it there, and the file wins.
    file_said: Option<String>,
}

impl SettingOverrides {
    /// Chooses the theme `name` in place of what `config` (as the files
    /// and earlier changes made it) has, reading it from `directory`.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no such theme or it is not valid; then
    /// nothing changes.
    pub(crate) fn set_theme(
        &mut self,
        name: &str,
        config: &mut Config,
        directory: &Path,
    ) -> Result<(), ConfigError> {
        let theme = ThemeFile::find(directory, name)?.read()?;
        let file_said = match &self.theme {
            Some(chosen) => chosen.file_said.clone(),
            None => config.settings.theme.clone(),
        };
        self.theme = Some(ThemeOverride {
            name: name.to_owned(),
            file_said,
        });
        config.theme = theme;
        config.settings.theme = Some(name.to_owned());
        Ok(())
    }

    /// Applies the changes to `config`, just read from the files in
    /// `directory`; a change whose setting the files now say otherwise
    /// than when it was made is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if a chosen theme can no longer be read; the change
    /// is dropped, and the files' configuration stays.
    pub(crate) fn apply(
        &mut self,
        config: &mut Config,
        directory: &Path,
    ) -> Result<(), ConfigError> {
        let Some(chosen) = &self.theme else {
            return Ok(());
        };
        if config.settings.theme != chosen.file_said {
            self.theme = None;
            return Ok(());
        }
        let name = chosen.name.clone();
        match ThemeFile::find(directory, &name).and_then(|file| file.read()) {
            Ok(theme) => {
                config.theme = theme;
                config.settings.theme = Some(name);
                Ok(())
            }
            Err(error) => {
                self.theme = None;
                Err(error)
            }
        }
    }

    /// Writes the changes into the files `source` reads, and forgets them:
    /// the files say them now.
    ///
    /// # Errors
    ///
    /// Returns an error if a file cannot be read, parsed or written; the
    /// changes are kept.
    pub(crate) fn save(&mut self, source: &ConfigSource) -> Result<(), ConfigError> {
        if let Some(chosen) = &self.theme {
            source.write_theme(&chosen.name)?;
        }
        self.theme = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration directory with themes `one` and `two`.
    fn directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let themes = directory.path().join(ThemeFile::DIRECTORY);
        std::fs::create_dir_all(&themes).expect("a directory");
        for name in ["one", "two"] {
            std::fs::write(
                themes.join(format!("{name}.toml")),
                "[taskbar]\nheight = 30\n",
            )
            .expect("a theme");
        }
        directory
    }

    #[test]
    fn a_chosen_theme_outlasts_reloads_until_the_file_names_another() {
        let directory = directory();
        let mut overrides = SettingOverrides::default();
        let mut config = Config::built_in();
        overrides
            .set_theme("one", &mut config, directory.path())
            .expect("a theme");
        assert_eq!(config.settings.theme.as_deref(), Some("one"));
        let mut reloaded = Config::built_in();
        overrides
            .apply(&mut reloaded, directory.path())
            .expect("still there");
        assert_eq!(reloaded.settings.theme.as_deref(), Some("one"), "kept");
        let mut edited = Config::built_in();
        edited.settings.theme = Some("two".to_owned());
        overrides
            .apply(&mut edited, directory.path())
            .expect("dropped");
        assert_eq!(
            edited.settings.theme.as_deref(),
            Some("two"),
            "the file wins"
        );
        assert_eq!(overrides, SettingOverrides::default());
    }

    #[test]
    fn an_unknown_theme_changes_nothing() {
        let directory = directory();
        let mut overrides = SettingOverrides::default();
        let mut config = Config::built_in();
        assert!(
            overrides
                .set_theme("three", &mut config, directory.path())
                .is_err()
        );
        assert_eq!(config.settings.theme, None);
        assert_eq!(overrides, SettingOverrides::default());
    }
}
