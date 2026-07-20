//! Tree-sitter block resolver and the language registry.
//!
//! Adding a language = one `LangSpec` entry + one grammar crate.

use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use super::blocks::{Block, BlockResolver};

pub(crate) struct LangSpec {
    /// Stable identifier for per-language theming (`[theme.rust]`).
    pub(crate) name: &'static str,
    extensions: &'static [&'static str],
    language: fn() -> Language,
    /// Node kinds that count as reviewable blocks, innermost-first walk.
    block_kinds: &'static [&'static str],
    /// Highlight query sources, concatenated at build time (some grammars
    /// layer on a base language's query, e.g. TypeScript over JavaScript).
    highlight_queries: &'static [&'static str],
}

impl LangSpec {
    pub(super) fn language(&self) -> Language {
        (self.language)()
    }

    pub(super) fn highlight_query_parts(&self) -> &'static [&'static str] {
        self.highlight_queries
    }
}

/// Registry lookup by file extension.
pub(super) fn spec_for(path: &Path) -> Option<&'static LangSpec> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES.iter().find(|spec| spec.extensions.contains(&ext))
}

/// Language identifier for a path, for per-language theming.
pub fn lang_name(path: &Path) -> Option<&'static str> {
    spec_for(path).map(|spec| spec.name)
}

/// All language identifiers the registry knows (config validation).
pub(crate) fn lang_names() -> impl Iterator<Item = &'static str> {
    LANGUAGES.iter().map(|spec| spec.name)
}

fn lang_rust() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}
fn lang_python() -> Language {
    tree_sitter_python::LANGUAGE.into()
}
fn lang_javascript() -> Language {
    tree_sitter_javascript::LANGUAGE.into()
}
fn lang_typescript() -> Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}
fn lang_tsx() -> Language {
    tree_sitter_typescript::LANGUAGE_TSX.into()
}
fn lang_go() -> Language {
    tree_sitter_go::LANGUAGE.into()
}

const JS_BLOCK_KINDS: &[&str] = &[
    // Wraps declarations; without it, spans touching the `export` keyword
    // would walk past the function to top level.
    "export_statement",
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "arrow_function",
    "method_definition",
    "class_declaration",
    "if_statement",
    "for_statement",
    "for_in_statement",
    "while_statement",
    "do_statement",
    "switch_statement",
    "try_statement",
];

const TS_BLOCK_KINDS: &[&str] = &[
    "export_statement",
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "arrow_function",
    "method_definition",
    "class_declaration",
    "abstract_class_declaration",
    "interface_declaration",
    "enum_declaration",
    "type_alias_declaration",
    "if_statement",
    "for_statement",
    "for_in_statement",
    "while_statement",
    "do_statement",
    "switch_statement",
    "try_statement",
];

const LANGUAGES: &[LangSpec] = &[
    LangSpec {
        name: "rust",
        extensions: &["rs"],
        language: lang_rust,
        block_kinds: &[
            "function_item",
            "impl_item",
            "trait_item",
            "struct_item",
            "enum_item",
            "mod_item",
            "macro_definition",
            "if_expression",
            "for_expression",
            "while_expression",
            "loop_expression",
            "match_expression",
        ],
        highlight_queries: &[tree_sitter_rust::HIGHLIGHTS_QUERY],
    },
    LangSpec {
        name: "python",
        extensions: &["py"],
        language: lang_python,
        block_kinds: &[
            "function_definition",
            "class_definition",
            "decorated_definition",
            "if_statement",
            "for_statement",
            "while_statement",
            "with_statement",
            "try_statement",
            "match_statement",
        ],
        highlight_queries: &[tree_sitter_python::HIGHLIGHTS_QUERY],
    },
    LangSpec {
        name: "javascript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        language: lang_javascript,
        block_kinds: JS_BLOCK_KINDS,
        highlight_queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        ],
    },
    LangSpec {
        name: "typescript",
        extensions: &["ts", "mts", "cts"],
        language: lang_typescript,
        block_kinds: TS_BLOCK_KINDS,
        highlight_queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
    },
    LangSpec {
        name: "tsx",
        extensions: &["tsx"],
        language: lang_tsx,
        block_kinds: TS_BLOCK_KINDS,
        highlight_queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
    },
    LangSpec {
        name: "go",
        extensions: &["go"],
        language: lang_go,
        block_kinds: &[
            "function_declaration",
            "method_declaration",
            "type_declaration",
            "if_statement",
            "for_statement",
            "expression_switch_statement",
            "type_switch_statement",
            "select_statement",
        ],
        highlight_queries: &[tree_sitter_go::HIGHLIGHTS_QUERY],
    },
];

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
        parser.set_language(&(spec.language)()).ok()?;
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
            if self.spec.block_kinds.contains(&node.kind()) {
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
