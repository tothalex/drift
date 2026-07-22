//! Every color the UI paints, resolvable from the config file. Values
//! accept ANSI names ("darkgray"), 256-color indexes ("114"), and hex
//! ("#87d787") — whatever `ratatui`'s color parser understands.

use std::collections::HashMap;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use ratatui::style::Color;

pub struct Theme {
    /// Added-change accent: bars, line numbers, diffstat, `A` files.
    pub added: Color,
    /// Removed-change accent.
    pub removed: Color,
    /// Renamed/copied files in the tree.
    pub renamed: Color,
    /// Word-level emphasis backgrounds on changed byte ranges.
    pub emph_added_bg: Color,
    pub emph_removed_bg: Color,
    /// The centered cursorline and the visual/mouse selection.
    pub cursor_bg: Color,
    pub select_bg: Color,
    /// Tree cursor row.
    pub tree_cursor_bg: Color,
    /// Structure: gutters, fold arrows, hints, headers.
    pub muted: Color,
    /// Comment prose and folded comment summaries.
    pub comment: Color,
    /// TODO / FIXME / … review tags inside comments.
    pub tag: Color,
    /// Review-thread accents in the PR view: quote bars and authors.
    pub thread: Color,
    /// Search-hit highlight, in the tree and the code view alike.
    pub search: Color,
    /// Floating panels: the `?` overlay, pickers, the comment composer.
    pub panel_bg: Color,
    /// The VISUAL mode badge.
    pub visual_badge_fg: Color,
    pub visual_badge_bg: Color,
    // Syntax palette.
    pub keyword: Color,
    pub function: Color,
    pub type_: Color,
    pub string: Color,
    pub number: Color,
    pub property: Color,
    pub attribute: Color,
    pub variable: Color,
    pub operator: Color,
    pub arrow: Color,
    pub bracket: Color,
    pub bracket_call: Color,
    pub punctuation: Color,
    /// Per-language syntax overrides: language name → key → color.
    lang: HashMap<String, HashMap<String, Color>>,
}

/// Default values, in config-file syntax — the single source for both the
/// runtime defaults and `--init-config`.
pub const THEME_DEFAULTS: &[(&str, &str)] = &[
    ("added", "114"),
    ("removed", "168"),
    ("renamed", "110"),
    ("emph_added_bg", "22"),
    ("emph_removed_bg", "52"),
    ("cursor_bg", "236"),
    ("select_bg", "238"),
    ("tree_cursor_bg", "darkgray"),
    ("muted", "darkgray"),
    ("comment", "#7f848e"),
    ("tag", "179"),
    ("thread", "110"),
    ("search", "179"),
    ("panel_bg", "235"),
    ("visual_badge_fg", "black"),
    ("visual_badge_bg", "yellow"),
    // Syntax defaults follow onedarkpro's `onedark_dark` palette.
    ("keyword", "#d55fde"),
    ("function", "#61afef"),
    ("type", "#e5c07b"),
    ("string", "#89ca78"),
    ("number", "#d19a66"),
    ("property", "#ef596f"),
    ("attribute", "#d55fde"),
    ("variable", "#ef596f"),
    ("operator", "#2bbac5"),
    ("arrow", "#d55fde"),
    // onedark's per-filetype files paint brackets orange in TS/JS, Rust,
    // and Python; only languages without an override (Go) keep the
    // global purple — see THEME_LANG_DEFAULTS.
    ("bracket", "#d19a66"),
    ("bracket_call", "#d19a66"),
    ("punctuation", "#abb2bf"),
];

/// The syntax-palette subset of the theme — the keys a `[theme.<lang>]`
/// section may override per language.
pub const SYNTAX_KEYS: &[&str] = &[
    "keyword",
    "function",
    "type",
    "string",
    "number",
    "property",
    "attribute",
    "variable",
    "operator",
    "arrow",
    "bracket",
    "bracket_call",
    "punctuation",
    "comment",
];

/// Per-language default overrides, in config-file syntax. onedark colors
/// some tokens differently per language: brackets are orange in TS/JS,
/// Rust, and Python (the base default) but purple in Go, and Rust
/// operators are plain foreground (`@operator.rust`), not cyan.
pub const THEME_LANG_DEFAULTS: &[(&str, &str, &str)] = &[
    ("go", "bracket", "#d55fde"),
    ("rust", "operator", "#abb2bf"),
    ("rust", "bracket_call", "#d55fde"),
];

impl Theme {
    /// Syntax colors for one language: defaults overlaid with the user's
    /// `[theme.<lang>]` entries; `None` when nothing is overridden.
    pub fn for_lang(&self, lang: &str) -> Option<&HashMap<String, Color>> {
        self.lang.get(lang)
    }

