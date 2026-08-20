//! Versions of the external CLIs drift shells out to (`gh`, `glab`,
//! `herdr`, `tmux`, `cmux`), probed once per process and parsed
//! tolerantly. drift never checks a version up front — the happy path
//! costs nothing — a version only explains a call the CLI just
//! rejected, and fills `drift doctor`.

use std::collections::HashMap;
use std::sync::Mutex;

/// The oldest version of a tool drift's calls need: (major, minor, patch).
pub type Floor = (u32, u32, u32);

/// The external CLIs drift drives, with the oldest version drift's
/// calls need. `None` means drift stays on a surface old and stable
/// enough that no minimum is worth stating — gh and glab ship breaking
/// changes in minor releases anyway, so their numbers carry no signal.
pub const TOOLS: &[(&str, Option<Floor>)] = &[
    ("gh", None),
    ("glab", None),
    // 0.7.5 replaced `agent send` with the pane primitives drift uses.
    ("herdr", Some((0, 7, 5))),
    ("tmux", None),
    ("cmux", None),
];

/// A probed CLI version. Comparisons use the numeric triple; `raw`
/// keeps the full string ("0.8.0-preview.2026-08-04") for messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub raw: String,
}

impl CliVersion {
    /// Parse `tool --version` output: the first token that starts with
    /// a digit, read as `major[.minor[.patch]]` with trailing suffixes
    /// per component ignored ("3.6a" → 3.6.0, "0.8.0-preview.…" →
    /// 0.8.0). Tolerant on purpose: preview and dev builds must compare
    /// as their base release, and an unparseable version means
    /// "unknown", never an error.
    pub fn parse(output: &str) -> Option<CliVersion> {
        let token = output
            .split_whitespace()
            .find(|token| token.starts_with(|c: char| c.is_ascii_digit()))?;
        let mut parts = token.splitn(3, '.').map(leading_number);
        let major = parts.next().flatten()?;
        let minor = parts.next().flatten().unwrap_or(0);
        let patch = parts.next().flatten().unwrap_or(0);
        Some(CliVersion {
            major,
            minor,
            patch,
            raw: token.to_string(),
        })
    }

    pub fn at_least(&self, (major, minor, patch): Floor) -> bool {
        (self.major, self.minor, self.patch) >= (major, minor, patch)
    }
}

/// The digits a version component starts with; None when it starts
/// with none ("preview").
fn leading_number(part: &str) -> Option<u32> {
    let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The version `program` reports, probed at most once per process —
/// including the misses, so a broken tool is not re-run on every
/// failure. `program` may be a configured path; the version flag is
/// chosen by basename (tmux only knows `-V`).
pub fn version_of(program: &str) -> Option<CliVersion> {
    type Probed = HashMap<String, Option<CliVersion>>;
    static CACHE: Mutex<Option<Probed>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(known) = cache.get(program) {
        return known.clone();
    }
    let flag = if basename(program) == "tmux" {
        "-V"
    } else {
        "--version"
    };
    let probed = std::process::Command::new(program)
        .arg(flag)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            // Some tools print the version to stderr.
            CliVersion::parse(&String::from_utf8_lossy(&output.stdout))
                .or_else(|| CliVersion::parse(&String::from_utf8_lossy(&output.stderr)))
        });
    cache.insert(program.to_string(), probed.clone());
    probed
}

