//! The herdr backend (herdr.dev): a terminal workspace manager built
//! for AI coding agents. Panes carry native agent detection, so targets
//! come from `herdr agent list` with the agent's name and idle/working
//! state already resolved — no process-name heuristics needed here.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{AgentTarget, Bridge, Place, run_cli, short_path};

/// herdr sets this in every pane — the "you are inside herdr" marker.
pub(super) const INSIDE_ENV: &str = "HERDR_ENV";

pub(super) fn make() -> Box<dyn Bridge> {
    Box::new(Herdr::from_env())
}

struct Herdr {
    /// Drift's own pane, excluded from the target list.
    own_pane: Option<String>,
    /// Drift's own tab and workspace, for ranking targets by closeness.
    own_tab: Option<String>,
    own_workspace: Option<String>,
}

impl Herdr {
    fn from_env() -> Herdr {
        Herdr {
            own_pane: std::env::var("HERDR_PANE_ID").ok(),
            own_tab: std::env::var("HERDR_TAB_ID").ok(),
            own_workspace: std::env::var("HERDR_WORKSPACE_ID").ok(),
        }
    }
}

impl Bridge for Herdr {
    fn label(&self) -> &'static str {
        "herdr"
    }

    fn targets(&self) -> Result<Vec<AgentTarget>> {
        let agents = run_cli("herdr", &["agent", "list"])?;
        // Tab and workspace names are a labeling nicety — a failure to
        // fetch them must not block sending.
        let tabs = run_cli("herdr", &["tab", "list"]).unwrap_or_default();
        let workspaces = run_cli("herdr", &["workspace", "list"]).unwrap_or_default();
        parse_agent_list(
            &agents,
            &parse_tab_labels(&tabs),
            &parse_workspace_labels(&workspaces),
            self,
        )
    }

    fn send(&self, target_id: &str, text: &str, submit: bool) -> Result<()> {
        // `pane send-text` writes the text literally into the pane's
        // input (multi-line stays multi-line, nothing submitted); enter
        // is a separate key event. herdr 0.7.5 removed `agent send`, so
        // the send path uses only these pane primitives — the target id
        // from `agent list` is the agent's pane id.
        run_cli("herdr", &["pane", "send-text", target_id, text])?;
        if submit {
            run_cli("herdr", &["pane", "send-keys", target_id, "enter"])?;
        }
        Ok(())
    }
}

/// `herdr agent list` reply: `{"id":…,"result":{"agents":[…]}}`.
#[derive(Deserialize)]
struct AgentList {
    result: AgentListResult,
}

#[derive(Deserialize)]
struct AgentListResult {
    agents: Vec<HerdrAgent>,
}

#[derive(Deserialize)]
struct HerdrAgent {
    agent: String,
    pane_id: String,
    #[serde(default)]
    agent_status: String,
    #[serde(default)]
    tab_id: String,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    cwd: String,
}

/// `herdr tab list` reply: tabs with their display labels.
#[derive(Deserialize)]
struct TabList {
    result: TabListResult,
}

#[derive(Deserialize)]
struct TabListResult {
    #[serde(default)]
    tabs: Vec<HerdrTab>,
}

#[derive(Deserialize)]
struct HerdrTab {
    tab_id: String,
    #[serde(default)]
    label: String,
}

/// `herdr workspace list` reply: workspaces with their display labels.
#[derive(Deserialize)]
struct WorkspaceList {
    result: WorkspaceListResult,
}

#[derive(Deserialize)]
struct WorkspaceListResult {
    #[serde(default)]
    workspaces: Vec<HerdrWorkspace>,
}

#[derive(Deserialize)]
struct HerdrWorkspace {
    workspace_id: String,
    #[serde(default)]
    label: String,
}

/// Tab id → display label; tolerant of fetch/parse failures (empty map).
fn parse_tab_labels(json: &str) -> HashMap<String, String> {
    serde_json::from_str::<TabList>(json.trim())
        .map(|list| {
            list.result
                .tabs
                .into_iter()
                .map(|tab| (tab.tab_id, tab.label))
                .collect()
        })
        .unwrap_or_default()
}

