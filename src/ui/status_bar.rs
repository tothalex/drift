use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let comparison = app.comparison_label();
    let mut spans = vec![
        Span::styled(
            comparison,
            Style::default().add_modifier(Modifier::REVERSED),
        ),
        Span::styled(" ? keys · q quit ", Style::default().fg(theme.muted)),
    ];
    if app.code.selection().is_some() {
        spans.push(Span::styled(
            " VISUAL ",
            Style::default()
                .fg(theme.visual_badge_fg)
                .bg(theme.visual_badge_bg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " y copy · esc cancel ",
            Style::default().fg(theme.muted),
        ));
    }
    // The live search prompt while `/` input is active.
    if app.search_input() {
        spans.push(Span::styled(
            format!("  /{}▌", app.search_query()),
            Style::default().fg(theme.search),
        ));
    }
    // The forge spinner animates in front of the working notice while a
    // request is in flight ("⠹ posting comment…").
    if let Some(frame) = app.spinner_frame() {
        spans.push(Span::styled(
            format!("  {frame}"),
            Style::default().fg(theme.thread),
        ));
    }
    if let Some(notice) = app.notice() {
        spans.push(Span::raw(format!("  {notice}")));
    }
    // A pending vim-style count is echoed like vim's cmdline does.
    if let Some(count) = app.pending_count() {
        spans.push(Span::raw(format!("  {count}")));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
