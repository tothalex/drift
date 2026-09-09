//! The picker overlays: the review board (every worktree of the repo,
//! expandable into its scopes and commits), and the flat lists it
//! chains into — base branches, pull requests, agent targets. Enter
//! selects, Esc cancels.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::picker::{BoardRow, ago, fit_branch, fit_summary};
use crate::app::{App, BoardAction, BoardAxis, BoardLine, Picker, WorktreeBoard};
use crate::ui::draw_panel;

/// Floors for the board's two identity columns — the worktree name and
/// its branch. They stay aligned across rows, so both are as wide as
/// the widest thing in them, and never narrower than this: a board that
/// says nothing about which row is which is not worth the panel.
const NAME_WIDTH: usize = 22;
const BRANCH_WIDTH: usize = 26;
/// The columns that never move: the marker and caret (" ● ▸ "), the
/// counts, the age, and the room an agent's name and status take. Each
/// is rendered with a trailing space but the last.
const MARKER_WIDTH: usize = 5;
const STATS_WIDTH: usize = 11;
const AGE_WIDTH: usize = 4;
const AGENT_WIDTH: usize = 17;
/// An expanded row's own columns: the indent its scopes and commits
/// hang under, and the count that follows their label ("999 commits").
const SCOPE_INDENT: usize = 7;
const COUNT_WIDTH: usize = 11;
/// The width the board never goes under: every fixed column plus both
/// identity floors, with the single space each is rendered with.
const BOARD_WIDTH: usize = MARKER_WIDTH
    + NAME_WIDTH
    + 1
    + BRANCH_WIDTH
    + 1
    + STATS_WIDTH
    + 1
    + AGE_WIDTH
    + 1
    + AGENT_WIDTH;
/// A worktree with no branch checked out.
const DETACHED: &str = "(detached)";

/// The board's column widths for one draw. Rows and expanded rows want
/// different widths — a long branch, a long commit summary — so the
/// panel takes the larger of the two, capped by [`ceiling`], and the
/// columns divide up what that leaves.
struct Widths {
    /// The panel itself.
    panel: u16,
    /// First column: a worktree's directory name, or — on the branch
    /// axis, which has no second column — the whole ref.
    name: usize,
    /// Second column: the branch a worktree has checked out. Zero on
    /// the branch axis.
    branch: usize,
    /// A scope or commit label, before its count column.
    label: usize,
}

