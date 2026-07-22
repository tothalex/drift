//! GitHub via the `gh` CLI.
//!
//! `gh` resolves owner/repo and hostname (github.com or GHE) from the
//! repository's remotes, so every command runs in the repo root and the
//! `{owner}/{repo}` placeholders in `gh api` paths are left to `gh`
//! itself. JSON → model mapping lives in pure functions parsed from
//! fixture strings in tests.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::forge::model::{Anchor, Comment, CommentThread, PrData, PrDetail, PullRequest, Side};
use crate::forge::{Forge, ForgeError, args, concat_arrays, patch, run_cli};

pub struct GhCli {
    root: PathBuf,
    program: String,
    /// Owner/repo never change within a session; fetched once for the
    /// GraphQL calls that can't use gh's `{owner}`/`{repo}` placeholders.
    repo: OnceLock<(String, String)>,
}

impl GhCli {
    pub fn new(root: PathBuf, program: String) -> GhCli {
        GhCli {
            root,
            program,
            repo: OnceLock::new(),
        }
    }

    fn run(&self, args: &[String]) -> Result<String, ForgeError> {
        run_cli("gh", &self.program, &self.root, args)
    }

    /// Owner and repo name — GraphQL has no `{owner}`/`{repo}`
    /// placeholder substitution, so they must be passed as variables.
    /// Fetched once per session.
    fn repo_owner_name(&self) -> Result<(String, String), ForgeError> {
        if let Some(repo) = self.repo.get() {
            return Ok(repo.clone());
        }
        let json = self.run(&args(&["repo", "view", "--json", "owner,name"]))?;
        let repo = parse_repo(&json)?;
        Ok(self.repo.get_or_init(|| repo).clone())
    }

    /// The PR's review threads from GraphQL: node id (the resolve
    /// handle), resolution state, and the root comment's REST id (the
    /// join key back to our threads).
    fn thread_meta(&self, number: u64) -> Result<Vec<ThreadMeta>, ForgeError> {
        let (owner, name) = self.repo_owner_name()?;
        // first:100 is unpaginated: PRs beyond 100 review threads lose
        // resolved state and can't resolve the overflow threads.
        const QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){\
             repository(owner:$owner,name:$name){pullRequest(number:$number){\
             reviewThreads(first:100){nodes{id isResolved \
             comments(first:1){nodes{databaseId}}}}}}}";
        let json = self.run(&args(&[
            "api",
            "graphql",
            "-f",
            &format!("query={QUERY}"),
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("name={name}"),
            "-F",
            &format!("number={number}"),
        ]))?;
        parse_thread_meta(&json)
    }
}

struct ThreadMeta {
    /// REST id of the thread's root review comment.
    root: u64,
    /// GraphQL node id of the thread.
    node_id: String,
    resolved: bool,
}

