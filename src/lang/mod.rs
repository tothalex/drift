//! The language registry: everything drift knows about a language —
//! its name, file extensions, tree-sitter grammar, reviewable block
//! kinds, and highlight queries. Sits below the processor (and any
//! future LSP client): consumers look languages up here and never
//! carry grammar knowledge of their own.
//!
//! No grammar is compiled into drift. Every language is a plugin — a
//! manifest, query file and dylib-compiled grammar per language under
//! the config directory's `languages/`, loaded at startup by
//! [`init_plugins`] and installable from inside the app (curated
//! languages prompt on first sight; [`register_installed`] hot-loads
//! the result). The curated registry ships complete manifests and
//! query stacks embedded from the repo's `languages/` directory.

pub mod cli;
pub mod compile;
mod curated;
mod loader;
mod manifest;

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use tree_sitter::Language;
use tree_sitter_language::LanguageFn;

pub(crate) struct LangSpec {
    /// Stable identifier for per-language theming (`[theme.rust]`).
    pub(crate) name: &'static str,
    extensions: &'static [&'static str],
    /// The grammar's language function; for plugins it points into a
    /// dylib the loader keeps mapped for the process lifetime.
    grammar: LanguageFn,
    /// Node kinds that count as reviewable blocks, innermost-first walk.
    block_kinds: &'static [&'static str],
    /// Highlight query sources, concatenated when compiled.
    highlight_queries: &'static [&'static str],
}

impl LangSpec {
    pub(crate) fn language(&self) -> Language {
        self.grammar.into()
    }

    pub(crate) fn block_kinds(&self) -> &'static [&'static str] {
        self.block_kinds
    }

    pub(crate) fn highlight_query_parts(&self) -> &'static [&'static str] {
        self.highlight_queries
    }
}

/// Loaded languages. Grows at startup ([`init_plugins`]) and after an
/// in-app install ([`register_installed`]); specs are leaked so lookups
/// hand out `&'static` references.
static REGISTRY: RwLock<Vec<&'static LangSpec>> = RwLock::new(Vec::new());

/// Load plugin languages from the user's `languages/` directory. Call
/// once at startup, before the config parses (`[theme.<lang>]` sections
/// validate against the registry). Returns one human-readable warning
/// per plugin that failed to load — a broken plugin never stops drift.
pub fn init_plugins() -> Vec<String> {
    init_plugins_at(&languages_dir(), &grammar_cache_dir())
}

/// [`init_plugins`] with explicit directories (tests).
pub fn init_plugins_at(languages_dir: &Path, cache_dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let specs = loader::load_all(languages_dir, cache_dir, &mut warnings);
    let mut registry = REGISTRY.write().unwrap();
    for spec in specs {
        if !registry.iter().any(|s| s.name == spec.name) {
            registry.push(Box::leak(Box::new(spec)));
        }
    }
    warnings
}

/// Load one just-installed plugin into the live registry — the in-app
/// install path, where drift keeps running. A language of the same name
/// is replaced (old leaked specs stay valid for views already built).
pub fn register_installed(name: &str) -> anyhow::Result<()> {
    let spec = loader::load_one(&languages_dir().join(name), &grammar_cache_dir())?;
    let mut registry = REGISTRY.write().unwrap();
    if let Some(at) = registry.iter().position(|s| s.name == spec.name) {
        registry.remove(at);
    }
    registry.push(Box::leak(Box::new(spec)));
    Ok(())
}

/// Run `f` over the current registry snapshot. Unit tests see a fixed
/// registry built from dev-dependency grammar crates plus the curated
/// manifests and query stacks — the exact data users install.
fn with_registry<T>(f: impl FnOnce(&[&'static LangSpec]) -> T) -> T {
    let registry = REGISTRY.read().unwrap();
    #[cfg(test)]
    if registry.is_empty() {
        return f(test_specs());
    }
    f(&registry)
}

/// Registry lookup by file extension.
pub(crate) fn spec_for(path: &Path) -> Option<&'static LangSpec> {
    let ext = path.extension()?.to_str()?;
    with_registry(|specs| {
        specs
            .iter()
            .find(|spec| spec.extensions.contains(&ext))
            .copied()
    })
}

/// Language identifier for a path, for per-language theming.
pub fn lang_name(path: &Path) -> Option<&'static str> {
    spec_for(path).map(|spec| spec.name)
}

