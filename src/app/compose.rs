//! The integrated comment composer: a small multi-line text box drawn
//! over the app, so writing a review comment never leaves the TUI. The
//! buffer is plain text with a char-offset cursor; display wrapping is
//! computed here so the renderer and cursor can never disagree.

use crate::forge::model::ComposeTarget;
use crate::processor::view::char_to_byte;

pub struct Compose {
    pub target: ComposeTarget,
    /// Panel title, e.g. "reply to mia".
    pub title: String,
    text: String,
    /// Cursor as a char offset into `text` (0..=chars).
    cursor: usize,
}

impl Compose {
    pub fn new(target: ComposeTarget, title: String) -> Compose {
        Compose {
            target,
            title,
            text: String::new(),
            cursor: 0,
        }
    }

    /// The trimmed body and its target; empty means cancelled.
    pub fn into_body(self) -> (ComposeTarget, String) {
        (self.target, self.text.trim().to_string())
    }

    pub fn insert(&mut self, ch: char) {
        let at = char_to_byte(&self.text, self.cursor);
        self.text.insert(at, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = char_to_byte(&self.text, self.cursor - 1);
        self.text.remove(at);
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }
        let at = char_to_byte(&self.text, self.cursor);
        self.text.remove(at);
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    /// Move a logical line up/down, keeping the column where possible.
    pub fn vertical(&mut self, delta: isize) {
        let lines = self.line_starts();
        let (row, col) = self.position(&lines);
        let target = row.saturating_add_signed(delta).min(lines.len() - 1);
        if target == row {
            return;
        }
        let len = self.line_len(&lines, target);
        self.cursor = lines[target] + col.min(len);
    }

    /// Jump to the start / end of the current logical line.
    pub fn home(&mut self) {
        let lines = self.line_starts();
        let (row, _) = self.position(&lines);
        self.cursor = lines[row];
    }

    pub fn end(&mut self) {
        let lines = self.line_starts();
        let (row, _) = self.position(&lines);
        self.cursor = lines[row] + self.line_len(&lines, row);
    }

    /// Char offsets where each logical line starts (always non-empty).
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        for (nth, ch) in self.text.chars().enumerate() {
            if ch == '\n' {
                starts.push(nth + 1);
            }
        }
        starts
    }

    /// Chars in line `row`, excluding its trailing newline.
    fn line_len(&self, starts: &[usize], row: usize) -> usize {
        let end = starts
            .get(row + 1)
            .map_or(self.text.chars().count(), |next| next - 1);
        end - starts[row]
    }

    /// (logical line, column) of the cursor.
    fn position(&self, starts: &[usize]) -> (usize, usize) {
        let row = starts
            .iter()
            .rposition(|&start| start <= self.cursor)
            .unwrap_or(0);
        (row, self.cursor - starts[row])
    }

    /// Display rows char-wrapped to `width`, plus the cursor's (row, col)
    /// within them. `col` may equal a row's char count (cursor after the
    /// last char) — the renderer pads a cell for it.
    pub fn rows(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut cursor_at = (0, 0);
        let mut offset = 0; // char offset of the current line start
        for line in split_lines(&self.text) {
            let chars: Vec<char> = line.chars().collect();
            let chunks = chars.len().div_ceil(width).max(1);
            let on_line = self.cursor >= offset && self.cursor <= offset + chars.len();
            if on_line {
                let col = self.cursor - offset;
                let mut row = col / width;
                let mut col = col % width;
                // Cursor at the exact end of a full row: give it its own
                // visual row rather than an out-of-range column.
                if row >= chunks {
                    row = chunks - 1;
                    col = width;
                }
                cursor_at = (rows.len() + row, col);
            }
            for chunk in 0..chunks {
                rows.push(
                    chars[chunk * width..((chunk + 1) * width).min(chars.len())]
                        .iter()
                        .collect(),
                );
            }
            offset += chars.len() + 1; // + the newline
        }
        (rows, cursor_at)
    }
}

/// `str::split('\n')` — named so the intent (logical lines, where an
/// empty text still has one line) is visible at the call site.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose(text: &str, cursor: usize) -> Compose {
        Compose {
            target: ComposeTarget::General,
            title: String::new(),
            text: text.to_string(),
            cursor,
        }
    }

    #[test]
    fn insert_backspace_delete_edit_at_the_cursor() {
        let mut c = compose("", 0);
        for ch in "héllo".chars() {
            c.insert(ch);
        }
        assert_eq!(c.text, "héllo");
        c.left();
        c.left();
        c.insert('!');
        assert_eq!(c.text, "hél!lo");
        c.backspace();
        assert_eq!(c.text, "héllo");
        c.delete();
        assert_eq!(c.text, "hélo");
        c.delete();
        c.delete(); // past the end: no-op
        assert_eq!(c.text, "hél");
    }

    #[test]
    fn vertical_and_home_end_move_by_logical_line() {
        let mut c = compose("short\na longer line\nx", 3);
        c.vertical(1);
        assert_eq!(c.cursor, 6 + 3); // same column on line 2
        c.end();
        assert_eq!(c.cursor, 6 + 13);
        c.vertical(1); // column clamps to "x"
        assert_eq!(c.cursor, 20 + 1);
        c.vertical(-2);
        c.home();
        assert_eq!(c.cursor, 0);
        c.vertical(-1); // already at the top: no-op
        assert_eq!(c.cursor, 0);
    }

    #[test]
    fn rows_wrap_and_track_the_cursor() {
        // "abcdefgh" wrapped at 4 → two rows.
        let c = compose("abcdefgh", 6);
        let (rows, cursor) = c.rows(4);
        assert_eq!(rows, vec!["abcd", "efgh"]);
        assert_eq!(cursor, (1, 2));
        // Cursor at the very end of a full final row keeps a valid cell.
        let c = compose("abcd", 4);
        let (rows, cursor) = c.rows(4);
        assert_eq!(rows, vec!["abcd"]);
        assert_eq!(cursor, (0, 4));
        // Newlines make rows even when empty; cursor on the empty line.
        let c = compose("ab\n\ncd", 3);
        let (rows, cursor) = c.rows(4);
        assert_eq!(rows, vec!["ab", "", "cd"]);
        assert_eq!(cursor, (1, 0));
    }

    #[test]
    fn into_body_trims() {
        let c = compose("  hello\nworld \n", 0);
        let (_, body) = c.into_body();
        assert_eq!(body, "hello\nworld");
        let (_, empty) = compose("  \n ", 0).into_body();
        assert!(empty.is_empty());
    }
}
