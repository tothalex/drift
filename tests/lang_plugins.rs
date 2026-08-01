//! End-to-end language plugin loading, in its own process because the
//! plugin registry initializes once per process: compile the vendored
//! JSON grammar fixture with the production compiler path, install it
//! into a temporary config layout, and review a JSON file through it.

use std::path::Path;

use drift::processor::blocks::BlockResolver;
use drift::processor::treesitter::TsResolver;

#[test]
fn json_plugin_loads_and_reviews_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let languages = tmp.path().join("languages");
    let cache = tmp.path().join("grammars");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars/json");

    // A broken plugin alongside must produce a warning, not an error,
    // and must not take the healthy one down with it.
    std::fs::create_dir_all(languages.join("broken")).unwrap();
    std::fs::write(languages.join("broken/language.toml"), "name = \"???\"\n").unwrap();

    let plugin = languages.join("json");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("language.toml"),
        "name = \"json\"\nextensions = [\"json\"]\n\n[grammar]\nblock_kinds = [\"object\", \"array\", \"pair\"]\n",
    )
    .unwrap();
    std::fs::copy(
        fixture.join("queries/highlights.scm"),
        plugin.join("highlights.scm"),
    )
    .unwrap();

    // The same compile path `drift lang build` uses.
    let dylib = cache.join(format!("json.{}", std::env::consts::DLL_EXTENSION));
    drift::lang::compile::grammar(&fixture.join("src"), &dylib).expect("grammar compiles");

    let warnings = drift::lang::init_plugins_at(&languages, &cache);
    assert_eq!(
        warnings.len(),
        1,
        "only the broken plugin warns: {warnings:?}"
    );
    assert!(warnings[0].contains("broken"), "{warnings:?}");

    // The plugin registers its extension…
    assert_eq!(
        drift::lang::lang_name(Path::new("package.json")),
        Some("json")
    );
    // …an installed language is no longer offered for install…
    assert_eq!(drift::lang::installable_for(Path::new("a.json")), None);
    // …and with no grammars compiled in, everything else is unknown —
    // curated languages are offered for install instead.
    assert_eq!(drift::lang::lang_name(Path::new("notes.txt")), None);
    assert_eq!(drift::lang::lang_name(Path::new("x.rs")), None);
    assert_eq!(
        drift::lang::installable_for(Path::new("x.rs")),
        Some("rust")
    );

    let source = "{\n  \"name\": \"drift\",\n  \"deps\": {\n    \"a\": 1\n  }\n}\n";

    // Block resolution through the dlopened grammar: a change on line 4
    // walks pair → nested object → document object, innermost first.
    let resolver = TsResolver::new(Path::new("x.json"), source).expect("resolver");
    let chain = resolver.enclosing_blocks((4, 4));
    let ranges: Vec<_> = chain.iter().map(|b| b.range).collect();
    assert_eq!(ranges, vec![(4, 4), (3, 5), (1, 6)]);

    // Highlighting through the plugin's highlights.scm: the string keys
    // and values on line 2 carry spans.
    let hl = drift::processor::highlight::highlight(Path::new("x.json"), source)
        .expect("plugin highlights");
    assert!(!hl.spans_for(2).is_empty());
}
