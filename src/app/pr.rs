//! The pull-request review session: its state, the view computation that
//! replaces `view_cache::compute` while a PR is open, and the virtual
//! "conversation" file carrying the description, PR-level comments, and
//! threads that no longer anchor to the diff.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::forge::date_of;
use crate::forge::model::{Anchor, Comment, CommentThread, PrData, Side};
use crate::processor::view::{FileView, Section, ViewLine};
use crate::processor::{self, ViewOptions};
use crate::vcs::Vcs;
use crate::vcs::model::{ChangedFile, DiffLine, FileStatus, RevisionId};

/// Virtual path of the conversation entry in the file tree.
pub const CONVERSATION_PATH: &str = "#conversation";

/// Comment bodies wrap to this many columns at view-build time so rows
/// are stable for the cursor and renderer.
const WRAP_WIDTH: usize = 96;

/// An open pull request under review. While set, the app's file list and
/// views come from the forge data instead of the working tree.
pub struct PrSession {
    pub data: Arc<PrData>,
    /// Thread keys folded down to their head row (`t` toggles).
    pub collapsed: HashSet<String>,
}

/// The synthetic tree entry for the conversation view.
pub fn conversation_entry() -> ChangedFile {
    ChangedFile {
        status: FileStatus::Modified,
        path: PathBuf::from(CONVERSATION_PATH),
        old_path: None,
    }
}

pub fn is_conversation(file: &ChangedFile) -> bool {
    file.path.as_os_str() == CONVERSATION_PATH
}

/// Compute one file's view from the PR data: the forge-provided diff run
/// through the normal processor, with full sources recovered from the
/// local object store when the PR's commits happen to exist there (no
/// fetch is ever run), and inline threads spliced under their lines.
pub fn compute(
    data: &PrData,
    collapsed: &HashSet<String>,
    file: &ChangedFile,
    vcs: &dyn Vcs,
    options: ViewOptions,
) -> FileView {
    if is_conversation(file) {
        return conversation_view(data);
    }
    let Some(pr_file) = data.files.iter().find(|f| f.changed.path == file.path) else {
        return FileView::Unchanged;
    };
    let mut diff = pr_file.diff.clone();
    processor::tabs::expand_diff(&mut diff);
    let head = RevisionId(data.detail.head_sha.clone());
    let base = RevisionId(data.detail.base_sha.clone());
    let new_source = (file.status != FileStatus::Deleted)
        .then(|| vcs.file_at_revision(&head, &file.path))
        .flatten()
        .map(processor::tabs::expand_tabs_owned);
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let old_source = if super::view_cache::has_removed_lines(&diff) {
        vcs.file_at_revision(&base, old_path)
            .map(processor::tabs::expand_tabs_owned)
    } else {
        None
    };
    let mut view = processor::process(
        &file.path,
        &diff,
        new_source.as_deref(),
        old_source.as_deref(),
        options,
    );
    splice_threads(&mut view, &data.threads, collapsed, &file.path);
    view
}

/// Insert each thread's rows right under the diff line it anchors to.
/// Anchored threads whose line isn't visible (collapsed away, or a stale
/// forge position) still belong to this file: they're appended at the end
/// rather than dropped.
fn splice_threads(
    view: &mut FileView,
    threads: &[CommentThread],
    collapsed: &HashSet<String>,
    path: &Path,
) {
    let FileView::Sections { sections, .. } = view else {
        return;
    };
    let mut unplaced: Vec<&CommentThread> = Vec::new();
    for thread in threads {
        let Some(anchor) = &thread.anchor else {
            continue; // unanchored: shown in the conversation view
        };
        if anchor.path != path {
            continue;
        }
        let placed = sections.iter_mut().any(|section| {
            let Some(at) = section.lines.iter().position(
                |line| matches!(line, ViewLine::Diff { line, .. } if anchors_at(anchor, line)),
            ) else {
                return false;
            };
            let rows = thread_rows(thread, collapsed.contains(&thread.key), THREAD_HINT);
            section.lines.splice(at + 1..at + 1, rows);
            true
        });
        if !placed {
            unplaced.push(thread);
        }
    }
    for thread in unplaced {
        let line = thread
            .anchor
            .as_ref()
            .and_then(|a| a.new_line.or(a.old_line))
            .map_or(String::new(), |n| format!("line {n}, "));
        let mut lines = vec![ViewLine::CommentBody {
            key: thread.key.clone(),
            id: String::new(),
            text: format!("thread on {line}not shown in this view:"),
        }];
        lines.extend(thread_rows(
            thread,
            collapsed.contains(&thread.key),
            THREAD_HINT,
        ));
        sections.push(Section { lines });
    }
}

