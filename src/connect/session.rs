//! Where an agent is actually working.
//!
//! A multiplexer only knows the working directory of the pane's own
//! process, and that stays wherever the agent was launched: a Claude
//! session that moves into a linked worktree (`.claude/worktrees/…`)
//! never changes it, so the review board would credit its work to the
//! branch of the checkout it started from — the row it reads as
//! "working on" is then the wrong one. Claude Code records the real
//! directory in its own transcript, and herdr reports the session id
//! that finds it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Transcript tail read to find the newest entry. Entries are appended,
/// so the answer is at the end; the window only has to be wide enough
/// to hold one whole line, and a single tool result can be large.
const TAIL: u64 = 512 * 1024;

/// The directory `agent`'s session is working in, for the agents that
/// keep a readable record of one. `None` for anything drift cannot
/// resolve — the caller then keeps the pane's own directory, which is
/// right for every agent that never left it.
pub(super) fn working_dir(agent: &str, session: &str) -> Option<PathBuf> {
    match agent {
        "claude" => latest_cwd(&transcript(&projects_root()?, session)?),
        _ => None,
    }
}

/// `~/.claude/projects`, honoring `CLAUDE_CONFIG_DIR`.
fn projects_root() -> Option<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::home_dir()?.join(".claude"),
    };
    Some(base.join("projects"))
}

/// The transcript file for a session id. A project directory is named
/// after the directory claude was launched in, not the one the session
/// went on to work in — the very thing this lookup exists to find — so
/// the id is searched for rather than derived from a path.
fn transcript(root: &Path, session: &str) -> Option<PathBuf> {
    let file = format!("{session}.jsonl");
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// One transcript line, of which only the directory matters.
#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    cwd: String,
}

/// The `cwd` of the last transcript entry that carries one. Lines are
/// parsed whole rather than scanned for the key: a tool result can
/// quote anything, and only a top-level field is the session's own.
fn latest_cwd(path: &Path) -> Option<PathBuf> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    // The window starts mid-line unless the file is short; that first
    // partial line simply fails to parse.
    String::from_utf8_lossy(&tail)
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .find(|entry| !entry.cwd.is_empty())
        .map(|entry| PathBuf::from(entry.cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A projects tree holding one session's transcript, as Claude Code
    /// lays it out: a directory per launch path, a file per session.
    fn projects(project: &str, session: &str, lines: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join(project);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
        dir
    }

    #[test]
    fn the_newest_entry_answers_where_the_session_moved() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                r#"{"type":"user","cwd":"/repo"}"#,
                r#"{"type":"assistant","cwd":"/repo/.claude/worktrees/a"}"#,
                r#"{"type":"assistant","cwd":"/repo/.claude/worktrees/a/src"}"#,
            ],
        );
        let file = transcript(dir.path(), "s1").unwrap();
        assert_eq!(
            latest_cwd(&file),
            Some(PathBuf::from("/repo/.claude/worktrees/a/src"))
        );
    }

    #[test]
    fn entries_without_a_directory_fall_through_to_older_ones() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                r#"{"type":"assistant","cwd":"/repo/wt"}"#,
                // A summary entry carries no cwd, and a truncated
                // trailing write parses as nothing at all.
                r#"{"type":"summary","summary":"…"}"#,
                r#"{"type":"assistant","cw"#,
            ],
        );
        let file = transcript(dir.path(), "s1").unwrap();
        assert_eq!(latest_cwd(&file), Some(PathBuf::from("/repo/wt")));
    }

    #[test]
    fn a_quoted_directory_inside_a_tool_result_is_not_the_sessions() {
        // Only a top-level field counts: transcripts are full of text
        // that mentions other directories, this one doubly so.
        let dir = projects(
            "-repo",
            "s1",
            &[
                r#"{"type":"assistant","cwd":"/repo/wt"}"#,
                r#"{"type":"user","message":{"content":"{\"cwd\":\"/elsewhere\"}"}}"#,
            ],
        );
        let file = transcript(dir.path(), "s1").unwrap();
        assert_eq!(latest_cwd(&file), Some(PathBuf::from("/repo/wt")));
    }

    #[test]
    fn an_unknown_session_or_agent_resolves_to_nothing() {
        let dir = projects("-repo", "s1", &[r#"{"cwd":"/repo"}"#]);
        assert_eq!(transcript(dir.path(), "s2"), None);
        assert_eq!(transcript(Path::new("/no/such/root"), "s1"), None);
        // Agents drift has no record for keep their pane's directory.
        assert_eq!(working_dir("codex", "s1"), None);
    }
}