/// Whether a failed invocation is the CLI rejecting the command shape
/// itself — unknown subcommand, unknown flag, usage text — rather than
/// failing to execute it. Grounded against the real tools: herdr exits
/// 2 with its usage list; gh/glab/tmux exit 1 saying "unknown command"
/// or "unknown flag"; clap-based tools exit 2 with "unrecognized
/// subcommand".
pub fn usage_error(code: Option<i32>, stderr: &str) -> bool {
    if code == Some(2) {
        return true;
    }
    let lower = stderr.to_ascii_lowercase();
    [
        "unknown command",
        "unrecognized subcommand",
        "unknown flag",
        "unknown option",
        "usage:",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// One line explaining a usage-shaped failure in version terms, for a
/// tool drift knows: too old for the declared floor, or newer than the
/// commands this drift was built against. Unknown tools and
/// unprobeable versions explain nothing.
pub fn explain_rejection(program: &str) -> Option<String> {
    let (_, floor) = TOOLS.iter().find(|(tool, _)| *tool == basename(program))?;
    mismatch(basename(program), &version_of(program)?, *floor)
}

fn mismatch(tool: &str, version: &CliVersion, floor: Option<Floor>) -> Option<String> {
    match floor {
        Some(floor) if !version.at_least(floor) => Some(format!(
            "{tool} {} is older than the {} drift needs — update {tool}",
            version.raw,
            triple(floor),
        )),
        Some(floor) => Some(format!(
            "{tool} {} no longer accepts this command (drift targets {tool} ≥ {}) — check for a drift update",
            version.raw,
            triple(floor),
        )),
        None => Some(format!(
            "{tool} {} did not accept this command — check for a drift update",
            version.raw,
        )),
    }
}

fn triple((major, minor, patch): Floor) -> String {
    format!("{major}.{minor}.{patch}")
}

fn basename(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(output: &str) -> (u32, u32, u32) {
        let v = CliVersion::parse(output).unwrap();
        (v.major, v.minor, v.patch)
    }

    #[test]
    fn versions_parse_from_real_tool_output() {
        assert_eq!(parsed("herdr 0.8.0"), (0, 8, 0));
        assert_eq!(parsed("gh version 2.97.0 (2026-07-31)"), (2, 97, 0));
        assert_eq!(parsed("glab 1.114.0"), (1, 114, 0));
        // Letter and preview suffixes compare as their base release.
        assert_eq!(parsed("tmux 3.6a"), (3, 6, 0));
        assert_eq!(
            parsed("herdr 0.8.0-preview.2026-08-04-d78e3d3b5126"),
            (0, 8, 0)
        );
        // Only the major is required.
        assert_eq!(parsed("tool 7"), (7, 0, 0));
        assert!(CliVersion::parse("no digits here").is_none());
        assert!(CliVersion::parse("").is_none());
    }

    #[test]
    fn the_raw_string_survives_for_messages() {
        let v = CliVersion::parse("herdr 0.8.0-preview.2026-08-04").unwrap();
        assert_eq!(v.raw, "0.8.0-preview.2026-08-04");
    }

    #[test]
    fn at_least_compares_the_triple() {
        let v = CliVersion::parse("herdr 0.7.5").unwrap();
        assert!(v.at_least((0, 7, 5)));
        assert!(v.at_least((0, 7, 0)));
        assert!(!v.at_least((0, 8, 0)));
        assert!(!v.at_least((1, 0, 0)));
    }

    /// Real rejections from the tools drift drives, verified by hand:
    /// herdr exits 2 printing its command list; gh and tmux exit 1.
    #[test]
    fn usage_errors_are_recognized_by_code_or_text() {
        assert!(usage_error(Some(2), "herdr agent commands:"));
        assert!(usage_error(
            Some(1),
            "unknown command \"nonsense\" for \"gh\""
        ));
        assert!(usage_error(Some(1), "unknown flag: --frobnicate"));
        assert!(usage_error(Some(1), "unknown command: badcmd"));
        assert!(usage_error(
            Some(2),
            "error: unrecognized subcommand 'send'"
        ));
        assert!(usage_error(Some(1), "usage: tmux send-keys …"));
        // Runtime failures are not surface mismatches.
        assert!(!usage_error(
            Some(1),
            "no server running on /tmp/tmux-501/default"
        ));
        assert!(!usage_error(
            Some(1),
            "GraphQL: Could not resolve to a PullRequest"
        ));
        assert!(!usage_error(None, "killed by signal"));
    }

    #[test]
    fn mismatches_name_the_direction() {
        let old = CliVersion::parse("herdr 0.6.2").unwrap();
        assert_eq!(
            mismatch("herdr", &old, Some((0, 7, 5))).unwrap(),
            "herdr 0.6.2 is older than the 0.7.5 drift needs — update herdr"
        );
        let new = CliVersion::parse("herdr 0.9.0").unwrap();
        assert_eq!(
            mismatch("herdr", &new, Some((0, 7, 5))).unwrap(),
            "herdr 0.9.0 no longer accepts this command (drift targets herdr ≥ 0.7.5) — check for a drift update"
        );
        let gh = CliVersion::parse("gh version 2.97.0").unwrap();
        assert_eq!(
            mismatch("gh", &gh, None).unwrap(),
            "gh 2.97.0 did not accept this command — check for a drift update"
        );
    }

    #[test]
    fn unknown_tools_explain_nothing() {
        // `ps` fails through the same run_cli path; it must never be
        // version-probed.
        assert!(explain_rejection("ps").is_none());
        assert!(explain_rejection("/usr/bin/ps").is_none());
    }

    #[test]
    fn floors_name_real_tools() {
        for (tool, _) in TOOLS {
            assert_eq!(*tool, basename(tool), "floors are keyed by basename");
        }
    }
}
