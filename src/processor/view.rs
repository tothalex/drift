//! The display model: what the UI renders for one file.

use crate::processor::highlight::HighlightSpan;
use crate::vcs::model::DiffLine;

#[derive(Debug)]
pub enum FileView {
    Binary,
    /// Pure rename or mode change — no content difference.
    Unchanged,
    Sections {
        sections: Vec<Section>,
        /// Deepest available scope level: how far outward the block scope
        /// can widen from the innermost enclosing block (0 = only that).
        scope_max: usize,
        /// (added, removed) line counts, precomputed for the header.
        diffstat: (usize, usize),
    },
}

/// One reviewable unit: a semantic block enclosing changes, or — when no
/// block could be resolved — a plain hunk. No headers: a block's first line
/// is its own signature, and the line-number gutter carries position;
/// sections are separated visually by the renderer.
#[derive(Debug)]
pub struct Section {
    pub lines: Vec<ViewLine>,
}

#[derive(Debug)]
pub enum ViewLine {
    Diff {
        line: DiffLine,
        /// Syntax highlight spans over `line.content`; empty when the
        /// language is unsupported.
        spans: Vec<HighlightSpan>,
        /// Byte ranges of `line.content` that actually differ from the
        /// paired line on the other side (word-level emphasis).
        emph: Vec<(usize, usize)>,
        /// The line is comment-only (precomputed; rendered as prose).
        comment: bool,
    },
    /// A collapsed run of unchanged lines inside a large block.
    Collapsed { count: u32 },
    /// A folded run of unchanged comment-only lines, summarized by the
    /// first line's prose.
    CommentFold { count: u32, summary: String },
}

impl ViewLine {
    pub fn diff(line: DiffLine) -> ViewLine {
        ViewLine::Diff {
            line,
            spans: Vec::new(),
            emph: Vec::new(),
            comment: false,
        }
    }
}

/// One row of the flattened view: section lines with a blank separator
/// between sections.
pub enum FlatLine<'a> {
    Separator,
    Line(&'a ViewLine),
}

impl<'a> FlatLine<'a> {
    /// Copyable text of this row; `None` for fold markers, whose display
    /// text is not content.
    pub fn content(&self) -> Option<&'a str> {
        match self {
            FlatLine::Separator => Some(""),
            FlatLine::Line(ViewLine::Diff { line, .. }) => Some(line.content.as_str()),
            FlatLine::Line(_) => None,
        }
    }
}

impl FileView {
    /// The canonical flattening of the view in display order. Cursor
    /// indexing, mouse hit-testing, copying, and rendering all derive
    /// from this one definition so they can never disagree.
    pub fn flat_lines(&self) -> impl Iterator<Item = FlatLine<'_>> {
        let sections: &[Section] = match self {
            FileView::Sections { sections, .. } => sections,
            _ => &[],
        };
        sections.iter().enumerate().flat_map(|(nth, section)| {
            let separator = (nth > 0).then_some(FlatLine::Separator);
            separator
                .into_iter()
                .chain(section.lines.iter().map(FlatLine::Line))
        })
    }

    /// Number of flattened rows (Binary/Unchanged render one message row).
    pub fn flat_len(&self) -> usize {
        match self {
            FileView::Sections { sections, .. } => {
                sections.iter().map(|s| s.lines.len()).sum::<usize>()
                    + sections.len().saturating_sub(1)
            }
            _ => 1,
        }
    }
}

/// Byte offset of the `ch`-th character of `s` (clamped to the end).
pub fn char_to_byte(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::LineKind;

    fn diff_line(content: &str) -> ViewLine {
        ViewLine::diff(DiffLine {
            kind: LineKind::Context,
            old_lineno: Some(1),
            new_lineno: Some(1),
            content: content.to_string(),
        })
    }

    #[test]
    fn flat_lines_inserts_separators_between_sections() {
        let view = FileView::Sections {
            sections: vec![
                Section { lines: vec![diff_line("a"), diff_line("b")] },
                Section { lines: vec![diff_line("c")] },
            ],
            scope_max: 0,
            diffstat: (0, 0),
        };
        let contents: Vec<Option<&str>> =
            view.flat_lines().map(|fl| fl.content()).collect();
        assert_eq!(contents, vec![Some("a"), Some("b"), Some(""), Some("c")]);
        assert_eq!(view.flat_len(), 4);
    }

    #[test]
    fn fold_markers_have_no_content() {
        let view = FileView::Sections {
            sections: vec![Section {
                lines: vec![
                    ViewLine::Collapsed { count: 5 },
                    ViewLine::CommentFold { count: 3, summary: String::new() },
                ],
            }],
            scope_max: 0,
            diffstat: (0, 0),
        };
        assert!(view.flat_lines().all(|fl| fl.content().is_none()));
    }

    #[test]
    fn char_to_byte_clamps() {
        assert_eq!(char_to_byte("abc", 1), 1);
        assert_eq!(char_to_byte("abc", 99), 3);
        assert_eq!(char_to_byte("é_", 1), 2); // multi-byte
    }
}
