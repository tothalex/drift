//! Where an agent is actually working.
//!
//! A multiplexer only knows the working directory of the pane's own
//! process, and that stays wherever the agent was launched. A Claude
//! session leaves it two ways: by moving into a linked worktree
//! (`.claude/worktrees/…`), and — just as often — by never moving at
//! all and reaching into another checkout by absolute path, `cd`-ing
//! afresh in every command because its shell's directory resets after
//! each one. Either way the review board would credit the work to the
//! checkout the pane started in. Claude Code's transcript records both
//! the session's own directory and every tool call it made, and herdr
//! reports the session id that finds it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::Value;

/// Transcript tail read per file. Entries are appended, so what the
/// session is doing now is at the end; the window only has to be wide
/// enough to hold the current turn's calls, and a single tool result
/// can be large.
const TAIL: u64 = 512 * 1024;

/// Subagent transcripts read per session, newest first. A session that
/// runs workflows leaves hundreds behind; only recent ones can still be
/// at work, and older entries are cut at the main window's start anyway.
const SUBAGENTS: usize = 16;

/// The tools that change files, and the input field naming the file.
/// Reads are not work: a session looks at other checkouts constantly.
const WRITE_TOOLS: &[(&str, &str)] = &[
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("Write", "file_path"),
    ("NotebookEdit", "notebook_path"),
];

/// Characters that end a path inside a shell command: separators,
/// quotes, and the `=` of a `--flag=/path`. Paths with spaces in them
/// are cut short, which still leaves the worktree root they start with.
const PATH_BREAKS: &[char] = &[
    '\'', '"', '`', ';', '|', '&', '<', '>', '(', ')', '=', ',', ':', '\\',
];

/// What a session's own record says about where it is working.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Activity {
    /// The session's own directory: the newest `cwd` it recorded.
    pub cwd: Option<PathBuf>,
    /// Its file-changing calls, a turn per prompt, newest turn first and
    /// each turn's calls newest first; turns that made none are left
    /// out. Subagents' calls count toward the turn they happened in.
    pub turns: Vec<Vec<Action>>,
}

/// One tool call that acts on files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A file-writing tool call, and the file it wrote.
    Wrote(PathBuf),
    /// A shell command: every absolute path it mentions, and where it
    /// ran — the directory its leading `cd` enters, else the session's.
    Ran {
        paths: Vec<PathBuf>,
        dir: Option<PathBuf>,
    },
}

