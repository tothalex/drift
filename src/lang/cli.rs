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
    // Progress updates ("Receiving objects: 42%") overwrite themselves
    // on one terminal line; ordinary messages first finish that line.
    // Piped output gets no progress — it would be one line per percent.
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let transient = std::cell::Cell::new(false);
    let log: &dyn Fn(&str) = &|line| {
        use std::io::Write;
        if line.ends_with('%') {
            if !tty {
                return;
            }
            print!("\r\x1b[2K{line}");
            let _ = std::io::stdout().flush();
            transient.set(true);
        } else {
            if transient.replace(false) {
                println!();
            }
            println!("{line}");
        }
    };
    let result = match command {
        LangCommand::Install { language, rev } => install(&dirs, language, rev.as_deref(), log),
        LangCommand::Build { language } => build(&dirs, language.as_deref(), log),
        LangCommand::List => list(&dirs),
        LangCommand::Remove { language } => remove(&dirs, language),
    };
    if transient.get() {
        println!();
    }
    result
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

    // Resolve the manifest without writing anything: nothing lands in
    // the plugin directory until the grammar is fetched, compiled, and
    // validated, so an interrupted install leaves no half-plugin that
    // would warn on every startup. An existing manifest is the user's —
    // reused, never clobbered.
    let (manifest, manifest_to_write) = if plugin_dir.join("language.toml").is_file() {
        log(&format!(
            "using existing manifest {}",
            display(&plugin_dir.join("language.toml"))
        ));
        (read_manifest(&plugin_dir)?, None)
    } else if let Some(entry) = curated_entry {
        (
            manifest::parse(entry.manifest)?,
            Some(entry.manifest.to_string()),
        )
    } else if is_url {
        let text = scaffold(&name, language);
        (manifest::parse(&text)?, Some(text))
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
    let query_to_write = if plugin_dir.join("highlights.scm").exists() {
        None
    } else if let Some(query) = curated_entry.and_then(|entry| entry.highlights) {
        Some(query.to_string())
    } else {
        // Monorepos keep queries beside the grammar or at the root.
        [
            dirs.grammar_src(&manifest).with_file_name("queries"),
            dirs.checkout(&name).join("queries"),
        ]
        .iter()
        .map(|dir| dir.join("highlights.scm"))
        .find(|f| f.is_file())
        .and_then(|f| std::fs::read_to_string(f).ok())
    };
    let query = match &query_to_write {
        Some(text) => Some(text.clone()),
        None => std::fs::read_to_string(plugin_dir.join("highlights.scm")).ok(),
    };

    build_one(dirs, &manifest, query.as_deref(), log)?;

    // Everything proved out — now the plugin may exist.
    if let Some(text) = manifest_to_write {
        write_plugin_file(&plugin_dir, "language.toml", &text, log)?;
    }
    if let Some(text) = query_to_write {
        write_plugin_file(&plugin_dir, "highlights.scm", &text, log)?;
    }
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
        let query = std::fs::read_to_string(dirs.plugin(&name).join("highlights.scm")).ok();
        build_one(dirs, &manifest, query.as_deref(), log)?;
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
/// and compile `query` against the freshly built grammar. The dylib is
/// built to a temp path and only renamed into place once it passed —
/// an interrupted build never leaves a truncated grammar behind.
fn build_one(
    dirs: &Dirs,
    manifest: &Manifest,
    query: Option<&str>,
    log: &dyn Fn(&str),
) -> Result<()> {
    log(&format!("compiling {}…", manifest.name));
    let dylib = grammar_path(&dirs.cache, &manifest.name);
    let staging = dylib.with_extension("tmp");
    compile::grammar(&dirs.grammar_src(manifest), &staging)?;
    let lang_fn = loader::load_language(&staging, &manifest.symbol)?;
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
    if let Some(query) = query {
        tree_sitter::Query::new(&language, query)
            .with_context(|| format!("{}'s highlights.scm does not compile", manifest.name))?;
    }
    std::fs::rename(&staging, &dylib)?;
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

/// Checkout of `repo` at `rev` (default branch when `None`), reused
/// when it already sits at the pinned rev. Cloning shells out to the
/// `git` CLI: an install-time-only network operation, like the forge's
/// gh/glab — the in-process gitoxide build deliberately carries no
/// network stack. A failed or interrupted fetch removes the partial
/// checkout, so no corrupt state survives into the next run.
fn fetch(repo: &str, rev: Option<&str>, dest: &Path, log: &dyn Fn(&str)) -> Result<()> {
    if let Some(rev) = rev
        && dest.join(".git").exists()
        && run_command_output(git_at(dest, &["rev-parse", "HEAD"]))
            .is_ok_and(|head| head.trim() == rev)
    {
        log("using cached sources");
        return Ok(());
    }
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
    let result = fetch_fresh(repo, rev, dest, log);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

fn fetch_fresh(repo: &str, rev: Option<&str>, dest: &Path, log: &dyn Fn(&str)) -> Result<()> {
    let Some(rev) = rev else {
        let mut clone = Command::new("git");
        clone
            .args(["clone", "--depth", "1", "--progress"])
            .arg(repo)
            .arg(dest);
        return run_streaming(clone, log);
    };
    // A pinned rev fetches shallowly by SHA — grammar histories can be
    // huge and GitHub serves single commits. Servers that refuse SHA
    // fetches get the full clone instead.
    std::fs::create_dir_all(dest)?;
    run_command_output(git_at(dest, &["init", "--quiet"]))?;
    let mut shallow = git_at(dest, &["fetch", "--depth", "1", "--progress"]);
    shallow.arg(repo).arg(rev);
    match run_streaming(shallow, log) {
        Ok(()) => run_command_output(git_at(
            dest,
            &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
        ))
        .map(drop),
        Err(_) => {
            std::fs::remove_dir_all(dest)?;
            log("shallow fetch failed — trying a full clone");
            let mut clone = Command::new("git");
            clone.args(["clone", "--progress"]).arg(repo).arg(dest);
            run_streaming(clone, log)?;
            run_command_output(git_at(dest, &["checkout", "--quiet", "--detach", rev])).map(drop)
        }
    }
}

fn git_at(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd
}

fn run_command_output(mut cmd: Command) -> Result<String> {
    let output = cmd.output().context("cannot run git — is git installed?")?;
    if !output.status.success() {
        bail!(
            "git failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run git forwarding its `--progress` stderr into `log` as it happens
/// ("Receiving objects: 42%"), one message per percent step; on failure
/// the last ordinary stderr lines become the error.
fn run_streaming(mut cmd: Command, log: &dyn Fn(&str)) -> Result<()> {
    use std::io::Read;
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!("git is not installed"),
            _ => anyhow::anyhow!("cannot run git: {err}"),
        })?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let mut pending: Vec<u8> = Vec::new();
    let mut recent: Vec<String> = Vec::new();
    let mut last_progress = String::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stderr.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..n]);
        // Progress updates end in `\r` (rewriting one line), phases in
        // `\n`; both delimit a message.
        while let Some(at) = pending.iter().position(|&b| b == b'\r' || b == b'\n') {
            let line: Vec<u8> = pending.drain(..=at).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match progress_head(line) {
                Some(head) if head != last_progress => {
                    log(&head);
                    last_progress = head;
                }
                Some(_) => {}
                None => {
                    recent.push(line.to_string());
                    if recent.len() > 4 {
                        recent.remove(0);
                    }
                }
            }
        }
    }
    let status = child.wait().context("cannot run git")?;
    if !status.success() {
        bail!("git failed ({status}):\n{}", recent.join("\n"));
    }
    Ok(())
}

/// "Receiving objects:  42% (12/456), 2 MiB | 1 MiB/s" →
/// "Receiving objects: 42%"; `None` for non-progress lines.
fn progress_head(line: &str) -> Option<String> {
    let head = line.split('(').next()?.trim();
    if !head.ends_with('%') {
        return None;
    }
    Some(head.split_whitespace().collect::<Vec<_>>().join(" "))
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
    fn progress_heads_parse_and_ordinary_lines_do_not() {
        assert_eq!(
            progress_head("Receiving objects:  42% (12/456), 2.00 MiB | 1.00 MiB/s").as_deref(),
            Some("Receiving objects: 42%")
        );
        assert_eq!(
            progress_head("remote: Compressing objects: 100% (5/5), done.").as_deref(),
            Some("remote: Compressing objects: 100%")
        );
        assert_eq!(progress_head("Cloning into 'dest'..."), None);
        assert_eq!(progress_head("fatal: repository not found"), None);
    }

    #[test]
    fn scaffold_round_trips_through_the_parser() {
        let m = manifest::parse(&scaffold("zig", "https://example.com/zig")).unwrap();
        assert_eq!(m.name, "zig");
        assert_eq!(m.repo.as_deref(), Some("https://example.com/zig"));
        assert!(m.block_kinds.is_empty());
    }
}
