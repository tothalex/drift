//! Code-view state: the centered cursor, free-scroll offset, and the two
//! selection kinds (visual line mode and mouse char selection).

/// (view line, character column) — a text position in the code view.
pub type TextPos = (usize, usize);

pub struct CodeView {
    /// Cursor line within the view's flattened lines; the viewport keeps
    /// it centered.
    pub cursor: usize,
    /// Free-scroll offset on top of the centered position (mouse wheel);
    /// reset by any cursor motion.
    pub view_offset: isize,
    /// Scroll position from the last render — the basis for translating
    /// mouse coordinates to text positions. Written by the renderer.
    pub scroll: usize,
    /// Visual mode: anchor line of the selection (`v` toggles).
    pub select_anchor: Option<usize>,
    /// Mouse drag-selection, char-precise (press position, drag position).
    pub mouse_sel: Option<(TextPos, TextPos)>,
    /// A new view was loaded: center the cursor at the next render, where
    /// the viewport geometry is known.
    pending_center: bool,
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
            scroll: 0,
            select_anchor: None,
            mouse_sel: None,
            pending_center: true,
        }
    }

    /// A different view is about to show: selections die, the cursor
    /// re-centers.
    pub fn reset_for_new_view(&mut self) {
        self.pending_center = true;
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

    /// Scroll the viewport without moving the cursor, clamped to the
    /// content; the cursor may leave the visible window.
    pub fn scroll_view(&mut self, delta: isize, viewport: usize, len: usize) {
        let max_scroll = len.saturating_sub(viewport) as isize;
        let base = (self.cursor.saturating_sub(viewport / 2) as isize).min(max_scroll);
        self.view_offset = (self.view_offset + delta).clamp(-base, max_scroll - base);
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
