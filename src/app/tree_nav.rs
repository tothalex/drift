//! Tree navigation state: the file tree plus its cursor and free-scroll
//! offset.

use crate::tree::FileTree;
use crate::vcs::model::ChangedFile;

pub struct TreeNav {
    pub tree: FileTree,
    /// Cursor over the tree's visible rows.
    pub cursor: usize,
    /// Top visible row (free-scrollable like the code view).
    offset: usize,
}

impl TreeNav {
    pub fn new(files: &[ChangedFile]) -> TreeNav {
        let tree = FileTree::build(files);
        let cursor = tree.first_file_row().unwrap_or(0);
        TreeNav {
            tree,
            cursor,
            offset: 0,
        }
    }

    pub fn rebuild(&mut self, files: &[ChangedFile]) {
        *self = TreeNav::new(files);
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Changed-files index of the file under the cursor, if any.
    pub fn selected_file(&self) -> Option<usize> {
        self.tree.file_at(self.cursor)
    }

    /// Visible row currently showing the file with this index.
    pub fn row_of_file(&self, file_index: usize) -> Option<usize> {
        (0..self.tree.visible_len()).find(|&row| self.tree.file_at(row) == Some(file_index))
    }

    /// Move the cursor, skipping rows whose file satisfies `skip`
    /// (reviewed files). Stops at the edges.
    pub fn move_cursor(&mut self, delta: isize, viewport: usize, skip: impl Fn(usize) -> bool) {
        if self.tree.visible_len() == 0 {
            return;
        }
        let step = delta.signum();
        if step == 0 {
            return;
        }
        let last = (self.tree.visible_len() - 1) as isize;
        let mut cursor = self.cursor as isize;
        for _ in 0..delta.abs() {
            let mut probe = cursor;
            loop {
                probe += step;
                if probe < 0 || probe > last {
                    break;
                }
                let skipped = self.tree.file_at(probe as usize).is_some_and(&skip);
                if !skipped {
                    cursor = probe;
                    break;
                }
            }
        }
        self.cursor = cursor as usize;
        self.keep_cursor_visible(viewport);
    }

    pub fn set_cursor(&mut self, row: usize, viewport: usize) {
        self.cursor = row.min(self.tree.visible_len().saturating_sub(1));
        self.keep_cursor_visible(viewport);
    }

    pub fn toggle_dir(&mut self) {
        if self.tree.toggle(self.cursor) {
            let last = self.tree.visible_len().saturating_sub(1);
            self.cursor = self.cursor.min(last);
            self.offset = self.offset.min(last); // re-clamp: rows may have vanished
        }
    }

    /// Scroll the tree without moving the selection.
    pub fn scroll(&mut self, delta: isize, viewport: usize) {
        let max = self.tree.visible_len().saturating_sub(viewport) as isize;
        self.offset = (self.offset as isize + delta).clamp(0, max.max(0)) as usize;
    }

    /// Nudge the scroll so the selection stays visible after moving.
    fn keep_cursor_visible(&mut self, viewport: usize) {
        let viewport = viewport.max(1);
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if self.cursor >= self.offset + viewport {
            self.offset = self.cursor + 1 - viewport;
        }
    }
}
