//! The dialogs' window: one line in an egui window, for a [`LineDialog`]
//! (the run, command and title dialogs of `vaht-dialog`).
//!
//! egui edits the text; the dialog says what Return, Escape, Tab, Up and Down
//! do, and may change the line itself (a completion, a line from the history).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Ui};
use vaht_dialog::{EditKey, LineDialog, Outcome};

/// The window of a dialog.
pub struct DialogApp {
    /// What the dialog does.
    dialog: Box<dyn LineDialog>,
    /// The line, as the field shows it.
    text: String,
    /// The title last given the window.
    shown_title: String,
    /// Whether the field has been given the keyboard yet.
    is_focused: bool,
    /// The height the window was last made, to fit the field.
    fitted_height: Option<f32>,
    /// Why the dialog failed, once it has: the program then ends
    /// unsuccessfully, saying so.
    failure: Arc<Mutex<Option<String>>>,
}

impl DialogApp {
    /// The field's identity, for its cursor.
    fn field_id() -> egui::Id {
        egui::Id::NULL.with("dialog-line")
    }

    /// The room kept free round the field, in points.
    const MARGIN: i8 = 4;

    /// The window's size, in points: a line of text and the margin round it.
    const SIZE: [f32; 2] = [560.0, 36.0];

    /// The keys the dialog owns, and what they ask of it.
    const KEYS: [(egui::Key, EditKey); 5] = [
        (egui::Key::Enter, EditKey::Accept),
        (egui::Key::Escape, EditKey::Cancel),
        (egui::Key::Tab, EditKey::Complete),
        (egui::Key::ArrowUp, EditKey::Older),
        (egui::Key::ArrowDown, EditKey::Newer),
    ];

    /// Returns the window of `dialog`; `failure` is set to why, if the
    /// dialog fails.
    pub fn new(dialog: Box<dyn LineDialog>, failure: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            text: dialog.text().to_owned(),
            shown_title: dialog.title().to_owned(),
            dialog,
            is_focused: false,
            fitted_height: None,
            failure,
        }
    }

    /// Opens the window and runs until the dialog closes.
    ///
    /// # Errors
    ///
    /// Returns why the window cannot open, or why the dialog failed.
    pub fn run(dialog: Box<dyn LineDialog>) -> anyhow::Result<()> {
        let failure = Arc::new(Mutex::new(None));
        let size = Self::SIZE;
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title(dialog.title())
                .with_app_id(dialog.app_id())
                .with_inner_size(size)
                .with_min_inner_size(size)
                .with_max_inner_size(size)
                .with_resizable(false),
            ..eframe::NativeOptions::default()
        };
        let app = Self::new(dialog, Arc::clone(&failure));
        eframe::run_native(
            "vaht dialog",
            options,
            Box::new(|creation| {
                crate::fonts::install(&creation.egui_ctx);
                Ok(Box::new(app))
            }),
        )
        .map_err(|error| anyhow::anyhow!("the dialog window failed: {error}"))?;
        // A poisoned lock still holds what was stored.
        let failure = failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        failure.map_or(Ok(()), |reason| Err(anyhow::anyhow!(reason)))
    }

    /// Returns the cursor of the field as a byte offset into `text`, if the
    /// field has one.
    fn cursor_in(ctx: &egui::Context, text: &str) -> Option<usize> {
        let state = egui::TextEdit::load_state(ctx, Self::field_id())?;
        let chars: usize = state.cursor.char_range()?.primary.index.into();
        Some(
            text.char_indices()
                .nth(chars)
                .map_or(text.len(), |(at, _)| at),
        )
    }

    /// Puts the field's cursor at the byte offset `cursor` of `text`.
    fn put_cursor(ctx: &egui::Context, text: &str, cursor: usize) {
        let chars = text[..cursor.min(text.len())].chars().count();
        let mut state = egui::TextEdit::load_state(ctx, Self::field_id()).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(chars),
            )));
        state.store(ctx, Self::field_id());
    }

    /// Makes the window exactly as tall as a field `field_height` tall and
    /// the margin round it, once the font is known: a window made for a
    /// guessed height has room left over, or cuts the line.
    fn fit_window(&mut self, ctx: &egui::Context, field_height: f32) {
        let height = field_height + 2.0 * f32::from(Self::MARGIN);
        if self
            .fitted_height
            .is_some_and(|fitted| (fitted - height).abs() < 0.5)
        {
            return;
        }
        self.fitted_height = Some(height);
        let size = egui::vec2(Self::SIZE[0], height);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    /// Draws the window in `ui` and does what the keys ask.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // What the field has, as the dialog should know it before a key is
        // pressed (Tab completes the word before the cursor).
        if let Some(cursor) = Self::cursor_in(&ctx, &self.text) {
            self.dialog.edit(&self.text, cursor);
        }
        let pressed = ui.input_mut(|input| {
            Self::KEYS
                .into_iter()
                .find(|(key, _)| input.consume_key(egui::Modifiers::NONE, *key))
                .map(|(_, asked)| asked)
        });
        if let Some(asked) = pressed {
            match self.dialog.press(asked) {
                Ok(Outcome::Open) => {}
                Ok(Outcome::Close) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(message) => {
                    // The program reports it once the window has closed.
                    if let Ok(mut failure) = self.failure.lock() {
                        *failure = Some(message);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            // The dialog may have changed the line.
            self.dialog.text().clone_into(&mut self.text);
            Self::put_cursor(&ctx, &self.text, self.dialog.cursor());
        }
        // The field fills the window's width, a few points in from its edges.
        // Its height is a line of text, so the window is made as tall as that
        // and the margins round it (below): no room is left over.
        let output = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(Self::MARGIN)))
            .show(ui, |ui| {
                egui::TextEdit::singleline(&mut self.text)
                    .id(Self::field_id())
                    .desired_width(f32::INFINITY)
                    .lock_focus(true)
                    .show(ui)
            })
            .inner;
        self.fit_window(&ctx, output.response.rect.height());
        // The window's title names the field for a screen reader.
        let response = egui::Response::clone(&output.response).accessible_name(self.dialog.title());
        if !self.is_focused {
            response.request_focus();
            self.is_focused = true;
        }
        if response.changed()
            && let Some(cursor) = Self::cursor_in(&ctx, &self.text)
        {
            self.dialog.edit(&self.text, cursor);
        }
        if self.dialog.title() != self.shown_title {
            self.dialog.title().clone_into(&mut self.shown_title);
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.shown_title.clone()));
        }
    }
}

