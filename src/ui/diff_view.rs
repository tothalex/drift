use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use std::path::Path;

use crate::app::{ActionSpot, App, Pane};
use crate::processor::comments;
use crate::processor::highlight::{HighlightSpan, TokenKind};
use crate::processor::treesitter::lang_name;
use crate::processor::view::{FileView, FlatLine, ViewLine, char_to_byte};
use crate::theme::Theme;
use crate::ui::{header_style, search_range};
use crate::vcs::model::{DiffLine, LineKind};

pub fn draw(frame: &mut Frame, app: &mut App, header: Rect, content: Rect) {
    let theme = &app.theme;
    // Per-language syntax overrides for the shown file, resolved once.
    let lang = app
        .current_file()
        .and_then(|f| lang_name(&f.path))
        .and_then(|name| theme.for_lang(name));
    let title = app
        .current_file()
        .map(|f| f.path.display().to_string())
        .unwrap_or_else(|| "no changes".to_string());

    let title_style = header_style(theme, app.focused_pane() == Pane::Code);
    let mut header_spans = vec![Span::styled(title, title_style)];
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
    // The in-flight forge mutation's target row gets the spinner, right
    // where the user acted.
    let action = app.action_spot().map(|(spot, frame)| (spot.clone(), frame));
    let current_path = app.current_file().map(|f| f.path.clone());
    // The code pane's own search query highlights on every matching row.
    let search = (!app.code_search().is_empty()).then(|| app.code_search().to_lowercase());

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
                let marked = action
                    .as_ref()
                    .is_some_and(|(spot, _)| spot_marks(spot, &flat, current_path.as_deref()));
                let mut line = match flat {
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
                    FlatLine::Line(ViewLine::CommentHead {
                        author,
                        date,
                        replies,
                        resolved,
                        collapsed,
                        ..
                    }) => render_comment_head(theme, author, date, *replies, *resolved, *collapsed),
                    FlatLine::Line(ViewLine::CommentBody { text, .. }) => Line::from(vec![
                        Span::styled("      ┃ ", Style::default().fg(theme.thread)),
                        Span::styled(
                            text.clone(),
                            Style::default().add_modifier(Modifier::ITALIC),
                        ),
                    ]),
                    FlatLine::Line(ViewLine::CommentHint { text, .. }) => Line::from(vec![
                        Span::styled("      ┃ ", Style::default().fg(theme.thread)),
                        Span::styled(
                            format!("↳ {text}"),
                            Style::default()
                                .fg(theme.muted)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ]),
                    FlatLine::Line(ViewLine::Diff {
                        line,
                        spans,
                        emph,
                        comment,
                    }) => {
                        let sel = mouse_sel_range(mouse_sel, index, &line.content);
                        // Comment-only lines read as prose (flag
                        // precomputed by the processor) and highlight
                        // search hits within the prose; code lines get
                        // the byte-exact renderer.
                        if sel.is_none() && *comment {
                            render_comment_line(theme, line, search.as_deref())
                        } else {
                            let hit = search
                                .as_deref()
                                .and_then(|query| search_range(&line.content, query));
                            render_diff_line(theme, lang, line, spans, emph, sel, hit)
                        }
                    }
                };
                if marked && let Some((_, frame)) = &action {
                    line.push_span(Span::styled(
                        format!("  {frame}"),
                        Style::default().fg(theme.thread),
                    ));
                }
                style_row(index, line)
            })
            .collect(),
    };

    frame.render_widget(Paragraph::new(lines), content);
}

/// Does the in-flight mutation's spot land on this row? Diff lines also
/// need the shown file to match — line numbers repeat across files.
fn spot_marks(spot: &ActionSpot, flat: &FlatLine, path: Option<&Path>) -> bool {
    match (spot, flat) {
        (
            ActionSpot::DiffLine { path: p, old, new },
            FlatLine::Line(ViewLine::Diff { line, .. }),
        ) => Some(p.as_path()) == path && line.old_lineno == *old && line.new_lineno == *new,
        (
            ActionSpot::ThreadHint { key },
            FlatLine::Line(ViewLine::CommentHint { key: row_key, .. }),
        ) => row_key == key,
        (
            ActionSpot::CommentHead { id },
            FlatLine::Line(ViewLine::CommentHead { id: row_id, .. }),
        ) => !id.is_empty() && row_id == id,
        (ActionSpot::ConversationHint, FlatLine::Line(ViewLine::CommentHint { key, .. })) => {
            key.is_empty()
        }
        _ => false,
    }
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

/// The head row of a review thread or conversation entry: author, date,
/// hidden-reply count when collapsed, and resolution state.
fn render_comment_head(
    theme: &Theme,
    author: &str,
    date: &str,
    replies: usize,
    resolved: Option<bool>,
    collapsed: bool,
) -> Line<'static> {
    let accent = Style::default().fg(theme.thread);
    let dim = Style::default().fg(theme.muted);
    let mut parts = vec![
        Span::styled("      ┃ ".to_string(), accent),
        Span::styled("● ".to_string(), accent),
        Span::styled(author.to_string(), accent.add_modifier(Modifier::BOLD)),
    ];
    if !date.is_empty() {
        parts.push(Span::styled(format!(" · {date}"), dim));
    }
    if collapsed {
        let replies = match replies {
            0 => String::new(),
            1 => " · 1 reply".to_string(),
            n => format!(" · {n} replies"),
        };
        parts.push(Span::styled(format!("{replies} ⋯"), dim));
    }
    match resolved {
        Some(true) => parts.push(Span::styled(
            " · resolved".to_string(),
            Style::default().fg(theme.added),
        )),
        Some(false) => parts.push(Span::styled(" · unresolved".to_string(), dim)),
        None => {}
    }
    Line::from(parts)
}