/// Workspace id → display label; same tolerance.
fn parse_workspace_labels(json: &str) -> HashMap<String, String> {
    serde_json::from_str::<WorkspaceList>(json.trim())
        .map(|list| {
            list.result
                .workspaces
                .into_iter()
                .map(|ws| (ws.workspace_id, ws.label))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_agent_list(
    json: &str,
    tab_labels: &HashMap<String, String>,
    workspace_labels: &HashMap<String, String>,
    own: &Herdr,
) -> Result<Vec<AgentTarget>> {
    let list: AgentList =
        serde_json::from_str(json.trim()).context("unexpected `herdr agent list` output")?;
    let mut targets: Vec<AgentTarget> = list
        .result
        .agents
        .into_iter()
        .filter(|agent| Some(agent.pane_id.as_str()) != own.own_pane.as_deref())
        .map(|agent| {
            let place = if Some(agent.tab_id.as_str()) == own.own_tab.as_deref() {
                Place::SameTab
            } else if Some(agent.workspace_id.as_str()) == own.own_workspace.as_deref() {
                Place::SameWorkspace
            } else {
                Place::Elsewhere
            };
            let where_label = match place {
                Place::SameTab => "this tab".to_string(),
                _ => {
                    match (
                        workspace_labels.get(&agent.workspace_id),
                        tab_labels.get(&agent.tab_id),
                    ) {
                        (Some(ws), Some(tab)) if !ws.is_empty() && !tab.is_empty() => {
                            format!("{ws}:{tab}")
                        }
                        _ if !agent.cwd.is_empty() => short_path(&agent.cwd),
                        _ => agent.pane_id.clone(),
                    }
                }
            };
            AgentTarget {
                place,
                where_label,
                name: agent.agent,
                id: agent.pane_id,
                status: agent.agent_status,
            }
        })
        .collect();
    targets.sort_by_key(|target| target.place);
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn herdr(pane: Option<&str>, tab: Option<&str>, workspace: Option<&str>) -> Herdr {
        Herdr {
            own_pane: pane.map(str::to_string),
            own_tab: tab.map(str::to_string),
            own_workspace: workspace.map(str::to_string),
        }
    }

    #[test]
    fn agent_list_parses_and_excludes_the_own_pane() {
        let json = r#"{"id":"cli:agent:list","result":{"agents":[
            {"agent":"claude","agent_status":"working","cwd":"/repo","pane_id":"w13:p7","tab_id":"w13:t7","workspace_id":"w13"},
            {"agent":"codex","agent_status":"idle","cwd":"/repo","pane_id":"w13:p2","tab_id":"w13:t2","workspace_id":"w13"}
        ]}}"#;
        let none = HashMap::new();
        let outside = herdr(None, None, None);
        let all = parse_agent_list(json, &none, &none, &outside).unwrap();
        assert_eq!(all.len(), 2);

        let own = herdr(Some("w13:p7"), Some("w13:t7"), Some("w13"));
        let others = parse_agent_list(json, &none, &none, &own).unwrap();
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].name, "codex");

        assert!(parse_agent_list("not json", &none, &none, &outside).is_err());
    }

    #[test]
    fn targets_rank_by_closeness_and_labels_say_where() {
        let json = r#"{"id":"cli:agent:list","result":{"agents":[
            {"agent":"claude","agent_status":"idle","cwd":"/away","pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"},
            {"agent":"claude","agent_status":"working","cwd":"/repo","pane_id":"w1:p3","tab_id":"w1:t2","workspace_id":"w1"},
            {"agent":"claude","agent_status":"idle","cwd":"/repo","pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1"}
        ]}}"#;
        let tabs = parse_tab_labels(
            r#"{"id":"x","result":{"tabs":[
                {"tab_id":"w1:t2","label":"2"},
                {"tab_id":"w2:t1","label":"1"}
            ]}}"#,
        );
        let workspaces = parse_workspace_labels(
            r#"{"id":"x","result":{"workspaces":[
                {"workspace_id":"w1","label":"drift"},
                {"workspace_id":"w2","label":"api"}
            ]}}"#,
        );
        // Drift sits in w1:t1: its tab-mate ranks first, the same
        // workspace next, the other workspace last.
        let own = herdr(Some("w1:p1"), Some("w1:t1"), Some("w1"));
        let targets = parse_agent_list(json, &tabs, &workspaces, &own).unwrap();
        let ids: Vec<&str> = targets.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["w1:p2", "w1:p3", "w2:p1"]);
        assert_eq!(targets[0].place, Place::SameTab);
        // Labels name places, never pane ids: the tab-mate is "this
        // tab", the rest are workspace:tab names.
        assert_eq!(targets[0].label(), "claude · this tab · idle");
        assert_eq!(targets[1].label(), "claude · drift:2 · working");
        assert_eq!(targets[2].label(), "claude · api:1 · idle");

        // Without tab/workspace names, the directory stands in.
        let none = HashMap::new();
        let targets = parse_agent_list(json, &none, &none, &own).unwrap();
        assert_eq!(targets[1].label(), "claude · /repo · working");
    }
}
