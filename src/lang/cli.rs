//! `drift lang …`: install, build, list and remove language plugins.
//!
//! Network and compilation happen only here, at install/build time —
//! and so does query validation, so a bad `highlights.scm` surfaces as
//! a command error instead of silently unhighlighted files at startup.
//! Progress goes through a `log` callback: the CLI prints it, while the
//! in-app installer routes it to the status line (the TUI owns stdout).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cli::LangCommand;

use super::manifest::{self, Manifest};
use super::{compile, curated, grammar_cache_dir, grammar_path, languages_dir, loader};

pub fn run(command: &LangCommand) -> Result<()> {
    let dirs = Dirs::user();
    let log: &dyn Fn(&str) = &|line| println!("{line}");
    match command {
        LangCommand::Install { language, rev } => install(&dirs, language, rev.as_deref(), log),
        LangCommand::Build { language } => build(&dirs, language.as_deref(), log),
        LangCommand::List => list(&dirs),
        LangCommand::Remove { language } => remove(&dirs, language),
    }
}

/// Install a curated language from inside the app: same path as the
/// CLI, silent stdout, progress via `log`.
pub fn install_curated(name: &str, log: &dyn Fn(&str)) -> Result<()> {
    install(&Dirs::user(), name, None, log)
}

struct Dirs {
    /// Manifests and query files, per language (config).
    languages: PathBuf,
    /// Compiled grammars, with checkouts under `src/` (cache).
    cache: PathBuf,
}

impl Dirs {
    fn user() -> Self {
        Dirs {
            languages: languages_dir(),
            cache: grammar_cache_dir(),
        }
    }

    fn plugin(&self, name: &str) -> PathBuf {
        self.languages.join(name)
    }

    fn checkout(&self, name: &str) -> PathBuf {
        self.cache.join("src").join(name)
    }

    /// The grammar's C sources: `<checkout>[/<grammar.path>]/src`.
    fn grammar_src(&self, manifest: &Manifest) -> PathBuf {
        let mut dir = self.checkout(&manifest.name);
        if let Some(path) = manifest.path.as_deref() {
            dir = dir.join(path);
        }
        dir.join("src")
    }
}

fn install(
    dirs: &Dirs,
    language: &str,
    rev_override: Option<&str>,
    log: &dyn Fn(&str),
) -> Result<()> {
    let is_url = language.contains("://") || language.starts_with("git@");
    let name = if is_url {
        name_from_url(language)?
    } else {
        language.to_string()
    };
    let plugin_dir = dirs.plugin(&name);
    let curated_entry = (!is_url).then(|| curated::find(&name)).flatten();

    // An existing manifest is the user's — reuse it rather than clobber
    // their edits; `drift lang remove` first gets a clean slate.
    let manifest = if plugin_dir.join("language.toml").is_file() {
        log(&format!(
            "using existing manifest {}",
            display(&plugin_dir.join("language.toml"))
        ));
        read_manifest(&plugin_dir)?
    } else if let Some(entry) = curated_entry {
        write_plugin_file(&plugin_dir, "language.toml", entry.manifest, log)?;
        manifest::parse(entry.manifest)?
    } else if is_url {
        let text = scaffold(&name, language);
        write_plugin_file(&plugin_dir, "language.toml", &text, log)?;
        manifest::parse(&text)?
    } else {
        bail!(
            "unknown language '{name}' — curated: {}; or pass a grammar repo git URL",
            curated::names().collect::<Vec<_>>().join(", ")
        );
    };

    let repo = manifest
        .repo
        .as_deref()
        .with_context(|| format!("{name}: manifest has no grammar.repo to fetch"))?;
    let rev = rev_override.or(manifest.rev.as_deref());
    fetch(repo, rev, &dirs.checkout(&name), log)?;

    // The plugin's own query file always wins. Absent one, a curated
    // stack — the grammar's bundled queries plus drift's hand-tuned
    // supplements — beats the barer query bundled in the grammar repo.
    if !plugin_dir.join("highlights.scm").exists() {
        if let Some(query) = curated_entry.and_then(|entry| entry.highlights) {
            std::fs::write(plugin_dir.join("highlights.scm"), query)?;
        } else {
            // Monorepos keep queries beside the grammar or at the root.
            let bundled = [
                dirs.grammar_src(&manifest).with_file_name("queries"),
                dirs.checkout(&name).join("queries"),
            ];
            if let Some(query) = bundled
                .iter()
                .map(|dir| dir.join("highlights.scm"))
                .find(|f| f.is_file())
            {
                std::fs::copy(&query, plugin_dir.join("highlights.scm"))?;
            }
        }
    }

    build_one(dirs, &manifest, log)?;
    log(&format!(
        "installed {name} ({})",
        manifest.extensions.join(", ")
    ));
    if manifest.block_kinds.is_empty() {
        log(&format!(
            "note: grammar.block_kinds is empty, so changes won't expand to their\n\
             enclosing blocks — list this grammar's declaration/statement node\n\
             kinds in {}",
            display(&plugin_dir.join("language.toml"))
        ));
    }
    Ok(())
}

