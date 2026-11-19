//! Layouts: `layouts/<name>.toml`, a named set of presentation settings
//! (which widgets the taskbar shows, how it is arranged, the window buttons)
//! that is swapped for another without touching the rest of the settings
//! (02 §10).
//!
//! A layout is a partial `settings.toml`, but only of what is shown and how it
//! is arranged: [`PRESENTATION`] lists what it may set, and anything else in it
//! is left out, so changing a layout can never change how a device or the
//! session behaves. It sits under `settings.toml`: a key `settings.toml` sets
//! wins (the file is the user's overrides), and a key it leaves out comes from
//! the layout, then from vaht's default.

use std::path::{Path, PathBuf};

use super::ConfigError;
use super::library::Library;

/// What a layout may set: whole tables, or single settings, by their dotted
/// path in `settings.toml`. Presentation, not behavior.
pub(crate) const PRESENTATION: [&str; 10] = [
    "taskbar",
    "windows.frame",
    "windows.focused_alpha",
    "windows.unfocused_alpha",
    "windows.snapping.layout_picker",
    "windows.snapping.picker_size",
    "windows.snapping.picker_columns",
    "windows.snapping.layouts",
    "blur",
    "menus.window_icons",
];

/// The layout in effect, and what it contributes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LayoutState {
    /// The layout `settings.toml` names, if it names one (even a layout that
    /// could not be found, which contributes nothing).
    pub(crate) name: Option<String>,
    /// What each setting is without `settings.toml`: vaht's defaults under
    /// the layout. It is what "reset" puts a setting back to.
    pub(crate) defaults: super::Settings,
    /// The settings that come from the layout: what it sets that
    /// `settings.toml` does not.
    pub(crate) keys: Vec<Vec<String>>,
}

/// The default layout, `data/layouts/vaht.toml`: installed for people to
/// read and copy, and built in so that it needs no file.
const BUILT_IN: &str = include_str!("../../../../data/layouts/vaht.toml");

/// Where a layout is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// A file.
    File(PathBuf),
    /// The default layout's copy in vaht.
    BuiltIn,
}

/// A layout file, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutFile {
    /// Where it is.
    source: Source,
}

impl LayoutFile {
    /// Finds the layout `name`: in the `layouts` directory of the
    /// configuration `directory`, then among the installed layouts.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a plain name, or no such layout
    /// exists.
    pub(crate) fn find(directory: &Path, name: &str) -> Result<Self, ConfigError> {
        match Library::LAYOUTS.find(directory, name) {
            Ok(path) => Ok(Self {
                source: Source::File(path),
            }),
            // The default layout needs no file.
            Err(ConfigError::MissingInLibrary { .. }) if name == Library::DEFAULT_NAME => {
                Ok(Self {
                    source: Source::BuiltIn,
                })
            }
            Err(error) => Err(error),
        }
    }

    /// Returns the names of the layouts there are, sorted, each once.
    pub(crate) fn available(directory: &Path) -> Vec<String> {
        Library::LAYOUTS.available(directory)
    }

    /// Reads the layout as a table of settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not TOML, with the
    /// line and column of the problem.
    pub(crate) fn read_table(&self) -> Result<toml::Table, ConfigError> {
        let path = match &self.source {
            Source::File(path) => path,
            Source::BuiltIn => {
                return Ok(
                    toml::from_str(BUILT_IN).expect("the built-in layout is valid TOML (tested)")
                );
            }
        };
        let text = std::fs::read_to_string(path).map_err(|error| ConfigError::Read {
            path: path.clone(),
            error,
        })?;
        toml::from_str(&text).map_err(|error| ConfigError::invalid(path.clone(), &text, &error))
    }

    /// Returns the layout's name for a message: its file, as the user knows it.
    pub(crate) fn file_name(name: &str) -> String {
        format!("{}/{name}.toml", Library::LAYOUTS.directory)
    }
}

/// Returns whether `key`, a dotted path, is something a layout may set.
fn is_presentation(key: &str) -> bool {
    PRESENTATION
        .iter()
        .any(|allowed| key == *allowed || key.starts_with(&format!("{allowed}.")))
}

/// Returns whether a layout may set something inside the table at `key`.
fn holds_presentation(key: &str) -> bool {
    PRESENTATION
        .iter()
        .any(|allowed| allowed.starts_with(&format!("{key}.")))
}

/// Takes out of `table` every setting a layout may not set; returns their
/// dotted paths, the outermost of each.
pub(crate) fn leave_out_other_settings(table: &mut toml::Table) -> Vec<String> {
    fn filter(table: &mut toml::Table, path: &mut Vec<String>, removed: &mut Vec<String>) {
        let keys: Vec<String> = table.keys().cloned().collect();
        for key in keys {
            path.push(key.clone());
            let dotted = path.join(".");
            if is_presentation(&dotted) {
                // Wholly allowed.
            } else if holds_presentation(&dotted)
                && let Some(toml::Value::Table(inner)) = table.get_mut(&key)
            {
                filter(inner, path, removed);
            } else {
                table.remove(&key);
                removed.push(dotted);
            }
            path.pop();
        }
    }
    let mut removed = Vec::new();
    filter(table, &mut Vec::new(), &mut removed);
    removed
}

/// Returns `over` laid on `under`: tables merge setting by setting, and
/// anything else in `over` (a value, a list) replaces what `under` has.
pub(crate) fn merge_under(under: &toml::Table, over: &toml::Table) -> toml::Table {
    let mut merged = under.clone();
    for (key, value) in over {
        let combined = match (merged.get(key), value) {
            (Some(toml::Value::Table(below)), toml::Value::Table(above)) => {
                toml::Value::Table(merge_under(below, above))
            }
            _ => value.clone(),
        };
        merged.insert(key.clone(), combined);
    }
    merged
}

