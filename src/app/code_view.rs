//! Code-view state: the centered cursor, free-scroll offset, and the two
//! selection kinds (visual line mode and mouse char selection).

/// (view line, character column) — a text position in the code view.
pub type TextPos = (usize, usize);

pub struct CodeView {
    /// Cursor line within the view's flattened lines; the viewport keeps
    /// it centered.
    pub cursor: usize,
    /// Free-scroll offset on top of the centered position (mouse wheel);
    /// reset by any cursor motion, re-clamped by the renderer (only it
    /// knows wrapped line heights).
    pub view_offset: isize,
    /// What the last render put on each visual row: (view line, chars of
    /// that line consumed before the row) — long lines wrap, so screen
    /// rows and view lines don't map 1:1. The basis for translating
    /// mouse coordinates to text positions. Written by the renderer.
    pub row_map: Vec<(usize, usize)>,
    /// Visual mode: anchor line of the selection (`v` toggles).
    pub select_anchor: Option<usize>,
    /// Mouse drag-selection, char-precise (press position, drag position).
    pub mouse_sel: Option<(TextPos, TextPos)>,
    /// A new view was loaded: center the cursor at the next render, where
    /// the viewport geometry is known.
    pending_center: bool,
    /// A click moved the cursor: the next render must keep this view line
    /// as its top row instead of re-centering, so the text stays put
    /// under the pointer.
    pending_top: Option<usize>,
}

impl Default for CodeView {
    fn default() -> Self {
        CodeView::new()
    }
}

impl CodeView {
    pub fn new() -> CodeView {
        CodeView {
            cursor: 0,
            view_offset: 0,
            row_map: Vec::new(),
            select_anchor: None,
            mouse_sel: None,
            pending_center: true,
            pending_top: None,
        }
    }

    /// A different view is about to show: selections die, the cursor
    /// re-centers.
    pub fn reset_for_new_view(&mut self) {
        self.pending_center = true;
        self.pending_top = None;
        self.select_anchor = None;
        self.mouse_sel = None;
        self.view_offset = 0;
    }

    pub fn move_cursor(&mut self, delta: isize, len: usize) {
        let last = len.saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
        // Cursor motion snaps the view back to centering on it.
        self.view_offset = 0;
    }

    pub fn jump(&mut self, target: usize, len: usize) {
        self.cursor = target.min(len.saturating_sub(1));
        self.view_offset = 0;
    }

    /// Mouse click: place the cursor on a view line without scrolling —
    /// the top row stays where the last render put it.
    pub fn click_cursor(&mut self, line: usize, len: usize) {
        self.pending_top = self.row_map.first().map(|&(top, _)| top);
        self.cursor = line.min(len.saturating_sub(1));
        self.view_offset = 0;
    }

    /// The view line the next render must keep as its top row, if a
    /// click anchored one. Consumed by the renderer.
    pub fn take_pending_top(&mut self) -> Option<usize> {
        self.pending_top.take()
    }

    /// Scroll the viewport without moving the cursor; the cursor may
    /// leave the visible window. The renderer clamps to the content and
    /// normalizes the offset back — wrapped line heights live there.
    pub fn scroll_view(&mut self, delta: isize) {
        self.view_offset += delta;
    }

    /// Place the cursor on the middle line of the viewport — or of the
    /// content, when the view is shorter than half the viewport.
    pub fn apply_pending_center(&mut self, viewport: usize, len: usize) {
        if self.pending_center {
            self.pending_center = false;
            self.cursor = (viewport / 2).min(len / 2);
            self.view_offset = 0;
        }
    }

    /// Ordered visual-selection range, while `v` mode is active.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.select_anchor?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn toggle_visual(&mut self) {
        self.select_anchor = match self.select_anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
    }

    /// Normalized (start ≤ end) mouse selection, while a drag is active.
    pub fn mouse_selection(&self) -> Option<(TextPos, TextPos)> {
        let (a, b) = self.mouse_sel?;
        Some(if a <= b { (a, b) } else { (b, a) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_moves_cursor_and_pins_the_top_row() {
        let mut code = CodeView::new();
        code.row_map = vec![(10, 0), (11, 0), (11, 80), (12, 0)];
        code.view_offset = 5;
        code.click_cursor(12, 100);
        assert_eq!(code.cursor, 12);
        assert_eq!(code.view_offset, 0);
        assert_eq!(code.take_pending_top(), Some(10));
        assert_eq!(code.take_pending_top(), None);
    }

    #[test]
    fn click_clamps_to_the_last_view_line() {
        let mut code = CodeView::new();
        code.click_cursor(50, 10);
        assert_eq!(code.cursor, 9);
    }

    #[test]
    fn new_view_drops_the_click_anchor() {
        let mut code = CodeView::new();
        code.row_map = vec![(3, 0)];
        code.click_cursor(3, 10);
        code.reset_for_new_view();
        assert_eq!(code.take_pending_top(), None);
    }
}
