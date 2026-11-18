//! The settings editor: the form, what the settings say now, and what the
//! user has done with them.
//!
//! [`SettingsEditor`] holds the [`Form`] and the latest reading of the
//! settings (their effective values, vaht's defaults, and the keys changed in
//! place and not written). It answers what the window needs to draw (the
//! value of a setting, whether it differs from its default, whether it is
//! changed and unsaved, whether it matches the search) and turns a change
//! into the edit to send. It sends nothing: the window does, and gives
//! the next reading back.

use serde_json::Value;
use vaht_ipc::{ConfigEdit, ConfigPath};

use crate::form::{Form, Section, Setting};

/// The form and the state of its settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsEditor {
    /// The settings to show.
    form: Form,
    /// What they say now, an object in the file's shape.
    value: Value,
    /// What vaht uses when the file leaves a setting out, in the same
    /// shape.
    defaults: Value,
    /// The keys changed in place and not yet written.
    changed: Vec<ConfigPath>,
    /// The layout in use, if there is one.
    layout: Option<String>,
    /// The settings the layout supplies: what it sets that `settings.toml`
    /// does not.
    from_layout: Vec<ConfigPath>,
    /// What the user searched for, lowercase; empty for everything.
    query: String,
}

impl SettingsEditor {
    /// Returns an editor of `form` that has not been told what the settings
    /// say.
    #[must_use]
    pub const fn new(form: Form) -> Self {
        Self {
            form,
            value: Value::Null,
            defaults: Value::Null,
            changed: Vec::new(),
            layout: None,
            from_layout: Vec::new(),
            query: String::new(),
        }
    }

    /// Takes a reading of the settings: `value` (what they say now),
    /// `defaults`, and `changed`, the keys changed in place.
    pub fn read(&mut self, value: Value, defaults: Value, changed: &[Vec<String>]) {
        self.value = value;
        self.defaults = defaults;
        self.changed = changed
            .iter()
            .filter_map(|parts| ConfigPath::from_parts(parts.clone()))
            .collect();
    }

    /// Takes the layout in use and the settings it supplies.
    pub fn set_layout(&mut self, layout: Option<String>, from_layout: &[Vec<String>]) {
        self.layout = layout;
        self.from_layout = from_layout
            .iter()
            .filter_map(|parts| ConfigPath::from_parts(parts.clone()))
            .collect();
    }

    /// Returns the layout in use, if there is one.
    #[must_use]
    pub fn layout(&self) -> Option<&str> {
        self.layout.as_deref()
    }

    /// Returns whether the layout supplies `path`: it sets it, and
    /// `settings.toml` does not.
    #[must_use]
    pub fn is_from_layout(&self, path: &ConfigPath) -> bool {
        self.from_layout.contains(path)
    }

    /// Returns the form.
    #[must_use]
    pub const fn form(&self) -> &Form {
        &self.form
    }

    /// Returns what `path` says now; none if it is not set.
    #[must_use]
    pub fn value_of(&self, path: &ConfigPath) -> Option<&Value> {
        Self::at(&self.value, path)
    }

    /// Returns what vaht uses when the file leaves `path` out; none if
    /// there is nothing to use (an optional setting).
    #[must_use]
    pub fn default_of(&self, path: &ConfigPath) -> Option<&Value> {
        Self::at(&self.defaults, path)
    }

    /// Returns whether `path` says what vaht defaults to.
    #[must_use]
    pub fn is_default(&self, path: &ConfigPath) -> bool {
        self.value_of(path) == self.default_of(path)
    }

    /// Returns whether `path` is changed in place and not yet written.
    #[must_use]
    pub fn is_changed_in_place(&self, path: &ConfigPath) -> bool {
        self.changed.contains(path)
    }

    /// Returns how many settings are changed in place and not yet written.
    #[must_use]
    pub const fn changed_count(&self) -> usize {
        self.changed.len()
    }

    /// Returns the edit that sets `setting` to `value`.
    #[must_use]
    pub fn set(&self, setting: &Setting, value: Value) -> ConfigEdit {
        ConfigEdit::Set {
            path: setting.path.clone(),
            value,
        }
    }

    /// Returns the edit that puts `setting` back to its default.
    #[must_use]
    pub fn reset(&self, setting: &Setting) -> ConfigEdit {
        ConfigEdit::Unset {
            path: setting.path.clone(),
        }
    }

    /// Searches for `text`: only the settings whose name, key or
    /// documentation contains it (any case) are shown. Empty shows all.
    pub fn search(&mut self, text: &str) {
        self.query = text.trim().to_lowercase();
    }

