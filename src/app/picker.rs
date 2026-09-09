//! The picker overlays' data: base branches, review scopes, or open pull
//! requests, each a flat list with a cursor. Key handling stays on `App`
//! (`handle_picker_key`); rendering lives in `ui::picker`.

use std::path::{Path, PathBuf};

use crate::connect::{AgentTarget, SendContext};
use crate::forge::date_of;
use crate::forge::model::PullRequest;
use crate::vcs::model::{BranchInfo, CommitInfo, Scope, WorktreeInfo, WorktreeStats};

/// The base-branch picker overlay: branch list and its cursor.
pub struct BasePicker {
    pub branches: Vec<String>,
    pub cursor: usize,
}

/// The scope picker: review everything, only committed work, only
/// uncommitted changes, or one commit. Reachable directly, for the
/// narrowing you do constantly without wanting the whole board.
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

/// Which list the board is showing. Two axes over the same repo:
/// worktrees are live working copies — what an agent is doing right
/// now, uncommitted work included; branches are committed work, which
/// is reviewable whether or not anything has them checked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardAxis {
    #[default]
    Worktrees,
    Branches,
}

impl BoardAxis {
    /// The axis `b` flips to.
    pub fn other(self) -> BoardAxis {
        match self {
            BoardAxis::Worktrees => BoardAxis::Branches,
            BoardAxis::Branches => BoardAxis::Worktrees,
        }
    }

    /// The `[board] first` config value naming this axis, and the
    /// panel caption drawn above the rows.
    pub fn name(self) -> &'static str {
        match self {
            BoardAxis::Worktrees => "worktrees",
            BoardAxis::Branches => "branches",
        }
    }

    /// Parse the config value; `None` for anything else.
    pub fn parse(name: &str) -> Option<BoardAxis> {
        [BoardAxis::Worktrees, BoardAxis::Branches]
            .into_iter()
            .find(|axis| axis.name() == name)
    }
}

/// What a board row is about, and so what picking it reviews: a
/// worktree's live working copy, or a branch at its tip. Also the
/// identity a row keeps across rebuilds — indices move as rows reorder
/// by commit time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowId {
    Worktree(PathBuf),
    Branch(String),
}

/// One row on the review board, with whatever the background scan and
/// the agent bridge have since learned about it. Both arrive after the
/// board is already on screen, so both are optional.
pub struct BoardRow {
    pub id: RowId,
    /// First column: the worktree's directory name, or the branch name.
    pub label: String,
    /// Second column: the branch a worktree has checked out. Branch
    /// rows leave it empty — the branch is already the label.
    pub branch: Option<String>,
    /// Commit time of the tip, for the age column.
    pub last_commit: i64,
    pub stats: Option<WorktreeStats>,
    /// Agent name and status ("claude", "working") for an agent working
    /// in this worktree — or, on the branch axis, in whichever worktree
    /// has this branch checked out. herdr only; see
    /// [`Bridge::board_agents`].
    ///
    /// [`Bridge::board_agents`]: crate::connect::Bridge::board_agents
    pub agent: Option<(String, String)>,
    /// The row drift is reviewing right now.
    pub current: bool,
    pub expanded: bool,
    /// Commits offered under the expanded row, read on first expand.
    pub commits: Vec<CommitInfo>,
}

impl BoardRow {
    /// A worktree row. `current` is the worktree drift has open.
    pub fn from_worktree(info: WorktreeInfo, current: &Path) -> BoardRow {
        BoardRow {
            current: info.path == current,
            id: RowId::Worktree(info.path),
            label: info.name,
            branch: info.branch,
            last_commit: info.last_commit,
            stats: None,
            agent: None,
            expanded: false,
            commits: Vec::new(),
        }
    }

    /// A branch row. `current` is the work side drift is reviewing —
    /// the branch of the open worktree, or a branch reviewed off this
    /// very board.
    pub fn from_branch(info: BranchInfo, current: &str) -> BoardRow {
        BoardRow {
            current: info.name == current,
            label: info.name.clone(),
            id: RowId::Branch(info.name),
            branch: None,
            last_commit: info.last_commit,
            stats: None,
            agent: None,
            expanded: false,
            commits: Vec::new(),
        }
    }

