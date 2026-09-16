//! A single-line text editor.
//!
//! ratatui draws; it does not edit. This is the smallest editor that behaves
//! correctly for the job: a cursor that moves by grapheme rather than by byte,
//! so an emoji or a combining accent is one step and never splits, and a
//! viewport that scrolls horizontally when the value is wider than its box.
//!
//! Single-line by design, matching the desktop surface. A list is entered as
//! comma-separated values and a JSON value on one line.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A single-line text buffer with a grapheme cursor.
#[derive(Debug, Clone, Default)]
pub struct Editor {
    text: String,
    /// Cursor position, counted in graphemes from the start.
    cursor: usize,
}

impl Editor {
    /// Creates an editor holding one initial value, with the cursor at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.graphemes(true).count();
        Self { text, cursor }
    }

    /// Returns the current value.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the value, placing the cursor at the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        *self = Self::new(text);
    }

    /// Returns whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Returns the cursor position in graphemes.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Inserts one character at the cursor.
    pub fn insert(&mut self, character: char) {
        let at = self.byte_offset(self.cursor);
        self.text.insert(at, character);
        self.cursor += 1;
    }

    /// Deletes the grapheme before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_offset(self.cursor - 1);
        let end = self.byte_offset(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Deletes the grapheme at the cursor.
    pub fn delete(&mut self) {
        let count = self.text.graphemes(true).count();
        if self.cursor >= count {
            return;
        }
        let start = self.byte_offset(self.cursor);
        let end = self.byte_offset(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    /// Moves the cursor one grapheme left.
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Moves the cursor one grapheme right.
    pub fn right(&mut self) {
        let count = self.text.graphemes(true).count();
        if self.cursor < count {
            self.cursor += 1;
        }
    }

    /// Moves the cursor to the start.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end.
    pub fn end(&mut self) {
        self.cursor = self.text.graphemes(true).count();
    }

    /// Clears the buffer.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Returns the visible slice and the cursor's column within it.
    ///
    /// A value wider than its box scrolls so the cursor stays in view, which is
    /// the difference between an editor and a label.
    pub fn viewport(&self, width: usize) -> (&str, usize) {
        if width == 0 {
            return ("", 0);
        }
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        // Keep the cursor visible, preferring to show the text before it.
        let first = self.cursor.saturating_sub(width.saturating_sub(1));
        let start = self.byte_offset_of(&graphemes, first);
        let mut end = self.text.len();
        let mut used = 0;
        for (index, grapheme) in graphemes.iter().enumerate().skip(first) {
            let next = used + grapheme.width().max(1);
            if next > width {
                end = self.byte_offset_of(&graphemes, index);
                break;
            }
            used = next;
        }
        let column = graphemes[first..self.cursor]
            .iter()
            .map(|grapheme| grapheme.width().max(1))
            .sum();
        (&self.text[start..end], column)
    }

    /// Returns the byte offset of one grapheme index.
    fn byte_offset(&self, index: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(index)
            .map(|(offset, _)| offset)
            .unwrap_or(self.text.len())
    }

    /// Returns the byte offset of one grapheme index in a precomputed slice.
    fn byte_offset_of(&self, graphemes: &[&str], index: usize) -> usize {
        graphemes
            .iter()
            .take(index)
            .map(|grapheme| grapheme.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut editor = Editor::default();
        for character in "abc".chars() {
            editor.insert(character);
        }
        editor.left();
        editor.insert('X');

        assert_eq!(editor.text(), "abXc");
        assert_eq!(editor.cursor(), 3);
    }

    #[test]
    fn backspace_removes_the_grapheme_before_the_cursor() {
        let mut editor = Editor::new("abc");
        editor.backspace();

        assert_eq!(editor.text(), "ab");
        assert_eq!(editor.cursor(), 2);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut editor = Editor::new("abc");
        editor.home();
        editor.backspace();

        assert_eq!(editor.text(), "abc");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn delete_removes_the_grapheme_at_the_cursor() {
        let mut editor = Editor::new("abc");
        editor.home();
        editor.delete();

        assert_eq!(editor.text(), "bc");
        assert_eq!(editor.cursor(), 0);
    }

    /// A multi-byte grapheme moves as one, and never splits.
    ///
    /// Counting bytes here would panic on a char boundary or corrupt the value;
    /// counting `char`s would take two presses to cross one family emoji.
    #[test]
    fn the_cursor_moves_by_grapheme_not_by_byte() {
        let mut editor = Editor::new("aé👩‍👩‍👧b");
        editor.home();
        editor.right();
        editor.right();
        editor.right();
        editor.backspace();

        assert_eq!(editor.text(), "aéb");
    }

    #[test]
    fn a_short_value_is_shown_whole() {
        let editor = Editor::new("abc");
        let (visible, column) = editor.viewport(10);

        assert_eq!(visible, "abc");
        assert_eq!(column, 3);
    }

    /// A value wider than its box scrolls to keep the cursor visible.
    #[test]
    fn a_long_value_scrolls_to_the_cursor() {
        let editor = Editor::new("abcdefghij");
        let (visible, column) = editor.viewport(5);

        assert!(
            visible.ends_with('j'),
            "the end must stay visible, got {visible:?}"
        );
        assert!(column < 5, "the cursor must sit inside the box");
    }

    #[test]
    fn a_zero_width_box_shows_nothing() {
        let editor = Editor::new("abc");

        assert_eq!(editor.viewport(0), ("", 0));
    }
}
