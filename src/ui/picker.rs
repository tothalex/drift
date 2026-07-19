//! The base-branch picker: a floating panel listing branches by recent
//! activity. Enter switches the comparison, Esc cancels.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(picker) = app.picker() else { return };
    let theme = &app.theme;

    let area = frame.area();
    let width = picker
        .branches
        .iter()
        .map(|b| b.chars().count() as u16 + 10)
        .max()
        .unwrap_or(20)
        .clamp(28, area.width);
    let rows = picker
        .branches
        .len()
        .min(area.height.saturating_sub(6) as usize);
    let height = (rows as u16 + 3).min(area.height);
    let panel = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    // Window the list around the cursor.
    let offset = picker
        .cursor
        .saturating_sub(rows / 2)
        .min(picker.branches.len().saturating_sub(rows));

    let mut lines = vec![Line::styled(
        "   compare against",
        Style::default().fg(theme.muted),
    )];
    for (index, branch) in picker.branches.iter().enumerate().skip(offset).take(rows) {
        let current = *branch == app.cmp.base_label;
        let marker = if current { "●" } else { " " };
        let mut line = Line::from(vec![
            Span::styled(format!("   {marker} "), Style::default().fg(theme.muted)),
            Span::raw(branch.clone()),
        ]);
        if index == picker.cursor {
            line.style = Style::default()
                .bg(theme.select_bg)
                .add_modifier(Modifier::BOLD);
        }
        lines.push(line);
    }

    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.panel_bg)),
        panel,
    );
}