    /// Defaults overlaid with the user's `[theme]` entries.
    pub fn from_overrides(overrides: &HashMap<String, String>) -> Result<Theme> {
        Theme::from_all_overrides(overrides, &HashMap::new())
    }

    /// Defaults overlaid with `[theme]` entries plus `[theme.<lang>]`
    /// per-language sections.
    pub fn from_all_overrides(
        overrides: &HashMap<String, String>,
        lang_overrides: &HashMap<String, HashMap<String, String>>,
    ) -> Result<Theme> {
        for name in overrides.keys() {
            if !THEME_DEFAULTS.iter().any(|(known, _)| known == name) {
                bail!(
                    "unknown theme entry '{name}' (known: {})",
                    THEME_DEFAULTS
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        let resolve = |name: &str| -> Result<Color> {
            let default = THEME_DEFAULTS
                .iter()
                .find(|(n, _)| *n == name)
                .expect("theme entry exists")
                .1;
            let value = overrides.get(name).map_or(default, String::as_str);
            Color::from_str(value)
                .ok()
                .with_context(|| format!("theme entry '{name}': invalid color '{value}'"))
        };
        // Per-language sections: defaults first, then the user's entries
        // (which may add languages or replace individual keys).
        let mut lang: HashMap<String, HashMap<String, Color>> = HashMap::new();
        for (language, key, value) in THEME_LANG_DEFAULTS {
            let color = Color::from_str(value).expect("lang default parses");
            lang.entry(language.to_string())
                .or_default()
                .insert(key.to_string(), color);
        }
        let known_langs: Vec<&str> = crate::processor::treesitter::lang_names().collect();
        for (language, entries) in lang_overrides {
            if !known_langs.contains(&language.as_str()) {
                bail!(
                    "unknown language section 'theme.{language}' (known: {})",
                    known_langs.join(", ")
                );
            }
            for (key, value) in entries {
                if !SYNTAX_KEYS.contains(&key.as_str()) {
                    bail!(
                        "theme.{language}: '{key}' is not a syntax key (known: {})",
                        SYNTAX_KEYS.join(", ")
                    );
                }
                let color = Color::from_str(value)
                    .ok()
                    .with_context(|| format!("theme.{language}.{key}: invalid color '{value}'"))?;
                lang.entry(language.clone())
                    .or_default()
                    .insert(key.clone(), color);
            }
        }
        Ok(Theme {
            added: resolve("added")?,
            removed: resolve("removed")?,
            renamed: resolve("renamed")?,
            emph_added_bg: resolve("emph_added_bg")?,
            emph_removed_bg: resolve("emph_removed_bg")?,
            cursor_bg: resolve("cursor_bg")?,
            select_bg: resolve("select_bg")?,
            tree_cursor_bg: resolve("tree_cursor_bg")?,
            muted: resolve("muted")?,
            comment: resolve("comment")?,
            tag: resolve("tag")?,
            thread: resolve("thread")?,
            search: resolve("search")?,
            panel_bg: resolve("panel_bg")?,
            visual_badge_fg: resolve("visual_badge_fg")?,
            visual_badge_bg: resolve("visual_badge_bg")?,
            keyword: resolve("keyword")?,
            function: resolve("function")?,
            type_: resolve("type")?,
            string: resolve("string")?,
            number: resolve("number")?,
            property: resolve("property")?,
            attribute: resolve("attribute")?,
            variable: resolve("variable")?,
            operator: resolve("operator")?,
            arrow: resolve("arrow")?,
            bracket: resolve("bracket")?,
            bracket_call: resolve("bracket_call")?,
            punctuation: resolve("punctuation")?,
            lang,
        })
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from_overrides(&HashMap::new()).expect("defaults parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve() {
        let theme = Theme::default();
        assert_eq!(theme.added, Color::Indexed(114));
        assert_eq!(theme.muted, Color::DarkGray);
    }

    #[test]
    fn overrides_apply_and_accept_hex() {
        let overrides = HashMap::from([("added".to_string(), "#87d787".to_string())]);
        let theme = Theme::from_overrides(&overrides).unwrap();
        assert_eq!(theme.added, Color::Rgb(0x87, 0xd7, 0x87));
    }

    #[test]
    fn unknown_entry_and_bad_color_error() {
        let unknown = HashMap::from([("addde".to_string(), "1".to_string())]);
        assert!(Theme::from_overrides(&unknown).is_err());
        let bad = HashMap::from([("added".to_string(), "not-a-color".to_string())]);
        assert!(Theme::from_overrides(&bad).is_err());
    }
}
