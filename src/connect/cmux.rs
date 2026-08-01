//! The cmux backend (cmux.com): a macOS terminal built around AI coding
//! agents. `cmux tree` gives the window/workspace nesting and `cmux top`
//! says which surface owns each process — ownership worth taking from
//! cmux rather than inferring, since a surface's reported tty can go
//! stale after a workspace closes and two surfaces claiming one tty
//! would make a shell prompt look like its neighbour's agent.
//!
//! The names come from `ps` instead, because cmux reports the resolved
//! executable: a versioned Claude Code install shows up as "2.1.220",
//! where `ps` still reports the `claude` it was launched as. Either
//! source naming a known agent is enough.
//!
//! The prompt goes in through the `terminal.paste` socket method, which
//! is the only cmux entry point that keeps a multi-line prompt whole:
//! `cmux send`, `surface.send_text` and `terminal.input` all turn a
//! newline into enter, so an agent receives one message per line. The
//! cost is that paste always submits — cmux offers no way to stage a
//! prompt without sending it, so `submit = false` needs another
//! backend.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{AGENT_NAMES, AgentTarget, Bridge, Place, run_cli};

/// cmux sets this in every surface — the "you are inside cmux" marker.
pub(super) const INSIDE_ENV: &str = "CMUX_SURFACE_ID";

pub(super) fn make() -> Box<dyn Bridge> {
    Box::new(Cmux {
        cli: cli_path(),
        own_surface: std::env::var("CMUX_SURFACE_ID").ok(),
        own_workspace: std::env::var("CMUX_WORKSPACE_ID").ok(),
    })
}

/// cmux only puts its CLI on `PATH` when the user symlinks it, but every
/// surface carries the bundled path; outside one, hope for the symlink.
fn cli_path() -> String {
    std::env::var("CMUX_BUNDLED_CLI_PATH")
        .ok()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "cmux".to_string())
}

struct Cmux {
    cli: String,
    /// Drift's own surface, excluded from the target list.
    own_surface: Option<String>,
    /// Drift's own workspace — a cmux workspace is the tab the user
    /// sees, so its surfaces are the splits sitting next to drift.
    own_workspace: Option<String>,
}

impl Bridge for Cmux {
    fn label(&self) -> &'static str {
        "cmux"
    }

    fn targets(&self) -> Result<Vec<AgentTarget>> {
        let tree = run_cli(
            &self.cli,
            &["tree", "--all", "--json", "--id-format", "both"],
        )?;
        let top = run_cli(
            &self.cli,
            &["top", "--all", "--processes", "--format", "tsv"],
        )?;
        let processes = run_cli("ps", &["-A", "-o", "pid=,comm="])?;
        resolve_targets(&tree, &top, &processes, self)
    }

    fn send(&self, target_id: &str, text: &str, submit: bool) -> Result<()> {
        if !submit {
            bail!("cmux always submits a pasted prompt; agent.submit = false needs herdr or tmux");
        }
        let params = serde_json::json!({ "surface_id": target_id, "text": text }).to_string();
        run_cli(&self.cli, &["rpc", "terminal.paste", &params])?;
        Ok(())
    }
}

/// `cmux tree --all --json --id-format both`: windows hold workspaces
/// hold panes hold surfaces.
#[derive(Deserialize)]
struct Tree {
    #[serde(default)]
    windows: Vec<TreeWindow>,
}

#[derive(Deserialize)]
struct TreeWindow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    workspaces: Vec<TreeWorkspace>,
}

#[derive(Deserialize)]
struct TreeWorkspace {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    panes: Vec<TreePane>,
}

#[derive(Deserialize)]
struct TreePane {
    #[serde(default)]
    surfaces: Vec<TreeSurface>,
}

#[derive(Deserialize)]
struct TreeSurface {
    #[serde(default)]
    id: String,
    /// `surface:N` — how `cmux top` names the surface a process runs in.
    #[serde(rename = "ref", default)]
    surface_ref: String,
    #[serde(default)]
    title: Option<String>,
}

