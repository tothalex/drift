//! Computed views by file path, plus the display options that shape them
//! (expansion, comment folding, per-file block scope). Path keys — not
//! indices — so views survive the file list shifting under a live refresh.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::processor;
use crate::processor::ViewOptions;
use crate::processor::view::FileView;
use crate::vcs::Vcs;
use crate::vcs::model::{ChangedFile, Comparison, FileDiff, LineKind, Scope};

pub struct ViewCache {
    views: HashMap<PathBuf, FileView>,
    /// Block-scope level — global, like the other view toggles; each
    /// file clamps it to its own chain depth at compute time.
    pub scope: usize,
    /// Show all unchanged lines inside blocks (`z` toggles collapsing).
    pub expand_unchanged: bool,
    /// Fold unchanged comment blocks to one-line summaries (`C` toggles).
    pub comment_fold: bool,
}

impl Default for ViewCache {
    fn default() -> Self {
        ViewCache::new()
    }
}

impl ViewCache {
    pub fn new() -> ViewCache {
        ViewCache {
            views: HashMap::new(),
            scope: 0,
            expand_unchanged: true,
            comment_fold: false,
        }
    }

    pub fn get(&self, path: &Path) -> Option<&FileView> {
        self.views.get(path)
    }

    /// Drop one file's view (its content changed).
    pub fn remove(&mut self, path: &Path) {
        self.views.remove(path);
    }

    /// Drop views for files that left the change list.
    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool) {
        self.views.retain(|path, _| keep(path));
    }

    /// Drop every cached view (options changed).
    pub fn clear_views(&mut self) {
        self.views.clear();
    }

    /// Full reset for a refresh: nothing is valid. The global toggles
    /// (scope, expansion, folding) survive.
    pub fn reset(&mut self) {
        self.views.clear();
    }

    /// Compute and cache the view for a file if it isn't cached yet.
    pub fn ensure(&mut self, file: &ChangedFile, vcs: &dyn Vcs, cmp: &Comparison) -> Result<()> {
        if self.views.contains_key(&file.path) {
            return Ok(());
        }
        let view = compute(file, vcs, cmp, self.options())?;
        self.views.insert(file.path.clone(), view);
        Ok(())
    }

    /// Insert a background-computed view unless one arrived meanwhile.
    pub fn insert_if_absent(&mut self, path: PathBuf, view: FileView) {
        self.views.entry(path).or_insert(view);
    }

    pub fn options(&self) -> ViewOptions {
        ViewOptions {
            expand_unchanged: self.expand_unchanged,
            scope: self.scope,
            fold_comments: self.comment_fold,
        }
    }
}

/// The new-side content of a file under `cmp`: the source the diff's
/// added lines belong to, which every view and the peek must agree on.
/// `None` when there is none to read — a deleted file, or an object the
/// repo doesn't have.
pub fn new_side_source(file: &ChangedFile, vcs: &dyn Vcs, cmp: &Comparison) -> Option<String> {
    match (&cmp.scope, &cmp.work) {
        // The picked commit's own tree: the working copy may have moved
        // on, and under a branch review it is someone else's entirely.
        (Scope::Commit(rev), _) => vcs.file_at_revision(rev, &file.path),
        // A branch reviewed off the board ends at its tip; HEAD belongs
        // to whatever happens to be checked out beside it.
        (_, Some(tip)) => vcs.file_at_revision(tip, &file.path),
        (Scope::Committed, None) => vcs.file_at_head(&file.path),
        _ => std::fs::read_to_string(vcs.root().join(&file.path)).ok(),
    }
}

