//! The keys a dialog's line understands, apart from the keyboard that
//! sends them.

/// What a key asks of a dialog's line.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Backspace: delete the character before the cursor.
    DeleteBack,
    /// Delete: delete the character after the cursor.
    DeleteForward,
    /// Left: move the cursor back a character.
    Left,
    /// Right: move the cursor forward a character.
    Right,
    /// Home, or Ctrl+A: move the cursor to the start.
    Home,
    /// End, or Ctrl+E: move the cursor to the end.
    End,
    /// Ctrl+U: delete everything before the cursor.
    KillToStart,
    /// Ctrl+K: delete everything after the cursor.
    KillToEnd,
    /// A key that types: insert its text.
    Insert(String),
}
