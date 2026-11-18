//! Loading the configuration as much as can be used (02 §2).
//!
//! [`Config::load`] is strict: it returns the first problem and nothing
//! else, which suits `vaht config check` and an edit made in the settings window,
//! that must be refused whole. A running vaht should instead use what it can
//! and say loudly what it could not. [`Config::load_lenient`] therefore
//! takes each file on its own: one that cannot be used is replaced by the
//! last good one (or the built-in one, at the start), and `settings.toml` is
//! taken a key at a time, so one misspelled setting leaves every other
//! setting, and every other file, in effect. Each thing left out is a
//! [`ConfigProblem`], which vaht shows on every output until the files are
//! fixed.

use std::fmt;
use std::path::Path;

use vaht_ui::Theme;

use super::{
    Bindings, Config, ConfigFile, Menus, OutputSettings, Rules, SettingOverrides, Settings,
    ThemeFile,
};

/// What vaht used in place of something it could not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instead {
    /// The setting's default.
    Default,
    /// What the file said when it was last good.
    LastGood,
    /// vaht's built-in version.
    BuiltIn,
}

impl fmt::Display for Instead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Default => "using the default",
            Self::LastGood => "using what it said before",
            Self::BuiltIn => "using the built-in one",
        })
    }
}

/// Something in the configuration vaht could not use, and what it did
/// about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigProblem {
    /// The file, as the user knows it (`settings.toml`).
    pub(crate) file: String,
    /// The setting, if the problem is one setting's (`taskbar.rows`).
    pub(crate) key: Option<String>,
    /// What is wrong, as the file's reader says.
    pub(crate) message: String,
    /// What vaht used instead.
    pub(crate) instead: Instead,
}

impl ConfigProblem {
    /// Returns what is wrong, for someone told a change was refused: the
    /// setting and why.
    pub(crate) fn reason(&self) -> String {
        match &self.key {
            Some(key) => format!("{key}: {}", self.message),
            None => self.message.clone(),
        }
    }
}

impl ConfigProblem {
    /// Returns the problem in a line short enough for a banner: without the
    /// list of what would have been valid (`, expected one of ...`) that a
    /// reader adds to its message.
    pub(crate) fn brief(&self) -> String {
        let message = self
            .message
            .split(", expected")
            .next()
            .unwrap_or(&self.message);
        let mut line = self.file.clone();
        if let Some(key) = &self.key {
            line.push_str(": ");
            line.push_str(key);
        }
        format!("{line}: {message} ({})", self.instead)
    }
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.file)?;
        if let Some(key) = &self.key {
            write!(formatter, ": {key}")?;
        }
        write!(formatter, ": {} ({})", self.message, self.instead)
    }
}

/// A configuration, and what was left out of it.
#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    /// What can be used.
    pub(crate) config: Config,
    /// What could not, in the order found; empty if all was well.
    pub(crate) problems: Vec<ConfigProblem>,
    /// Whether the settings changed in place no longer fit the files, and
    /// were not applied.
    pub(crate) dropped_overrides: bool,
}

impl Config {
    /// The most settings left out of `settings.toml` before it is given up
    /// on, so a loop cannot run away.
    const MOST_LEFT_OUT: usize = 200;

    /// Reads the configuration in `directory`, using everything in it that
    /// is valid: a file that is not is replaced by its part of `previous`
    /// (the configuration in effect), else the built-in one, and in
    /// `settings.toml` a setting that is not valid is left out, so it is the
    /// default. `overrides` are the settings changed in place.
    pub(crate) fn load_lenient(
        directory: &Path,
        overrides: &SettingOverrides,
        previous: Option<&Self>,
    ) -> Loaded {
        let [
            bindings_file,
            settings_file,
            rules_file,
            menus_file,
            outputs_file,
        ] = Self::FILES;
        let built_in = Self::built_in();
        let fallback = previous.unwrap_or(&built_in);
        let instead = if previous.is_some() {
            Instead::LastGood
        } else {
            Instead::BuiltIn
        };
        let mut problems = Vec::new();
        let (settings, dropped_overrides) = Self::lenient_settings(
            settings_file,
            directory,
            overrides,
            (fallback, instead),
            &mut problems,
        );
        let theme = match &settings.theme {
            Some(name) => ThemeFile::find(directory, name)
                .and_then(|file| file.read())
                .unwrap_or_else(|error| {
                    problems.push(ConfigProblem {
                        file: format!("theme {name}"),
                        key: None,
                        message: error.to_string(),
                        instead: Instead::BuiltIn,
                    });
                    Theme::built_in()
                }),
            None => Theme::built_in(),
        };
        let mut config = Self {
            bindings: Self::lenient_file::<Bindings>(
                bindings_file,
                directory,
                (&fallback.bindings, instead),
                &mut problems,
            ),
            settings,
            rules: Self::lenient_file::<Rules>(
                rules_file,
                directory,
                (&fallback.rules, instead),
                &mut problems,
            ),
            menus: Self::lenient_file::<Menus>(
                menus_file,
                directory,
                (&fallback.menus, instead),
                &mut problems,
            ),
            outputs: Self::lenient_file::<OutputSettings>(
                outputs_file,
                directory,
                (&fallback.outputs, instead),
                &mut problems,
            ),
            theme,
        };
        // Bindings and menus name each other's menus and key modes, so when
        // they disagree neither can be trusted: both are replaced.
        if let Err(error) = config
            .check_custom_menus()
            .and_then(|()| config.check_key_modes())
        {
            problems.push(ConfigProblem {
                file: "bindings.toml and menus.toml".to_owned(),
                key: None,
                message: error.to_string(),
                instead,
            });
            config.bindings = fallback.bindings.clone();
            config.menus = fallback.menus.clone();
        }
        Loaded {
            config,
            problems,
            dropped_overrides,
        }
    }

