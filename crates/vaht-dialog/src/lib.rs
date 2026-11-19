//! vaht's text dialogs: ordinary Wayland windows with a line to edit, such
//! as the run dialog.
//!
//! A dialog is a client like any other (01 §2): vaht frames it, focuses
//! it, lists it on the taskbar, and applies rules to it. It acts through
//! vaht's IPC, so what it runs starts from vaht itself, with vaht's
//! environment, and vaht reaps it.
//!
//! [`LineDialog`] is what a dialog is: a line, and what its keys do.
//! [`RunDialog`] runs a program, [`CommandDialog`] a vaht command, and
//! [`TitleDialog`] retitles a window; each asks vaht through a
//! [`Commander`]. The window (egui, in `vaht-settings`) draws the field and
//! edits its text.

mod command;
mod commander;
mod key;
mod line;
mod run;
mod title;

pub use command::CommandDialog;
pub use commander::{CommandError, Commander, IpcCommander};
pub use key::EditKey;
pub use run::RunDialog;
pub use title::TitleDialog;

/// A dialog: a line of text, with what Return, Escape, Tab, Up and Down do.
/// The window it is in edits the text itself and tells the dialog.
pub trait LineDialog {
    /// Returns the window's title, which may change after a key.
    fn title(&self) -> &str;

    /// Returns the window's application id, which rules match.
    fn app_id(&self) -> &str;

    /// Returns the line.
    fn text(&self) -> &str;

    /// Returns the cursor, as a byte offset into the line.
    fn cursor(&self) -> usize;

    /// Takes the line as the user's field has it now, the cursor at the byte
    /// offset `cursor`. Telling a dialog what it already has changes nothing.
    fn edit(&mut self, text: &str, cursor: usize);

    /// Does what `key` asks.
    ///
    /// # Errors
    ///
    /// Returns why the dialog could not do what was asked; the dialog then
    /// closes, and the window's program reports the error.
    fn press(&mut self, key: EditKey) -> Result<Outcome, String>;
}

/// Whether a dialog stays open after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It stays open.
    Open,
    /// It is done, and closes.
    Close,
}
