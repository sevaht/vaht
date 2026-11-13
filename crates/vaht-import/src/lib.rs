//! Converts a Fluxbox setup to vaht's configuration (02 §11).
//!
//! [`FluxboxSetup::read`] reads a Fluxbox configuration directory (`~/.fluxbox`,
//! or my fluxland one: the format is the same), following the file paths its
//! `init` names. [`FluxboxSetup::convert`] turns it into vaht's files, and
//! [`Conversion::write`] writes them, never over existing files unless asked,
//! and checks the result as `vaht check` would. The source is never changed.
//! Every line that is not converted is in the [`Report`], with why.
//!
//! Converted today: `init` to `settings.toml`, `keys` to `bindings.toml`,
//! `apps` to `rules.toml`. `menu`, the style and `startup` are reported until
//! vaht has menus and themes.

mod apps;
mod command;
mod keys;
mod report;
mod resources;
mod settings;

use std::io;
use std::path::{Path, PathBuf};

pub use report::{Note, Reason, Report};

use crate::apps::AppsFile;
use crate::keys::KeysFile;
use crate::resources::Resources;
use crate::settings::InitFile;

/// A Fluxbox configuration directory, read.
#[derive(Debug)]
pub struct FluxboxSetup {
    /// The directory.
    directory: PathBuf,
    /// The `init` resources.
    init: Resources,
    /// The `keys` file: where, and what it says.
    keys: Option<(PathBuf, String)>,
    /// The `apps` file: where, and what it says.
    apps: Option<(PathBuf, String)>,
    /// The other files vaht cannot convert yet, if they exist.
    unconverted: Vec<(PathBuf, &'static str)>,
}

impl FluxboxSetup {
    /// Reads the setup in `directory`.
    ///
    /// # Errors
    ///
    /// Returns an error if `directory` has no `init`, or a file cannot be
    /// read.
    pub fn read(directory: &Path) -> Result<Self, ImportError> {
        let init_path = directory.join("init");
        let init_text = std::fs::read_to_string(&init_path).map_err(|error| ImportError::Read {
            path: init_path.clone(),
            error,
        })?;
        let mut init = Resources::parse(&init_path, &init_text);
        let mut file = |resource: &str, default: &str| {
            init.value(resource).map_or_else(
                || directory.join(default),
                |path| directory.join(Self::expand_home(path)),
            )
        };
        let keys_path = file("session.keyFile", "keys");
        let apps_path = file("session.appsFile", "apps");
        let unconverted = [
            (
                file("session.menuFile", "menu"),
                "menus: vaht has no menus yet",
            ),
            (
                file("session.screen0.windowMenu", "windowmenu"),
                "the window menu: vaht has no menus yet",
            ),
            (
                file("session.styleFile", "styles"),
                "the style: vaht has no themes yet",
            ),
            (
                directory.join("startup"),
                "the startup script: add it to session.autostart if vaht should run it",
            ),
        ]
        .into_iter()
        .filter(|(path, _)| path.exists())
        .collect();
        Ok(Self {
            directory: directory.to_path_buf(),
            init,
            keys: Self::read_optional(keys_path)?,
            apps: Self::read_optional(apps_path)?,
            unconverted,
        })
    }

    /// Reads the file at `path`, if it exists.
    fn read_optional(path: PathBuf) -> Result<Option<(PathBuf, String)>, ImportError> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some((path, text))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ImportError::Read { path, error }),
        }
    }

    /// Converts the setup to vaht's files.
    #[must_use]
    pub fn convert(self) -> Conversion {
        let mut report = Report::default();
        let header = |source: &Path| {
            format!(
                "# Converted from {} by vaht import fluxbox.\n\n",
                source.display()
            )
        };
        let mut files = Vec::new();
        let init_path = self.init.path().to_path_buf();
        let settings = InitFile::new(self.init).convert(&mut report);
        files.push(OutputFile {
            name: "settings.toml",
            text: header(&init_path) + &settings.to_string(),
        });
        if let Some((path, text)) = &self.keys {
            let bindings = KeysFile::new(path, text).convert(&mut report);
            files.push(OutputFile {
                name: "bindings.toml",
                text: header(path) + &bindings.to_string(),
            });
        }
        if let Some((path, text)) = &self.apps {
            let rules = AppsFile::new(path, text).convert(&mut report);
            files.push(OutputFile {
                name: "rules.toml",
                text: header(path) + &rules.to_string(),
            });
        }
        for (path, why) in &self.unconverted {
            report.note(path, 1, Reason::NotYetInVaht, *why);
        }
        Conversion {
            source: self.directory,
            files,
            report,
        }
    }

    /// Expands a leading `~/` to the home directory, as Fluxbox does; a
    /// relative result is then taken from the setup's directory.
    fn expand_home(path: &str) -> PathBuf {
        match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => Path::new(&home).join(rest),
            (Some(_) | None, _) => PathBuf::from(path),
        }
    }
}

