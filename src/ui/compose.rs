//! The comment composer overlay: a centered floating text box with a
//! visible cursor. All wrapping and cursor math lives in
//! [`crate::app::compose::Compose`]; this only paints rows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(compose) = app.compose() else { return };
    let theme = &app.theme;

    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(30, 76);
    // 3 columns of framing on each side, +1 spare cell for the cursor
    // resting after a row's last character.
    let inner = width.saturating_sub(7) as usize;
    let (rows, (cursor_row, cursor_col)) = compose.rows(inner);

    // Window tall bodies around the cursor.
    let max_body = (area.height.saturating_sub(6) as usize).max(1);
    let shown = rows.len().min(max_body).max(3);
    let offset = cursor_row
        .saturating_sub(shown - 1)
        .min(rows.len().saturating_sub(shown));

    let dim = Style::default().fg(theme.muted);
    let accent = Style::default().fg(theme.thread);
    let mut lines = vec![Line::from(vec![
        Span::styled("   ● ", accent),
        Span::styled(compose.title.clone(), accent.add_modifier(Modifier::BOLD)),
    ])];
    for (nth, row) in rows.iter().enumerate().skip(offset).take(shown) {
        let mut parts = vec![Span::styled("   ┃ ", accent)];
        if nth == cursor_row {
            // Paint the cursor cell reversed; it may rest one past the
            // last character.
            let at = crate::processor::view::char_to_byte(row, cursor_col);
            let under: String = row[at..].chars().take(1).collect();
            parts.push(Span::raw(row[..at].to_string()));
            parts.push(Span::styled(
                if under.is_empty() {
                    " ".to_string()
                } else {
                    under.clone()
                },
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            parts.push(Span::raw(row[at + under.len()..].to_string()));
        } else {
            parts.push(Span::raw(row.clone()));
        }
        lines.push(Line::from(parts));
    }
    // Shift+enter needs the kitty keyboard protocol; elsewhere only
    // alt+enter is distinguishable from enter — advertise what works.
    let newline_key = if app.keyboard_enhanced() {
        "shift-enter"
    } else {
        "alt-enter"
    };
    lines.push(Line::from(Span::styled(
        format!("   enter post · {newline_key} newline · esc cancel"),
        dim,
    )));

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
