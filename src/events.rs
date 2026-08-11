//! App events delivered to the main loop via channel, herdr-style:
//! background work (terminal input, view prefetching, the filesystem
//! watcher, status scans) sends events instead of the main loop polling.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::forge::model::{Comment, CommentThread, PrData, PullRequest};
use crate::processor::view::FileView;
use crate::vcs::model::{ChangedFile, Comparison};

pub enum AppEvent {
    Input(crossterm::event::Event),
    /// A background-computed view; stale generations are discarded.
    ViewReady {
        generation: u64,
        index: usize,
        view: FileView,
    },
    /// A debounced batch from the filesystem watcher: repo-relative paths
    /// that changed (already filtered against the VCS ignore rules), and
    /// whether git metadata (HEAD, refs, the index) moved.
    FsChanged {
        paths: Vec<PathBuf>,
        meta: bool,
    },
    /// A background status scan finished; stale sequences are discarded.
    StatusReady {
        seq: u64,
        result: Result<(Comparison, Vec<ChangedFile>), String>,
    },
    /// The forge listed open pull requests; stale sequences are discarded.
    PrListReady {
        seq: u64,
        result: Result<Vec<PullRequest>, String>,
    },
    /// One whole pull request (detail, diffs, comments) arrived; stale
    /// sequences are discarded.
    PrReady {
        seq: u64,
        result: Result<Box<PrData>, String>,
    },
    /// A comment was posted; on success the refetched threads and
    /// conversation ride along so the view updates in place.
    PrPosted {
        seq: u64,
        result: Result<RefreshedComments, String>,
    },
    /// One file's viewed state reached the forge, or didn't — a failure
    /// puts the optimistic check back the way it was.
    ViewedSynced {
        seq: u64,
        path: PathBuf,
        /// What was pushed — a failure restores the check to its
        /// negation, not to the negation of whatever it is by then.
        viewed: bool,
        result: Result<(), String>,
    },
    /// Progress line from a background language install ("fetching …");
    /// shown in the status bar as it happens.
    LangProgress(String),
    /// A background language install finished (the plugin is written and
    /// compiled, but not yet in the registry — the main loop registers).
    LangInstalled {
        name: &'static str,
        result: Result<(), String>,
    },
    /// The launch check found a newer release; shown as a status-bar
    /// notice unless something more urgent is already there.
    UpdateAvailable {
        version: String,
    },
    /// Spinner heartbeat while a forge request is in flight — the only
    /// time-driven redraws; the ticker thread stops when the wait ends.
    Tick,
}

/// The refetched comment side of a pull request after posting.
pub type RefreshedComments = Box<(Vec<CommentThread>, Vec<Comment>)>;

/// How long the input thread can stay inside one poll — the ceiling on
/// how stale a just-set pause flag can go unnoticed.
pub const INPUT_POLL_MS: u64 = 100;

/// Ask the terminal to disambiguate modified keys (the kitty keyboard
/// protocol), so shift+enter is distinguishable from enter in the
/// comment composer. Returns whether the terminal supports it — callers
/// only pop what was pushed. Must run while raw mode is active.
pub fn push_keyboard_enhancement() -> bool {
    if !matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) {
        return false;
    }
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    )
    .is_ok()
}

pub fn pop_keyboard_enhancement() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
}

/// Read terminal input, pausing while `paused` is set — an external
/// editor owns the terminal then, and reading here would steal its
/// keystrokes.
pub fn spawn_input_thread(tx: Sender<AppEvent>, paused: Arc<AtomicBool>) {
    thread::spawn(move || {
        loop {
            if paused.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            match crossterm::event::poll(Duration::from_millis(INPUT_POLL_MS)) {
                Ok(true) => {
                    let Ok(event) = crossterm::event::read() else {
                        return;
                    };
                    if tx.send(AppEvent::Input(event)).is_err() {
                        return;
                    }
                }
                Ok(false) => {}
                Err(_) => return,
            }
        }
    });
}

