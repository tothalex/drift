//! Forge-agnostic domain types: pull requests, review threads, comments.
//!
//! Everything above the `forge` module — app state, UI — speaks only in
//! these types. Nothing here may reference a concrete forge; GitHub pull
//! requests and GitLab merge requests both map onto them.

use std::path::PathBuf;

use crate::vcs::model::{ChangedFile, FileDiff};

/// One row of the pull-request picker.
#[derive(Debug, Clone)]
pub struct PullRequest {
    /// GitHub PR number / GitLab MR iid.
    pub number: u64,
    pub title: String,
    pub author: String,
    pub source_branch: String,
    pub target_branch: String,
    pub draft: bool,
    /// RFC 3339 timestamp as the forge sent it; only the date prefix is
    /// displayed.
    pub updated_at: String,
    pub url: String,
}

/// Everything posting and anchoring needs about the opened pull request.
#[derive(Debug, Clone)]
pub struct PrDetail {
    pub number: u64,
    pub title: String,
    /// The PR description.
    pub body: String,
    pub author: String,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: String,
    pub base_sha: String,
    /// GitLab `diff_refs.start_sha`, required by its position API;
    /// `None` on GitHub.
    pub start_sha: Option<String>,
    pub url: String,
}

/// One changed file of the pull request, in the shape the existing
/// diff pipeline consumes.
#[derive(Debug, Clone)]
pub struct PrFile {
    pub changed: ChangedFile,
    pub diff: FileDiff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

/// Where an inline thread attaches to the diff. Both line numbers are
/// carried when known: GitLab requires old *and* new for context lines,
/// GitHub wants (side, line-on-that-side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: PathBuf,
    /// Previous path, for comments on renamed files.
    pub old_path: Option<PathBuf>,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub side: Side,
}

/// An inline review thread: a root comment and its replies.
#[derive(Debug, Clone)]
pub struct CommentThread {
    /// Forge-side key for replying: GitHub root review-comment id
    /// (stringified), GitLab discussion id.
    pub key: String,
    /// `None` when the thread no longer maps to the current diff.
    pub anchor: Option<Anchor>,
    /// The forge says the thread's diff version is no longer current.
    pub outdated: bool,
    /// GitLab reports resolution over REST; GitHub does not (`None`).
    pub resolved: Option<bool>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone)]
pub struct Comment {
    /// Forge-side id of this single comment (GitHub comment id, GitLab
    /// note id), the handle for deleting it; empty when unknown.
    pub id: String,
    pub author: String,
    pub body: String,
    /// RFC 3339 timestamp as the forge sent it.
    pub created_at: String,
}

/// One `Forge::load` result: the whole pull request.
#[derive(Debug, Clone)]
pub struct PrData {
    pub detail: PrDetail,
    pub files: Vec<PrFile>,
    /// Inline review threads.
    pub threads: Vec<CommentThread>,
    /// PR-level conversation comments.
    pub conversation: Vec<Comment>,
}

/// What a composed comment is aimed at.
#[derive(Debug, Clone)]
pub enum ComposeTarget {
    General,
    Reply { thread_key: String },
    Inline { anchor: Anchor },
}
