//! A line of text being edited: its text, and where the cursor is.

use vaht_ui::TextField;

/// A line being edited, as a shell's line editor edits it.
///
/// The cursor is a byte offset, always at a character boundary: every
/// method moves it by whole characters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LineEditor {
    /// The text.
    text: String,
    /// The cursor, as a byte offset into the text.
    cursor: usize,
}

impl LineEditor {
    /// The longest line, in bytes: as long as fluxland-run's.
    const MAX_BYTES: usize = 1023;

    /// Returns the text.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Returns the cursor, as a byte offset into the text.
    pub(crate) const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Returns the line as a field widget shows it.
    pub(crate) fn field(&self) -> TextField {
        TextField::new(self.text.clone(), self.cursor)
    }

    /// Returns where the word before the cursor starts: just after the
    /// last space or tab before the cursor.
    pub(crate) fn word_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind([' ', '\t'])
            .map_or(0, |space| space + 1)
    }

    /// Replaces the whole line with `text`, the cursor at its end.
    pub(crate) fn set(&mut self, text: &str) {
        text.clone_into(&mut self.text);
        self.cursor = self.text.len();
    }

    /// Inserts `text` at the cursor, unless the line would grow too long.
    pub(crate) fn insert(&mut self, text: &str) {
        if self.text.len() + text.len() <= Self::MAX_BYTES {
            self.text.insert_str(self.cursor, text);
            self.cursor += text.len();
        }
    }

    /// Replaces the text from `start` to the cursor with `text`, the cursor
    /// after it, unless the line would grow too long.
    pub(crate) fn replace_to_cursor(&mut self, start: usize, text: &str) {
        let length = self.text.len() - (self.cursor - start) + text.len();
        if length <= Self::MAX_BYTES {
            self.text.replace_range(start..self.cursor, text);
            self.cursor = start + text.len();
        }
    }

    /// Deletes the character before the cursor.
    pub(crate) fn delete_back(&mut self) {
        let start = self.previous_boundary();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Deletes the character after the cursor.
    pub(crate) fn delete_forward(&mut self) {
        let end = self.next_boundary();
        self.text.replace_range(self.cursor..end, "");
    }

    /// Moves the cursor back a character.
    pub(crate) fn move_left(&mut self) {
        self.cursor = self.previous_boundary();
    }

    /// Moves the cursor forward a character.
    pub(crate) fn move_right(&mut self) {
        self.cursor = self.next_boundary();
    }

    /// Moves the cursor to the start.
    pub(crate) const fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end.
    pub(crate) const fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Deletes everything before the cursor (a shell's Ctrl+U).
    pub(crate) fn kill_to_start(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    /// Deletes everything after the cursor (a shell's Ctrl+K).
    pub(crate) fn kill_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    /// Returns where the character before the cursor starts.
    fn previous_boundary(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(start, _)| start)
    }

    /// Returns where the character after the cursor ends.
    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |character| self.cursor + character.len_utf8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a line holding `text`, the cursor at its end.
    fn line(text: &str) -> LineEditor {
        let mut line = LineEditor::default();
        line.set(text);
        line
    }

    #[test]
    fn editing_moves_by_whole_characters() {
        let mut editor = line("né");
        editor.delete_back();
        assert_eq!(editor.text(), "n");
        editor.insert("ü");
        editor.move_left();
        assert_eq!(editor.cursor(), 1);
        editor.delete_forward();
        assert_eq!(editor.text(), "n");
    }

    #[test]
    fn the_shell_kills_cut_before_and_after_the_cursor() {
        let mut editor = line("make install");
        editor.move_home();
        for _ in 0..4 {
            editor.move_right();
        }
        editor.kill_to_end();
        assert_eq!(editor.text(), "make");
        editor.move_left();
        editor.kill_to_start();
        assert_eq!((editor.text(), editor.cursor()), ("e", 0));
    }

    #[test]
    fn the_word_before_the_cursor_starts_after_a_space() {
        let editor = line("ls ~/sc");
        assert_eq!(editor.word_start(), 3);
        assert_eq!(line("kit").word_start(), 0);
    }

    #[test]
    fn a_word_is_replaced_up_to_the_cursor() {
        let mut editor = line("vim ~/sc tail");
        editor.move_home();
        for _ in 0..8 {
            editor.move_right();
        }
        editor.replace_to_cursor(4, "~/scm/");
        assert_eq!((editor.text(), editor.cursor()), ("vim ~/scm/ tail", 10));
    }

    #[test]
    fn the_line_never_grows_past_its_limit() {
        let mut editor = line(&"x".repeat(LineEditor::MAX_BYTES));
        editor.insert("y");
        assert_eq!(editor.text().len(), LineEditor::MAX_BYTES);
    }
}
