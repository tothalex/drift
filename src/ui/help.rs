//! The `?` keybinding overlay: a floating panel in the middle of the
//! screen, closed by any key. Borderless like the rest of the UI — a
//! slightly lifted background delineates it. Key rows come from the
//! keymap, so they can't drift from the actual bindings.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::App;

/// Behaviors that aren't single-action bindings.
const EXTRA_ROWS: &[(&str, &str)] = &[
    ("1-9…", "repeat count, vim-style (10j)"),
    ("", ""),
    ("wheel", "scrolls the view; cursor stays put"),
    ("click", "opens a file / folds a folder"),
    ("drag", "select text; copies on release"),
];

pub fn draw(frame: &mut Frame, app: &App) {
    let mut rows: Vec<(String, &str)> = app.keymap.help_rows();
    rows.extend(EXTRA_ROWS.iter().map(|(k, v)| (k.to_string(), *v)));

    let area = frame.area();
    let width = 50.min(area.width);
    let height = (rows.len() as u16 + 2).min(area.height);
    let panel = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    let mut lines = vec![Line::default()];
    for (keys, action) in &rows {
        lines.push(Line::from(vec![
            Span::raw(format!("   {keys:<8} ")),
            Span::styled(action.to_string(), Style::default().fg(app.theme.muted)),
        ]));
    }

    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.panel_bg)),
        panel,
    );
}
