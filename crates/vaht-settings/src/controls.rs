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
use vaht_settings_model::{Alternative, Choice, Setting, SettingKind, Shape};

/// What a control keeps while it is being edited.
#[derive(Debug, Clone, PartialEq)]
enum Draft {
    /// A line of text.
    Text(String),
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
    /// Whether the layout supplies it (`settings.toml` does not set it).
    pub is_from_layout: bool,
    /// The names there are, for a setting that names a file (a style, a
    /// layout).
    pub names: &'setting [String],
}

impl SettingView<'_> {
    /// Draws the setting; returns the edit the user made, if they made one.
    pub fn show(self, ui: &mut Ui, drafts: &mut Drafts) -> Option<ConfigEdit> {
        let setting = self.setting;
        let mut edit = None;
        ui.horizontal_wrapped(|ui| {
            let name = ui.label(&setting.label);
            if !setting.description.is_empty() {
                name.clone().on_hover_text(&setting.description);
            }
            let mut is_set = self.value.is_some();
            // A name picks from a list, set or not: choosing sets it, and Reset
            // unsets it, so it has no Set box.
            let names_a_file = matches!(setting.kind, SettingKind::Name(_));
            if setting.is_optional && !names_a_file {
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
            if (is_set || names_a_file)
                && let Some(value) = (Control {
                    path: &setting.path,
                    kind: &setting.kind,
                    label: name.id,
                    names: self.names,
                })
                .show(ui, drafts, self.value.unwrap_or(&Value::Null))
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
            } else if self.is_from_layout {
                ui.small("from layout")
                    .on_hover_text("The layout supplies this; set it here to override it");
            }
        });
        // What the chosen form means, then what the setting is for.
        if let Some(note) = self.value.and_then(|value| setting.kind.note(value)) {
            ui.weak(note);
        }
        if !setting.description.is_empty() {
            ui.weak(&setting.description);
        }
        if edit.is_some() {
            // What was typed is superseded: a reset or a new choice shows
            // the setting's own value, not the old draft (or its error).
            drafts
                .by_setting
                .retain(|path, _| !path.parts().starts_with(setting.path.parts()));
        }
        edit
    }
}

/// The control for a value of one kind.
#[derive(Debug, Clone, Copy)]
struct Control<'control> {
    /// Where the value is (for what is typed into it, and for ids).
    path: &'control ConfigPath,
    /// What kind of value it is.
    kind: &'control SettingKind,
    /// The label that names it, for a screen reader.
    label: Id,
    /// The names there are, for a value that names a file.
    names: &'control [String],
}

impl<'control> Control<'control> {
    /// Returns the control for a value inside this one, at `path`.
    fn inside<'inner>(&self, path: &'inner ConfigPath, kind: &'inner SettingKind) -> Control<'inner>
    where
        'control: 'inner,
    {
        Control {
            path,
            kind,
            label: self.label,
            names: self.names,
        }
    }

    /// Draws the control, the value currently `current`; returns the value
    /// the user chose, if they did.
    fn show(&self, ui: &mut Ui, drafts: &mut Drafts, current: &Value) -> Option<Value> {
        let label = self.label;
        match self.kind {
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
            SettingKind::Text => self.text(ui, drafts, current),
            SettingKind::Name(_) => Self::name(ui, self.path, self.names, current, label),
            SettingKind::Choice(choices) => Self::choice(
                ui,
                self.path,
                choices,
                current.as_str().unwrap_or_default(),
                label,
            ),
            SettingKind::Alternatives(alternatives) => {
                self.alternatives(ui, drafts, alternatives, current)
            }
            SettingKind::List(item) => self.list(ui, drafts, item, current),
            // Scopes have a section each, which the window draws.
            SettingKind::Scopes(_) => None,
            SettingKind::Mapping(item) => self.mapping(ui, drafts, item, current),
            SettingKind::Raw => self.json(ui, drafts, current),
        }
    }

