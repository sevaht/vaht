//! The run dialog: a command line to run, with completion and history, as
//! fbrun and fluxland-run have.

mod completion;
mod history;

use std::path::PathBuf;

use vaht_ui::TextField;

use self::completion::{Completer, common_prefix};
use self::history::RunHistory;
use crate::commander::{Commander, IpcCommander};
use crate::line::LineEditor;
use crate::{EditKey, LineDialog, Outcome};

/// The names Tab goes through, one per press, once the word is as long
/// as they all agree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cycle {
    /// Where the word being completed starts.
    start: usize,
    /// The names.
    names: Vec<String>,
    /// The one the next press puts in.
    next: usize,
}

/// Where Up and Down are in the history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Browsing {
    /// The line shown, by index; none for the line being typed.
    position: Option<usize>,
    /// The line being typed, kept while older ones are shown.
    draft: String,
}

/// The run dialog.
pub struct RunDialog {
    /// The line.
    line: LineEditor,
    /// The lines run before.
    history: RunHistory,
    /// Where the history is kept; none to keep it only while open.
    history_path: Option<PathBuf>,
    /// Where Up and Down are.
    browsing: Browsing,
    /// What Tab goes through next, after a completion with several names.
    cycle: Option<Cycle>,
    /// Where names to complete come from.
    completer: Completer,
    /// Runs what is typed, as an `exec` command.
    commander: Box<dyn Commander>,
}

impl RunDialog {
    /// The window's application id, which rules match.
    pub const APP_ID: &str = "vaht-run";

    /// Returns a run dialog that keeps its history where vaht keeps it,
    /// completes from `PATH` and `HOME`, and runs lines through vaht.
    #[must_use]
    pub fn new() -> Self {
        let history_path = RunHistory::default_path();
        let history = history_path
            .as_deref()
            .map(RunHistory::load)
            .unwrap_or_default();
        Self::with_parts(
            history,
            history_path,
            Completer::from_environment(),
            Box::new(IpcCommander),
        )
    }

    /// Returns a run dialog of these parts.
    fn with_parts(
        history: RunHistory,
        history_path: Option<PathBuf>,
        completer: Completer,
        commander: Box<dyn Commander>,
    ) -> Self {
        Self {
            line: LineEditor::default(),
            history,
            history_path,
            browsing: Browsing::default(),
            cycle: None,
            completer,
            commander,
        }
    }

    /// Runs the line, unless it is blank, and remembers it.
    fn run(&mut self) -> Result<Outcome, String> {
        let line = self.line.text().trim_matches([' ', '\t']).to_owned();
        if line.is_empty() {
            return Ok(Outcome::Close);
        }
        self.history.add(&line);
        let saved = self.history_path.as_deref().map_or(Ok(()), |path| {
            self.history.save(path).map_err(|error| {
                format!("could not save the history to {}: {error}", path.display())
            })
        });
        self.commander
            .run(&format!("exec {line}"), None)
            .map_err(|error| error.to_string())?;
        saved.map(|()| Outcome::Close)
    }

    /// Finishes the word before the cursor as far as every name that could
    /// finish it agrees; with nothing more to add, the next presses go
    /// through the names one by one.
    fn complete(&mut self) {
        if let Some(cycle) = &mut self.cycle {
            self.line
                .replace_to_cursor(cycle.start, &cycle.names[cycle.next]);
            cycle.next = (cycle.next + 1) % cycle.names.len();
            return;
        }
        let start = self.line.word_start();
        let before = &self.line.text()[..start];
        let is_first = before.trim_matches([' ', '\t']).is_empty();
        let word = self.line.text()[start..self.line.cursor()].to_owned();
        let names = self.completer.complete(&word, is_first);
        let common = common_prefix(&names).to_owned();
        if common.len() > word.len() {
            self.line.replace_to_cursor(start, &common);
        }
        if names.len() > 1 {
            self.cycle = Some(Cycle {
                start,
                names,
                next: 0,
            });
        }
    }

    /// Shows the line run before the one shown.
    fn show_older(&mut self) {
        let position = match self.browsing.position {
            Some(0) => return,
            Some(position) => position - 1,
            None => {
                if self.history.lines().is_empty() {
                    return;
                }
                self.browsing.draft = self.line.text().to_owned();
                self.history.lines().len() - 1
            }
        };
        self.browsing.position = Some(position);
        self.line.set(&self.history.lines()[position]);
    }

    /// Shows the line run after the one shown, or the one being typed.
    fn show_newer(&mut self) {
        let Some(position) = self.browsing.position else {
            return;
        };
        let next = position + 1;
        if next < self.history.lines().len() {
            self.browsing.position = Some(next);
            self.line.set(&self.history.lines()[next]);
        } else {
            self.browsing.position = None;
            let draft = std::mem::take(&mut self.browsing.draft);
            self.line.set(&draft);
        }
    }
}

impl Default for RunDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RunDialog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunDialog")
            .field("line", &self.line)
            .field("history_path", &self.history_path)
            .finish_non_exhaustive()
    }
}

