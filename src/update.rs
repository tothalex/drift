//! `drift update`: self-update from GitHub releases, plus the launch
//! check behind the status-bar notice.
//!
//! Network goes through the `curl` binary — drift never speaks HTTP
//! itself, and curl ships with macOS, Windows 10+, and virtually every
//! Linux. The latest version is read from the redirect target of
//! `releases/latest` (no API, no rate limits); the release asset is
//! verified against the published `checksums.txt` before `self-replace`
//! swaps the running executable.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::events::AppEvent;

const REPO: &str = "tothalex/drift";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How long a launch-check result is trusted before asking the network
/// again. The notice still shows on every launch from the cached answer.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The `[update]` section: the launch check behind the status-bar notice.
pub struct UpdateConfig {
    pub check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig { check: true }
    }
}

/// `drift update [--check]`: report or install the latest release.
pub fn run(check_only: bool) -> Result<()> {
    println!("\n  ~ drift update\n");
    log(&format!("current version {CURRENT}"));
    let tag = latest_tag()?;
    let latest = tag.trim_start_matches('v');
    remember(&tag);
    let (Some(latest_v), Some(current_v)) = (parse_version(latest), parse_version(CURRENT)) else {
        bail!("cannot compare versions '{latest}' and '{CURRENT}'");
    };
    if latest_v <= current_v {
        log(&format!("latest release is {latest} — up to date"));
        println!();
        return Ok(());
    }
    log(&format!("drift {latest} is available"));
    if check_only {
        println!("\n  run `drift update` to install\n");
        return Ok(());
    }

    let exe = std::env::current_exe().context("cannot locate the running executable")?;
    // Resolve symlinks: homebrew links bin/drift into its Cellar.
    let exe = exe.canonicalize().unwrap_or(exe);
    if let Some(hint) = managed_hint(&exe) {
        bail!("{hint}");
    }

    let Some(asset) = asset_name() else {
        bail!(
            "no prebuilt binary for {}/{} — build from source: https://github.com/{REPO}",
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
    };

    let staging = std::env::temp_dir().join(format!("drift-update-{}", std::process::id()));
    std::fs::create_dir_all(&staging)?;
    let result = install(&staging, &tag, &asset);
    let _ = std::fs::remove_dir_all(&staging);
    result?;

    log(&format!("updated {} to {latest}", exe.display()));
    println!("\n  if highlighting breaks, rebuild the grammar plugins: drift lang build\n");
    Ok(())
}

fn install(staging: &Path, tag: &str, asset: &str) -> Result<()> {
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let archive = staging.join(asset);
    log(&format!("downloading {base}/{asset}"));
    curl_to(&format!("{base}/{asset}"), &archive)?;
    let checksums = staging.join("checksums.txt");
    curl_to(&format!("{base}/checksums.txt"), &checksums)?;

    let expected = checksum_for(&std::fs::read_to_string(&checksums)?, asset)
        .with_context(|| format!("{asset} is not listed in the release's checksums.txt"))?;
    let actual = sha256_hex(&std::fs::read(&archive)?);
    if actual != expected {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }
    log("checksum verified");

    // Windows' tar (bsdtar) extracts zip archives with the same flags.
    let mut tar = Command::new("tar");
    tar.arg("-xf").arg(&archive).arg("-C").arg(staging);
    let output = tar.output().context("cannot run tar — is tar installed?")?;
    if !output.status.success() {
        bail!(
            "tar failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let binary = staging.join(format!("drift{}", std::env::consts::EXE_SUFFIX));
    if !binary.is_file() {
        bail!("release archive {asset} does not contain a drift binary");
    }
    self_replace::self_replace(&binary).context("cannot replace the running executable")?;
    Ok(())
}

/// Spawn the throttled launch check; a newer release lands in the app
/// as [`AppEvent::UpdateAvailable`]. Best-effort: failures are silent.
pub fn spawn_launch_check(tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        if let Some(version) = launch_check() {
            let _ = tx.send(AppEvent::UpdateAvailable { version });
        }
    });
}

fn launch_check() -> Option<String> {
    let stamp = stamp_path();
    let fresh = std::fs::metadata(&stamp)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < CHECK_INTERVAL);
    let tag = if fresh {
        std::fs::read_to_string(&stamp).ok()?.trim().to_string()
    } else {
        let tag = latest_tag().ok()?;
        remember(&tag);
        tag
    };
    let latest = tag.trim_start_matches('v');
    (parse_version(latest)? > parse_version(CURRENT)?).then(|| latest.to_string())
}

/// The launch check's cache: the last tag the network reported, aged by
/// the file's mtime. `~/.cache/drift` (or `$XDG_CACHE_HOME`), like the
/// compiled grammars.
fn stamp_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // HOME is absent on Windows; home_dir falls back to USERPROFILE.
            std::env::home_dir().unwrap_or_default().join(".cache")
        });
    base.join("drift").join("latest-release")
}

fn remember(tag: &str) {
    let stamp = stamp_path();
    if let Some(dir) = stamp.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&stamp, tag);
}

