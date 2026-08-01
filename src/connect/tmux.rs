//! The tmux backend. tmux has no notion of agents, so agent panes are
//! found by walking each pane's process tree for a known agent CLI —
//! the pane's own command is just the shell, and pane titles are
//! whatever the agent last painted. Prompts go in with `send-keys -l`,
//! which agents receive as a literal multi-line insert (verified
//! against Claude Code: bare `\n` inserts, it never submits); enter is
//! a separate key, exactly like the herdr bridge. tmux reports no
//! idle/working state, so targets carry none.

use std::collections::HashMap;

use anyhow::Result;

use super::{AGENT_NAMES, AgentTarget, Bridge, Place, run_cli, short_path};

/// tmux sets this in every pane — the "you are inside tmux" marker.
pub(super) const INSIDE_ENV: &str = "TMUX";

pub(super) fn make() -> Box<dyn Bridge> {
    Box::new(Tmux {
        own_pane: std::env::var("TMUX_PANE").ok(),
    })
}

struct Tmux {
    /// Drift's own pane (`$TMUX_PANE`), excluded from the target list;
    /// `None` when drift runs outside tmux (forced backend).
    own_pane: Option<String>,
}

/// One line per pane across every session of the default server.
const LIST_FORMAT: &str = "#{pane_id}\t#{window_id}\t#{session_id}\t#{session_name}\t\
                           #{window_name}\t#{pane_pid}\t#{pane_current_path}";

impl Bridge for Tmux {
    fn label(&self) -> &'static str {
        "tmux"
    }

    fn targets(&self) -> Result<Vec<AgentTarget>> {
        let panes = run_cli("tmux", &["list-panes", "-a", "-F", LIST_FORMAT])?;
        // One process snapshot serves every pane's tree walk.
        let processes = run_cli("ps", &["-A", "-o", "pid=,ppid=,comm="])?;
        Ok(resolve_targets(
            &panes,
            &processes,
            self.own_pane.as_deref(),
        ))
    }

    fn send(&self, target_id: &str, text: &str, submit: bool) -> Result<()> {
        // `--` so a prompt starting with `-` is never read as a flag.
        run_cli("tmux", &["send-keys", "-t", target_id, "-l", "--", text])?;
        if submit {
            run_cli("tmux", &["send-keys", "-t", target_id, "Enter"])?;
        }
        Ok(())
    }
}

struct Pane<'a> {
    id: &'a str,
    window: &'a str,
    session: &'a str,
    session_name: &'a str,
    window_name: &'a str,
    pid: u32,
    cwd: &'a str,
}

fn parse_panes(text: &str) -> Vec<Pane<'_>> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(Pane {
                id: fields.next()?,
                window: fields.next()?,
                session: fields.next()?,
                session_name: fields.next()?,
                window_name: fields.next()?,
                pid: fields.next()?.trim().parse().ok()?,
                cwd: fields.next().unwrap_or(""),
            })
        })
        .collect()
}

/// Parent pid → children `(pid, process basename)`, from `ps -A`.
fn parse_process_tree(text: &str) -> HashMap<u32, Vec<(u32, &str)>> {
    let mut children: HashMap<u32, Vec<(u32, &str)>> = HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (
            fields.next().and_then(|f| f.parse().ok()),
            fields.next().and_then(|f| f.parse().ok()),
        ) else {
            continue;
        };
        let Some(command) = fields.next() else {
            continue;
        };
        let name = command.rsplit('/').next().unwrap_or(command);
        children.entry(ppid).or_default().push((pid, name));
    }
    children
}

/// The first known agent among `pid`'s descendants (the pane runs a
/// shell; the agent sits one or more forks below it).
fn agent_under(pid: u32, children: &HashMap<u32, Vec<(u32, &str)>>) -> Option<&'static str> {
    let mut queue = vec![pid];
    while let Some(pid) = queue.pop() {
        for &(child, name) in children.get(&pid).into_iter().flatten() {
            if let Some(agent) = AGENT_NAMES.iter().find(|&&agent| agent == name) {
                return Some(agent);
            }
            queue.push(child);
        }
    }
    None
}

