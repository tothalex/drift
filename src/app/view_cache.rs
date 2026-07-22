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
use crate::vcs::model::{ChangedFile, Comparison, FileDiff, LineKind};

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
    // New-side content; None (deleted/unreadable) → hunk fallback.
    let source = std::fs::read_to_string(vcs.root().join(&file.path))
        .ok()
        .map(processor::tabs::expand_tabs_owned);
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
