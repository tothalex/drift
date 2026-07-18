//! Merging hunk lines into full block content.
//!
//! Given the hunks of a file and its new-side source, reconstruct any
//! new-side line range with additions substituted, removals inserted at the
//! position where the lines used to be, and everything else filled from the
//! actual file — so a block is shown complete, not just ±3 context lines.

use std::collections::HashMap;

use crate::vcs::model::{DiffLine, Hunk, LineKind};

use super::view::ViewLine;

/// A file's diff lines indexed by new-side position.
pub struct DiffIndex {
    /// Added and in-hunk context lines occupying a new-side line.
    at: HashMap<u32, DiffLine>,
    /// Removed lines displayed immediately before a new-side line.
    removed_before: HashMap<u32, Vec<DiffLine>>,
    /// Per hunk: (next new-side line after it, old−new line delta after it),
    /// for mapping line numbers outside hunks. Sorted.
    deltas: Vec<(u32, i64)>,
}

/// Unified-diff convention: a zero-count range's start is the line *before*
/// the gap; the "next line" is start+1. For non-empty ranges it's start+count.
fn end_exclusive(range: (u32, u32)) -> u32 {
    if range.1 == 0 { range.0 + 1 } else { range.0 + range.1 }
}

impl DiffIndex {
    pub fn new(hunks: &[Hunk]) -> Self {
        let mut at = HashMap::new();
        let mut removed_before: HashMap<u32, Vec<DiffLine>> = HashMap::new();
        let mut deltas = Vec::with_capacity(hunks.len());
        for hunk in hunks {
            let mut new_no = if hunk.new_range.1 == 0 {
                hunk.new_range.0 + 1
            } else {
                hunk.new_range.0
            };
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Removed => {
                        removed_before.entry(new_no).or_default().push(line.clone());
                    }
                    _ => {
                        if let Some(n) = line.new_lineno {
                            at.insert(n, line.clone());
                            new_no = n + 1;
                        }
                    }
                }
            }
            deltas.push((
                end_exclusive(hunk.new_range),
                i64::from(end_exclusive(hunk.old_range)) - i64::from(end_exclusive(hunk.new_range)),
            ));
        }
        deltas.sort_unstable();
        DiffIndex { at, removed_before, deltas }
    }

    /// Old-side line number for a new-side line that is not inside any hunk.
    fn old_for_new(&self, n: u32) -> u32 {
        let delta = self
            .deltas
            .iter()
            .take_while(|(end, _)| *end <= n)
            .last()
            .map_or(0, |(_, d)| *d);
        u32::try_from(i64::from(n) + delta).unwrap_or(n)
    }

    /// Reconstruct an inclusive new-side line range as view lines.
    pub fn splice(&self, range: (u32, u32), file_lines: &[&str]) -> Vec<ViewLine> {
        let file_len = file_lines.len() as u32;
        let end = range.1.min(file_len);
        let mut out = Vec::new();
        for n in range.0..=end {
            if let Some(removed) = self.removed_before.get(&n) {
                out.extend(removed.iter().cloned().map(ViewLine::diff));
            }
            let line = match self.at.get(&n) {
                Some(diff_line) => diff_line.clone(),
                None => DiffLine {
                    kind: LineKind::Context,
                    old_lineno: Some(self.old_for_new(n)),
                    new_lineno: Some(n),
                    content: file_lines.get(n as usize - 1).copied().unwrap_or("").to_string(),
                },
            };
            out.push(ViewLine::diff(line));
        }
        // A deletion at end of file anchors past the last line.
        if end >= file_len
            && let Some(removed) = self.removed_before.get(&(end + 1))
        {
            out.extend(removed.iter().cloned().map(ViewLine::diff));
        }
        out
    }
}

/// New-side line spans of each contiguous change segment in a hunk
/// (context excluded). A hunk can contain several distinct changes —
/// resolving blocks per segment keeps a change in one function from
/// dragging its sibling into the same span.
pub fn change_segments(hunk: &Hunk) -> Vec<(u32, u32)> {
    let mut segments: Vec<(u32, u32)> = Vec::new();
    let mut current: Option<(u32, u32)> = None;
    let mut new_no = if hunk.new_range.1 == 0 {
        hunk.new_range.0 + 1
    } else {
        hunk.new_range.0
    };
    for line in &hunk.lines {
        let point = match line.kind {
            LineKind::Added => {
                let Some(n) = line.new_lineno else { continue };
                new_no = n + 1;
                n
            }
            LineKind::Removed => {
                // The gap sits between new lines new_no-1 and new_no.
                new_no.saturating_sub(1).max(1)
            }
            LineKind::Context => {
                if let Some(n) = line.new_lineno {
                    new_no = n + 1;
                }
                if let Some(segment) = current.take() {
                    segments.push(segment);
                }
                continue;
            }
        };
        current = Some(match current {
            Some((min, max)) => (min.min(point), max.max(point)),
            None => (point, point),
        });
    }
    if let Some(segment) = current {
        segments.push(segment);
    }
    segments
}

