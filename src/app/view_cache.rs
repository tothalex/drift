//! Computed views by file index, plus the display options that shape them
//! (expansion, comment folding, per-file block scope).

use std::collections::HashMap;

use anyhow::Result;

use crate::processor;
use crate::processor::view::FileView;
use crate::processor::ViewOptions;
use crate::vcs::model::{ChangedFile, Comparison, FileDiff, LineKind};
use crate::vcs::Vcs;

pub struct ViewCache {
    views: HashMap<usize, FileView>,
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

    pub fn get(&self, index: usize) -> Option<&FileView> {
        self.views.get(&index)
    }

    /// Drop every cached view (options changed).
    pub fn clear_views(&mut self) {
        self.views.clear();
    }

    /// Full reset for a refresh: file indices changed, nothing is valid.
    /// The global toggles (scope, expansion, folding) survive.
    pub fn reset(&mut self) {
        self.views.clear();
    }

    /// Compute and cache the view for a file if it isn't cached yet.
    pub fn ensure(
        &mut self,
        index: usize,
        file: &ChangedFile,
        vcs: &dyn Vcs,
        cmp: &Comparison,
    ) -> Result<()> {
        if self.views.contains_key(&index) {
            return Ok(());
        }
        let view = compute(file, vcs, cmp, self.options_for(index))?;
        self.views.insert(index, view);
        Ok(())
    }

    /// Insert a background-computed view unless one arrived meanwhile.
    pub fn insert_if_absent(&mut self, index: usize, view: FileView) {
        self.views.entry(index).or_insert(view);
    }

    pub fn options_for(&self, _index: usize) -> ViewOptions {
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
    let diff = vcs.file_diff(cmp, file)?;
    // New-side content; None (deleted/unreadable) → hunk fallback.
    let source = std::fs::read_to_string(vcs.root().join(&file.path)).ok();
    // Ancestor-side content is only needed to highlight removed lines;
    // skip the lookup when the diff has none.
    let old_source = if has_removed_lines(&diff) {
        vcs.file_at_ancestor(cmp, file)
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

fn has_removed_lines(diff: &FileDiff) -> bool {
    match diff {
        FileDiff::Binary => false,
        FileDiff::Text { hunks } => hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.kind == LineKind::Removed)),
    }
}
