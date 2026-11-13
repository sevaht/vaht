//! The command lines run before, kept between runs of the dialog.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

/// The command lines run before, oldest first, each once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RunHistory {
    /// The lines, oldest first.
    lines: Vec<String>,
}

impl RunHistory {
    /// How many lines are kept: the oldest go first.
    const MAX_LINES: usize = 200;

    /// Returns where the history is kept: `$XDG_STATE_HOME/vaht/run_history`,
    /// normally `~/.local/state/vaht/run_history`; none without a home.
    pub(crate) fn default_path() -> Option<PathBuf> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|state| !state.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
                Some(Path::new(&home).join(".local/state"))
            })?;
        Some(state.join("vaht/run_history"))
    }

    /// Reads the history at `path`; empty if there is none yet, or it
    /// cannot be read.
    pub(crate) fn load(path: &Path) -> Self {
        let mut history = Self::default();
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                history.add(line);
            }
        }
        history
    }

    /// Returns the lines, oldest first.
    pub(crate) fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Adds `line` as the newest, moving it there if it was run before,
    /// and forgetting the oldest beyond the limit.
    pub(crate) fn add(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        self.lines.retain(|old| old != line);
        self.lines.push(line.to_owned());
        let excess = self.lines.len().saturating_sub(Self::MAX_LINES);
        self.lines.drain(..excess);
    }

    /// Writes the history to `path`, readable by the user only, replacing
    /// what was there all at once: a temporary file renamed over it.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory or file cannot be written.
    pub(crate) fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(directory) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
        }
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(format!(".{}.tmp", std::process::id()));
        let temporary = PathBuf::from(temporary);
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .and_then(|mut file| {
                let mut text = self.lines.join("\n");
                text.push('\n');
                file.write_all(text.as_bytes())
            })
            .and_then(|()| fs::rename(&temporary, path));
        if written.is_err() {
            // The temporary file, if made, is of no use; its own error
            // matters less than the one being returned.
            let _ = fs::remove_file(&temporary);
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn a_line_run_again_moves_to_the_newest() {
        let mut history = RunHistory::default();
        for line in ["kitty", "brave", "kitty", ""] {
            history.add(line);
        }
        assert_eq!(history.lines(), ["brave", "kitty"]);
    }

    #[test]
    fn the_oldest_lines_go_beyond_the_limit() {
        let mut history = RunHistory::default();
        for number in 0..=RunHistory::MAX_LINES {
            history.add(&number.to_string());
        }
        assert_eq!(history.lines().len(), RunHistory::MAX_LINES);
        assert_eq!(history.lines()[0], "1");
    }

    #[test]
    fn the_history_is_saved_privately_and_read_back() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state/vaht/run_history");
        let mut history = RunHistory::default();
        history.add("make -j8");
        history.add("kitty");
        history.save(&path).expect("saved");
        assert_eq!(RunHistory::load(&path), history);
        let mode = fs::metadata(&path).expect("written").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let leftovers = fs::read_dir(path.parent().expect("a directory"))
            .expect("readable")
            .count();
        assert_eq!(leftovers, 1, "no temporary file stays");
    }
}
