//! Loads plugin languages: a manifest plus its compiled grammar dylib
//! and optional `highlights.scm` become registry entries.

use std::path::Path;

use anyhow::{Context, Result, bail};
use tree_sitter_language::LanguageFn;

use super::LangSpec;
use super::manifest::{self, Manifest};

/// Load every plugin under `languages_dir`, in directory-name order so
/// precedence is deterministic. A plugin that fails to load becomes a
/// warning, never an error: drift must start with whatever is healthy.
pub(super) fn load_all(
    languages_dir: &Path,
    cache_dir: &Path,
    warnings: &mut Vec<String>,
) -> Vec<LangSpec> {
    let Ok(entries) = std::fs::read_dir(languages_dir) else {
        return Vec::new(); // no plugins installed
    };
    let mut dirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("language.toml").is_file())
        .collect();
    dirs.sort();

    let mut specs: Vec<LangSpec> = Vec::new();
    for dir in dirs {
        match load_one(&dir, cache_dir) {
            Ok(spec) => {
                if specs.iter().any(|s| s.name == spec.name) {
                    warnings.push(format!(
                        "language plugin {}: duplicate language '{}', keeping the first",
                        dir.display(),
                        spec.name
                    ));
                } else {
                    specs.push(spec);
                }
            }
            Err(err) => warnings.push(format!("language plugin {}: {:#}", dir.display(), err)),
        }
    }
    specs
}

pub(super) fn load_one(dir: &Path, cache_dir: &Path) -> Result<LangSpec> {
    let text = std::fs::read_to_string(dir.join("language.toml"))?;
    let manifest = manifest::parse(&text)?;
    let dylib = super::grammar_path(cache_dir, &manifest.name);
    if !dylib.exists() {
        bail!(
            "grammar not built — run `drift lang build {}`",
            manifest.name
        );
    }
    let lang_fn = load_language(&dylib, &manifest.symbol)?;
    check_abi(lang_fn, &manifest.name)?;

    let highlights = read_query(dir, "highlights.scm")?; // absent = blocks-only support
    let injections = read_query(dir, "injections.scm")?;
    Ok(spec_from(manifest, lang_fn, highlights, injections))
}

/// An optional query file: absent is fine, unreadable is an error.
fn read_query(dir: &Path, file: &str) -> Result<Option<String>> {
    match std::fs::read_to_string(dir.join(file)) {
        Ok(query) => Ok(Some(query)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("cannot read {file}")),
    }
}

/// dlopen `dylib` and resolve `symbol` as a tree-sitter language function.
///
/// The library is leaked deliberately: the returned function pointer (and
/// every parse table behind it) lives in the mapped library, which must
/// therefore stay mapped for the life of the process.
pub fn load_language(dylib: &Path, symbol: &str) -> Result<LanguageFn> {
    // SAFETY: loading a plugin runs arbitrary native initialization; that
    // is the documented trust model of language plugins (like an editor
    // loading grammar parsers).
    let library = unsafe { libloading::Library::new(dylib) }
        .with_context(|| format!("cannot load {}", dylib.display()))?;
    let library = Box::leak(Box::new(library));
    // SAFETY: the symbol is only ever used as a tree-sitter language
    // function; a grammar exporting anything else under this name is a
    // broken plugin, and set_language's ABI check rejects garbage.
    let function = unsafe {
        library
            .get::<unsafe extern "C" fn() -> *const ()>(symbol.as_bytes())
            .with_context(|| format!("{} exports no `{symbol}`", dylib.display()))?
    };
    Ok(unsafe { LanguageFn::from_raw(*function) })
}

/// Reject grammars built against an incompatible tree-sitter ABI with a
/// message naming the fix, instead of a cryptic parse failure later.
pub(super) fn check_abi(lang_fn: LanguageFn, name: &str) -> Result<()> {
    let version = tree_sitter::Language::from(lang_fn).abi_version();
    let supported = tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION;
    if !supported.contains(&version) {
        bail!(
            "grammar ABI version {version} is outside drift's supported range \
             {}..={} — run `drift lang build {name}` to rebuild it",
            supported.start(),
            supported.end()
        );
    }
    Ok(())
}

fn spec_from(
    manifest: Manifest,
    lang_fn: LanguageFn,
    highlights: Option<String>,
    injections: Option<String>,
) -> LangSpec {
    // Plugin specs live for the whole process once registered, so
    // leaking their strings lets lookups hand out `&'static` references.
    let leak_query = |query: Option<String>| match query {
        Some(query) => leak_all(vec![query]),
        None => &[][..],
    };
    LangSpec {
        name: leak(manifest.name),
        extensions: leak_all(manifest.extensions),
        grammar: lang_fn,
        block_kinds: leak_all(manifest.block_kinds),
        highlight_queries: leak_query(highlights),
        injection_queries: leak_query(injections),
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

pub(super) fn leak_all(strings: Vec<String>) -> &'static [&'static str] {
    Box::leak(
        strings
            .into_iter()
            .map(leak)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}
