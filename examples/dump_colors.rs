//! Emit drift's resolved foreground color per character, as
//! `row<TAB>col<TAB>char<TAB>#rrggbb` (missing color = `-`). Paired with a
//! headless-neovim dump of the same file, this drives an exact
//! highlighting-parity check. `cargo run --example dump_colors -- <file>`

use std::path::Path;

use drift::processor::highlight::{TokenKind, highlight};
use drift::theme::Theme;
use ratatui::style::Color;

fn hex(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        other => format!("{other:?}"),
    }
}

fn token_color(theme: &Theme, lang: Option<&str>, token: TokenKind) -> Color {
    let (key, base) = match token {
        TokenKind::Keyword => ("keyword", theme.keyword),
        TokenKind::Function => ("function", theme.function),
        TokenKind::Type => ("type", theme.type_),
        TokenKind::String => ("string", theme.string),
        TokenKind::Number | TokenKind::Constant => ("number", theme.number),
        TokenKind::Property => ("property", theme.property),
        TokenKind::Attribute => ("attribute", theme.attribute),
        TokenKind::Comment => ("comment", theme.comment),
        TokenKind::Variable => ("variable", theme.variable),
        TokenKind::Operator => ("operator", theme.operator),
        TokenKind::Arrow => ("arrow", theme.arrow),
        TokenKind::Bracket => ("bracket", theme.bracket),
        TokenKind::CallBracket => ("bracket_call", theme.bracket_call),
        TokenKind::Punctuation => ("punctuation", theme.punctuation),
    };
    lang.and_then(|l| theme.for_lang(l))
        .and_then(|m| m.get(key))
        .copied()
        .unwrap_or(base)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_colors <file>");
    let source = std::fs::read_to_string(&path).expect("read file");
    let theme = Theme::default();
    let lang = drift::processor::treesitter::lang_name(Path::new(&path));
    let hl = highlight(Path::new(&path), &source).expect("supported language");
    let mut out = String::new();
    for (row, line) in source.lines().enumerate() {
        let spans = hl.spans_for(row as u32 + 1);
        // Map each byte offset in the line to its span color, if any.
        for (col, _) in line.char_indices() {
            let color = spans
                .iter()
                .find(|s| s.start <= col && col < s.end)
                .map(|s| hex(token_color(&theme, lang, s.token)))
                .unwrap_or_else(|| "-".to_string());
            let ch = &line[col..line[col..]
                .char_indices()
                .nth(1)
                .map_or(line.len(), |(i, _)| col + i)];
            out.push_str(&format!("{}\t{}\t{}\t{}\n", row + 1, col, ch, color));
        }
    }
    print!("{out}");
}