fn build(dirs: &Dirs, language: Option<&str>, log: &dyn Fn(&str)) -> Result<()> {
    let names = match language {
        Some(name) => vec![name.to_string()],
        None => installed(dirs)?,
    };
    if names.is_empty() {
        log("no language plugins installed");
        return Ok(());
    }
    for name in names {
        let manifest = read_manifest(&dirs.plugin(&name))?;
        if !dirs.grammar_src(&manifest).join("parser.c").exists() {
            let repo = manifest.repo.as_deref().with_context(|| {
                format!("{name}: no sources cached and manifest has no grammar.repo")
            })?;
            fetch(repo, manifest.rev.as_deref(), &dirs.checkout(&name), log)?;
        }
        build_one(dirs, &manifest, log)?;
        log(&format!("built {name}"));
    }
    Ok(())
}

fn list(dirs: &Dirs) -> Result<()> {
    let plugins = installed(dirs)?;
    if plugins.is_empty() {
        println!("no languages installed");
    }
    for name in &plugins {
        match read_manifest(&dirs.plugin(name)) {
            Ok(manifest) => {
                let state = if grammar_path(&dirs.cache, name).exists() {
                    "installed"
                } else {
                    "not built — run `drift lang build`"
                };
                println!(
                    "{:<12} {state}    ({})",
                    manifest.name,
                    manifest.extensions.join(", ")
                );
            }
            Err(err) => println!("{name:<12} broken       {err:#}"),
        }
    }
    let available: Vec<_> = curated::names()
        .filter(|name| !plugins.iter().any(|p| p == name))
        .collect();
    if !available.is_empty() {
        println!("\navailable to install: {}", available.join(", "));
    }
    Ok(())
}

fn remove(dirs: &Dirs, name: &str) -> Result<()> {
    if !dirs.plugin(name).exists() {
        bail!("no language plugin '{name}' installed");
    }
    std::fs::remove_dir_all(dirs.plugin(name))?;
    for path in [dirs.checkout(name), grammar_path(&dirs.cache, name)] {
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else if path.is_file() {
            std::fs::remove_file(&path)?;
        }
    }
    println!("removed {name}");
    Ok(())
}

/// Compile the checkout and prove the result: dlopen it, check the ABI,
/// and compile the highlight query against the freshly built grammar.
fn build_one(dirs: &Dirs, manifest: &Manifest, log: &dyn Fn(&str)) -> Result<()> {
    let dylib = grammar_path(&dirs.cache, &manifest.name);
    compile::grammar(&dirs.grammar_src(manifest), &dylib)?;
    let lang_fn = loader::load_language(&dylib, &manifest.symbol)?;
    loader::check_abi(lang_fn, &manifest.name)?;
    let language = tree_sitter::Language::from(lang_fn);
    // A typo'd block kind silently never matches; the built grammar
    // knows its node kinds, so name the mistake now.
    for kind in &manifest.block_kinds {
        if language.id_for_node_kind(kind, true) == 0 {
            log(&format!(
                "warning: '{kind}' is not a node kind of the {} grammar",
                manifest.name
            ));
        }
    }
    let query_file = dirs.plugin(&manifest.name).join("highlights.scm");
    if let Ok(query) = std::fs::read_to_string(&query_file) {
        tree_sitter::Query::new(&language, &query)
            .with_context(|| format!("{} does not compile", display(&query_file)))?;
    }
    Ok(())
}