impl Widths {
    fn measure(board: &WorktreeBoard, max_width: u16, area_width: u16) -> Widths {
        let rows = board.rows();
        let names = rows
            .iter()
            .map(|row| row.label.chars().count())
            .max()
            .unwrap_or(0);
        let branches = rows
            .iter()
            .map(|row| row.branch.as_deref().unwrap_or(DETACHED).chars().count())
            .max()
            .unwrap_or(0);
        let summaries = board
            .lines
            .iter()
            .filter_map(|line| match line {
                BoardLine::Scope { label, .. } => Some(label.chars().count()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        // The branch axis has no second column — the whole ref goes in
        // the first — so both floors, and the space between them, fall
        // to the name.
        let (name_floor, branch_floor, gap, branch_want) = match board.axis {
            BoardAxis::Worktrees => (NAME_WIDTH, BRANCH_WIDTH, 1, branches),
            BoardAxis::Branches => (NAME_WIDTH + 1 + BRANCH_WIDTH, 0, 0, 0),
        };
        // What each kind of line asks for; the wider one sets the panel.
        let fixed = MARKER_WIDTH + 1 + gap + STATS_WIDTH + 1 + AGE_WIDTH + 1 + AGENT_WIDTH;
        let panel = (fixed + names.max(name_floor) + branch_want.max(branch_floor))
            .max(SCOPE_INDENT + summaries + 1 + COUNT_WIDTH)
            .min(max_width as usize)
            .max(BOARD_WIDTH)
            .min(area_width as usize);
        let (name, branch) = share(
            panel.saturating_sub(fixed),
            (name_floor, branch_floor),
            (names, branch_want),
        );
        Widths {
            panel: panel as u16,
            name,
            branch,
            label: panel.saturating_sub(SCOPE_INDENT + 1 + COUNT_WIDTH),
        }
    }
}

/// Divide `room` between the name and branch columns: each starts at its
/// floor, and the slack goes first to the names — the column the eye
/// scans — then to the branches, neither taking more than it asked for.
/// What is left over stays empty rather than padding one column out of
/// line with the counts beside it.
fn share(room: usize, floors: (usize, usize), wants: (usize, usize)) -> (usize, usize) {
    let (mut name, mut branch) = floors;
    let mut slack = room.saturating_sub(name + branch);
    let take = wants.0.saturating_sub(name).min(slack);
    name += take;
    slack -= take;
    branch += wants.1.saturating_sub(branch).min(slack);
    (name, branch)
}

/// How far a picker panel may grow: `[picker]` as a share of the
/// terminal, in columns and in rows the panel's lines may fill. A
/// ceiling only — every picker is sized by what it holds, and stops
/// here rather than swallow the screen.
fn ceiling(app: &App, area: Rect) -> (u16, u16) {
    let portion = |size: u16, percent: u16| (u32::from(size) * u32::from(percent) / 100) as u16;
    let width = portion(area.width, app.picker_size.width).max(1);
    // The caption, and the board's footer separator, come out of the
    // same ceiling as the rows.
    let rows = portion(area.height, app.picker_size.height)
        .saturating_sub(2)
        .max(1);
    (width, rows)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(picker) = app.picker() else { return };
    let theme = &app.theme;
    if let Picker::Board(board) = picker {
        draw_board(frame, app, board);
        return;
    }

    // Rows as (label, is the active choice) pairs, picker-agnostic.
    let (title, items, cursor): (&str, Vec<(&str, bool)>, usize) = match picker {
        // Drawn above; the flat-list path never sees it.
        Picker::Board(_) => return,
        Picker::Base(picker) => (
            "compare against",
            picker
                .branches
                .iter()
                .map(|branch| (branch.as_str(), *branch == app.cmp.base_label))
                .collect(),
            picker.cursor,
        ),
        Picker::Scope(picker) => (
            "review",
            picker
                .entries
                .iter()
                .map(|(scope, label)| (label.as_str(), *scope == app.cmp.scope))
                .collect(),
            picker.cursor,
        ),
        Picker::Pr(picker) => (
            picker.title.as_str(),
            picker
                .rows
                .iter()
                .map(|(label, current)| (label.as_str(), *current))
                .collect(),
            picker.cursor,
        ),
        Picker::Agent(picker) => (
            "send to",
            picker
                .rows
                .iter()
                .map(|(label, current)| (label.as_str(), *current))
                .collect(),
            picker.cursor,
        ),
    };

    let area = frame.area();
    let (max_width, max_rows) = ceiling(app, area);
    // max first: a terminal narrower than the 28-column floor must not
    // invert the clamp bounds (u16 clamp panics on min > max).
    let width = items
        .iter()
        .map(|(label, _)| label.chars().count() as u16 + 10)
        .max()
        .unwrap_or(20)
        .max(28)
        .min(max_width);
    let rows = items.len().min(max_rows as usize);

    // Window the list around the cursor.
    let offset = cursor
        .saturating_sub(rows / 2)
        .min(items.len().saturating_sub(rows));

    let mut lines = vec![caption(app, title.to_string())];
    for (index, (label, current)) in items.iter().enumerate().skip(offset).take(rows) {
        let marker = if *current { "●" } else { " " };
        let mut line = Line::from(vec![
            Span::styled(format!("   {marker} "), Style::default().fg(theme.muted)),
            Span::raw(label.to_string()),
        ]);
        if index == cursor {
            line.style = Style::default()
                .bg(theme.select_bg)
                .add_modifier(Modifier::BOLD);
        }
        lines.push(line);
    }
    draw_panel(frame, theme, lines, width);
}

/// The panel caption: what the list is, plus the `/` query when there
/// is one — with a block cursor while it is still being typed, the
/// same shape the panes' search prompt has.
fn caption(app: &App, title: String) -> Line<'static> {
    let theme = &app.theme;
    let mut spans = vec![Span::styled(
        format!("   {title}"),
        Style::default().fg(theme.muted),
    )];
    if !app.picker_search().is_empty() || app.picker_search_input() {
        let cursor = if app.picker_search_input() { "▌" } else { "" };
        spans.push(Span::styled(
            format!("   /{}{cursor}", app.picker_search()),
            Style::default().fg(theme.dir),
        ));
    }
    Line::from(spans)
}

/// The review board. One line per worktree — name, branch, what it
/// holds, how long ago it moved, and the agent working in it — with an
/// expanded row's scopes and commits indented underneath.
fn draw_board(frame: &mut Frame, app: &App, board: &WorktreeBoard) {
    let theme = &app.theme;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let area = frame.area();
    let (max_width, max_rows) = ceiling(app, area);
    let widths = Widths::measure(board, max_width, area.width);
    let width = widths.panel;
    let rows = board.lines.len().min(max_rows as usize);
    let offset = board
        .cursor
        .saturating_sub(rows / 2)
        .min(board.lines.len().saturating_sub(rows));

    // The caption names the axis on screen and the key to the other
    // one: nothing else would ever tell you the second list exists.
    let mut lines = vec![caption(
        app,
        format!("{} · b {}", board.axis.name(), board.axis.other().name()),
    )];
    // Where the cursored line landed in the panel, and the text the
    // columns clipped off it: the tooltip needs both, and only the loop
    // knows how many separator lines it inserted along the way.
    let mut cursor_row = None;
    for (index, line) in board.lines.iter().enumerate().skip(offset).take(rows) {
        let mut spans = match line {
            BoardLine::Row(row) => row_spans(app, board.axis, &board.rows()[*row], now, &widths),
            BoardLine::Scope { row, scope, label } => {
                scope_spans(app, &board.rows()[*row], scope, label, widths.label)
            }
            BoardLine::Action(action) => vec![
                Span::styled("     ".to_string(), Style::default()),
                Span::styled(
                    match action {
                        BoardAction::Base => "compare against…".to_string(),
                        BoardAction::Pr => format!("{}s…", app.noun()),
                    },
                    Style::default().fg(theme.muted),
                ),
            ],
        };
        // A separator above the footer keeps the rare axes visibly
        // apart from the worktrees without costing a line.
        if matches!(line, BoardLine::Action(BoardAction::Base)) {
            lines.push(Line::styled(
                format!("   {}", "─".repeat(width.saturating_sub(6) as usize)),
                Style::default().fg(theme.muted),
            ));
        }
        let mut drawn = Line::from(std::mem::take(&mut spans));
        if index == board.cursor {
            drawn.style = Style::default()
                .bg(theme.select_bg)
                .add_modifier(Modifier::BOLD);
            cursor_row = clipped_text(board.axis, line, board.rows(), &widths)
                .map(|text| (lines.len() as u16, text));
        }
        lines.push(drawn);
    }
    let panel = draw_panel(frame, theme, lines, width);
    if let Some((row, text)) = cursor_row {
        draw_tooltip(frame, app, &text, panel, panel.y + row);
    }
}

/// The full text of a board line, when the columns it renders into are
/// too narrow to hold it — `None` when it fits, so the board only shows
/// a tooltip for what it actually had to clip.
fn clipped_text(
    axis: BoardAxis,
    line: &BoardLine,
    rows: &[BoardRow],
    widths: &Widths,
) -> Option<String> {
    match line {
        BoardLine::Scope { label, .. } => {
            (label.chars().count() > widths.label).then(|| label.clone())
        }
        BoardLine::Row(row) => {
            let row = &rows[*row];
            let name = row.label.chars().count();
            match axis {
                BoardAxis::Worktrees => {
                    let branch = row.branch.as_deref().unwrap_or(DETACHED);
                    (name > widths.name || branch.chars().count() > widths.branch)
                        .then(|| format!("{}  {branch}", row.label))
                }
                BoardAxis::Branches => (name > widths.name).then(|| row.label.clone()),
            }
        }
        BoardLine::Action(_) => None,
    }
}

/// A one-line overlay under the cursored board line with its full text,
/// the same answer to a clipped row the tree pane gives. Painted a step
/// above both the panel and the cursor row it hangs under — the tree's
/// trick of using the panel color only reads over the terminal's own
/// ground — and free to run past the panel's edge, the text being the
/// whole point of it.
fn draw_tooltip(frame: &mut Frame, app: &App, text: &str, panel: Rect, row_y: u16) {
    let text = format!(" {text} ");
    let area = frame.area();
    // At least the panel's width: a tooltip cut to its text leaves the
    // tail of the row it covers peeking out beside it.
    let width = (text.chars().count() as u16)
        .max(panel.width)
        .min(area.width.saturating_sub(panel.x));
    // Below the cursored line, or above it when that would fall off the
    // panel's last row into the status bar.
    let y = if row_y + 1 < area.height.saturating_sub(1) {
        row_y + 1
    } else {
        row_y.saturating_sub(1)
    };
    let tooltip = Rect {
        x: panel.x,
        y,
        width,
        height: 1,
    };
    frame.render_widget(Clear, tooltip);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().bg(app.theme.tooltip_bg)),
        tooltip,
    );
}