    /// The worktree this row opens, when it is one.
    pub fn path(&self) -> Option<&Path> {
        match &self.id {
            RowId::Worktree(path) => Some(path),
            RowId::Branch(_) => None,
        }
    }

    /// The scopes an expanded row offers above its commits. A worktree
    /// has a working copy, so it can be sliced; a branch is committed
    /// work already — picking the row itself is the whole of it, and
    /// the only narrowing left is one commit.
    fn scopes(&self) -> &'static [(Scope, &'static str)] {
        match self.id {
            RowId::Worktree(_) => &[
                (Scope::All, "all changes"),
                (Scope::Uncommitted, "uncommitted"),
                (Scope::Committed, "committed"),
            ],
            RowId::Branch(_) => &[],
        }
    }
}

/// A footer entry: the rare axes, kept out of the row list but reachable
/// without a second key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardAction {
    Base,
    Pr,
}

/// A drawn board line. Rows flatten into these so the cursor is one
/// index into one list, the way the file tree already works. Only the
/// active axis is ever flattened, so `Row` indexes into it.
pub enum BoardLine {
    Row(usize),
    /// A scope under an expanded row: which row it belongs to, what
    /// picking it selects, and the label to draw.
    Scope {
        row: usize,
        scope: Scope,
        label: String,
    },
    Action(BoardAction),
}

/// Where the cursor sat, in terms that survive a rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardMark {
    Row(RowId),
    Scope(RowId, Scope),
    Action(BoardAction),
}

/// One axis as it was left: which rows were open and which line the
/// cursor was on.
#[derive(Debug, Default, Clone)]
pub struct AxisMemory {
    pub expanded: Vec<RowId>,
    pub cursor: Option<BoardMark>,
}

/// The board as it was left: the axis that was up, and each axis's own
/// place. Reopening restores all of it — after picking a scope, `b` is
/// how you get back to the list you picked it from, and a freshly
/// collapsed board with the cursor elsewhere is not that list.
#[derive(Debug, Default, Clone)]
pub struct BoardMemory {
    pub axis: BoardAxis,
    pub worktrees: AxisMemory,
    pub branches: AxisMemory,
}

impl BoardMemory {
    /// The remembered place of one axis.
    pub fn axis(&self, axis: BoardAxis) -> &AxisMemory {
        match axis {
            BoardAxis::Worktrees => &self.worktrees,
            BoardAxis::Branches => &self.branches,
        }
    }

    pub fn set(&mut self, axis: BoardAxis, memory: AxisMemory) {
        match axis {
            BoardAxis::Worktrees => self.worktrees = memory,
            BoardAxis::Branches => self.branches = memory,
        }
    }
}

/// What `l` reaches on a board line — one key for the whole descent:
/// into a row, then into the scope or commit it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Descend {
    /// Open (`true`) or close (`false`) the row: the key that opened a
    /// row is the one you reach for to close it.
    Fold(bool),
    /// Take the line, exactly as Enter does. A scope is the deepest
    /// thing on the board, so `l` must not stop one key short of the
    /// thing the row was opened for.
    Choose,
}

/// The review board: every worktree of the repo, or every branch of it,
/// most recently committed first, each expandable into the scopes and
/// commits inside it. Replaces the old base picker as the answer to
/// "what am I reviewing?" — base and pull requests are footer actions.
pub struct WorktreeBoard {
    pub axis: BoardAxis,
    pub worktrees: Vec<BoardRow>,
    pub branches: Vec<BoardRow>,
    pub lines: Vec<BoardLine>,
    pub cursor: usize,
    /// Staleness sequence for the background fill; results from an
    /// older board are dropped.
    pub seq: u64,
}

impl WorktreeBoard {
    pub fn new(
        worktrees: Vec<BoardRow>,
        branches: Vec<BoardRow>,
        axis: BoardAxis,
        seq: u64,
    ) -> WorktreeBoard {
        let mut board = WorktreeBoard {
            axis,
            worktrees,
            branches,
            lines: Vec::new(),
            cursor: 0,
            seq,
        };
        board.open_on_current();
        board
    }

