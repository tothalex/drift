//! The fetch prompt's `git fetch`, against a local "forge": a repository
//! whose pull request head lives only under `refs/pull/<n>/head`, as
//! GitHub keeps it, cloned the way a reviewer's checkout would be.

use std::path::Path;
use std::process::Command;

use drift::forge::fetch::fetch_pr;
use drift::vcs::detect;
use drift::vcs::model::RevisionId;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn a_pull_requests_head_arrives_without_touching_branches_or_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let forge = tmp.path().join("forge");
    std::fs::create_dir(&forge).unwrap();
    git(&forge, &["init", "-q", "-b", "main"]);
    git(&forge, &["config", "user.email", "t@t.co"]);
    git(&forge, &["config", "user.name", "T"]);
    git(&forge, &["config", "commit.gpgsign", "false"]);
    std::fs::write(forge.join("lib.rs"), "fn main() {}\n").unwrap();
    git(&forge, &["add", "."]);
    git(&forge, &["commit", "-qm", "base"]);
    // The pull request's commit, reachable only through the forge's ref.
    git(&forge, &["checkout", "-qb", "feature"]);
    std::fs::write(forge.join("lib.rs"), "fn main() { println!(\"pr\"); }\n").unwrap();
    git(&forge, &["commit", "-qam", "pr work"]);
    let head = git(&forge, &["rev-parse", "HEAD"]);
    git(&forge, &["update-ref", "refs/pull/7/head", &head]);
    git(&forge, &["checkout", "-q", "main"]);
    git(&forge, &["branch", "-qD", "feature"]);

    // `--no-local`: a path clone would copy every object, unreachable
    // ones included — a real forge sends only what the refs reach.
    let clone = tmp.path().join("clone");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--no-local",
            forge.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let rev = RevisionId(head.clone());
    assert!(!detect(&clone).unwrap().has_commit(&rev));

    fetch_pr(&clone, "pull/7/head", None).unwrap();

    assert!(detect(&clone).unwrap().has_commit(&rev));
    assert_eq!(git(&clone, &["branch", "--list"]), "* main");
    assert_eq!(git(&clone, &["status", "--porcelain"]), "");

    let err = fetch_pr(&clone, "pull/99/head", None).unwrap_err();
    assert!(err.contains("pull/99/head"), "{err}");
}