fn row_spans(
    app: &App,
    axis: BoardAxis,
    row: &BoardRow,
    now: i64,
    widths: &Widths,
) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let marker = if row.current { "●" } else { " " };
    let caret = if row.expanded { "▾" } else { "▸" };
    // A branch row has no second column, so its name gets both: branch
    // names are long, and the tail is what distinguishes them.
    let (name, branch) = match axis {
        BoardAxis::Worktrees => (
            row.label.chars().take(widths.name).collect::<String>(),
            Some(
                row.branch
                    .as_deref()
                    .map(|branch| fit_branch(branch, widths.branch))
                    .unwrap_or_else(|| DETACHED.to_string()),
            ),
        ),
        BoardAxis::Branches => (fit_branch(&row.label, widths.name), None),
    };

    // Counts only once the scan has answered; a blank column reads as
    // "still counting", never as "nothing here".
    let stats = match row.stats {
        Some(stats) => {
            let uncommitted = if stats.uncommitted > 0 {
                format!(" ~{}", stats.uncommitted)
            } else {
                String::new()
            };
            format!("+{}{}", stats.commits, uncommitted)
        }
        None => String::new(),
    };

    let mut spans = vec![
        Span::styled(
            format!(" {marker} {caret} "),
            // The blue the branch column already uses: the marker says
            // "this is the one you are on", the same thing the theme's
            // directory color says everywhere else in drift. Green is
            // left to mean added lines, and to the agent dot below.
            Style::default().fg(if row.current { theme.dir } else { theme.muted }),
        ),
        Span::raw(format!("{name:<0$} ", widths.name)),
        Span::styled(
            match &branch {
                Some(branch) => format!("{branch:<0$} ", widths.branch),
                None => String::new(),
            },
            Style::default().fg(theme.dir),
        ),
        Span::styled(
            format!("{stats:<STATS_WIDTH$} "),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            format!("{:>AGE_WIDTH$} ", ago(now, row.last_commit)),
            Style::default().fg(theme.muted),
        ),
    ];
    if let Some((name, status)) = &row.agent {
        // Only a working agent gets the filled dot: the point of the
        // column is spotting the one that is moving right now.
        let (dot, color) = match status.as_str() {
            "working" => ("●", theme.added),
            "" => (" ", theme.muted),
            _ => ("○", theme.muted),
        };
        spans.push(Span::styled(
            format!("{dot} {name}"),
            Style::default().fg(color),
        ));
        if !status.is_empty() {
            spans.push(Span::styled(
                format!(" {status}"),
                Style::default().fg(theme.muted),
            ));
        }
    }
    spans
}

