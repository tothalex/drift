//! Conformance tests for the git provider, driven through the `Vcs` trait.
//! A future provider gets the same assertions against its own fixture.

use std::path::Path;
use std::process::Command;

use drift::vcs::model::{FileDiff, FileStatus, LineKind, Scope};
use drift::vcs::{VcsError, detect};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git");
    assert!(status.status.success(), "git {args:?} failed");
}

/// Like [`git`], with a fixed committer date — commit ordering tests need
/// distinct timestamps.
fn git_dated(dir: &Path, date: &str, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .env("GIT_COMMITTER_DATE", date)
        .current_dir(dir)
        .output()
        .expect("failed to run git");
    assert!(status.status.success(), "git {args:?} failed");
}

fn write(dir: &Path, file: &str, content: &str) {
    std::fs::write(dir.join(file), content).unwrap();
}

/// A repo on branch `feature` with committed work (a modification and a
/// rename), an uncommitted edit, and an untracked file.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "master"]);
    git(dir, &["config", "user.email", "t@t.co"]);
    git(dir, &["config", "user.name", "T"]);
    git(dir, &["config", "commit.gpgsign", "false"]);

    write(dir, "lib.rs", "fn main() {}\n");
    write(dir, "notes.txt", "one\ntwo\n");
    write(dir, "oldname.txt", "rename me\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "initial"]);

    git(dir, &["checkout", "-qb", "feature"]);
    write(dir, "lib.rs", "fn main() { println!(\"hi\"); }\n");
    git(dir, &["mv", "oldname.txt", "newname.txt"]);
    git(dir, &["commit", "-qam", "committed work"]);

    write(dir, "notes.txt", "one\ntwo\nthree uncommitted\n");
    write(dir, "untracked.rs", "brand new\n");
    tmp
}

#[test]
fn reports_committed_uncommitted_renamed_and_untracked() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();

    let cmp = vcs.comparison(Some("master")).unwrap();
    assert_eq!(cmp.base_label, "master");
    assert_eq!(cmp.work_label, "feature");

    let files = vcs.changed_files(&cmp).unwrap();
    let status_of = |name: &str| {
        files
            .iter()
            .find(|f| f.path == Path::new(name))
            .unwrap_or_else(|| panic!("{name} missing from {files:?}"))
    };

    assert_eq!(status_of("lib.rs").status, FileStatus::Modified);
    assert_eq!(status_of("notes.txt").status, FileStatus::Modified); // uncommitted
    assert_eq!(status_of("untracked.rs").status, FileStatus::Untracked);
    let renamed = status_of("newname.txt");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some(Path::new("oldname.txt")));
    assert_eq!(files.len(), 4);
}

#[test]
fn file_diff_returns_structured_hunks() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let cmp = vcs.comparison(Some("master")).unwrap();
    let files = vcs.changed_files(&cmp).unwrap();

    let lib = files
        .iter()
        .find(|f| f.path == Path::new("lib.rs"))
        .unwrap();
    let FileDiff::Text { hunks } = vcs.file_diff(&cmp, lib).unwrap() else {
        panic!("expected text diff");
    };
    assert_eq!(hunks.len(), 1);
    let kinds: Vec<_> = hunks[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, vec![LineKind::Removed, LineKind::Added]);
    assert_eq!(hunks[0].lines[1].content, "fn main() { println!(\"hi\"); }");
    assert_eq!(hunks[0].lines[1].new_lineno, Some(1));
}

#[test]
fn untracked_file_diff_is_all_additions() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let cmp = vcs.comparison(Some("master")).unwrap();
    let files = vcs.changed_files(&cmp).unwrap();

    let new = files
        .iter()
        .find(|f| f.path == Path::new("untracked.rs"))
        .unwrap();
    let FileDiff::Text { hunks } = vcs.file_diff(&cmp, new).unwrap() else {
        panic!("expected text diff");
    };
    assert!(!hunks.is_empty());
    assert!(hunks[0].lines.iter().all(|l| l.kind == LineKind::Added));
}

