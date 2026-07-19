//! Comment presentation: detecting comment-only lines, stripping the
//! syntax markers for prose display, summaries for folded blocks, and
//! review tags (TODO/FIXME/…) worth accenting.

use super::highlight::{HighlightSpan, TokenKind};

/// True when every non-whitespace character of `content` sits inside a
/// comment span — the line is nothing but comment.
pub fn is_comment_line(content: &str, spans: &[HighlightSpan]) -> bool {
    if content.trim().is_empty() {
        return false;
    }
    content.char_indices().all(|(offset, ch)| {
        ch.is_whitespace()
            || spans
                .iter()
                .any(|s| s.token == TokenKind::Comment && s.start <= offset && offset < s.end)
    })
}

/// Strip comment markers for prose display: `// text` → `text`.
pub fn strip_markers(content: &str) -> &str {
    let mut text = content.trim();
    for marker in ["///", "//!", "//", "/**", "/*", "*/", "*", "#!", "#"] {
        if let Some(rest) = text.strip_prefix(marker) {
            text = rest;
            break;
        }
    }
    text.strip_suffix("*/").unwrap_or(text).trim()
}

/// Byte length of a leading review tag (`TODO`, `FIXME:`, …), if present.
pub fn tag_len(text: &str) -> Option<usize> {
    const TAGS: &[&str] = &["TODO", "FIXME", "HACK", "SAFETY", "NOTE", "WARNING", "XXX"];
    TAGS.iter().find_map(|tag| {
        let rest = text.strip_prefix(tag)?;
        match rest.chars().next() {
            Some(c) if c.is_alphanumeric() => None, // e.g. "NOTEBOOK"
            Some(':') => Some(tag.len() + 1),
            _ => Some(tag.len()),
        }
    })
}

/// One-line summary for a folded comment block.
pub fn summary(first_line: &str, max_chars: usize) -> String {
    let text = strip_markers(first_line);
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment_span(start: usize, end: usize) -> HighlightSpan {
        HighlightSpan {
            start,
            end,
            token: TokenKind::Comment,
        }
    }

    #[test]
    fn detects_full_comment_lines() {
        let content = "    // all comment";
        assert!(is_comment_line(content, &[comment_span(4, content.len())]));
        // Trailing comment after code is not a comment line.
        let mixed = "let a = 1; // trailing";
        assert!(!is_comment_line(mixed, &[comment_span(11, mixed.len())]));
        // Blank lines are not comments.
        assert!(!is_comment_line("   ", &[]));
    }

    #[test]
    fn strips_markers() {
        assert_eq!(strip_markers("  // plain text"), "plain text");
        assert_eq!(strip_markers("/// doc line"), "doc line");
        assert_eq!(strip_markers("# python style"), "python style");
        assert_eq!(strip_markers("/* boxed */"), "boxed");
        assert_eq!(strip_markers(" * continuation"), "continuation");
        assert_eq!(strip_markers("//"), "");
    }

    #[test]
    fn finds_tags() {
        assert_eq!(tag_len("TODO: support stacking"), Some(5));
        assert_eq!(tag_len("FIXME later"), Some(5));
        assert_eq!(tag_len("NOTEBOOK is a word"), None);
        assert_eq!(tag_len("nothing here"), None);
    }

    #[test]
    fn summarizes_and_truncates() {
        assert_eq!(summary("// short", 48), "short");
        let long = format!("// {}", "x".repeat(60));
        assert_eq!(summary(&long, 10).chars().count(), 10);
        assert!(summary(&long, 10).ends_with('…'));
    }
}
