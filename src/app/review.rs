//! Review progress: which files are checked off, keyed by path so the
//! state survives refresh, plus the check history for undo. A local
//! check also remembers the version of the file it was made against;
//! when that version moves on — an agent edits a file already reviewed
//! — the check drops and the file shows as changed until checked again.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::vcs::Vcs;
use crate::vcs::model::{ChangedFile, Comparison, RevisionId, Scope};

/// Which version of a file a scope shows on its new side. Versions are
/// only compared within one side: switching scopes changes what is on
/// screen, not what was reviewed, so it never unchecks anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Worktree,
    /// The work side's tip: HEAD, or the reviewed branch's tip.
    Tip,
    Commit(RevisionId),
}

impl Side {
    pub fn of(cmp: &Comparison) -> Side {
        match &cmp.scope {
            Scope::Commit(rev) => Side::Commit(rev.clone()),
            Scope::Committed => Side::Tip,
            _ if cmp.work.is_some() => Side::Tip,
            _ => Side::Worktree,
        }
    }
}

/// The version a check was made against: a content id on one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub side: Side,
    pub id: String,
}

impl Stamp {
    /// The file's version as `cmp` shows it now.
    pub fn read(vcs: &dyn Vcs, cmp: &Comparison, file: &ChangedFile) -> Option<Stamp> {
        let id = vcs.new_side_id(cmp, file)?;
        Some(Stamp {
            side: Side::of(cmp),
            id,
        })
    }
}

/// Of a snapshot of stamped checks, the ones whose file now reads
/// differently on the side it was checked on. Files gone from the
/// changeset aren't judged: their checks wait until they come back.
pub fn stale(
    vcs: &dyn Vcs,
    cmp: &Comparison,
    files: &[ChangedFile],
    snapshot: Vec<(PathBuf, Stamp)>,
) -> Vec<(PathBuf, Stamp)> {
    let side = Side::of(cmp);
    let by_path: HashMap<&Path, &ChangedFile> =
        files.iter().map(|f| (f.path.as_path(), f)).collect();
    snapshot
        .into_iter()
        .filter(|(path, stamp)| {
            stamp.side == side
                && by_path
                    .get(path.as_path())
                    .is_some_and(|file| Stamp::read(vcs, cmp, file).as_ref() != Some(stamp))
        })
        .collect()
}

#[derive(Default)]
pub struct Review {
    /// Checked files, each with the version it was checked at — `None`
    /// for a check that never goes stale (pull requests, where the
    /// forge's own viewed state does that job).
    checked: HashMap<PathBuf, Option<Stamp>>,
    /// Check history, newest last — `X` pops it to uncheck.
    order: Vec<PathBuf>,
    /// Files whose check dropped because they changed after it.
    changed: HashSet<PathBuf>,
}

impl Review {
    pub fn contains(&self, path: &Path) -> bool {
        self.checked.contains_key(path)
    }

    /// The file was checked, then changed underneath the check.
    pub fn is_changed(&self, path: &Path) -> bool {
        self.changed.contains(path)
    }

    /// Toggle a file, checking it at `stamp`; returns true when it is
    /// now checked.
    pub fn toggle(&mut self, path: &Path, stamp: Option<Stamp>) -> bool {
        if self.checked.remove(path).is_some() {
            self.order.retain(|p| p != path);
            false
        } else {
            self.checked.insert(path.to_path_buf(), stamp);
            self.order.push(path.to_path_buf());
            self.changed.remove(path);
            true
        }
    }

    /// Uncheck and return the most recent check, skipping stale history.
    pub fn pop_last(&mut self) -> Option<PathBuf> {
        while let Some(path) = self.order.pop() {
            if self.checked.remove(&path).is_some() {
                return Some(path);
            }
        }
        None
    }