    /// Returns whether `setting` matches the search.
    #[must_use]
    pub fn is_shown(&self, setting: &Setting) -> bool {
        self.query.is_empty()
            || [
                setting.label.as_str(),
                &setting.path.to_string(),
                setting.description.as_str(),
            ]
            .iter()
            .any(|text| text.to_lowercase().contains(&self.query))
    }

    /// Returns whether `section`, or one inside it, has a setting the
    /// search shows.
    #[must_use]
    pub fn has_shown_settings(&self, section: &Section) -> bool {
        section
            .all_settings()
            .into_iter()
            .any(|setting| self.is_shown(setting))
    }

    /// Returns what `value` has at `path`, if anything but `null`.
    fn at<'value>(value: &'value Value, path: &ConfigPath) -> Option<&'value Value> {
        let mut node = value;
        for part in path.parts() {
            node = node.get(part)?;
        }
        (!node.is_null()).then_some(node)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Returns an editor of a small form, with settings read.
    fn editor() -> SettingsEditor {
        let form = Form::from_schema(&json!({
            "type": "object",
            "properties": {
                "focus": {
                    "type": "object",
                    "properties": {
                        "auto_raise": {
                            "type": "boolean", "default": false,
                            "description": "Whether a focused window rises."
                        },
                        "model": { "enum": ["click", "sloppy"], "default": "click" },
                    },
                },
                "style": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            },
        }))
        .expect("a form");
        let mut editor = SettingsEditor::new(form);
        editor.read(
            json!({ "focus": { "auto_raise": false, "model": "sloppy" }, "style": null }),
            json!({ "focus": { "auto_raise": false, "model": "click" }, "style": null }),
            &[vec!["focus".to_owned(), "model".to_owned()]],
        );
        editor
    }

    /// Returns the path of the dotted `key`.
    fn path(key: &str) -> ConfigPath {
        ConfigPath::parse(key).expect("a key")
    }

    #[test]
    fn a_setting_differs_from_its_default_when_it_says_otherwise() {
        let editor = editor();
        assert!(
            !editor.is_default(&path("focus.model")),
            "sloppy, not click"
        );
        assert!(editor.is_default(&path("focus.auto_raise")));
        assert!(
            editor.is_default(&path("style")),
            "unset, with nothing to default to"
        );
        assert_eq!(
            editor.value_of(&path("focus.model")),
            Some(&json!("sloppy"))
        );
        assert_eq!(
            editor.default_of(&path("focus.model")),
            Some(&json!("click"))
        );
        assert_eq!(editor.value_of(&path("style")), None);
    }

    #[test]
    fn the_keys_changed_in_place_are_known() {
        let editor = editor();
        assert!(editor.is_changed_in_place(&path("focus.model")));
        assert!(!editor.is_changed_in_place(&path("focus.auto_raise")));
        assert_eq!(editor.changed_count(), 1);
    }

    #[test]
    fn a_change_is_the_edit_to_send() {
        let editor = editor();
        let model = editor
            .form()
            .setting(&path("focus.model"))
            .expect("there")
            .clone();
        assert_eq!(
            editor.set(&model, json!("click")),
            ConfigEdit::Set {
                path: path("focus.model"),
                value: json!("click")
            }
        );
        assert_eq!(
            editor.reset(&model),
            ConfigEdit::Unset {
                path: path("focus.model")
            }
        );
    }

    #[test]
    fn a_search_shows_the_settings_whose_words_or_documentation_hold_it() {
        let mut editor = editor();
        let shown = |editor: &SettingsEditor| -> Vec<String> {
            editor
                .form()
                .settings()
                .into_iter()
                .filter(|setting| editor.is_shown(setting))
                .map(|setting| setting.path.to_string())
                .collect()
        };
        assert_eq!(shown(&editor).len(), 3, "no search shows everything");
        editor.search("  RISES ");
        assert_eq!(
            shown(&editor),
            ["focus.auto_raise"],
            "the documentation, in any case"
        );
        editor.search("focus.mod");
        assert_eq!(shown(&editor), ["focus.model"], "the key");
        editor.search("nothing like it");
        assert_eq!(shown(&editor), Vec::<String>::new());
        assert!(!editor.has_shown_settings(editor.form().root()));
    }

    #[test]
    fn the_layout_in_use_and_what_it_supplies_are_known() {
        let mut editor = editor();
        assert_eq!(editor.layout(), None);
        editor.set_layout(
            Some("slim".to_owned()),
            &[vec!["focus".to_owned(), "model".to_owned()]],
        );
        assert_eq!(editor.layout(), Some("slim"));
        assert!(editor.is_from_layout(&path("focus.model")));
        assert!(!editor.is_from_layout(&path("focus.auto_raise")));
    }
}
