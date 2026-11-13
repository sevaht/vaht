//! The configuration: TOML files parsed once into typed values (02).
//!
//! [`Config::load`] reads every file in the configuration directory and
//! validates all of it before anything uses it, so a reload either applies a
//! complete new configuration or none at all (02 §2). A file that does not
//! exist is replaced by vaht's built-in version of it, the one
//! `vaht check --print-defaults` prints: defaults are written down once,
//! as configuration (K3).
//!
//! Today the directory holds `bindings.toml`, `settings.toml`,
//! `rules.toml` and `menus.toml`; the other files of 02 §2 join them with
//! the features they configure.

mod bindings;
mod command;
mod matcher;
mod menus;
mod overrides;
mod rules;
mod settings;
mod source;
mod theme;
mod unique_map;
mod watch;

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use vaht_ui::Theme;

pub(crate) use self::bindings::{Bindings, KeyChord, Modifiers, MouseButton, MouseContext};
pub(crate) use self::command::{
    Action, Command, CycleOrder, MenuRequest, ResizeFrom, WindowCycle, WindowManagerAction,
};
pub(crate) use self::matcher::WindowMatcher;
pub(crate) use self::menus::{MenuDefinition, MenuEntry, MenuGenerator, Menus, WindowToggle};
pub(crate) use self::overrides::SettingOverrides;
pub(crate) use self::rules::{FocusStealing, Rules};
pub(crate) use self::settings::{
    FocusModel, FrameSettings, MenuSearch, Settings, TaskbarAlign, TaskbarEdge, TaskbarSettings,
    TaskbarWidget, WallpaperSettings, WidthPercent,
};
pub(crate) use self::source::ConfigSource;
pub(crate) use self::theme::ThemeFile;

/// One configuration file and vaht's built-in version of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigFile {
    /// The file's name in the configuration directory.
    pub name: &'static str,
    /// Its built-in contents, used when the file does not exist.
    pub defaults: &'static str,
}

impl ConfigFile {
    /// Returns whether `name` is the name of one of vaht's configuration
    /// files, as opposed to an editor's temporary or backup file.
    pub(crate) fn is_named(name: &OsStr) -> bool {
        Config::FILES.iter().any(|file| name == file.name)
    }

    /// Reads this file from `directory`, or its built-in version if it is
    /// missing, and parses it.
    fn read<T: serde::de::DeserializeOwned>(self, directory: &Path) -> Result<T, ConfigError> {
        let path = directory.join(self.name);
        let (text, source) = match std::fs::read_to_string(&path) {
            Ok(text) => (text, path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (
                self.defaults.to_owned(),
                PathBuf::from(format!("(built-in) {}", self.name)),
            ),
            Err(error) => return Err(ConfigError::Read { path, error }),
        };
        toml::from_str(&text).map_err(|error| ConfigError::invalid(source, &text, &error))
    }
}

/// The validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Key and mouse bindings (`bindings.toml`).
    pub(crate) bindings: Bindings,
    /// How vaht behaves (`settings.toml`).
    pub(crate) settings: Settings,
    /// What happens to particular windows (`rules.toml`).
    pub(crate) rules: Rules,
    /// The menus (`menus.toml`).
    pub(crate) menus: Menus,
    /// How everything vaht draws looks: the theme `settings.toml` names.
    pub(crate) theme: Theme,
}

impl Config {
    /// Every configuration file vaht reads.
    pub const FILES: [ConfigFile; 4] = [
        ConfigFile {
            name: "bindings.toml",
            defaults: include_str!("config/defaults/bindings.toml"),
        },
        ConfigFile {
            name: "settings.toml",
            defaults: include_str!("config/defaults/settings.toml"),
        },
        ConfigFile {
            name: "rules.toml",
            defaults: include_str!("config/defaults/rules.toml"),
        },
        ConfigFile {
            name: "menus.toml",
            defaults: include_str!("config/defaults/menus.toml"),
        },
    ];

    /// Reads and validates the configuration in `directory`; a missing file,
    /// or a missing directory, means that file's built-in version.
    ///
    /// # Errors
    ///
    /// Returns the first problem found: a file that cannot be read, or one
    /// that is not valid, with the line and column of the problem.
    pub fn load(directory: &Path) -> Result<Self, ConfigError> {
        let [bindings, settings, rules, menus] = Self::FILES;
        let settings: Settings = settings.read(directory)?;
        let theme = match &settings.theme {
            Some(name) => ThemeFile::find(directory, name)?.read()?,
            None => Theme::built_in(),
        };
        let config = Self {
            bindings: bindings.read(directory)?,
            settings,
            rules: rules.read(directory)?,
            menus: menus.read(directory)?,
            theme,
        };
        config.check_custom_menus()?;
        Ok(config)
    }

