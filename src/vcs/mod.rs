//! The VCS abstraction: providers implement [`Vcs`], the rest of the app
//! consumes [`model`] types and never sees a concrete VCS.

pub mod gitoxide;
pub mod model;
pub mod unidiff;

use std::path::{Path, PathBuf};

use model::{
    BranchInfo, ChangedFile, CommitInfo, Comparison, FileDiff, RevisionId, Scope, WorktreeInfo,
    WorktreeStats,
};

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("no supported repository found at '{}'", .0.display())]
    NoRepository(PathBuf),
    #[error("revision '{0}' not found (does it have any commits?)")]
    RevisionNotFound(String),
    #[error("could not detect a base branch; pass one with --base")]
    NoDefaultBase,
    #[error("no common ancestor between '{base}' and '{work}'")]
    NoCommonAncestor { base: String, work: String },
    #[error("{0}")]
    Tool(String),
}

/// A version control provider.
///
/// Object-safe by design — the app holds a `Box<dyn Vcs>` chosen at runtime.
/// Providers are stateless query interfaces: the comparison is resolved once
/// via [`Vcs::comparison`] and passed back into the query methods.
pub trait Vcs {
    fn root(&self) -> &Path;

    /// Resolve what to review: an explicit base override, or the provider's
    /// notion of a default base (for git: origin/HEAD, then main, master;
    /// when that shares no history with HEAD, HEAD's upstream).
    fn comparison(&self, base_override: Option<&str>) -> Result<Comparison, VcsError>;

    /// The same, for work that isn't this worktree's HEAD: a branch
    /// picked off the review board. The result carries
    /// [`Comparison::work`], so only committed work is in scope — the
    /// branch may be checked out nowhere, and the working copy beside
    /// drift belongs to a different branch entirely.
    /// A provider that lists no [`Vcs::branch_tips`] never sees this
    /// called — the branch axis has no rows to select — so the default
    /// refuses rather than pretending to review something.
    fn comparison_at(
        &self,
        _base_override: Option<&str>,
        work: &str,
    ) -> Result<Comparison, VcsError> {
        Err(VcsError::RevisionNotFound(work.to_string()))
    }

    /// Everything different between the ancestor and the working copy —
    /// committed or not — plus untracked files, narrowed by `cmp.scope`.
    fn changed_files(&self, cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError>;

    /// Structured diff for one file. Called lazily per selection.
    fn file_diff(&self, cmp: &Comparison, file: &ChangedFile) -> Result<FileDiff, VcsError>;

    /// The file's content on the old side of the comparison (at the
    /// ancestor). Best-effort: `None` when it didn't exist there or can't
    /// be read — callers degrade gracefully.
    fn file_at_ancestor(&self, cmp: &Comparison, file: &ChangedFile) -> Option<String>;

    /// The file's content at an arbitrary revision, if the object exists
    /// locally. Best-effort — the pull-request view uses it to recover
    /// full sources for commits the repo happens to have fetched, and
    /// degrades to the plain hunk view when it returns `None`.
    fn file_at_revision(&self, _rev: &RevisionId, _path: &Path) -> Option<String> {
        None
    }

    /// The file's content at HEAD — the committed scope's new side.
    /// Best-effort, like [`Self::file_at_revision`].
    fn file_at_head(&self, _path: &Path) -> Option<String> {
        None
    }

    /// Branches usable as a comparison base, most recently active first.
    fn branches(&self) -> Result<Vec<String>, VcsError> {
        Ok(self
            .branch_tips()?
            .into_iter()
            .map(|branch| branch.name)
            .collect())
    }

    /// The same branches with their tips and ages, for the review
    /// board's branch axis. Empty for providers without the concept —
    /// the board then has only worktrees to show.
    fn branch_tips(&self) -> Result<Vec<BranchInfo>, VcsError> {
        Ok(Vec::new())
    }

    /// Commits on the work side since the ancestor, newest first — or,
    /// when the work side is the base itself (e.g. sitting on main),
    /// recent history, capped. Feeds the scope picker; the comparison's
    /// own scope is ignored.
    fn commits(&self, cmp: &Comparison) -> Result<Vec<CommitInfo>, VcsError>;

    /// The tip of the work side, when the provider can name it. The
    /// board's counts need it to tell "no work" from "the whole
    /// history": [`Vcs::commits`] deliberately answers with recent
    /// history when the work side *is* the base, which is a picker
    /// convenience and not a count.
    fn work_tip(&self, _cmp: &Comparison) -> Option<RevisionId> {
        None
    }

    /// Of these root-relative paths, the ones the VCS does not ignore.
    /// Used by the file watcher to drop build-artifact noise; best-effort
    /// (on error, paths pass through unfiltered).
    fn unignored(&self, paths: Vec<PathBuf>) -> Vec<PathBuf>;

    /// Every worktree of this repository including the current one,
    /// most recently committed first. Empty for providers without the
    /// concept — the board then has nothing to switch between.
    fn worktrees(&self) -> Result<Vec<WorktreeInfo>, VcsError> {
        Ok(Vec::new())
    }
}

/// Ordered detection: the first provider that recognizes `path` wins.
pub fn detect(path: &Path) -> Result<Box<dyn Vcs>, VcsError> {
    if !path.is_dir() {
        return Err(VcsError::NoRepository(path.to_path_buf()));
    }
    if let Some(git) = gitoxide::GixVcs::detect(path)? {
        return Ok(Box::new(git));
    }
    Err(VcsError::NoRepository(path.to_path_buf()))
}

/// Count what one worktree holds against `base`, for the review board.
///
/// Takes a path rather than a repository handle: this runs on a
/// background thread, one worktree at a time, and provider handles
/// aren't shared across threads. `None` when the worktree can't be
/// read at all — a board row simply stays blank.
pub fn worktree_stats(path: &Path, base: &str) -> Option<WorktreeStats> {
    let vcs = detect(path).ok()?;
    let cmp = vcs.comparison(Some(base)).ok()?;
    let commits = commits_ahead(vcs.as_ref(), &cmp);
    let uncommitted = vcs
        .changed_files(&Comparison {
            scope: Scope::Uncommitted,
            ..cmp
        })
        .map(|files| files.len())
        .unwrap_or(0);
    Some(WorktreeStats {
        commits,
        uncommitted,
    })
}

/// Count what one branch holds against `base`, for the board's branch
/// axis. A branch has no working copy of its own, so only commits are
/// counted; takes a path for the same threading reason as
/// [`worktree_stats`].
pub fn branch_stats(root: &Path, base: &str, branch: &str) -> Option<WorktreeStats> {
    let vcs = detect(root).ok()?;
    let cmp = vcs.comparison_at(Some(base), branch).ok()?;
    Some(WorktreeStats {
        commits: commits_ahead(vcs.as_ref(), &cmp),
        uncommitted: 0,
    })
}

/// How many commits the work side is ahead of the base — the board's
/// `+N`. A branch already merged (or the base itself) is ahead by
/// nothing, however much history it has.
fn commits_ahead(vcs: &dyn Vcs, cmp: &Comparison) -> usize {
    let merged = vcs.work_tip(cmp).is_some_and(|tip| tip == cmp.ancestor);
    match merged {
        true => 0,
        false => vcs.commits(cmp).map(|list| list.len()).unwrap_or(0),
    }
}
