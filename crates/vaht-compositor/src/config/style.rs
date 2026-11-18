//! Styles: `styles/<name>.toml` in the configuration directory, or in the
//! system's style directory (02 §10).
//!
//! `settings.toml` names the style; [`StyleFile`] finds it, the user's copy
//! first, and reads it into a resolved [`Style`] with the same line-and-
//! column errors as every other configuration file.

use std::io;
use std::path::{Path, PathBuf};

use vaht_ui::Style;
use vaht_ui::style::StyleLayers;

use super::ConfigError;

/// A style file, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StyleFile {
    /// Where it is.
    path: PathBuf,
}

impl StyleFile {
    /// The directory, inside the configuration directory, of the user's
    /// styles.
    pub(crate) const DIRECTORY: &str = "styles";

    /// The directory of styles installed with vaht.
    const SYSTEM_DIRECTORY: &str = "/usr/share/vaht/styles";

    /// Finds the style `name`: in the `styles` directory of the
    /// configuration `directory`, then among the system's styles.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a plain name, or no such style
    /// exists.
    pub(crate) fn find(directory: &Path, name: &str) -> Result<Self, ConfigError> {
        if name.is_empty() || name.contains(['/', '\0']) || name.starts_with('.') {
            return Err(ConfigError::StyleName {
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
            .ok_or_else(|| ConfigError::MissingStyle {
                name: name.to_owned(),
                searched: searched.to_vec(),
            })
    }

    /// Reads the style `name`: as [`StyleFile::find`] finds it, or, for the
    /// default style, the copy built in when no file of it is found (it is
    /// installed for people to read and copy, so missing it is no loss).
    ///
    /// # Errors
    ///
    /// Returns an error if the style cannot be found, read, or is not valid.
    pub(crate) fn load(directory: &Path, name: &str) -> Result<Style, ConfigError> {
        match Self::find(directory, name) {
            Err(ConfigError::MissingStyle { .. }) if name == Style::DEFAULT_NAME => {
                Ok(Style::built_in())
            }
            found => found?.read(),
        }
    }

    /// Returns the names of the styles there are, sorted, each once: the
    /// user's, in the configuration `directory`, and the system's, and the
    /// default style, which is always there.
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
                let is_style = path
                    .extension()
                    .is_some_and(|extension| extension == "toml");
                is_style.then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
            })
            .collect();
        names.push(Style::DEFAULT_NAME.to_owned());
        names.sort();
        names.dedup();
        names
    }

    /// Reads and resolves the style.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a valid style.
    pub(crate) fn read(&self) -> Result<Style, ConfigError> {
        let text =
            std::fs::read_to_string(&self.path).map_err(|error: io::Error| ConfigError::Read {
                path: self.path.clone(),
                error,
            })?;
        let layers: StyleLayers = toml::from_str(&text)
            .map_err(|error| ConfigError::invalid(self.path.clone(), &text, &error))?;
        Ok(Style::resolve(&layers))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn a_style_in_the_configuration_directory_is_found_and_read() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let styles = directory.path().join(StyleFile::DIRECTORY);
        std::fs::create_dir(&styles).expect("the styles directory");
        std::fs::write(styles.join("mine.toml"), "[taskbar]\nheight = 30\n").expect("written");
        let style = StyleFile::find(directory.path(), "mine")
            .and_then(|file| file.read())
            .expect("the style reads");
        assert_eq!(style.metrics().taskbar_height, 30);
    }

    #[rstest]
    #[case::missing("nowhere", "no style `nowhere`")]
    #[case::a_path("../settings", "not a style name")]
    fn a_style_that_cannot_be_found_is_explained(#[case] name: &str, #[case] expected: &str) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let error = StyleFile::find(directory.path(), name).expect_err(name);
        assert!(error.to_string().contains(expected), "{error}");
    }

    #[test]
    fn the_default_style_needs_no_file_and_is_always_listed() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let loaded = StyleFile::load(directory.path(), Style::DEFAULT_NAME).expect("built in");
        assert_eq!(
            loaded,
            Style::built_in(),
            "the copy built in stands in for the file"
        );
        assert!(
            StyleFile::available(directory.path()).contains(&Style::DEFAULT_NAME.to_owned()),
            "chooseable from a menu even when no file is installed"
        );
        assert!(
            StyleFile::load(directory.path(), "no-such-style").is_err(),
            "any other missing style is still an error"
        );
    }

    #[test]
    fn a_file_named_for_the_default_style_wins_over_the_copy_built_in() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let styles = directory.path().join(StyleFile::DIRECTORY);
        std::fs::create_dir(&styles).expect("a directory");
        std::fs::write(styles.join("vaht.toml"), "[window.titlebar]\nheight = 33\n")
            .expect("written");
        let loaded = StyleFile::load(directory.path(), Style::DEFAULT_NAME).expect("loads");
        assert_eq!(
            loaded.metrics().titlebar_height,
            33,
            "the user's own vaht.toml is used"
        );
    }
}