    /// Makes sure every `custom-menu` names a menu `menus.toml` has.
    ///
    /// # Errors
    ///
    /// Returns the first one that does not, and the file naming it.
    fn check_custom_menus(&self) -> Result<(), ConfigError> {
        let named = [
            (
                "bindings.toml",
                self.bindings.commands().collect::<Vec<_>>(),
            ),
            ("menus.toml", self.menus.commands().collect::<Vec<_>>()),
        ];
        for (file, commands) in named {
            for name in commands.iter().flat_map(|command| command.custom_menus()) {
                if self.menus.get(name).is_none() {
                    return Err(ConfigError::UnknownMenu {
                        file,
                        name: name.to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Returns how everything vaht draws looks: the theme the settings
    /// name. vaht's dialogs, which are separate programs, draw with it too.
    #[must_use]
    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Returns the built-in configuration, which every file's absence gives.
    ///
    /// # Panics
    ///
    /// Panics if a built-in file is invalid; a unit test makes sure none is.
    #[must_use]
    pub fn built_in() -> Self {
        let [bindings, _, _, menus] = Self::FILES;
        Self {
            bindings: toml::from_str(bindings.defaults)
                .expect("the built-in bindings are valid (tested)"),
            settings: Settings::default(),
            rules: Rules::default(),
            menus: toml::from_str(menus.defaults).expect("the built-in menus are valid (tested)"),
            theme: Theme::built_in(),
        }
    }
}

/// Why the configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file exists but could not be read.
    #[error("could not read {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: io::Error,
    },

    /// A file is not valid.
    #[error("{}:{line}:{column}: {message}", path.display())]
    Invalid {
        /// The file.
        path: PathBuf,
        /// The line of the problem, counting from 1.
        line: usize,
        /// The column of the problem, counting from 1.
        column: usize,
        /// What is wrong.
        message: String,
    },

    /// A `custom-menu` names a menu `menus.toml` does not have.
    #[error("{file}: `custom-menu {name}` names no [menu.{name}] in menus.toml")]
    UnknownMenu {
        /// The file naming it.
        file: &'static str,
        /// The menu's name.
        name: String,
    },

    /// A file could not be written.
    #[error("could not write {}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: io::Error,
    },

    /// The settings name a theme by something other than a plain name.
    #[error("not a theme name: {name:?} (a theme is themes/<name>.toml)")]
    ThemeName {
        /// The name as written.
        name: String,
    },

    /// The theme the settings name exists nowhere vaht looks.
    #[error("no theme `{name}`: looked for {}", searched.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" and "))]
    MissingTheme {
        /// The theme's name.
        name: String,
        /// Where vaht looked.
        searched: Vec<PathBuf>,
    },
}

impl ConfigError {
    /// Describes `error`, found in `text` read from `path`, by line and
    /// column.
    pub(crate) fn invalid(path: PathBuf, text: &str, error: &toml::de::Error) -> Self {
        let offset = error.span().map_or(0, |span| span.start);
        let before = text.get(..offset).unwrap_or(text);
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |start| start.chars().count())
            + 1;
        Self::Invalid {
            path,
            line,
            column,
            message: error.message().trim_end().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn a_custom_menu_must_be_one_menus_toml_has() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            directory.path().join("bindings.toml"),
            "[keys]\nF1 = \"custom-menu games\"\n",
        )
        .expect("written");
        let error = Config::load(directory.path()).expect_err("no such menu");
        assert_eq!(
            error.to_string(),
            "bindings.toml: `custom-menu games` names no [menu.games] in menus.toml"
        );
        std::fs::write(
            directory.path().join("menus.toml"),
            "[menu.root]\nitems = []\n[menu.window]\nitems = []\n\
             [menu.workspace]\nitems = []\n[menu.games]\nitems = []\n",
        )
        .expect("written");
        Config::load(directory.path()).expect("the menu is there now");
    }

    #[test]
    fn a_missing_directory_gives_the_valid_built_in_files() {
        let loaded = Config::load(Path::new("/nonexistent/vaht-config"));
        assert!(loaded.is_ok(), "{loaded:?}");
        let _ = Config::built_in();
    }

    #[test]
    fn a_problem_is_reported_by_file_line_and_column() {
        let text = "[keys]\n\"Alt+F4\" = \"close\"\n\"Alt+F5\" = \"explode\"\n";
        let error = toml::from_str::<Bindings>(text).expect_err("explode is no action");
        let ConfigError::Invalid {
            line,
            column,
            message,
            ..
        } = ConfigError::invalid(PathBuf::from("bindings.toml"), text, &error)
        else {
            panic!("an invalid file is reported as invalid");
        };
        assert_eq!((line, column), (3, 12));
        assert!(message.contains("explode"), "{message}");
    }

    #[rstest]
    #[case::configuration_file("bindings.toml", true)]
    #[case::editor_backup("bindings.toml~", false)]
    #[case::editor_swap_file(".bindings.toml.swp", false)]
    #[case::vim_write_test("4913", false)]
    #[case::other_toml("notes.toml", false)]
    fn only_configuration_file_names_count(#[case] name: &str, #[case] is_named: bool) {
        assert_eq!(ConfigFile::is_named(OsStr::new(name)), is_named);
    }
}
