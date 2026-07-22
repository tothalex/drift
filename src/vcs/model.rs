//! VCS-agnostic domain types.
//!
//! Everything above the `vcs` module — app state, UI — speaks only in these
//! types. Nothing here may reference a concrete VCS.

use std::path::PathBuf;

/// An opaque revision identifier (git: sha, hg: nodeid, jj: change id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionId(pub String);

/// A resolved comparison: what we're reviewing against what.
#[derive(Debug, Clone)]
pub struct Comparison {
    /// What the user thinks of as the base, e.g. "origin/main".
    pub base_label: String,
    /// Where the work diverged from the base (merge-base equivalent).
    /// Diffs run from here to the working copy.
    pub ancestor: RevisionId,
    /// The work being reviewed, e.g. the current branch name.
    pub work_label: String,
    /// Which slice of the work is under review.
    pub scope: Scope,
}

/// A slice of the comparison to review.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// Everything different from the ancestor — committed or not.
    #[default]
    All,
    /// Only files the VCS does not track yet.
    Untracked,
    /// One commit's own changes, against its first parent.
    Commit(RevisionId),
}

/// A commit on the work side, as offered by the scope picker.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: RevisionId,
    /// Abbreviated id for display.
    pub short_id: String,
    /// First line of the commit message.
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
}

impl FileStatus {
    pub fn letter(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Copied => 'C',
            FileStatus::Untracked => '?',
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub status: FileStatus,
    pub path: PathBuf,
    /// Previous path, for renames and copies.
    pub old_path: Option<PathBuf>,
}

/// A parsed diff for one file.
#[derive(Debug, Clone)]
pub enum FileDiff {
    /// No hunks means the content is unchanged (pure rename or mode change).
    Text {
        hunks: Vec<Hunk>,
    },
    Binary,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    /// (start line, line count) on the old side.
    pub old_range: (u32, u32),
    /// (start line, line count) on the new side.
    pub new_range: (u32, u32),
    /// Trailing context from the `@@` line, e.g. the enclosing function.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number on the old side; `None` for added lines.
    pub old_lineno: Option<u32>,
    /// Line number on the new side; `None` for removed lines.
    pub new_lineno: Option<u32>,
    /// Line content without the leading `+`/`-`/space sigil.
    pub content: String,
}
