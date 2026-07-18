//! Word-level change emphasis: pair up removed/added line runs and mark
//! the byte ranges that actually differ, so the UI can highlight the exact
//! edit instead of the whole line (delta/GitHub style).

use similar::{DiffOp, TextDiff};

use crate::vcs::model::LineKind;

use super::view::ViewLine;

/// Pairs below this character-level similarity get no emphasis — the whole
/// line effectively changed and per-word markers would just be noise.
const MIN_SIMILARITY: f32 = 0.5;

/// Byte range within a line's content.
type ByteRange = (usize, usize);

pub fn emphasize(lines: &mut [ViewLine]) {
    let kind_of = |line: &ViewLine| match line {
        ViewLine::Diff { line, .. } => Some(line.kind),
        _ => None,
    };

    // Find each run of Removed lines directly followed by Added lines and
    // pair them positionally — the standard unified-diff correspondence.
    let mut i = 0;
    while i < lines.len() {
        if kind_of(&lines[i]) != Some(LineKind::Removed) {
            i += 1;
            continue;
        }
        let removed_start = i;
        while i < lines.len() && kind_of(&lines[i]) == Some(LineKind::Removed) {
            i += 1;
        }
        let added_start = i;
        while i < lines.len() && kind_of(&lines[i]) == Some(LineKind::Added) {
            i += 1;
        }
        let pairs = (added_start - removed_start).min(i - added_start);
        for k in 0..pairs {
            let (old_ranges, new_ranges) = {
                let (ViewLine::Diff { line: old, .. }, ViewLine::Diff { line: new, .. }) =
                    (&lines[removed_start + k], &lines[added_start + k])
                else {
                    continue;
                };
                diff_ranges(&old.content, &new.content)
            };
            set_emph(&mut lines[removed_start + k], old_ranges);
            set_emph(&mut lines[added_start + k], new_ranges);
        }
    }
}

fn set_emph(line: &mut ViewLine, ranges: Vec<ByteRange>) {
    if let ViewLine::Diff { emph, .. } = line {
        *emph = ranges;
    }
}

/// Byte ranges that differ between the two lines (old side, new side).
fn diff_ranges(old: &str, new: &str) -> (Vec<ByteRange>, Vec<ByteRange>) {
    let diff = TextDiff::from_chars(old, new);
    if diff.ratio() < MIN_SIMILARITY {
        return (Vec::new(), Vec::new());
    }
    let old_bytes = byte_offsets(old);
    let new_bytes = byte_offsets(new);
    let mut old_ranges = Vec::new();
    let mut new_ranges = Vec::new();
    for op in diff.ops() {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { .. } => push(&mut old_ranges, op.old_range(), &old_bytes),
            DiffOp::Insert { .. } => push(&mut new_ranges, op.new_range(), &new_bytes),
            DiffOp::Replace { .. } => {
                push(&mut old_ranges, op.old_range(), &old_bytes);
                push(&mut new_ranges, op.new_range(), &new_bytes);
            }
        }
    }
    (old_ranges, new_ranges)
}

/// Convert a char-index range to bytes, merging with an adjacent range.
fn push(ranges: &mut Vec<ByteRange>, chars: std::ops::Range<usize>, offsets: &[usize]) {
    let (start, end) = (offsets[chars.start], offsets[chars.end]);
    if let Some(last) = ranges.last_mut()
        && last.1 == start
    {
        last.1 = end;
    } else {
        ranges.push((start, end));
    }
}

fn byte_offsets(s: &str) -> Vec<usize> {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::DiffLine;

    fn line(kind: LineKind, content: &str) -> ViewLine {
        ViewLine::diff(DiffLine {
            kind,
            old_lineno: Some(1),
            new_lineno: Some(1),
            content: content.to_string(),
        })
    }

    fn emph_of(view_line: &ViewLine) -> &[(usize, usize)] {
        match view_line {
            ViewLine::Diff { emph, .. } => emph,
            _ => panic!("not a diff line"),
        }
    }

    #[test]
    fn marks_only_the_changed_segment() {
        let mut lines = vec![
            line(LineKind::Removed, "    let b = 2;"),
            line(LineKind::Added, "    let b = 3;"),
        ];
        emphasize(&mut lines);
        assert_eq!(emph_of(&lines[0]), &[(12, 13)]); // the `2`
        assert_eq!(emph_of(&lines[1]), &[(12, 13)]); // the `3`
    }

    #[test]
    fn dissimilar_lines_get_no_emphasis() {
        let mut lines = vec![
            line(LineKind::Removed, "let alpha = compute();"),
            line(LineKind::Added, "return None;"),
        ];
        emphasize(&mut lines);
        assert!(emph_of(&lines[0]).is_empty());
        assert!(emph_of(&lines[1]).is_empty());
    }

    #[test]
    fn unpaired_additions_get_no_emphasis() {
        let mut lines = vec![
            line(LineKind::Context, "fn main() {"),
            line(LineKind::Added, "    println!(\"new\");"),
        ];
        emphasize(&mut lines);
        assert!(emph_of(&lines[1]).is_empty());
    }
}
