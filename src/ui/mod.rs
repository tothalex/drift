mod diff_view;
mod file_list;
mod help;
mod picker;
mod status_bar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;

/// Columns before the code text: accent bar (1) + line number (4) + gap (1).
pub const CODE_GUTTER: u16 = 6;

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
