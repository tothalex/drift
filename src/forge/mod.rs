//! Forge integration: pull requests and review comments, via the official
//! `gh` / `glab` CLIs.
//!
//! drift never speaks HTTP itself — authentication, hostnames (including
//! self-hosted instances), and owner/repo resolution are all delegated to
//! the CLIs, which read them from the repository's remotes. Every call
//! runs with the repo root as its working directory for exactly that
//! reason. The CLIs return JSON; parsing into [`model`] types happens in
//! pure functions so they are testable from fixture strings.

pub mod model;

mod gh;
mod glab;
mod patch;

use std::path::Path;

use model::{Anchor, CommentThread, PrData, PrDetail, PullRequest};

/// Network-facing pull-request operations. Implementations shell out and
/// block; the app calls them from background threads only.
pub trait Forge: Send + Sync {
    /// UI copy: "pull request" / "merge request".
    fn pr_noun(&self) -> &'static str;
    fn list_open(&self) -> Result<Vec<PullRequest>, ForgeError>;
    fn load(&self, number: u64) -> Result<PrData, ForgeError>;
    fn post_general(&self, number: u64, body: &str) -> Result<(), ForgeError>;
    fn reply(&self, number: u64, thread_key: &str, body: &str) -> Result<(), ForgeError>;
    fn comment_inline(
        &self,
        detail: &PrDetail,
        anchor: &Anchor,
        body: &str,
    ) -> Result<(), ForgeError>;
    /// Delete one comment. `inline` distinguishes review comments from
    /// PR-level conversation comments where the forge's endpoints differ.
    fn delete_comment(&self, number: u64, comment_id: &str, inline: bool)
    -> Result<(), ForgeError>;
    /// Resolve or unresolve an inline review thread.
    fn resolve(&self, number: u64, thread_key: &str, resolved: bool) -> Result<(), ForgeError>;
    /// Refetch only the comment side of a pull request (after posting).
    fn threads(
        &self,
        number: u64,
        detail: &PrDetail,
    ) -> Result<(Vec<CommentThread>, Vec<model::Comment>), ForgeError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("{0} not installed — install it and run `{0} auth login`")]
    CliMissing(&'static str),
    #[error("{cli}: {stderr}")]
    CliFailed { cli: &'static str, stderr: String },
    #[error("unexpected {0} output: {1}")]
    Parse(&'static str, String),
    /// A request that can't be made (unanchorable comment, vanished
    /// thread) — not a parse failure; the message stands alone.
    #[error("{0}")]
    Invalid(String),
    #[error("origin remote '{0}' is not a recognized forge; set [forge] kind in the config")]
    UnknownForge(String),
    #[error("no origin remote")]
    NoRemote,
}

/// The `[forge]` config section, decoupled from the config crate types.
#[derive(Debug, Clone, Default)]
pub struct ForgeConfig {
    /// "github" | "gitlab" — overrides host detection (self-hosted
    /// instances whose hostname names neither).
    pub kind: Option<String>,
    /// Binary path overrides.
    pub gh: Option<String>,
    pub glab: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForgeKind {
    GitHub,
    GitLab,
}

/// The forge for the repository at `root`: origin's host decides between
/// GitHub and GitLab, with the config override for hosts naming neither.
pub fn detect(root: &Path, config: &ForgeConfig) -> Result<Box<dyn Forge>, ForgeError> {
    let host = origin_host(root)?;
    let kind = detect_kind(&host, config.kind.as_deref())?;
    Ok(match kind {
        ForgeKind::GitHub => Box::new(gh::GhCli::new(
            root.to_path_buf(),
            config.gh.clone().unwrap_or_else(|| "gh".to_string()),
        )),
        ForgeKind::GitLab => Box::new(glab::GlabCli::new(
            root.to_path_buf(),
            config.glab.clone().unwrap_or_else(|| "glab".to_string()),
        )),
    })
}

/// The hostname of the repository's origin remote. Opens its own gix
/// handle: gix repositories aren't shared across threads, and detection
/// runs on background threads.
fn origin_host(root: &Path) -> Result<String, ForgeError> {
    let repo = gix::discover(root).map_err(|_| ForgeError::NoRemote)?;
    let remote = repo
        .find_remote("origin")
        .map_err(|_| ForgeError::NoRemote)?;
    let url = remote
        .url(gix::remote::Direction::Fetch)
        .ok_or(ForgeError::NoRemote)?;
    host_of_url(url).ok_or_else(|| ForgeError::UnknownForge(url.to_bstring().to_string()))
}

fn host_of_url(url: &gix::Url) -> Option<String> {
    url.host().map(str::to_string)
}

/// host + optional config override → forge kind. Exact cloud hosts first,
/// then a substring heuristic for self-hosted instances like
/// `gitlab.mycompany.com`.
fn detect_kind(host: &str, override_: Option<&str>) -> Result<ForgeKind, ForgeError> {
    match override_ {
        Some("github") => return Ok(ForgeKind::GitHub),
        Some("gitlab") => return Ok(ForgeKind::GitLab),
        Some(other) => return Err(ForgeError::UnknownForge(format!("kind '{other}'"))),
        None => {}
    }
    let lower = host.to_ascii_lowercase();
    if lower == "github.com" || lower.contains("github") {
        return Ok(ForgeKind::GitHub);
    }
    if lower == "gitlab.com" || lower.contains("gitlab") {
        return Ok(ForgeKind::GitLab);
    }
    Err(ForgeError::UnknownForge(host.to_string()))
}

/// Run a forge CLI non-interactively in the repo root and return stdout.
/// A missing binary becomes [`ForgeError::CliMissing`]; a non-zero exit
/// (not installed repos, not authenticated, API rejections) surfaces the
/// first meaningful stderr line.
pub(crate) fn run_cli(
    cli: &'static str,
    program: &str,
    root: &Path,
    args: &[String],
) -> Result<String, ForgeError> {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("PAGER", "cat")
        .env("GH_PAGER", "cat")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GLAB_CHECK_UPDATE", "false")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => ForgeError::CliMissing(cli),
            _ => ForgeError::CliFailed {
                cli,
                stderr: err.to_string(),
            },
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("failed")
            .to_string();
        return Err(ForgeError::CliFailed { cli, stderr: line });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// CLI argument lists are built as owned strings (formatted paths, body
/// text); plain flags go through this.
pub(crate) fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(ToString::to_string).collect()
}

/// Parse `--paginate` output: both CLIs emit each page's JSON array
/// back-to-back (`[…][…]`), not one merged array. Also accepts a single
/// array, and returns empty for blank output.
pub(crate) fn concat_arrays<T: serde::de::DeserializeOwned>(
    cli: &'static str,
    text: &str,
) -> Result<Vec<T>, ForgeError> {
    let mut items = Vec::new();
    for page in serde_json::Deserializer::from_str(text).into_iter::<Vec<T>>() {
        items.extend(page.map_err(|err| ForgeError::Parse(cli, err.to_string()))?);
    }
    Ok(items)
}

/// The date prefix of an RFC 3339 timestamp, for display.
pub(crate) fn date_of(timestamp: &str) -> &str {
    timestamp.split('T').next().unwrap_or(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(url: &str) -> Option<String> {
        let parsed = gix::url::parse(url.into()).ok()?;
        host_of_url(&parsed)
    }

    #[test]
    fn origin_urls_yield_hosts() {
        assert_eq!(
            host("https://github.com/owner/repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host("https://gitlab.com/group/sub/repo.git").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            host("ssh://git@github.com/owner/repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host("ssh://git@gitlab.example.com:2222/owner/repo.git").as_deref(),
            Some("gitlab.example.com")
        );
        // scp-like syntax.
        assert_eq!(
            host("git@github.com:owner/repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host("git@gitlab.mycompany.io:owner/repo.git").as_deref(),
            Some("gitlab.mycompany.io")
        );
        // Local paths have no host.
        assert_eq!(host("/srv/git/repo.git"), None);
    }

    #[test]
    fn kind_detection_covers_cloud_self_hosted_and_override() {
        assert_eq!(detect_kind("github.com", None).unwrap(), ForgeKind::GitHub);
        assert_eq!(detect_kind("gitlab.com", None).unwrap(), ForgeKind::GitLab);
        assert_eq!(
            detect_kind("github.mycorp.net", None).unwrap(),
            ForgeKind::GitHub
        );
        assert_eq!(
            detect_kind("gitlab.mycorp.net", None).unwrap(),
            ForgeKind::GitLab
        );
        assert!(matches!(
            detect_kind("git.mycorp.net", None),
            Err(ForgeError::UnknownForge(_))
        ));
        assert_eq!(
            detect_kind("git.mycorp.net", Some("github")).unwrap(),
            ForgeKind::GitHub
        );
        assert_eq!(
            detect_kind("git.mycorp.net", Some("gitlab")).unwrap(),
            ForgeKind::GitLab
        );
        assert!(detect_kind("git.mycorp.net", Some("bitbucket")).is_err());
    }

    #[test]
    fn concat_arrays_accepts_single_and_concatenated_pages() {
        let one: Vec<u32> = concat_arrays("gh", "[1, 2]").unwrap();
        assert_eq!(one, vec![1, 2]);
        let paged: Vec<u32> = concat_arrays("gh", "[1, 2]\n[3]").unwrap();
        assert_eq!(paged, vec![1, 2, 3]);
        let empty: Vec<u32> = concat_arrays("gh", "  \n").unwrap();
        assert!(empty.is_empty());
        assert!(concat_arrays::<u32>("gh", "not json").is_err());
    }

    #[test]
    fn date_of_takes_the_date_prefix() {
        assert_eq!(date_of("2026-07-01T10:20:30Z"), "2026-07-01");
        assert_eq!(date_of("unparsed"), "unparsed");
    }
}
