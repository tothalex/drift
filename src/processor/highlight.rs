//! Syntax highlighting by running the grammar's highlight query directly
//! over an existing parse tree — no second parse, and the query can be
//! restricted to the byte ranges that will actually be displayed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor, Tree};

use super::treesitter;
use super::treesitter::LangSpec;

/// Semantic token classes the UI knows how to color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Attribute,
    Comment,
    Constant,
    Function,
    Keyword,
    Number,
    Operator,
    Property,
    String,
    Type,
    Variable,
    /// The `=>` fat arrow — its own token so it can be themed apart from
    /// other operators, the way onedark paints it.
    Arrow,
    /// Brackets: `()`, `{}`, `[]`.
    Bracket,
    /// Delimiters and interpolation markers (`.`, `,`, `;`, `${`, `}`) —
    /// rendered as plain foreground, which also stops a template
    /// literal's string color from bleeding across its `${…}` holes.
    Punctuation,
}

/// Capture names we recognize, with the token each maps to. Dotted capture
/// names in queries (e.g. `function.method`) match their longest prefix.
const CAPTURES: &[(&str, TokenKind)] = &[
    ("attribute", TokenKind::Attribute),
    ("comment", TokenKind::Comment),
    ("constant", TokenKind::Constant),
    // Class/constructor names take the type color (yellow in onedark),
    // not the function color: `new Error(…)`, `StockLockTier`.
    ("constructor", TokenKind::Type),
    ("function", TokenKind::Function),
    ("keyword", TokenKind::Keyword),
    ("label", TokenKind::Keyword),
    ("number", TokenKind::Number),
    ("operator", TokenKind::Operator),
    ("property", TokenKind::Property),
    ("punctuation.bracket", TokenKind::Bracket),
    ("punctuation.delimiter", TokenKind::Punctuation),
    ("punctuation.special", TokenKind::Punctuation),
    ("string", TokenKind::String),
    ("tag", TokenKind::Function),
    ("type", TokenKind::Type),
    ("variable", TokenKind::Variable),
    // Builtins (`this`, `self`) take the type color, like onedarkpro's
    // yellow `@variable.builtin`.
    ("variable.builtin", TokenKind::Type),
];

impl TokenKind {
    /// Precedence when two captures cover the exact same bytes. A name is
    /// often captured several ways at once (`parse` is both
    /// `@function.method` and `@property`; a function definition is both
    /// `@function` and `@variable`); the most specific role should win,
    /// the way an editor resolves it — call sites beat plain property or
    /// variable reads.
    fn rank(self) -> u8 {
        match self {
            // A template substitution's `}` is captured as both a bracket
            // and interpolation punctuation; the foreground punctuation
            // must win so it doesn't render as a stray purple bracket.
            TokenKind::Punctuation => 7,
            // Names with a distinct role, most specific first.
            TokenKind::Function => 5,
            TokenKind::Type => 4,
            TokenKind::Constant => 3,
            TokenKind::Property => 2,
            TokenKind::Variable => 1,
            // Non-name tokens (keywords, strings, numbers, …) don't
            // meaningfully collide with names; keep whichever came last.
            _ => 6,
        }
    }
}

fn token_for_capture(name: &str) -> Option<TokenKind> {
    CAPTURES
        .iter()
        .filter(|(known, _)| {
            name == *known || name.strip_prefix(known).is_some_and(|r| r.starts_with('.'))
        })
        .max_by_key(|(known, _)| known.len())
        .map(|(_, token)| *token)
}

/// A compiled highlight query plus its capture-index → token table.
struct CompiledQuery {
    query: Query,
    tokens: Vec<Option<TokenKind>>,
}

/// Query compilation costs tens of milliseconds; each language compiles
/// exactly once.
fn query_for(spec: &'static LangSpec) -> Option<Arc<CompiledQuery>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Option<Arc<CompiledQuery>>>>> = OnceLock::new();
    let key = std::ptr::from_ref(spec) as usize;
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry(key)
        .or_insert_with(|| {
            let source = spec.highlight_query_parts().join("\n");
            let query = Query::new(&spec.language(), &source).ok()?;
            let tokens = query
                .capture_names()
                .iter()
                .map(|name| token_for_capture(name))
                .collect();
            Some(Arc::new(CompiledQuery { query, tokens }))
        })
        .clone()
}

/// A highlighted region within a single line, in byte offsets relative to
/// that line's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub token: TokenKind,
}

/// Per-line highlight spans for one source file.
#[derive(Debug, Default)]
pub struct FileHighlights {
    /// Index 0 = line 1. Spans are ordered and non-overlapping.
    lines: Vec<Vec<HighlightSpan>>,
}

impl FileHighlights {
    pub fn spans_for(&self, lineno: u32) -> &[HighlightSpan] {
        self.lines
            .get(lineno as usize - 1)
            .map_or(&[], Vec::as_slice)
    }
}