    /// The rows of the axis on screen.
    pub fn rows(&self) -> &[BoardRow] {
        match self.axis {
            BoardAxis::Worktrees => &self.worktrees,
            BoardAxis::Branches => &self.branches,
        }
    }

    fn rows_mut(&mut self) -> &mut Vec<BoardRow> {
        match self.axis {
            BoardAxis::Worktrees => &mut self.worktrees,
            BoardAxis::Branches => &mut self.branches,
        }
    }

    /// Flatten the active axis and put the cursor on the row in view:
    /// the board is also a status display, and the row you are on is
    /// the one you know about.
    pub fn open_on_current(&mut self) {
        self.reflow();
        self.cursor = self
            .lines
            .iter()
            .position(|line| matches!(line, BoardLine::Row(i) if self.rows()[*i].current))
            .unwrap_or(0);
    }

    /// Rebuild the drawn lines from the active axis's rows, keeping the
    /// cursor on whatever row it was on (an expand must not move it).
    pub fn reflow(&mut self) {
        let anchor = self.row_at_cursor();
        let mut lines = Vec::new();
        for (index, row) in self.rows().iter().enumerate() {
            lines.push(BoardLine::Row(index));
            if !row.expanded {
                continue;
            }
            for (scope, label) in row.scopes() {
                lines.push(BoardLine::Scope {
                    row: index,
                    scope: scope.clone(),
                    label: label.to_string(),
                });
            }
            for commit in &row.commits {
                lines.push(BoardLine::Scope {
                    row: index,
                    scope: Scope::Commit(commit.id.clone()),
                    label: format!("{} {}", commit.short_id, commit.summary),
                });
            }
        }
        self.lines = lines;
        if let Some(anchor) = anchor {
            self.cursor = self
                .lines
                .iter()
                .position(|line| matches!(line, BoardLine::Row(i) if *i == anchor))
                .unwrap_or(self.cursor);
        }
        self.lines.push(BoardLine::Action(BoardAction::Base));
        self.lines.push(BoardLine::Action(BoardAction::Pr));
        self.cursor = self.cursor.min(self.lines.len().saturating_sub(1));
    }

    /// The row the cursor sits on or under — a scope line belongs to
    /// its parent row, so the answer is the same either way.
    pub fn row_at_cursor(&self) -> Option<usize> {
        match self.lines.get(self.cursor)? {
            BoardLine::Row(index) => Some(*index),
            BoardLine::Scope { row, .. } => Some(*row),
            BoardLine::Action(_) => None,
        }
    }

    /// The row id the cursor sits on or under.
    pub fn id_at_cursor(&self) -> Option<&RowId> {
        Some(&self.rows()[self.row_at_cursor()?].id)
    }

    /// Attach a finished stats scan to its row. Both axes are filled —
    /// the counts of an axis you have not switched to yet are already
    /// waiting when you do.
    pub fn set_stats(&mut self, id: &RowId, stats: WorktreeStats) {
        for rows in [&mut self.worktrees, &mut self.branches] {
            if let Some(row) = rows.iter_mut().find(|row| row.id == *id) {
                row.stats = Some(stats);
            }
        }
    }

    /// Join agents onto rows by directory — the one the agent is
    /// working in, which the bridge has already resolved. That is often
    /// deeper than the worktree root, so the deepest matching root
    /// wins: nested worktrees (`.claude/worktrees/` inside the
    /// checkout) would otherwise all match the parent, and an agent
    /// that moved into one would be credited to the branch it left.
    ///
    /// A branch row inherits the agent of the worktree that has it
    /// checked out — the same agent, named on the other axis.
    pub fn set_agents(&mut self, agents: &[AgentTarget]) {
        for agent in agents {
            if agent.cwd.as_os_str().is_empty() {
                continue;
            }
            let best = self
                .worktrees
                .iter()
                .enumerate()
                .filter_map(|(index, row)| Some((index, row.path()?)))
                .filter(|(_, path)| agent.cwd.starts_with(path))
                .max_by_key(|(_, path)| path.components().count());
            if let Some((index, _)) = best {
                self.worktrees[index].agent = Some((agent.name.clone(), agent.status.clone()));
            }
        }
        for branch in &mut self.branches {
            branch.agent = self
                .worktrees
                .iter()
                .find(|row| row.branch.as_deref() == Some(branch.label.as_str()))
                .and_then(|row| row.agent.clone());
        }
    }