/// Plugin directory names that carry a manifest, sorted.
fn installed(dirs: &Dirs) -> Result<Vec<String>> {
    let Ok(entries) = std::fs::read_dir(&dirs.languages) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("language.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

fn read_manifest(plugin_dir: &Path) -> Result<Manifest> {
    let path = plugin_dir.join("language.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", display(&path)))?;
    manifest::parse(&text).with_context(|| format!("invalid {}", display(&path)))
}

/// Fresh checkout of `repo` at `rev` (default branch when `None`).
/// Cloning shells out to the `git` CLI: an install-time-only network
/// operation, like the forge's gh/glab — the in-process gitoxide build
/// deliberately carries no network stack.
fn fetch(repo: &str, rev: Option<&str>, dest: &Path, log: &dyn Fn(&str)) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    log(&format!(
        "fetching {repo}{}",
        rev.map(|r| format!(" @ {r}")).unwrap_or_default()
    ));
    let mut clone = Command::new("git");
    clone.arg("clone").arg("--quiet");
    if rev.is_none() {
        clone.args(["--depth", "1"]);
    }
    clone.arg(repo).arg(dest);
    run_command(clone, "git clone")?;
    if let Some(rev) = rev {
        let mut checkout = Command::new("git");
        checkout
            .arg("-C")
            .arg(dest)
            .args(["checkout", "--quiet", "--detach", rev]);
        run_command(checkout, "git checkout")?;
    }
    Ok(())
}

fn run_command(mut cmd: Command, what: &str) -> Result<()> {
    let output = cmd
        .output()
        .with_context(|| format!("cannot run {what} — is git installed?"))?;
    if !output.status.success() {
        bail!(
            "{what} failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// `https://github.com/x/tree-sitter-zig.git` → `zig`.
fn name_from_url(url: &str) -> Result<String> {
    let last = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git");
    let name = last
        .strip_prefix("tree-sitter-")
        .unwrap_or(last)
        .replace('_', "-")
        .to_lowercase();
    // Same rule the manifest enforces — a host name or empty segment
    // (e.g. a bare domain URL) fails here, with the URL in the message.
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("cannot derive a language name from '{url}'");
    }
    Ok(name)
}

/// Manifest for a URL install: correct where derivable, and explicit
/// about the parts only a human can fill in.
fn scaffold(name: &str, repo: &str) -> String {
    format!(
        "name = \"{name}\"\n\
         # Adjust if this language's file extensions differ.\n\
         extensions = [\"{name}\"]\n\
         \n\
         [grammar]\n\
         repo = \"{repo}\"\n\
         # Node kinds that count as reviewable blocks (functions, classes,\n\
         # loops…); without them changes can't expand to enclosing blocks.\n\
         block_kinds = []\n"
    )
}

fn write_plugin_file(plugin_dir: &Path, file: &str, text: &str, log: &dyn Fn(&str)) -> Result<()> {
    std::fs::create_dir_all(plugin_dir)?;
    std::fs::write(plugin_dir.join(file), text)?;
    log(&format!("wrote {}", display(&plugin_dir.join(file))));
    Ok(())
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_names_strip_prefix_suffix_and_normalize() {
        assert_eq!(
            name_from_url("https://github.com/ts/tree-sitter-zig.git").unwrap(),
            "zig"
        );
        assert_eq!(
            name_from_url("https://github.com/ts/tree-sitter-php/").unwrap(),
            "php"
        );
        assert_eq!(
            name_from_url("git@github.com:x/my_lang.git").unwrap(),
            "my-lang"
        );
        assert!(name_from_url("https://github.com/").is_err());
    }

    #[test]
    fn scaffold_round_trips_through_the_parser() {
        let m = manifest::parse(&scaffold("zig", "https://example.com/zig")).unwrap();
        assert_eq!(m.name, "zig");
        assert_eq!(m.repo.as_deref(), Some("https://example.com/zig"));
        assert!(m.block_kinds.is_empty());
    }
}
