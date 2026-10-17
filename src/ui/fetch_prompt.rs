//! The fetch prompt: a small floating panel offering to fetch the open
//! pull request's commits when they aren't in the local clone — without
//! them its files show as plain hunks. `y` fetches in the background,
//! `n` (or Esc) declines for the session; every other key keeps working
//! underneath.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::ui::draw_panel;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(offer) = app.fetch_offer() else {
        return;
    };
    // Other overlays own the screen (and the keys) while open.
    if app.help_open() || app.picker().is_some() {
        return;
    }
    let theme = &app.theme;
    let title = format!("   #{}'s commits aren't in this clone", offer.number);
    let command = format!("   git fetch origin {}", offer.head_ref);
    let width = (title.chars().count().max(command.chars().count()) as u16 + 4).max(44);
    let lines = vec![
        Line::styled(title, Style::default().fg(theme.muted)),
        Line::default(),
        Line::from(vec![
            Span::raw("   fetch them for highlighting? "),
            Span::styled("y", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("es / ", Style::default().fg(theme.muted)),
            Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("o", Style::default().fg(theme.muted)),
        ]),
        Line::styled(command, Style::default().fg(theme.muted)),
    ];
    draw_panel(frame, theme, lines, width);
}
