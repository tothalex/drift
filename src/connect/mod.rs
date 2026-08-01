//! Sending code to an AI agent pane: the current line or visual
//! selection, wrapped in a typed prompt, delivered to a running
//! claude/codex/… CLI in a sibling multiplexer pane.
//!
//! drift never talks to a model itself — it hands the prompt to whatever
//! agent the user already has open, through the multiplexer's own CLI.
//!
//! Each way of delivering a prompt is one [`Bridge`] implementation in
//! its own submodule. To add one (tmux, wezterm, an HTTP API…):
//! implement the trait in a new file and register it in [`BACKENDS`]
//! with a [`Backend`] variant — config parsing, auto-detection, and the
//! target picker all follow from that table.

mod cmux;
mod herdr;
mod tmux;

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

/// Process names that mark a pane as an agent, for the backends that
/// have to recognize one from the process it runs. Matched against a
/// process basename.
pub(crate) const AGENT_NAMES: &[&str] = &[
    "claude",
    "codex",
    "aider",
    "gemini",
    "goose",
    "opencode",
    "cursor-agent",
];

/// Default prompt template. `{input}` is the typed instruction; the
/// other placeholders describe the selection.
pub const TEMPLATE_DEFAULT: &str = "{input}\n\n{file}:{lines}\n```\n{code}\n```";

/// The `[agent]` config section, decoupled from the config crate types.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub backend: Backend,
    /// Pinned target: an agent name ("claude") or pane id; `None` means
    /// auto-pick (the closest agent, or a picker).
    pub target: Option<String>,
    /// Press enter in the agent pane after inserting the prompt.
    pub submit: bool,
    pub template: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            backend: Backend::Auto,
            target: None,
            submit: true,
            template: TEMPLATE_DEFAULT.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Use whichever multiplexer drift is running inside.
    Auto,
    Off,
    /// A specific backend, even when drift runs outside it (a
    /// multiplexer's CLI reaches the default session regardless —
    /// except cmux, whose socket only answers its own surfaces unless
    /// its `socketControlMode` setting is widened).
    Herdr,
    Tmux,
    Cmux,
}

/// One registered backend: its config name, how to tell drift is
/// running inside it, and how to build its bridge. [`Backend::parse`]
/// and [`detect`] are both driven by this table — a new backend is one
/// row here plus its submodule.
struct BackendDef {
    /// The `agent.backend` config value.
    name: &'static str,
    backend: Backend,
    /// The process that owns this backend's panes: whichever one
    /// [`detect`] meets first walking up from drift is the session
    /// drift is a pane of.
    host_process: &'static str,
    /// Env var the multiplexer sets in its panes — the fallback probe
    /// for when the process table can't be read.
    inside_env: &'static str,
    make: fn() -> Box<dyn Bridge>,
}

/// Order settles only the fallback probe, since a marker is inherited
/// by whatever a pane starts: a multiplexer running inside cmux still
/// sees `CMUX_SURFACE_ID`, so cmux ranks below both, or drift would aim
/// at the cmux surface holding the whole session instead of the pane
/// beside it. Between the two multiplexers herdr wins: its native agent
/// tracking is the richer bridge wherever both would match.
const BACKENDS: &[BackendDef] = &[
    BackendDef {
        name: "herdr",
        backend: Backend::Herdr,
        host_process: "herdr",
        inside_env: herdr::INSIDE_ENV,
        make: herdr::make,
    },
    BackendDef {
        name: "tmux",
        backend: Backend::Tmux,
        host_process: "tmux",
        inside_env: tmux::INSIDE_ENV,
        make: tmux::make,
    },
    BackendDef {
        name: "cmux",
        backend: Backend::Cmux,
        host_process: "cmux",
        inside_env: cmux::INSIDE_ENV,
        make: cmux::make,
    },
];