impl std::fmt::Debug for DialogApp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DialogApp")
            .field("text", &self.text)
            .finish_non_exhaustive()
    }
}

impl eframe::App for DialogApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;
    use vaht_dialog::{CommandDialog, CommandError, Commander, RunDialog};

    use super::*;

    /// Records the commands it is asked to run.
    #[derive(Debug, Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<String>>>);

    impl Commander for Recorder {
        fn run(&mut self, command: &str, _: Option<u64>) -> Result<(), CommandError> {
            self.0.borrow_mut().push(command.to_owned());
            Ok(())
        }
    }

    /// A window on `dialog`, in a harness.
    fn harness_of(app: &mut DialogApp) -> Harness<'_> {
        Harness::builder()
            .with_size(DialogApp::SIZE)
            .build_ui(|ui| app.show(ui))
    }

    fn app_of(dialog: Box<dyn LineDialog>) -> DialogApp {
        DialogApp::new(dialog, Arc::new(Mutex::new(None)))
    }

    #[test]
    fn what_is_typed_is_run_when_return_is_pressed() {
        let recorder = Recorder::default();
        let mut app = app_of(Box::new(RunDialog::detached(Box::new(recorder.clone()))));
        let mut harness = harness_of(&mut app);
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("kitty --hold");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(*recorder.0.borrow(), ["exec kitty --hold"]);
    }

    #[test]
    fn escape_closes_without_running_anything() {
        let recorder = Recorder::default();
        let mut app = app_of(Box::new(RunDialog::detached(Box::new(recorder.clone()))));
        let mut harness = harness_of(&mut app);
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("kitty");
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(recorder.0.borrow().is_empty());
    }

    #[test]
    fn a_command_vaht_refuses_leaves_the_line_to_fix() {
        /// Refuses everything, as vaht refuses an unknown action.
        struct Refusing;
        impl Commander for Refusing {
            fn run(&mut self, _: &str, _: Option<u64>) -> Result<(), CommandError> {
                Err(CommandError::Refused("unknown action `dance`".to_owned()))
            }
        }
        let mut app = app_of(Box::new(CommandDialog::with_commander(Box::new(Refusing))));
        let mut harness = harness_of(&mut app);
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("dance");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        drop(harness);
        assert_eq!(app.text, "dance", "the line stays");
        assert_eq!(app.dialog.title(), "Command: unknown action `dance`");
    }

    #[test]
    fn the_window_is_made_as_tall_as_its_field_and_margins() {
        let mut app = app_of(Box::new(RunDialog::detached(Box::new(Recorder::default()))));
        let mut harness = harness_of(&mut app);
        harness.run();
        let field = harness.get_by_role(egui::accesskit::Role::TextInput).rect();
        drop(harness);
        let margin = f32::from(DialogApp::MARGIN);
        assert_eq!(
            app.fitted_height,
            Some(field.height() + 2.0 * margin),
            "a line and the margins, nothing more: {field:?}"
        );
        // (The test harness adds a margin of its own to the window.)
        assert!(
            field.width() > DialogApp::SIZE[0] - 2.0 * (margin + 8.0) - 1.0,
            "as wide as the window less its margins: {field:?}"
        );
    }
}