/// Highlight `source` using its existing parse `tree`, optionally limited
/// to the given byte ranges (what will actually be displayed). `None` when
/// the language has no working highlight query.
pub(crate) fn highlight_tree(
    spec: &'static LangSpec,
    tree: &Tree,
    source: &str,
    byte_ranges: Option<&[(usize, usize)]>,
) -> Option<FileHighlights> {
    let compiled = query_for(spec)?;
    let ranges = match byte_ranges {
        // Merge overlapping/adjacent ranges only: measurement showed that
        // restricted query passes are cheap while queried text dominates,
        // so bridging gaps (scanning extra text) is a net loss.
        Some(ranges) => coalesce(ranges.to_vec(), 0),
        None => vec![(0, source.len())],
    };

    // Collect global spans; nodes never partially overlap, so overlapping
    // captures are nested (or identical ranges from multiple patterns).
    let mut collected: Vec<(usize, usize, TokenKind)> = Vec::new();
    let mut cursor = QueryCursor::new();
    for (start, end) in ranges {
        cursor.set_byte_range(start..end.min(source.len()));
        let mut matches = cursor.matches(&compiled.query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            for capture in m.captures {
                let Some(mut token) = compiled.tokens[capture.index as usize] else {
                    continue;
                };
                let node = capture.node;
                let (start, end) = (node.start_byte(), node.end_byte());
                // The grammar tags `=>` as a plain operator; promote it to
                // its own token so it can be themed like onedark's arrow.
                if token == TokenKind::Operator && &source[start..end] == "=>" {
                    token = TokenKind::Arrow;
                }
                collected.push((start, end, token));
            }
        }
    }
    collected.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    collected.dedup();

    // Collapse captures over the exact same bytes to a single token by
    // precedence (a name is often captured several ways at once). Doing
    // it here, before the nesting pass, is what keeps it correct: an
    // enclosing span (a template string over its `${…}`) otherwise splits
    // and pushes a fragment between two identical-range captures, so they
    // never become adjacent for the nesting pass to merge.
    let mut merged: Vec<(usize, usize, TokenKind)> = Vec::with_capacity(collected.len());
    for (start, end, token) in collected {
        match merged.last_mut() {
            Some(last) if last.0 == start && last.1 == end => {
                if token.rank() >= last.2.rank() {
                    last.2 = token;
                }
            }
            _ => merged.push((start, end, token)),
        }
    }

    // Resolve nesting: inner (later-starting) captures win, splitting the
    // outer span around them — matching editor semantics.
    let mut flat: Vec<(usize, usize, TokenKind)> = Vec::new();
    for (start, end, token) in merged {
        match flat.last_mut() {
            Some(last) if last.1 > start => {
                let tail = (end, last.1, last.2);
                last.1 = start;
                if last.1 == last.0 {
                    flat.pop();
                }
                flat.push((start, end, token));
                if tail.1 > tail.0 {
                    flat.push(tail);
                }
            }
            _ => flat.push((start, end, token)),
        }
    }

    let line_starts = line_starts(source);
    let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new(); line_starts.len()];
    for (start, end, token) in flat {
        push_split_by_line(&mut lines, &line_starts, source, start, end, token);
    }
    Some(FileHighlights { lines })
}

/// Convenience: parse and highlight a whole file (no tree at hand).
pub fn highlight(path: &Path, source: &str) -> Option<FileHighlights> {
    let resolver = treesitter::TsResolver::new(path, source)?;
    highlight_tree(resolver.spec(), resolver.tree(), source, None)
}

/// Sort and merge byte ranges, joining neighbors closer than `gap`.
fn coalesce(mut ranges: Vec<(usize, usize)>, gap: usize) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 + gap => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