impl LineDialog for RunDialog {
    fn title(&self) -> &'static str {
        "Run program"
    }

    fn app_id(&self) -> &str {
        Self::APP_ID
    }

    fn field(&self) -> TextField {
        self.line.field()
    }

    fn press(&mut self, key: EditKey) -> Result<Outcome, String> {
        if key != EditKey::Complete {
            self.cycle = None;
        }
        match key {
            EditKey::Accept => return self.run(),
            EditKey::Cancel => return Ok(Outcome::Close),
            EditKey::Complete => self.complete(),
            EditKey::Older => self.show_older(),
            EditKey::Newer => self.show_newer(),
            EditKey::DeleteBack => self.line.delete_back(),
            EditKey::DeleteForward => self.line.delete_forward(),
            EditKey::Left => self.line.move_left(),
            EditKey::Right => self.line.move_right(),
            EditKey::Home => self.line.move_home(),
            EditKey::End => self.line.move_end(),
            EditKey::KillToStart => self.line.kill_to_start(),
            EditKey::KillToEnd => self.line.kill_to_end(),
            EditKey::Insert(text) => self.line.insert(&text),
        }
        Ok(Outcome::Open)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::rc::Rc;

    use super::*;
    use crate::commander::CommandError;

    /// Records the commands it is asked to run.
    #[derive(Debug, Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<String>>>);

    impl Commander for Recorder {
        fn run(&mut self, command: &str, _: Option<u64>) -> Result<(), CommandError> {
            self.0.borrow_mut().push(command.to_owned());
            Ok(())
        }
    }

    /// A run dialog with history `lines`, kept in `directory`, completing
    /// programs `kitty`, `kill` and `brave` there, recording what it runs.
    fn dialog(directory: &tempfile::TempDir, lines: &[&str]) -> (RunDialog, Recorder) {
        let bin = directory.path().join("bin");
        fs::create_dir_all(&bin).expect("a directory");
        for name in ["kitty", "kill", "brave"] {
            let path = bin.join(name);
            fs::write(&path, "").expect("a file");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("a mode");
        }
        let mut history = RunHistory::default();
        for line in lines {
            history.add(line);
        }
        let recorder = Recorder::default();
        let dialog = RunDialog::with_parts(
            history,
            Some(directory.path().join("history")),
            Completer::new(vec![bin], None),
            Box::new(recorder.clone()),
        );
        (dialog, recorder)
    }

    /// Types `text` into `dialog`.
    fn type_text(dialog: &mut RunDialog, text: &str) {
        for character in text.chars() {
            dialog
                .press(EditKey::Insert(character.to_string()))
                .expect("typing works");
        }
    }

    #[test]
    fn return_runs_the_trimmed_line_remembers_it_and_closes() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut dialog, recorder) = dialog(&directory, &[]);
        type_text(&mut dialog, "  kitty --hold ");
        assert_eq!(dialog.press(EditKey::Accept), Ok(Outcome::Close));
        assert_eq!(*recorder.0.borrow(), ["exec kitty --hold"]);
        let saved = RunHistory::load(&directory.path().join("history"));
        assert_eq!(saved.lines(), ["kitty --hold"]);
    }

    #[test]
    fn a_blank_line_closes_without_running_anything() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut dialog, recorder) = dialog(&directory, &[]);
        type_text(&mut dialog, "  ");
        assert_eq!(dialog.press(EditKey::Accept), Ok(Outcome::Close));
        assert!(recorder.0.borrow().is_empty());
    }

    #[test]
    fn tab_finishes_what_names_agree_on_then_goes_through_them() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut dialog, _) = dialog(&directory, &[]);
        type_text(&mut dialog, "k");
        let press_tab = |dialog: &mut RunDialog| {
            dialog.press(EditKey::Complete).expect("completes");
            dialog.field().text().to_owned()
        };
        assert_eq!(
            press_tab(&mut dialog),
            "ki",
            "as far as kill and kitty agree"
        );
        assert_eq!(press_tab(&mut dialog), "kill");
        assert_eq!(press_tab(&mut dialog), "kitty");
        assert_eq!(press_tab(&mut dialog), "kill", "around again");
        dialog.press(EditKey::End).expect("moves");
        assert_eq!(
            press_tab(&mut dialog),
            "kill",
            "a new completion: only kill"
        );
    }

    #[test]
    fn a_single_name_is_finished_at_once() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut dialog, _) = dialog(&directory, &[]);
        type_text(&mut dialog, "br");
        dialog.press(EditKey::Complete).expect("completes");
        assert_eq!(dialog.field().text(), "brave");
    }

    #[test]
    fn up_and_down_go_through_the_history_and_back_to_the_draft() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut dialog, _) = dialog(&directory, &["first", "second"]);
        type_text(&mut dialog, "draft");
        let press = |dialog: &mut RunDialog, key| {
            dialog.press(key).expect("browses");
            dialog.field().text().to_owned()
        };
        assert_eq!(press(&mut dialog, EditKey::Older), "second");
        assert_eq!(press(&mut dialog, EditKey::Older), "first");
        assert_eq!(press(&mut dialog, EditKey::Older), "first", "the oldest");
        assert_eq!(press(&mut dialog, EditKey::Newer), "second");
        assert_eq!(press(&mut dialog, EditKey::Newer), "draft");
        assert_eq!(press(&mut dialog, EditKey::Newer), "draft");
    }

    #[test]
    fn a_command_that_cannot_be_run_is_reported() {
        /// Reaches no vaht.
        struct Failing;
        impl Commander for Failing {
            fn run(&mut self, _: &str, _: Option<u64>) -> Result<(), CommandError> {
                Err(CommandError::Unreachable("no vaht".to_owned()))
            }
        }
        let mut dialog = RunDialog::with_parts(
            RunHistory::default(),
            None,
            Completer::new(Vec::new(), None),
            Box::new(Failing),
        );
        type_text(&mut dialog, "x");
        assert_eq!(dialog.press(EditKey::Accept), Err("no vaht".to_owned()));
    }
}
