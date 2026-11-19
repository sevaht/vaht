//! The keys a dialog handles itself, apart from the toolkit that sends them:
//! the field edits its own text.

/// What a key asks of a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    /// Return: do what the dialog is for with the line.
    Accept,
    /// Escape: close without doing anything.
    Cancel,
    /// Tab: finish the word before the cursor.
    Complete,
    /// Up: the line used before the one shown.
    Older,
    /// Down: the line used after the one shown.
    Newer,
}
