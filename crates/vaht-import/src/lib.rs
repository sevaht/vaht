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
//! `apps` to `rules.toml`, `menu` and `windowmenu` to `menus.toml`, and the
//! style `init` names to `themes/<name>.toml`, which `settings.toml` then
//! chooses. The wallpaper `startup` sets becomes `[wallpaper]`; the rest of
//! `startup` is reported: its commands belong in `session.autostart`.

mod apps;
mod command;
mod input;
mod keys;
mod menu;
mod outputs;
mod pattern;
mod report;
mod resources;
mod settings;
mod style;
mod wallpaper;

use std::io;
use std::path::{Path, PathBuf};

pub use pattern::RUN_DIALOG_APP_ID;
pub use report::{Note, Reason, Report};

use crate::apps::AppsFile;
use crate::keys::KeysFile;
use crate::menu::MenuFiles;
use crate::resources::Resources;
use crate::settings::InitFile;
use crate::style::StyleFile;
use crate::wallpaper::StartupWallpaper;

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
    /// The style `init` names: where, and what it says.
    style: Option<(PathBuf, String)>,
    /// The root menu: where, and what it says.
    menu: Option<(PathBuf, String)>,
    /// The window menu: where, and what it says.
    window_menu: Option<(PathBuf, String)>,
    /// The `startup` script: where, and what it says.
    startup: Option<(PathBuf, String)>,
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
        // A style is a file, or a directory holding theme.cfg.
        let style_path = init
            .value("session.styleFile")
            .map(|path| directory.join(Self::expand_home(path)))
            .map(|path| {
                if path.is_dir() {
                    path.join("theme.cfg")
                } else {
                    path
                }
            });
        let mut file = |resource: &str, default: &str| {
            init.value(resource).map_or_else(
                || directory.join(default),
                |path| directory.join(Self::expand_home(path)),
            )
        };
        let keys_path = file("session.keyFile", "keys");
        let apps_path = file("session.appsFile", "apps");
        let menu_path = file("session.menuFile", "menu");
        let window_menu_path = file("session.screen0.windowMenu", "windowmenu");
        Ok(Self {
            directory: directory.to_path_buf(),
            init,
            keys: Self::read_optional(keys_path)?,
            apps: Self::read_optional(apps_path)?,
            style: style_path.map(Self::read_optional).transpose()?.flatten(),
            menu: Self::read_optional(menu_path)?,
            window_menu: Self::read_optional(window_menu_path)?,
            startup: Self::read_optional(directory.join("startup"))?,
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
        let init = InitFile::new(self.init).convert(&mut report);
        let mut settings = init.settings;
        if let Some(outputs) = init.outputs {
            files.push(OutputFile {
                name: "outputs.toml".to_owned(),
                text: header(&init_path) + &outputs.to_string(),
            });
        }
        if let Some((path, text)) = &self.style {
            let name = Self::theme_name(path);
            let mut theme = StyleFile::new(path, text).convert(&mut report);
            Self::place_alphas(&init.alphas, Some(&mut theme), &init_path, &mut report);
            settings.insert("theme", toml_edit::value(name.clone()));
            files.push(OutputFile {
                name: format!("themes/{name}.toml"),
                text: header(path) + &theme.to_string(),
            });
        }
        if self.style.is_none() {
            Self::place_alphas(&init.alphas, None, &init_path, &mut report);
        }
        if let Some((path, text)) = &self.startup {
            let why = match StartupWallpaper::find(text) {
                Some(wallpaper) => {
                    settings.insert("wallpaper", wallpaper.settings());
                    "the startup script, but for its wallpaper: add the rest to \
                     session.autostart if vaht should run it"
                }
                None => "the startup script: add it to session.autostart if vaht should run it",
            };
            report.note(path, 1, Reason::NotYetInVaht, why);
        }
        files.insert(
            0,
            OutputFile {
                name: "settings.toml".to_owned(),
                text: header(&init_path) + &settings.to_string(),
            },
        );
        if let Some((path, text)) = &self.keys {
            let mut bindings = KeysFile::new(path, text).convert(&mut report);
            if !init.switches.is_empty() {
                let mut switches = toml_edit::Table::new();
                for (switch, command, _) in init.switches {
                    switches.insert(switch, toml_edit::Item::Value(command));
                }
                bindings.insert("switches", toml_edit::Item::Table(switches));
            }
            files.push(OutputFile {
                name: "bindings.toml".to_owned(),
                text: header(path) + &bindings.to_string(),
            });
        } else {
            // bindings.toml replaces the built-in bindings whole, so it is
            // not written for the switches alone.
            for (switch, command, line) in init.switches {
                report.note(
                    &init_path,
                    line,
                    Reason::NotYetInVaht,
                    format!(
                        "{switch} = {command}: there is no keys file, so add it to \
                         [switches] in bindings.toml by hand"
                    ),
                );
            }
        }
        if let Some((path, text)) = &self.menu {
            let mut menus = MenuFiles::new();
            menus.convert_root(path, text, &mut report);
            if let Some((path, text)) = &self.window_menu {
                menus.convert_window(path, text, &mut report);
            }
            files.push(OutputFile {
                name: "menus.toml".to_owned(),
                text: header(path) + &menus.document().to_string(),
            });
        }
        if let Some((path, text)) = &self.apps {
            let rules = AppsFile::new(path, text).convert(&mut report);
            files.push(OutputFile {
                name: "rules.toml".to_owned(),
                text: header(path) + &rules.to_string(),
            });
        }
        Conversion {
            source: self.directory,
            files,
            report,
        }
    }

    /// Puts the `alphas` `init` (at `init_path`) sets into the converted
    /// `theme`; without one, notes in `report` that they are left out.
    fn place_alphas(
        alphas: &[(&str, i64, usize)],
        theme: Option<&mut toml_edit::DocumentMut>,
        init_path: &Path,
        report: &mut Report,
    ) {
        let Some(theme) = theme else {
            for (element, _, line) in alphas {
                report.note(
                    init_path,
                    *line,
                    Reason::NotYetInVaht,
                    format!(
                        "a translucent {element}: with no style converted, add `alpha` to a theme"
                    ),
                );
            }
            return;
        };
        for (element, alpha, _) in alphas {
            let mut table = theme.as_item_mut();
            for part in element.split('.') {
                table = &mut table[part];
            }
            table["alpha"] = toml_edit::value(*alpha);
        }
    }

    /// Returns the name the theme converted from the style at `path` gets:
    /// the style's own (its directory's, for a `theme.cfg`), with anything
    /// but letters, digits, `-` and `_` made `-`.
    fn theme_name(path: &Path) -> String {
        let named = if path.file_name().is_some_and(|name| name == "theme.cfg") {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let name: String = named
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        if name.is_empty() {
            "imported".to_owned()
        } else {
            name
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
    /// Its path in the configuration directory, such as `settings.toml` or
    /// `themes/nacitar.toml`.
    pub name: String,
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
                .map(|file| directory.join(&file.name))
                .find(|path| path.exists())
        {
            return Err(ImportError::Exists(existing));
        }
        std::fs::create_dir_all(directory).map_err(|error| ImportError::Write {
            path: directory.to_path_buf(),
            error,
        })?;
        for file in &self.files {
            let path = directory.join(&file.name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| ImportError::Write {
                    path: parent.to_path_buf(),
                    error,
                })?;
            }
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
        std::fs::write(
            source.path().join("menu"),
            "[begin] (vaht)\n[exec] (kitty) {kitty}\n[submenu] (More)\n[exit] (Exit)\n[end]\n[end]\n",
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
        assert!(bindings.contains("Right = \"root-menu\""), "{bindings}");
        let menus = std::fs::read_to_string(target.path().join("menus.toml")).expect("written");
        assert!(menus.contains("submenu = \"more\""), "{menus}");
        assert!(
            conversion.report().notes().is_empty(),
            "{}",
            conversion.report()
        );
    }

    #[test]
    fn switch_commands_become_switch_bindings() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            source.path().join("init"),
            "session.switch.lidClose: Exec swaylock\nsession.switch.tabletModeOn: Exec onboard\n",
        )
        .expect("written");
        std::fs::write(source.path().join("keys"), "Mod4 T :Exec kitty\n").expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        let target = tempfile::tempdir().expect("a temporary directory");
        conversion
            .write(target.path(), false)
            .expect("written and valid");
        let bindings =
            std::fs::read_to_string(target.path().join("bindings.toml")).expect("written");
        assert!(
            bindings.contains(
                "[switches]\nlid-close = \"exec swaylock\"\ntablet-mode-on = \"exec onboard\"\n"
            ),
            "{bindings}"
        );
        assert!(
            conversion.report().notes().is_empty(),
            "{}",
            conversion.report()
        );
    }

    #[test]
    fn switch_commands_without_a_keys_file_are_reported() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            source.path().join("init"),
            "session.switch.lidClose: Exec swaylock\n",
        )
        .expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        assert!(
            conversion
                .files()
                .iter()
                .all(|file| file.name != "bindings.toml"),
            "the built-in bindings are kept"
        );
        assert_eq!(
            conversion.report().notes().len(),
            1,
            "{}",
            conversion.report()
        );
    }

    #[test]
    fn the_wallpaper_startup_sets_becomes_the_wallpaper_setting() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(source.path().join("init"), "").expect("written");
        std::fs::write(
            source.path().join("startup"),
            "swaybg -m tile -i ~/sky.png &\nexec fluxbox\n",
        )
        .expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        let settings = &conversion.files()[0].text;
        assert!(
            settings.contains("[wallpaper]\nimage = \"~/sky.png\"\nfit = \"tile\"\n"),
            "{settings}"
        );
        let target = tempfile::tempdir().expect("a temporary directory");
        conversion.write(target.path(), false).expect("valid");
    }
}