/// The latest release tag, read from where `releases/latest` redirects.
fn latest_tag() -> Result<String> {
    let latest = format!("https://github.com/{REPO}/releases/latest");
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = Command::new("curl")
        .args(["-fsSLI", "-o", null, "-w", "%{url_effective}"])
        .args(["--connect-timeout", "10", "--max-time", "30"])
        .arg(&latest)
        .output()
        .context("cannot run curl — is curl installed?")?;
    if !output.status.success() {
        bail!(
            "cannot reach {latest}:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&output.stdout);
    tag_from_release_url(url.trim())
        .map(str::to_string)
        .with_context(|| format!("no published release found at {latest}"))
}

fn curl_to(url: &str, dest: &Path) -> Result<()> {
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "--retry", "3"])
        .args(["--connect-timeout", "10"])
        .arg("-o")
        .arg(dest)
        .arg(url);
    let output = curl
        .output()
        .context("cannot run curl — is curl installed?")?;
    if !output.status.success() {
        bail!(
            "download failed for {url}:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn log(line: &str) {
    println!("  {line}");
}

// --- pure helpers ---

/// "v1.2.3" or "1.2.3" → (1, 2, 3); anything else is None.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').splitn(3, '.');
    let mut next = || parts.next()?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// The tag at the end of a `…/releases/tag/<tag>` URL.
fn tag_from_release_url(url: &str) -> Option<&str> {
    let (_, tag) = url.rsplit_once("/releases/tag/")?;
    (!tag.is_empty() && !tag.contains('/')).then_some(tag)
}

/// The release asset for this platform — the names `release.yml` builds.
fn asset_name() -> Option<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("linux" | "macos", "x86_64" | "aarch64") => Some(format!("drift-{os}-{arch}.tar.gz")),
        ("windows", "x86_64") => Some("drift-windows-x86_64.zip".to_string()),
        _ => None,
    }
}

/// The `shasum -a 256` line for `asset`: "<hex>  <name>".
fn checksum_for(checksums: &str, asset: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?;
        // shasum marks binary-mode files with a leading '*'.
        (name.trim_start_matches('*') == asset).then(|| hash.to_ascii_lowercase())
    })
}

/// A refusal when another installer owns this binary — self-updating
/// would be overwritten (cargo) or corrupt its bookkeeping (homebrew).
fn managed_hint(exe: &Path) -> Option<&'static str> {
    let path = exe.to_string_lossy().replace('\\', "/");
    if path.contains("/.cargo/bin/") {
        Some("this drift was installed with cargo — update with: cargo install drift")
    } else if path.contains("/Cellar/") || path.contains("/linuxbrew/") {
        Some("this drift is managed by homebrew — update with: brew upgrade")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parses_with_and_without_prefix() {
        assert_eq!(parse_version("v0.14.0"), Some((0, 14, 0)));
        assert_eq!(parse_version("1.2.30"), Some((1, 2, 30)));
        assert_eq!(parse_version("0.14"), None);
        assert_eq!(parse_version("0.14.0.1"), None);
        assert_eq!(parse_version("v0.14.0-rc1"), None);
        assert_eq!(parse_version("tag"), None);
    }

    #[test]
    fn version_tuples_order() {
        assert!(parse_version("0.15.0") > parse_version("0.14.9"));
        assert!(parse_version("1.0.0") > parse_version("0.99.99"));
        assert!(parse_version("0.14.0") <= parse_version("0.14.0"));
    }

    #[test]
    fn tag_parses_from_redirect_target() {
        assert_eq!(
            tag_from_release_url("https://github.com/tothalex/drift/releases/tag/v0.14.0"),
            Some("v0.14.0")
        );
        // No releases: /releases/latest lands on the releases index.
        assert_eq!(
            tag_from_release_url("https://github.com/tothalex/drift/releases"),
            None
        );
        assert_eq!(
            tag_from_release_url("https://github.com/releases/tag/"),
            None
        );
    }

    #[test]
    fn checksum_finds_the_asset_line() {
        let sums = "abc123  drift-macos-aarch64.tar.gz\n\
                    def456 *drift-windows-x86_64.zip\n";
        assert_eq!(
            checksum_for(sums, "drift-macos-aarch64.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            checksum_for(sums, "drift-windows-x86_64.zip").as_deref(),
            Some("def456")
        );
        assert_eq!(checksum_for(sums, "drift-linux-x86_64.tar.gz"), None);
    }

    #[test]
    fn asset_name_matches_release_workflow() {
        // The dev/CI platforms are all release targets.
        let name = asset_name().expect("current platform must be a release target");
        assert!(name.starts_with("drift-"));
        assert!(name.ends_with(".tar.gz") || name.ends_with(".zip"));
    }

    #[test]
    fn managed_installs_are_refused_with_a_hint() {
        assert!(managed_hint(Path::new("/Users/x/.cargo/bin/drift")).is_some());
        assert!(managed_hint(Path::new("C:\\Users\\x\\.cargo\\bin\\drift.exe")).is_some());
        assert!(managed_hint(Path::new("/opt/homebrew/Cellar/drift/0.14.0/bin/drift")).is_some());
        assert!(managed_hint(Path::new("/home/x/.local/bin/drift")).is_none());
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
