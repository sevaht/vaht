//! The window: a sidebar to search in, the form, and Save and Revert.
//!
//! [`SettingsApp`] draws what [`SettingsEditor`] decides. Each change the
//! user makes is sent to vaht at once and runs at once; Save writes the
//! changes into the file, and Revert drops them. Until vaht answers an edit
//! the value the user chose is shown (it is *pending*), so a value being
//! dragged does not snap back between frames.

use std::collections::HashMap;

use eframe::egui::{self, Ui};
use serde_json::Value;
use vaht_ipc::{ConfigEdit, ConfigPath};
use vaht_settings_model::{Form, Section, SettingsEditor};

use crate::controls::{Drafts, SettingView};
use crate::link::{Command, Link, Update, VahtLink};

/// What the window has to say about its connection to vaht.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    /// Waiting for the first reading.
    Loading,
    /// Showing what vaht says.
    Ready,
    /// vaht refused the last change, for this reason.
    Refused(String),
    /// vaht cannot be reached, or said something unreadable.
    Disconnected(String),
}

/// The settings window.
#[derive(Debug)]
pub struct SettingsApp<L: Link> {
    /// The line to vaht.
    link: L,
    /// The form and what it says, once vaht has sent both.
    editor: Option<SettingsEditor>,
    /// Edits sent and not yet answered, by setting: the value chosen, or
    /// none for "back to the default".
    pending: HashMap<ConfigPath, Option<Value>>,
    /// Text being typed.
    drafts: Drafts,
    /// What the search box holds.
    search: String,
    /// How the connection is.
    status: Status,
}

impl SettingsApp<VahtLink> {
    /// Opens the window on the vaht this runs under, and runs until it is
    /// closed.
    ///
    /// # Errors
    ///
    /// Returns why vaht cannot be reached, or the window cannot open.
    pub fn run() -> anyhow::Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("vaht settings")
                .with_app_id("vaht-settings")
                .with_inner_size([900.0, 650.0]),
            ..eframe::NativeOptions::default()
        };
        eframe::run_native(
            "vaht settings",
            options,
            Box::new(|creation| {
                let context = creation.egui_ctx.clone();
                let link = VahtLink::connect(move || context.request_repaint())
                    .map_err(|error| format!("vaht cannot be reached: {error}"))?;
                link.send(Command::Reload);
                Ok(Box::new(Self::new(link)))
            }),
        )
        .map_err(|error| anyhow::anyhow!("the settings window failed: {error}"))
    }
}

impl<L: Link> SettingsApp<L> {
    /// Returns a window on `link`, which has not told it anything yet.
    pub fn new(link: L) -> Self {
        Self {
            link,
            editor: None,
            pending: HashMap::new(),
            drafts: Drafts::default(),
            search: String::new(),
            status: Status::Loading,
        }
    }

    /// Takes what vaht has sent.
    fn take_updates(&mut self) {
        while let Some(update) = self.link.receive() {
            match update {
                Update::Schema(schema) => match Form::from_schema(&schema) {
                    Ok(form) => self.editor = Some(SettingsEditor::new(form)),
                    Err(error) => {
                        self.status = Status::Disconnected(format!(
                            "vaht's settings cannot be shown: {error}"
                        ));
                    }
                },
                Update::Reading {
                    value,
                    defaults,
                    changed,
                    themes: _,
                } => {
                    if let Some(editor) = &mut self.editor {
                        editor.read(value, defaults, &changed);
                    }
                    self.pending.clear();
                    if !matches!(self.status, Status::Refused(_) | Status::Disconnected(_)) {
                        self.status = Status::Ready;
                    }
                }
                Update::Refused(message) => {
                    self.pending.clear();
                    self.status = Status::Refused(message);
                }
                Update::Saved => self.status = Status::Ready,
                Update::Disconnected(message) => self.status = Status::Disconnected(message),
            }
        }
    }

    /// Sends `edit` to vaht, showing the value it chose until vaht answers.
    fn send(&mut self, edit: ConfigEdit) {
        let (path, value) = match &edit {
            ConfigEdit::Set { path, value } => (path.clone(), Some(value.clone())),
            ConfigEdit::Unset { path } => (path.clone(), None),
        };
        self.pending.insert(path, value);
        if self.status == Status::Ready || matches!(self.status, Status::Refused(_)) {
            self.status = Status::Ready;
        }
        self.link.send(Command::Edit(vec![edit]));
    }

