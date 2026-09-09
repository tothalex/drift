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
    /// The tip of the work side when it isn't this worktree's HEAD: a
    /// branch reviewed from the board, which has no working copy of its
    /// own, so only committed work exists to review. `None` is the live
    /// review — HEAD plus whatever is uncommitted beside it.
    pub work: Option<RevisionId>,
}

/// A branch as the review board lists it: where its tip is and when it
/// last moved, so a row can be drawn before anything is counted.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// Short name, as the base picker spells it ("main", "origin/main").
    pub name: String,
    pub tip: RevisionId,
    /// Commit time of the tip, for the age column.
    pub last_commit: i64,
}

/// A slice of the comparison to review.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// Everything different from the ancestor — committed or not.
    #[default]
    All,
    /// Only what is committed: HEAD against the ancestor, the working
    /// copy left out.
    Committed,
    /// Everything not committed yet: the working copy against HEAD,
    /// tracked and untracked alike — what `git status` reports.
    Uncommitted,
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

/// One worktree of the repository, as the review board lists it.
///
/// Cheap to build — refs only, no status scan. The counts the board
/// shows arrive later as [`WorktreeStats`].
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Directory name, which is what the board shows ("sms-provider").
    pub name: String,
    pub path: PathBuf,
    /// Checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Tip commit time in unix seconds — the board's sort key, because
    /// the worktree an agent just touched is the one worth looking at.
    pub last_commit: i64,
}

/// What a worktree holds relative to the base, scanned in the
/// background because the uncommitted count needs a full status walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeStats {
    /// Commits since the merge-base.
    pub commits: usize,
    /// Files the working copy changed on top of them.
    pub uncommitted: usize,
}