pub fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// Record byte range `start..end` of `source` as `token`, split per line
/// with line-relative offsets, excluding newline characters.
fn push_split_by_line(
    lines: &mut [Vec<HighlightSpan>],
    line_starts: &[usize],
    source: &str,
    start: usize,
    end: usize,
    token: TokenKind,
) {
    let first = line_starts.partition_point(|&s| s <= start) - 1;
    for (i, &line_start) in line_starts.iter().enumerate().skip(first) {
        if line_start >= end {
            break;
        }
        let line_end = line_starts.get(i + 1).map_or(source.len(), |next| next - 1);
        let seg_start = start.max(line_start);
        let seg_end = end.min(line_end);
        if seg_start < seg_end {
            lines[i].push(HighlightSpan {
                start: seg_start - line_start,
                end: seg_end - line_start,
                token,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_tokens_per_line() {
        let source = "fn main() {\n    let msg = \"hello\";\n}\n";
        let hl = highlight(Path::new("x.rs"), source).expect("highlights");

        // Line 1: `fn` is a keyword at 0..2, `main` a function.
        let line1 = hl.spans_for(1);
        assert!(
            line1
                .iter()
                .any(|s| s.token == TokenKind::Keyword && (s.start, s.end) == (0, 2))
        );
        assert!(line1.iter().any(|s| s.token == TokenKind::Function));

        // Line 2: the string literal, offsets relative to the line.
        let line2 = hl.spans_for(2);
        let string_span = line2
            .iter()
            .find(|s| s.token == TokenKind::String)
            .expect("string span");
        assert_eq!(
            &source.lines().nth(1).unwrap()[string_span.start..string_span.end],
            "\"hello\""
        );
    }

    #[test]
    fn spans_never_cross_lines_or_overlap() {
        let source = "/* multi\nline\ncomment */\n";
        let hl = highlight(Path::new("x.rs"), source).expect("highlights");
        for lineno in 1..=3 {
            let spans = hl.spans_for(lineno);
            let line_len = source.lines().nth(lineno as usize - 1).unwrap().len();
            for pair in spans.windows(2) {
                assert!(pair[0].end <= pair[1].start, "overlap on line {lineno}");
            }
            for span in spans {
                assert!(span.end <= line_len, "span exceeds line {lineno}");
            }
        }
        assert!(!hl.spans_for(2).is_empty());
    }

    #[test]
    fn typescript_queries_build() {
        let source = "export function greet(name: string): string {\n  return `hi ${name}`;\n}\n";
        let hl = highlight(Path::new("x.ts"), source).expect("ts highlights");
        assert!(
            hl.spans_for(1)
                .iter()
                .any(|s| s.token == TokenKind::Keyword)
        );
    }

    #[test]
    fn method_calls_and_types_beat_property_and_variable() {
        // `parse` is captured as both @function.method and @property, and
        // a class name as @constructor + @type + @variable; precedence
        // must land on the call/type role, matching editor coloring.
        let source = "const t = JSON.parse(x);\nconst c = new StockLockTier();\n";
        let hl = highlight(Path::new("x.ts"), source).expect("ts highlights");
        let token_of = |lineno: u32, needle: &str| {
            let line = source.lines().nth(lineno as usize - 1).unwrap();
            let at = line.find(needle).unwrap();
            hl.spans_for(lineno)
                .iter()
                .find(|s| s.start == at)
                .map(|s| s.token)
        };
        assert_eq!(token_of(1, "parse"), Some(TokenKind::Function));
        assert_eq!(token_of(1, "JSON"), Some(TokenKind::Type));
        assert_eq!(token_of(1, "="), Some(TokenKind::Operator));
        assert_eq!(token_of(2, "StockLockTier"), Some(TokenKind::Type));
    }

    #[test]
    fn template_interpolation_does_not_overlap_or_bleed() {
        // The string spans the whole `` `…${…}` ``; the interpolation must
        // punch clean, non-overlapping holes so code inside is not green.
        let source = "const s = `R-${Date.now()}`;\n";
        let hl = highlight(Path::new("x.ts"), source).expect("ts highlights");
        let spans = hl.spans_for(1);
        for pair in spans.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "overlapping spans: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        let line = source.lines().next().unwrap();
        let token_at = |needle: &str| {
            let at = line.find(needle).unwrap();
            spans.iter().find(|s| s.start == at).map(|s| s.token)
        };
        assert_eq!(token_at("${"), Some(TokenKind::Punctuation));
        assert_eq!(token_at("Date"), Some(TokenKind::Type));
        // The `.` inside the interpolation is punctuation, not string green.
        assert_eq!(token_at("."), Some(TokenKind::Punctuation));
    }

    #[test]
    fn brackets_and_fat_arrow_get_their_own_tokens() {
        let source = "const f = (a: number) => [a];\n";
        let hl = highlight(Path::new("x.ts"), source).expect("ts highlights");
        let line = source.lines().next().unwrap();
        let token_of = |needle: &str| {
            let at = line.find(needle).unwrap();
            hl.spans_for(1)
                .iter()
                .find(|s| s.start == at)
                .map(|s| s.token)
        };
        assert_eq!(token_of("("), Some(TokenKind::Bracket));
        assert_eq!(token_of("["), Some(TokenKind::Bracket));
        // The fat arrow is promoted off the generic operator token…
        assert_eq!(token_of("=>"), Some(TokenKind::Arrow));
        // …while a plain `=` stays an operator.
        assert_eq!(token_of("="), Some(TokenKind::Operator));
    }

    #[test]
    fn ranged_highlight_skips_outside_lines() {
        let source = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let resolver = treesitter::TsResolver::new(Path::new("x.rs"), source).unwrap();
        // Only the second line's byte range.
        let hl = highlight_tree(resolver.spec(), resolver.tree(), source, Some(&[(10, 19)]))
            .expect("highlights");
        assert!(!hl.spans_for(2).is_empty());
        assert!(hl.spans_for(3).is_empty());
    }

    #[test]
    fn unknown_language_is_none() {
        assert!(highlight(Path::new("notes.txt"), "plain").is_none());
    }

    #[test]
    fn coalesce_merges_nearby_and_keeps_distant() {
        let merged = coalesce(vec![(500, 600), (0, 100), (150, 200)], 64);
        assert_eq!(merged, vec![(0, 200), (500, 600)]);
        let merged = coalesce(vec![(0, 10), (5, 30)], 0);
        assert_eq!(merged, vec![(0, 30)]);
    }

    #[test]
    fn out_of_range_line_is_empty() {
        let hl = highlight(Path::new("x.rs"), "fn a() {}\n").unwrap();
        assert!(hl.spans_for(99).is_empty());
    }
}
