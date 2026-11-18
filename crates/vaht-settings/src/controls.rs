//! The control for each kind of setting.
//!
//! A [`SettingView`] draws one setting as a row (its name, a control for its
//! kind, its unit, a reset button, a note when it is changed and not saved,
//! and its documentation underneath) and returns the edit the user made, if
//! any. Text being typed lives in [`Drafts`] until it is finished, so a
//! half-typed value is never sent to vaht. Every control is labelled by the
//! setting's name, which is what a screen reader reads.

use std::collections::HashMap;

use eframe::egui::{self, Id, Response, Ui};
use serde_json::Value;
use vaht_ipc::{ConfigEdit, ConfigPath};
use vaht_settings_model::{Choice, Setting, SettingKind};

/// What a control keeps while it is being edited.
#[derive(Debug, Clone, PartialEq)]
enum Draft {
    /// A line of text.
    Text(String),
    /// The words of a list.
    Words(Vec<String>),
    /// JSON text, and why it is not valid, if it is not.
    Json {
        /// What was typed.
        text: String,
        /// Why it cannot be read as JSON.
        error: Option<String>,
    },
}

/// The drafts of the settings being edited.
#[derive(Debug, Clone, Default)]
pub struct Drafts {
    /// Each by the setting it is for.
    by_setting: HashMap<ConfigPath, Draft>,
}

/// One setting, to show and edit.
#[derive(Debug, Clone, Copy)]
pub struct SettingView<'setting> {
    /// The setting.
    pub setting: &'setting Setting,
    /// What it says now; none if it is not set.
    pub value: Option<&'setting Value>,
    /// Whether it says what vaht defaults to.
    pub is_default: bool,
    /// Whether it is changed and not yet written.
    pub is_changed: bool,
}

