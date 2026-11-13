//! Themes: `themes/<name>.toml` in the configuration directory, or in the
//! system's theme directory (02 §10).
//!
//! `settings.toml` names the theme; [`ThemeFile`] finds it, the user's copy
//! first, and reads it into a resolved [`Theme`] with the same line-and-
//! column errors as every other configuration file.

use std::io;
use std::path::{Path, PathBuf};

use vaht_ui::Theme;
use vaht_ui::theme::ThemeLayers;

use super::ConfigError;

/// A theme file, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeFile {
    /// Where it is.
    path: PathBuf,
}

impl ThemeFile {
    /// The directory, inside the configuration directory, of the user's
    /// themes.
    pub(crate) const DIRECTORY: &str = "themes";

    /// The directory of themes installed with vaht.
    const SYSTEM_DIRECTORY: &str = "/usr/share/vaht/themes";

    /// Finds the theme `name`: in the `themes` directory of the
    /// configuration `directory`, then among the system's themes.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a plain name, or no such theme
    /// exists.
    pub(crate) fn find(directory: &Path, name: &str) -> Result<Self, ConfigError> {
        if name.is_empty() || name.contains(['/', '\0']) || name.starts_with('.') {
            return Err(ConfigError::ThemeName {
                name: name.to_owned(),
            });
        }
        let file_name = format!("{name}.toml");
        let searched = [
            directory.join(Self::DIRECTORY).join(&file_name),
            Path::new(Self::SYSTEM_DIRECTORY).join(&file_name),
        ];
        searched
            .iter()
            .find(|path| path.is_file())
            .map(|path| Self { path: path.clone() })
            .ok_or_else(|| ConfigError::MissingTheme {
                name: name.to_owned(),
                searched: searched.to_vec(),
            })
    }

    /// Returns the names of the themes there are, sorted, each once: the
    /// user's, in the configuration `directory`, and the system's.
    pub(crate) fn available(directory: &Path) -> Vec<String> {
        let directories = [
            directory.join(Self::DIRECTORY),
            PathBuf::from(Self::SYSTEM_DIRECTORY),
        ];
        let mut names: Vec<String> = directories
            .iter()
            .filter_map(|directory| std::fs::read_dir(directory).ok())
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let is_theme = path
                    .extension()
                    .is_some_and(|extension| extension == "toml");
                is_theme.then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Reads and resolves the theme.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a valid theme.
    pub(crate) fn read(&self) -> Result<Theme, ConfigError> {
        let text =
            std::fs::read_to_string(&self.path).map_err(|error: io::Error| ConfigError::Read {
                path: self.path.clone(),
                error,
            })?;
        let layers: ThemeLayers = toml::from_str(&text)
            .map_err(|error| ConfigError::invalid(self.path.clone(), &text, &error))?;
        Ok(Theme::resolve(&layers))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn a_theme_in_the_configuration_directory_is_found_and_read() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let themes = directory.path().join(ThemeFile::DIRECTORY);
        std::fs::create_dir(&themes).expect("the themes directory");
        std::fs::write(themes.join("mine.toml"), "[taskbar]\nheight = 30\n").expect("written");
        let theme = ThemeFile::find(directory.path(), "mine")
            .and_then(|file| file.read())
            .expect("the theme reads");
        assert_eq!(theme.metrics().taskbar_height, 30);
    }

    #[rstest]
    #[case::missing("nowhere", "no theme `nowhere`")]
    #[case::a_path("../settings", "not a theme name")]
    fn a_theme_that_cannot_be_found_is_explained(#[case] name: &str, #[case] expected: &str) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let error = ThemeFile::find(directory.path(), name).expect_err(name);
        assert!(error.to_string().contains(expected), "{error}");
    }
}
