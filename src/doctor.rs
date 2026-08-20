//! `drift doctor`: one screen saying whether every external CLI drift
//! drives is present, what version it is against the floor drift needs,
//! and what this directory resolves to. Its output is what a bug report
//! needs — the issue template asks for it.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cliver::{self, CliVersion};
use crate::{config, connect, forge};

pub fn run() -> Result<()> {
    println!("\n  ~ drift doctor\n");
    log(&format!("drift {}", env!("CARGO_PKG_VERSION")));

    // A config that fails to parse is itself a finding; the checks go
    // on with defaults so one bad section doesn't hide the rest.
    let (forge_config, agent_config) = match config::load() {
        Ok(config) => {
            log("config ok");
            (config.forge, config.agent)
        }
        Err(err) => {
            log(&format!("config: {err:#}"));
            (
                forge::ForgeConfig::default(),
                connect::AgentConfig::default(),
            )
        }
    };

    println!();
    for (tool, floor) in cliver::TOOLS {
        // The binaries the config points drift at, not bare names —
        // the version of a gh that isn't the one drift runs would
        // diagnose nothing.
        let program = match *tool {
            "gh" => forge_config.gh.clone().unwrap_or_else(|| "gh".to_string()),
            "glab" => forge_config
                .glab
                .clone()
                .unwrap_or_else(|| "glab".to_string()),
            "cmux" => connect::cmux_cli_path(),
            other => other.to_string(),
        };
        let mut line = tool_line(tool, &program, *floor);
        if let Some(auth) = auth_status(tool, &program) {
            line.push_str(&format!(" ({auth})"));
        }
        log(&line);
    }

    println!();
    match forge::detect_name(Path::new("."), &forge_config) {
        Ok(name) => log(&format!("forge here: {name}")),
        Err(err) => log(&format!("forge here: {err}")),
    }
    if agent_config.backend == connect::Backend::Off {
        log("agent backend: off (config)");
    } else {
        match connect::detect(&agent_config) {
            Some(bridge) => log(&format!("agent backend: {}", bridge.label())),
            None => log("agent backend: none detected"),
        }
    }
    println!();
    Ok(())
}

fn log(line: &str) {
    println!("  {line}");
}

/// "herdr  0.8.0 (needs ≥ 0.7.5: ok) — /opt/homebrew/bin/herdr".
fn tool_line(tool: &str, program: &str, floor: Option<cliver::Floor>) -> String {
    let Some(path) = resolve(program) else {
        return format!("{tool:<6} not installed");
    };
    let state = match cliver::version_of(program) {
        Some(version) => verdict(&version, floor),
        // Present but not answering --version: the path is the clue.
        None => "version unknown".to_string(),
    };
    format!("{tool:<6} {state} — {}", path.display())
}

/// The version against its floor, when the tool has one.
fn verdict(version: &CliVersion, floor: Option<cliver::Floor>) -> String {
    match floor {
        Some((major, minor, patch)) => {
            let ok = if version.at_least((major, minor, patch)) {
                "ok"
            } else {
                "too old"
            };
            format!("{} (needs ≥ {major}.{minor}.{patch}: {ok})", version.raw)
        }
        None => version.raw.clone(),
    }
}

/// The forge CLIs know whether they're signed in; nothing else does.
/// Not part of [`tool_line`] so tests never spawn `auth status`.
fn auth_status(tool: &str, program: &str) -> Option<&'static str> {
    if tool != "gh" && tool != "glab" {
        return None;
    }
    let ok = std::process::Command::new(program)
        .args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?
        .status
        .success();
    Some(if ok {
        "auth ok"
    } else {
        "not authenticated — run `auth login`"
    })
}

/// Where `program` actually is: verbatim if it names a path, else the
/// first PATH hit — the answer to "which binary is drift running?"
/// when two versions are installed.
fn resolve(program: &str) -> Option<PathBuf> {
    let direct = Path::new(program);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .flat_map(|dir| {
            let plain = dir.join(program);
            let suffixed = dir.join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
            [plain, suffixed]
        })
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_compare_against_the_floor() {
        let version = CliVersion::parse("herdr 0.8.0").unwrap();
        assert_eq!(
            verdict(&version, Some((0, 7, 5))),
            "0.8.0 (needs ≥ 0.7.5: ok)"
        );
        let old = CliVersion::parse("herdr 0.6.2-preview.2026-01-01").unwrap();
        assert_eq!(
            verdict(&old, Some((0, 7, 5))),
            "0.6.2-preview.2026-01-01 (needs ≥ 0.7.5: too old)"
        );
        let gh = CliVersion::parse("gh version 2.97.0 (2026-07-31)").unwrap();
        assert_eq!(verdict(&gh, None), "2.97.0");
    }

    #[test]
    fn every_known_tool_renders_a_line() {
        // Smoke over the real environment: never panics, always names
        // the tool, whatever is or isn't installed here.
        for (tool, floor) in cliver::TOOLS {
            let line = tool_line(tool, tool, *floor);
            assert!(line.starts_with(tool), "{line}");
        }
    }
}
