//! The configuration: TOML files parsed once into typed values (02).
//!
//! [`Config::load`] reads every file in the configuration directory and
//! validates all of it, returning the first problem, for a program that needs
//! a whole configuration or none (`vaht dialog`). A running vaht uses
//! [`Config::load_lenient`] instead, which uses what is valid and returns what
//! it left out (02 §2), and `vaht config check` lists every problem
//! ([`Config::problems`]). A file that does not exist is replaced by vaht's built-in version of it, the one
//! `vaht config check --print-defaults` prints: defaults are written down once,
//! as configuration (K3).
//!
//! Today the directory holds `bindings.toml`, `settings.toml`,
//! `rules.toml`, `menus.toml` and `outputs.toml`; the other files of 02 §2
//! join them with the features they configure.

mod bindings;
mod command;
mod lenient;
mod matcher;
mod menus;
mod outputs;
mod overrides;
mod rules;
pub(crate) mod schema;
mod settings;
mod source;
mod style;
mod unique_map;
mod watch;

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use vaht_ui::Style;

pub(crate) use self::bindings::{
    Bindings, KeyChord, KeyLookup, Modifiers, MouseButton, MouseContext, SwitchFlip, SwitchKind,
};
pub(crate) use self::command::{
    Action, Command, CycleOrder, Delay, KeyModeSwitch, MenuRequest, ResizeFrom, TaskbarAction,
    TaskbarOutput, WindowCycle, WindowManagerAction,
};
pub(crate) use self::lenient::{ConfigProblem, Loaded};
pub(crate) use self::matcher::WindowMatcher;
pub(crate) use self::menus::{MenuDefinition, MenuEntry, MenuGenerator, Menus, WindowToggle};
pub(crate) use self::outputs::{
    HdrRequest, OutputEntry, OutputIdentity, OutputMode, OutputPlan, OutputProfile, OutputScale,
    OutputSettings, OutputTransform, Vrr,
};
pub(crate) use self::overrides::{SettingChange, SettingOverrides};
pub(crate) use self::rules::{FocusStealing, RuleFrame, RuleOutput, RuleSettings, Rules};
pub(crate) use self::settings::{
    AccelProfile, AccelSpeed, BlurSettings, ClickMethod, DeviceSettings, FileCount, FocusModel,
    FrameSettings, InputSettings, LoggingSettings, MenuSearch, ScrollMethod, SendEvents, Settings,
    SnapSettings, TapButtonMap, TaskbarAlign, TaskbarEdge, TaskbarSettings, TaskbarWidget,
    WallpaperSettings, WidthPercent, WindowListScope,
};
pub(crate) use self::source::ConfigSource;
pub(crate) use self::style::StyleFile;

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
        let (text, source) = self.text(directory)?;
        toml::from_str(&text).map_err(|error| ConfigError::invalid(source, &text, &error))
    }

    /// Reads this file from `directory`, or its built-in version if it is
    /// missing, as a TOML table.
    pub(crate) fn read_table(self, directory: &Path) -> Result<toml::Table, ConfigError> {
        self.read(directory)
    }

    /// Returns this file's text in `directory`, or its built-in version if
    /// it is missing, and where it came from.
    fn text(self, directory: &Path) -> Result<(String, PathBuf), ConfigError> {
        let path = directory.join(self.name);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok((text, path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((
                self.defaults.to_owned(),
                PathBuf::from(format!("(built-in) {}", self.name)),
            )),
            Err(error) => Err(ConfigError::Read { path, error }),
        }
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
    /// How each output is set up (`outputs.toml`).
    pub(crate) outputs: OutputSettings,
    /// How everything vaht draws looks: the style `settings.toml` names.
    pub(crate) style: Style,
}

impl Config {
    /// Every configuration file vaht reads.
    pub const FILES: [ConfigFile; 5] = [
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
        ConfigFile {
            name: "outputs.toml",
            defaults: include_str!("config/defaults/outputs.toml"),
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
        Self::load_with(directory, &SettingOverrides::default())
    }

    /// Returns the JSON Schema of `file`, or none if it is not described yet
    /// (only `settings.toml` is).
    #[must_use]
    pub fn schema(file: vaht_ipc::ConfigFile) -> Option<serde_json::Value> {
        schema::schema_of(file)
    }

    /// Returns every problem in the configuration in `directory`, a sentence
    /// each (the file, the setting, what is wrong, and what vaht uses
    /// instead): empty if all of it is valid. Where [`Config::load`] stops
    /// at the first, this finds them all, as a running vaht does.
    #[must_use]
    pub fn problems(directory: &Path) -> Vec<String> {
        Self::load_lenient(directory, &SettingOverrides::default(), None)
            .problems
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Reads and validates the configuration in `directory`, as
    /// [`Config::load`] does, with the settings `overrides` changes.
    ///
    /// # Errors
    ///
    /// Returns the first problem found, as [`Config::load`] does, or what
    /// is wrong with the settings changed.
    pub(crate) fn load_with(
        directory: &Path,
        overrides: &SettingOverrides,
    ) -> Result<Self, ConfigError> {
        let [bindings, settings, rules, menus, outputs] = Self::FILES;
        let settings: Settings = if overrides.is_empty() {
            settings.read(directory)?
        } else {
            let mut table = settings.read_table(directory)?;
            overrides
                .apply_to(&mut table)
                .and_then(|()| Settings::from_table(table))
                .map_err(|message| ConfigError::Changed { message })?
        };
        // No style chosen is the default style, which is a file like any other
        // (a user's, or the installed one) and only falls back to the copy built in.
        let name = settings.style.as_deref().unwrap_or(Style::DEFAULT_NAME);
        let style = StyleFile::load(directory, name)?;
        let config = Self {
            bindings: bindings.read(directory)?,
            settings,
            rules: rules.read(directory)?,
            menus: menus.read(directory)?,
            outputs: outputs.read(directory)?,
            style,
        };
        config.check_custom_menus()?;
        config.check_key_modes()?;
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

    /// Makes sure every `key-mode` names a mode `bindings.toml` has, or
    /// the default one.
    ///
    /// # Errors
    ///
    /// Returns the first one that does not, and the file naming it.
    fn check_key_modes(&self) -> Result<(), ConfigError> {
        let named = [
            (
                "bindings.toml",
                self.bindings.commands().collect::<Vec<_>>(),
            ),
            ("menus.toml", self.menus.commands().collect::<Vec<_>>()),
        ];
        for (file, commands) in named {
            for name in commands.iter().flat_map(|command| command.key_modes()) {
                if name != KeyModeSwitch::DEFAULT && !self.bindings.has_key_mode(name) {
                    return Err(ConfigError::UnknownKeyMode {
                        file,
                        name: name.to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Returns how everything vaht draws looks: the style the settings
    /// name. vaht's dialogs, which are separate programs, draw with it too.
    #[must_use]
    pub const fn style(&self) -> &Style {
        &self.style
    }

    /// Returns the built-in configuration, which every file's absence gives.
    ///
    /// # Panics
    ///
    /// Panics if a built-in file is invalid; a unit test makes sure none is.
    #[must_use]
    pub fn built_in() -> Self {
        let [bindings, _, _, menus, _] = Self::FILES;
        Self {
            bindings: toml::from_str(bindings.defaults)
                .expect("the built-in bindings are valid (tested)"),
            settings: Settings::default(),
            rules: Rules::default(),
            menus: toml::from_str(menus.defaults).expect("the built-in menus are valid (tested)"),
            outputs: OutputSettings::default(),
            style: Style::built_in(),
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

    /// The settings changed in place (`set-setting`) are not valid.
    #[error("the settings changed in place: {message}")]
    Changed {
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

    /// A `key-mode` names a mode `bindings.toml` does not have.
    #[error("{file}: `key-mode {name}` names no [key_modes.{name}] in bindings.toml")]
    UnknownKeyMode {
        /// The file naming it.
        file: &'static str,
        /// The mode's name.
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

    /// The settings name a style by something other than a plain name.
    #[error("not a style name: {name:?} (a style is styles/<name>.toml)")]
    StyleName {
        /// The name as written.
        name: String,
    },

    /// The style the settings name exists nowhere vaht looks.
    #[error("no style `{name}`: looked for {}", searched.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" and "))]
    MissingStyle {
        /// The style's name.
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
