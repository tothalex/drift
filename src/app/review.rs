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

    /// How many of the given files are checked (stale paths don't count).
    pub fn count_in(&self, files: &[ChangedFile]) -> usize {
        files
            .iter()
            .filter(|f| self.checked.contains(&f.path))
            .count()
    }
}