    /// Draws the sidebar: the files, and the search.
    fn sidebar(&mut self, ui: &mut Ui) {
        ui.heading("Configuration");
        // The only file there is, until the others have pages.
        ui.strong("settings.toml");
        ui.separator();
        ui.label("Search");
        let response =
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("name or words"));
        if response.changed()
            && let Some(editor) = &mut self.editor
        {
            editor.search(&self.search);
        }
    }

    /// Draws the buttons and the line of status along the bottom.
    fn actions(&mut self, ui: &mut Ui) {
        let unsaved = self
            .editor
            .as_ref()
            .map_or(0, SettingsEditor::changed_count);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(unsaved > 0, egui::Button::new("Save"))
                .on_hover_text("Write the changes into settings.toml")
                .clicked()
            {
                self.link.send(Command::Save);
            }
            if ui
                .add_enabled(unsaved > 0, egui::Button::new("Revert"))
                .on_hover_text("Drop the changes: settings.toml runs as it says")
                .clicked()
            {
                self.link.send(Command::Revert);
            }
            match &self.status {
                Status::Loading => {
                    ui.label("Reading the settings…");
                }
                Status::Ready if unsaved == 0 => {
                    ui.label("Everything is saved.");
                }
                Status::Ready => {
                    ui.label(format!("{unsaved} changed, not saved yet."));
                }
                Status::Refused(message) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Not changed: {message}"),
                    );
                }
                Status::Disconnected(message) => {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
            }
        });
    }

    /// Draws `section`: its settings, then its sections, collapsed.
    fn section(&mut self, ui: &mut Ui, section: &Section, edits: &mut Vec<ConfigEdit>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let is_searching = !self.search.trim().is_empty();
        for setting in &section.settings {
            if !editor.is_shown(setting) {
                continue;
            }
            let value = match self.pending.get(&setting.path) {
                Some(pending) => pending
                    .as_ref()
                    .or_else(|| editor.default_of(&setting.path)),
                None => editor.value_of(&setting.path),
            };
            let view = SettingView {
                setting,
                value,
                is_default: editor.is_default(&setting.path)
                    && !self.pending.contains_key(&setting.path),
                is_changed: editor.is_changed_in_place(&setting.path),
            };
            if let Some(edit) = view.show(ui, &mut self.drafts) {
                edits.push(edit);
            }
            ui.add_space(4.0);
        }
        for inner in &section.sections {
            if !self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.has_shown_settings(inner))
            {
                continue;
            }
            egui::CollapsingHeader::new(&inner.label)
                .id_salt(inner.path.join("."))
                .default_open(is_searching)
                .open(is_searching.then_some(true))
                .show(ui, |ui| {
                    if !inner.description.is_empty() {
                        ui.weak(&inner.description);
                    }
                    self.section(ui, inner, edits);
                });
        }
    }
}

