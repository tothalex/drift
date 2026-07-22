//! The VCS abstraction: providers implement [`Vcs`], the rest of the app
//! consumes [`model`] types and never sees a concrete VCS.

pub mod gitoxide;
pub mod model;
pub mod unidiff;

use std::path::{Path, PathBuf};

use model::{ChangedFile, CommitInfo, Comparison, FileDiff};

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
    /// notion of a default base (for git: origin/HEAD, then main, master).
    fn comparison(&self, base_override: Option<&str>) -> Result<Comparison, VcsError>;

    /// Everything different between the ancestor and the working copy —
    /// committed or not — plus untracked files, narrowed by `cmp.scope`.
    fn changed_files(&self, cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError>;

    /// Structured diff for one file. Called lazily per selection.
    fn file_diff(&self, cmp: &Comparison, file: &ChangedFile) -> Result<FileDiff, VcsError>;

    /// The file's content on the old side of the comparison (at the
    /// ancestor). Best-effort: `None` when it didn't exist there or can't
    /// be read — callers degrade gracefully.
    fn file_at_ancestor(&self, cmp: &Comparison, file: &ChangedFile) -> Option<String>;

    /// Branches usable as a comparison base, most recently active first.
    fn branches(&self) -> Result<Vec<String>, VcsError>;

    /// Commits on the work side since the ancestor, newest first. Feeds
    /// the scope picker; the comparison's own scope is ignored.
    fn commits(&self, cmp: &Comparison) -> Result<Vec<CommitInfo>, VcsError>;

    /// Of these root-relative paths, the ones the VCS does not ignore.
    /// Used by the file watcher to drop build-artifact noise; best-effort
    /// (on error, paths pass through unfiltered).
    fn unignored(&self, paths: Vec<PathBuf>) -> Vec<PathBuf>;
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
