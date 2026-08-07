//! Review progress: which files are checked off, keyed by path so the
//! state survives refresh, plus the check history for undo.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::vcs::model::ChangedFile;

#[derive(Default)]
pub struct Review {
    checked: HashSet<PathBuf>,
    /// Check history, newest last — `X` pops it to uncheck.
    order: Vec<PathBuf>,
}

impl Review {
    pub fn contains(&self, path: &Path) -> bool {
        self.checked.contains(path)
    }

    /// Toggle a file; returns true when it is now checked.
    pub fn toggle(&mut self, path: &Path) -> bool {
        if self.checked.remove(path) {
            self.order.retain(|p| p != path);
            false
        } else {
            self.checked.insert(path.to_path_buf());
            self.order.push(path.to_path_buf());
            true
        }
    }

    /// Uncheck and return the most recent check, skipping stale history.
    pub fn pop_last(&mut self) -> Option<PathBuf> {
        while let Some(path) = self.order.pop() {
            if self.checked.remove(&path) {
                return Some(path);
            }
        }
        None
    }

    /// Set one file's check state outright, for reverting a check the
    /// forge refused. Unlike [`Review::toggle`] this is idempotent.
    pub fn set(&mut self, path: &Path, checked: bool) {
        if checked {
            if self.checked.insert(path.to_path_buf()) {
                self.order.push(path.to_path_buf());
            }
        } else {
            self.checked.remove(path);
            self.order.retain(|p| p != path);
        }
    }

    /// Adopt the forge's viewed ticks for the pull request's files: what
    /// the server says wins on open, so a tick made in the web UI or on
    /// another machine shows up here and a stale local one doesn't
    /// linger. Only `files` are touched — checks on locally changed
    /// files the pull request doesn't include survive the visit.
    pub fn adopt(&mut self, files: &[ChangedFile], viewed: &[PathBuf]) {
        for file in files {
            self.set(&file.path, viewed.contains(&file.path));
        }
    }

    /// How many of the given files are checked (stale paths don't count).
    pub fn count_in(&self, files: &[ChangedFile]) -> usize {
        files
            .iter()
            .filter(|f| self.checked.contains(&f.path))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::FileStatus;

    fn file(path: &str) -> ChangedFile {
        ChangedFile {
            status: FileStatus::Modified,
            path: PathBuf::from(path),
            old_path: None,
        }
    }

    #[test]
    fn adopt_replaces_checks_for_the_listed_files_only() {
        let mut review = Review::default();
        review.toggle(Path::new("pr/stale.rs")); // ticked here, not on the forge
        review.toggle(Path::new("local/mine.rs")); // not in the pull request

        review.adopt(
            &[file("pr/stale.rs"), file("pr/fresh.rs")],
            &[PathBuf::from("pr/fresh.rs")],
        );

        assert!(!review.contains(Path::new("pr/stale.rs")));
        assert!(review.contains(Path::new("pr/fresh.rs")));
        assert!(review.contains(Path::new("local/mine.rs")));
    }

    #[test]
    fn adopt_drops_undo_history_for_files_it_unchecks() {
        let mut review = Review::default();
        review.toggle(Path::new("a.rs"));

        review.adopt(&[file("a.rs")], &[]);

        assert_eq!(review.pop_last(), None);
    }

    #[test]
    fn set_is_idempotent_where_toggle_flips() {
        let mut review = Review::default();

        review.set(Path::new("a.rs"), true);
        review.set(Path::new("a.rs"), true);

        assert!(review.contains(Path::new("a.rs")));
        assert_eq!(review.pop_last(), Some(PathBuf::from("a.rs")));
        assert_eq!(review.pop_last(), None);
    }
}
