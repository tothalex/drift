//! Compiles a grammar's C sources into the shared library the loader
//! dlopens. The compiler comes from the `cc` crate's discovery (cc,
//! clang, MSVC), targeting the triple drift itself was built for.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Build `src_dir` (a grammar repo's `src/`: `parser.c`, optional
/// `scanner.c`) into the shared library at `out`.
pub fn grammar(src_dir: &Path, out: &Path) -> Result<()> {
    let parser = src_dir.join("parser.c");
    if !parser.exists() {
        bail!("no parser.c in {}", src_dir.display());
    }
    // tree-sitter deprecated C++ scanners years ago; every maintained
    // grammar ships scanner.c. Supporting C++ would drag in a second
    // toolchain and stdlib-linking questions for a shrinking tail.
    if src_dir.join("scanner.cc").exists() {
        bail!("this grammar has a C++ scanner (scanner.cc), which drift does not support");
    }
    let scanner = src_dir.join("scanner.c");

    let target = env!("DRIFT_TARGET");
    let mut build = cc::Build::new();
    // cc is designed for build scripts; outside one, everything cargo
    // would provide via env vars must be set explicitly.
    build
        .cargo_metadata(false)
        .warnings(false)
        .opt_level(3)
        .host(target)
        .target(target);
    let compiler = build.try_get_compiler().context("no C compiler found")?;

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut cmd = compiler.to_command();
    if compiler.is_like_msvc() {
        cmd.arg(format!("/I{}", src_dir.display()))
            .args(["/nologo", "/LD", "/O2", "/utf-8"])
            .arg(&parser);
        if scanner.exists() {
            cmd.arg(&scanner);
        }
        cmd.arg("/link").arg(format!("/out:{}", out.display()));
    } else {
        cmd.args(["-shared", "-fPIC", "-fno-exceptions", "-O3", "-I"])
            .arg(src_dir)
            .arg(&parser);
        if scanner.exists() {
            cmd.arg(&scanner);
        }
        cmd.arg("-o").arg(out);
    }
    // MSVC drops intermediate .obj files in the working directory.
    if let Some(dir) = out.parent() {
        cmd.current_dir(dir);
    }
    run(cmd)
}

fn run(mut cmd: Command) -> Result<()> {
    let output = cmd
        .output()
        .with_context(|| format!("cannot run {:?}", cmd.get_program()))?;
    if !output.status.success() {
        bail!(
            "compiler failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
