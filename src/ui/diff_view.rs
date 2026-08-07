use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use std::path::Path;

use crate::app::{ActionSpot, App, Pane};
use crate::lang::lang_name;
use crate::processor::comments;
use crate::processor::highlight::{HighlightSpan, TokenKind};
use crate::processor::view::{FileView, FlatLine, ViewLine, char_to_byte};
use crate::theme::Theme;
use crate::ui::{CODE_GUTTER, header_style, search_range};
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

    // The window is assembled in visual rows: a line wider than the pane
    // wraps (vim-like) instead of clipping, so one view line can occupy
    // several rows. Walking up from the cursor by wrapped heights keeps
    // it centered, easing off at the start and end of the file so the
    // pane always stays full. Per-keystroke render cost is O(viewport),
    // not O(file) — only lines near the window are ever built.
    let total = app.current_view().map_or(0, FileView::flat_len);
    let height = content.height as usize;
    let width = content.width as usize;
    let cursor = app.code.cursor.min(total.saturating_sub(1));
    let view_offset = app.code.view_offset;

    // Per-row styling: visual selection under the cursorline; the
    // cursorline hides while a mouse selection is in progress. Applied
    // to every visual row of the view line.
    let style_row = |index: usize, mut line: Line<'static>| {
        if selection.is_some_and(|(from, to)| (from..=to).contains(&index)) {
            line.style = line.style.patch(Style::default().bg(theme.select_bg));
        }
        if index == cursor && mouse_sel.is_none() {
            line.style = line.style.patch(Style::default().bg(theme.cursor_bg));
        }
        line
    };

    // rows/row_map are per visual row; row_map records (view line, chars
    // of it consumed before the row) for mouse translation. `offset` is
    // view_offset re-clamped to the content.
    let (rows, row_map, offset): (Vec<Line>, Vec<(usize, usize)>, isize) = match app.current_view()
    {
        None => (Vec::new(), Vec::new(), 0),
        Some(FileView::Binary) => (
            vec![style_row(0, Line::styled(" binary file changed", dim))],
            vec![(0, 0)],
            0,
        ),
        Some(FileView::Unchanged) => (
            vec![style_row(
                0,
                Line::styled(" no content changes (rename or mode change)", dim),
            )],
            vec![(0, 0)],
            0,
        ),
        Some(view @ FileView::Sections { .. }) => {
            let flats: Vec<FlatLine> = view.flat_lines().collect();
            let build = |index: usize, flat: &FlatLine| {
                let marked = action
                    .as_ref()
                    .is_some_and(|(spot, _)| spot_marks(spot, flat, current_path.as_deref()));
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
                line
            };

            let gutter = CODE_GUTTER as usize;
            // Top line: walk up from the cursor until about half the
            // pane's rows sit above it.
            let mut centered = cursor;
            let mut above = 0;
            while centered > 0 {
                let h = wrap_line(build(centered - 1, &flats[centered - 1]), width, gutter).len();
                if above + h > height / 2 {
                    break;
                }
                centered -= 1;
                above += h;
            }
            // Free-scroll (mouse wheel) offsets the centered position.
            let mut top = centered
                .saturating_add_signed(view_offset)
                .min(total.saturating_sub(1));
            let mut rows = Vec::with_capacity(height);
            let mut map = Vec::with_capacity(height);
            let mut index = top;
            while index < total && rows.len() < height {
                for (row, start) in wrap_line(build(index, &flats[index]), width, gutter) {
                    if rows.len() == height {
                        break;
                    }
                    rows.push(style_row(index, row));
                    map.push((index, start));
                }
                index += 1;
            }
            // Ease off at the end of the file: pull the top up until the
            // pane is full again.
            while rows.len() < height && top > 0 {
                top -= 1;
                for (at, (row, start)) in wrap_line(build(top, &flats[top]), width, gutter)
                    .into_iter()
                    .enumerate()
                {
                    rows.insert(at, style_row(top, row));
                    map.insert(at, (top, start));
                }
            }
            // A bottom-anchored pull may overshoot; the pulled-in line
            // then shows only its tail rows.
            if rows.len() > height {
                let cut = rows.len() - height;
                rows.drain(..cut);
                map.drain(..cut);
            }
            (rows, map, top as isize - centered as isize)
        }
    };

    app.code.view_offset = offset;
    app.code.row_map = row_map;
    frame.render_widget(Paragraph::new(rows), content);
}

