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
        TreeNav::starting(FileTree::build(files), |_| false)
    }

    /// A fresh tree, its cursor on the first file `skip` passes over —
    /// or on the very first file when it would pass over them all.
    fn starting(tree: FileTree, skip: impl Fn(usize) -> bool) -> TreeNav {
        let cursor = (0..tree.visible_len())
            .find(|&row| tree.file_at(row).is_some_and(|index| !skip(index)))
            .or_else(|| tree.first_file_row())
            .unwrap_or(0);
        TreeNav {
            tree,
            cursor,
            offset: 0,
        }
    }

    /// Start over on a new tree, the cursor on its first file that
    /// `skip` (generated files) doesn't pass over.
    pub fn rebuild(&mut self, tree: FileTree, skip: impl Fn(usize) -> bool) {
        *self = TreeNav::starting(tree, skip);
    }

    /// Rebuild for a live refresh or a layout switch: collapsed
    /// directories and the cursor carry over by path, so the user's
    /// place survives the tree changing underneath them.
    pub fn rebuild_preserving(&mut self, mut tree: FileTree, viewport: usize) {
        let cursor_path = self.tree.row_path(self.cursor).map(str::to_string);
        let collapsed = self.tree.collapsed_paths();
        tree.collapse_paths(&collapsed);
        let last = tree.visible_len().saturating_sub(1);
        // The cursor's node may be gone (file reverted); stay near the
        // old row rather than jumping to the top.
        self.cursor = cursor_path
            .and_then(|path| tree.row_of_path(&path))
            .unwrap_or_else(|| self.cursor.min(last));
        self.tree = tree;
        self.offset = self.offset.min(last);
        self.keep_cursor_visible(viewport);
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

    /// Move the cursor `delta` rows, every row but a group gap a stop.
    /// Stops at the edges.
    pub fn move_cursor(&mut self, delta: isize, viewport: usize) {
        self.step(delta, viewport, |tree, row| !tree.is_gap(row));
    }

    /// Move the cursor `delta` files: directory and group rows are
    /// passed over, and so are files `skip` names (reviewed, generated).
    /// Stays put when no such file lies that way.
    pub fn move_file(&mut self, delta: isize, viewport: usize, skip: impl Fn(usize) -> bool) {
        self.step(delta, viewport, |tree, row| {
            tree.file_at(row).is_some_and(|index| !skip(index))
        });
    }

    fn step(&mut self, delta: isize, viewport: usize, stop: impl Fn(&FileTree, usize) -> bool) {
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
                if stop(&self.tree, probe as usize) {
                    cursor = probe;
                    break;
                }
            }
        }
        self.cursor = cursor as usize;
        self.keep_cursor_visible(viewport);
    }

    /// Put the cursor on `row` — or, on a group gap, on the header
    /// right below it.
    pub fn set_cursor(&mut self, row: usize, viewport: usize) {
        let last = self.tree.visible_len().saturating_sub(1);
        let row = row.min(last);
        self.cursor = match self.tree.is_gap(row) {
            true => (row + 1).min(last),
            false => row,
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::FileStatus;
    use std::path::PathBuf;

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            status: FileStatus::Modified,
            path: PathBuf::from(path),
            old_path: None,
        }
    }

    #[test]
    fn rebuild_preserving_keeps_cursor_by_path() {
        let mut nav = TreeNav::new(&[changed("src/b.rs"), changed("z.rs")]);
        // Rows: src, b.rs, z.rs — put the cursor on z.rs.
        nav.set_cursor(2, 10);
        // A new file shifts z.rs to a different row.
        nav.rebuild_preserving(
            FileTree::build(&[changed("src/a.rs"), changed("src/b.rs"), changed("z.rs")]),
            10,
        );
        assert_eq!(nav.tree.row_path(nav.cursor), Some("z.rs"));
    }

    #[test]
    fn rebuild_starts_past_skipped_files_unless_all_are() {
        let files = [changed("bun.lock"), changed("main.ts")];
        let mut nav = TreeNav::new(&[]);

        nav.rebuild(FileTree::build(&files), |index| index == 0);
        assert_eq!(nav.tree.row_path(nav.cursor), Some("main.ts"));

        nav.rebuild(FileTree::build(&files), |_| true);
        assert_eq!(nav.tree.row_path(nav.cursor), Some("bun.lock"));
    }

    #[test]
    fn move_file_lands_on_files_only() {
        // Rows: src, src/a.rs, src/b.rs, tests, tests/c.rs, z.rs
        let mut nav = TreeNav::new(&[
            changed("src/a.rs"),
            changed("src/b.rs"),
            changed("tests/c.rs"),
            changed("z.rs"),
        ]);
        let at = |nav: &TreeNav| nav.tree.row_path(nav.cursor).map(str::to_string);
        assert_eq!(at(&nav).as_deref(), Some("src/a.rs"));

        // Past the reviewed b.rs and the tests directory row.
        nav.move_file(1, 10, |index| index == 1);
        assert_eq!(at(&nav).as_deref(), Some("tests/c.rs"));
        nav.move_file(2, 10, |_| false);
        assert_eq!(at(&nav).as_deref(), Some("z.rs"));
        // Nothing further: stay.
        nav.move_file(1, 10, |_| false);
        assert_eq!(at(&nav).as_deref(), Some("z.rs"));
        // From a directory row, onward to its file.
        nav.set_cursor(3, 10);
        nav.move_file(-1, 10, |_| false);
        assert_eq!(at(&nav).as_deref(), Some("src/b.rs"));
        // j/k still stop on every row.
        nav.move_cursor(1, 10);
        assert_eq!(at(&nav).as_deref(), Some("tests"));
    }

    #[test]
    fn the_cursor_never_rests_on_a_group_gap() {
        use crate::group::{Group, Kind};
        let files = [changed("src/a.rs"), changed("README.md")];
        let groups = [
            Group {
                kind: Kind::Code,
                label: "code".into(),
                files: vec![0],
            },
            Group {
                kind: Kind::Docs,
                label: "docs".into(),
                files: vec![1],
            },
        ];
        // Rows: code, src, a.rs, (gap), docs, README.md
        let mut nav = TreeNav::new(&[]);
        nav.rebuild(FileTree::build_grouped(&files, &groups), |_| false);
        assert_eq!(nav.tree.row_path(nav.cursor), Some("src/a.rs"));

        nav.move_cursor(1, 10);
        assert_eq!(nav.tree.row_path(nav.cursor), Some("#group:docs"));
        nav.move_cursor(-1, 10);
        assert_eq!(nav.tree.row_path(nav.cursor), Some("src/a.rs"));
        nav.set_cursor(3, 10); // `4G`, or a click, on the gap
        assert_eq!(nav.tree.row_path(nav.cursor), Some("#group:docs"));
    }

    #[test]
    fn rebuild_preserving_survives_vanished_cursor_file() {
        let mut nav = TreeNav::new(&[changed("a.rs"), changed("b.rs")]);
        nav.set_cursor(1, 10);
        nav.rebuild_preserving(FileTree::build(&[changed("a.rs")]), 10);
        // b.rs is gone: the cursor stays nearby instead of resetting.
        assert_eq!(nav.cursor, 0);
        assert_eq!(nav.tree.row_path(0), Some("a.rs"));
    }
}