/// What `agent`'s session has been doing, for the agents that keep a
/// readable record of it. `None` for anything drift cannot resolve —
/// the caller then keeps the pane's own directory, which is right for
/// every agent that never left it.
pub(super) fn activity(agent: &str, session: &str) -> Option<Activity> {
    match agent {
        "claude" => read_activity(
            &transcript(&projects_root()?, session)?,
            std::env::home_dir().as_deref(),
        ),
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

/// The activity in a transcript and its subagents' transcripts, which
/// sit in a directory named after the session file.
fn read_activity(path: &Path, home: Option<&Path>) -> Option<Activity> {
    let main = parse(&tail(path)?, home);
    let since = main
        .start
        .as_deref()
        .and_then(unix_seconds)
        .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    let mut actions = main.actions;
    for file in subagent_files(&path.with_extension(""), since) {
        let Some(text) = tail(&file) else { continue };
        actions.extend(parse(&text, home).actions.into_iter().filter(|(at, _)| {
            main.start
                .as_deref()
                .is_none_or(|start| at.as_str() >= start)
        }));
    }
    Some(Activity {
        cwd: main.cwd,
        turns: turns(&main.prompts, actions),
    })
}

/// The last [`TAIL`] bytes of a file.
fn tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    Some(String::from_utf8_lossy(&tail).into_owned())
}

/// The newest [`SUBAGENTS`] transcripts under a session's directory
/// written to since `since` — older ones hold nothing the window shows,
/// and skipping them unread is most of what keeps this cheap. Plain
/// subagents sit in `subagents/`; workflows nest theirs a run directory
/// deeper (`subagents/workflows/<run>/`), beside a journal that is not
/// an agent's.
fn subagent_files(session_dir: &Path, since: Option<SystemTime>) -> Vec<PathBuf> {
    fn collect(dir: &Path, depth: usize, out: &mut Vec<(SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 2 {
                    collect(&path, depth + 1, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("agent-"))
                && let Ok(modified) = meta.modified()
            {
                out.push((modified, path));
            }
        }
    }
    let mut found = Vec::new();
    collect(&session_dir.join("subagents"), 0, &mut found);
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found
        .into_iter()
        .take(SUBAGENTS)
        .filter(|(modified, _)| since.is_none_or(|since| *modified >= since))
        .map(|(_, path)| path)
        .collect()
}

/// Seconds since the epoch for a transcript timestamp
/// ("2026-10-01T10:00:01.000Z"), sub-second part dropped.
fn unix_seconds(stamp: &str) -> Option<u64> {
    let field = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    // Days from the civil date, counting years from March so the leap
    // day falls last (Howard Hinnant's `days_from_civil`).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// One transcript line. `message` stays untyped: its shape varies by
/// entry type, and a line that fails to parse loses its `cwd` too.
#[derive(Deserialize)]
struct Entry {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default, rename = "isMeta")]
    meta: bool,
    #[serde(default, rename = "isSidechain")]
    sidechain: bool,
    #[serde(default)]
    message: Value,
}

/// What one transcript window holds.
#[derive(Default)]
struct Parsed {
    /// The newest `cwd`.
    cwd: Option<PathBuf>,
    /// The window's first timestamp: subagent calls older than this
    /// belong to turns the window no longer shows.
    start: Option<String>,
    /// When each prompt arrived, oldest first.
    prompts: Vec<String>,
    /// File-changing calls with their timestamps, oldest first.
    actions: Vec<(String, Action)>,
}

/// Read a transcript window. Lines are parsed whole rather than scanned
/// for keys: a tool result can quote anything, and only a top-level
/// field is the session's own. The window starts mid-line unless the
/// file is short; that first partial line simply fails to parse.
fn parse(text: &str, home: Option<&Path>) -> Parsed {
    let mut parsed = Parsed::default();
    let mut at = String::new();
    for entry in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
    {
        if !entry.timestamp.is_empty() {
            at = entry.timestamp;
            parsed.start.get_or_insert_with(|| at.clone());
        }
        if !entry.cwd.is_empty() {
            parsed.cwd = Some(PathBuf::from(&entry.cwd));
        }
        let content = entry.message.get("content");
        match entry.kind.as_str() {
            "user" if !entry.meta && !entry.sidechain && is_prompt(content) => {
                parsed.prompts.push(at.clone());
            }
            "assistant" => {
                let blocks = content.and_then(Value::as_array).into_iter().flatten();
                for block in blocks {
                    if let Some(action) = action(block, Path::new(&entry.cwd), home) {
                        parsed.actions.push((at.clone(), action));
                    }
                }
            }
            _ => {}
        }
    }
    parsed
}

/// Is a user entry's content a prompt — typed text, or a background
/// task's notification — rather than tool results handed back?
fn is_prompt(content: Option<&Value>) -> bool {
    let text = |block: &Value| block.get("type").and_then(Value::as_str) == Some("text");
    match content {
        Some(Value::String(_)) => true,
        Some(Value::Array(blocks)) => blocks.iter().any(text),
        _ => false,
    }
}

/// The action a content block records, if it is a file-changing call.
fn action(block: &Value, cwd: &Path, home: Option<&Path>) -> Option<Action> {
    if block.get("type")?.as_str()? != "tool_use" {
        return None;
    }
    let name = block.get("name")?.as_str()?;
    let input = block.get("input")?;
    if name == "Bash" {
        return Some(command_action(input.get("command")?.as_str()?, cwd, home));
    }
    let (_, field) = WRITE_TOOLS.iter().find(|(tool, _)| *tool == name)?;
    Some(Action::Wrote(cwd.join(input.get(*field)?.as_str()?)))
}

/// A shell command as an action. It is not parsed as shell — the board
/// only needs to know which worktrees it touches, and a worktree root
/// is unmistakable wherever in the text it appears.
fn command_action(command: &str, cwd: &Path, home: Option<&Path>) -> Action {
    let paths = command
        .split(|c: char| c.is_whitespace() || PATH_BREAKS.contains(&c))
        .map(|word| expand(word, home))
        .filter(|path| path.is_absolute())
        .collect();
    let dir = match leading_cd(command) {
        Some(dir) => Some(cwd.join(expand(dir, home))),
        None => (!cwd.as_os_str().is_empty()).then(|| cwd.to_path_buf()),
    };
    Action::Ran { paths, dir }
}

/// The directory a command `cd`s into before anything else, as written.
fn leading_cd(command: &str) -> Option<&str> {
    let rest = command.trim_start().strip_prefix("cd")?;
    // `cdk deploy` is not a `cd`.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let dir = match rest.chars().next()? {
        quote @ ('"' | '\'') => rest[1..].split(quote).next()?,
        _ => rest
            .split(|c: char| c.is_whitespace() || ";&|".contains(c))
            .next()?,
    };
    (!dir.is_empty() && dir != "-").then_some(dir)
}

/// A word as a path, with the home directory spelled out.
fn expand(word: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        for prefix in ["~", "$HOME", "${HOME}"] {
            match word.strip_prefix(prefix) {
                Some("") => return home.to_path_buf(),
                Some(rest) if rest.starts_with('/') => return home.join(&rest[1..]),
                _ => {}
            }
        }
    }
    PathBuf::from(word)
}