/// One file of vaht's configuration, converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// Its name in the configuration directory.
    pub name: &'static str,
    /// Its contents.
    pub text: String,
}

/// A converted setup, ready to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    /// The directory converted.
    source: PathBuf,
    /// The files.
    files: Vec<OutputFile>,
    /// What was not converted.
    report: Report,
}

impl Conversion {
    /// Returns the converted files.
    #[must_use]
    pub fn files(&self) -> &[OutputFile] {
        &self.files
    }

    /// Returns what was not converted, and why.
    #[must_use]
    pub const fn report(&self) -> &Report {
        &self.report
    }

    /// Writes the files into `directory`, creating it if needed. Existing
    /// files are replaced only with `replace`. The result is then read as
    /// vaht reads it.
    ///
    /// # Errors
    ///
    /// Returns an error if a file exists and `replace` is false, a file
    /// cannot be written, the target is the source, or vaht would not
    /// accept the result.
    pub fn write(&self, directory: &Path, replace: bool) -> Result<(), ImportError> {
        if directory == self.source {
            return Err(ImportError::SameDirectory(directory.to_path_buf()));
        }
        if !replace
            && let Some(existing) = self
                .files
                .iter()
                .map(|file| directory.join(file.name))
                .find(|path| path.exists())
        {
            return Err(ImportError::Exists(existing));
        }
        std::fs::create_dir_all(directory).map_err(|error| ImportError::Write {
            path: directory.to_path_buf(),
            error,
        })?;
        for file in &self.files {
            let path = directory.join(file.name);
            std::fs::write(&path, &file.text)
                .map_err(|error| ImportError::Write { path, error })?;
        }
        vaht_compositor::Config::load(directory).map_err(ImportError::Invalid)?;
        Ok(())
    }
}

/// Why an import failed.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// A source file could not be read.
    #[error("could not read {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: io::Error,
    },
    /// A converted file could not be written.
    #[error("could not write {}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: io::Error,
    },
    /// A file to write exists already.
    #[error("{} exists; pass --force to replace it", .0.display())]
    Exists(PathBuf),
    /// The target directory is the source directory.
    #[error("{} is the setup being converted; choose another directory", .0.display())]
    SameDirectory(PathBuf),
    /// The written configuration is not valid: a bug in the conversion.
    #[error("the converted configuration is not valid (a bug in vaht import)")]
    Invalid(#[source] vaht_compositor::ConfigError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_converts_and_writes_a_valid_configuration() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            source.path().join("init"),
            "session.screen0.windowPlacement: CascadePlacement\nsession.keyFile: keys\n",
        )
        .expect("written");
        std::fs::write(
            source.path().join("keys"),
            "Mod4 T :Exec kitty\nOnDesktop Mouse3 :RootMenu\n",
        )
        .expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        let target = tempfile::tempdir().expect("a temporary directory");
        conversion
            .write(target.path(), false)
            .expect("written and valid");
        assert!(
            conversion.write(target.path(), false).is_err(),
            "existing files are kept"
        );
        conversion
            .write(target.path(), true)
            .expect("replaced on request");
        let bindings =
            std::fs::read_to_string(target.path().join("bindings.toml")).expect("written");
        assert!(
            bindings.contains("\"Super+T\" = \"exec kitty\""),
            "{bindings}"
        );
        let notes: Vec<_> = conversion
            .report()
            .notes()
            .iter()
            .map(|note| note.line)
            .collect();
        assert_eq!(notes, [2], "the root menu line: {}", conversion.report());
    }
}
