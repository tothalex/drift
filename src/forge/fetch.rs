//! Fetching a pull request's commits into the local clone, so its files
//! get full sources — highlighting and block scoping. Only ever on the
//! reviewer's say-so (the PR view's fetch prompt), never by itself.
//!
//! Besides `drift lang`, the one place drift runs `git`: gitoxide could
//! fetch only with a network stack of its own, while `git` already
//! carries the user's credentials (ssh agent, credential helpers).

use std::path::Path;
use std::process::{Command, Stdio};

/// Fetch `head_ref` (the forge's ref for the pull request's head) from
/// `origin`, then — best-effort — the `base` commit, when it is missing
/// too: a server refusing fetches by SHA only leaves removed lines
/// unhighlighted. Objects only: no branch moves, the worktree stays.
pub fn fetch_pr(root: &Path, head_ref: &str, base: Option<&str>) -> Result<(), String> {
    git(root, &["fetch", "--no-tags", "--quiet", "origin", head_ref])?;
    if let Some(base) = base {
        let _ = git(root, &["fetch", "--no-tags", "--quiet", "origin", base]);
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    // drift owns the terminal, so ssh must fail rather than ask for a
    // passphrase or a host key on it — unless the user set up an ssh
    // command of their own, which is theirs to keep.
    let own_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || config_set(root, "core.sshCommand");
    if !own_ssh {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = cmd
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git fetch failed")
        .to_string())
}

fn config_set(root: &Path, key: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get", key])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.trim_ascii().is_empty())
}
