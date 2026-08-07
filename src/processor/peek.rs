//! On-demand peek: the new-side text of the semantic blocks enclosing a
//! line, computed when the overlay opens — nothing is retained in the
//! cached views for it.

use std::collections::HashSet;
use std::path::Path;

use crate::vcs::model::{FileDiff, LineKind};

use super::blocks::{Block, BlockResolver};
use super::highlight::{self, HighlightSpan};
use super::treesitter::TsResolver;

/// One new-side line of the peeked range.
pub struct PeekLine {
    pub content: String,
    pub spans: Vec<HighlightSpan>,
    /// The line is new (added by the diff) — the overlay marks it so the
    /// change stays findable inside the clean text.
    pub changed: bool,
}

/// The peekable content at one line: the chain of enclosing blocks
/// (innermost first) and the new-side lines covering the outermost one.
pub struct PeekView {
    pub chain: Vec<Block>,
    /// 1-based line number of `lines[0]` — the outermost block's start.
    pub first: u32,
    pub lines: Vec<PeekLine>,
}

/// Resolve the blocks enclosing new-side `line` and slice their text out
/// of `source`. `None` when the language is unsupported or the line sits
/// at top level outside any block.
pub fn peek(path: &Path, source: &str, diff: &FileDiff, line: u32) -> Option<PeekView> {
    let resolver = TsResolver::new(path, source)?;
    let chain = resolver.enclosing_blocks((line, line));
    let outer = chain.last()?.range;

    let byte_range = resolver.byte_range_of_lines(outer.0, outer.1);
    let highlights = highlight::highlight_tree(
        resolver.spec(),
        resolver.tree(),
        source,
        Some(&[byte_range]),
    );

    let added: HashSet<u32> = match diff {
        FileDiff::Binary => HashSet::new(),
        FileDiff::Text { hunks } => hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == LineKind::Added)
            .filter_map(|l| l.new_lineno)
            .collect(),
    };

    let file_lines: Vec<&str> = source.lines().collect();
    let last = outer.1.min(file_lines.len() as u32);
    let lines = (outer.0..=last)
        .map(|n| PeekLine {
            content: file_lines
                .get(n as usize - 1)
                .copied()
                .unwrap_or("")
                .to_string(),
            spans: highlights
                .as_ref()
                .map_or_else(Vec::new, |h| h.spans_for(n).to_vec()),
            changed: added.contains(&n),
        })
        .collect();

    Some(PeekView {
        chain,
        first: outer.0,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::unidiff;

    const SOURCE: &str = "\
fn alpha() {
    let a = 1;
    if a > 0 {
        println!(\"positive\");
    }
}
";

    const PATCH: &str = "\
diff --git a/x.rs b/x.rs
--- a/x.rs
+++ b/x.rs
@@ -1,5 +1,6 @@
 fn alpha() {
     let a = 1;
     if a > 0 {
+        println!(\"positive\");
     }
 }";

    #[test]
    fn chain_lines_and_changed_marks() {
        let diff = unidiff::parse(PATCH);
        let view = peek(Path::new("x.rs"), SOURCE, &diff, 4).expect("peek");

        // Innermost first: the if, then the function.
        assert_eq!(view.chain.len(), 2);
        assert_eq!(view.chain[0].range, (3, 5));
        assert_eq!(view.chain[1].range, (1, 6));
        assert_eq!(view.chain[1].title, "fn alpha()");

        // Lines cover the outermost block.
        assert_eq!(view.first, 1);
        assert_eq!(view.lines.len(), 6);
        assert_eq!(view.lines[0].content, "fn alpha() {");

        // Only the added line is marked, and highlighting attached.
        let changed: Vec<bool> = view.lines.iter().map(|l| l.changed).collect();
        assert_eq!(changed, vec![false, false, false, true, false, false]);
        assert!(!view.lines[0].spans.is_empty());
    }

    #[test]
    fn top_level_and_unknown_language_yield_none() {
        let diff = unidiff::parse(PATCH);
        let top = "use std::fmt;\n\nfn alpha() {}\n";
        assert!(peek(Path::new("x.rs"), top, &diff, 1).is_none());
        assert!(peek(Path::new("x.unknown"), SOURCE, &diff, 4).is_none());
    }
}
