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

    /// The name of the one of each kind that is built in, whose file is
    /// optional: the default style and layout.
    pub(crate) const DEFAULT_NAME: &'static str = vaht_ui::Style::DEFAULT_NAME;

    /// Returns the names there are, each once: the default first (always
    /// there, file or not), then the user's, in the configuration
    /// `directory`, and the installed ones, sorted.
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
        names.retain(|name| name != Self::DEFAULT_NAME);
        names.sort();
        names.dedup();
        names.insert(0, Self::DEFAULT_NAME.to_owned());
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
        // What is installed on this machine may be listed too.
        assert!(
            Library::LAYOUTS
                .available(configuration.path())
                .contains(&"minimal".to_owned())
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
        assert!(
            Library::STYLES
                .available(configuration.path())
                .contains(&"a".to_owned())
        );
        assert!(
            !Library::LAYOUTS
                .available(configuration.path())
                .contains(&"a".to_owned())
        );
    }

    #[test]
    fn the_default_is_always_listed_and_first() {
        let configuration = tempfile::tempdir().expect("a temporary directory");
        assert_eq!(
            Library::LAYOUTS.available(configuration.path())[0],
            "vaht",
            "listed with no files at all"
        );
        let layouts = configuration.path().join("layouts");
        std::fs::create_dir(&layouts).expect("a directory");
        for name in ["zebra", "apple", "vaht"] {
            std::fs::write(layouts.join(format!("{name}.toml")), "").expect("written");
        }
        let names = Library::LAYOUTS.available(configuration.path());
        assert_eq!(&names[..3], ["vaht", "apple", "zebra"], "{names:?}");
        assert_eq!(
            names.iter().filter(|name| *name == "vaht").count(),
            1,
            "once, though a file has the name too"
        );
    }
}
