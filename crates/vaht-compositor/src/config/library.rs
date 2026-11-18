//! Named files of one kind, found by name: `styles/<name>.toml`,
//! `layouts/<name>.toml` (02 §10).
//!
//! Each kind has a directory in the configuration directory, where the user's
//! files are, and one installed with vaht; [`Library::find`] looks in the
//! user's first. One lookup serves every kind, so they are found, listed and
//! refused alike.

use std::path::{Path, PathBuf};

use super::ConfigError;

/// One kind of named file, and where it is looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Library {
    /// What the files are called in messages: `style`, `layout`.
    pub(crate) kind: &'static str,
    /// The directory of the user's, inside the configuration directory.
    pub(crate) directory: &'static str,
    /// The directory of the ones installed with vaht.
    system_directory: &'static str,
}

impl Library {
    /// The styles (colors, fonts, textures).
    pub(crate) const STYLES: Self = Self {
        kind: "style",
        directory: "styles",
        system_directory: "/usr/share/vaht/styles",
    };

    /// The layouts (named sets of presentation settings).
    pub(crate) const LAYOUTS: Self = Self {
        kind: "layout",
        directory: "layouts",
        system_directory: "/usr/share/vaht/layouts",
    };

    /// Every kind: the directories the configuration watches.
    pub(crate) const ALL: [Self; 2] = [Self::STYLES, Self::LAYOUTS];

    /// Finds the file `name`: in the kind's directory of the configuration
    /// `directory`, then among the installed ones.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a plain name, or no such file exists.
    pub(crate) fn find(self, directory: &Path, name: &str) -> Result<PathBuf, ConfigError> {
        if name.is_empty() || name.contains(['/', '\0']) || name.starts_with('.') {
            return Err(ConfigError::LibraryName {
                kind: self.kind,
                directory: self.directory,
                name: name.to_owned(),
            });
        }
        let file_name = format!("{name}.toml");
        let searched = [
            directory.join(self.directory).join(&file_name),
            Path::new(self.system_directory).join(&file_name),
        ];
        searched
            .iter()
            .find(|path| path.is_file())
            .cloned()
            .ok_or_else(|| ConfigError::MissingInLibrary {
                kind: self.kind,
                name: name.to_owned(),
                searched: searched.to_vec(),
            })
    }

    /// Returns the names there are, sorted, each once: the user's, in the
    /// configuration `directory`, and the installed ones.
    pub(crate) fn available(self, directory: &Path) -> Vec<String> {
        let directories = [
            directory.join(self.directory),
            PathBuf::from(self.system_directory),
        ];
        let mut names: Vec<String> = directories
            .iter()
            .filter_map(|directory| std::fs::read_dir(directory).ok())
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let is_file = path
                    .extension()
                    .is_some_and(|extension| extension == "toml");
                is_file.then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn a_file_in_the_users_directory_is_found_and_listed() {
        let configuration = tempfile::tempdir().expect("a temporary directory");
        let directory = configuration.path().join("layouts");
        std::fs::create_dir(&directory).expect("a directory");
        std::fs::write(directory.join("minimal.toml"), "").expect("written");
        std::fs::write(directory.join("notes.txt"), "").expect("written");
        assert_eq!(
            Library::LAYOUTS
                .find(configuration.path(), "minimal")
                .expect("found"),
            directory.join("minimal.toml")
        );
        assert_eq!(
            Library::LAYOUTS.available(configuration.path()),
            ["minimal"]
        );
    }

    #[rstest]
    #[case::missing("nowhere", "no layout `nowhere`")]
    #[case::a_path("../settings", "not a layout name")]
    #[case::hidden(".secret", "not a layout name")]
    fn a_name_that_cannot_be_found_is_explained(#[case] name: &str, #[case] expected: &str) {
        let configuration = tempfile::tempdir().expect("a temporary directory");
        let error = Library::LAYOUTS
            .find(configuration.path(), name)
            .expect_err(name);
        assert!(error.to_string().contains(expected), "{error}");
    }

    #[test]
    fn each_kind_has_its_own_directory() {
        let configuration = tempfile::tempdir().expect("a temporary directory");
        std::fs::create_dir(configuration.path().join("styles")).expect("a directory");
        std::fs::write(configuration.path().join("styles").join("a.toml"), "").expect("written");
        assert_eq!(Library::STYLES.available(configuration.path()), ["a"]);
        assert_eq!(
            Library::LAYOUTS.available(configuration.path()),
            Vec::<String>::new()
        );
    }
}
