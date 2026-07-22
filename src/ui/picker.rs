//! The picker overlays: a floating panel listing base branches or, after
//! a branch is chosen, review scopes (all changes, untracked files, one
//! commit). Enter selects, Esc cancels.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, Picker};

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(picker) = app.picker() else { return };
    let theme = &app.theme;

    // Rows as (label, is the active choice) pairs, picker-agnostic.
    let (title, items, cursor): (&str, Vec<(&str, bool)>, usize) = match picker {
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
    };

    let area = frame.area();
    let width = items
        .iter()
        .map(|(label, _)| label.chars().count() as u16 + 10)
        .max()
        .unwrap_or(20)
        .clamp(28, area.width);
    let rows = items.len().min(area.height.saturating_sub(6) as usize);
    let height = (rows as u16 + 3).min(area.height);
    let panel = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    // Window the list around the cursor.
    let offset = cursor
        .saturating_sub(rows / 2)
        .min(items.len().saturating_sub(rows));

    let mut lines = vec![Line::styled(
        format!("   {title}"),
        Style::default().fg(theme.muted),
    )];
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

    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.panel_bg)),
        panel,
    );
}
