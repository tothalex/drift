//! Pane geometry: the rendered rectangles (for mouse hit-testing) and the
//! resizable tree/code split.

use ratatui::layout::{Position, Rect};

pub struct PaneLayout {
    /// Content rectangles from the last render.
    pub tree_area: Rect,
    pub code_area: Rect,
    /// Full pane row (tree + gap + code) and the divider column within it.
    pub main_area: Rect,
    pub divider_x: u16,
    /// Tree pane width as a percentage of the window.
    pub split_percent: u16,
    /// The divider is being dragged.
    pub resizing: bool,
}

impl Default for PaneLayout {
    fn default() -> Self {
        PaneLayout::new()
    }
}

impl PaneLayout {
    pub fn new() -> PaneLayout {
        PaneLayout {
            tree_area: Rect::default(),
            code_area: Rect::default(),
            main_area: Rect::default(),
            divider_x: 0,
            split_percent: 16,
            resizing: false,
        }
    }

    pub fn on_divider(&self, position: Position) -> bool {
        self.main_area.contains(position)
            && (i32::from(position.x) - i32::from(self.divider_x)).abs() <= 1
    }

    pub fn resize(&mut self, delta: isize) {
        self.split_percent = (self.split_percent as isize + delta).clamp(10, 85) as u16;
    }

    pub fn drag(&mut self, column: u16) {
        if self.main_area.width == 0 {
            return;
        }
        let rel = u32::from(column.saturating_sub(self.main_area.x));
        let percent = (rel * 100 / u32::from(self.main_area.width)) as isize;
        self.split_percent = percent.clamp(10, 85) as u16;
    }
}