fn scope_spans(
    app: &App,
    row: &BoardRow,
    scope: &crate::vcs::model::Scope,
    label: &str,
    label_width: usize,
) -> Vec<Span<'static>> {
    use crate::vcs::model::Scope;
    let theme = &app.theme;
    // The dot marks what drift is showing now — only meaningful on the
    // worktree it is actually showing.
    let current = row.current && *scope == app.cmp.scope;
    let count = match (scope, row.stats) {
        (Scope::Uncommitted, Some(stats)) => stats.uncommitted.to_string(),
        (Scope::Committed, Some(stats)) => format!("{} commits", stats.commits),
        _ => String::new(),
    };
    // Clip rather than let the panel decide, so the count column stays
    // where the eye expects it; the tooltip has the rest.
    let label = fit_summary(label, label_width);
    vec![
        Span::styled(
            format!("   {}   ", if current { "●" } else { " " }),
            Style::default().fg(theme.dir),
        ),
        Span::raw(format!("{label:<label_width$} ")),
        Span::styled(count, Style::default().fg(theme.muted)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slack_goes_to_the_names_then_the_branches() {
        // Both columns fit: each is as wide as its content.
        assert_eq!(share(100, (22, 26), (30, 40)), (30, 40));
        // Only some of the slack to hand out: names take theirs first,
        // branches what is left.
        assert_eq!(share(60, (22, 26), (30, 40)), (30, 30));
        // None to hand out — and none to take back: the floors keep the
        // counts beside them aligned, and the tooltip covers the rest.
        assert_eq!(share(40, (22, 26), (30, 40)), (22, 26));
        // A column asking for less than its floor never shrinks it.
        assert_eq!(share(100, (22, 26), (4, 8)), (22, 26));
    }
}