    /// Reads `file` from `directory` as a `T`, or `fallback`'s version of it,
    /// with a problem noted, if it is not valid.
    fn lenient_file<T>(
        file: ConfigFile,
        directory: &Path,
        fallback: (&T, Instead),
        problems: &mut Vec<ConfigProblem>,
    ) -> T
    where
        T: serde::de::DeserializeOwned + Clone,
    {
        file.read::<T>(directory).unwrap_or_else(|error| {
            problems.push(ConfigProblem {
                file: file.name.to_owned(),
                key: None,
                message: error.to_string(),
                instead: fallback.1,
            });
            fallback.0.clone()
        })
    }

    /// Reads `settings.toml` (with `overrides`) a setting at a time: one that
    /// does not deserialize is left out, so it is the default. A file that
    /// is not even TOML is replaced by `fallback`'s settings. Returns them
    /// and whether `overrides` could not be applied, and were not.
    fn lenient_settings(
        file: ConfigFile,
        directory: &Path,
        overrides: &SettingOverrides,
        fallback: (&Self, Instead),
        problems: &mut Vec<ConfigProblem>,
    ) -> (Settings, bool) {
        let mut dropped_overrides = false;
        let mut table = match file.read_table(directory) {
            Ok(table) => table,
            Err(error) => {
                problems.push(ConfigProblem {
                    file: file.name.to_owned(),
                    key: None,
                    message: error.to_string(),
                    instead: fallback.1,
                });
                return (fallback.0.settings.clone(), false);
            }
        };
        if !overrides.is_empty() {
            let mut changed = table.clone();
            match overrides.apply_to(&mut changed) {
                Ok(()) => table = changed,
                Err(message) => {
                    dropped_overrides = true;
                    problems.push(ConfigProblem {
                        file: "the settings changed in place".to_owned(),
                        key: None,
                        message,
                        instead: Instead::Default,
                    });
                }
            }
        }
        for _ in 0..Self::MOST_LEFT_OUT {
            let result: Result<Settings, _> =
                serde_path_to_error::deserialize(toml::Value::Table(table.clone()));
            let Err(error) = result else {
                return (result.unwrap_or_default(), dropped_overrides);
            };
            let message = error.inner().message().trim_end().to_owned();
            let key = Self::key_of(error.path());
            if key.is_empty() || !Self::leave_out(&mut table, &key) {
                problems.push(ConfigProblem {
                    file: file.name.to_owned(),
                    key: None,
                    message,
                    instead: Instead::Default,
                });
                return (Settings::default(), dropped_overrides);
            }
            problems.push(ConfigProblem {
                file: file.name.to_owned(),
                key: Some(key.join(".")),
                message,
                instead: Instead::Default,
            });
        }
        (Settings::default(), dropped_overrides)
    }

    /// Returns the key `path` is in: its leading names, up to the first
    /// place a list or a variant begins (a bad item leaves out the whole
    /// list).
    fn key_of(path: &serde_path_to_error::Path) -> Vec<String> {
        path.iter()
            .map_while(|segment| match segment {
                serde_path_to_error::Segment::Map { key } => Some(key.clone()),
                serde_path_to_error::Segment::Seq { .. }
                | serde_path_to_error::Segment::Enum { .. }
                | serde_path_to_error::Segment::Unknown => None,
            })
            .collect()
    }