impl Backend {
    /// Parse the config's `agent.backend` value ("auto" when absent).
    pub fn parse(name: Option<&str>) -> Result<Backend> {
        match name {
            None | Some("auto") => Ok(Backend::Auto),
            Some("off") => Ok(Backend::Off),
            Some(other) => BACKENDS
                .iter()
                .find(|def| def.name == other)
                .map(|def| def.backend)
                .ok_or_else(|| {
                    anyhow!(
                        "agent.backend must be \"auto\", \"off\", or one of: {}; not '{other}'",
                        BACKENDS
                            .iter()
                            .map(|def| format!("\"{}\"", def.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
        }
    }
}

/// The bridge for the session drift runs in: `auto` walks drift's own
/// process ancestry, a named backend is built unconditionally, `off`
/// disables the feature.
pub fn detect(config: &AgentConfig) -> Option<Box<dyn Bridge>> {
    match config.backend {
        Backend::Off => None,
        Backend::Auto => host_backend()
            .or_else(marked_backend)
            .map(|def| (def.make)()),
        forced => BACKENDS
            .iter()
            .find(|def| def.backend == forced)
            .map(|def| (def.make)()),
    }
}

/// The session drift is a pane of, from the process that owns it.
///
/// An environment marker outlives the pane it names: a terminal opened
/// from a herdr pane inherits that pane's `HERDR_PANE_ID`, and every
/// pane inside it would claim to *be* that one pane — resolving to a
/// live pane elsewhere, so nothing errors and the prompt just goes to
/// the wrong place. A parent is what it is.
fn host_backend() -> Option<&'static BackendDef> {
    let processes = run_cli("ps", &["-A", "-o", "pid=,ppid=,comm="]).ok()?;
    backend_of(std::process::id(), &parse_ancestry(&processes))
}

/// The nearest host among a pid's ancestors. Nearest, so a tmux session
/// started inside a herdr pane resolves to tmux — the panes beside
/// drift are tmux's, not herdr's.
fn backend_of(pid: u32, tree: &HashMap<u32, (u32, &str)>) -> Option<&'static BackendDef> {
    let mut at = pid;
    // A pane's shell, an agent and a few forks sit under the host; the
    // bound is only so a malformed table cannot spin.
    for _ in 0..64 {
        let (parent, name) = *tree.get(&at)?;
        if let Some(def) = BACKENDS.iter().find(|def| def.host_process == name) {
            return Some(def);
        }
        at = parent;
    }
    None
}

/// pid → (parent, process basename), from `ps -A -o pid=,ppid=,comm=`.
fn parse_ancestry(processes: &str) -> HashMap<u32, (u32, &str)> {
    processes
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let command = fields.next()?;
            Some((pid, (parent, command.rsplit('/').next().unwrap_or(command))))
        })
        .collect()
}

/// The environment marker, for when the process table is unreadable.
fn marked_backend() -> Option<&'static BackendDef> {
    BACKENDS
        .iter()
        .find(|def| std::env::var_os(def.inside_env).is_some())
}

/// One way of listing agent targets and typing a prompt into one.
/// Implementations shell out to their multiplexer's CLI and block
/// briefly; calls run inline from the key handler.
pub trait Bridge {
    /// Name for notices: "herdr".
    fn label(&self) -> &'static str;
    /// Agent panes a prompt can go to, excluding drift's own pane,
    /// closest first (same tab, same workspace, elsewhere).
    fn targets(&self) -> Result<Vec<AgentTarget>>;
    /// Insert `text` into the target's input; `submit` presses enter.
    fn send(&self, target_id: &str, text: &str, submit: bool) -> Result<()>;
}

/// Where an agent pane sits relative to drift's own pane. Ordered by
/// closeness: an agent split in the same tab is visibly next to drift
/// and the natural default target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// A split in drift's tab — on screen right now, next to drift.
    SameTab,
    SameWorkspace,
    Elsewhere,
}

/// One agent a prompt can be sent to.
#[derive(Debug, Clone)]
pub struct AgentTarget {
    /// Agent label as the backend reports it ("claude", "codex").
    pub name: String,
    /// Backend-side id used for sending ("w13:p1"); never shown, ids
    /// mean nothing to someone looking at panes.
    pub id: String,
    /// "idle" / "working" / "blocked", when known.
    pub status: String,
    pub place: Place,
    /// Where the target is, in human terms: "this tab", the workspace
    /// and tab names ("drift:2"), or the agent's directory.
    pub where_label: String,
}

impl AgentTarget {
    /// Picker row / notice / compose-title label:
    /// "claude · this tab · idle" — the status segment only when the
    /// backend knows one (tmux doesn't).
    pub fn label(&self) -> String {
        let mut label = format!("{} · {}", self.name, self.where_label);
        if !self.status.is_empty() {
            label.push_str(" · ");
            label.push_str(&self.status);
        }
        label
    }
}

/// A path for a target label: the home directory shortened to `~`.
pub(crate) fn short_path(path: &str) -> String {
    std::env::home_dir()
        .and_then(|home| {
            let home = home.to_str()?;
            Some(format!("~{}", path.strip_prefix(home)?))
        })
        .unwrap_or_else(|| path.to_string())
}

/// What the prompt is about: the selection's place and text, captured
/// when the send is requested (the view may change before it's typed).
#[derive(Debug, Clone)]
pub struct SendContext {
    /// Absolute path — the agent may be running anywhere, so the
    /// default template names the full location.
    pub file: String,
    /// Repo-relative path, for the `{relfile}` placeholder.
    pub rel: String,
    /// New-side line numbers of the selection.
    pub start: u32,
    pub end: u32,
    pub code: String,
}

impl SendContext {
    /// "12" or "12-18", for the `{lines}` placeholder.
    fn lines(&self) -> String {
        if self.start == self.end {
            self.start.to_string()
        } else {
            format!("{}-{}", self.start, self.end)
        }
    }
}

