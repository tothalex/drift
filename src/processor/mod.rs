//! The processor: turns a [`FileDiff`] into what the reviewer should see.
//!
//! For each hunk it resolves the innermost semantic block (function, class,
//! if, loop, …) enclosing the change via tree-sitter, shows that block in
//! full with the changed lines highlighted in place, and collapses long
//! unchanged runs. Files it can't parse keep the plain hunk view.

pub mod blocks;
pub mod comments;
pub mod emphasis;
pub mod highlight;
pub mod splice;
pub mod tabs;
pub mod treesitter;
pub mod view;

use std::path::Path;

use crate::vcs::model::{FileDiff, Hunk, LineKind};

use blocks::BlockResolver;
use highlight::FileHighlights;
use splice::{DiffIndex, change_segments, collapse};
use treesitter::TsResolver;
use view::{FileView, Section, ViewLine};

/// Runs of unchanged lines longer than this collapse…
const COLLAPSE_THRESHOLD: usize = 24;
/// …keeping this many lines on each edge.
const COLLAPSE_KEEP: usize = 4;
/// Context lines shown around changes that have no enclosing block.
const FALLBACK_CONTEXT: u32 = 3;

/// Unchanged comment blocks of at least this many lines fold to one line.
const COMMENT_FOLD_MIN: usize = 3;

/// Per-view display options, controlled by the user.
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewOptions {
    /// Show every unchanged line inside blocks (no `⋯` collapsing).
    pub expand_unchanged: bool,
    /// Block scope: how many levels outward from the innermost enclosing
    /// block (0 = innermost; clamped per change to what actually encloses it).
    pub scope: usize,
    /// Fold unchanged comment-only blocks into a one-line summary.
    pub fold_comments: bool,
}

/// `new_source` is the new-side (working tree) content; `None` — e.g. for
/// deleted or unreadable files — forces the hunk fallback. `old_source` is
/// the ancestor-side content, used only to highlight removed lines.
pub fn process(
    path: &Path,
    diff: &FileDiff,
    new_source: Option<&str>,
    old_source: Option<&str>,
    options: ViewOptions,
) -> FileView {
    let hunks = match diff {
        FileDiff::Binary => return FileView::Binary,
        FileDiff::Text { hunks } if hunks.is_empty() => return FileView::Unchanged,
        FileDiff::Text { hunks } => hunks,
    };

    // One parse of the new source serves block resolution and
    // highlighting alike.
    let resolver = new_source.and_then(|src| TsResolver::new(path, src));
    let (mut sections, scope_max, line_ranges) =
        build_sections(hunks, resolver.as_ref(), new_source, options);

    for section in &mut sections {
        emphasis::emphasize(&mut section.lines);
    }

    // Highlight only the byte ranges that will be displayed: the section
    // ranges on the new side, the removed lines on the old side.
    let new_highlights = resolver.as_ref().and_then(|resolver| {
        let byte_ranges: Vec<(usize, usize)> = line_ranges
            .iter()
            .map(|&(start, end)| resolver.byte_range_of_lines(start, end))
            .collect();
        highlight::highlight_tree(
            resolver.spec(),
            resolver.tree(),
            new_source.unwrap_or_default(),
            Some(&byte_ranges),
        )
    });
    let old_highlights = old_source.and_then(|src| {
        let old_resolver = TsResolver::new(path, src)?;
        let byte_ranges = removed_line_byte_ranges(&sections, &old_resolver);
        highlight::highlight_tree(
            old_resolver.spec(),
            old_resolver.tree(),
            src,
            Some(&byte_ranges),
        )
    });
    if new_highlights.is_some() || old_highlights.is_some() {
        for section in &mut sections {
            attach_spans(section, new_highlights.as_ref(), old_highlights.as_ref());
        }
    }
    // Comment classification needs the spans attached above; it's
    // precomputed here so the renderer never re-derives it per frame.
    for section in &mut sections {
        for view_line in &mut section.lines {
            if let ViewLine::Diff {
                line,
                spans,
                comment,
                ..
            } = view_line
            {
                *comment = comments::is_comment_line(&line.content, spans);
            }
        }
    }
    if options.fold_comments {
        for section in &mut sections {
            fold_comment_blocks(section);
        }
    }
    let diffstat = diffstat_of(&sections);
    FileView::Sections {
        sections,
        scope_max,
        diffstat,
    }
}

/// Merged byte ranges of the removed lines' positions in the old source.
fn removed_line_byte_ranges(
    sections: &[Section],
    old_resolver: &TsResolver<'_>,
) -> Vec<(usize, usize)> {
    let mut linenos: Vec<u32> = sections
        .iter()
        .flat_map(|s| s.lines.iter())
        .filter_map(|view_line| match view_line {
            ViewLine::Diff { line, .. } if line.kind == LineKind::Removed => line.old_lineno,
            _ => None,
        })
        .collect();
    linenos.sort_unstable();
    linenos.dedup();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for lineno in linenos {
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == lineno => last.1 = lineno,
            _ => ranges.push((lineno, lineno)),
        }
    }
    ranges
        .into_iter()
        .map(|(start, end)| old_resolver.byte_range_of_lines(start, end))
        .collect()
}