#[test]
fn pure_rename_has_no_hunks() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let cmp = vcs.comparison(Some("master")).unwrap();
    let files = vcs.changed_files(&cmp).unwrap();

    let renamed = files
        .iter()
        .find(|f| f.path == Path::new("newname.txt"))
        .unwrap();
    let FileDiff::Text { hunks } = vcs.file_diff(&cmp, renamed).unwrap() else {
        panic!("expected text diff");
    };
    assert!(hunks.is_empty());
}

#[test]
fn file_at_revision_reads_local_objects_and_degrades_to_none() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let cmp = vcs.comparison(Some("master")).unwrap();

    // The ancestor commit exists locally: content comes back.
    assert_eq!(
        vcs.file_at_revision(&cmp.ancestor, Path::new("lib.rs"))
            .as_deref(),
        Some("fn main() {}\n")
    );
    // Absent path at that revision, and an unknown revision (the PR-view
    // case when the PR's commits were never fetched): both are None.
    assert_eq!(
        vcs.file_at_revision(&cmp.ancestor, Path::new("nope.rs")),
        None
    );
    let unknown = drift::vcs::model::RevisionId("f".repeat(40));
    assert_eq!(vcs.file_at_revision(&unknown, Path::new("lib.rs")), None);
}

#[test]
fn file_at_ancestor_returns_old_side_content() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let cmp = vcs.comparison(Some("master")).unwrap();
    let files = vcs.changed_files(&cmp).unwrap();

    let lib = files
        .iter()
        .find(|f| f.path == Path::new("lib.rs"))
        .unwrap();
    assert_eq!(
        vcs.file_at_ancestor(&cmp, lib).as_deref(),
        Some("fn main() {}\n")
    );

    // Renames resolve through the old path.
    let renamed = files
        .iter()
        .find(|f| f.path == Path::new("newname.txt"))
        .unwrap();
    assert_eq!(
        vcs.file_at_ancestor(&cmp, renamed).as_deref(),
        Some("rename me\n")
    );

    // Untracked files have no old side.
    let untracked = files
        .iter()
        .find(|f| f.path == Path::new("untracked.rs"))
        .unwrap();
    assert_eq!(vcs.file_at_ancestor(&cmp, untracked), None);
}

#[test]
fn hunk_line_numbers_match_file_content() {
    // Regression: hunk headers/linenos must agree with the content —
    // an insertion mid-file once shifted every lineno by two.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@t.co"]);
    git(dir, &["config", "user.name", "T"]);
    let original: String = (1..=8).map(|i| format!("line {i}\n")).collect();
    write(dir, "f.txt", &original);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "init"]);
    let edited: String = (1..=8)
        .map(|i| {
            if i == 6 {
                format!("inserted\nline {i}\n")
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    write(dir, "f.txt", &edited);

    let vcs = detect(dir).unwrap();
    let cmp = vcs.comparison(Some("main")).unwrap();
    let files = vcs.changed_files(&cmp).unwrap();
    let file = files.iter().find(|f| f.path == Path::new("f.txt")).unwrap();
    let FileDiff::Text { hunks } = vcs.file_diff(&cmp, file).unwrap() else {
        panic!("expected text diff");
    };
    let new_lines: Vec<&str> = edited.lines().collect();
    let old_lines: Vec<&str> = original.lines().collect();
    for hunk in &hunks {
        for line in &hunk.lines {
            if let Some(n) = line.new_lineno {
                assert_eq!(
                    new_lines[n as usize - 1],
                    line.content,
                    "new_lineno {n} doesn't match file content"
                );
            }
            if let Some(n) = line.old_lineno {
                assert_eq!(
                    old_lines[n as usize - 1],
                    line.content,
                    "old_lineno {n} doesn't match old content"
                );
            }
        }
    }
    // The hunk must start where its first context line actually is.
    assert_eq!(hunks[0].lines[0].new_lineno, Some(hunks[0].new_range.0));
}

#[test]
fn commits_lists_branch_commits_newest_first() {
    let tmp = fixture();
    let dir = tmp.path();
    write(dir, "second.txt", "more\n");
    git(dir, &["add", "second.txt"]);
    git_dated(
        dir,
        "2030-01-01T00:00:00",
        &["commit", "-qm", "second change"],
    );

    let vcs = detect(dir).unwrap();
    let cmp = vcs.comparison(Some("master")).unwrap();
    let commits = vcs.commits(&cmp).unwrap();
    let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, vec!["second change", "committed work"]);
    assert!(!commits[0].short_id.is_empty());
    assert!(commits[0].id.0.starts_with(&commits[0].short_id));
}

#[test]
fn untracked_scope_lists_only_untracked_files() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let mut cmp = vcs.comparison(Some("master")).unwrap();
    cmp.scope = Scope::Untracked;

    let files = vcs.changed_files(&cmp).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, Path::new("untracked.rs"));
    assert_eq!(files[0].status, FileStatus::Untracked);
}