/// Fill the prompt template. Placeholders the template doesn't mention
/// are simply not used.
pub fn format_prompt(template: &str, input: &str, ctx: &SendContext) -> String {
    template
        .replace("{input}", input)
        .replace("{file}", &ctx.file)
        .replace("{relfile}", &ctx.rel)
        .replace("{lines}", &ctx.lines())
        .replace("{start}", &ctx.start.to_string())
        .replace("{end}", &ctx.end.to_string())
        .replace("{code}", &ctx.code)
        .trim()
        .to_string()
}

/// Run a backend's CLI non-interactively and return stdout. A missing
/// binary and a non-zero exit (no server, unknown pane) both surface as
/// one readable line.
pub(crate) fn run_cli(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => anyhow!("{program} is not installed"),
            _ => anyhow!("cannot run {program}: {err}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("failed");
        bail!("{program}: {line}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_parse_and_unknown_ones_list_the_options() {
        assert_eq!(Backend::parse(None).unwrap(), Backend::Auto);
        assert_eq!(Backend::parse(Some("auto")).unwrap(), Backend::Auto);
        assert_eq!(Backend::parse(Some("off")).unwrap(), Backend::Off);
        assert_eq!(Backend::parse(Some("herdr")).unwrap(), Backend::Herdr);
        assert_eq!(Backend::parse(Some("tmux")).unwrap(), Backend::Tmux);
        assert_eq!(Backend::parse(Some("cmux")).unwrap(), Backend::Cmux);
        let err = Backend::parse(Some("zellij")).unwrap_err().to_string();
        assert!(err.contains("\"herdr\""), "{err}");
        assert!(err.contains("\"tmux\""), "{err}");
        assert!(err.contains("\"cmux\""), "{err}");
        assert!(err.contains("'zellij'"), "{err}");
    }

    #[test]
    fn labels_omit_an_unknown_status() {
        let target = AgentTarget {
            name: "claude".to_string(),
            id: "%1".to_string(),
            status: String::new(),
            place: Place::SameTab,
            where_label: "this tab".to_string(),
        };
        assert_eq!(target.label(), "claude · this tab");
    }

    /// drift, its shell, and the hosts above them. herdr and the tmux
    /// server both detach, so each sits directly under launchd.
    const PROCESSES: &str = "\
    1     0 /sbin/launchd
  786     1 /opt/homebrew/bin/herdr
  900     1 tmux
 1000     1 /Applications/cmux.app/Contents/MacOS/cmux
 2000   786 -fish
 2001  2000 drift
 3000   900 -fish
 3001  3000 drift
 4000  1000 -fish
 4001  4000 drift
 5000     1 -fish
 5001  5000 drift";

    #[test]
    fn the_host_is_the_nearest_one_above_drift() {
        let tree = parse_ancestry(PROCESSES);
        let backend = |pid| backend_of(pid, &tree).map(|def| def.backend);
        assert_eq!(backend(2001), Some(Backend::Herdr));
        assert_eq!(backend(4001), Some(Backend::Cmux));
        // No host above it at all — drift in a bare terminal.
        assert_eq!(backend(5001), None);
        // An unknown pid can't be placed.
        assert_eq!(backend(9999), None);
    }

    /// A tmux session started inside a herdr pane: the panes beside
    /// drift are tmux's, even though `HERDR_PANE_ID` is still in the
    /// environment it inherited.
    #[test]
    fn a_nested_session_beats_the_one_it_runs_in() {
        let nested = format!("{PROCESSES}\n   900   2000 tmux");
        let tree = parse_ancestry(&nested);
        assert_eq!(
            backend_of(3001, &tree).map(|def| def.backend),
            Some(Backend::Tmux)
        );
    }

    #[test]
    fn a_cyclic_process_table_resolves_to_no_host() {
        let tree = parse_ancestry("100 200 -fish\n200 100 -fish");
        assert!(backend_of(100, &tree).is_none());
    }

    #[test]
    fn registered_backends_are_unique() {
        for (nth, def) in BACKENDS.iter().enumerate() {
            for other in &BACKENDS[nth + 1..] {
                assert_ne!(def.name, other.name);
                assert_ne!(def.backend, other.backend);
                assert_ne!(def.host_process, other.host_process);
            }
        }
    }

    #[test]
    fn format_prompt_fills_placeholders() {
        let ctx = SendContext {
            file: "/repo/src/app/mod.rs".to_string(),
            rel: "src/app/mod.rs".to_string(),
            start: 12,
            end: 18,
            code: "fn demo() {}".to_string(),
        };
        let text = format_prompt(TEMPLATE_DEFAULT, "explain this", &ctx);
        assert_eq!(
            text,
            "explain this\n\n/repo/src/app/mod.rs:12-18\n```\nfn demo() {}\n```"
        );
        // A single line renders as one number, and unused placeholders
        // are no error.
        let one = SendContext {
            start: 7,
            end: 7,
            ..ctx
        };
        assert_eq!(
            format_prompt("{relfile}:{lines}", "", &one),
            "src/app/mod.rs:7"
        );
        assert_eq!(format_prompt("{start}..{end}", "", &one), "7..7");
    }
}
