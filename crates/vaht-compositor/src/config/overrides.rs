//! Settings changed while vaht runs (`set-setting`, `set-style`, `set-layout`), kept
//! apart from the files until `save-settings` writes them there (00 §3,
//! `SaveSettings`).
//!
//! A change is a key of `settings.toml` (`focus.model`, or
//! `input.device."My Mouse".accel_speed`) and a TOML value. It lasts across
//! reloads, so editing another file does not undo it; but if the user
//! edits the same setting in its file, the file wins and the change is
//! dropped.

use std::fmt;

use super::schema;
use vaht_ipc::{ConfigEdit, ConfigPath};

use super::{ConfigError, ConfigSource, Settings};

/// One setting changed: its key and value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingChange {
    /// The key's parts: `["focus", "model"]` for `focus.model`.
    key: Vec<String>,
    /// The value, written as TOML; none for a key put back to its default
    /// (taken out of the file when written).
    value: Option<String>,
}

impl SettingChange {
    /// Parses `<key> <value>`: the value in TOML (`true`, `30`,
    /// `"sloppy"`), or a bare word taken as a string (`sloppy`). A change
    /// `settings.toml` could not hold is refused.
    ///
    /// # Errors
    ///
    /// Returns what is wrong.
    pub(crate) fn parse(arguments: &str) -> Result<Self, String> {
        let (key, value) = Self::split_key(arguments)?;
        let value = if value.is_empty() {
            return Err("`set-setting` needs a key, then its value".to_owned());
        } else {
            toml::from_str::<toml::Table>(&format!("value = {value}"))
                .ok()
                .and_then(|mut table| table.remove("value"))
                .unwrap_or_else(|| toml::Value::String(value.to_owned()))
        };
        let change = Self {
            key,
            value: Some(value.to_string()),
        };
        let mut table = toml::Table::new();
        change.apply_to(&mut table)?;
        Settings::from_table(table).map_err(|error| format!("`{}`: {error}", change.key()))?;
        Ok(change)
    }

    /// Returns the change of `style` to `name`.
    pub(crate) fn style(name: &str) -> Self {
        Self::new(&["style"], &toml::Value::String(name.to_owned()))
    }

    /// Returns the change of the choice of the style's option `option` to
    /// `choice`.
    pub(crate) fn style_option(option: &str, choice: &str) -> Self {
        Self::new(
            &["style_options", option],
            &toml::Value::String(choice.to_owned()),
        )
    }

    /// Returns whether the change is of a choice of a style's option.
    pub(crate) fn is_style_option(&self) -> bool {
        self.key.first().is_some_and(|part| part == "style_options")
    }

    /// Returns the change of `layout` to `name`.
    pub(crate) fn layout(name: &str) -> Self {
        Self::new(&["layout"], &toml::Value::String(name.to_owned()))
    }

    /// Returns the change of `key`, its parts, to `value`.
    pub(crate) fn new(key: &[&str], value: &toml::Value) -> Self {
        Self::set(key.iter().map(|&part| part.to_owned()).collect(), value)
    }

    /// Returns the change of `key`, its parts, to `value`.
    pub(crate) fn set(key: Vec<String>, value: &toml::Value) -> Self {
        Self {
            key,
            value: Some(value.to_string()),
        }
    }

    /// Returns the change putting `key`, its parts, back to its default:
    /// the setting is left out of the file.
    pub(crate) const fn unset(key: Vec<String>) -> Self {
        Self { key, value: None }
    }

    /// Returns the change `edit`, one an IPC client sent, asks for; it is
    /// checked against the settings when made.
    ///
    /// # Errors
    ///
    /// Returns what in the edit cannot be a setting's value.
    pub(crate) fn of_edit(edit: &ConfigEdit) -> Result<Self, String> {
        match edit {
            ConfigEdit::Set { path, value } => {
                let converted = schema::toml_from_json(value)
                    .map_err(|message| format!("`{path}`: {message}"))?;
                Ok(Self::set(path.parts().to_vec(), &converted))
            }
            ConfigEdit::Unset { path } => Ok(Self::unset(path.parts().to_vec())),
        }
    }