impl<L: Link> eframe::App for SettingsApp<L> {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl<L: Link> SettingsApp<L> {
    /// Draws the window in `ui`, after taking what vaht has sent; sends the
    /// edits the user makes.
    pub fn show(&mut self, ui: &mut Ui) {
        self.take_updates();
        egui::Panel::bottom("actions").show(ui, |ui| self.actions(ui));
        egui::Panel::left("sidebar")
            .default_size(220.0)
            .show(ui, |ui| self.sidebar(ui));
        let mut edits = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let Some(root) = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.form().root().clone())
                else {
                    ui.label("Waiting for vaht…");
                    return;
                };
                self.section(ui, &root, &mut edits);
            });
        });
        for edit in edits {
            self.send(edit);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;
    use serde_json::json;

    use super::*;

    /// A line to a pretend vaht: what the window sends is kept, and what it
    /// is to be told is handed over.
    #[derive(Debug, Clone, Default)]
    struct FakeLink {
        /// What the window sent.
        sent: Rc<RefCell<Vec<Command>>>,
        /// What it has yet to be told.
        to_tell: Rc<RefCell<VecDeque<Update>>>,
    }

    impl Link for FakeLink {
        fn send(&self, command: Command) {
            self.sent.borrow_mut().push(command);
        }

        fn receive(&self) -> Option<Update> {
            self.to_tell.borrow_mut().pop_front()
        }
    }

    /// The schema of a small settings file.
    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "focus": {
                    "type": "object",
                    "description": "How focus works.",
                    "properties": {
                        "raise_on_click": {
                            "type": "boolean", "default": true,
                            "description": "Whether clicking raises a window."
                        },
                        "model": { "enum": ["click", "sloppy"], "default": "click" },
                        "auto_raise_delay_ms": {
                            "type": "integer", "minimum": 0, "maximum": 5000, "default": 250
                        },
                    },
                },
            },
        })
    }

    /// Returns a window told the schema and what the settings say, and the
    /// line it uses.
    fn window() -> (SettingsApp<FakeLink>, FakeLink) {
        let link = FakeLink::default();
        link.to_tell.borrow_mut().extend([
            Update::Schema(schema()),
            Update::Reading {
                value: json!({ "focus": { "raise_on_click": true, "model": "click", "auto_raise_delay_ms": 250 } }),
                defaults: json!({ "focus": { "raise_on_click": true, "model": "click", "auto_raise_delay_ms": 250 } }),
                changed: Vec::new(),
                themes: Vec::new(),
            },
        ]);
        (SettingsApp::new(link.clone()), link)
    }

    #[test]
    fn the_window_shows_each_setting_by_its_name() {
        let (mut app, _link) = window();
        let mut harness = Harness::builder()
            .with_size([900.0, 650.0])
            .build_ui(|ui| app.show(ui));
        harness.run();
        harness.get_by_label("Focus").click();
        harness.run();
        harness.get_by_label("Raise on click");
        harness.get_by_label("Model");
        harness.get_by_label("Auto raise delay");
        harness.get_by_label("Everything is saved.");
    }

    /// Returns a harness drawing `app` at the size of the window.
    fn harness_of(app: &mut SettingsApp<FakeLink>) -> Harness<'_> {
        Harness::builder()
            .with_size([900.0, 650.0])
            .build_ui(|ui| app.show(ui))
    }

    /// Returns what the window sent, as the edits it asked for.
    fn edits_sent(link: &FakeLink) -> Vec<ConfigEdit> {
        link.sent
            .borrow()
            .iter()
            .filter_map(|command| match command {
                Command::Edit(edits) => Some(edits.clone()),
                Command::Reload | Command::Save | Command::Revert => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn switching_a_setting_sends_the_edit_for_its_new_value() {
        let (mut app, link) = window();
        let mut harness = harness_of(&mut app);
        harness.run();
        harness.get_by_label("Focus").click();
        harness.run();
        harness.get_by_role(egui::accesskit::Role::CheckBox).click();
        harness.run();
        assert_eq!(
            edits_sent(&link),
            [ConfigEdit::Set {
                path: ConfigPath::parse("focus.raise_on_click").expect("a key"),
                value: json!(false),
            }]
        );
    }

    #[test]
    fn what_differs_from_its_default_can_be_put_back() {
        let (mut app, link) = window();
        link.to_tell.borrow_mut().push_back(Update::Reading {
            value: json!({ "focus": { "raise_on_click": true, "model": "sloppy", "auto_raise_delay_ms": 250 } }),
            defaults: json!({ "focus": { "raise_on_click": true, "model": "click", "auto_raise_delay_ms": 250 } }),
            changed: Vec::new(),
            themes: Vec::new(),
        });
        let mut harness = harness_of(&mut app);
        harness.run();
        harness.get_by_label("Focus").click();
        harness.run();
        harness.get_by_label("Reset").click();
        harness.run();
        assert_eq!(
            edits_sent(&link),
            [ConfigEdit::Unset {
                path: ConfigPath::parse("focus.model").expect("a key")
            }],
            "only the setting that differs has a reset"
        );
    }

    #[test]
    fn changes_not_saved_are_counted_and_saved_or_reverted_on_request() {
        let (mut app, link) = window();
        link.to_tell.borrow_mut().push_back(Update::Reading {
            value: json!({ "focus": { "raise_on_click": true, "model": "sloppy", "auto_raise_delay_ms": 250 } }),
            defaults: json!({ "focus": { "raise_on_click": true, "model": "click", "auto_raise_delay_ms": 250 } }),
            changed: vec![vec!["focus".to_owned(), "model".to_owned()]],
            themes: Vec::new(),
        });
        let mut harness = harness_of(&mut app);
        harness.run();
        harness.get_by_label("1 changed, not saved yet.");
        harness.get_by_label("Save").click();
        harness.run();
        harness.get_by_label("Revert").click();
        harness.run();
        assert_eq!(*link.sent.borrow(), [Command::Save, Command::Revert]);
    }

    #[test]
    fn nothing_to_save_leaves_save_and_revert_unavailable() {
        let (mut app, link) = window();
        let mut harness = harness_of(&mut app);
        harness.run();
        harness.get_by_label("Save").click();
        harness.get_by_label("Revert").click();
        harness.run();
        assert!(link.sent.borrow().is_empty(), "{:?}", link.sent.borrow());
    }

    #[test]
    fn a_refusal_says_why() {
        let (mut app, link) = window();
        link.to_tell.borrow_mut().push_back(Update::Refused(
            "blur passes are from 1 to 6, not 99".to_owned(),
        ));
        let mut harness = harness_of(&mut app);
        harness.run();
        harness.get_by_label("Not changed: blur passes are from 1 to 6, not 99");
    }

    #[test]
    fn a_lost_connection_is_reported() {
        let (mut app, link) = window();
        link.to_tell.borrow_mut().push_back(Update::Disconnected(
            "vaht cannot be reached: closed".to_owned(),
        ));
        let mut harness = harness_of(&mut app);
        harness.run();
        harness.get_by_label("vaht cannot be reached: closed");
    }
}
