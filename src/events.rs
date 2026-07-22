//! App events delivered to the main loop via channel, herdr-style:
//! background work (terminal input, view prefetching, the filesystem
//! watcher, status scans) sends events instead of the main loop polling.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};

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

/// Watch the working tree and surface debounced, ignore-filtered change
/// batches. Best-effort: if the watcher can't start, live reload is
/// silently off and `R` still refreshes manually.
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
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<DebounceEventResult>();
        let Ok(mut debouncer) = new_debouncer(Duration::from_millis(300), None, raw_tx) else {
            return;
        };
        if debouncer.watch(&root, RecursiveMode::Recursive).is_err() {
            return;
        }
        while let Ok(result) = raw_rx.recv() {
            let Ok(events) = result else {
                continue;
            };
            let mut meta = false;
            let mut candidates = Vec::new();
            for event in &events {
                for path in &event.paths {
                    let Some(rel) = path
                        .strip_prefix(&root)
                        .or_else(|_| path.strip_prefix(&canonical_root))
                        .ok()
                    else {
                        continue;
                    };
                    if rel.starts_with(".git") {
                        meta |= is_git_meta(rel);
                    } else if !rel.as_os_str().is_empty() {
                        candidates.push(rel.to_path_buf());
                    }
                }
            }
            candidates.sort();
            candidates.dedup();
            // Ignore-filtering here keeps build storms (target/, …) from
            // ever reaching the app.
            let paths = vcs.unignored(candidates);
            if paths.is_empty() && !meta {
                continue;
            }
            if tx.send(AppEvent::FsChanged { paths, meta }).is_err() {
                break;
            }
        }
    });
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
    fn git_meta_matches_head_index_and_refs_only() {
        assert!(is_git_meta(Path::new(".git/HEAD")));
        assert!(is_git_meta(Path::new(".git/index")));
        assert!(is_git_meta(Path::new(".git/refs/heads/main")));
        assert!(!is_git_meta(Path::new(".git/index.lock")));
        assert!(!is_git_meta(Path::new(".git/objects/ab/cdef")));
        assert!(!is_git_meta(Path::new("src/HEAD")));
    }
}