    /// Parses `<key>`, to put back to its default (`unset-setting`).
    ///
    /// # Errors
    ///
    /// Returns what is wrong.
    pub(crate) fn parse_unset(arguments: &str) -> Result<Self, String> {
        let (key, rest) = Self::split_key(arguments)?;
        if !rest.is_empty() {
            return Err("`unset-setting` takes only a key".to_owned());
        }
        Ok(Self::unset(key))
    }

    /// Reads the key `arguments` begins with, as its parts, and returns it
    /// with the rest, the value.
    fn split_key(arguments: &str) -> Result<(Vec<String>, &str), String> {
        let (path, rest) = ConfigPath::parse_leading(arguments).map_err(|error| {
            format!("`set-setting` needs a key, such as focus.model, then its value: {error}")
        })?;
        Ok((path.parts().to_vec(), rest))
    }

    /// Returns the key as written: `focus.model`.
    fn key(&self) -> String {
        ConfigPath::from_parts(self.key.clone()).map_or_else(String::new, |path| path.to_string())
    }

    /// Returns the value, if the change gives one.
    fn value(&self) -> Option<toml::Value> {
        let written = self.value.as_ref()?;
        Some(
            toml::from_str::<toml::Table>(&format!("value = {written}"))
                .ok()
                .and_then(|mut table| table.remove("value"))
                .unwrap_or_else(|| toml::Value::String(written.clone())),
        )
    }

    /// Returns what `table` says at the key, if anything.
    fn read_from<'table>(&self, table: &'table toml::Table) -> Option<&'table toml::Value> {
        let (last, path) = self.key.split_last()?;
        let mut table = table;
        for part in path {
            table = table.get(part)?.as_table()?;
        }
        table.get(last)
    }

    /// Sets the key in `table` to the value, making the tables on the way;
    /// or takes the key out of `table`, if the change puts it back to its
    /// default.
    ///
    /// # Errors
    ///
    /// Returns what is wrong if a part of the key is not a table there.
    fn apply_to(&self, table: &mut toml::Table) -> Result<(), String> {
        let Some((last, path)) = self.key.split_last() else {
            return Err("a setting needs a key".to_owned());
        };
        let Some(value) = self.value() else {
            // Nothing to take out where a table on the way is missing.
            let mut table = Some(table);
            for part in path {
                table = table.and_then(|table| table.get_mut(part)?.as_table_mut());
            }
            if let Some(table) = table {
                table.remove(last);
            }
            return Ok(());
        };
        let mut table = table;
        for part in path {
            table = table
                .entry(part.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .ok_or_else(|| format!("`{}`: `{part}` is not a table", self.key()))?;
        }
        table.insert(last.clone(), value);
        Ok(())
    }
}

impl fmt::Display for SettingChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            Some(value) => write!(formatter, "{} {value}", self.key()),
            None => formatter.write_str(&self.key()),
        }
    }
}

/// A change made, and what the file said at its key then.
#[derive(Debug, Clone, PartialEq)]
struct Override {
    /// The change.
    change: SettingChange,
    /// What `settings.toml` said at the key when it was first changed;
    /// when the file says otherwise, the user changed it there, and the
    /// file wins.
    file_said: Option<toml::Value>,
}

/// The settings changed in place, not (yet) in the files.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SettingOverrides {
    /// Each change, one per key, oldest first.
    changes: Vec<Override>,
}

impl SettingOverrides {
    /// Returns these changes with `change` made too, `file` being what
    /// `settings.toml` says now.
    #[must_use]
    pub(crate) fn with(&self, change: SettingChange, file: &toml::Table) -> Self {
        let mut changes = self.changes.clone();
        let file_said = match changes
            .iter()
            .position(|made| made.change.key == change.key)
        {
            Some(index) => changes.remove(index).file_said,
            None => change.read_from(file).cloned(),
        };
        changes.push(Override { change, file_said });
        Self { changes }
    }

    /// Follows `settings.toml` read again as `file`: a change whose key
    /// the file now says otherwise than when it was made is dropped.
    pub(crate) fn follow_file(&mut self, file: &toml::Table) {
        self.changes.retain(|made| {
            let is_kept = made.change.read_from(file) == made.file_said.as_ref();
            if !is_kept {
                tracing::info!(
                    key = made.change.key(),
                    "a setting changed in place was edited in its file"
                );
            }
            is_kept
        });
    }