impl Forge for GhCli {
    fn pr_noun(&self) -> &'static str {
        "pull request"
    }

    fn list_open(&self) -> Result<Vec<PullRequest>, ForgeError> {
        let json = self.run(&args(&[
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,author,headRefName,baseRefName,isDraft,updatedAt,url",
        ]))?;
        parse_list(&json)
    }

    fn load(&self, number: u64) -> Result<PrData, ForgeError> {
        let detail = parse_detail(&self.run(&args(&[
            "api",
            &format!("repos/{{owner}}/{{repo}}/pulls/{number}"),
        ]))?)?;
        let diff = self.run(&args(&["pr", "diff", &number.to_string()]))?;
        let files = patch::split(&diff);
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
        _detail: &PrDetail,
    ) -> Result<(Vec<CommentThread>, Vec<Comment>), ForgeError> {
        let mut threads = parse_threads(&self.run(&args(&[
            "api",
            &format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments"),
            "--paginate",
        ]))?)?;
        // Resolution state only exists in GraphQL; merge it best-effort
        // so a failing GraphQL call degrades to "unknown", not an error.
        if let Ok(meta) = self.thread_meta(number) {
            for thread in &mut threads {
                let root: Option<u64> = thread.key.parse().ok();
                if let Some(meta) = meta.iter().find(|m| Some(m.root) == root) {
                    thread.resolved = Some(meta.resolved);
                }
            }
        }
        let conversation = parse_conversation(&self.run(&args(&[
            "api",
            &format!("repos/{{owner}}/{{repo}}/issues/{number}/comments"),
            "--paginate",
        ]))?)?;
        Ok((threads, conversation))
    }

    fn delete_comment(
        &self,
        _number: u64,
        comment_id: &str,
        inline: bool,
    ) -> Result<(), ForgeError> {
        self.run(&delete_args(comment_id, inline)).map(|_| ())
    }

    fn resolve(&self, number: u64, thread_key: &str, resolved: bool) -> Result<(), ForgeError> {
        // Resolving is GraphQL-only; find the thread node whose root
        // review comment matches our REST-side key.
        let meta = self.thread_meta(number)?;
        let root: Option<u64> = thread_key.parse().ok();
        let node = meta.iter().find(|m| Some(m.root) == root).ok_or_else(|| {
            ForgeError::Invalid("thread not found among review threads".to_string())
        })?;
        let mutation = if resolved {
            "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{id}}}"
        } else {
            "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{id}}}"
        };
        self.run(&args(&[
            "api",
            "graphql",
            "-f",
            &format!("query={mutation}"),
            "-f",
            &format!("id={}", node.node_id),
        ]))
        .map(|_| ())
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
}

fn general_args(number: u64, body: &str) -> Vec<String> {
    args(&[
        "api",
        &format!("repos/{{owner}}/{{repo}}/issues/{number}/comments"),
        "-X",
        "POST",
        "-f",
        &format!("body={body}"),
    ])
}

fn reply_args(number: u64, thread_key: &str, body: &str) -> Vec<String> {
    args(&[
        "api",
        &format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments/{thread_key}/replies"),
        "-X",
        "POST",
        "-f",
        &format!("body={body}"),
    ])
}

/// GitHub addresses comments repo-wide, not per PR.
fn delete_args(comment_id: &str, inline: bool) -> Vec<String> {
    let path = if inline {
        format!("repos/{{owner}}/{{repo}}/pulls/comments/{comment_id}")
    } else {
        format!("repos/{{owner}}/{{repo}}/issues/comments/{comment_id}")
    };
    args(&["api", &path, "-X", "DELETE"])
}

fn inline_args(detail: &PrDetail, anchor: &Anchor, body: &str) -> Result<Vec<String>, ForgeError> {
    let (side, line) = match anchor.side {
        Side::New => ("RIGHT", anchor.new_line),
        Side::Old => ("LEFT", anchor.old_line),
    };
    let line =
        line.ok_or_else(|| ForgeError::Invalid("comment anchor has no line number".to_string()))?;
    Ok(args(&[
        "api",
        &format!("repos/{{owner}}/{{repo}}/pulls/{}/comments", detail.number),
        "-X",
        "POST",
        "-f",
        &format!("body={body}"),
        "-f",
        &format!("commit_id={}", detail.head_sha),
        "-f",
        &format!("path={}", anchor.path.display()),
        "-F",
        &format!("line={line}"),
        "-f",
        &format!("side={side}"),
    ]))
}