impl SettingView<'_> {
    /// Draws the setting; returns the edit the user made, if they made one.
    pub fn show(self, ui: &mut Ui, drafts: &mut Drafts) -> Option<ConfigEdit> {
        let setting = self.setting;
        let mut edit = None;
        ui.horizontal(|ui| {
            let name = ui.label(&setting.label);
            if !setting.description.is_empty() {
                name.clone().on_hover_text(&setting.description);
            }
            let mut is_set = self.value.is_some();
            if setting.is_optional {
                let toggle = ui.checkbox(&mut is_set, "Set").labelled_by(name.id);
                if toggle.changed() {
                    edit = Some(if is_set {
                        ConfigEdit::Set {
                            path: setting.path.clone(),
                            value: setting.kind.starting_value(),
                        }
                    } else {
                        ConfigEdit::Unset {
                            path: setting.path.clone(),
                        }
                    });
                }
            }
            if is_set
                && let Some(current) = self.value
                && let Some(value) = self.control(ui, drafts, current, name.id)
            {
                edit = Some(ConfigEdit::Set {
                    path: setting.path.clone(),
                    value,
                });
            }
            if let Some(unit) = setting.unit {
                ui.weak(unit);
            }
            if !self.is_default
                && ui
                    .small_button("Reset")
                    .on_hover_text("Back to the default")
                    .clicked()
            {
                edit = Some(ConfigEdit::Unset {
                    path: setting.path.clone(),
                });
            }
            if self.is_changed {
                ui.small("unsaved");
            }
        });
        if !setting.description.is_empty() {
            ui.weak(&setting.description);
        }
        edit
    }

    /// Draws the control for the setting's kind, currently set to `current`;
    /// returns the value the user chose, if they did.
    fn control(
        &self,
        ui: &mut Ui,
        drafts: &mut Drafts,
        current: &Value,
        label: Id,
    ) -> Option<Value> {
        let path = &self.setting.path;
        match &self.setting.kind {
            SettingKind::Toggle => {
                let mut is_on = current.as_bool().unwrap_or(false);
                let response = ui.checkbox(&mut is_on, "").labelled_by(label);
                response.changed().then_some(Value::Bool(is_on))
            }
            SettingKind::Whole { minimum, maximum } => {
                let mut number = current.as_i64().unwrap_or(0);
                let range = minimum.unwrap_or(i64::MIN)..=maximum.unwrap_or(i64::MAX);
                let response = ui
                    .add(egui::DragValue::new(&mut number).range(range))
                    .labelled_by(label);
                response.changed().then(|| Value::from(number))
            }
            SettingKind::Decimal { minimum, maximum } => {
                let mut number = current.as_f64().unwrap_or(0.0);
                let range = minimum.unwrap_or(f64::MIN)..=maximum.unwrap_or(f64::MAX);
                let response = ui
                    .add(egui::DragValue::new(&mut number).range(range).speed(0.01))
                    .labelled_by(label);
                response
                    .changed()
                    .then(|| serde_json::Number::from_f64(number).map(Value::Number))
                    .flatten()
            }
            SettingKind::Text => self.text(ui, drafts, current, label),
            SettingKind::Choice(choices) => Self::choice(
                ui,
                path,
                choices,
                current.as_str().unwrap_or_default(),
                label,
            ),
            SettingKind::List(item) => match item.as_ref() {
                SettingKind::Text | SettingKind::Choice(_) => {
                    self.words(ui, drafts, current, label)
                }
                SettingKind::Toggle
                | SettingKind::Whole { .. }
                | SettingKind::Decimal { .. }
                | SettingKind::List(_)
                | SettingKind::Mapping(_)
                | SettingKind::Raw => self.json(ui, drafts, current, label),
            },
            SettingKind::Mapping(_) | SettingKind::Raw => self.json(ui, drafts, current, label),
        }
    }

    /// A line of text, sent when the user leaves it.
    fn text(&self, ui: &mut Ui, drafts: &mut Drafts, current: &Value, label: Id) -> Option<Value> {
        let path = &self.setting.path;
        let mut text = match drafts.by_setting.remove(path) {
            Some(Draft::Text(text)) => text,
            Some(Draft::Words(_) | Draft::Json { .. }) | None => {
                current.as_str().unwrap_or_default().to_owned()
            }
        };
        let response = ui.text_edit_singleline(&mut text).labelled_by(label);
        if response.lost_focus() {
            return Some(Value::String(text));
        }
        if response.has_focus() {
            drafts.by_setting.insert(path.clone(), Draft::Text(text));
        }
        None
    }

    /// One word of several, chosen from a list.
    fn choice(
        ui: &mut Ui,
        path: &ConfigPath,
        choices: &[Choice],
        current: &str,
        label: Id,
    ) -> Option<Value> {
        let mut selected = current.to_owned();
        let combo = egui::ComboBox::from_id_salt(path.to_string())
            .selected_text(current)
            .show_ui(ui, |ui| {
                for choice in choices {
                    let response =
                        ui.selectable_value(&mut selected, choice.value.clone(), &choice.value);
                    if !choice.description.is_empty() {
                        response.on_hover_text(&choice.description);
                    }
                }
            });
        combo.response.labelled_by(label);
        (selected != current).then_some(Value::String(selected))
    }

    /// A list of words, each a line of text or a choice, that can be
    /// added to, removed from and reordered; sent when an item is left or
    /// the list's shape changes.
    fn words(&self, ui: &mut Ui, drafts: &mut Drafts, current: &Value, label: Id) -> Option<Value> {
        let SettingKind::List(boxed_item) = &self.setting.kind else {
            return None;
        };
        let item = boxed_item.as_ref();
        let path = &self.setting.path;
        let mut words = match drafts.by_setting.remove(path) {
            Some(Draft::Words(words)) => words,
            Some(Draft::Text(_) | Draft::Json { .. }) | None => current
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|word| word.as_str().unwrap_or_default().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
        };
        let (mut is_done, mut is_editing) = (false, false);
        let mut shape: Option<(usize, i8)> = None;
        ui.vertical(|ui| {
            for (index, word) in words.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    match item {
                        SettingKind::Choice(choices) => {
                            let before = word.clone();
                            let changed = Self::choice(
                                ui,
                                &ConfigPath::from_parts(vec![path.to_string(), index.to_string()])
                                    .unwrap_or_else(|| path.clone()),
                                choices,
                                &before,
                                label,
                            );
                            if let Some(Value::String(chosen)) = changed {
                                *word = chosen;
                                is_done = true;
                            }
                        }
                        SettingKind::Text
                        | SettingKind::Toggle
                        | SettingKind::Whole { .. }
                        | SettingKind::Decimal { .. }
                        | SettingKind::List(_)
                        | SettingKind::Mapping(_)
                        | SettingKind::Raw => {
                            let response = ui.text_edit_singleline(word).labelled_by(label);
                            is_done |= response.lost_focus();
                            is_editing |= response.has_focus();
                        }
                    }
                    if ui.small_button("Up").clicked() {
                        shape = Some((index, -1));
                    }
                    if ui.small_button("Down").clicked() {
                        shape = Some((index, 1));
                    }
                    if ui.small_button("Remove").clicked() {
                        shape = Some((index, 0));
                    }
                });
            }
            if ui.small_button("Add").clicked() {
                words.push(match item {
                    SettingKind::Choice(choices) => choices
                        .first()
                        .map(|choice| choice.value.clone())
                        .unwrap_or_default(),
                    SettingKind::Text
                    | SettingKind::Toggle
                    | SettingKind::Whole { .. }
                    | SettingKind::Decimal { .. }
                    | SettingKind::List(_)
                    | SettingKind::Mapping(_)
                    | SettingKind::Raw => String::new(),
                });
                is_done = true;
            }
        });
        if let Some((index, step)) = shape {
            match step {
                0 => {
                    words.remove(index);
                }
                -1 if index > 0 => words.swap(index, index - 1),
                1 if index + 1 < words.len() => words.swap(index, index + 1),
                _ => {}
            }
            is_done = true;
        }
        if is_done {
            return Some(Value::Array(words.into_iter().map(Value::String).collect()));
        }
        if is_editing {
            drafts.by_setting.insert(path.clone(), Draft::Words(words));
        }
        None
    }

    /// Anything else, as JSON text, sent when the user leaves it if it is
    /// valid JSON.
    fn json(&self, ui: &mut Ui, drafts: &mut Drafts, current: &Value, label: Id) -> Option<Value> {
        let path = &self.setting.path;
        let (mut text, mut error) = match drafts.by_setting.remove(path) {
            Some(Draft::Json { text, error }) => (text, error),
            Some(Draft::Text(_) | Draft::Words(_)) | None => (
                serde_json::to_string_pretty(current).unwrap_or_default(),
                None,
            ),
        };
        let response: Response = ui
            .add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .desired_rows(2),
            )
            .labelled_by(label);
        let mut chosen = None;
        if response.lost_focus() {
            match serde_json::from_str::<Value>(&text) {
                Ok(value) => chosen = Some(value),
                Err(parse_error) => error = Some(parse_error.to_string()),
            }
        }
        if let Some(message) = &error {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        if chosen.is_none() && (response.has_focus() || error.is_some()) {
            drafts
                .by_setting
                .insert(path.clone(), Draft::Json { text, error });
        }
        chosen
    }
}
