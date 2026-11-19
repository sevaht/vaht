//! Styles: `styles/<name>.toml` in the configuration directory, or in the
//! system's style directory (02 §10).
//!
//! `settings.toml` names the style; [`StyleFile`] finds it, the user's copy
//! first, and reads it into a resolved [`Style`] with the same line-and-
//! column errors as every other configuration file.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use vaht_ui::Style;
use vaht_ui::style::StyleLayers;
use vaht_ui::style::options::Selection;

use super::ConfigError;
use super::library::Library;
use super::options::OptionLibrary;

/// A style file, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StyleFile {
    /// Where it is.
    path: PathBuf,
}

impl StyleFile {
    /// Finds the style `name`: in the `styles` directory of the
    /// configuration `directory`, then among the system's styles.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a plain name, or no such style
    /// exists.
    pub(crate) fn find(directory: &Path, name: &str) -> Result<Self, ConfigError> {
        Library::STYLES
            .find(directory, name)
            .map(|path| Self { path })
    }

    /// Reads the style `name`: as [`StyleFile::find`] finds it, or, for the
    /// default style, the copy built in when no file of it is found (it is
    /// installed for people to read and copy, so missing it is no loss).
    ///
    /// # Errors
    ///
    /// Returns an error if the style cannot be found, read, or is not valid.
    pub(crate) fn load(
        directory: &Path,
        name: &str,
        chosen: &BTreeMap<String, String>,
    ) -> Result<Style, ConfigError> {
        match Self::find(directory, name) {
            Err(ConfigError::MissingInLibrary { .. }) if name == Style::DEFAULT_NAME => {
                Ok(Style::built_in_selecting(&Selection {
                    directory: None,
                    chosen,
                    library: &OptionLibrary::new(directory),
                }))
            }
            found => found?.read(directory, chosen),
        }
    }

    /// Returns the names of the styles there are, sorted, each once: the
    /// user's, in the configuration `directory`, and the system's, and the
    /// default style, which is always there.
    pub(crate) fn available(directory: &Path) -> Vec<String> {
        Library::STYLES.available(directory)
    }

    /// Reads and resolves the style.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a valid style.
    pub(crate) fn read(
        &self,
        configuration: &Path,
        chosen: &BTreeMap<String, String>,
    ) -> Result<Style, ConfigError> {
        let text =
            std::fs::read_to_string(&self.path).map_err(|error: io::Error| ConfigError::Read {
                path: self.path.clone(),
                error,
            })?;
        // The images and choice files a style names are beside it.
        let layers = StyleLayers::from_toml_selecting(
            &text,
            &Selection {
                directory: self.path.parent(),
                chosen,
                library: &OptionLibrary::new(configuration),
            },
        )
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
        let styles = directory.path().join(Library::STYLES.directory);
        std::fs::create_dir(&styles).expect("the styles directory");
        std::fs::write(styles.join("mine.toml"), "[taskbar]\nheight = 30\n").expect("written");
        let style = StyleFile::find(directory.path(), "mine")
            .and_then(|file| file.read(directory.path(), &BTreeMap::new()))
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
        let loaded = StyleFile::load(directory.path(), Style::DEFAULT_NAME, &BTreeMap::new())
            .expect("built in");
        // Where vaht is installed, the installed file is found first, and it
        // is whatever version was installed: this machine's, not this
        // source's. Without it, the copy built in stands in.
        if !Path::new("/usr/share/vaht/styles/default.toml").exists() {
            assert_eq!(
                loaded,
                Style::built_in(),
                "the copy built in stands in for the file"
            );
        }
        assert!(
            StyleFile::available(directory.path()).contains(&Style::DEFAULT_NAME.to_owned()),
            "chooseable from a menu even when no file is installed"
        );
        assert!(
            StyleFile::load(directory.path(), "no-such-style", &BTreeMap::new()).is_err(),
            "any other missing style is still an error"
        );
    }

    #[test]
    fn a_file_named_for_the_default_style_wins_over_the_copy_built_in() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let styles = directory.path().join(Library::STYLES.directory);
        std::fs::create_dir(&styles).expect("a directory");
        std::fs::write(
            styles.join("default.toml"),
            "[window.titlebar]\nheight = 33\n",
        )
        .expect("written");
        let loaded = StyleFile::load(directory.path(), Style::DEFAULT_NAME, &BTreeMap::new())
            .expect("loads");
        assert_eq!(
            loaded.metrics().titlebar_height,
            33,
            "the user's own default.toml is used"
        );
    }
}