#[derive(Deserialize)]
struct ApiUser {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct ListItem {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: Option<ApiUser>,
    #[serde(default, rename = "headRefName")]
    head_ref_name: String,
    #[serde(default, rename = "baseRefName")]
    base_ref_name: String,
    #[serde(default, rename = "isDraft")]
    is_draft: bool,
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
    #[serde(default)]
    url: String,
}

fn parse_list(json: &str) -> Result<Vec<PullRequest>, ForgeError> {
    let items: Vec<ListItem> =
        serde_json::from_str(json).map_err(|err| ForgeError::Parse("gh", err.to_string()))?;
    Ok(items
        .into_iter()
        .map(|item| PullRequest {
            number: item.number,
            title: item.title,
            author: item.author.map(|a| a.login).unwrap_or_default(),
            source_branch: item.head_ref_name,
            target_branch: item.base_ref_name,
            draft: item.is_draft,
            updated_at: item.updated_at,
            url: item.url,
        })
        .collect())
}

#[derive(Deserialize)]
struct ApiRef {
    #[serde(default, rename = "ref")]
    name: String,
    #[serde(default)]
    sha: String,
}

#[derive(Deserialize)]
struct ApiPull {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<ApiUser>,
    head: ApiRef,
    base: ApiRef,
    #[serde(default)]
    html_url: String,
}

fn parse_detail(json: &str) -> Result<PrDetail, ForgeError> {
    let pull: ApiPull =
        serde_json::from_str(json).map_err(|err| ForgeError::Parse("gh", err.to_string()))?;
    Ok(PrDetail {
        number: pull.number,
        title: pull.title,
        body: pull.body.unwrap_or_default(),
        author: pull.user.map(|u| u.login).unwrap_or_default(),
        source_branch: pull.head.name,
        target_branch: pull.base.name,
        head_sha: pull.head.sha,
        base_sha: pull.base.sha,
        start_sha: None,
        url: pull.html_url,
    })
}

#[derive(Deserialize)]
struct ApiReviewComment {
    id: u64,
    #[serde(default)]
    in_reply_to_id: Option<u64>,
    #[serde(default)]
    path: String,
    /// `null` when the thread is outdated (the diff moved on).
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    user: Option<ApiUser>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
}

/// Group flat review comments into threads by their root: replies carry
/// `in_reply_to_id` pointing at the root comment.
fn parse_threads(json: &str) -> Result<Vec<CommentThread>, ForgeError> {
    let comments: Vec<ApiReviewComment> = concat_arrays("gh", json)?;
    let mut threads: Vec<(u64, CommentThread)> = Vec::new();
    for comment in comments {
        let entry = Comment {
            id: comment.id.to_string(),
            author: comment.user.map(|u| u.login).unwrap_or_default(),
            body: comment.body,
            created_at: comment.created_at,
        };
        if let Some(root) = comment.in_reply_to_id
            && let Some((_, thread)) = threads.iter_mut().find(|(id, _)| *id == root)
        {
            thread.comments.push(entry);
            continue;
        }
        let anchor = comment.line.map(|line| {
            let side = match comment.side.as_deref() {
                Some("LEFT") => Side::Old,
                _ => Side::New,
            };
            Anchor {
                path: PathBuf::from(&comment.path),
                old_path: None,
                old_line: (side == Side::Old).then_some(line),
                new_line: (side == Side::New).then_some(line),
                side,
            }
        });
        threads.push((
            comment.id,
            CommentThread {
                key: comment.id.to_string(),
                outdated: anchor.is_none(),
                anchor,
                resolved: None,
                comments: vec![entry],
            },
        ));
    }
    Ok(threads.into_iter().map(|(_, thread)| thread).collect())
}

#[derive(Deserialize)]
struct ApiIssueComment {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    user: Option<ApiUser>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
}

fn parse_conversation(json: &str) -> Result<Vec<Comment>, ForgeError> {
    let comments: Vec<ApiIssueComment> = concat_arrays("gh", json)?;
    Ok(comments
        .into_iter()
        .map(|comment| Comment {
            id: comment.id.map(|id| id.to_string()).unwrap_or_default(),
            author: comment.user.map(|u| u.login).unwrap_or_default(),
            body: comment.body,
            created_at: comment.created_at,
        })
        .collect())
}

#[derive(Deserialize)]
struct ApiRepoOwner {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct ApiRepo {
    name: String,
    owner: ApiRepoOwner,
}

fn parse_repo(json: &str) -> Result<(String, String), ForgeError> {
    let repo: ApiRepo =
        serde_json::from_str(json).map_err(|err| ForgeError::Parse("gh", err.to_string()))?;
    Ok((repo.owner.login, repo.name))
}

fn parse_thread_meta(json: &str) -> Result<Vec<ThreadMeta>, ForgeError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| ForgeError::Parse("gh", err.to_string()))?;
    let nodes = value
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")
        .and_then(|nodes| nodes.as_array())
        .ok_or_else(|| ForgeError::Parse("gh", "unexpected reviewThreads shape".to_string()))?;
    Ok(nodes
        .iter()
        .filter_map(|node| {
            Some(ThreadMeta {
                root: node.pointer("/comments/nodes/0/databaseId")?.as_u64()?,
                node_id: node.get("id")?.as_str()?.to_string(),
                resolved: node.get("isResolved")?.as_bool()?,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses() {
        let json = r#"[{"number":12,"title":"fix parser","author":{"login":"alice"},
            "headRefName":"fix/parser","baseRefName":"main","isDraft":true,
            "updatedAt":"2026-07-01T10:00:00Z","url":"https://github.com/o/r/pull/12"}]"#;
        let prs = parse_list(json).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 12);
        assert_eq!(prs[0].author, "alice");
        assert_eq!(prs[0].source_branch, "fix/parser");
        assert!(prs[0].draft);
    }

    #[test]
    fn detail_parses() {
        let json = r#"{"number":12,"title":"fix parser","body":null,
            "user":{"login":"alice"},
            "head":{"ref":"fix/parser","sha":"aaa111"},
            "base":{"ref":"main","sha":"bbb222"},
            "html_url":"https://github.com/o/r/pull/12"}"#;
        let detail = parse_detail(json).unwrap();
        assert_eq!(detail.head_sha, "aaa111");
        assert_eq!(detail.base_sha, "bbb222");
        assert_eq!(detail.body, "");
        assert!(detail.start_sha.is_none());
    }

    #[test]
    fn threads_group_replies_and_flag_outdated() {
        let json = r#"[
            {"id":1,"path":"src/x.rs","line":42,"side":"RIGHT",
             "user":{"login":"alice"},"body":"root","created_at":"2026-07-01T10:00:00Z"},
            {"id":2,"in_reply_to_id":1,"path":"src/x.rs","line":42,"side":"RIGHT",
             "user":{"login":"bob"},"body":"reply","created_at":"2026-07-01T11:00:00Z"},
            {"id":3,"path":"src/y.rs","line":null,"side":"LEFT",
             "user":{"login":"carol"},"body":"stale","created_at":"2026-07-01T12:00:00Z"}
        ]"#;
        let threads = parse_threads(json).unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].key, "1");
        assert_eq!(threads[0].comments.len(), 2);
        assert_eq!(threads[0].comments[1].author, "bob");
        let anchor = threads[0].anchor.as_ref().unwrap();
        assert_eq!(anchor.new_line, Some(42));
        assert_eq!(anchor.side, Side::New);
        assert!(!threads[0].outdated);
        assert!(threads[1].outdated);
        assert!(threads[1].anchor.is_none());
    }

    #[test]
    fn threads_parse_paginated_output() {
        let page1 = r#"[{"id":1,"path":"a.rs","line":1,"side":"RIGHT","user":{"login":"a"},"body":"x","created_at":""}]"#;
        let page2 = r#"[{"id":2,"path":"b.rs","line":2,"side":"RIGHT","user":{"login":"b"},"body":"y","created_at":""}]"#;
        let threads = parse_threads(&format!("{page1}{page2}")).unwrap();
        assert_eq!(threads.len(), 2);
    }

    #[test]
    fn old_side_comments_anchor_left() {
        let json = r#"[{"id":9,"path":"src/x.rs","line":7,"side":"LEFT",
            "user":{"login":"alice"},"body":"why removed?","created_at":""}]"#;
        let threads = parse_threads(json).unwrap();
        let anchor = threads[0].anchor.as_ref().unwrap();
        assert_eq!(anchor.side, Side::Old);
        assert_eq!(anchor.old_line, Some(7));
        assert_eq!(anchor.new_line, None);
    }

    #[test]
    fn conversation_parses() {
        let json =
            r#"[{"user":{"login":"alice"},"body":"LGTM","created_at":"2026-07-02T09:00:00Z"}]"#;
        let comments = parse_conversation(json).unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "LGTM");
    }

    #[test]
    fn comment_ids_come_through_for_deletion() {
        let threads = parse_threads(
            r#"[{"id":31,"path":"a.rs","line":1,"side":"RIGHT","user":{"login":"a"},"body":"x","created_at":""}]"#,
        )
        .unwrap();
        assert_eq!(threads[0].comments[0].id, "31");
        let conversation =
            parse_conversation(r#"[{"id":77,"user":{"login":"a"},"body":"x","created_at":""}]"#)
                .unwrap();
        assert_eq!(conversation[0].id, "77");
    }

    #[test]
    fn repo_and_thread_meta_parse() {
        let (owner, name) = parse_repo(r#"{"name":"drift","owner":{"login":"tothalex"}}"#).unwrap();
        assert_eq!((owner.as_str(), name.as_str()), ("tothalex", "drift"));

        let json = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
            {"id":"PRRT_abc","isResolved":true,"comments":{"nodes":[{"databaseId":31}]}},
            {"id":"PRRT_def","isResolved":false,"comments":{"nodes":[{"databaseId":42}]}}
        ]}}}}}"#;
        let meta = parse_thread_meta(json).unwrap();
        assert_eq!(meta.len(), 2);
        assert_eq!(meta[0].root, 31);
        assert_eq!(meta[0].node_id, "PRRT_abc");
        assert!(meta[0].resolved);
        assert!(!meta[1].resolved);
    }

    #[test]
    fn delete_args_pick_the_endpoint() {
        assert_eq!(
            delete_args("31", true),
            vec![
                "api",
                "repos/{owner}/{repo}/pulls/comments/31",
                "-X",
                "DELETE"
            ]
        );
        assert_eq!(
            delete_args("77", false),
            vec![
                "api",
                "repos/{owner}/{repo}/issues/comments/77",
                "-X",
                "DELETE"
            ]
        );
    }

    #[test]
    fn post_args_are_exact() {
        assert_eq!(
            general_args(12, "hello"),
            vec![
                "api",
                "repos/{owner}/{repo}/issues/12/comments",
                "-X",
                "POST",
                "-f",
                "body=hello"
            ]
        );
        assert_eq!(
            reply_args(12, "34", "agreed"),
            vec![
                "api",
                "repos/{owner}/{repo}/pulls/12/comments/34/replies",
                "-X",
                "POST",
                "-f",
                "body=agreed"
            ]
        );
        let detail = PrDetail {
            number: 12,
            title: String::new(),
            body: String::new(),
            author: String::new(),
            source_branch: String::new(),
            target_branch: String::new(),
            head_sha: "aaa111".to_string(),
            base_sha: "bbb222".to_string(),
            start_sha: None,
            url: String::new(),
        };
        let anchor = Anchor {
            path: PathBuf::from("src/x.rs"),
            old_path: None,
            old_line: None,
            new_line: Some(42),
            side: Side::New,
        };
        assert_eq!(
            inline_args(&detail, &anchor, "check this").unwrap(),
            vec![
                "api",
                "repos/{owner}/{repo}/pulls/12/comments",
                "-X",
                "POST",
                "-f",
                "body=check this",
                "-f",
                "commit_id=aaa111",
                "-f",
                "path=src/x.rs",
                "-F",
                "line=42",
                "-f",
                "side=RIGHT"
            ]
        );
    }
}
