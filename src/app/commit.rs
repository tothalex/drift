//! The single-commit review's virtual "commit" entry: the message the
//! commit was written with, pinned above its files the way a pull
//! request's conversation is.

use std::path::PathBuf;

use crate::processor::view::{FileView, Section, ViewLine};
use crate::vcs::Vcs;
use crate::vcs::model::{ChangedFile, CommitDetail, Comparison, FileStatus, Scope};

/// Virtual path of the commit entry in the file tree. The leading
/// [`crate::tree::VIRTUAL_PREFIX`] pins it above every directory and file.
pub const COMMIT_PATH: &str = "#commit";

/// The synthetic tree entry for the commit message view.
pub fn entry() -> ChangedFile {
    ChangedFile {
        status: FileStatus::Modified,
        path: PathBuf::from(COMMIT_PATH),
        old_path: None,
    }
}

pub fn is_entry(file: &ChangedFile) -> bool {
    file.path.as_os_str() == COMMIT_PATH
}

/// The commit message view for the comparison's scoped commit. Read
/// fresh rather than handed in, so prefetch workers can build it from
/// their own repository handle like any file view.
pub fn compute(vcs: &dyn Vcs, cmp: &Comparison) -> FileView {
    let detail = match &cmp.scope {
        Scope::Commit(rev) => vcs.commit_detail(rev),
        _ => None,
    };
    view(detail.as_ref())
}

/// Author and date as a thread-style head, then the message as prose.
fn view(detail: Option<&CommitDetail>) -> FileView {
    let mut lines = Vec::new();
    if let Some(detail) = detail {
        lines.push(ViewLine::CommentHead {
            key: String::new(),
            id: String::new(),
            author: detail.author.clone(),
            date: detail.date.clone(),
            replies: 0,
            resolved: None,
            collapsed: false,
        });
    }
    let message = detail.map_or("", |detail| detail.message.as_str());
    let message = match message.is_empty() {
        true => "(no message)",
        false => message,
    };
    lines.extend(super::pr::body_rows("", "", message));
    FileView::Sections {
        sections: vec![Section { lines }],
        scope_max: 0,
        diffstat: (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(message: &str) -> CommitDetail {
        CommitDetail {
            short_id: "abc1234".to_string(),
            author: "Ada".to_string(),
            date: "2026-10-05".to_string(),
            message: message.to_string(),
        }
    }

    fn rows(view: &FileView) -> Vec<String> {
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        sections
            .iter()
            .flat_map(|section| &section.lines)
            .map(|line| match line {
                ViewLine::CommentHead { author, date, .. } => format!("{author} · {date}"),
                ViewLine::CommentBody { text, .. } => text.clone(),
                _ => panic!("unexpected row"),
            })
            .collect()
    }

    #[test]
    fn message_reads_under_its_author_with_the_body_intact() {
        let view = view(Some(&detail(
            "fix: the thing\n\nIt broke because\nof reasons.",
        )));
        assert_eq!(
            rows(&view),
            [
                "Ada · 2026-10-05",
                "fix: the thing",
                "",
                "It broke because",
                "of reasons."
            ]
        );
    }

    #[test]
    fn empty_or_unreadable_message_says_so() {
        assert_eq!(
            rows(&view(Some(&detail("")))).last().unwrap(),
            "(no message)"
        );
        assert_eq!(rows(&view(None)), ["(no message)"]);
    }

    #[test]
    fn entry_is_recognized_by_its_virtual_path() {
        assert!(is_entry(&entry()));
        assert!(
            entry()
                .path
                .to_str()
                .unwrap()
                .starts_with(crate::tree::VIRTUAL_PREFIX)
        );
    }
}
