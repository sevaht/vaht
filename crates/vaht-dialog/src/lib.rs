//! vaht's text dialogs: ordinary Wayland windows with a line to edit, such
//! as the run dialog (`vaht dialog run`).
//!
//! A dialog is a client like any other (01 §2): vaht frames it, focuses
//! it, lists it on the taskbar, and applies rules to it. It acts through
//! vaht's IPC, so what it runs starts from vaht itself, with vaht's
//! environment, and vaht reaps it.
//!
//! [`LineDialog`] is what a dialog is: a line, and what keys do to it.
//! [`RunDialog`] runs a program, [`CommandDialog`] a vaht command; both
//! ask vaht through a [`Commander`]. [`show`] puts a dialog in a window
//! drawn with vaht-ui in the theme's `dialog.field` style, and runs it
//! until it closes.

mod command;
mod commander;
mod key;
mod line;
mod run;
mod window;

pub use command::CommandDialog;
pub use commander::{CommandError, Commander, IpcCommander};
pub use key::EditKey;
pub use run::RunDialog;
pub use window::{DialogError, show};

/// A dialog: a line of text, edited by keys.
pub trait LineDialog {
    /// Returns the window's title, which may change after a key.
    fn title(&self) -> &str;

    /// Returns the window's application id, which rules match.
    fn app_id(&self) -> &str;

    /// Returns the line, with its cursor.
    fn field(&self) -> vaht_ui::TextField;

    /// Does what `key` asks.
    ///
    /// # Errors
    ///
    /// Returns why the dialog could not do what was asked; the dialog then
    /// closes, and [`show`] returns the error.
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