fn resolve_targets(tree: &str, top: &str, processes: &str, own: &Cmux) -> Result<Vec<AgentTarget>> {
    let tree: Tree = serde_json::from_str(tree.trim()).context("unexpected `cmux tree` output")?;
    let agents = agents_by_surface(top, processes);
    // Drift's window is whichever holds its workspace; outside cmux
    // there is none and every target is elsewhere.
    let own_window = tree
        .windows
        .iter()
        .find(|window| {
            window
                .workspaces
                .iter()
                .any(|workspace| Some(workspace.id.as_str()) == own.own_workspace.as_deref())
        })
        .map(|window| window.id.as_str());

    let mut targets: Vec<AgentTarget> = Vec::new();
    for window in &tree.windows {
        for workspace in &window.workspaces {
            for surface in workspace.panes.iter().flat_map(|pane| &pane.surfaces) {
                if Some(surface.id.as_str()) == own.own_surface.as_deref() {
                    continue;
                }
                let Some(agent) = agents.get(surface.surface_ref.as_str()) else {
                    continue;
                };
                let place = if Some(workspace.id.as_str()) == own.own_workspace.as_deref() {
                    Place::SameTab
                } else if Some(window.id.as_str()) == own_window {
                    Place::SameWorkspace
                } else {
                    Place::Elsewhere
                };
                let where_label = match place {
                    Place::SameTab => "this tab".to_string(),
                    _ => place_label(workspace, surface),
                };
                targets.push(AgentTarget {
                    name: (*agent).to_string(),
                    id: surface.id.clone(),
                    // cmux tracks notifications, not idle/working state.
                    status: String::new(),
                    place,
                    where_label,
                });
            }
        }
    }
    targets.sort_by_key(|target| target.place);
    Ok(targets)
}

/// The workspace title is what cmux shows in its sidebar, so it is the
/// name the user would give the place.
fn place_label(workspace: &TreeWorkspace, surface: &TreeSurface) -> String {
    [workspace.title.as_deref(), surface.title.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|title| !title.is_empty())
        .unwrap_or(&surface.id)
        .to_string()
}

/// Surface ref → the first known agent running in it. `cmux top
/// --processes --format tsv` rows are cpu, memory, count, kind, id,
/// parent, title; a `process` row's parent is either the surface that
/// owns it or the pid it was forked from.
fn agents_by_surface<'a>(top: &'a str, processes: &'a str) -> HashMap<&'a str, &'static str> {
    let names = process_names(processes);
    let mut parent_of: HashMap<&str, &str> = HashMap::new();
    let mut found: Vec<(&str, &'static str)> = Vec::new();
    for line in top.lines() {
        let mut fields = line.split('\t');
        let (Some("process"), Some(pid), Some(parent), Some(title)) =
            (fields.nth(3), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        parent_of.insert(pid, parent);
        if let Some(agent) = [Some(title), names.get(pid).copied()]
            .into_iter()
            .flatten()
            .find_map(agent_named)
        {
            found.push((pid, agent));
        }
    }

    let mut agents = HashMap::new();
    for (pid, agent) in found {
        if let Some(surface) = owning_surface(pid, &parent_of) {
            agents.entry(surface).or_insert(agent);
        }
    }
    agents
}

/// The agent a command names, if any: the basename of its first word.
fn agent_named(command: &str) -> Option<&'static str> {
    let first = command.split_whitespace().next()?;
    let name = first.rsplit('/').next().unwrap_or(first);
    AGENT_NAMES.iter().copied().find(|&agent| agent == name)
}

/// pid → command, from `ps -A -o pid=,comm=`.
fn process_names(processes: &str) -> HashMap<&str, &str> {
    processes
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?, fields.next()?))
        })
        .collect()
}