    /// Returns the path of `part`, a name inside the value at this path.
    fn child(&self, part: &str) -> ConfigPath {
        ConfigPath::from_parts(
            self.path
                .parts()
                .iter()
                .cloned()
                .chain([part.to_owned()])
                .collect(),
        )
        .unwrap_or_else(|| self.path.clone())
    }

    /// Values of one kind by name: a row for each, to change or remove, and
    /// a name to add one by. Sends the whole table when it changes.
    fn mapping(
        &self,
        ui: &mut Ui,
        drafts: &mut Drafts,
        item: &SettingKind,
        current: &Value,
    ) -> Option<Value> {
        let mut table = current.as_object().cloned().unwrap_or_default();
        let mut is_changed = false;
        let mut removed = None;
        let adding_path = self.child("+");
        let mut adding = match drafts.by_setting.remove(&adding_path) {
            Some(Draft::Text(text)) => text,
            Some(Draft::Json { .. }) | None => String::new(),
        };
        ui.vertical(|ui| {
            for (name, value) in &mut table {
                ui.horizontal(|ui| {
                    ui.label(name);
                    let path = self.child(name);
                    if let Some(chosen) = self.inside(&path, item).show(ui, drafts, value) {
                        *value = chosen;
                        is_changed = true;
                    }
                    if ui.small_button("Remove").clicked() {
                        removed = Some(name.clone());
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut adding).hint_text("name to add"));
                let name = adding.trim().to_owned();
                let can_add = !name.is_empty() && !table.contains_key(&name);
                if ui.add_enabled(can_add, egui::Button::new("Add")).clicked() {
                    table.insert(name, item.starting_value());
                    adding.clear();
                    is_changed = true;
                }
            });
        });
        if let Some(name) = removed {
            table.remove(&name);
            is_changed = true;
        }
        if !adding.is_empty() {
            drafts.by_setting.insert(adding_path, Draft::Text(adding));
        }
        is_changed.then_some(Value::Object(table))
    }

    /// A value written one of several ways: a list to say which, and
    /// beside it the control for the way chosen.
    fn alternatives(
        &self,
        ui: &mut Ui,
        drafts: &mut Drafts,
        alternatives: &[Alternative],
        current: &Value,
    ) -> Option<Value> {
        let active = alternatives
            .iter()
            .position(|alternative| alternative.is(current));
        let mut chosen = None;
        let combo = egui::ComboBox::from_id_salt(self.path.to_string())
            .selected_text(active.map_or("choose", |index| alternatives[index].label.as_str()))
            .show_ui(ui, |ui| {
                for (index, alternative) in alternatives.iter().enumerate() {
                    let response = ui.selectable_label(active == Some(index), &alternative.label);
                    if !alternative.description.is_empty() {
                        response.clone().on_hover_text(&alternative.description);
                    }
                    if response.clicked() && active != Some(index) {
                        chosen = Some(alternative.starting_value());
                    }
                }
            });
        combo.response.labelled_by(self.label);
        if chosen.is_some() {
            return chosen;
        }
        let shape = &alternatives[active?].shape;
        let inner_path = ConfigPath::from_parts(
            self.path
                .parts()
                .iter()
                .cloned()
                .chain(["#".to_owned()])
                .collect(),
        )
        .unwrap_or_else(|| self.path.clone());
        match shape {
            Shape::Exact(_) => None,
            Shape::Value(kind) => self.inside(&inner_path, kind).show(ui, drafts, current),
            Shape::Field { key, kind } => {
                let inner = current.get(key).unwrap_or(&Value::Null);
                self.inside(&inner_path, kind)
                    .show(ui, drafts, inner)
                    .map(|value| Value::Object(serde_json::Map::from_iter([(key.clone(), value)])))
            }
        }
    }