/// Does the diff line carry the anchor's line number on the anchor's side?
fn anchors_at(anchor: &Anchor, line: &DiffLine) -> bool {
    match anchor.side {
        Side::New => anchor.new_line.is_some() && line.new_lineno == anchor.new_line,
        Side::Old => anchor.old_line.is_some() && line.old_lineno == anchor.old_line,
    }
}

/// The key hint under an anchored thread; orphaned threads in the
/// conversation drop the fold (the conversation never folds them).
const THREAD_HINT: &str = "[a] reply · [t] fold · [r] resolve · [d] delete (on a comment)";
const ORPHAN_HINT: &str = "[a] reply · [r] resolve · [d] delete (on a comment)";

/// A thread as view rows: every comment expanded and a trailing key
/// hint, or just a head row summarizing the fold.
fn thread_rows(thread: &CommentThread, collapsed: bool, hint: &str) -> Vec<ViewLine> {
    let Some(first) = thread.comments.first() else {
        return Vec::new();
    };
    if collapsed {
        return vec![ViewLine::CommentHead {
            key: thread.key.clone(),
            id: first.id.clone(),
            author: first.author.clone(),
            date: date_of(&first.created_at).to_string(),
            replies: thread.comments.len() - 1,
            resolved: thread.resolved,
            collapsed: true,
        }];
    }
    let mut rows = Vec::new();
    for (nth, comment) in thread.comments.iter().enumerate() {
        rows.push(ViewLine::CommentHead {
            key: thread.key.clone(),
            id: comment.id.clone(),
            author: comment.author.clone(),
            date: date_of(&comment.created_at).to_string(),
            replies: 0,
            resolved: if nth == 0 { thread.resolved } else { None },
            collapsed: false,
        });
        rows.extend(body_rows(&thread.key, &comment.id, &comment.body));
    }
    rows.push(ViewLine::CommentHint {
        key: thread.key.clone(),
        text: hint.to_string(),
    });
    rows
}

/// The conversation view: PR description, then the PR-level comments,
/// then inline threads that no longer anchor anywhere (still replyable
/// from here via their head rows).
fn conversation_view(data: &PrData) -> FileView {
    let detail = &data.detail;
    let mut sections = Vec::new();

    let mut header = vec![ViewLine::CommentHead {
        key: String::new(),
        id: String::new(),
        author: detail.author.clone(),
        date: String::new(),
        replies: 0,
        resolved: None,
        collapsed: false,
    }];
    let body = if detail.body.trim().is_empty() {
        "(no description)"
    } else {
        detail.body.as_str()
    };
    header.extend(body_rows("", "", body));
    header.push(ViewLine::CommentHint {
        key: String::new(),
        text: "[a] comment".to_string(),
    });
    sections.push(Section { lines: header });

    for comment in &data.conversation {
        sections.push(Section {
            lines: comment_rows("", comment),
        });
    }

    let orphaned: Vec<&CommentThread> = data
        .threads
        .iter()
        .filter(|thread| thread.anchor.is_none())
        .collect();
    if !orphaned.is_empty() {
        sections.push(Section {
            lines: vec![ViewLine::CommentBody {
                key: String::new(),
                id: String::new(),
                text: format!("outdated threads ({}), from earlier diffs:", orphaned.len()),
            }],
        });
        for thread in orphaned {
            sections.push(Section {
                lines: thread_rows(thread, false, ORPHAN_HINT),
            });
        }
    }
    FileView::Sections {
        sections,
        scope_max: 0,
        diffstat: (0, 0),
    }
}

/// One comment as rows: a head plus its wrapped body.
fn comment_rows(key: &str, comment: &Comment) -> Vec<ViewLine> {
    let mut rows = vec![ViewLine::CommentHead {
        key: key.to_string(),
        id: comment.id.clone(),
        author: comment.author.clone(),
        date: date_of(&comment.created_at).to_string(),
        replies: 0,
        resolved: None,
        collapsed: false,
    }];
    rows.extend(body_rows(key, &comment.id, &comment.body));
    rows
}

fn body_rows(key: &str, id: &str, body: &str) -> Vec<ViewLine> {
    wrap(body, WRAP_WIDTH)
        .into_iter()
        .map(|text| ViewLine::CommentBody {
            key: key.to_string(),
            id: id.to_string(),
            text,
        })
        .collect()
}