fn diffstat_of(sections: &[Section]) -> (usize, usize) {
    let mut adds = 0;
    let mut dels = 0;
    for view_line in sections.iter().flat_map(|s| s.lines.iter()) {
        if let ViewLine::Diff { line, .. } = view_line {
            match line.kind {
                LineKind::Added => adds += 1,
                LineKind::Removed => dels += 1,
                LineKind::Context => {}
            }
        }
    }
    (adds, dels)
}

/// Replace runs of `COMMENT_FOLD_MIN`+ unchanged comment-only lines with a
/// one-line summary.
fn fold_comment_blocks(section: &mut Section) {
    fn flush(run: &mut Vec<ViewLine>, out: &mut Vec<ViewLine>) {
        if run.len() >= COMMENT_FOLD_MIN {
            let summary = match run.first() {
                Some(ViewLine::Diff { line, .. }) => comments::summary(&line.content, 48),
                _ => String::new(),
            };
            out.push(ViewLine::CommentFold {
                count: run.len() as u32,
                summary,
            });
            run.clear();
        } else {
            out.append(run);
        }
    }

    let lines = std::mem::take(&mut section.lines);
    let mut out = Vec::with_capacity(lines.len());
    let mut run: Vec<ViewLine> = Vec::new();
    for view_line in lines {
        let foldable = matches!(&view_line, ViewLine::Diff { line, comment, .. }
            if line.kind == LineKind::Context && *comment);
        if foldable {
            run.push(view_line);
        } else {
            flush(&mut run, &mut out);
            out.push(view_line);
        }
    }
    flush(&mut run, &mut out);
    section.lines = out;
}

/// Syntax spans come from the side of the comparison the line lives on:
/// removed lines from the ancestor source, everything else from the
/// working-tree source.
fn attach_spans(
    section: &mut Section,
    new_highlights: Option<&FileHighlights>,
    old_highlights: Option<&FileHighlights>,
) {
    for view_line in &mut section.lines {
        let ViewLine::Diff { line, spans, .. } = view_line else {
            continue;
        };
        let looked_up = match line.kind {
            LineKind::Removed => old_highlights.zip(line.old_lineno),
            _ => new_highlights.zip(line.new_lineno),
        };
        if let Some((highlights, lineno)) = looked_up {
            *spans = highlights.spans_for(lineno).to_vec();
        }
    }
}

fn build_sections(
    hunks: &[Hunk],
    resolver: Option<&TsResolver<'_>>,
    new_source: Option<&str>,
    options: ViewOptions,
) -> (Vec<Section>, usize, Vec<(u32, u32)>) {
    let (Some(resolver), Some(source)) = (resolver, new_source) else {
        let ranges = hunks.iter().map(hunk_line_range).collect();
        return (hunks.iter().map(hunk_section).collect(), 0, ranges);
    };

    // Resolve each change segment's chain of enclosing blocks (innermost
    // first) and pick the level the scope option asks for. Top-level
    // changes (imports, …) get a small context window instead of a block.
    let mut scope_max = 0;
    let mut resolved: Vec<blocks::Block> = Vec::new();
    let mut sections: Vec<(u32, Section)> = Vec::new();
    let mut line_ranges: Vec<(u32, u32)> = Vec::new();
    for hunk in hunks {
        for span in change_segments(hunk) {
            let mut chain = resolver.enclosing_blocks(span);
            if chain.is_empty() {
                resolved.push(blocks::Block {
                    range: (
                        span.0.saturating_sub(FALLBACK_CONTEXT).max(1),
                        span.1 + FALLBACK_CONTEXT, // splice clamps to the file
                    ),
                    title: String::new(),
                });
            } else {
                scope_max = scope_max.max(chain.len() - 1);
                let level = options.scope.min(chain.len() - 1);
                resolved.push(chain.swap_remove(level));
            }
        }
    }

    // Blocks from one tree are nested or disjoint: merge overlaps, keeping
    // the outer block's title.
    resolved.sort_by_key(|b| (b.range.0, std::cmp::Reverse(b.range.1)));
    resolved.dedup_by(|next, kept| {
        if next.range.0 <= kept.range.1 {
            kept.range.1 = kept.range.1.max(next.range.1);
            true
        } else {
            false
        }
    });

    let index = DiffIndex::new(hunks);
    let file_lines: Vec<&str> = source.lines().collect();
    for block in resolved {
        line_ranges.push(block.range);
        let mut lines = index.splice(block.range, &file_lines);
        if !options.expand_unchanged {
            lines = collapse(lines, COLLAPSE_THRESHOLD, COLLAPSE_KEEP);
        }
        sections.push((block.range.0, Section { lines }));
    }

    sections.sort_by_key(|(pos, _)| *pos);
    (
        sections.into_iter().map(|(_, s)| s).collect(),
        scope_max,
        line_ranges,
    )
}

