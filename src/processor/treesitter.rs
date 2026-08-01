//! Tree-sitter block resolver: parses a file with the grammar the
//! language registry (`crate::lang`) prescribes and walks the tree for
//! the blocks enclosing a change.

use std::path::Path;

use tree_sitter::{Node, Parser, Tree};

use crate::lang::{LangSpec, spec_for};

use super::blocks::{Block, BlockResolver};

pub struct TsResolver<'a> {
    source: &'a str,
    tree: Tree,
    spec: &'static LangSpec,
    /// Byte offset of each line start; index i = 1-based line i+1.
    line_starts: Vec<usize>,
}

impl<'a> TsResolver<'a> {
    /// `None` when the extension is unknown or parsing is impossible —
    /// callers fall back to plain hunk sections.
    pub fn new(path: &Path, source: &'a str) -> Option<Self> {
        let spec = spec_for(path)?;
        let mut parser = Parser::new();
        parser.set_language(&spec.language()).ok()?;
        let tree = parser.parse(source, None)?;

        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        Some(TsResolver {
            source,
            tree,
            spec,
            line_starts,
        })
    }

    pub(super) fn tree(&self) -> &Tree {
        &self.tree
    }

    pub(super) fn spec(&self) -> &'static LangSpec {
        self.spec
    }

    /// Byte range covering 1-based lines `start..=end`, clamped.
    pub(super) fn byte_range_of_lines(&self, start: u32, end: u32) -> (usize, usize) {
        let (from, _) = self.line_bytes(start);
        let (_, to) = self.line_bytes(end.max(start));
        (from, to)
    }

    /// Byte range (inclusive start, exclusive end) of 1-based line `n`,
    /// clamped to the file.
    fn line_bytes(&self, n: u32) -> (usize, usize) {
        let i = (n as usize)
            .saturating_sub(1)
            .min(self.line_starts.len() - 1);
        let start = self.line_starts[i];
        let end = self
            .line_starts
            .get(i + 1)
            .map_or(self.source.len(), |next| next - 1);
        (start, end)
    }

    fn block_from(&self, node: Node) -> Block {
        let start = node.start_position().row as u32 + 1;
        let mut end = node.end_position().row as u32 + 1;
        // A node ending exactly at column 0 doesn't occupy that line.
        if node.end_position().column == 0 && end > start {
            end -= 1;
        }
        let first_line = self.source[node.start_byte()..]
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches('{')
            .trim_end();
        Block {
            range: (start, end),
            title: truncate(first_line, 60),
        }
    }
}

impl BlockResolver for TsResolver<'_> {
    fn enclosing_blocks(&self, changed: (u32, u32)) -> Vec<Block> {
        let (line_start, line_end) = self.line_bytes(changed.0);
        // Skip the first line's indentation: those whitespace bytes belong
        // to the *enclosing* node and would widen the walk by one level.
        let content = &self.source[line_start..line_end];
        let start = line_start + (content.len() - content.trim_start().len());
        let (_, end) = self.line_bytes(changed.1.max(changed.0));
        let mut blocks = Vec::new();
        let Some(mut node) = self
            .tree
            .root_node()
            .descendant_for_byte_range(start, end.max(start))
        else {
            return blocks;
        };
        loop {
            if self.spec.block_kinds().contains(&node.kind()) {
                let block = self.block_from(node);
                // Skip wrappers with the same span (e.g. a decorated
                // definition around a function) — not a useful level.
                if blocks.last().map(|b: &Block| b.range) != Some(block.range) {
                    blocks.push(block);
                }
            }
            match node.parent() {
                Some(parent) => node = parent,
                None => return blocks,
            }
        }
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST_SRC: &str = "\
use std::fmt;

fn alpha() {
    let a = 1;
    if a > 0 {
        println!(\"positive\");
    }
}

fn beta() -> u32 {
    42
}
";

    fn resolver<'a>(src: &'a str, file: &str) -> TsResolver<'a> {
        TsResolver::new(Path::new(file), src).expect("resolver")
    }

    #[test]
    fn chain_is_innermost_first() {
        let r = resolver(RUST_SRC, "x.rs");
        // Line 6 is inside the if inside alpha: chain = [if, fn].
        let chain = r.enclosing_blocks((6, 6));
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].range, (5, 7));
        assert!(chain[0].title.starts_with("if a > 0"));
        assert_eq!(chain[1].range, (3, 8));
        assert_eq!(chain[1].title, "fn alpha()");
        // Line 4 is directly in alpha's body: only the fn.
        let chain = r.enclosing_blocks((4, 4));
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].range, (3, 8));
    }

    #[test]
    fn top_level_change_has_no_block() {
        let r = resolver(RUST_SRC, "x.rs");
        assert!(r.enclosing_blocks((1, 1)).is_empty());
    }

    #[test]
    fn span_across_siblings_walks_up_to_none_at_top_level() {
        let r = resolver(RUST_SRC, "x.rs");
        // Covers end of alpha and start of beta → source_file → no block.
        assert!(r.enclosing_blocks((7, 10)).is_empty());
    }

    #[test]
    fn python_method_resolves_to_method_then_class() {
        let src = "\
class Greeter:
    def greet(self, name):
        message = f\"hi {name}\"
        return message
";
        let r = resolver(src, "x.py");
        let chain = r.enclosing_blocks((3, 3));
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].range, (2, 4));
        assert!(chain[0].title.starts_with("def greet"));
        assert!(chain[1].title.starts_with("class Greeter"));
    }

    #[test]
    fn unknown_extension_is_rejected() {
        assert!(TsResolver::new(Path::new("notes.txt"), "hello").is_none());
    }
}
