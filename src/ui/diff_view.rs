use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::processor::comments;
use crate::processor::highlight::{HighlightSpan, TokenKind};
use crate::processor::view::{FileView, FlatLine, ViewLine, char_to_byte};
use crate::theme::Theme;
use crate::vcs::model::{DiffLine, LineKind};

pub fn draw(frame: &mut Frame, app: &mut App, header: Rect, content: Rect) {
    let theme = &app.theme;
    let title = app
        .current_file()
        .map(|f| f.path.display().to_string())
        .unwrap_or_else(|| "no changes".to_string());

    let mut header_spans = vec![Span::raw(title)];
    if let Some(FileView::Sections {
        diffstat: (adds, dels),
        ..
    }) = app.current_view()
    {
        header_spans.push(Span::styled(
            format!("  +{adds}"),
            Style::default().fg(theme.added),
        ));
        header_spans.push(Span::styled(
            format!(" −{dels}"),
            Style::default().fg(theme.removed),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header);

    let dim = Style::default().fg(theme.muted);
    let mouse_sel = app.code.mouse_selection();
    let selection = app.code.selection();

    // Scroll math first, so only the visible window of lines is ever
    // built — per-keystroke render cost is O(viewport), not O(file).
    // Center-scrolling keeps the cursor line mid-view, easing off at the
    // start and end of the file so the pane always stays full.
    let total = app.current_view().map_or(0, FileView::flat_len);
    let height = content.height as usize;
    let cursor = app.code.cursor.min(total.saturating_sub(1));
    let max_scroll = total.saturating_sub(height);
    let centered = cursor.saturating_sub(height / 2).min(max_scroll);
    // Free-scroll (mouse wheel) offsets the centered position.
    let scroll = (centered as isize + app.code.view_offset).clamp(0, max_scroll as isize) as usize;
    app.code.scroll = scroll;

    // Per-row styling: visual selection under the cursorline; the
    // cursorline hides while a mouse selection is in progress.
    let style_row = |index: usize, mut line: Line<'static>| {
        if selection.is_some_and(|(from, to)| (from..=to).contains(&index)) {
            line.style = line.style.patch(Style::default().bg(theme.select_bg));
        }
        if index == cursor && mouse_sel.is_none() {
            line.style = line.style.patch(Style::default().bg(theme.cursor_bg));
        }
        line
    };

    let lines: Vec<Line> = match app.current_view() {
        None => Vec::new(),
        Some(FileView::Binary) => {
            vec![style_row(0, Line::styled(" binary file changed", dim))]
        }
        Some(FileView::Unchanged) => vec![style_row(
            0,
            Line::styled(" no content changes (rename or mode change)", dim),
        )],
        Some(view @ FileView::Sections { .. }) => view
            .flat_lines()
            .enumerate()
            .skip(scroll)
            .take(height)
            .map(|(index, flat)| {
                let line = match flat {
                    FlatLine::Separator => Line::default(),
                    FlatLine::Line(ViewLine::Collapsed { count }) => Line::styled(
                        format!("       ⋯ {count} unchanged lines"),
                        Style::default()
                            .fg(theme.muted)
                            .add_modifier(Modifier::ITALIC),
                    ),
                    FlatLine::Line(ViewLine::CommentFold { count, summary }) => Line::styled(
                        format!("      ▏ {summary} ⋯ {count} lines"),
                        Style::default()
                            .fg(theme.comment)
                            .add_modifier(Modifier::ITALIC),
                    ),
                    FlatLine::Line(ViewLine::Diff {
                        line,
                        spans,
                        emph,
                        comment,
                    }) => {
                        let sel = mouse_sel_range(mouse_sel, index, &line.content);
                        // Comment-only lines read as prose (flag
                        // precomputed by the processor); added/removed
                        // ones keep their diff gutter.
                        if sel.is_none() && *comment {
                            render_comment_line(theme, line)
                        } else {
                            render_diff_line(theme, line, spans, emph, sel)
                        }
                    }
                };
                style_row(index, line)
            })
            .collect(),
    };

    frame.render_widget(Paragraph::new(lines), content);
}

/// Byte range of `content` covered by the mouse selection on view line
/// `index` (end char inclusive), if any.
fn mouse_sel_range(
    sel: Option<((usize, usize), (usize, usize))>,
    index: usize,
    content: &str,
) -> Option<(usize, usize)> {
    let ((l0, c0), (l1, c1)) = sel?;
    if index < l0 || index > l1 {
        return None;
    }
    let start = if index == l0 {
        char_to_byte(content, c0)
    } else {
        0
    };
    let end = if index == l1 {
        char_to_byte(content, c1 + 1)
    } else {
        content.len()
    };
    (start < end).then_some((start, end))
}

/// A comment-only line as prose: gutter (with the diff accent when the
/// line was added/removed), original indent, a quote bar in place of the
/// comment markers, and an amber review tag when it starts with one.
fn render_comment_line(theme: &Theme, line: &DiffLine) -> Line<'static> {
    let prose = Style::default()
        .fg(theme.comment)
        .add_modifier(Modifier::ITALIC);
    let (bar, accent, number) = gutter_parts(theme, line);
    let number_style = accent.map_or(Style::default().fg(theme.muted), |c| Style::default().fg(c));
    let indent: String = line
        .content
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let text = comments::strip_markers(&line.content);

    let mut parts = vec![
        Span::styled(bar.to_string(), number_style),
        Span::styled(format!("{:>4} ", lineno(number)), number_style),
        Span::raw(indent),
        Span::styled("▏ ".to_string(), Style::default().fg(theme.comment)),
    ];
    match comments::tag_len(text) {
        Some(tag) => {
            parts.push(Span::styled(
                text[..tag].to_string(),
                Style::default().fg(theme.tag),
            ));
            parts.push(Span::styled(text[tag..].to_string(), prose));
        }
        None => parts.push(Span::styled(text.to_string(), prose)),
    }
    Line::from(parts)
}