    /// The name of a file, chosen from the `names` there are; `current` is the
    /// name now, or none (the default).
    fn name(
        ui: &mut Ui,
        path: &ConfigPath,
        names: &[String],
        current: &Value,
        label: Id,
    ) -> Option<Value> {
        let now = current.as_str();
        let mut chosen = None;
        let combo = egui::ComboBox::from_id_salt(path.to_string())
            .selected_text(now.unwrap_or("(default)"))
            .show_ui(ui, |ui| {
                for (index, name) in names.iter().enumerate() {
                    // The default (vaht's own) is first, a line below it.
                    if index == 1 && names.first().is_some_and(|first| first == "default") {
                        ui.separator();
                    }
                    if ui
                        .selectable_label(now == Some(name.as_str()), name)
                        .clicked()
                        && now != Some(name.as_str())
                    {
                        chosen = Some(Value::String(name.clone()));
                    }
                }
            });
        combo.response.labelled_by(label);
        chosen
    }

    /// A line of text, sent when the user leaves it.
    fn text(&self, ui: &mut Ui, drafts: &mut Drafts, current: &Value) -> Option<Value> {
        let path = self.path;
        let mut text = match drafts.by_setting.remove(path) {
            Some(Draft::Text(text)) => text,
            Some(Draft::Json { .. }) | None => current.as_str().unwrap_or_default().to_owned(),
        };
        let response = ui.text_edit_singleline(&mut text).labelled_by(self.label);
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

    /// A list of values of one kind: each with the control for its kind,
    /// and (for all but numbers, which sit in one row) buttons to move and
    /// remove it, and one to add. Sends the whole list when it changes.
    fn list(
        &self,
        ui: &mut Ui,
        drafts: &mut Drafts,
        item: &SettingKind,
        current: &Value,
    ) -> Option<Value> {
        let mut items = current.as_array().cloned().unwrap_or_default();
        let is_compact = matches!(
            item,
            SettingKind::Whole { .. } | SettingKind::Decimal { .. }
        );
        let mut is_changed = false;
        let mut shape: Option<(usize, i8)> = None;
        let mut added = false;
        let mut layout = |ui: &mut Ui| {
            for (index, value) in items.iter_mut().enumerate() {
                let mut row = |ui: &mut Ui| {
                    let path = self.child(&index.to_string());
                    if let Some(chosen) = self.inside(&path, item).show(ui, drafts, value) {
                        *value = chosen;
                        is_changed = true;
                    }
                    if !is_compact {
                        if ui.small_button("Up").clicked() {
                            shape = Some((index, -1));
                        }
                        if ui.small_button("Down").clicked() {
                            shape = Some((index, 1));
                        }
                        if ui.small_button("Remove").clicked() {
                            shape = Some((index, 0));
                        }
                    }
                };
                if is_compact {
                    row(ui);
                } else {
                    ui.horizontal(row);
                }
            }
            if ui
                .small_button(if is_compact { "+" } else { "Add" })
                .clicked()
            {
                added = true;
            }
            if is_compact && ui.small_button("\u{2212}").clicked() {
                shape = Some((usize::MAX, 0));
            }
        };
        if is_compact {
            ui.horizontal(&mut layout);
        } else {
            ui.vertical(&mut layout);
        }
        if added {
            items.push(item.starting_value());
            is_changed = true;
        }
        if let Some((index, step)) = shape {
            let last = items.len().saturating_sub(1);
            is_changed = match step {
                0 if !items.is_empty() => {
                    items.remove(index.min(last));
                    true
                }
                -1 if index > 0 => {
                    items.swap(index, index - 1);
                    true
                }
                1 if index < last => {
                    items.swap(index, index + 1);
                    true
                }
                _ => is_changed,
            };
        }
        is_changed.then_some(Value::Array(items))
    }

    /// Anything else, as JSON text, sent when the user leaves it if it is
    /// valid JSON. No setting of `settings.toml` is shown this way.
    fn json(&self, ui: &mut Ui, drafts: &mut Drafts, current: &Value) -> Option<Value> {
        let path = self.path;
        let (mut text, mut error) = match drafts.by_setting.remove(path) {
            Some(Draft::Json { text, error }) => (text, error),
            Some(Draft::Text(_)) | None => (
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
            .labelled_by(self.label);
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