/// A comment-only line as prose: gutter (with the diff accent when the
/// line was added/removed), original indent, a quote bar in place of the
/// comment markers, and an amber review tag when it starts with one. A
/// search hit highlights within the prose, keeping the prose rendering.
fn render_comment_line(theme: &Theme, line: &DiffLine, query_lower: Option<&str>) -> Line<'static> {
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
    if let Some((start, end)) = query_lower.and_then(|query| search_range(text, query)) {
        parts.push(Span::styled(text[..start].to_string(), prose));
        parts.push(Span::styled(
            text[start..end].to_string(),
            prose.fg(theme.search).add_modifier(Modifier::BOLD),
        ));
        parts.push(Span::styled(text[end..].to_string(), prose));
        return Line::from(parts);
    }
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

#[allow(clippy::too_many_arguments)]
fn render_diff_line(
    theme: &Theme,
    lang: Option<&HashMap<String, Color>>,
    line: &DiffLine,
    spans: &[HighlightSpan],
    emph: &[(usize, usize)],
    sel: Option<(usize, usize)>,
    search: Option<(usize, usize)>,
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
        lang,
        &line.content,
        spans,
        emph,
        emph_bg,
        sel,
        search,
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

/// Split `content` into segments along the syntax spans (foreground),
/// the emphasis ranges (background on the exact changed bytes), and the
/// search hit (accented foreground).
#[allow(clippy::too_many_arguments)]
fn render_content(
    theme: &Theme,
    lang: Option<&HashMap<String, Color>>,
    content: &str,
    spans: &[HighlightSpan],
    emph: &[(usize, usize)],
    emph_bg: Color,
    sel: Option<(usize, usize)>,
    search: Option<(usize, usize)>,
) -> Vec<Span<'static>> {
    let mut bounds: Vec<usize> = Vec::with_capacity(spans.len() * 2 + emph.len() * 2 + 6);
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
    for range in [sel, search].into_iter().flatten() {
        bounds.push(range.0.min(content.len()));
        bounds.push(range.1.min(content.len()));
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
            style = style.patch(token_style(theme, lang, span.token));
        }
        if emph.iter().any(|&(s, e)| s <= start && end <= e) {
            style = style.bg(emph_bg);
        }
        // The search hit overrides syntax color…
        if search.is_some_and(|(s, e)| s <= start && end <= e) {
            style = style.fg(theme.search).add_modifier(Modifier::BOLD);
        }
        // …and the mouse selection paints over everything else.
        if sel.is_some_and(|(s, e)| s <= start && end <= e) {
            style = style.bg(theme.select_bg);
        }
        parts.push(Span::styled(text.to_string(), style));
    }
    parts
}

/// One Dark syntax palette (onedarkpro's `onedark_dark` hex values), soft
/// enough that the green/red change accents stay the loudest signal.
/// `lang` carries the shown file's `[theme.<lang>]` overrides.
fn token_style(theme: &Theme, lang: Option<&HashMap<String, Color>>, token: TokenKind) -> Style {
    let (key, base) = match token {
        TokenKind::Keyword => ("keyword", theme.keyword),
        TokenKind::Function => ("function", theme.function),
        TokenKind::Type => ("type", theme.type_),
        TokenKind::String => ("string", theme.string),
        TokenKind::Number | TokenKind::Constant => ("number", theme.number),
        TokenKind::Property => ("property", theme.property),
        TokenKind::Variable => ("variable", theme.variable),
        TokenKind::Attribute => ("attribute", theme.attribute),
        TokenKind::Comment => ("comment", theme.comment),
        TokenKind::Operator => ("operator", theme.operator),
        TokenKind::Arrow => ("arrow", theme.arrow),
        TokenKind::Bracket => ("bracket", theme.bracket),
        TokenKind::CallBracket => ("bracket_call", theme.bracket_call),
        TokenKind::Punctuation => ("punctuation", theme.punctuation),
    };
    let color = lang.and_then(|m| m.get(key)).copied().unwrap_or(base);
    let style = Style::default().fg(color);
    match token {
        TokenKind::Comment => style.add_modifier(Modifier::ITALIC),
        _ => style,
    }
}

fn lineno(no: Option<u32>) -> String {
    no.map(|n| n.to_string()).unwrap_or_default()
}
