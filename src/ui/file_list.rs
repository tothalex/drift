use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme::Theme;
use crate::tree::NodeKind;
use crate::vcs::model::FileStatus;

pub fn draw(frame: &mut Frame, app: &App, header: Rect, content: Rect) {
    let theme = &app.theme;
    let progress = match app.checked_count() {
        0 => format!("files ({})", app.files.len()),
        done => format!("files ({done}/{} reviewed)", app.files.len()),
    };
    frame.render_widget(
        Paragraph::new(progress).style(Style::default().fg(theme.muted)),
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
                NodeKind::Dir { expanded, .. } => {
                    let mut spans = vec![Span::styled(
                        format!("{indent}{} ", if *expanded { '▾' } else { '▸' }),
                        Style::default().fg(theme.muted),
                    )];
                    spans.extend(label(Style::default()));
                    Line::from(spans)
                }
                NodeKind::File { status, index } if app.is_checked(*index) => {
                    // Reviewed: the whole row recedes behind a checkmark.
                    let dim = Style::default().fg(theme.muted);
                    let mut spans = vec![Span::styled(format!("{indent}✓ "), dim)];
                    spans.extend(label(dim));
                    Line::from(spans)
                }
                NodeKind::File { status, .. } => {
                    let mut spans = vec![Span::styled(
                        format!("{indent}{} ", status.letter()),
                        Style::default().fg(status_color(theme, *status)),
                    )];
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

/// A row label, with the search query's match highlighted within it.
fn label_spans(label: String, base: Style, app: &App) -> Vec<Span<'static>> {
    let query = app.search_query();
    if !query.is_empty()
        && let Some(start) = label.to_lowercase().find(&query.to_lowercase())
        && let end = start + query.len()
        && label.get(start..end).is_some()
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
