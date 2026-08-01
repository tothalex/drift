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

mod herdr;

use anyhow::{Result, anyhow, bail};

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
    /// multiplexer's CLI reaches the default session regardless).
    Herdr,
}

/// One registered backend: its config name, how to tell drift is
/// running inside it, and how to build its bridge. [`Backend::parse`]
/// and [`detect`] are both driven by this table — a new backend is one
/// row here plus its submodule.
struct BackendDef {
    /// The `agent.backend` config value.
    name: &'static str,
    backend: Backend,
    /// Env var the multiplexer sets in its panes — the auto-detect probe.
    inside_env: &'static str,
    make: fn() -> Box<dyn Bridge>,
}

const BACKENDS: &[BackendDef] = &[BackendDef {
    name: "herdr",
    backend: Backend::Herdr,
    inside_env: herdr::INSIDE_ENV,
    make: herdr::make,
}];

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

/// The bridge for the environment drift runs in: `auto` probes each
/// registered backend's environment marker, a named backend is built
/// unconditionally, `off` disables the feature.
pub fn detect(config: &AgentConfig) -> Option<Box<dyn Bridge>> {
    match config.backend {
        Backend::Off => None,
        Backend::Auto => BACKENDS
            .iter()
            .find(|def| std::env::var_os(def.inside_env).is_some())
            .map(|def| (def.make)()),
        forced => BACKENDS
            .iter()
            .find(|def| def.backend == forced)
            .map(|def| (def.make)()),
    }
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
    /// "claude · this tab · idle".
    pub fn label(&self) -> String {
        format!("{} · {} · {}", self.name, self.where_label, self.status)
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
        let err = Backend::parse(Some("tmux")).unwrap_err().to_string();
        assert!(err.contains("\"herdr\""), "{err}");
        assert!(err.contains("'tmux'"), "{err}");
    }

    #[test]
    fn registered_backends_are_unique() {
        for (nth, def) in BACKENDS.iter().enumerate() {
            for other in &BACKENDS[nth + 1..] {
                assert_ne!(def.name, other.name);
                assert_ne!(def.backend, other.backend);
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
