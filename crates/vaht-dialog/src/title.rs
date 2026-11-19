//! The title dialog: one window's title, to change (Fluxbox's
//! `SetTitleDialog`).

use crate::commander::{CommandError, Commander, IpcCommander};
use crate::line::LineEditor;
use crate::{EditKey, LineDialog, Outcome};

/// The title dialog.
pub struct TitleDialog {
    /// The window, by id.
    window: u64,
    /// The line: the title, as it is to be.
    line: LineEditor,
    /// The window's title: what the dialog is, or why vaht refused.
    title: String,
    /// Sets the title.
    commander: Box<dyn Commander>,
}

impl TitleDialog {
    /// The window's application id, which rules match.
    pub const APP_ID: &str = "vaht-title";

    /// The window's title while vaht has refused nothing.
    const TITLE: &str = "Set Window Title";

    /// Returns a dialog for the window `window` (by id), whose title is
    /// `current`, that sets it through vaht.
    #[must_use]
    pub fn new(window: u64, current: &str) -> Self {
        Self::with_commander(window, current, Box::new(IpcCommander))
    }

    /// Returns a dialog for `window`, whose title is `current`, that sets
    /// it with `commander`.
    #[must_use]
    pub fn with_commander(window: u64, current: &str, commander: Box<dyn Commander>) -> Self {
        let mut line = LineEditor::default();
        line.set(current);
        Self {
            window,
            line,
            title: Self::TITLE.to_owned(),
            commander,
        }
    }

    /// Sets the title to the line; an empty line gives the window its
    /// program's title again.
    fn set(&mut self) -> Result<Outcome, String> {
        let title = self.line.text().trim();
        let command = if title.is_empty() {
            "set-title".to_owned()
        } else {
            format!("set-title {title}")
        };
        match self.commander.run(&command, Some(self.window)) {
            Ok(()) => Ok(Outcome::Close),
            Err(CommandError::Refused(reason)) => {
                self.title = format!("{}: {reason}", Self::TITLE);
                Ok(Outcome::Open)
            }
            Err(error @ CommandError::Unreachable(_)) => Err(error.to_string()),
        }
    }
}

impl std::fmt::Debug for TitleDialog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TitleDialog")
            .field("window", &self.window)
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

impl LineDialog for TitleDialog {
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
            EditKey::Accept => return self.set(),
            EditKey::Cancel => return Ok(Outcome::Close),
            EditKey::Complete | EditKey::Older | EditKey::Newer => {}
        }
        Ok(Outcome::Open)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// The commands run, and on which window.
    type Calls = Rc<RefCell<Vec<(String, Option<u64>)>>>;

    /// Records the commands it runs, and on which window.
    #[derive(Debug, Clone, Default)]
    struct Recorder(Calls);

    impl Commander for Recorder {
        fn run(&mut self, command: &str, window: Option<u64>) -> Result<(), CommandError> {
            self.0.borrow_mut().push((command.to_owned(), window));
            Ok(())
        }
    }

    #[test]
    fn the_line_starts_as_the_title_and_sets_it() {
        let recorder = Recorder::default();
        let mut dialog = TitleDialog::with_commander(7, "kitty", Box::new(recorder.clone()));
        assert_eq!(dialog.text(), "kitty");
        dialog.edit("kitty logs", 10);
        assert_eq!(dialog.press(EditKey::Accept), Ok(Outcome::Close));
        assert_eq!(
            *recorder.0.borrow(),
            [("set-title kitty logs".to_owned(), Some(7))]
        );
    }

    #[test]
    fn an_empty_line_gives_the_programs_title_back() {
        let recorder = Recorder::default();
        let mut dialog = TitleDialog::with_commander(7, "x", Box::new(recorder.clone()));
        dialog.edit("", 0);
        dialog.press(EditKey::Accept).expect("set");
        assert_eq!(*recorder.0.borrow(), [("set-title".to_owned(), Some(7))]);
    }
}