/// The surface a pid belongs to: agents sit a fork or two under the
/// shell. The bound only guards against a reply that parents a process
/// to itself — real chains are a handful of forks deep.
fn owning_surface<'a>(pid: &'a str, parent_of: &HashMap<&'a str, &'a str>) -> Option<&'a str> {
    let mut at = pid;
    for _ in 0..64 {
        let parent = parent_of.get(at)?;
        if parent.starts_with("surface:") {
            return Some(parent);
        }
        at = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two windows. Drift sits in w1/ws1 beside a claude split; the same
    /// window holds a second workspace, the other window a third.
    const TREE: &str = r#"{"windows":[
        {"id":"w1","workspaces":[
            {"id":"ws1","title":"drift","panes":[
                {"surfaces":[{"id":"s-drift","ref":"surface:1","title":"drift"}]},
                {"surfaces":[
                    {"id":"s-claude","ref":"surface:2","title":"claude"},
                    {"id":"s-docs","ref":"surface:6","title":"docs"}
                ]}
            ]},
            {"id":"ws2","title":"tests","panes":[
                {"surfaces":[{"id":"s-codex","ref":"surface:3","title":"codex"}]}
            ]}
        ]},
        {"id":"w2","workspaces":[
            {"id":"ws3","title":"api","panes":[
                {"surfaces":[
                    {"id":"s-aider","ref":"surface:4","title":"aider"},
                    {"id":"s-vim","ref":"surface:5","title":"vim"}
                ]}
            ]}
        ]}
    ]}"#;

    /// cmux's own process map: claude one fork under its shell, codex
    /// two, aider straight off the surface. surface:5 runs vim and
    /// surface:6 is a browser, so neither owns a process cmux names.
    /// A versioned Claude Code install is named for its version here.
    const TOP: &str = "\
0.0\t100\t9\ttotal\ttotal\t\t
0.0\t100\t9\twindow\twindow:1\ttotal\t
0.0\t100\t1\tsurface\tsurface:1\tpane:1\tfish
0.0\t100\t1\tprocess\t100\tsurface:1\t/opt/homebrew/bin/fish
0.0\t100\t1\tprocess\t200\tsurface:2\t/bin/zsh
0.0\t100\t1\tprocess\t201\t200\t2.1.220
0.0\t100\t1\tprocess\t300\tsurface:3\t/bin/zsh
0.0\t100\t1\tprocess\t301\t300\tnode
0.0\t100\t1\tprocess\t302\t301\tcodex
0.0\t100\t1\tprocess\t400\tsurface:4\taider
0.0\t100\t1\tprocess\t500\tsurface:5\tvim
0.0\t100\t1\tprocess\t600\twindow:1\tcmux";

    /// `ps` names a process by how it was launched, so it still calls
    /// pid 201 claude where cmux reports the version it resolved to.
    const PS: &str = "\
  100 /opt/homebrew/bin/fish
  200 /bin/zsh
  201 /Users/tothalex/.local/bin/claude
  300 /bin/zsh
  301 /usr/local/bin/node
  302 /usr/local/bin/codex
  400 /usr/local/bin/aider
  500 /usr/bin/vim
  600 /Applications/cmux.app/Contents/MacOS/cmux";

    fn cmux(surface: Option<&str>, workspace: Option<&str>) -> Cmux {
        Cmux {
            cli: "cmux".to_string(),
            own_surface: surface.map(str::to_string),
            own_workspace: workspace.map(str::to_string),
        }
    }

    #[test]
    fn agents_resolve_through_the_process_map_and_drift_drops_out() {
        let own = cmux(Some("s-drift"), Some("ws1"));
        let targets = resolve_targets(TREE, TOP, PS, &own).unwrap();
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["claude", "codex", "aider"],
            "a browser surface owns no process and vim is no agent"
        );
        assert!(targets.iter().all(|t| t.id != "s-drift"));

        assert!(resolve_targets("not json", TOP, PS, &own).is_err());
    }

    /// A stale or reused tty once made two surfaces look like the same
    /// agent; the process map names the owning surface outright.
    #[test]
    fn an_agent_is_claimed_only_by_the_surface_that_owns_its_process() {
        let own = cmux(Some("s-drift"), Some("ws1"));
        let targets = resolve_targets(TREE, TOP, PS, &own).unwrap();
        let claude: Vec<&str> = targets
            .iter()
            .filter(|t| t.name == "claude")
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(claude, ["s-claude"]);
    }

    /// A process parented to itself must not spin the parent walk.
    #[test]
    fn a_cyclic_process_map_resolves_to_no_surface() {
        let top = "0.0\t100\t1\tprocess\t900\t900\tclaude";
        assert!(agents_by_surface(top, "").is_empty());
    }

    #[test]
    fn placement_ranks_workspace_then_window_and_labels_say_where() {
        let own = cmux(Some("s-drift"), Some("ws1"));
        let targets = resolve_targets(TREE, TOP, PS, &own).unwrap();
        let places: Vec<Place> = targets.iter().map(|t| t.place).collect();
        assert_eq!(
            places,
            [Place::SameTab, Place::SameWorkspace, Place::Elsewhere]
        );
        // No status segment: cmux has no idle/working notion.
        assert_eq!(targets[0].label(), "claude · this tab");
        assert_eq!(targets[1].label(), "codex · tests");
        assert_eq!(targets[2].label(), "aider · api");
    }

    #[test]
    fn an_untitled_workspace_falls_back_to_the_surface_then_the_id() {
        let tree = r#"{"windows":[{"id":"w1","workspaces":[
            {"id":"ws9","title":null,"panes":[{"surfaces":[
                {"id":"s-claude","ref":"surface:2","title":"claude"},
                {"id":"s-codex","ref":"surface:3","title":"  "}
            ]}]}
        ]}]}"#;
        let targets = resolve_targets(tree, TOP, PS, &cmux(None, None)).unwrap();
        assert_eq!(targets[0].label(), "claude · claude");
        assert_eq!(targets[1].label(), "codex · s-codex");
    }

    #[test]
    fn outside_cmux_everything_is_elsewhere() {
        let targets = resolve_targets(TREE, TOP, PS, &cmux(None, None)).unwrap();
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().all(|t| t.place == Place::Elsewhere));
        // Drift's own surface isn't excluded (there is none) but holds
        // no agent, so it still never shows up.
        assert_eq!(targets[0].label(), "claude · drift");
    }
}
