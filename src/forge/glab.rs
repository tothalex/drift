//! GitLab via the `glab` CLI.
//!
//! Everything goes through `glab api`, whose `:id` placeholder resolves
//! to the URL-encoded project path of the current repo (and whose host
//! comes from the repo's remotes — self-managed instances included).
//! GitLab's inline-comment positions are strict: an added line takes
//! only `new_line`, a removed line only `old_line`, a context line both,
//! and the three diff shas must match the merge request's `diff_refs`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::forge::model::{
    Anchor, Comment, CommentThread, PrData, PrDetail, PrFile, PullRequest, Side,
};
use crate::forge::{Forge, ForgeError, args, concat_arrays, run_cli};
use crate::vcs::model::{ChangedFile, FileStatus};
use crate::vcs::unidiff;

pub struct GlabCli {
    root: PathBuf,
    program: String,
}

impl GlabCli {
    pub fn new(root: PathBuf, program: String) -> GlabCli {
        GlabCli { root, program }
    }

    fn run(&self, args: &[String]) -> Result<String, ForgeError> {
        run_cli("glab", &self.program, &self.root, args)
    }
}

/// API path under the merge request, e.g. `mr(7, "/notes")` — the `:id`
/// project placeholder is resolved by glab itself.
fn mr(number: u64, tail: &str) -> String {
    format!("projects/:id/merge_requests/{number}{tail}")
}

impl Forge for GlabCli {
    fn pr_noun(&self) -> &'static str {
        "merge request"
    }

    fn list_open(&self) -> Result<Vec<PullRequest>, ForgeError> {
        let json = self.run(&args(&[
            "api",
            "projects/:id/merge_requests?state=opened&order_by=updated_at&sort=desc&per_page=100",
        ]))?;
        parse_list(&json)
    }

    fn load(&self, number: u64) -> Result<PrData, ForgeError> {
        let detail = parse_detail(&self.run(&args(&["api", &mr(number, "")]))?)?;
        let files = parse_diffs(&self.run(&args(&[
            "api",
            &mr(number, "/diffs?per_page=100"),
            "--paginate",
        ]))?)?;
        let (threads, conversation) = self.threads(number, &detail)?;
        Ok(PrData {
            detail,
            files,
            threads,
            conversation,
        })
    }

    fn threads(
        &self,
        number: u64,
        detail: &PrDetail,
    ) -> Result<(Vec<CommentThread>, Vec<Comment>), ForgeError> {
        let json = self.run(&args(&[
            "api",
            &mr(number, "/discussions?per_page=100"),
            "--paginate",
        ]))?;
        parse_discussions(&json, &detail.head_sha)
    }

    fn post_general(&self, number: u64, body: &str) -> Result<(), ForgeError> {
        self.run(&general_args(number, body)).map(|_| ())
    }

    fn reply(&self, number: u64, thread_key: &str, body: &str) -> Result<(), ForgeError> {
        self.run(&reply_args(number, thread_key, body)).map(|_| ())
    }

    fn comment_inline(
        &self,
        detail: &PrDetail,
        anchor: &Anchor,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.run(&inline_args(detail, anchor, body)?).map(|_| ())
    }

    fn delete_comment(
        &self,
        number: u64,
        comment_id: &str,
        _inline: bool, // every GitLab comment is a note
    ) -> Result<(), ForgeError> {
        self.run(&delete_args(number, comment_id)).map(|_| ())
    }

    fn resolve(&self, number: u64, thread_key: &str, resolved: bool) -> Result<(), ForgeError> {
        self.run(&resolve_args(number, thread_key, resolved))
            .map(|_| ())
    }
}

fn delete_args(number: u64, comment_id: &str) -> Vec<String> {
    args(&[
        "api",
        &mr(number, &format!("/notes/{comment_id}")),
        "-X",
        "DELETE",
    ])
}

fn resolve_args(number: u64, thread_key: &str, resolved: bool) -> Vec<String> {
    args(&[
        "api",
        &mr(number, &format!("/discussions/{thread_key}")),
        "-X",
        "PUT",
        "-f",
        &format!("resolved={resolved}"),
    ])
}

fn general_args(number: u64, body: &str) -> Vec<String> {
    args(&[
        "api",
        &mr(number, "/notes"),
        "-X",
        "POST",
        "-f",
        &format!("body={body}"),
    ])
}

fn reply_args(number: u64, thread_key: &str, body: &str) -> Vec<String> {
    args(&[
        "api",
        &mr(number, &format!("/discussions/{thread_key}/notes")),
        "-X",
        "POST",
        "-f",
        &format!("body={body}"),
    ])
}