/// New-side line range a fallback hunk displays.
fn hunk_line_range(hunk: &Hunk) -> (u32, u32) {
    let start = hunk.new_range.0.max(1);
    (start, start + hunk.new_range.1.saturating_sub(1))
}

/// Fallback: the hunk exactly as the VCS reported it.
fn hunk_section(hunk: &Hunk) -> Section {
    Section {
        lines: hunk.lines.iter().cloned().map(ViewLine::diff).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::LineKind;
    use crate::vcs::unidiff;

    const NEW_SOURCE: &str = "\
fn alpha() {
    let a = 1;
    let b = 3;
    let c = a + b;
    println!(\"{c}\");
}

fn beta() {
    println!(\"beta\");
}
";

    const PATCH: &str = "\
diff --git a/x.rs b/x.rs
--- a/x.rs
+++ b/x.rs
@@ -1,5 +1,5 @@
 fn alpha() {
     let a = 1;
-    let b = 2;
+    let b = 3;
     let c = a + b;
     println!(\"{c}\");";

    #[test]
    fn change_expands_to_whole_function() {
        let diff = unidiff::parse(PATCH);
        let view = process(
            Path::new("x.rs"),
            &diff,
            Some(NEW_SOURCE),
            None,
            ViewOptions::default(),
        );
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        assert_eq!(sections.len(), 1);
        // No header line — the section starts with the signature itself.
        let ViewLine::Diff { line: first, .. } = &sections[0].lines[0] else {
            panic!("expected diff line");
        };
        assert_eq!(first.content, "fn alpha() {");

        let kinds: Vec<_> = sections[0]
            .lines
            .iter()
            .map(|l| match l {
                ViewLine::Diff { line: d, .. } => d.kind,
                _ => panic!("unexpected non-diff line"),
            })
            .collect();
        // Whole function incl. the closing brace, removal in place.
        assert_eq!(
            kinds,
            vec![
                LineKind::Context,
                LineKind::Context,
                LineKind::Removed,
                LineKind::Added,
                LineKind::Context,
                LineKind::Context,
                LineKind::Context,
            ]
        );
    }

    #[test]
    fn spans_attach_from_the_correct_side() {
        let old_source = NEW_SOURCE.replace("let b = 3;", "let b = 2;");
        let diff = unidiff::parse(PATCH);
        let view = process(
            Path::new("x.rs"),
            &diff,
            Some(NEW_SOURCE),
            Some(&old_source),
            ViewOptions::default(),
        );
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        for view_line in &sections[0].lines {
            let ViewLine::Diff { line, spans, .. } = view_line else {
                continue;
            };
            // Every line of this rust block contains a keyword or call —
            // all must carry syntax spans, including the removed line
            // (highlighted from the old source) and the closing brace
            // (no tokens → empty is fine, skip it).
            if line.content.trim() == "}" {
                continue;
            }
            assert!(
                !spans.is_empty(),
                "line {:?} has no highlight spans",
                line.content
            );
        }
    }

    #[test]
    fn scope_widens_from_if_to_function_and_clamps() {
        let source = "\
fn outer() {
    let flag = true;
    if flag {
        do_it();
    }
}
";
        let patch = "\
@@ -1,6 +1,6 @@
 fn outer() {
     let flag = true;
     if flag {
-        do_old();
+        do_it();
     }
 }";
        let diff = unidiff::parse(patch);
        let section_start = |scope: usize| {
            let opts = ViewOptions {
                scope,
                ..Default::default()
            };
            let FileView::Sections {
                sections,
                scope_max,
                ..
            } = process(Path::new("x.rs"), &diff, Some(source), None, opts)
            else {
                panic!("expected sections");
            };
            assert_eq!(scope_max, 1); // if → fn is the whole chain
            assert_eq!(sections.len(), 1);
            let ViewLine::Diff { line, .. } = &sections[0].lines[0] else {
                panic!("expected diff line");
            };
            line.content.clone()
        };

        assert_eq!(section_start(0), "    if flag {"); // innermost
        assert_eq!(section_start(1), "fn outer() {"); // one level out
        assert_eq!(section_start(99), "fn outer() {"); // clamped
    }

    #[test]
    fn expand_option_disables_collapsing() {
        let mut source = String::from("fn long() {\n");
        for i in 0..40 {
            source += &format!("    let v{i} = {i};\n");
        }
        source += "}\n";
        let patch = "@@ -2 +2 @@\n-    let v0 = 9;\n+    let v0 = 0;";
        let diff = unidiff::parse(patch);

        let count = |expand_unchanged: bool| {
            let opts = ViewOptions {
                expand_unchanged,
                ..Default::default()
            };
            let FileView::Sections { sections, .. } =
                process(Path::new("x.rs"), &diff, Some(&source), None, opts)
            else {
                panic!("expected sections");
            };
            let collapsed = sections[0]
                .lines
                .iter()
                .filter(|l| matches!(l, ViewLine::Collapsed { .. }))
                .count();
            (collapsed, sections[0].lines.len())
        };

        let (collapsed, _) = count(false);
        assert_eq!(collapsed, 1);
        let (collapsed, total) = count(true);
        assert_eq!(collapsed, 0);
        assert_eq!(total, 43); // 42 file lines + 1 removed line, all shown
    }

    #[test]
    fn one_hunk_spanning_two_functions_resolves_each_block() {
        // Regression: hunk grouping can put changes to sibling functions
        // in a single hunk; blocks must resolve per change segment.
        let source = "\
fn a() {
    one();
}

fn b() {
    two();
}
";
        let patch = "\
@@ -1,7 +1,7 @@
 fn a() {
-    zero();
+    one();
 }

 fn b() {
-    nil();
+    two();
 }";
        let diff = unidiff::parse(patch);
        let FileView::Sections { sections, .. } = process(
            Path::new("x.rs"),
            &diff,
            Some(source),
            None,
            ViewOptions::default(),
        ) else {
            panic!("expected sections");
        };
        assert_eq!(sections.len(), 2);
        let first_content = |s: &Section| match &s.lines[0] {
            ViewLine::Diff { line, .. } => line.content.clone(),
            _ => panic!("expected diff line"),
        };
        assert_eq!(first_content(&sections[0]), "fn a() {");
        assert_eq!(first_content(&sections[1]), "fn b() {");
    }

    #[test]
    fn export_wrapped_function_resolves_to_block() {
        // Regression: a span touching the `export` keyword must stop at
        // the export statement, not walk past it to top level.
        let source = "\
export function a(): number {
  return 1;
}

export function b(): number {
  return 2;
}
";
        let patch = "\
@@ -1,3 +1,3 @@
 export function a(): number {
-  return 0;
+  return 1;
 }";
        let diff = unidiff::parse(patch);
        let FileView::Sections { sections, .. } = process(
            Path::new("x.ts"),
            &diff,
            Some(source),
            None,
            ViewOptions::default(),
        ) else {
            panic!("expected sections");
        };
        assert_eq!(sections.len(), 1);
        // The whole exported function: signature through closing brace.
        assert_eq!(sections[0].lines.len(), 4); // 3 new-side lines + removal
        let ViewLine::Diff { line: last, .. } = sections[0].lines.last().unwrap() else {
            panic!("expected diff line");
        };
        assert_eq!(last.content, "}");
    }

    #[test]
    fn unparseable_file_falls_back_to_hunks() {
        let diff = unidiff::parse(PATCH);
        let view = process(
            Path::new("notes.txt"),
            &diff,
            Some("whatever"),
            None,
            ViewOptions::default(),
        );
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        assert_eq!(sections[0].lines.len(), 6);
    }

    #[test]
    fn missing_source_falls_back_to_hunks() {
        let diff = unidiff::parse(PATCH);
        let view = process(Path::new("x.rs"), &diff, None, None, ViewOptions::default());
        let FileView::Sections { sections, .. } = view else {
            panic!("expected sections");
        };
        // Hunk fallback: exactly the diff's own lines, nothing spliced in.
        assert_eq!(sections[0].lines.len(), 6);
    }

    #[test]
    fn two_hunks_in_one_function_merge_into_one_section() {
        let source = "\
fn long() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
}
";
        let patch = "\
@@ -2,2 +2,2 @@
-    let a = 0;
+    let a = 1;
     let b = 2;
@@ -7,2 +7,2 @@
-    let f = 0;
+    let f = 6;
     let g = 7;";
        let diff = unidiff::parse(patch);
        let FileView::Sections { sections, .. } = process(
            Path::new("x.rs"),
            &diff,
            Some(source),
            None,
            ViewOptions::default(),
        ) else {
            panic!("expected sections");
        };
        assert_eq!(sections.len(), 1);
        // The merged section spans the whole function: 9 new-side lines
        // L1–9 plus the two removed lines shown in place.
        assert_eq!(sections[0].lines.len(), 11);
        let ViewLine::Diff { line: first, .. } = &sections[0].lines[0] else {
            panic!("expected diff line");
        };
        assert_eq!(first.new_lineno, Some(1));
    }
}