/// Split a styled line into visual rows of at most `width` cells;
/// continuation rows hang behind a blank `indent`-column prefix so
/// wrapped code stays clear of the gutter. Each row carries the count
/// of the line's chars consumed before it — the renderer's row map
/// translates mouse positions back to content chars with it. Wrapping
/// is display-only: cursor, selections and yanks stay on view lines.
/// Shared with the peek view, which wraps identically.
pub(super) fn wrap_line(
    line: Line<'static>,
    width: usize,
    indent: usize,
) -> Vec<(Line<'static>, usize)> {
    let mut rows = Vec::new();
    if width == 0 {
        rows.push((line, 0));
        return rows;
    }
    let indent = indent.min(width - 1);
    let style = line.style;
    let mut spans: Vec<Span> = Vec::new();
    let mut cells = 0; // cells filled in the current row
    let mut floor = 0; // cells the current row starts with (its indent)
    let mut chars = 0; // chars of the line consumed so far
    let mut start = 0; // chars consumed before the current row
    for span in line.spans {
        let mut buf = String::new();
        for c in span.content.chars() {
            let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            // Break before overflowing — unless the row holds nothing
            // yet, where an over-wide cell is placed rather than looped.
            if cells + w > width && cells > floor {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), span.style));
                }
                let mut row = Line::from(std::mem::take(&mut spans));
                row.style = style;
                rows.push((row, start));
                spans.push(Span::raw(" ".repeat(indent)));
                cells = indent;
                floor = indent;
                start = chars;
            }
            buf.push(c);
            cells += w;
            chars += 1;
        }
        if !buf.is_empty() {
            spans.push(Span::styled(buf, span.style));
        }
    }
    let mut row = Line::from(spans);
    row.style = style;
    rows.push((row, start));
    rows
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
/// search hit (accented foreground). Shared with the peek overlay.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_content(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(rows: &[(Line<'static>, usize)]) -> Vec<String> {
        rows.iter()
            .map(|(line, _)| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn starts(rows: &[(Line<'static>, usize)]) -> Vec<usize> {
        rows.iter().map(|(_, start)| *start).collect()
    }

    #[test]
    fn short_line_stays_one_row() {
        let rows = wrap_line(Line::from("abc"), 10, 6);
        assert_eq!(texts(&rows), vec!["abc"]);
        assert_eq!(starts(&rows), vec![0]);
    }

    #[test]
    fn long_line_wraps_with_hanging_indent() {
        // A 6-cell gutter followed by content, like a diff row.
        let line = Line::from(vec![Span::raw("▎  42 "), Span::raw("abcdefghij")]);
        let rows = wrap_line(line, 10, 6);
        assert_eq!(texts(&rows), vec!["▎  42 abcd", "      efgh", "      ij"]);
        // Row starts count the line's own chars (gutter included), not
        // the injected indent prefixes.
        assert_eq!(starts(&rows), vec![0, 10, 14]);
    }

    #[test]
    fn styles_survive_the_split() {
        let styled = Style::default().fg(Color::Red);
        let line = Line::from(vec![Span::raw("aaaa"), Span::styled("bbbb", styled)]);
        let rows = wrap_line(line, 6, 2);
        assert_eq!(texts(&rows), vec!["aaaabb", "  bb"]);
        // The split span keeps its style on both sides.
        assert_eq!(rows[0].0.spans.last().unwrap().style, styled);
        assert_eq!(rows[1].0.spans.last().unwrap().style, styled);
    }

    #[test]
    fn wide_chars_wrap_by_cells_not_chars() {
        let rows = wrap_line(Line::from("日本語は広い"), 5, 0);
        // Each char is 2 cells: only two fit in 5.
        assert_eq!(texts(&rows), vec!["日本", "語は", "広い"]);
        assert_eq!(starts(&rows), vec![0, 2, 4]);
    }

    #[test]
    fn empty_and_degenerate_widths_yield_one_row() {
        assert_eq!(wrap_line(Line::default(), 10, 6).len(), 1);
        assert_eq!(wrap_line(Line::from("abc"), 0, 6).len(), 1);
        // An indent as wide as the pane still leaves one content cell.
        let rows = wrap_line(Line::from("abcd"), 2, 6);
        assert_eq!(texts(&rows), vec!["ab", " c", " d"]);
    }
}
