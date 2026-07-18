//! Every color the UI paints, resolvable from the config file. Values
//! accept ANSI names ("darkgray"), 256-color indexes ("114"), and hex
//! ("#87d787") — whatever `ratatui`'s color parser understands.

use std::collections::HashMap;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
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
    /// Matched characters during tree search.
    pub search: Color,
    /// The `?` overlay background.
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
    ("comment", "241"),
    ("tag", "179"),
    ("search", "179"),
    ("panel_bg", "235"),
    ("visual_badge_fg", "black"),
    ("visual_badge_bg", "yellow"),
    ("keyword", "176"),
    ("function", "75"),
    ("type", "180"),
    ("string", "114"),
    ("number", "173"),
    ("property", "73"),
    ("attribute", "180"),
];

impl Theme {
    /// Defaults overlaid with the user's `[theme]` entries.
    pub fn from_overrides(overrides: &HashMap<String, String>) -> Result<Theme> {
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