/// Compute one file's view — shared by the cache and the prefetch worker.
pub fn compute(
    file: &ChangedFile,
    vcs: &dyn Vcs,
    cmp: &Comparison,
    options: ViewOptions,
) -> Result<FileView> {
    let mut diff = vcs.file_diff(cmp, file)?;
    // Tabs render zero-width in the terminal; expand them everywhere the
    // processor looks so spans stay aligned with the displayed text.
    processor::tabs::expand_diff(&mut diff);
    // None (deleted/unreadable) → hunk fallback.
    let source = new_side_source(file, vcs, cmp).map(processor::tabs::expand_tabs_owned);
    // Ancestor-side content is only needed to highlight removed lines;
    // skip the lookup when the diff has none.
    let old_source = if has_removed_lines(&diff) {
        vcs.file_at_ancestor(cmp, file)
            .map(processor::tabs::expand_tabs_owned)
    } else {
        None
    };
    Ok(processor::process(
        &file.path,
        &diff,
        source.as_deref(),
        old_source.as_deref(),
        options,
    ))
}

pub(crate) fn has_removed_lines(diff: &FileDiff) -> bool {
    match diff {
        FileDiff::Binary => false,
        FileDiff::Text { hunks } => hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.kind == LineKind::Removed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processor::view::{FileView, ViewLine};
    use crate::vcs::VcsError;
    use crate::vcs::model::{DiffLine, FileStatus, Hunk, RevisionId, Scope};

    const COMMIT_SOURCE: &str = "fn main() {\n    let x = 1;\n}\n";

    /// A repo whose working copy has no `x.rs` (it was deleted after the
    /// picked commit); the commit's own tree still has it.
    struct CommitOnlyVcs;
    impl Vcs for CommitOnlyVcs {
        fn root(&self) -> &Path {
            Path::new("/nowhere")
        }
        fn comparison(&self, _base: Option<&str>) -> Result<Comparison, VcsError> {
            unimplemented!()
        }
        fn changed_files(&self, _cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError> {
            unimplemented!()
        }
        fn file_diff(&self, _cmp: &Comparison, _file: &ChangedFile) -> Result<FileDiff, VcsError> {
            let lines = COMMIT_SOURCE
                .lines()
                .enumerate()
                .map(|(i, content)| DiffLine {
                    kind: LineKind::Added,
                    old_lineno: None,
                    new_lineno: Some(i as u32 + 1),
                    content: content.to_string(),
                })
                .collect();
            Ok(FileDiff::Text {
                hunks: vec![Hunk {
                    old_range: (0, 0),
                    new_range: (1, 3),
                    header: String::new(),
                    lines,
                }],
            })
        }
        fn file_at_ancestor(&self, _cmp: &Comparison, _file: &ChangedFile) -> Option<String> {
            None
        }
        fn file_at_revision(&self, rev: &RevisionId, path: &Path) -> Option<String> {
            (rev.0 == "abc123" && path == Path::new("x.rs")).then(|| COMMIT_SOURCE.to_string())
        }
        fn branches(&self) -> Result<Vec<String>, VcsError> {
            unimplemented!()
        }
        fn commits(
            &self,
            _cmp: &Comparison,
        ) -> Result<Vec<crate::vcs::model::CommitInfo>, VcsError> {
            unimplemented!()
        }
        fn unignored(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
            paths
        }
    }

    /// Under a commit scope the highlighting source is the commit's own
    /// tree, not the working copy — which may have diverged or, as here,
    /// no longer have the file at all.
    #[test]
    fn commit_scope_highlights_from_the_commit_not_the_working_copy() {
        let cmp = Comparison {
            base_label: "main".to_string(),
            ancestor: RevisionId("def456".to_string()),
            work_label: "feature".to_string(),
            scope: Scope::Commit(RevisionId("abc123".to_string())),
            work: None,
        };
        let file = ChangedFile {
            status: FileStatus::Added,
            path: PathBuf::from("x.rs"),
            old_path: None,
        };
        let view = compute(&file, &CommitOnlyVcs, &cmp, ViewOptions::default()).unwrap();
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        let has_spans = sections
            .iter()
            .flat_map(|s| &s.lines)
            .any(|line| matches!(line, ViewLine::Diff { spans, .. } if !spans.is_empty()));
        assert!(has_spans, "commit-scoped view should be highlighted");
    }
}