fn render_diff_line(
    theme: &Theme,
    line: &DiffLine,
    spans: &[HighlightSpan],
    emph: &[(usize, usize)],
    sel: Option<(usize, usize)>,
) -> Line<'static> {
    let (bar, accent, number) = gutter_parts(theme, line);
    let emph_bg = match line.kind {
        LineKind::Added => theme.emph_added_bg,
        LineKind::Removed => theme.emph_removed_bg,
        LineKind::Context => Color::Reset,
    };
    let number_style = accent.map_or(Style::default().fg(theme.muted), |c| Style::default().fg(c));

    let mut parts = vec![
        Span::styled(bar.to_string(), number_style),
        Span::styled(format!("{:>4} ", lineno(number)), number_style),
    ];
    parts.extend(render_content(
        theme,
        &line.content,
        spans,
        emph,
        emph_bg,
        sel,
    ));
    Line::from(parts)
}

/// Accent bar, its color, and the gutter line number for a diff line.
/// Removed lines show their old-side number; the color disambiguates.
fn gutter_parts(theme: &Theme, line: &DiffLine) -> (&'static str, Option<Color>, Option<u32>) {
    match line.kind {
        LineKind::Added => ("▎", Some(theme.added), line.new_lineno),
        LineKind::Removed => ("▎", Some(theme.removed), line.old_lineno),
        LineKind::Context => (" ", None, line.new_lineno),
    }
}

/// Split `content` into segments along both the syntax spans (foreground)
/// and the emphasis ranges (background on the exact changed bytes).
fn render_content(
    theme: &Theme,
    content: &str,
    spans: &[HighlightSpan],
    emph: &[(usize, usize)],
    emph_bg: Color,
    sel: Option<(usize, usize)>,
) -> Vec<Span<'static>> {
    let mut bounds: Vec<usize> = Vec::with_capacity(spans.len() * 2 + emph.len() * 2 + 4);
    bounds.push(0);
    bounds.push(content.len());
    for span in spans {
        bounds.push(span.start.min(content.len()));
        bounds.push(span.end.min(content.len()));
    }
    for &(start, end) in emph {
        bounds.push(start.min(content.len()));
        bounds.push(end.min(content.len()));
    }
    if let Some((start, end)) = sel {
        bounds.push(start.min(content.len()));
        bounds.push(end.min(content.len()));
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut parts = Vec::with_capacity(bounds.len());
    for pair in bounds.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let Some(text) = content.get(start..end) else {
            continue; // not on char boundaries; skip this segment
        };
        let mut style = Style::default();
        if let Some(span) = spans.iter().find(|s| s.start <= start && end <= s.end) {
            style = style.patch(token_style(theme, span.token));
        }
        if emph.iter().any(|&(s, e)| s <= start && end <= e) {
            style = style.bg(emph_bg);
        }
        // The mouse selection paints over everything else.
        if sel.is_some_and(|(s, e)| s <= start && end <= e) {
            style = style.bg(theme.select_bg);
        }
        parts.push(Span::styled(text.to_string(), style));
    }
    parts
}

/// One Dark syntax palette (onedarkpro's `onedark_dark` hex values), soft
/// enough that the green/red change accents stay the loudest signal.
fn token_style(theme: &Theme, token: TokenKind) -> Style {
    match token {
        TokenKind::Keyword => Style::default().fg(theme.keyword),
        TokenKind::Function => Style::default().fg(theme.function),
        TokenKind::Type => Style::default().fg(theme.type_),
        TokenKind::String => Style::default().fg(theme.string),
        TokenKind::Number | TokenKind::Constant => Style::default().fg(theme.number),
        TokenKind::Property => Style::default().fg(theme.property),
        TokenKind::Variable => Style::default().fg(theme.variable),
        TokenKind::Attribute => Style::default().fg(theme.attribute),
        TokenKind::Comment => Style::default()
            .fg(theme.comment)
            .add_modifier(Modifier::ITALIC),
        TokenKind::Operator => Style::default().fg(theme.operator),
        TokenKind::Arrow => Style::default().fg(theme.arrow),
        TokenKind::Bracket => Style::default().fg(theme.bracket),
        TokenKind::Punctuation => Style::default().fg(theme.punctuation),
    }
}

fn lineno(no: Option<u32>) -> String {
    no.map(|n| n.to_string()).unwrap_or_default()
}