/// All language identifiers configuration may reference: installed
/// plugins plus every curated language — `[theme.rust]` must stay valid
/// while rust happens not to be installed.
pub(crate) fn lang_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> =
        with_registry(|specs| specs.iter().map(|spec| spec.name).collect());
    for name in curated::names() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The curated language that could review `path`, when no installed one
/// covers it — what the app's install prompt offers.
pub fn installable_for(path: &Path) -> Option<&'static str> {
    if spec_for(path).is_some() {
        return None;
    }
    let ext = path.extension()?.to_str()?;
    curated::manifests()
        .iter()
        .find(|(_, manifest)| manifest.extensions.iter().any(|e| e == ext))
        .map(|(name, _)| *name)
}

/// Plugin manifests and query files: `<config dir>/languages/<name>/`.
pub fn languages_dir() -> PathBuf {
    crate::config::config_path()
        .parent()
        .map(|dir| dir.join("languages"))
        .unwrap_or_default()
}

/// Compiled grammars: `~/.cache/drift/grammars` (or `$XDG_CACHE_HOME`).
/// Cache, not config — `drift lang build` recreates everything in it.
pub fn grammar_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // HOME is absent on Windows; home_dir falls back to USERPROFILE.
            std::env::home_dir().unwrap_or_default().join(".cache")
        });
    base.join("drift").join("grammars")
}

/// The shared library a language's grammar compiles to.
fn grammar_path(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir.join(format!("{name}.{}", std::env::consts::DLL_EXTENSION))
}

/// The unit-test registry: curated manifests and query stacks paired
/// with statically linked grammar crates (dev-dependencies only), so
/// the whole suite runs hermetically — no git, no C compiler — while
/// exercising the very data `drift lang install` ships.
#[cfg(test)]
fn test_specs() -> &'static [&'static LangSpec] {
    use std::sync::OnceLock;
    static SPECS: OnceLock<Vec<&'static LangSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let crates: &[(&str, LanguageFn)] = &[
            ("rust", tree_sitter_rust::LANGUAGE),
            ("python", tree_sitter_python::LANGUAGE),
            ("javascript", tree_sitter_javascript::LANGUAGE),
            ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            ("tsx", tree_sitter_typescript::LANGUAGE_TSX),
            ("go", tree_sitter_go::LANGUAGE),
        ];
        crates
            .iter()
            .map(|&(name, grammar)| {
                let entry = curated::find(name)
                    .unwrap_or_else(|| panic!("{name} missing from the curated registry"));
                let manifest = manifest::parse(entry.manifest).unwrap();
                assert_eq!(manifest.name, name);
                let highlights = entry
                    .highlights
                    .unwrap_or_else(|| panic!("{name} ships no curated highlights.scm"));
                &*Box::leak(Box::new(LangSpec {
                    name: entry.name,
                    extensions: loader::leak_all(manifest.extensions),
                    grammar,
                    block_kinds: loader::leak_all(manifest.block_kinds),
                    highlight_queries: Box::leak(Box::new([highlights])),
                }))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_by_extension() {
        assert_eq!(lang_name(Path::new("a/b.rs")), Some("rust"));
        assert_eq!(lang_name(Path::new("x.tsx")), Some("tsx"));
        assert_eq!(lang_name(Path::new("notes.txt")), None);
        assert_eq!(lang_name(Path::new("no_extension")), None);
    }

    #[test]
    fn names_cover_curated_without_duplicates() {
        let names = lang_names();
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"c"), "curated-but-uninstalled included");
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(deduped.len(), names.len());
    }

    #[test]
    fn curated_extensions_map_to_one_language() {
        let mut extensions: Vec<&str> = curated::manifests()
            .iter()
            .flat_map(|(_, m)| m.extensions.iter().map(String::as_str))
            .collect();
        let total = extensions.len();
        extensions.sort_unstable();
        extensions.dedup();
        assert_eq!(extensions.len(), total, "duplicate extension in registry");
    }

    #[test]
    fn every_test_grammar_loads() {
        for spec in test_specs() {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&spec.language())
                .unwrap_or_else(|e| panic!("grammar {} rejected: {e}", spec.name));
        }
    }

    #[test]
    fn installable_points_at_curated_not_installed() {
        // rust resolves via the test registry, so nothing to install.
        assert_eq!(installable_for(Path::new("x.rs")), None);
        // c is curated but not in the registry — the prompt's case.
        assert_eq!(installable_for(Path::new("x.c")), Some("c"));
        assert_eq!(installable_for(Path::new("x.rake")), Some("ruby"));
        assert_eq!(installable_for(Path::new("notes.txt")), None);
    }
}
