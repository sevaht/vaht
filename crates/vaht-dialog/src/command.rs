//! The command dialog: a vaht command to type and run, as Fluxbox's
//! `CommandDialog` runs Fluxbox commands.

use crate::commander::{CommandError, Commander, IpcCommander};
use crate::line::LineEditor;
use crate::{EditKey, LineDialog, Outcome};

/// The command dialog.
pub struct CommandDialog {
    /// The line.
    line: LineEditor,
    /// The window's title: what the dialog is, or why vaht refused the
    /// last command.
    title: String,
    /// Runs what is typed.
    commander: Box<dyn Commander>,
}

impl CommandDialog {
    /// The window's application id, which rules match.
    pub const APP_ID: &str = "vaht-command";

    /// The window's title while no command was refused.
    const TITLE: &str = "Command";

    /// Returns a command dialog that runs what is typed through vaht.
    #[must_use]
    pub fn new() -> Self {
        Self::with_commander(Box::new(IpcCommander))
    }

    /// Returns a command dialog that runs what is typed with `commander`.
    #[must_use]
    pub fn with_commander(commander: Box<dyn Commander>) -> Self {
        Self {
            line: LineEditor::default(),
            title: Self::TITLE.to_owned(),
            commander,
        }
    }

    /// Runs the line, unless it is blank. A command vaht refuses leaves the
    /// dialog open, the line as it was, and the reason in the title.
    fn run(&mut self) -> Result<Outcome, String> {
        let command = self.line.text().trim().to_owned();
        if command.is_empty() {
            return Ok(Outcome::Close);
        }
        match self.commander.run(&command, None) {
            Ok(()) => Ok(Outcome::Close),
            Err(CommandError::Refused(reason)) => {
                self.title = format!("{}: {reason}", Self::TITLE);
                Ok(Outcome::Open)
            }
            Err(error @ CommandError::Unreachable(_)) => Err(error.to_string()),
        }
    }
}

impl Default for CommandDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CommandDialog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandDialog")
            .field("line", &self.line)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl LineDialog for CommandDialog {
    fn title(&self) -> &str {
        &self.title
    }

    fn app_id(&self) -> &str {
        Self::APP_ID
    }

    fn text(&self) -> &str {
        self.line.text()
    }

    fn cursor(&self) -> usize {
        self.line.cursor()
    }

    fn edit(&mut self, text: &str, cursor: usize) {
        self.line.edit(text, cursor);
    }

    fn press(&mut self, key: EditKey) -> Result<Outcome, String> {
        match key {
            EditKey::Accept => return self.run(),
            EditKey::Cancel => return Ok(Outcome::Close),
            // There is nothing to complete, or to go back through.
            EditKey::Complete | EditKey::Older | EditKey::Newer => {}
        }
        Ok(Outcome::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refuses `dance` as vaht does, and runs anything else.
    struct Picky;

    impl Commander for Picky {
        fn run(&mut self, command: &str, _: Option<u64>) -> Result<(), CommandError> {
            if command == "dance" {
                Err(CommandError::Refused("unknown action `dance`".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    /// Types `text` and presses Return.
    fn enter(dialog: &mut CommandDialog, text: &str) -> Result<Outcome, String> {
        dialog.edit(text, text.len());
        dialog.press(EditKey::Accept)
    }

    #[test]
    fn a_command_vaht_runs_closes_the_dialog() {
        let mut dialog = CommandDialog::with_commander(Box::new(Picky));
        assert_eq!(enter(&mut dialog, "workspace 2"), Ok(Outcome::Close));
    }

    #[test]
    fn a_refused_command_keeps_the_dialog_open_and_says_why() {
        let mut dialog = CommandDialog::with_commander(Box::new(Picky));
        assert_eq!(enter(&mut dialog, "dance"), Ok(Outcome::Open));
        assert_eq!(dialog.title(), "Command: unknown action `dance`");
        assert_eq!(dialog.text(), "dance", "the line stays, to fix");
    }
}
