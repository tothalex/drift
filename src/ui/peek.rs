//! The peek view: while open it replaces the code pane with the
//! new-side text of the block under the cursor — no deletions, just how
//! the code reads now. The scope keys walk the chain of enclosing
//! blocks; new lines keep a gutter accent so the change stays findable
//! in the clean text. Lines wrap and the cursor centers exactly like
//! the diff view.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::keymap::Action;
use crate::lang::lang_name;
use crate::ui::{CODE_GUTTER, diff_view, header_style};

pub fn draw(frame: &mut Frame, app: &App, header: Rect, content: Rect) {
    let Some(peek) = app.peek() else { return };
    let theme = &app.theme;
    let lang = app
        .current_file()
        .and_then(|f| lang_name(&f.path))
        .and_then(|name| theme.for_lang(name));
    let dim = Style::default().fg(theme.muted);

    let (level, of) = peek.level_of();
    let title = app
        .current_file()
        .map(|f| f.path.display().to_string())
        .unwrap_or_default();
    let mut header_spans = vec![
        Span::styled(title, header_style(theme, true)),
        Span::styled("  ▸ ".to_string(), dim),
        Span::styled(
            peek.block().title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if of > 1 {
        header_spans.push(Span::styled(format!(" · {level}/{of}"), dim));
    }
    let mut hints = vec![format!("{} close", app.keymap.primary_key(Action::Peek))];
    if of > 1 {
        hints.push(format!(
            "{} widen · {} narrow",
            app.keymap.primary_key(Action::ScopeWiden),
            app.keymap.primary_key(Action::ScopeNarrow)
        ));
    }
    header_spans.push(Span::styled(format!("   {}", hints.join(" · ")), dim));
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header);

    let block_start = peek.start();
    let lines = peek.block_lines();
    let cursor =
        (peek.cursor.saturating_sub(block_start) as usize).min(lines.len().saturating_sub(1));
    let height = content.height as usize;
    let width = content.width as usize;
    let gutter = CODE_GUTTER as usize;

    let build = |i: usize| {
        let line = &lines[i];
        let accent = if line.changed {
            Style::default().fg(theme.added)
        } else {
            dim
        };
        let bar = if line.changed { "▎" } else { " " };
        let mut parts = vec![
            Span::styled(bar.to_string(), accent),
            Span::styled(format!("{:>4} ", block_start + i as u32), accent),
        ];
        parts.extend(diff_view::render_content(
            theme,
            lang,
            &line.content,
            &line.spans,
            &[],
            Color::Reset,
            None,
            None,
        ));
        let mut row = Line::from(parts);
        if i == cursor {
            row.style = Style::default().bg(theme.cursor_bg);
        }
        row
    };

    // The same window walk as the diff view: keep the cursor centered
    // by wrapped heights, easing off at the block's edges so the pane
    // stays full.
    let mut top = cursor;
    let mut above = 0;
    while top > 0 {
        let h = diff_view::wrap_line(build(top - 1), width, gutter).len();
        if above + h > height / 2 {
            break;
        }
        top -= 1;
        above += h;
    }
    let mut rows = Vec::with_capacity(height);
    let mut index = top;
    while index < lines.len() && rows.len() < height {
        for (row, _) in diff_view::wrap_line(build(index), width, gutter) {
            if rows.len() == height {
                break;
            }
            rows.push(row);
        }
        index += 1;
    }
    while rows.len() < height && top > 0 {
        top -= 1;
        for (at, (row, _)) in diff_view::wrap_line(build(top), width, gutter)
            .into_iter()
            .enumerate()
        {
            rows.insert(at, row);
        }
    }
    if rows.len() > height {
        let cut = rows.len() - height;
        rows.drain(..cut);
    }
    frame.render_widget(Paragraph::new(rows), content);
}
