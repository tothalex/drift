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
    /// First row of a review-comment thread (or one conversation entry)
    /// in the pull-request view: who wrote it and when. `key` targets the
    /// forge-side thread for replies; empty for rows nothing replies to.
    /// `id` is the single comment this row belongs to (for deletion).
    CommentHead {
        key: String,
        id: String,
        author: String,
        date: String,
        /// Replies hidden behind a collapsed head (0 when expanded).
        replies: usize,
        resolved: Option<bool>,
        collapsed: bool,
    },
    /// One pre-wrapped prose row of a review-comment body.
    CommentBody {
        key: String,
        id: String,
        text: String,
    },
    /// A muted key hint under a thread ("a reply · t fold"). Carries the
    /// thread key so acting on the hint row targets the thread above it.
    CommentHint { key: String, text: String },
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

    /// New-side line number at flat row `row`, or the nearest one above —
    /// the anchor for keeping the cursor in place across a live refresh.
    pub fn lineno_at(&self, row: usize) -> Option<u32> {
        let mut best = None;
        for (i, flat) in self.flat_lines().enumerate() {
            if i > row {
                break;
            }
            if let FlatLine::Line(ViewLine::Diff { line, .. }) = flat
                && let Some(lineno) = line.new_lineno
            {
                best = Some(lineno);
            }
        }
        best
    }

    /// Flat row showing new-side line `lineno`, or the nearest row after
    /// it (falling back to the last numbered row when nothing follows).
    pub fn row_of_lineno(&self, lineno: u32) -> Option<usize> {
        let mut fallback = None;
        for (i, flat) in self.flat_lines().enumerate() {
            if let FlatLine::Line(ViewLine::Diff { line, .. }) = flat
                && let Some(n) = line.new_lineno
            {
                if n >= lineno {
                    return Some(i);
                }
                fallback = Some(i);
            }
        }
        fallback
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
                Section {
                    lines: vec![diff_line("a"), diff_line("b")],
                },
                Section {
                    lines: vec![diff_line("c")],
                },
            ],
            scope_max: 0,
            diffstat: (0, 0),
        };
        let contents: Vec<Option<&str>> = view.flat_lines().map(|fl| fl.content()).collect();
        assert_eq!(contents, vec![Some("a"), Some("b"), Some(""), Some("c")]);
        assert_eq!(view.flat_len(), 4);
    }

    #[test]
    fn fold_markers_have_no_content() {
        let view = FileView::Sections {
            sections: vec![Section {
                lines: vec![
                    ViewLine::Collapsed { count: 5 },
                    ViewLine::CommentFold {
                        count: 3,
                        summary: String::new(),
                    },
                ],
            }],
            scope_max: 0,
            diffstat: (0, 0),
        };
        assert!(view.flat_lines().all(|fl| fl.content().is_none()));
    }

    #[test]
    fn lineno_anchor_round_trips_across_views() {
        let numbered = |n: u32| {
            ViewLine::diff(DiffLine {
                kind: LineKind::Context,
                old_lineno: Some(n),
                new_lineno: Some(n),
                content: format!("line {n}"),
            })
        };
        let view = FileView::Sections {
            sections: vec![Section {
                lines: vec![numbered(10), numbered(11), numbered(14)],
            }],
            scope_max: 0,
            diffstat: (0, 0),
        };
        assert_eq!(view.lineno_at(1), Some(11));
        assert_eq!(view.row_of_lineno(11), Some(1));
        // Line 12 no longer shown: land on the nearest following row.
        assert_eq!(view.row_of_lineno(12), Some(2));
        // Past the end: fall back to the last numbered row.
        assert_eq!(view.row_of_lineno(99), Some(2));
        // Anchor above the first numbered row.
        assert_eq!(view.row_of_lineno(1), Some(0));
    }

    #[test]
    fn char_to_byte_clamps() {
        assert_eq!(char_to_byte("abc", 1), 1);
        assert_eq!(char_to_byte("abc", 99), 3);
        assert_eq!(char_to_byte("é_", 1), 2); // multi-byte
    }
}
