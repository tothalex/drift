//! The peek: while open it replaces the code pane with the new-side
//! text of the block under the cursor, shown whole — no deletions —
//! with the scope keys walking the chain of enclosing blocks. Modal
//! like the picker; motions go through the keymap so rebinds carry
//! over.

use crate::processor::blocks::Block;
use crate::processor::peek::{PeekLine, PeekView};

pub struct Peek {
    view: PeekView,
    /// Shown chain level (0 = innermost).
    level: usize,
    /// Cursor line (1-based, new side), highlighted and kept centered
    /// by the renderer. Opens on the line the peek was pressed on;
    /// narrowing clamps it back into the smaller block.
    pub cursor: u32,
}

impl Peek {
    pub fn new(view: PeekView, origin: u32) -> Peek {
        let mut peek = Peek {
            view,
            level: 0,
            cursor: origin,
        };
        peek.clamp_cursor();
        peek
    }

    pub fn block(&self) -> &Block {
        &self.view.chain[self.level]
    }

    /// Level indicator for the header: (shown, of), 1-based outward.
    pub fn level_of(&self) -> (usize, usize) {
        (self.level + 1, self.view.chain.len())
    }

    /// The shown block's lines (the file may end before the block's
    /// reported range, so the slice clamps).
    pub fn block_lines(&self) -> &[PeekLine] {
        let (start, end) = self.block().range;
        let from = (start.saturating_sub(self.view.first) as usize).min(self.view.lines.len());
        let to = ((end + 1).saturating_sub(self.view.first) as usize).min(self.view.lines.len());
        &self.view.lines[from..to]
    }

    /// Line number of the shown block's first line.
    pub fn start(&self) -> u32 {
        self.block().range.0
    }

    /// Last line the shown block actually has.
    fn last_line(&self) -> u32 {
        self.start() + self.block_lines().len().saturating_sub(1) as u32
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.clamp(self.start(), self.last_line());
    }

    pub fn widen(&mut self) {
        if self.level + 1 < self.view.chain.len() {
            self.level += 1;
        }
    }

    pub fn narrow(&mut self) {
        if self.level > 0 {
            self.level -= 1;
            self.clamp_cursor();
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.cursor = self.cursor.saturating_add_signed(delta as i32);
        self.clamp_cursor();
    }

    pub fn to_top(&mut self) {
        self.cursor = self.start();
    }

    /// Jump to an absolute line (vim-style `10G`), clamped into the
    /// shown block.
    pub fn to_line(&mut self, line: u32) {
        self.cursor = line;
        self.clamp_cursor();
    }

    pub fn to_bottom(&mut self) {
        self.cursor = self.last_line();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> PeekView {
        let lines = (1..=10)
            .map(|n| PeekLine {
                content: format!("line {n}"),
                spans: Vec::new(),
                changed: n == 5,
            })
            .collect();
        PeekView {
            chain: vec![
                Block {
                    range: (4, 6),
                    title: "if a > 0".into(),
                },
                Block {
                    range: (1, 10),
                    title: "fn alpha()".into(),
                },
            ],
            first: 1,
            lines,
        }
    }

    #[test]
    fn opens_on_the_pressed_line_and_walks_the_chain() {
        let mut peek = Peek::new(view(), 5);
        assert_eq!(peek.cursor, 5);
        assert_eq!(peek.level_of(), (1, 2));
        assert_eq!(peek.block_lines().len(), 3);
        assert_eq!(peek.block_lines()[0].content, "line 4");

        peek.widen();
        assert_eq!(peek.level_of(), (2, 2));
        assert_eq!(peek.block_lines().len(), 10);
        assert_eq!(peek.cursor, 5);

        peek.widen(); // clamped at the outermost level
        assert_eq!(peek.level_of(), (2, 2));

        // Narrowing pulls a wandered cursor back into the block.
        peek.move_by(4);
        assert_eq!(peek.cursor, 9);
        peek.narrow();
        assert_eq!(peek.level_of(), (1, 2));
        assert_eq!(peek.cursor, 6);
    }

    #[test]
    fn cursor_clamps_to_the_shown_block() {
        let mut peek = Peek::new(view(), 5);
        peek.move_by(-10);
        assert_eq!(peek.cursor, 4);
        peek.move_by(100);
        assert_eq!(peek.cursor, 6);
        peek.to_top();
        assert_eq!(peek.cursor, 4);
        peek.to_bottom();
        assert_eq!(peek.cursor, 6);
    }

    #[test]
    fn block_range_past_the_file_end_clamps() {
        let mut v = view();
        v.chain[1].range = (1, 99);
        let mut peek = Peek::new(v, 5);
        assert_eq!(peek.block_lines().len(), 3); // innermost unaffected
        peek.widen();
        assert_eq!(peek.block_lines().len(), 10);
        peek.to_bottom();
        assert_eq!(peek.cursor, 10); // the file's last line, not 99
    }
}