/// Greedy word wrap preserving explicit newlines; words longer than the
/// width hard-break. Every body renders at least one (possibly empty) row.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.replace('\r', "").split('\n') {
        if line.chars().count() <= width {
            rows.push(line.to_string());
            continue;
        }
        let mut row = String::new();
        let mut row_len = 0;
        for word in line.split(' ') {
            let word_len = word.chars().count();
            if row_len > 0 && row_len + 1 + word_len > width {
                rows.push(std::mem::take(&mut row));
                row_len = 0;
            }
            if row_len > 0 {
                row.push(' ');
                row_len += 1;
            }
            // A single overlong word hard-breaks at the width.
            let mut chars = word.chars().peekable();
            while chars.peek().is_some() {
                let take = width.saturating_sub(row_len).max(1);
                let piece: String = chars.by_ref().take(take).collect();
                row_len += piece.chars().count();
                row.push_str(&piece);
                if chars.peek().is_some() {
                    rows.push(std::mem::take(&mut row));
                    row_len = 0;
                }
            }
        }
        rows.push(row);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::model::{PrDetail, PrFile};
    use crate::vcs::model::{FileDiff, Hunk, LineKind};

    fn detail() -> PrDetail {
        PrDetail {
            number: 12,
            title: "fix parser".to_string(),
            body: "why and how".to_string(),
            author: "alice".to_string(),
            source_branch: "fix".to_string(),
            target_branch: "main".to_string(),
            head_sha: "nonexistent-head".to_string(),
            base_sha: "nonexistent-base".to_string(),
            start_sha: None,
            url: String::new(),
        }
    }

    fn diff_line(kind: LineKind, old: Option<u32>, new: Option<u32>, content: &str) -> DiffLine {
        DiffLine {
            kind,
            old_lineno: old,
            new_lineno: new,
            content: content.to_string(),
        }
    }

    fn one_file_data(threads: Vec<CommentThread>) -> PrData {
        PrData {
            detail: detail(),
            files: vec![PrFile {
                changed: ChangedFile {
                    status: FileStatus::Modified,
                    path: PathBuf::from("src/a.rs"),
                    old_path: None,
                },
                diff: FileDiff::Text {
                    hunks: vec![Hunk {
                        old_range: (1, 2),
                        new_range: (1, 2),
                        header: String::new(),
                        lines: vec![
                            diff_line(LineKind::Context, Some(1), Some(1), "fn main() {"),
                            diff_line(LineKind::Removed, Some(2), None, "    old();"),
                            diff_line(LineKind::Added, None, Some(2), "    new();"),
                        ],
                    }],
                },
            }],
            threads,
            conversation: vec![Comment {
                id: "900".to_string(),
                author: "bob".to_string(),
                body: "LGTM overall".to_string(),
                created_at: "2026-07-02T10:00:00Z".to_string(),
            }],
        }
    }

    fn thread(key: &str, anchor: Option<Anchor>) -> CommentThread {
        CommentThread {
            key: key.to_string(),
            anchor,
            outdated: false,
            resolved: None,
            comments: vec![
                Comment {
                    id: "31".to_string(),
                    author: "mia".to_string(),
                    body: "why this?".to_string(),
                    created_at: "2026-07-01T09:00:00Z".to_string(),
                },
                Comment {
                    id: "32".to_string(),
                    author: "leo".to_string(),
                    body: "legacy reasons".to_string(),
                    created_at: "2026-07-01T10:00:00Z".to_string(),
                },
            ],
        }
    }

    fn anchor_new(line: u32) -> Anchor {
        Anchor {
            path: PathBuf::from("src/a.rs"),
            old_path: None,
            old_line: None,
            new_line: Some(line),
            side: Side::New,
        }
    }

    /// A no-op Vcs for tests: no local objects exist.
    struct NoVcs;
    impl Vcs for NoVcs {
        fn root(&self) -> &Path {
            Path::new("/nowhere")
        }
        fn comparison(
            &self,
            _base: Option<&str>,
        ) -> Result<crate::vcs::model::Comparison, crate::vcs::VcsError> {
            unimplemented!()
        }
        fn changed_files(
            &self,
            _cmp: &crate::vcs::model::Comparison,
        ) -> Result<Vec<ChangedFile>, crate::vcs::VcsError> {
            unimplemented!()
        }
        fn file_diff(
            &self,
            _cmp: &crate::vcs::model::Comparison,
            _file: &ChangedFile,
        ) -> Result<FileDiff, crate::vcs::VcsError> {
            unimplemented!()
        }
        fn file_at_ancestor(
            &self,
            _cmp: &crate::vcs::model::Comparison,
            _file: &ChangedFile,
        ) -> Option<String> {
            None
        }
        fn branches(&self) -> Result<Vec<String>, crate::vcs::VcsError> {
            unimplemented!()
        }
        fn commits(
            &self,
            _cmp: &crate::vcs::model::Comparison,
        ) -> Result<Vec<crate::vcs::model::CommitInfo>, crate::vcs::VcsError> {
            unimplemented!()
        }
        fn unignored(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
            paths
        }
    }

    fn rows_of(view: &FileView) -> Vec<String> {
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        sections
            .iter()
            .flat_map(|s| &s.lines)
            .map(|line| match line {
                ViewLine::Diff { line, .. } => format!("diff:{}", line.content),
                ViewLine::CommentHead { author, .. } => format!("head:{author}"),
                ViewLine::CommentBody { text, .. } => format!("body:{text}"),
                ViewLine::CommentHint { text, .. } => format!("hint:{text}"),
                ViewLine::Collapsed { .. } => "collapsed".to_string(),
                ViewLine::CommentFold { .. } => "fold".to_string(),
            })
            .collect()
    }

    #[test]
    fn compute_splices_thread_under_its_line_without_sources() {
        let data = one_file_data(vec![thread("t1", Some(anchor_new(2)))]);
        let view = compute(
            &data,
            &HashSet::new(),
            &data.files[0].changed,
            &NoVcs,
            ViewOptions::default(),
        );
        let rows = rows_of(&view);
        let added = rows.iter().position(|r| r == "diff:    new();").unwrap();
        assert_eq!(rows[added + 1], "head:mia");
        assert_eq!(rows[added + 2], "body:why this?");
        assert_eq!(rows[added + 3], "head:leo");
        assert_eq!(rows[added + 4], "body:legacy reasons");
        // The reply affordance sits right under the thread.
        assert_eq!(
            rows[added + 5],
            "hint:[a] reply · [t] fold · [r] resolve · [d] delete (on a comment)"
        );
    }

    #[test]
    fn collapsed_thread_is_one_head_row() {
        let data = one_file_data(vec![thread("t1", Some(anchor_new(2)))]);
        let collapsed = HashSet::from(["t1".to_string()]);
        let view = compute(
            &data,
            &collapsed,
            &data.files[0].changed,
            &NoVcs,
            ViewOptions::default(),
        );
        let rows = rows_of(&view);
        assert!(rows.contains(&"head:mia".to_string()));
        assert!(!rows.contains(&"head:leo".to_string()));
        assert!(!rows.contains(&"body:why this?".to_string()));
    }

    #[test]
    fn old_side_anchor_lands_on_removed_line() {
        let anchor = Anchor {
            old_line: Some(2),
            new_line: None,
            side: Side::Old,
            ..anchor_new(0)
        };
        let data = one_file_data(vec![thread("t1", Some(anchor))]);
        let view = compute(
            &data,
            &HashSet::new(),
            &data.files[0].changed,
            &NoVcs,
            ViewOptions::default(),
        );
        let rows = rows_of(&view);
        let removed = rows.iter().position(|r| r == "diff:    old();").unwrap();
        assert_eq!(rows[removed + 1], "head:mia");
    }

    #[test]
    fn unmatched_anchor_appends_thread_at_the_end() {
        let data = one_file_data(vec![thread("t1", Some(anchor_new(999)))]);
        let view = compute(
            &data,
            &HashSet::new(),
            &data.files[0].changed,
            &NoVcs,
            ViewOptions::default(),
        );
        let rows = rows_of(&view);
        let note = rows
            .iter()
            .position(|r| r.starts_with("body:thread on line 999"))
            .unwrap();
        assert_eq!(rows[note + 1], "head:mia");
    }

    #[test]
    fn conversation_lists_description_comments_and_orphans() {
        let data = one_file_data(vec![thread("t1", None)]);
        let view = compute(
            &data,
            &HashSet::new(),
            &conversation_entry(),
            &NoVcs,
            ViewOptions::default(),
        );
        let rows = rows_of(&view);
        assert_eq!(rows[0], "head:alice");
        assert_eq!(rows[1], "body:why and how");
        assert_eq!(rows[2], "hint:[a] comment");
        assert!(rows.contains(&"head:bob".to_string()));
        assert!(rows.contains(&"body:LGTM overall".to_string()));
        assert!(
            rows.iter()
                .any(|r| r.starts_with("body:outdated threads (1)"))
        );
        assert!(rows.contains(&"head:mia".to_string()));
    }

    #[test]
    fn wrap_preserves_newlines_and_breaks_long_words() {
        assert_eq!(wrap("a b", 10), vec!["a b"]);
        assert_eq!(wrap("one\ntwo", 10), vec!["one", "two"]);
        assert_eq!(wrap("", 10), vec![""]);
        let wrapped = wrap("aaaa bbbb cccc", 9);
        assert_eq!(wrapped, vec!["aaaa bbbb", "cccc"]);
        let hard = wrap("abcdefghijkl", 5);
        assert_eq!(hard, vec!["abcde", "fghij", "kl"]);
    }
}