/// Returns the dotted path of every setting in `table` that is not a table.
pub(crate) fn leaf_keys(table: &toml::Table) -> Vec<Vec<String>> {
    fn walk(table: &toml::Table, path: &mut Vec<String>, keys: &mut Vec<Vec<String>>) {
        for (key, value) in table {
            path.push(key.clone());
            match value {
                toml::Value::Table(inner) => walk(inner, path, keys),
                toml::Value::String(_)
                | toml::Value::Integer(_)
                | toml::Value::Float(_)
                | toml::Value::Boolean(_)
                | toml::Value::Datetime(_)
                | toml::Value::Array(_) => keys.push(path.clone()),
            }
            path.pop();
        }
    }
    let mut keys = Vec::new();
    walk(table, &mut Vec::new(), &mut keys);
    keys
}

/// Returns whether `table` sets the setting at `key`.
pub(crate) fn sets(table: &toml::Table, key: &[String]) -> bool {
    let mut current = table;
    let Some((last, parents)) = key.split_last() else {
        return false;
    };
    for part in parents {
        let Some(toml::Value::Table(next)) = current.get(part) else {
            return false;
        };
        current = next;
    }
    current.contains_key(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `text` as a table.
    fn table(text: &str) -> toml::Table {
        toml::from_str(text).expect("TOML")
    }

    #[test]
    fn a_layout_keeps_presentation_settings_and_drops_the_rest() {
        let mut layout = table(
            "[taskbar]\nrows = 2\n[taskbar.clock]\nformat = \"%H\"\n\
             [windows.frame]\nhandle = false\n[windows]\nedge_snap_distance = 5\n\
             [windows.snapping]\nlayout_picker = false\nenabled = false\n\
             [focus]\nmodel = \"sloppy\"\n[blur]\npasses = 4\n",
        );
        let mut removed = leave_out_other_settings(&mut layout);
        removed.sort();
        assert_eq!(
            removed,
            [
                "focus",
                "windows.edge_snap_distance",
                "windows.snapping.enabled"
            ],
            "behavior is left out"
        );
        assert_eq!(
            layout,
            table(
                "[taskbar]\nrows = 2\n[taskbar.clock]\nformat = \"%H\"\n\
                 [windows.frame]\nhandle = false\n\
                 [windows.snapping]\nlayout_picker = false\n[blur]\npasses = 4\n"
            )
        );
    }

    #[test]
    fn every_setting_a_layout_may_set_is_a_setting_vaht_has() {
        let value = super::super::schema::settings_value(&super::super::Settings::default());
        for allowed in PRESENTATION {
            let mut node = &value;
            for part in allowed.split('.') {
                node = node
                    .get(part)
                    .unwrap_or_else(|| panic!("{allowed} is no setting (stopped at {part})"));
            }
        }
    }

    #[test]
    fn the_users_settings_win_over_a_layout_table_by_table() {
        let layout =
            table("[taskbar]\nrows = 2\nwidgets = [\"clock\"]\n[taskbar.clock]\nformat = \"%H\"\n");
        let user = table("[taskbar]\nwidgets = [\"tray\", \"clock\"]\n");
        assert_eq!(
            merge_under(&layout, &user),
            table(
                "[taskbar]\nrows = 2\nwidgets = [\"tray\", \"clock\"]\n\
                 [taskbar.clock]\nformat = \"%H\"\n"
            ),
            "the user's list replaces the layout's whole; the rest of the layout stays"
        );
    }

    #[test]
    fn the_settings_a_table_sets_are_found_by_path() {
        let layout = table("[taskbar]\nrows = 2\n[taskbar.clock]\nformat = \"%H\"\n");
        let keys = leaf_keys(&layout);
        assert_eq!(
            keys,
            [
                vec![
                    "taskbar".to_owned(),
                    "clock".to_owned(),
                    "format".to_owned()
                ],
                vec!["taskbar".to_owned(), "rows".to_owned()],
            ]
        );
        assert!(sets(&layout, &keys[1]));
        assert!(!sets(&layout, &["taskbar".to_owned(), "edge".to_owned()]));
    }

    #[test]
    fn the_built_in_layout_is_the_defaults_presentation_and_nothing_else() {
        let mut table = LayoutFile::find(Path::new("/nonexistent"), "vaht")
            .expect("the default layout needs no file")
            .read_table()
            .expect("valid");
        assert_eq!(
            leave_out_other_settings(&mut table),
            Vec::<String>::new(),
            "only presentation"
        );
        let settings = super::super::Settings::from_table(table).expect("valid settings");
        assert_eq!(
            super::super::schema::settings_value(&settings),
            super::super::schema::settings_value(&super::super::Settings::default()),
            "it says what the defaults say, so choosing it changes nothing"
        );
    }

    #[test]
    fn every_layout_shipped_is_presentation_only_and_valid() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/layouts");
        let mut count = 0;
        for entry in std::fs::read_dir(directory).expect("the shipped layouts") {
            let path = entry.expect("an entry").path();
            let text = std::fs::read_to_string(&path).expect("readable");
            let mut table: toml::Table = toml::from_str(&text).expect("TOML");
            assert_eq!(
                leave_out_other_settings(&mut table),
                Vec::<String>::new(),
                "{}: only presentation",
                path.display()
            );
            super::super::Settings::from_table(table)
                .unwrap_or_else(|message| panic!("{}: {message}", path.display()));
            count += 1;
        }
        assert!(count >= 4, "the default and the others: {count}");
    }
}