    /// What `l` should do where the cursor is. `None` on the footer
    /// actions, which neither nest nor belong to a row; `h` (which
    /// means "out") always folds instead.
    pub fn descend(&self) -> Option<Descend> {
        match self.lines.get(self.cursor)? {
            BoardLine::Row(index) => Some(Descend::Fold(!self.rows()[*index].expanded)),
            BoardLine::Scope { .. } => Some(Descend::Choose),
            BoardLine::Action(_) => None,
        }
    }

    /// Whether Enter closes the row under the cursor rather than
    /// picking a line. An open row's Enter means "I am done with this
    /// row" — walking away to review the whole worktree is not what
    /// the key that opened it should do next. A closed row still picks,
    /// so reviewing one stays a single Enter.
    pub fn enter_folds(&self) -> bool {
        matches!(
            self.lines.get(self.cursor),
            Some(BoardLine::Row(index)) if self.rows()[*index].expanded
        )
    }

    /// Snapshot the axis on screen for the next open, or for coming
    /// back to it after a look at the other one.
    pub fn remember(&self) -> AxisMemory {
        AxisMemory {
            expanded: self
                .rows()
                .iter()
                .filter(|row| row.expanded)
                .map(|row| row.id.clone())
                .collect(),
            cursor: self.mark(),
        }
    }

    /// The line under the cursor, as something worth remembering.
    fn mark(&self) -> Option<BoardMark> {
        Some(match self.lines.get(self.cursor)? {
            BoardLine::Row(index) => BoardMark::Row(self.rows()[*index].id.clone()),
            BoardLine::Scope { row, scope, .. } => {
                BoardMark::Scope(self.rows()[*row].id.clone(), scope.clone())
            }
            BoardLine::Action(action) => BoardMark::Action(*action),
        })
    }

    /// Mark a row expanded, taking `commits` when the caller has just
    /// read them — `None` keeps whatever the row already cached, so
    /// reopening a row costs no second rev walk.
    pub fn expand(&mut self, id: &RowId, commits: Option<Vec<CommitInfo>>) {
        let Some(row) = self.rows_mut().iter_mut().find(|row| row.id == *id) else {
            return;
        };
        if let Some(commits) = commits {
            row.commits = commits;
        }
        row.expanded = true;
    }

    pub fn collapse(&mut self, id: &RowId) {
        if let Some(row) = self.rows_mut().iter_mut().find(|row| row.id == *id) {
            row.expanded = false;
        }
    }

    /// Put the cursor back on a remembered line. Anything no longer
    /// there — a pruned worktree, a deleted branch, a commit that was
    /// amended away — leaves the cursor where [`Self::open_on_current`]
    /// put it, on the row in view.
    pub fn place_cursor(&mut self, mark: &BoardMark) {
        let rows = self.rows();
        let found = self.lines.iter().position(|line| match (line, mark) {
            (BoardLine::Row(index), BoardMark::Row(id)) => rows[*index].id == *id,
            (BoardLine::Scope { row, scope, .. }, BoardMark::Scope(id, want)) => {
                rows[*row].id == *id && scope == want
            }
            (BoardLine::Action(line), BoardMark::Action(want)) => line == want,
            _ => false,
        });
        if let Some(index) = found {
            self.cursor = index;
        }
    }
}

/// Whichever picker overlay is open.
pub enum Picker {
    Board(WorktreeBoard),
    Base(BasePicker),
    Scope(ScopePicker),
    Pr(PrPicker),
    Agent(AgentPicker),
}