/// GitLab's line rule: added → `new_line` only, removed → `old_line`
/// only, context → both. The anchor carries both linenos exactly for
/// this decision.
fn inline_args(detail: &PrDetail, anchor: &Anchor, body: &str) -> Result<Vec<String>, ForgeError> {
    if anchor.old_line.is_none() && anchor.new_line.is_none() {
        return Err(ForgeError::Invalid(
            "comment anchor has no line number".to_string(),
        ));
    }
    let start_sha = detail.start_sha.as_deref().unwrap_or(&detail.base_sha);
    let old_path = anchor.old_path.as_deref().unwrap_or(&anchor.path);
    let mut out = args(&[
        "api",
        &mr(detail.number, "/discussions"),
        "-X",
        "POST",
        "-f",
        &format!("body={body}"),
        "-f",
        "position[position_type]=text",
        "-f",
        &format!("position[base_sha]={}", detail.base_sha),
        "-f",
        &format!("position[head_sha]={}", detail.head_sha),
        "-f",
        &format!("position[start_sha]={start_sha}"),
        "-f",
        &format!("position[new_path]={}", anchor.path.display()),
        "-f",
        &format!("position[old_path]={}", old_path.display()),
    ]);
    if let Some(line) = anchor.old_line {
        out.push("-f".to_string());
        out.push(format!("position[old_line]={line}"));
    }
    if let Some(line) = anchor.new_line {
        out.push("-f".to_string());
        out.push(format!("position[new_line]={line}"));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct ApiAuthor {
    #[serde(default)]
    username: String,
}

#[derive(Deserialize)]
struct ListItem {
    iid: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: Option<ApiAuthor>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    web_url: String,
}

fn parse_list(json: &str) -> Result<Vec<PullRequest>, ForgeError> {
    let items: Vec<ListItem> = concat_arrays("glab", json)?;
    Ok(items
        .into_iter()
        .map(|item| PullRequest {
            number: item.iid,
            title: item.title,
            author: item.author.map(|a| a.username).unwrap_or_default(),
            source_branch: item.source_branch,
            target_branch: item.target_branch,
            draft: item.draft,
            updated_at: item.updated_at,
            url: item.web_url,
        })
        .collect())
}

#[derive(Default, Deserialize)]
struct ApiDiffRefs {
    #[serde(default)]
    base_sha: String,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    start_sha: String,
}

#[derive(Deserialize)]
struct ApiMergeRequest {
    iid: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<ApiAuthor>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    diff_refs: Option<ApiDiffRefs>,
    #[serde(default)]
    web_url: String,
}

fn parse_detail(json: &str) -> Result<PrDetail, ForgeError> {
    let mr: ApiMergeRequest =
        serde_json::from_str(json).map_err(|err| ForgeError::Parse("glab", err.to_string()))?;
    let refs = mr.diff_refs.unwrap_or_default();
    Ok(PrDetail {
        number: mr.iid,
        title: mr.title,
        body: mr.description.unwrap_or_default(),
        author: mr.author.map(|a| a.username).unwrap_or_default(),
        source_branch: mr.source_branch,
        target_branch: mr.target_branch,
        head_sha: refs.head_sha,
        base_sha: refs.base_sha,
        start_sha: (!refs.start_sha.is_empty()).then_some(refs.start_sha),
        url: mr.web_url,
    })
}

#[derive(Deserialize)]
struct ApiDiff {
    #[serde(default)]
    old_path: String,
    #[serde(default)]
    new_path: String,
    #[serde(default)]
    new_file: bool,
    #[serde(default)]
    renamed_file: bool,
    #[serde(default)]
    deleted_file: bool,
    /// Hunks-only unified diff text.
    #[serde(default)]
    diff: String,
}

fn parse_diffs(json: &str) -> Result<Vec<PrFile>, ForgeError> {
    let diffs: Vec<ApiDiff> = concat_arrays("glab", json)?;
    let mut files: Vec<PrFile> = diffs
        .into_iter()
        .map(|entry| {
            let status = if entry.new_file {
                FileStatus::Added
            } else if entry.deleted_file {
                FileStatus::Deleted
            } else if entry.renamed_file {
                FileStatus::Renamed
            } else {
                FileStatus::Modified
            };
            let path = if entry.deleted_file {
                &entry.old_path
            } else {
                &entry.new_path
            };
            PrFile {
                changed: ChangedFile {
                    status,
                    path: PathBuf::from(path),
                    old_path: entry.renamed_file.then(|| PathBuf::from(&entry.old_path)),
                },
                diff: unidiff::parse(&entry.diff),
            }
        })
        .collect();
    files.sort_by(|a, b| a.changed.path.cmp(&b.changed.path));
    Ok(files)
}

#[derive(Deserialize)]
struct ApiPosition {
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
    #[serde(default)]
    old_line: Option<u32>,
    #[serde(default)]
    new_line: Option<u32>,
    #[serde(default)]
    head_sha: String,
}

#[derive(Deserialize)]
struct ApiNote {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    author: Option<ApiAuthor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    resolvable: bool,
    #[serde(default)]
    resolved: Option<bool>,
    #[serde(default)]
    position: Option<ApiPosition>,
}

#[derive(Deserialize)]
struct ApiDiscussion {
    id: String,
    #[serde(default)]
    notes: Vec<ApiNote>,
}

/// Discussions → (inline threads, conversation comments). System notes
/// (state changes) are dropped; position-less discussions are the
/// conversation; positioned ones become threads, outdated when their
/// position's head sha is no longer the merge request's.
fn parse_discussions(
    json: &str,
    head_sha: &str,
) -> Result<(Vec<CommentThread>, Vec<Comment>), ForgeError> {
    let discussions: Vec<ApiDiscussion> = concat_arrays("glab", json)?;
    let mut threads = Vec::new();
    let mut conversation = Vec::new();
    for discussion in discussions {
        let notes: Vec<&ApiNote> = discussion
            .notes
            .iter()
            .filter(|note| !note.system)
            .collect();
        let Some(first) = notes.first() else {
            continue;
        };
        let comments: Vec<Comment> = notes
            .iter()
            .map(|note| Comment {
                id: note.id.map(|id| id.to_string()).unwrap_or_default(),
                author: note
                    .author
                    .as_ref()
                    .map(|a| a.username.clone())
                    .unwrap_or_default(),
                body: note.body.clone(),
                created_at: note.created_at.clone(),
            })
            .collect();
        let Some(position) = &first.position else {
            conversation.extend(comments);
            continue;
        };
        let outdated = !head_sha.is_empty() && position.head_sha != head_sha;
        let path = position
            .new_path
            .clone()
            .or_else(|| position.old_path.clone());
        let anchor = (!outdated).then_some(path).flatten().map(|path| Anchor {
            path: PathBuf::from(&path),
            old_path: position
                .old_path
                .as_ref()
                .filter(|old| **old != path)
                .map(PathBuf::from),
            old_line: position.old_line,
            new_line: position.new_line,
            side: if position.new_line.is_some() {
                Side::New
            } else {
                Side::Old
            },
        });
        let resolved = notes.iter().any(|note| note.resolvable).then(|| {
            notes
                .iter()
                .all(|note| !note.resolvable || note.resolved == Some(true))
        });
        threads.push(CommentThread {
            key: discussion.id,
            anchor,
            outdated,
            resolved,
            comments,
        });
    }
    Ok((threads, conversation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses() {
        let json = r#"[{"iid":7,"title":"add cache","author":{"username":"mia"},
            "source_branch":"cache","target_branch":"main","draft":false,
            "updated_at":"2026-07-03T08:00:00Z","web_url":"https://gitlab.com/g/p/-/merge_requests/7"}]"#;
        let mrs = parse_list(json).unwrap();
        assert_eq!(mrs.len(), 1);
        assert_eq!(mrs[0].number, 7);
        assert_eq!(mrs[0].author, "mia");
    }

    #[test]
    fn detail_parses_diff_refs() {
        let json = r#"{"iid":7,"title":"add cache","description":"why not",
            "author":{"username":"mia"},"source_branch":"cache","target_branch":"main",
            "diff_refs":{"base_sha":"b1","head_sha":"h1","start_sha":"s1"},
            "web_url":"https://gitlab.com/g/p/-/merge_requests/7"}"#;
        let detail = parse_detail(json).unwrap();
        assert_eq!(detail.head_sha, "h1");
        assert_eq!(detail.base_sha, "b1");
        assert_eq!(detail.start_sha.as_deref(), Some("s1"));
        assert_eq!(detail.body, "why not");
    }

    #[test]
    fn diffs_parse_statuses_and_hunks() {
        let json = r#"[
            {"old_path":"src/a.rs","new_path":"src/a.rs","new_file":false,
             "renamed_file":false,"deleted_file":false,
             "diff":"@@ -1,2 +1,2 @@\n context\n-old\n+new\n"},
            {"old_path":"was.rs","new_path":"is.rs","new_file":false,
             "renamed_file":true,"deleted_file":false,"diff":""},
            {"old_path":"gone.rs","new_path":"gone.rs","new_file":false,
             "renamed_file":false,"deleted_file":true,"diff":"@@ -1 +0,0 @@\n-bye\n"}
        ]"#;
        let files = parse_diffs(json).unwrap();
        let summary: Vec<(char, &str)> = files
            .iter()
            .map(|f| (f.changed.status.letter(), f.changed.path.to_str().unwrap()))
            .collect();
        assert_eq!(
            summary,
            vec![('D', "gone.rs"), ('R', "is.rs"), ('M', "src/a.rs")]
        );
        let modified = files
            .iter()
            .find(|f| f.changed.path.ends_with("a.rs"))
            .unwrap();
        let crate::vcs::model::FileDiff::Text { hunks } = &modified.diff else {
            panic!("expected text diff");
        };
        assert_eq!(hunks[0].lines.len(), 3);
        let renamed = files
            .iter()
            .find(|f| f.changed.path.ends_with("is.rs"))
            .unwrap();
        assert_eq!(
            renamed
                .changed
                .old_path
                .as_deref()
                .unwrap()
                .to_str()
                .unwrap(),
            "was.rs"
        );
    }

    #[test]
    fn discussions_split_threads_and_conversation() {
        let json = r#"[
            {"id":"d1","notes":[{"system":true,"body":"changed milestone","created_at":""}]},
            {"id":"d2","notes":[{"system":false,"author":{"username":"mia"},
                "body":"nice work","created_at":"2026-07-01T09:00:00Z"}]},
            {"id":"d3","notes":[
                {"system":false,"author":{"username":"mia"},"body":"why this?",
                 "created_at":"","resolvable":true,"resolved":false,
                 "position":{"old_path":"src/a.rs","new_path":"src/a.rs",
                             "old_line":null,"new_line":5,"head_sha":"h1"}},
                {"system":false,"author":{"username":"leo"},"body":"legacy",
                 "created_at":"","resolvable":true,"resolved":false}
            ]}
        ]"#;
        let (threads, conversation) = parse_discussions(json, "h1").unwrap();
        assert_eq!(conversation.len(), 1);
        assert_eq!(conversation[0].body, "nice work");
        assert_eq!(threads.len(), 1);
        let thread = &threads[0];
        assert_eq!(thread.key, "d3");
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.resolved, Some(false));
        assert!(!thread.outdated);
        let anchor = thread.anchor.as_ref().unwrap();
        assert_eq!(anchor.new_line, Some(5));
        assert_eq!(anchor.side, Side::New);
        assert!(anchor.old_path.is_none());
    }

    #[test]
    fn stale_position_marks_outdated() {
        let json = r#"[{"id":"d1","notes":[{"system":false,"author":{"username":"mia"},
            "body":"old","created_at":"","position":{"old_path":"a.rs","new_path":"a.rs",
            "old_line":null,"new_line":3,"head_sha":"older"}}]}]"#;
        let (threads, _) = parse_discussions(json, "h1").unwrap();
        assert!(threads[0].outdated);
        assert!(threads[0].anchor.is_none());
    }

    #[test]
    fn post_args_follow_the_line_rule() {
        let detail = PrDetail {
            number: 7,
            title: String::new(),
            body: String::new(),
            author: String::new(),
            source_branch: String::new(),
            target_branch: String::new(),
            head_sha: "h1".to_string(),
            base_sha: "b1".to_string(),
            start_sha: Some("s1".to_string()),
            url: String::new(),
        };
        // Added line: new_line only.
        let added = Anchor {
            path: PathBuf::from("src/a.rs"),
            old_path: None,
            old_line: None,
            new_line: Some(5),
            side: Side::New,
        };
        let args = inline_args(&detail, &added, "hm").unwrap();
        assert!(args.contains(&"position[new_line]=5".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("position[old_line]")));
        assert!(args.contains(&"position[start_sha]=s1".to_string()));
        assert!(args.contains(&"position[old_path]=src/a.rs".to_string()));
        // Context line: both.
        let context = Anchor {
            old_line: Some(4),
            new_line: Some(5),
            ..added.clone()
        };
        let args = inline_args(&detail, &context, "hm").unwrap();
        assert!(args.contains(&"position[old_line]=4".to_string()));
        assert!(args.contains(&"position[new_line]=5".to_string()));
        // Removed line: old_line only.
        let removed = Anchor {
            old_line: Some(4),
            new_line: None,
            side: Side::Old,
            ..added.clone()
        };
        let args = inline_args(&detail, &removed, "hm").unwrap();
        assert!(args.contains(&"position[old_line]=4".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("position[new_line]")));

        assert_eq!(
            general_args(7, "hi"),
            vec![
                "api",
                "projects/:id/merge_requests/7/notes",
                "-X",
                "POST",
                "-f",
                "body=hi"
            ]
        );
        assert_eq!(
            reply_args(7, "d3", "ok"),
            vec![
                "api",
                "projects/:id/merge_requests/7/discussions/d3/notes",
                "-X",
                "POST",
                "-f",
                "body=ok"
            ]
        );
        assert_eq!(
            delete_args(7, "901"),
            vec![
                "api",
                "projects/:id/merge_requests/7/notes/901",
                "-X",
                "DELETE"
            ]
        );
        assert_eq!(
            resolve_args(7, "d3", true),
            vec![
                "api",
                "projects/:id/merge_requests/7/discussions/d3",
                "-X",
                "PUT",
                "-f",
                "resolved=true"
            ]
        );
    }
}