    /// Returns the key of each change, as its parts (`["focus", "model"]`),
    /// oldest first.
    pub(crate) fn keys(&self) -> Vec<Vec<String>> {
        self.changes
            .iter()
            .map(|made| made.change.key.clone())
            .collect()
    }

    /// Returns whether nothing is changed.
    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Makes the changes in `table`, `settings.toml`'s.
    ///
    /// # Errors
    ///
    /// Returns what is wrong if a change cannot be made there.
    pub(crate) fn apply_to(&self, table: &mut toml::Table) -> Result<(), String> {
        self.changes
            .iter()
            .try_for_each(|made| made.change.apply_to(table))
    }

    /// Writes the changes into the files `source` reads, and forgets them:
    /// the files say them now.
    ///
    /// # Errors
    ///
    /// Returns an error if a file cannot be read, parsed or written; the
    /// changes are kept.
    pub(crate) fn save(&mut self, source: &ConfigSource) -> Result<(), ConfigError> {
        let settings: Vec<_> = self
            .changes
            .iter()
            .map(|made| (made.change.key.as_slice(), made.change.value.as_deref()))
            .collect();
        source.write_settings(&settings)?;
        self.changes.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The change `arguments` makes.
    fn change(arguments: &str) -> SettingChange {
        SettingChange::parse(arguments).expect(arguments)
    }

    /// A `settings.toml` table.
    fn file(text: &str) -> toml::Table {
        toml::from_str(text).expect("valid TOML")
    }

    #[rstest]
    #[case::bare_word("focus.model sloppy", "focus.model \"sloppy\"")]
    #[case::toml_value("taskbar.auto_hide true", "taskbar.auto_hide true")]
    #[case::number("focus.auto_raise_delay_ms 300", "focus.auto_raise_delay_ms 300")]
    #[case::quoted_part(
        "input.device.\"My Mouse\".accel_speed 0.5",
        "input.device.\"My Mouse\".accel_speed 0.5"
    )]
    fn a_change_writes_what_it_reads(#[case] arguments: &str, #[case] written: &str) {
        assert_eq!(change(arguments).to_string(), written);
    }

    #[rstest]
    #[case::no_value("focus.model", "needs a key")]
    #[case::no_key("", "needs a key")]
    #[case::unknown_key("focus.modle sloppy", "unknown field")]
    #[case::wrong_type("taskbar.auto_hide maybe", "invalid type")]
    #[case::a_string_as_a_table("style.name x", "expected a string")]
    fn a_change_settings_cannot_hold_is_refused(#[case] arguments: &str, #[case] expected: &str) {
        let error = SettingChange::parse(arguments).expect_err(arguments);
        assert!(error.contains(expected), "{error}");
    }

    #[test]
    fn a_change_outlasts_reloads_until_its_file_says_otherwise() {
        let original = file("[focus]\nmodel = \"click\"\n");
        let overrides = SettingOverrides::default()
            .with(change("focus.model sloppy"), &original)
            .with(SettingChange::style("one"), &original);
        let mut table = original.clone();
        overrides.apply_to(&mut table).expect("applies");
        let settings = Settings::from_table(table).expect("valid");
        assert_eq!(settings.style.as_deref(), Some("one"));

        let mut followed = overrides.clone();
        followed.follow_file(&file(
            "[focus]\nmodel = \"click\"\n[taskbar]\nauto_hide = true\n",
        ));
        assert_eq!(followed, overrides, "another setting edited");
        followed.follow_file(&file("style = \"two\"\n[focus]\nmodel = \"click\"\n"));
        assert_eq!(
            followed,
            SettingOverrides::default().with(change("focus.model sloppy"), &original),
            "the file wins"
        );
    }

    #[test]
    fn changing_a_key_again_keeps_what_the_file_first_said() {
        let original = file("");
        let overrides = SettingOverrides::default()
            .with(change("focus.model sloppy"), &original)
            .with(
                change("focus.model click"),
                &file("[focus]\nmodel = \"x\"\n"),
            );
        let mut followed = overrides.clone();
        followed.follow_file(&original);
        assert_eq!(followed, overrides);
        assert!(!followed.is_empty());
    }
}