impl Picker {
    /// Drawn lines, whichever overlay this is.
    pub(crate) fn len(&self) -> usize {
        match self {
            Picker::Board(board) => board.lines.len(),
            Picker::Base(picker) => picker.branches.len(),
            Picker::Scope(picker) => picker.entries.len(),
            Picker::Pr(picker) => picker.rows.len(),
            Picker::Agent(picker) => picker.rows.len(),
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        match self {
            Picker::Board(board) => board.cursor,
            Picker::Base(picker) => picker.cursor,
            Picker::Scope(picker) => picker.cursor,
            Picker::Pr(picker) => picker.cursor,
            Picker::Agent(picker) => picker.cursor,
        }
    }

    /// Put the cursor on `index`, clamped to the last line.
    pub(crate) fn set_cursor(&mut self, index: usize) {
        let last = self.len().saturating_sub(1);
        let cursor = match self {
            Picker::Board(board) => &mut board.cursor,
            Picker::Base(picker) => &mut picker.cursor,
            Picker::Scope(picker) => &mut picker.cursor,
            Picker::Pr(picker) => &mut picker.cursor,
            Picker::Agent(picker) => &mut picker.cursor,
        };
        *cursor = index.min(last);
    }

    pub(crate) fn move_cursor(&mut self, delta: isize) {
        self.set_cursor(self.cursor().saturating_add_signed(delta));
    }

    /// What `/` matches on each drawn line: the text the eye sees, so a
    /// search finds what is on screen and nothing invisible.
    pub(crate) fn line_text(&self, index: usize) -> String {
        match self {
            Picker::Board(board) => match board.lines.get(index) {
                Some(BoardLine::Row(row)) => {
                    let row = &board.rows()[*row];
                    match &row.branch {
                        Some(branch) => format!("{} {branch}", row.label),
                        None => row.label.clone(),
                    }
                }
                Some(BoardLine::Scope { label, .. }) => label.clone(),
                Some(BoardLine::Action(action)) => match action {
                    BoardAction::Base => "compare against".to_string(),
                    BoardAction::Pr => "pull requests".to_string(),
                },
                None => String::new(),
            },
            Picker::Base(picker) => picker.branches.get(index).cloned().unwrap_or_default(),
            Picker::Scope(picker) => picker
                .entries
                .get(index)
                .map(|(_, label)| label.clone())
                .unwrap_or_default(),
            Picker::Pr(picker) => picker
                .rows
                .get(index)
                .map(|(label, _)| label.clone())
                .unwrap_or_default(),
            Picker::Agent(picker) => picker
                .rows
                .get(index)
                .map(|(label, _)| label.clone())
                .unwrap_or_default(),
        }
    }

    /// The next line matching `query` at or after `from`, wrapping —
    /// `step` is +1 forwards, -1 back. Case-insensitive, like the
    /// searches in the panes.
    pub(crate) fn find(&self, query: &str, from: usize, step: isize) -> Option<usize> {
        let len = self.len();
        if query.is_empty() || len == 0 {
            return None;
        }
        let query = query.to_lowercase();
        (0..len)
            .map(|offset| {
                let offset = offset as isize * step + from as isize;
                offset.rem_euclid(len as isize) as usize
            })
            .find(|index| self.line_text(*index).to_lowercase().contains(&query))
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

/// Compact age of a commit for a board row: the board is scanned, not
/// read, so one or two characters beat a date. Saturates at years.
pub fn ago(now: i64, then: i64) -> String {
    if then <= 0 {
        return String::new();
    }
    let secs = (now - then).max(0);
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const YEAR: i64 = 365 * DAY;
    match secs {
        s if s < MINUTE => "now".to_string(),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < WEEK => format!("{}d", s / DAY),
        s if s < YEAR => format!("{}w", s / WEEK),
        s => format!("{}y", s / YEAR),
    }
}

/// Fit a branch name into `width`, dropping leading path segments
/// first: `feature/FQBB-XX/sms-provider` reads as `…/sms-provider`,
/// because the tail is what distinguishes one agent's branch from the
/// next when every branch shares a prefix.
pub fn fit_branch(branch: &str, width: usize) -> String {
    if branch.chars().count() <= width {
        return branch.to_string();
    }
    for (index, _) in branch.match_indices('/') {
        let tail = &branch[index..];
        if tail.chars().count() < width {
            return format!("…{tail}");
        }
    }
    let keep = width.saturating_sub(1);
    let tail: String = branch
        .chars()
        .skip(branch.chars().count().saturating_sub(keep))
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::Place;
    use crate::vcs::model::WorktreeInfo;

    /// A worktree-axis board over `paths`, each on its own branch
    /// ("/a" → "topic/a"), open on `current`.
    fn board(paths: &[&str], current: &str) -> WorktreeBoard {
        board_with(paths, current, &[])
    }

    /// The same, plus a branch axis over `branches`.
    fn board_with(paths: &[&str], current: &str, branches: &[&str]) -> WorktreeBoard {
        let worktrees = paths
            .iter()
            .map(|path| {
                BoardRow::from_worktree(
                    WorktreeInfo {
                        name: PathBuf::from(path)
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        path: PathBuf::from(path),
                        branch: Some(format!("topic{path}")),
                        last_commit: 1,
                    },
                    Path::new(current),
                )
            })
            .collect();
        let branches = branches
            .iter()
            .map(|name| {
                BoardRow::from_branch(
                    BranchInfo {
                        name: (*name).to_string(),
                        tip: crate::vcs::model::RevisionId("abc".to_string()),
                        last_commit: 1,
                    },
                    "topic/a",
                )
            })
            .collect();
        WorktreeBoard::new(worktrees, branches, BoardAxis::Worktrees, 1)
    }

    fn agent(cwd: &str) -> AgentTarget {
        AgentTarget {
            name: "claude".to_string(),
            id: "p1".to_string(),
            status: "working".to_string(),
            cwd: PathBuf::from(cwd),
            session: None,
            place: Place::Elsewhere,
            where_label: String::new(),
        }
    }

    #[test]
    fn board_opens_on_the_worktree_in_view() {
        let board = board(
            &["/repo", "/repo/.claude/worktrees/a"],
            "/repo/.claude/worktrees/a",
        );
        assert!(matches!(board.lines[board.cursor], BoardLine::Row(1)));
    }

    #[test]
    fn agents_join_the_deepest_matching_worktree() {
        // The nested layout agents actually use: the linked worktrees
        // live inside the main checkout, so every one of their cwds is
        // also a prefix match on the parent.
        let mut board = board(&["/repo", "/repo/.claude/worktrees/a"], "/repo");
        board.set_agents(&[agent("/repo/.claude/worktrees/a/src")]);
        assert_eq!(board.rows()[0].agent, None);
        assert_eq!(
            board.rows()[1].agent,
            Some(("claude".to_string(), "working".to_string()))
        );
    }

    #[test]
    fn an_agent_without_a_directory_lands_nowhere() {
        // Backends other than herdr contribute no board agents at all,
        // but a herdr pane can still report no cwd; such an agent must
        // not pile onto the first row.
        let mut board = board(&["/repo"], "/repo");
        board.set_agents(&[agent("")]);
        assert_eq!(board.rows()[0].agent, None);
    }

    #[test]
    fn expanding_keeps_the_cursor_on_its_worktree() {
        let mut board = board(&["/a", "/b"], "/a");
        board.cursor = 1;
        board.worktrees[1].expanded = true;
        board.reflow();
        assert!(matches!(board.lines[board.cursor], BoardLine::Row(1)));
        // The scopes belong to the row they were opened under.
        assert!(matches!(
            board.lines[board.cursor + 1],
            BoardLine::Scope { row: 1, .. }
        ));
    }

    #[test]
    fn l_folds_a_worktree_row_and_selects_a_scope() {
        let mut board = board(&["/a", "/b"], "/a");
        assert_eq!(board.descend(), Some(Descend::Fold(true)));
        board.worktrees[0].expanded = true;
        board.reflow();
        assert_eq!(board.descend(), Some(Descend::Fold(false)));
        // One row deeper there is nothing left to open: `l` selects
        // the scope, the same as Enter.
        board.cursor += 1;
        assert!(matches!(board.lines[board.cursor], BoardLine::Scope { .. }));
        assert_eq!(board.descend(), Some(Descend::Choose));
        // The footer actions belong to no worktree and don't nest.
        board.cursor = board.lines.len() - 1;
        assert_eq!(board.descend(), None);
    }

    #[test]
    fn enter_closes_an_open_row_instead_of_picking_it() {
        let mut board = board(&["/a", "/b"], "/a");
        // A closed row: Enter reviews it, one key as before.
        assert!(!board.enter_folds());
        board.worktrees[0].expanded = true;
        board.reflow();
        assert!(board.enter_folds());
        // Its scopes still pick — they are the leaves.
        board.cursor += 1;
        assert!(matches!(board.lines[board.cursor], BoardLine::Scope { .. }));
        assert!(!board.enter_folds());
        // So do the footer actions.
        board.cursor = board.lines.len() - 1;
        assert!(!board.enter_folds());
    }

    #[test]
    fn the_board_reopens_on_the_line_it_was_left_on() {
        let mut left = board(&["/a", "/b"], "/a");
        left.worktrees[1].expanded = true;
        left.reflow();
        left.cursor = left
            .lines
            .iter()
            .position(|line| {
                matches!(
                    line,
                    BoardLine::Scope {
                        row: 1,
                        scope: Scope::Uncommitted,
                        ..
                    }
                )
            })
            .unwrap();
        let memory = left.remember();
        assert_eq!(memory.expanded, vec![RowId::Worktree(PathBuf::from("/b"))]);

        // Reopened after reviewing /b: rows are ordered by commit time,
        // so the remembered row has moved and the current worktree is
        // no longer the one the cursor belongs on.
        let mut reopened = board(&["/b", "/a"], "/b");
        for id in &memory.expanded {
            reopened.expand(id, None);
        }
        reopened.reflow();
        reopened.place_cursor(memory.cursor.as_ref().unwrap());
        assert!(matches!(
            reopened.lines[reopened.cursor],
            BoardLine::Scope {
                row: 0,
                scope: Scope::Uncommitted,
                ..
            }
        ));
    }

    #[test]
    fn a_line_that_is_gone_leaves_the_cursor_in_view() {
        // A pruned worktree, or a commit amended away since: the board
        // opens on the worktree being reviewed, as it does with no
        // memory at all.
        let mut board = board(&["/a", "/b"], "/b");
        let opened_on = board.cursor;
        board.place_cursor(&BoardMark::Row(RowId::Worktree(PathBuf::from("/gone"))));
        assert_eq!(board.cursor, opened_on);
        board.place_cursor(&BoardMark::Scope(
            RowId::Worktree(PathBuf::from("/a")),
            Scope::Uncommitted,
        ));
        assert_eq!(board.cursor, opened_on);
    }

    #[test]
    fn the_branch_axis_lists_branches_and_expands_into_commits() {
        let mut board = board_with(&["/a"], "/a", &["topic/a", "release/2.0"]);
        // The axis on screen is the one whose rows are flattened.
        assert_eq!(board.rows().len(), 1);
        board.axis = BoardAxis::Branches;
        board.open_on_current();
        assert_eq!(board.rows().len(), 2);
        // The branch drift is reviewing carries the marker, whichever
        // axis names it.
        assert!(board.rows()[0].current);
        assert!(!board.rows()[1].current);

        // A branch is committed work already: the row itself is the
        // whole changeset, so the only narrowing under it is a commit.
        board.expand(
            &RowId::Branch("release/2.0".to_string()),
            Some(vec![CommitInfo {
                id: crate::vcs::model::RevisionId("f1".to_string()),
                short_id: "f1ab918".to_string(),
                summary: "feat: executor".to_string(),
            }]),
        );
        board.reflow();
        let under: Vec<_> = board
            .lines
            .iter()
            .filter_map(|line| match line {
                BoardLine::Scope { row: 1, scope, .. } => Some(scope.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            under,
            vec![Scope::Commit(crate::vcs::model::RevisionId(
                "f1".to_string()
            ))]
        );
    }

    #[test]
    fn a_branch_row_shows_the_agent_of_the_worktree_holding_it() {
        // The same agent, named on the other axis: you should not have
        // to know which worktree has a branch to see it is busy.
        let mut board = board_with(&["/a", "/b"], "/a", &["topic/a", "topic/b", "idle-branch"]);
        board.set_agents(&[agent("/b/src")]);
        assert_eq!(board.worktrees[0].agent, None);
        assert_eq!(
            board.branches[1].agent,
            Some(("claude".to_string(), "working".to_string()))
        );
        assert_eq!(board.branches[0].agent, None);
        assert_eq!(board.branches[2].agent, None);
    }

    #[test]
    fn each_axis_remembers_its_own_place() {
        let mut memory = BoardMemory::default();
        assert_eq!(memory.axis, BoardAxis::Worktrees);

        let mut board = board_with(&["/a", "/b"], "/a", &["topic/a"]);
        board.cursor = 1;
        memory.set(board.axis, board.remember());
        // Flipping axis and back must not carry one list's cursor onto
        // the other's rows.
        board.axis = board.axis.other();
        board.open_on_current();
        memory.set(board.axis, board.remember());
        assert_eq!(
            memory.worktrees.cursor,
            Some(BoardMark::Row(RowId::Worktree(PathBuf::from("/b"))))
        );
        assert_eq!(
            memory.branches.cursor,
            Some(BoardMark::Row(RowId::Branch("topic/a".to_string())))
        );
        assert_eq!(BoardAxis::Branches.other(), BoardAxis::Worktrees);
    }

    #[test]
    fn the_footer_actions_always_close_the_list() {
        let board = board(&["/a"], "/a");
        let tail: Vec<_> = board.lines.iter().rev().take(2).collect();
        assert!(matches!(tail[0], BoardLine::Action(BoardAction::Pr)));
        assert!(matches!(tail[1], BoardLine::Action(BoardAction::Base)));
    }

    #[test]
    fn search_matches_the_text_on_screen_and_wraps() {
        let mut board = board_with(&["/a"], "/a", &["feature/sms-provider", "main"]);
        board.axis = BoardAxis::Branches;
        board.open_on_current();
        let picker = Picker::Board(board);
        // Case-insensitive, on the label as drawn.
        assert_eq!(picker.find("SMS", 0, 1), Some(0));
        assert_eq!(picker.find("main", 0, 1), Some(1));
        // Wraps: from past the last row, forwards finds row 0 again.
        assert_eq!(picker.find("sms", 1, 1), Some(0));
        // Backwards from the top wraps to the end — the footer actions
        // are searchable lines too.
        assert_eq!(picker.find("pull requests", 0, -1), Some(3));
        assert_eq!(picker.find("nothing here", 0, 1), None);
        assert_eq!(picker.find("", 0, 1), None);
    }

    #[test]
    fn a_worktree_row_matches_its_branch_as_well_as_its_name() {
        // Both columns are on screen, so both are searchable.
        let picker = Picker::Board(board(&["/checkout"], "/checkout"));
        assert_eq!(picker.find("checkout", 0, 1), Some(0));
        assert_eq!(picker.find("topic/checkout", 0, 1), Some(0));
    }

    #[test]
    fn the_cursor_clamps_to_the_last_line() {
        let mut picker = Picker::Board(board(&["/a", "/b"], "/a"));
        // Two rows plus the two footer actions.
        assert_eq!(picker.len(), 4);
        picker.set_cursor(99);
        assert_eq!(picker.cursor(), 3);
        picker.move_cursor(-99);
        assert_eq!(picker.cursor(), 0);
        picker.move_cursor(2);
        assert_eq!(picker.cursor(), 2);
    }

    #[test]
    fn ago_steps_through_the_units() {
        let now = 1_000_000_000;
        assert_eq!(ago(now, now - 30), "now");
        assert_eq!(ago(now, now - 180), "3m");
        assert_eq!(ago(now, now - 7200), "2h");
        assert_eq!(ago(now, now - 4 * 86400), "4d");
        assert_eq!(ago(now, now - 21 * 86400), "3w");
        assert_eq!(ago(now, now - 800 * 86400), "2y");
        // An unborn branch has no tip time and gets no age column.
        assert_eq!(ago(now, 0), "");
    }

    #[test]
    fn fit_branch_keeps_the_distinguishing_tail() {
        assert_eq!(fit_branch("main", 20), "main");
        assert_eq!(
            fit_branch("feature/FQBB-XX/sms-provider", 20),
            "…/sms-provider"
        );
        // No slash to cut at: fall back to a plain left-trim.
        assert_eq!(fit_branch("averyveryverylongname", 10), "…ylongname");
    }
}
