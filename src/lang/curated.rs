//! The curated language registry: complete, known-good manifests (and,
//! where drift maintains one, the full highlight-query stack) shipped
//! in the repo's `languages/` directory and embedded into the binary —
//! `drift lang install <name>` needs no lookup service and the registry
//! version is simply the drift version. Contributing a language
//! upstream = one manifest file.

use std::sync::OnceLock;

use super::manifest::{self, Manifest};

pub(super) struct Curated {
    pub(super) name: &'static str,
    pub(super) manifest: &'static str,
    /// drift's own query stack for this language (grammar-bundled
    /// queries plus hand-tuned supplements). `None` = installs copy
    /// the grammar repo's bundled query instead.
    pub(super) highlights: Option<&'static str>,
}

macro_rules! curated {
    ($name:literal) => {
        Curated {
            name: $name,
            manifest: include_str!(concat!("../../languages/", $name, "/language.toml")),
            highlights: None,
        }
    };
    ($name:literal, highlights) => {
        Curated {
            name: $name,
            manifest: include_str!(concat!("../../languages/", $name, "/language.toml")),
            highlights: Some(include_str!(concat!(
                "../../languages/",
                $name,
                "/highlights.scm"
            ))),
        }
    };
}

/// Keep sorted; a test checks this table against the `languages/`
/// directory listing.
pub(super) const CURATED: &[Curated] = &[
    curated!("c"),
    curated!("css"),
    curated!("go", highlights),
    curated!("html"),
    curated!("java"),
    curated!("javascript", highlights),
    curated!("json"),
    curated!("python", highlights),
    curated!("ruby"),
    curated!("rust", highlights),
    curated!("toml"),
    curated!("tsx", highlights),
    curated!("typescript", highlights),
];

pub(super) fn find(name: &str) -> Option<&'static Curated> {
    CURATED.iter().find(|entry| entry.name == name)
}

pub(super) fn names() -> impl Iterator<Item = &'static str> {
    CURATED.iter().map(|entry| entry.name)
}

/// Every curated manifest, parsed once — extension lookups for the
/// install prompt happen per viewed file.
pub(super) fn manifests() -> &'static [(&'static str, Manifest)] {
    static PARSED: OnceLock<Vec<(&'static str, Manifest)>> = OnceLock::new();
    PARSED.get_or_init(|| {
        CURATED
            .iter()
            // A curated manifest failing to parse is a drift bug caught
            // by tests; at runtime the entry silently drops.
            .filter_map(|entry| Some((entry.name, manifest::parse(entry.manifest).ok()?)))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A malformed contribution must fail CI, not the user's install.
    #[test]
    fn every_curated_manifest_is_valid_and_complete() {
        for entry in CURATED {
            let m = manifest::parse(entry.manifest)
                .unwrap_or_else(|e| panic!("curated {}: {e:#}", entry.name));
            assert_eq!(m.name, entry.name, "table key must match manifest name");
            assert!(m.repo.is_some(), "curated {} must pin a repo", entry.name);
            assert!(m.rev.is_some(), "curated {} must pin a rev", entry.name);
            assert!(
                !m.block_kinds.is_empty(),
                "curated {} must list block_kinds — that's the point of curation",
                entry.name
            );
        }
        assert_eq!(manifests().len(), CURATED.len(), "every manifest parses");
    }

    #[test]
    fn curated_table_matches_languages_directory() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("languages");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("languages/ directory")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("language.toml").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let in_table: Vec<&str> = names().collect();
        assert_eq!(on_disk, in_table, "languages/ and curated.rs disagree");
        for entry in CURATED {
            let on_disk_query = dir.join(entry.name).join("highlights.scm").is_file();
            assert_eq!(
                on_disk_query,
                entry.highlights.is_some(),
                "{}: highlights.scm on disk and in the table disagree",
                entry.name
            );
        }
    }

    /// The curated query stacks must compile against the grammar
    /// versions their manifests pin (linked here as dev-dependencies) —
    /// otherwise `drift lang install rust` ships a broken query.
    #[test]
    fn curated_query_stacks_compile_against_their_grammars() {
        let grammars: &[(&str, tree_sitter_language::LanguageFn)] = &[
            ("rust", tree_sitter_rust::LANGUAGE),
            ("python", tree_sitter_python::LANGUAGE),
            ("javascript", tree_sitter_javascript::LANGUAGE),
            ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            ("tsx", tree_sitter_typescript::LANGUAGE_TSX),
            ("go", tree_sitter_go::LANGUAGE),
        ];
        for &(name, grammar) in grammars {
            let query = find(name)
                .and_then(|entry| entry.highlights)
                .unwrap_or_else(|| panic!("{name} has no curated highlights.scm"));
            tree_sitter::Query::new(&tree_sitter::Language::new(grammar), query)
                .unwrap_or_else(|e| panic!("{name} curated query does not compile: {e}"));
        }
    }

    /// The supplements that used to be compiled-in constants must stay
    /// part of the shipped query stacks.
    #[test]
    fn curated_stacks_carry_the_query_supplements() {
        let rust = find("rust").unwrap().highlights.unwrap();
        assert!(rust.contains("punctuation.bracket.call"));
        assert!(rust.contains("(lifetime"));
        let ts = find("typescript").unwrap().highlights.unwrap();
        assert!(ts.contains("(decorator"));
        assert!(ts.contains("abstract_method_signature"));
        let tsx = find("tsx").unwrap().highlights.unwrap();
        assert!(tsx.contains("jsx"));
        assert!(tsx.contains("(decorator"));
        let js = find("javascript").unwrap().highlights.unwrap();
        assert!(js.contains("(decorator"));
    }
}
