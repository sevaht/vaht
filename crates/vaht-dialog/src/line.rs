//! A line of text being edited: its text, and where the cursor is.

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

    /// Takes the line as the user's field now has it: `text`, the cursor at
    /// the byte offset `cursor` (moved back to a character boundary if it is
    /// not on one, and to the end if it is past it). A line longer than the
    /// limit is cut.
    pub(crate) fn edit(&mut self, text: &str, cursor: usize) {
        let mut end = text.len().min(Self::MAX_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].clone_into(&mut self.text);
        let mut cursor = cursor.min(end);
        while !self.text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        self.cursor = cursor;
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

    /// Replaces the text from `start` to the cursor with `text`, the cursor
    /// after it, unless the line would grow too long.
    pub(crate) fn replace_to_cursor(&mut self, start: usize, text: &str) {
        let length = self.text.len() - (self.cursor - start) + text.len();
        if length <= Self::MAX_BYTES {
            self.text.replace_range(start..self.cursor, text);
            self.cursor = start + text.len();
        }
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
    fn the_users_edit_is_taken_with_the_cursor_on_a_character() {
        let mut editor = LineEditor::default();
        editor.edit("né", 2);
        assert_eq!(
            (editor.text(), editor.cursor()),
            ("né", 1),
            "inside é: back"
        );
        editor.edit("né", 99);
        assert_eq!(editor.cursor(), 3, "past the end: at the end");
    }

    #[test]
    fn the_word_before_the_cursor_starts_after_a_space() {
        let editor = line("ls ~/sc");
        assert_eq!(editor.word_start(), 3);
        assert_eq!(line("kit").word_start(), 0);
    }

    #[test]
    fn a_word_is_replaced_up_to_the_cursor() {
        let mut editor = LineEditor::default();
        editor.edit("vim ~/sc tail", 8);
        editor.replace_to_cursor(4, "~/scm/");
        assert_eq!((editor.text(), editor.cursor()), ("vim ~/scm/ tail", 10));
    }

    #[test]
    fn the_line_never_grows_past_its_limit() {
        let mut editor = LineEditor::default();
        editor.edit(&"x".repeat(LineEditor::MAX_BYTES + 5), 0);
        assert_eq!(editor.text().len(), LineEditor::MAX_BYTES);
        let mut multi = LineEditor::default();
        multi.edit(&"é".repeat(LineEditor::MAX_BYTES), 0);
        assert!(
            multi.text().len() <= LineEditor::MAX_BYTES,
            "cut on a character"
        );
    }
}