fn resolve_targets(panes: &str, processes: &str, own_pane: Option<&str>) -> Vec<AgentTarget> {
    let panes = parse_panes(panes);
    let children = parse_process_tree(processes);
    // Drift's own row tells which window and session count as "close";
    // outside tmux there is none and every target is elsewhere.
    let own = own_pane.and_then(|id| panes.iter().find(|pane| pane.id == id));
    let (own_window, own_session) = (own.map(|p| p.window), own.map(|p| p.session));

    let mut targets: Vec<AgentTarget> = panes
        .iter()
        .filter(|pane| Some(pane.id) != own_pane)
        .filter_map(|pane| {
            let agent = agent_under(pane.pid, &children)?;
            let place = if Some(pane.window) == own_window {
                Place::SameTab
            } else if Some(pane.session) == own_session {
                Place::SameWorkspace
            } else {
                Place::Elsewhere
            };
            let where_label = match place {
                Place::SameTab => "this tab".to_string(),
                _ if !pane.session_name.is_empty() && !pane.window_name.is_empty() => {
                    format!("{}:{}", pane.session_name, pane.window_name)
                }
                _ if !pane.cwd.is_empty() => short_path(pane.cwd),
                _ => pane.id.to_string(),
            };
            Some(AgentTarget {
                name: agent.to_string(),
                id: pane.id.to_string(),
                // tmux knows nothing about what the agent is doing.
                status: String::new(),
                place,
                where_label,
            })
        })
        .collect();
    targets.sort_by_key(|target| target.place);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANES: &str = "\
%0\t@0\t$0\tmain\tdrift\t100\t/repo
%1\t@0\t$0\tmain\tdrift\t200\t/repo
%2\t@1\t$0\tmain\ttests\t300\t/repo
%3\t@2\t$1\tapi\tserver\t400\t/api";

    /// Pane shells fork the agents at varying depths: %1 has claude one
    /// fork down (sh -> claude), %3 two forks down; %2 runs plain vim.
    const PROCESSES: &str = "\
  100  1 /opt/homebrew/bin/fish
  200  1 /bin/zsh
  201  200 /bin/sh
  202  201 caffeinate
  203  201 claude
  300  1 /bin/zsh
  301  300 vim
  400  1 /bin/bash
  401  400 node
  402  401 codex";

    #[test]
    fn agents_resolve_through_the_process_tree() {
        let targets = resolve_targets(PANES, PROCESSES, Some("%0"));
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["claude", "codex"], "vim pane is no agent");
    }

    #[test]
    fn placement_ranks_window_then_session_and_labels_say_where() {
        let targets = resolve_targets(PANES, PROCESSES, Some("%0"));
        assert_eq!(targets[0].id, "%1");
        assert_eq!(targets[0].place, Place::SameTab);
        assert_eq!(targets[1].id, "%3");
        assert_eq!(targets[1].place, Place::Elsewhere);
        // No status segment: tmux has no idle/working notion.
        assert_eq!(targets[0].label(), "claude · this tab");
        assert_eq!(targets[1].label(), "codex · api:server");
    }

    #[test]
    fn outside_tmux_everything_is_elsewhere() {
        let targets = resolve_targets(PANES, PROCESSES, None);
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().all(|t| t.place == Place::Elsewhere));
        // The drift pane isn't excluded (there is none) but holds no
        // agent, so it still never shows up.
        assert_eq!(targets[0].label(), "claude · main:drift");
    }

    #[test]
    fn same_session_other_window_is_the_middle_rank() {
        // Give the vim pane an agent to exercise SameWorkspace.
        let processes = format!("{PROCESSES}\n  302  301 aider");
        let targets = resolve_targets(PANES, &processes, Some("%0"));
        let places: Vec<Place> = targets.iter().map(|t| t.place).collect();
        assert_eq!(
            places,
            [Place::SameTab, Place::SameWorkspace, Place::Elsewhere]
        );
        assert_eq!(targets[1].label(), "aider · main:tests");
    }
}
