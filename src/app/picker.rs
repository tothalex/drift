//! The picker overlays' data: base branches, review scopes, or open pull
//! requests, each a flat list with a cursor. Key handling stays on `App`
//! (`handle_picker_key`); rendering lives in `ui::picker`.

use crate::connect::{AgentTarget, SendContext};
use crate::forge::date_of;
use crate::forge::model::PullRequest;
use crate::vcs::model::Scope;

/// The base-branch picker overlay: branch list and its cursor.
pub struct BasePicker {
    pub branches: Vec<String>,
    pub cursor: usize,
}

/// The scope picker overlay that follows a branch choice: review
/// everything, only tracked changes, only untracked files, or one
/// commit.
pub struct ScopePicker {
    pub entries: Vec<(Scope, String)>,
    pub cursor: usize,
}

/// The pull-request picker overlay: open PRs/MRs fetched from the forge.
pub struct PrPicker {
    /// Panel title, e.g. "open pull requests".
    pub title: String,
    /// Display rows: (label, is the currently open PR). With `back` set,
    /// row 0 is "← back to local changes" and `items[i]` maps to
    /// `rows[i + 1]`.
    pub rows: Vec<(String, bool)>,
    pub items: Vec<PullRequest>,
    pub back: bool,
    pub cursor: usize,
}

/// The agent-target picker overlay: agent panes a prompt can go to,
/// shown when more than one is open. Carries the captured selection so
/// the compose step follows the choice.
pub struct AgentPicker {
    /// Display rows: (label, is the last-used target).
    pub rows: Vec<(String, bool)>,
    pub targets: Vec<AgentTarget>,
    pub ctx: SendContext,
    pub cursor: usize,
}

/// Whichever picker overlay is open.
pub enum Picker {
    Base(BasePicker),
    Scope(ScopePicker),
    Pr(PrPicker),
    Agent(AgentPicker),
}

impl Picker {
    pub(crate) fn move_cursor(&mut self, delta: isize) {
        let (cursor, len) = match self {
            Picker::Base(picker) => (&mut picker.cursor, picker.branches.len()),
            Picker::Scope(picker) => (&mut picker.cursor, picker.entries.len()),
            Picker::Pr(picker) => (&mut picker.cursor, picker.rows.len()),
            Picker::Agent(picker) => (&mut picker.cursor, picker.rows.len()),
        };
        *cursor = cursor
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }
}

/// One picker row for a pull request: number, title, author, freshness.
pub(crate) fn pr_label(pr: &PullRequest) -> String {
    let draft = if pr.draft { " · draft" } else { "" };
    format!(
        "#{} {} · {} · {}{draft}",
        pr.number,
        pr.title,
        pr.author,
        date_of(&pr.updated_at)
    )
}
