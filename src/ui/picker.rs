//! The picker overlays: the review board (every worktree of the repo,
//! expandable into its scopes and commits), and the flat lists it
//! chains into — base branches, pull requests, agent targets. Enter
//! selects, Esc cancels.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::picker::{ago, fit_branch};
use crate::app::{App, BoardAction, BoardAxis, BoardLine, Picker, WorktreeBoard};
use crate::ui::draw_panel;

/// Widths of the board's fixed columns. The branch column gets what is
/// left, so a narrow terminal shortens branches rather than dropping
/// the counts that say whether a worktree is worth opening.
const NAME_WIDTH: usize = 22;
const BRANCH_WIDTH: usize = 26;
const STATS_WIDTH: usize = 11;
const AGE_WIDTH: usize = 4;

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
    // max first: a terminal narrower than the 28-column floor must not
    // invert the clamp bounds (u16 clamp panics on min > max).
    let width = items
        .iter()
        .map(|(label, _)| label.chars().count() as u16 + 10)
        .max()
        .unwrap_or(20)
        .max(28)
        .min(area.width);
    let rows = items.len().min(area.height.saturating_sub(6) as usize);

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

    let width = (NAME_WIDTH + BRANCH_WIDTH + STATS_WIDTH + AGE_WIDTH + 26) as u16;
    let area = frame.area();
    let width = width.min(area.width);
    let rows = board
        .lines
        .len()
        .min(area.height.saturating_sub(6).max(1) as usize);
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
    for (index, line) in board.lines.iter().enumerate().skip(offset).take(rows) {
        let mut spans = match line {
            BoardLine::Row(row) => row_spans(app, board.axis, &board.rows()[*row], now),
            BoardLine::Scope { row, scope, label } => {
                scope_spans(app, &board.rows()[*row], scope, label)
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
        }
        lines.push(drawn);
    }
    draw_panel(frame, theme, lines, width);
}

fn row_spans(
    app: &App,
    axis: BoardAxis,
    row: &crate::app::picker::BoardRow,
    now: i64,
) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let marker = if row.current { "●" } else { " " };
    let caret = if row.expanded { "▾" } else { "▸" };
    // A branch row has no second column, so its name gets both: branch
    // names are long, and the tail is what distinguishes them.
    let (name, branch) = match axis {
        BoardAxis::Worktrees => (
            row.label.chars().take(NAME_WIDTH).collect::<String>(),
            Some(
                row.branch
                    .as_deref()
                    .map(|branch| fit_branch(branch, BRANCH_WIDTH))
                    .unwrap_or_else(|| "(detached)".to_string()),
            ),
        ),
        BoardAxis::Branches => (fit_branch(&row.label, NAME_WIDTH + BRANCH_WIDTH), None),
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
        match &branch {
            Some(_) => Span::raw(format!("{name:<NAME_WIDTH$} ")),
            None => Span::raw(format!("{name:<0$} ", NAME_WIDTH + BRANCH_WIDTH + 1)),
        },
        Span::styled(
            match &branch {
                Some(branch) => format!("{branch:<BRANCH_WIDTH$} "),
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
    row: &crate::app::picker::BoardRow,
    scope: &crate::vcs::model::Scope,
    label: &str,
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
    // Commit summaries are long; clip rather than let the panel decide,
    // so the count column stays where the eye expects it.
    const LABEL_WIDTH: usize = 44;
    let label: String = match label.chars().count() > LABEL_WIDTH {
        true => label.chars().take(LABEL_WIDTH - 1).chain(['…']).collect(),
        false => label.to_string(),
    };
    vec![
        Span::styled(
            format!("   {}   ", if current { "●" } else { " " }),
            Style::default().fg(theme.dir),
        ),
        Span::raw(format!("{label:<LABEL_WIDTH$} ")),
        Span::styled(count, Style::default().fg(theme.muted)),
    ]
}
