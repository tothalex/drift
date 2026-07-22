mod compose;
mod diff_view;
mod file_list;
mod help;
mod picker;
mod status_bar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};

use crate::app::App;
use crate::theme::Theme;

/// Columns before the code text: accent bar (1) + line number (4) + gap (1).
pub const CODE_GUTTER: u16 = 6;

/// The focused pane's header lights up: cursor keys act there.
fn header_style(theme: &Theme, focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    }
}

/// Draw a centered floating panel over the app: clear the area, paint
/// the shared panel background. All overlays center identically.
fn draw_panel(frame: &mut Frame, theme: &Theme, lines: Vec<Line<'static>>, width: u16) {
    let area = frame.area();
    let width = width.min(area.width);
    let height = (lines.len() as u16 + 1).min(area.height);
    let panel = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.panel_bg)),
        panel,
    );
}

/// First case-insensitive occurrence of `query_lower` in `content`, as a
/// byte range — guarded against lowercasing shifting char boundaries.
fn search_range(content: &str, query_lower: &str) -> Option<(usize, usize)> {
    let start = content.to_lowercase().find(query_lower)?;
    let end = start + query_lower.len();
    content.get(start..end).map(|_| (start, end))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [sidebar, diff] = Layout::horizontal([
        Constraint::Percentage(app.layout.split_percent),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(main);

    let (tree_header, tree_content) = pane_areas(sidebar);
    let (code_header, code_content) = pane_areas(diff);

    // Geometry for mouse hit-testing: pane contents and the divider gap.
    app.layout.tree_area = tree_content;
    app.layout.code_area = code_content;
    app.layout.main_area = main;
    app.layout.divider_x = diff.x.saturating_sub(1);
    // A freshly opened view starts with its cursor on the middle line.
    app.apply_pending_center();

    file_list::draw(frame, app, tree_header, tree_content);
    diff_view::draw(frame, app, code_header, code_content);
    status_bar::draw(frame, app, status);
    if app.help_open() {
        help::draw(frame, app);
    }
    picker::draw(frame, app);
    compose::draw(frame, app);
}

/// Header row + content area of a pane, with a one-column left margin.
fn pane_areas(area: Rect) -> (Rect, Rect) {
    let area = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(1),
        ..area
    };
    let [header, _, content] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    (header, content)
}