/// Collapse runs of more than `threshold` consecutive context lines, keeping
/// `keep` lines on each edge of the run.
pub fn collapse(lines: Vec<ViewLine>, threshold: usize, keep: usize) -> Vec<ViewLine> {
    let mut out = Vec::with_capacity(lines.len());
    let mut run: Vec<ViewLine> = Vec::new();
    let flush = |run: &mut Vec<ViewLine>, out: &mut Vec<ViewLine>| {
        if run.len() > threshold {
            let tail = run.split_off(run.len() - keep);
            let collapsed = run.split_off(keep);
            out.append(run);
            out.push(ViewLine::Collapsed { count: collapsed.len() as u32 });
            out.extend(tail);
        } else {
            out.append(run);
        }
    };
    for line in lines {
        let is_context =
            matches!(&line, ViewLine::Diff { line: d, .. } if d.kind == LineKind::Context);
        if is_context {
            run.push(line);
        } else {
            flush(&mut run, &mut out);
            out.push(line);
        }
    }
    flush(&mut run, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::unidiff;
    use crate::vcs::model::FileDiff;

    fn hunks_of(patch: &str) -> Vec<Hunk> {
        match unidiff::parse(patch) {
            FileDiff::Text { hunks } => hunks,
            _ => panic!("expected text diff"),
        }
    }

    const PATCH: &str = "\
@@ -1,5 +1,5 @@
 fn alpha() {
     let a = 1;
-    let b = 2;
+    let b = 3;
     let c = a + b;
     println!(\"{c}\");";

    const NEW_SOURCE: &str = "\
fn alpha() {
    let a = 1;
    let b = 3;
    let c = a + b;
    println!(\"{c}\");
}
";

    #[test]
    fn splice_fills_block_from_file_and_inserts_removals() {
        let hunks = hunks_of(PATCH);
        let index = DiffIndex::new(&hunks);
        let file_lines: Vec<&str> = NEW_SOURCE.lines().collect();

        let lines = index.splice((1, 6), &file_lines);
        let kinds: Vec<_> = lines
            .iter()
            .map(|l| match l {
                ViewLine::Diff { line: d, .. } => d.kind,
                _ => panic!("unexpected non-diff line"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Context, // fn alpha() {
                LineKind::Context, // let a = 1;
                LineKind::Removed, // let b = 2;
                LineKind::Added,   // let b = 3;
                LineKind::Context, // let c = a + b;
                LineKind::Context, // println!
                LineKind::Context, // }  ← beyond hunk, from file
            ]
        );
        let ViewLine::Diff { line: last, .. } = &lines[6] else { unreachable!() };
        assert_eq!(last.content, "}");
        assert_eq!(last.old_lineno, Some(6)); // same-size change: no shift
    }

    #[test]
    fn old_linenos_shift_after_asymmetric_hunk() {
        // Two lines added at the top: old 1 ↔ new 3.
        let hunks = hunks_of("@@ -1,1 +1,3 @@\n+one\n+two\n three");
        let index = DiffIndex::new(&hunks);
        let lines = index.splice((4, 4), &["one", "two", "three", "four"]);
        let ViewLine::Diff { line, .. } = &lines[0] else { panic!() };
        assert_eq!(line.old_lineno, Some(2));
        assert_eq!(line.new_lineno, Some(4));
    }

    #[test]
    fn deletion_at_eof_is_emitted() {
        let hunks = hunks_of("@@ -2,2 +2,1 @@\n keep\n-gone");
        let index = DiffIndex::new(&hunks);
        let lines = index.splice((1, 2), &["first", "keep"]);
        let kinds: Vec<_> = lines
            .iter()
            .map(|l| match l {
                ViewLine::Diff { line: d, .. } => d.kind,
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![LineKind::Context, LineKind::Context, LineKind::Removed]
        );
    }

    #[test]
    fn segments_cover_changes_not_context() {
        let hunks = hunks_of(PATCH);
        assert_eq!(change_segments(&hunks[0]), vec![(2, 3)]);
    }

    #[test]
    fn segments_of_pure_deletion_anchor_to_gap() {
        // Deletion after new line 4.
        let hunks = hunks_of("@@ -5,2 +4,0 @@\n-gone\n-gone too");
        assert_eq!(change_segments(&hunks[0]), vec![(4, 4)]);
    }

    #[test]
    fn separate_changes_in_one_hunk_become_separate_segments() {
        let hunks = hunks_of(
            "@@ -1,7 +1,8 @@\n ctx\n+first\n ctx\n ctx\n-old\n+second\n ctx\n ctx",
        );
        // The removal anchors to the line before its gap (4).
        assert_eq!(change_segments(&hunks[0]), vec![(2, 2), (4, 5)]);
    }

    #[test]
    fn collapse_keeps_edges() {
        let context = |n: u32| {
            ViewLine::diff(DiffLine {
                kind: LineKind::Context,
                old_lineno: Some(n),
                new_lineno: Some(n),
                content: String::new(),
            })
        };
        let mut lines: Vec<ViewLine> = (1..=30).map(context).collect();
        lines.push(ViewLine::diff(DiffLine {
            kind: LineKind::Added,
            old_lineno: None,
            new_lineno: Some(31),
            content: String::new(),
        }));

        let out = collapse(lines, 10, 3);
        assert!(matches!(out[3], ViewLine::Collapsed { count: 24 }));
        assert_eq!(out.len(), 3 + 1 + 3 + 1); // edges + marker + added
    }
}