/// How long a change batch coalesces before it is filtered and
/// delivered — the ceiling on live-reload latency.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Watch the working tree and surface debounced, ignore-filtered change
/// batches. Best-effort: if the watcher can't start, live reload is
/// silently off and `R` still refreshes manually.
///
/// Debouncing is done here, on raw `notify` events, rather than by a
/// stock debouncer: the stock ones keep a file-ID cache that stat-walks
/// whole subtrees per event, which melts on repos with huge ignored
/// trees (a 90 GB `target/`). Per event, this thread pays one set
/// insert; everything expensive happens once per flush, on the deduped
/// batch.
pub fn spawn_watcher_thread(tx: Sender<AppEvent>, root: PathBuf) {
    thread::spawn(move || {
        // The watcher needs its own repository handle (for ignore rules);
        // gix handles aren't shared across threads.
        let Ok(vcs) = crate::vcs::detect(&root) else {
            return;
        };
        // FSEvents (and editors writing through symlinks) can report
        // resolved paths; accept either spelling of the root.
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let Ok(mut watcher) = notify::recommended_watcher(move |result| {
            let _ = raw_tx.send(result);
        }) else {
            return;
        };
        if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
            return;
        }
        let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
        // Set when the first path of a batch arrives and never pushed
        // back, so a sustained event storm still flushes every DEBOUNCE.
        let mut deadline: Option<Instant> = None;
        loop {
            let received = match deadline {
                None => match raw_rx.recv() {
                    Ok(result) => Some(result),
                    Err(_) => break,
                },
                Some(at) => {
                    match raw_rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                        Ok(result) => Some(result),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            };
            match received {
                Some(Ok(event)) => {
                    // Reads aren't changes; everything else (creates,
                    // writes, removes, renames) marks its paths dirty.
                    if matches!(event.kind, notify::EventKind::Access(_)) {
                        continue;
                    }
                    pending.extend(event.paths);
                    if deadline.is_none() && !pending.is_empty() {
                        deadline = Some(Instant::now() + DEBOUNCE);
                    }
                }
                Some(Err(_)) => {}
                None => {
                    deadline = None;
                    let (candidates, meta) =
                        split_batch(std::mem::take(&mut pending), &root, &canonical_root);
                    // Ignore-filtering here keeps build storms (target/, …)
                    // from ever reaching the app.
                    let paths = vcs.unignored(candidates);
                    if paths.is_empty() && !meta {
                        continue;
                    }
                    if tx.send(AppEvent::FsChanged { paths, meta }).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Split a batch of watcher paths into sorted, deduped repo-relative
/// change candidates plus whether git metadata moved. `.git` internals
/// and paths outside the repo never become candidates.
fn split_batch(
    batch: BTreeSet<PathBuf>,
    root: &Path,
    canonical_root: &Path,
) -> (Vec<PathBuf>, bool) {
    let mut meta = false;
    // Re-collect into a set: both root spellings can report the same
    // file, and they only collapse after prefix-stripping.
    let mut candidates = BTreeSet::new();
    for path in batch {
        let Ok(rel) = path
            .strip_prefix(root)
            .or_else(|_| path.strip_prefix(canonical_root))
        else {
            continue;
        };
        if rel.starts_with(".git") {
            meta |= is_git_meta(rel);
        } else if !rel.as_os_str().is_empty() {
            candidates.insert(rel.to_path_buf());
        }
    }
    (candidates.into_iter().collect(), meta)
}

/// The `.git` entries whose change means the status is stale: commits and
/// branch switches (HEAD, refs) and staging (index). Everything else —
/// index.lock churn, objects, logs — is noise.
fn is_git_meta(rel: &Path) -> bool {
    let Ok(sub) = rel.strip_prefix(".git") else {
        return false;
    };
    sub == Path::new("HEAD") || sub == Path::new("index") || sub.starts_with("refs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_batch_relativizes_dedups_and_flags_meta() {
        let root = Path::new("/repo");
        let canonical = Path::new("/private/repo");
        let batch: BTreeSet<PathBuf> = [
            "/repo/src/main.rs",
            "/private/repo/src/main.rs",
            "/repo/README.md",
            "/repo/.git/HEAD",
            "/repo/.git/objects/ab/cdef",
            "/elsewhere/file.rs",
            "/repo",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let (candidates, meta) = split_batch(batch, root, canonical);
        assert_eq!(
            candidates,
            vec![PathBuf::from("README.md"), PathBuf::from("src/main.rs")]
        );
        assert!(meta);
    }

    #[test]
    fn split_batch_ignores_git_noise() {
        let root = Path::new("/repo");
        let batch: BTreeSet<PathBuf> = [PathBuf::from("/repo/.git/index.lock")]
            .into_iter()
            .collect();
        let (candidates, meta) = split_batch(batch, root, root);
        assert!(candidates.is_empty());
        assert!(!meta);
    }

    #[test]
    fn git_meta_matches_head_index_and_refs_only() {
        assert!(is_git_meta(Path::new(".git/HEAD")));
        assert!(is_git_meta(Path::new(".git/index")));
        assert!(is_git_meta(Path::new(".git/refs/heads/main")));
        assert!(!is_git_meta(Path::new(".git/index.lock")));
        assert!(!is_git_meta(Path::new(".git/objects/ab/cdef")));
        assert!(!is_git_meta(Path::new("src/HEAD")));
    }
}
