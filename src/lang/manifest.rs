//! `language.toml`: the plugin manifest describing one language.
//!
//! ```toml
//! name = "zig"
//! extensions = ["zig", "zon"]
//!
//! [grammar]
//! repo = "https://github.com/tree-sitter-grammars/tree-sitter-zig"
//! rev = "abc123…"
//! block_kinds = ["function_declaration", "if_statement"]
//! ```

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Unknown top-level keys are deliberately tolerated (no
/// `deny_unknown_fields`): a manifest written for a future drift — say
/// with an `[lsp]` section — must still load in this one.
#[derive(Debug, Deserialize)]
struct ManifestFile {
    name: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    grammar: GrammarSection,
}

/// Within `[grammar]` unknown keys are typos, not future features.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrammarSection {
    /// Git URL the sources come from (`drift lang install`/`build`).
    #[serde(default)]
    repo: Option<String>,
    /// Commit to build; the repository's default branch when absent.
    #[serde(default)]
    rev: Option<String>,
    /// Subdirectory holding the grammar, for repositories hosting
    /// several (tree-sitter-typescript: `typescript/` and `tsx/`).
    #[serde(default)]
    path: Option<String>,
    /// Exported language function; `tree_sitter_<name>` when absent.
    #[serde(default)]
    symbol: Option<String>,
    /// Node kinds that count as reviewable blocks.
    #[serde(default)]
    block_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub extensions: Vec<String>,
    pub repo: Option<String>,
    pub rev: Option<String>,
    pub path: Option<String>,
    pub symbol: String,
    pub block_kinds: Vec<String>,
}

pub fn parse(text: &str) -> Result<Manifest> {
    let file: ManifestFile = toml::from_str(text).context("invalid language.toml")?;
    // The name doubles as the dylib filename and the `[theme.<name>]` key.
    if file.name.is_empty()
        || !file
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        bail!(
            "name '{}' must be lowercase ascii letters, digits, '-' or '_'",
            file.name
        );
    }
    if file.extensions.is_empty() {
        bail!("extensions must list at least one file extension");
    }
    for ext in &file.extensions {
        if ext.is_empty() || ext.contains('.') || ext.contains(char::is_whitespace) {
            bail!("extension '{ext}' must be a bare suffix like \"zig\", without the dot");
        }
    }
    let mut deduped = file.extensions.clone();
    deduped.sort_unstable();
    deduped.dedup();
    if deduped.len() != file.extensions.len() {
        bail!("extensions contains duplicates");
    }
    if let Some(path) = file.grammar.path.as_deref()
        && (path.is_empty()
            || Path::new(path).is_absolute()
            || Path::new(path)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_))))
    {
        bail!("grammar.path '{path}' must be a plain subdirectory within the repository");
    }
    Ok(Manifest {
        symbol: file
            .grammar
            .symbol
            .unwrap_or_else(|| format!("tree_sitter_{}", file.name.replace('-', "_"))),
        name: file.name,
        extensions: file.extensions,
        repo: file.grammar.repo,
        rev: file.grammar.rev,
        path: file.grammar.path,
        block_kinds: file.grammar.block_kinds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_manifest_parses() {
        let m = parse(
            "name = \"zig\"\nextensions = [\"zig\", \"zon\"]\n\n[grammar]\nrepo = \"https://example.com/zig\"\nrev = \"abc\"\nsymbol = \"tree_sitter_zig\"\nblock_kinds = [\"function_declaration\"]\n",
        )
        .unwrap();
        assert_eq!(m.name, "zig");
        assert_eq!(m.extensions, vec!["zig", "zon"]);
        assert_eq!(m.repo.as_deref(), Some("https://example.com/zig"));
        assert_eq!(m.rev.as_deref(), Some("abc"));
        assert_eq!(m.symbol, "tree_sitter_zig");
        assert_eq!(m.block_kinds, vec!["function_declaration"]);
    }

    #[test]
    fn symbol_defaults_from_name_with_dashes_mapped() {
        let m = parse("name = \"proto-buf\"\nextensions = [\"proto\"]\n").unwrap();
        assert_eq!(m.symbol, "tree_sitter_proto_buf");
    }

    #[test]
    fn future_sections_are_tolerated() {
        let m = parse(
            "name = \"zig\"\nextensions = [\"zig\"]\n\n[lsp]\ncommand = \"zls\"\nroot_markers = [\"build.zig\"]\n",
        )
        .unwrap();
        assert_eq!(m.name, "zig");
    }

    #[test]
    fn grammar_typos_are_rejected() {
        let err = parse("name = \"zig\"\nextensions = [\"zig\"]\n\n[grammar]\nblock_kind = []\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid language.toml"), "{err}");
    }

    #[test]
    fn grammar_path_selects_a_subdirectory_and_rejects_escapes() {
        let m =
            parse("name = \"tsx\"\nextensions = [\"tsx\"]\n\n[grammar]\npath = \"tsx\"\n").unwrap();
        assert_eq!(m.path.as_deref(), Some("tsx"));
        for bad in ["/abs", "../up", "a/../../b", ""] {
            let toml =
                format!("name = \"x\"\nextensions = [\"x\"]\n\n[grammar]\npath = \"{bad}\"\n");
            assert!(parse(&toml).is_err(), "path '{bad}' must be rejected");
        }
    }

    #[test]
    fn invalid_names_and_extensions_are_rejected() {
        assert!(parse("name = \"\"\nextensions = [\"x\"]\n").is_err());
        assert!(parse("name = \"Zig\"\nextensions = [\"x\"]\n").is_err());
        assert!(parse("name = \"a b\"\nextensions = [\"x\"]\n").is_err());
        assert!(parse("name = \"zig\"\nextensions = []\n").is_err());
        assert!(parse("name = \"zig\"\nextensions = [\".zig\"]\n").is_err());
        assert!(parse("name = \"zig\"\nextensions = [\"zig\", \"zig\"]\n").is_err());
    }
}