/// Group actions into the turns their timestamps fall in. Timestamps
/// compare as text: Claude Code writes them all as fixed-width UTC ISO
/// 8601, so text order is time order.
fn turns(prompts: &[String], mut actions: Vec<(String, Action)>) -> Vec<Vec<Action>> {
    actions.sort_by(|a, b| a.0.cmp(&b.0));
    let mut turns = vec![Vec::new(); prompts.len() + 1];
    for (at, action) in actions {
        turns[prompts.partition_point(|prompt| prompt.as_str() <= at.as_str())].push(action);
    }
    turns
        .into_iter()
        .rev()
        .filter(|turn| !turn.is_empty())
        .map(|mut turn| {
            turn.reverse();
            turn
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A projects tree holding one session's transcript, as Claude Code
    /// lays it out: a directory per launch path, a file per session.
    fn projects(project: &str, session: &str, lines: &[String]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join(project);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
        dir
    }

    fn activity_of(dir: &tempfile::TempDir, session: &str) -> Activity {
        read_activity(&transcript(dir.path(), session).unwrap(), None).unwrap()
    }

    /// The nth second of a test morning, as Claude Code stamps entries.
    /// Long past, so files written by the test are always newer.
    fn t(second: u32) -> String {
        format!("2000-01-01T10:00:{second:02}.000Z")
    }

    fn prompt(at: u32) -> String {
        json!({"type":"user","timestamp":t(at),"cwd":"/repo",
            "message":{"role":"user","content":"do the thing"}})
        .to_string()
    }

    fn call(at: u32, cwd: &str, tool: &str, input: Value) -> String {
        json!({"type":"assistant","timestamp":t(at),"cwd":cwd,
            "message":{"content":[{"type":"tool_use","name":tool,"input":input}]}})
        .to_string()
    }

    fn wrote(path: &str) -> Action {
        Action::Wrote(PathBuf::from(path))
    }

    #[test]
    fn the_newest_entry_answers_where_the_session_moved() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                r#"{"type":"user","cwd":"/repo"}"#.to_string(),
                r#"{"type":"assistant","cwd":"/repo/.claude/worktrees/a"}"#.to_string(),
                r#"{"type":"assistant","cwd":"/repo/.claude/worktrees/a/src"}"#.to_string(),
            ],
        );
        assert_eq!(
            activity_of(&dir, "s1").cwd,
            Some(PathBuf::from("/repo/.claude/worktrees/a/src"))
        );
    }

    #[test]
    fn entries_without_a_directory_fall_through_to_older_ones() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                r#"{"type":"assistant","cwd":"/repo/wt"}"#.to_string(),
                // A summary entry carries no cwd, and a truncated
                // trailing write parses as nothing at all.
                r#"{"type":"summary","summary":"…"}"#.to_string(),
                r#"{"type":"assistant","cw"#.to_string(),
            ],
        );
        assert_eq!(activity_of(&dir, "s1").cwd, Some(PathBuf::from("/repo/wt")));
    }

    #[test]
    fn a_quoted_directory_inside_a_tool_result_is_not_the_sessions() {
        // Only a top-level field counts: transcripts are full of text
        // that mentions other directories, this one doubly so.
        let dir = projects(
            "-repo",
            "s1",
            &[
                r#"{"type":"assistant","cwd":"/repo/wt"}"#.to_string(),
                r#"{"type":"user","message":{"content":"{\"cwd\":\"/elsewhere\"}"}}"#.to_string(),
            ],
        );
        assert_eq!(activity_of(&dir, "s1").cwd, Some(PathBuf::from("/repo/wt")));
    }

    #[test]
    fn an_unknown_session_or_agent_resolves_to_nothing() {
        let dir = projects("-repo", "s1", &[r#"{"cwd":"/repo"}"#.to_string()]);
        assert_eq!(transcript(dir.path(), "s2"), None);
        assert_eq!(transcript(Path::new("/no/such/root"), "s1"), None);
        // Agents drift has no record for keep their pane's directory.
        assert_eq!(activity("codex", "s1"), None);
    }

    #[test]
    fn calls_group_into_turns_at_each_prompt_newest_first() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                prompt(1),
                call(2, "/repo", "Edit", json!({"file_path":"/repo/wt/a.rs"})),
                // Tool results and meta entries are user entries too,
                // but neither starts a turn.
                json!({"type":"user","timestamp":t(3),"message":{"content":[
                    {"type":"tool_result","content":"ok"}]}})
                .to_string(),
                json!({"type":"user","isMeta":true,"timestamp":t(4),
                    "message":{"content":"<local-command-caveat>"}})
                .to_string(),
                call(5, "/repo", "Write", json!({"file_path":"/repo/wt/b.rs"})),
                prompt(6),
                call(7, "/repo", "Write", json!({"file_path":"/repo/c.rs"})),
            ],
        );
        assert_eq!(
            activity_of(&dir, "s1").turns,
            vec![
                vec![wrote("/repo/c.rs")],
                vec![wrote("/repo/wt/b.rs"), wrote("/repo/wt/a.rs")],
            ]
        );
    }

    #[test]
    fn reads_are_not_work() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                prompt(1),
                call(2, "/repo", "Read", json!({"file_path":"/repo/wt/a.rs"})),
                call(3, "/repo", "Grep", json!({"path":"/repo/wt"})),
            ],
        );
        assert_eq!(activity_of(&dir, "s1").turns, Vec::<Vec<Action>>::new());
    }

    #[test]
    fn a_command_names_its_paths_and_runs_where_it_cds() {
        let cwd = Path::new("/repo");
        let home = Some(Path::new("/home/me"));
        let ran = |paths: &[&str], dir: &str| Action::Ran {
            paths: paths.iter().map(PathBuf::from).collect(),
            dir: Some(PathBuf::from(dir)),
        };
        let action = |command| command_action(command, cwd, home);

        // The shape a session reaching into another checkout leaves.
        assert_eq!(
            action("cd /wt && sed -i '' 's/a/b/' \"/wt/x.rs\" --manifest-path=/m/Cargo.toml"),
            ran(&["/wt", "/wt/x.rs", "/m/Cargo.toml"], "/wt")
        );
        // Nothing named: it ran where the session stands.
        assert_eq!(action("cargo test 2>&1 | tail -5"), ran(&[], "/repo"));
        assert_eq!(action("cd src && make"), ran(&[], "/repo/src"));
        assert_eq!(
            action("cd ~/code/x; ls $HOME/notes"),
            ran(&["/home/me/code/x", "/home/me/notes"], "/home/me/code/x")
        );
        assert_eq!(action("cd \"/a b\" && ls"), ran(&["/a"], "/a b"));
        assert_eq!(action("cdk deploy"), ran(&[], "/repo"));
    }

    #[test]
    fn subagent_calls_join_the_turn_they_happened_in() {
        let dir = projects(
            "-repo",
            "s1",
            &[
                prompt(1),
                call(2, "/repo", "Write", json!({"file_path":"/repo/a.rs"})),
                prompt(5),
            ],
        );
        let session = dir.path().join("-repo/s1/subagents");
        let workflow = session.join("workflows/wf_1");
        std::fs::create_dir_all(&workflow).unwrap();
        let older = json!({"type":"assistant","isSidechain":true,"timestamp":"2000-01-01T09:00:00.000Z",
            "message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/old.rs"}}]}});
        std::fs::write(
            session.join("agent-a.jsonl"),
            [
                // Before the main window starts: a turn it cannot show.
                older.to_string(),
                call(3, "/repo/wt", "Edit", json!({"file_path":"/repo/wt/b.rs"})),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            workflow.join("agent-b.jsonl"),
            call(6, "/repo/wt2", "Bash", json!({"command":"cargo test"})),
        )
        .unwrap();
        assert_eq!(
            activity_of(&dir, "s1").turns,
            vec![
                vec![Action::Ran {
                    paths: Vec::new(),
                    dir: Some(PathBuf::from("/repo/wt2")),
                }],
                vec![wrote("/repo/wt/b.rs"), wrote("/repo/a.rs")],
            ]
        );
    }

    #[test]
    fn a_subagent_untouched_since_the_window_began_is_not_read() {
        // Its entries claim the current turn, but a file last written
        // before the window starts cannot hold them; only its mtime is
        // looked at.
        let dir = projects(
            "-repo",
            "s1",
            &[
                prompt(1),
                call(2, "/repo", "Write", json!({"file_path":"/repo/a.rs"})),
            ],
        );
        let session = dir.path().join("-repo/s1/subagents");
        std::fs::create_dir_all(&session).unwrap();
        let stale = session.join("agent-a.jsonl");
        std::fs::write(
            &stale,
            call(3, "/wt", "Write", json!({"file_path":"/wt/b.rs"})),
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000))
            .unwrap();
        assert_eq!(
            activity_of(&dir, "s1").turns,
            vec![vec![wrote("/repo/a.rs")]]
        );
    }

    #[test]
    fn timestamps_convert_to_epoch_seconds() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(unix_seconds("2000-03-01T00:00:00.000Z"), Some(951_868_800));
        assert_eq!(
            unix_seconds("2026-10-01T10:00:01.000Z"),
            Some(1_790_848_801)
        );
        assert_eq!(unix_seconds("not a time"), None);
    }
}