    /// Set one file's check state outright, for reverting a check the
    /// forge refused. Unlike [`Review::toggle`] this is idempotent.
    pub fn set(&mut self, path: &Path, checked: bool) {
        if checked {
            if !self.checked.contains_key(path) {
                self.checked.insert(path.to_path_buf(), None);
                self.order.push(path.to_path_buf());
            }
            self.changed.remove(path);
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
    pub fn count_in<'a>(&self, files: impl IntoIterator<Item = &'a ChangedFile>) -> usize {
        files
            .into_iter()
            .filter(|f| self.checked.contains_key(&f.path))
            .count()
    }

    /// The checks that carry a version, for [`stale`] to judge — off
    /// the main thread, so the result is applied with [`Self::drop_stale`].
    pub fn snapshot(&self) -> Vec<(PathBuf, Stamp)> {
        self.checked
            .iter()
            .filter_map(|(path, stamp)| Some((path.clone(), stamp.clone()?)))
            .collect()
    }

    /// Uncheck what [`stale`] found and mark it changed — each only if
    /// it is still checked at the snapshot's version, so a check made
    /// while the verdict was computed survives it. Returns the paths
    /// that dropped.
    pub fn drop_stale(&mut self, stale: Vec<(PathBuf, Stamp)>) -> Vec<PathBuf> {
        let mut dropped = Vec::new();
        for (path, stamp) in stale {
            if self.checked.get(&path) == Some(&Some(stamp)) {
                self.checked.remove(&path);
                self.order.retain(|p| *p != path);
                self.changed.insert(path.clone());
                dropped.push(path);
            }
        }
        dropped
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::VcsError;
    use crate::vcs::model::{CommitInfo, FileDiff, FileStatus};

    fn file(path: &str) -> ChangedFile {
        ChangedFile {
            status: FileStatus::Modified,
            path: PathBuf::from(path),
            old_path: None,
        }
    }

    /// A provider whose files read as whatever content id the test
    /// says, on every side alike.
    #[derive(Default)]
    struct Versions(HashMap<PathBuf, String>);

    impl Versions {
        fn edit(&mut self, path: &str, id: &str) {
            self.0.insert(PathBuf::from(path), id.to_string());
        }
    }

    impl Vcs for Versions {
        fn root(&self) -> &Path {
            Path::new("/repo")
        }
        fn comparison(&self, _base: Option<&str>) -> Result<Comparison, VcsError> {
            Ok(worktree_cmp())
        }
        fn changed_files(&self, _cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError> {
            Ok(Vec::new())
        }
        fn file_diff(&self, _cmp: &Comparison, _file: &ChangedFile) -> Result<FileDiff, VcsError> {
            unimplemented!()
        }
        fn new_side_id(&self, _cmp: &Comparison, file: &ChangedFile) -> Option<String> {
            self.0.get(&file.path).cloned()
        }
        fn file_at_ancestor(&self, _cmp: &Comparison, _file: &ChangedFile) -> Option<String> {
            None
        }
        fn commits(&self, _cmp: &Comparison) -> Result<Vec<CommitInfo>, VcsError> {
            Ok(Vec::new())
        }
        fn unignored(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
            paths
        }
    }

    fn worktree_cmp() -> Comparison {
        Comparison {
            base_label: "main".to_string(),
            ancestor: RevisionId("ancestor".to_string()),
            work_label: "feature".to_string(),
            scope: Scope::All,
            work: None,
        }
    }

    /// Check `path` the way the app does: at the version on screen.
    fn check(review: &mut Review, vcs: &Versions, cmp: &Comparison, path: &str) {
        let stamp = Stamp::read(vcs, cmp, &file(path));
        assert!(review.toggle(Path::new(path), stamp));
    }

    #[test]
    fn a_check_drops_when_its_file_changes_and_shows_changed_until_rechecked() {
        let (mut vcs, cmp) = (Versions::default(), worktree_cmp());
        vcs.edit("a.rs", "v1");
        vcs.edit("b.rs", "v1");
        let files = [file("a.rs"), file("b.rs")];
        let mut review = Review::default();
        check(&mut review, &vcs, &cmp, "a.rs");
        check(&mut review, &vcs, &cmp, "b.rs");

        vcs.edit("a.rs", "v2"); // an agent edits a reviewed file
        let verdict = stale(&vcs, &cmp, &files, review.snapshot());
        assert_eq!(review.drop_stale(verdict), vec![PathBuf::from("a.rs")]);

        assert!(!review.contains(Path::new("a.rs")));
        assert!(review.is_changed(Path::new("a.rs")));
        assert!(review.contains(Path::new("b.rs")));
        assert_eq!(review.pop_last(), Some(PathBuf::from("b.rs")));

        check(&mut review, &vcs, &cmp, "a.rs");
        assert!(!review.is_changed(Path::new("a.rs")));
        assert!(stale(&vcs, &cmp, &files, review.snapshot()).is_empty());
    }

    #[test]
    fn switching_scopes_never_unchecks() {
        let (mut vcs, cmp) = (Versions::default(), worktree_cmp());
        vcs.edit("a.rs", "worktree");
        let mut review = Review::default();
        check(&mut review, &vcs, &cmp, "a.rs");

        // The committed scope reads HEAD's blob, which differs from the
        // working copy that was reviewed — a different side, not a change.
        vcs.edit("a.rs", "head");
        let committed = Comparison {
            scope: Scope::Committed,
            ..worktree_cmp()
        };
        assert!(stale(&vcs, &committed, &[file("a.rs")], review.snapshot()).is_empty());
        assert_eq!(
            stale(&vcs, &cmp, &[file("a.rs")], review.snapshot()).len(),
            1
        );
    }

    #[test]
    fn a_check_made_while_the_scan_ran_survives_its_verdict() {
        let (mut vcs, cmp) = (Versions::default(), worktree_cmp());
        vcs.edit("a.rs", "v1");
        let mut review = Review::default();
        check(&mut review, &vcs, &cmp, "a.rs");
        let snapshot = review.snapshot();

        vcs.edit("a.rs", "v2");
        let verdict = stale(&vcs, &cmp, &[file("a.rs")], snapshot);
        review.toggle(Path::new("a.rs"), None); // re-checked at v2 meanwhile
        check(&mut review, &vcs, &cmp, "a.rs");

        assert!(review.drop_stale(verdict).is_empty());
        assert!(review.contains(Path::new("a.rs")));
    }

    #[test]
    fn checks_outside_the_changeset_wait_unjudged() {
        let (mut vcs, cmp) = (Versions::default(), worktree_cmp());
        vcs.edit("gone.rs", "v1");
        let mut review = Review::default();
        check(&mut review, &vcs, &cmp, "gone.rs");

        vcs.edit("gone.rs", "v2");
        assert!(stale(&vcs, &cmp, &[], review.snapshot()).is_empty());
    }

    #[test]
    fn unstamped_checks_never_go_stale() {
        let mut review = Review::default();
        review.toggle(Path::new("pr.rs"), None);
        review.set(Path::new("viewed.rs"), true);

        assert!(review.snapshot().is_empty());
    }

    #[test]
    fn adopt_replaces_checks_for_the_listed_files_only() {
        let mut review = Review::default();
        review.toggle(Path::new("pr/stale.rs"), None); // ticked here, not on the forge
        review.toggle(Path::new("local/mine.rs"), None); // not in the pull request

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
        review.toggle(Path::new("a.rs"), None);

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
