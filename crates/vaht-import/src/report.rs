//! What an import left behind, line by line, and why.

use std::fmt;
use std::path::{Path, PathBuf};

/// Why a source line was not converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// vaht deliberately has no such thing (tabs, the slit, X11 tuning).
    Dropped,
    /// vaht does not have it yet; importing again later may convert it.
    NotYetInVaht,
    /// The line could not be read.
    Unreadable,
}

impl fmt::Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Dropped => "dropped",
            Self::NotYetInVaht => "not in vaht yet",
            Self::Unreadable => "unreadable",
        })
    }
}

/// One source line not converted, or converted only in part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The source file.
    pub file: PathBuf,
    /// The line, counting from 1.
    pub line: usize,
    /// Why.
    pub reason: Reason,
    /// What was left out.
    pub detail: String,
}

impl fmt::Display for Note {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}: {}: {}",
            self.file.display(),
            self.line,
            self.reason,
            self.detail
        )
    }
}

/// Every note of an import, in the order found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The notes.
    notes: Vec<Note>,
}

impl Report {
    /// Returns the notes.
    #[must_use]
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// Notes that line `line` of `file` was not converted, for `reason`.
    pub(crate) fn note(
        &mut self,
        file: &Path,
        line: usize,
        reason: Reason,
        detail: impl Into<String>,
    ) {
        self.notes.push(Note {
            file: file.to_path_buf(),
            line,
            reason,
            detail: detail.into(),
        });
    }
}

/// One note per line.
impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for note in &self.notes {
            writeln!(formatter, "{note}")?;
        }
        Ok(())
    }
}
