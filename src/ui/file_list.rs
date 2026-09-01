use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Pane};
use crate::theme::Theme;
use crate::tree::NodeKind;
use crate::ui::{header_style, icons, search_range};
use crate::vcs::model::FileStatus;

pub fn draw(frame: &mut Frame, app: &App, header: Rect, content: Rect) {
    let theme = &app.theme;
    let progress = match app.checked_count() {
        0 => format!("files ({})", app.files.len()),
        done => format!("files ({done}/{} reviewed)", app.files.len()),
    };
    frame.render_widget(
        Paragraph::new(progress).style(header_style(theme, app.focused_pane() == Pane::Tree)),
        header,
    );

    // Rendered manually (not a stateful List) so the tree can free-scroll
    // without the widget snapping back to keep the selection visible.
    let rows: Vec<Line> = app
        .nav
        .tree
        .rows()
        .enumerate()
        .skip(app.nav.offset())
        .take(content.height as usize)
        .map(|(row, node)| {
            let indent = "  ".repeat(node.depth);
            let label = |base: Style| label_spans(node.label.clone(), base, app);
            let mut line = match &node.kind {
                // The PR session's virtual conversation entry.
                NodeKind::File { index, .. } if app.is_pr_conversation(*index) => {
                    let accent = Style::default().fg(theme.thread);
                    let spans = vec![
                        Span::styled(format!("{indent}# "), accent),
                        Span::styled(app.pr_conversation_label(), Style::default()),
                    ];
                    Line::from(spans)
                }
                NodeKind::Dir { expanded, .. } => {
                    let mut spans = vec![Span::styled(
                        format!("{indent}{} ", if *expanded { '▾' } else { '▸' }),
                        Style::default().fg(theme.muted),
                    )];
                    if app.icons {
                        spans.push(Span::styled(
                            format!(
                                "{} ",
                                if *expanded {
                                    icons::DIR_OPEN
                                } else {
                                    icons::DIR_CLOSED
                                }
                            ),
                            Style::default().fg(theme.dir),
                        ));
                    }
                    spans.extend(label(
                        Style::default().fg(theme.dir).add_modifier(Modifier::BOLD),
                    ));
                    // ls -F style trailing slash: the folder cue that
                    // survives themes without a distinct dir color.
                    spans.push(Span::styled("/", Style::default().fg(theme.muted)));
                    Line::from(spans)
                }
                NodeKind::File { index, .. } if app.is_checked(*index) => {
                    // Reviewed: the whole row recedes behind a checkmark.
                    let dim = Style::default().fg(theme.muted);
                    let mut spans = vec![Span::styled(format!("{indent}✓ "), dim)];
                    if app.icons {
                        spans.push(Span::styled(
                            format!("{} ", icons::file(&node.label).0),
                            dim,
                        ));
                    }
                    spans.extend(label(dim));
                    Line::from(spans)
                }
                NodeKind::File { status, .. } => {
                    let mut spans = vec![Span::styled(
                        format!("{indent}{} ", status.letter()),
                        Style::default().fg(status_color(theme, *status)),
                    )];
                    if app.icons {
                        let (glyph, color) = icons::file(&node.label);
                        spans.push(Span::styled(
                            format!("{glyph} "),
                            Style::default().fg(color),
                        ));
                    }
                    spans.extend(label(Style::default()));
                    Line::from(spans)
                }
            };
            if row == app.nav.cursor {
                line.style = Style::default()
                    .bg(theme.tree_cursor_bg)
                    .add_modifier(Modifier::BOLD);
            }
            line
        })
        .collect();

    frame.render_widget(Paragraph::new(rows), content);
}

/// A one-line overlay under the tree cursor with the node's full
/// repo-relative path — shown only while the tree pane has focus and
/// the cursored row is clipped by the pane edge. Pure rendering: keys
/// and mouse hit-testing are untouched.
pub fn draw_path_tooltip(frame: &mut Frame, app: &App) {
    if app.focused_pane() != Pane::Tree {
        return;
    }
    let area = app.layout.tree_area;
    let cursor = app.nav.cursor;
    let offset = app.nav.offset();
    if cursor < offset || cursor >= offset + area.height as usize {
        return; // the cursored row is scrolled out of view
    }
    let Some(node) = app.nav.tree.row(cursor) else {
        return;
    };
    // The PR session's virtual conversation entry has no real path.
    let conversation = matches!(node.kind, crate::tree::NodeKind::File { index, .. }
        if app.is_pr_conversation(index));
    let (label, path) = if conversation {
        let label = app.pr_conversation_label();
        (label.clone(), label)
    } else {
        (node.label.clone(), node.path.clone())
    };
    // Rows render as indent + a two-cell marker + the label, plus a
    // trailing slash on directories and a two-cell icon when enabled
    // (the conversation entry never gets one).
    let slash = usize::from(matches!(node.kind, crate::tree::NodeKind::Dir { .. }));
    let icon = usize::from(app.icons && !conversation) * 2;
    let row_width = node.depth * 2 + 2 + icon + label.as_str().width() + slash;
    if row_width <= area.width as usize {
        return; // the name fits — stay quiet
    }

    let text = format!(" {path} ");
    let width = (text.as_str().width() as u16).min(frame.area().width.saturating_sub(area.x));
    let row_y = area.y + (cursor - offset) as u16;
    // Below the cursored row; above it when that would hit the status
    // bar on the terminal's last row.
    let y = if row_y + 1 < frame.area().height.saturating_sub(1) {
        row_y + 1
    } else {
        row_y.saturating_sub(1)
    };
    let panel = Rect {
        x: area.x,
        y,
        width,
        height: 1,
    };
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().bg(app.theme.panel_bg)),
        panel,
    );
}

/// A row label, with the tree query's match highlighted within it.
fn label_spans(label: String, base: Style, app: &App) -> Vec<Span<'static>> {
    let query = app.tree_search().to_lowercase();
    if !query.is_empty()
        && let Some((start, end)) = search_range(&label, &query)
    {
        return vec![
            Span::styled(label[..start].to_string(), base),
            Span::styled(
                label[start..end].to_string(),
                base.fg(app.theme.search).add_modifier(Modifier::BOLD),
            ),
            Span::styled(label[end..].to_string(), base),
        ];
    }
    vec![Span::styled(label, base)]
}

/// Modified is the expected state in a diff tool and stays muted; only the
/// notable states carry color.
fn status_color(theme: &Theme, status: FileStatus) -> Color {
    match status {
        FileStatus::Added | FileStatus::Untracked => theme.added,
        FileStatus::Modified => theme.muted,
        FileStatus::Deleted => theme.removed,
        FileStatus::Renamed | FileStatus::Copied => theme.renamed,
    }
}