    /// Takes `key`'s setting out of `table`; returns whether it was there.
    fn leave_out(table: &mut toml::Table, key: &[String]) -> bool {
        let Some((last, parents)) = key.split_last() else {
            return false;
        };
        let mut current = table;
        for part in parents {
            let Some(toml::Value::Table(next)) = current.get_mut(part) else {
                return false;
            };
            current = next;
        }
        current.remove(last).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `files` into a new configuration directory.
    fn directory(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for (name, text) in files {
            std::fs::write(directory.path().join(name), text).expect("written");
        }
        directory
    }

    fn load(directory: &tempfile::TempDir) -> Loaded {
        Config::load_lenient(directory.path(), &SettingOverrides::default(), None)
    }

    #[test]
    fn a_valid_configuration_has_no_problems() {
        let loaded = load(&directory(&[(
            "settings.toml",
            "[focus]\nmodel = \"sloppy\"\n",
        )]));
        assert_eq!(loaded.problems, []);
        assert_eq!(
            loaded.config.settings.focus.model,
            super::super::FocusModel::Sloppy
        );
    }

    #[test]
    fn a_setting_that_is_not_valid_is_left_out_and_the_rest_is_used() {
        let loaded = load(&directory(&[(
            "settings.toml",
            "[focus]\nmodel = \"sloppy\"\n[taskbar]\nrows = 4\nnonsense = true\n\
             [wallpaper]\nimage = \"sky.png\"\n",
        )]));
        let keys: Vec<_> = loaded
            .problems
            .iter()
            .map(|problem| problem.key.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            keys,
            ["taskbar.nonsense"],
            "only the bad key: {:?}",
            loaded.problems
        );
        assert_eq!(loaded.problems[0].instead, Instead::Default);
        let settings = &loaded.config.settings;
        assert_eq!(
            settings.focus.model,
            super::super::FocusModel::Sloppy,
            "valid, kept"
        );
        assert_eq!(
            settings.taskbar.row_range(),
            (4, 4),
            "valid, in the same table: kept"
        );
        assert!(
            settings.wallpaper.image_path(Path::new("/etc")).is_some(),
            "valid, in another table: kept"
        );
    }

    #[test]
    fn a_value_out_of_range_names_its_own_key() {
        let loaded = load(&directory(&[(
            "settings.toml",
            "[taskbar]\nrows = 0\nedge = \"top\"\n[blur]\npasses = 99\n",
        )]));
        let mut keys: Vec<_> = loaded
            .problems
            .iter()
            .filter_map(|problem| problem.key.clone())
            .collect();
        keys.sort();
        assert_eq!(keys, ["blur.passes", "taskbar.rows"]);
        assert_eq!(
            loaded.config.settings.taskbar.edge,
            super::super::TaskbarEdge::Top,
            "the valid neighbour of a bad key stays"
        );
    }

    #[test]
    fn a_bad_item_in_a_list_leaves_out_that_list() {
        let loaded = load(&directory(&[(
            "settings.toml",
            "[taskbar]\nwidgets = [\"clock\", \"pager\"]\nrows = 2\n",
        )]));
        assert_eq!(
            loaded
                .problems
                .iter()
                .filter_map(|problem| problem.key.as_deref())
                .collect::<Vec<_>>(),
            ["taskbar.widgets"]
        );
        assert_eq!(loaded.config.settings.taskbar.row_range(), (2, 2));
    }

    #[test]
    fn a_file_that_is_not_toml_is_replaced_and_the_others_are_used() {
        let loaded = load(&directory(&[
            ("settings.toml", "[taskbar]\nrows = 3\n"),
            ("outputs.toml", "this is [not toml"),
        ]));
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].file, "outputs.toml");
        assert_eq!(loaded.problems[0].instead, Instead::BuiltIn);
        assert_eq!(loaded.config.settings.taskbar.row_range(), (3, 3));
    }

    #[test]
    fn a_file_that_is_not_valid_is_replaced_by_the_one_in_effect_on_a_reload() {
        let good = directory(&[("settings.toml", "[taskbar]\nrows = 3\n")]);
        let before = load(&good).config;
        let broken = directory(&[("settings.toml", "[taskbar\nrows = ")]);
        let loaded =
            Config::load_lenient(broken.path(), &SettingOverrides::default(), Some(&before));
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].instead, Instead::LastGood);
        assert_eq!(
            loaded.config.settings.taskbar.row_range(),
            (3, 3),
            "what was in effect"
        );
    }

    #[test]
    fn a_missing_menu_is_a_problem_not_a_failure() {
        let loaded = load(&directory(&[(
            "bindings.toml",
            "[keys]\n\"Super+M\" = \"custom-menu nosuch\"\n",
        )]));
        assert_eq!(loaded.problems.len(), 1);
        assert!(
            loaded.problems[0].message.contains("nosuch"),
            "{:?}",
            loaded.problems
        );
    }

    #[test]
    fn a_missing_theme_is_a_problem_and_the_built_in_theme_is_used() {
        let loaded = load(&directory(&[(
            "settings.toml",
            "theme = \"no-such-theme\"\n",
        )]));
        assert_eq!(loaded.problems.len(), 1);
        assert!(loaded.problems[0].file.contains("no-such-theme"));
        assert_eq!(loaded.problems[0].instead, Instead::BuiltIn);
    }

    #[test]
    fn a_problem_reads_as_a_sentence_saying_what_was_used_instead() {
        let problem = ConfigProblem {
            file: "settings.toml".to_owned(),
            key: Some("taskbar.rows".to_owned()),
            message: "rows is at least 1".to_owned(),
            instead: Instead::Default,
        };
        assert_eq!(
            problem.to_string(),
            "settings.toml: taskbar.rows: rows is at least 1 (using the default)"
        );
    }

    #[test]
    fn a_brief_problem_drops_what_would_have_been_valid() {
        let problem = ConfigProblem {
            file: "settings.toml".to_owned(),
            key: Some("taskbar.windows".to_owned()),
            message: "unknown field `windows`, expected one of `show`, `icons`".to_owned(),
            instead: Instead::Default,
        };
        assert_eq!(
            problem.brief(),
            "settings.toml: taskbar.windows: unknown field `windows` (using the default)"
        );
    }
}
