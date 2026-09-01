//! Nerd Font file icons for the tree (`icons = true` / `--icons`).
//! The glyphs are Private Use Area codepoints that only render under a
//! patched font, and a terminal never reports its font — so this is
//! strictly opt-in, never detected.

use ratatui::style::Color;

/// nvim-tree's folder pair; painted in `theme.dir` like the name.
pub const DIR_OPEN: &str = "\u{e5fe}";
pub const DIR_CLOSED: &str = "\u{e5ff}";

const FILE_DEFAULT: (&str, u32) = ("\u{f15b}", 0x9da5b3);

/// Icon and color for a file name. Colors follow the devicon
/// conventions (orange Rust, blue TypeScript, …), fixed values rather
/// than theme keys — thirty extra theme entries would be noise.
pub fn file(name: &str) -> (&'static str, Color) {
    let lower = name.to_lowercase();
    let (glyph, rgb) = by_filename(&lower)
        .or_else(|| {
            lower
                .rsplit_once('.')
                .and_then(|(_, ext)| by_extension(ext))
        })
        .unwrap_or(FILE_DEFAULT);
    (glyph, rgb_color(rgb))
}

fn rgb_color(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Whole-name matches — files whose identity isn't in an extension.
fn by_filename(lower: &str) -> Option<(&'static str, u32)> {
    Some(match lower {
        "dockerfile" | "containerfile" => ("\u{f308}", 0x458ee6),
        "makefile" | "justfile" => ("\u{f489}", 0x6d8086),
        ".gitignore" | ".gitattributes" | ".gitmodules" => ("\u{e702}", 0xf14c28),
        _ => return None,
    })
}

fn by_extension(ext: &str) -> Option<(&'static str, u32)> {
    Some(match ext {
        "rs" => ("\u{e7a8}", 0xdea584),
        "go" => ("\u{e626}", 0x519aba),
        "py" | "pyi" => ("\u{e73c}", 0xffbc03),
        "ts" | "mts" | "cts" => ("\u{e628}", 0x519aba),
        "js" | "mjs" | "cjs" => ("\u{e74e}", 0xcbcb41),
        "tsx" | "jsx" => ("\u{e7ba}", 0x61dafb),
        "rb" | "erb" => ("\u{e791}", 0x701516),
        "java" => ("\u{e738}", 0xcc3e44),
        "c" | "h" => ("\u{e61e}", 0x599eff),
        "cpp" | "cc" | "cxx" | "hpp" => ("\u{e61d}", 0xf34b7d),
        "php" => ("\u{e73d}", 0xa074c4),
        "swift" => ("\u{e755}", 0xe37933),
        "kt" | "kts" => ("\u{e634}", 0x7f52ff),
        "lua" => ("\u{e620}", 0x51a0cf),
        "sh" | "bash" | "zsh" | "fish" => ("\u{f489}", 0x89e051),
        "html" | "htm" => ("\u{e736}", 0xe44d26),
        "css" | "scss" | "sass" => ("\u{e749}", 0x42a5f5),
        "json" | "jsonc" => ("\u{e60b}", 0xcbcb41),
        "yaml" | "yml" | "toml" | "ini" | "conf" | "cfg" => ("\u{e615}", 0x6d8086),
        "md" | "markdown" => ("\u{f48a}", 0x519aba),
        "lock" => ("\u{f023}", 0xd0bf41),
        "txt" => ("\u{f15c}", 0x9da5b3),
        "sql" | "db" => ("\u{f1c0}", 0xdad8d8),
        "vim" => ("\u{e62b}", 0x019833),
        "xml" | "csv" => ("\u{f121}", 0xe37933),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" => ("\u{f1c5}", 0xa074c4),
        "pdf" => ("\u{f1c1}", 0xb30b00),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" => ("\u{f1c6}", 0xeca517),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_and_filename_lookups() {
        assert_eq!(file("main.rs").0, "\u{e7a8}");
        assert_eq!(file("APP.TSX").0, "\u{e7ba}"); // case-insensitive
        assert_eq!(file("Dockerfile").0, "\u{f308}"); // filename beats extension-less
        assert_eq!(file(".gitignore").0, "\u{e702}"); // leading dot isn't an extension
        assert_eq!(file("Cargo.lock").0, "\u{f023}");
    }

    #[test]
    fn unknown_files_fall_back() {
        assert_eq!(file("mystery.xyz").0, FILE_DEFAULT.0);
        assert_eq!(file("no-extension").0, FILE_DEFAULT.0);
    }

    #[test]
    fn every_glyph_is_one_column() {
        // Rows budget exactly two cells per icon (glyph + space); a
        // double-width glyph would shear the tree.
        use unicode_width::UnicodeWidthStr;
        for glyph in [DIR_OPEN, DIR_CLOSED, FILE_DEFAULT.0] {
            assert_eq!(glyph.width(), 1, "{glyph:?}");
        }
        for name in ["a.rs", "a.go", "a.py", "a.md", "a.png", "Makefile"] {
            assert_eq!(file(name).0.width(), 1, "{name}");
        }
    }
}
