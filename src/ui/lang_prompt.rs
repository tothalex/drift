//! The language-install prompt: a small floating panel offering to
//! install the curated language that could review the shown file.
//! `y` installs in the background, `n` (or Esc) declines for the
//! session; every other key keeps working underneath.

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::ui::draw_panel;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(name) = app.lang_prompt() else {
        return;
    };
    // Other overlays own the screen (and the keys) while open.
    if app.help_open() || app.picker().is_some() {
        return;
    }
    let theme = &app.theme;
    let title = format!("   {name} support is available");
    let width = (title.chars().count() as u16 + 4).max(34);
    let lines = vec![
        Line::styled(title, Style::default().fg(theme.muted)),
        Line::default(),
        Line::from(vec![
            Span::raw("   install the grammar? "),
            Span::styled("y", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("es / ", Style::default().fg(theme.muted)),
            Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("o", Style::default().fg(theme.muted)),
        ]),
        Line::styled(
            "   uses git + a C compiler, ~10s",
            Style::default().fg(theme.muted),
        ),
    ];
    draw_panel(frame, theme, lines, width);
}