#[test]
fn commit_scope_reviews_one_commit_against_its_parent() {
    let tmp = fixture();
    let dir = tmp.path();
    let vcs = detect(dir).unwrap();
    let mut cmp = vcs.comparison(Some("master")).unwrap();
    let commits = vcs.commits(&cmp).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "committed work");
    cmp.scope = Scope::Commit(commits[0].id.clone());

    // The commit modified lib.rs and renamed oldname.txt; the uncommitted
    // edit and the untracked file are outside this scope.
    let files = vcs.changed_files(&cmp).unwrap();
    assert_eq!(files.len(), 2);
    let lib = files
        .iter()
        .find(|f| f.path == Path::new("lib.rs"))
        .unwrap();
    assert_eq!(lib.status, FileStatus::Modified);
    let renamed = files
        .iter()
        .find(|f| f.path == Path::new("newname.txt"))
        .unwrap();
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some(Path::new("oldname.txt")));

    // The new side is the commit's tree, not the working copy: an edit
    // after the commit must not leak into its diff.
    write(dir, "lib.rs", "fn main() { changed again }\n");
    let FileDiff::Text { hunks } = vcs.file_diff(&cmp, lib).unwrap() else {
        panic!("expected text diff");
    };
    assert_eq!(hunks[0].lines[1].content, "fn main() { println!(\"hi\"); }");
    assert_eq!(
        vcs.file_at_ancestor(&cmp, lib).as_deref(),
        Some("fn main() {}\n")
    );
}

#[test]
fn branches_lists_local_branches() {
    let tmp = fixture();
    let vcs = detect(tmp.path()).unwrap();
    let branches = vcs.branches().unwrap();
    assert!(branches.contains(&"master".to_string()));
    assert!(branches.contains(&"feature".to_string()));
}

#[test]
fn non_repo_directory_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(detect(tmp.path()), Err(VcsError::NoRepository(_))));
}

#[test]
fn unborn_branch_reports_missing_revision() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    let vcs = detect(tmp.path()).unwrap();
    assert!(matches!(
        vcs.comparison(Some("main")),
        Err(VcsError::RevisionNotFound(rev)) if rev == "main"
    ));
}

#[test]
fn unignored_filters_gitignored_paths() {
    let tmp = fixture();
    let dir = tmp.path();
    write(dir, ".gitignore", "target/\n*.log\n");
    git(dir, &["add", ".gitignore"]);
    git(dir, &["commit", "-qm", "ignore rules"]);
    let vcs = detect(dir).unwrap();

    let paths = vec![
        "src/main.rs".into(),
        "target/debug/build.d".into(),
        "debug.log".into(),
        "notes.txt".into(),
    ];
    let kept = vcs.unignored(paths);
    assert_eq!(
        kept,
        vec![
            std::path::PathBuf::from("src/main.rs"),
            std::path::PathBuf::from("notes.txt"),
        ]
    );
}
