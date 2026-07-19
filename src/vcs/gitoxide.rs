//! Git provider backed by gitoxide (`gix`) — fully in-process, no `git`
//! binary required.
//!
//! The change list is computed by building an in-memory index from the
//! merge-base tree and running gix's index-vs-worktree status against it:
//! that is semantically `git diff <ancestor>` plus untracked files, with
//! rename tracking. Per-file diffs pair the ancestor blob with the
//! working-tree file through imara-diff.

use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::status::UntrackedFiles;
use gix::status::index_worktree::Item;
use gix::status::index_worktree::iter::Summary;
use imara_diff::{Algorithm, Diff, InternedInput};

use crate::vcs::model::{
    ChangedFile, Comparison, DiffLine, FileDiff, FileStatus, Hunk, LineKind, RevisionId,
};
use crate::vcs::{Vcs, VcsError};

pub struct GixVcs {
    repo: gix::Repository,
    root: PathBuf,
}

impl GixVcs {
    /// `Ok(None)` when `path` is not inside a (non-bare) git repository.
    pub fn detect(path: &Path) -> Result<Option<GixVcs>, VcsError> {
        let Ok(mut repo) = gix::discover(path) else {
            return Ok(None);
        };
        // Blob and tree lookups repeat heavily (per-file diffs walk the
        // ancestor tree); a small object cache removes that cost.
        repo.object_cache_size_if_unset(4 * 1024 * 1024);
        let Some(root) = repo.workdir().map(Path::to_path_buf) else {
            return Ok(None); // bare repository: nothing to review
        };
        Ok(Some(GixVcs { repo, root }))
    }

    fn rev_commit_id(&self, rev: &str) -> Option<gix::ObjectId> {
        let id = self.repo.rev_parse_single(rev).ok()?;
        let commit = id.object().ok()?.peel_to_commit().ok()?;
        Some(commit.id)
    }

    fn default_base(&self) -> Result<String, VcsError> {
        if let Ok(reference) = self.repo.find_reference("refs/remotes/origin/HEAD")
            && let gix::refs::TargetRef::Symbolic(name) = reference.target()
            && let Some(name) = name.as_bstr().strip_prefix(b"refs/remotes/")
        {
            return Ok(name.to_str_lossy().into_owned());
        }
        for candidate in ["main", "master"] {
            if self.rev_commit_id(candidate).is_some() {
                return Ok(candidate.to_string());
            }
        }
        Err(VcsError::NoDefaultBase)
    }

    fn ancestor_blob(&self, ancestor: &RevisionId, path: &Path) -> Option<Vec<u8>> {
        let id = gix::ObjectId::from_hex(ancestor.0.as_bytes()).ok()?;
        let commit = self.repo.find_object(id).ok()?.peel_to_commit().ok()?;
        let entry = commit.tree().ok()?.lookup_entry_by_path(path).ok()??;
        Some(entry.object().ok()?.detach().data)
    }
}

impl Vcs for GixVcs {
    fn root(&self) -> &Path {
        &self.root
    }

    fn comparison(&self, base_override: Option<&str>) -> Result<Comparison, VcsError> {
        let base = match base_override {
            Some(base) => {
                if self.rev_commit_id(base).is_none() {
                    return Err(VcsError::RevisionNotFound(base.to_string()));
                }
                base.to_string()
            }
            None => self.default_base()?,
        };
        let head = self
            .repo
            .head_id()
            .map_err(|_| VcsError::RevisionNotFound("HEAD".to_string()))?;
        let base_id = self
            .rev_commit_id(&base)
            .ok_or_else(|| VcsError::RevisionNotFound(base.clone()))?;
        let ancestor = self.repo.merge_base(base_id, head.detach()).map_err(|_| {
            VcsError::NoCommonAncestor {
                base: base.clone(),
                work: "HEAD".to_string(),
            }
        })?;
        let work_label = self
            .repo
            .head_name()
            .ok()
            .flatten()
            .map(|name| name.shorten().to_str_lossy().into_owned())
            .unwrap_or_else(|| "HEAD (detached)".to_string());
        Ok(Comparison {
            base_label: base,
            ancestor: RevisionId(ancestor.to_string()),
            work_label,
        })
    }

    fn changed_files(&self, cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError> {
        let ancestor_id = gix::ObjectId::from_hex(cmp.ancestor.0.as_bytes())
            .map_err(|err| VcsError::Tool(format!("bad ancestor id: {err}")))?;
        let tree_id = self
            .repo
            .find_object(ancestor_id)
            .map_err(tool)?
            .peel_to_commit()
            .map_err(tool)?
            .tree_id()
            .map_err(tool)?;
        let ancestor_index = self
            .repo
            .index_from_tree(&tree_id)
            .map_err(|err| VcsError::Tool(format!("index from tree: {err}")))?;
        // The real index tells committed additions apart from untracked
        // files (both are absent from the ancestor index).
        let tracked = self.repo.index_or_empty().map_err(tool)?;

        let platform = self
            .repo
            .status(gix::progress::Discard)
            .map_err(tool)?
            .index(gix::worktree::IndexPersistedOrInMemory::InMemory(
                ancestor_index,
            ))
            .untracked_files(UntrackedFiles::Files)
            .index_worktree_rewrites(gix::diff::Rewrites {
                copies: None,
                percentage: Some(0.5),
                limit: 1000,
                track_empty: false,
            });
        let iter = platform
            .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
            .map_err(tool)?;

        let mut files = Vec::new();
        for item in iter {
            let item = item.map_err(tool)?;
            let Some(summary) = item.summary() else {
                continue; // index-update bookkeeping, not a change
            };
            let path = PathBuf::from(item.rela_path().to_str_lossy().into_owned());
            let (status, old_path) = match (summary, &item) {
                (Summary::Renamed | Summary::Copied, Item::Rewrite { source, copy, .. }) => (
                    if *copy {
                        FileStatus::Copied
                    } else {
                        FileStatus::Renamed
                    },
                    Some(PathBuf::from(
                        source.rela_path().to_str_lossy().into_owned(),
                    )),
                ),
                (Summary::Removed, _) => (FileStatus::Deleted, None),
                // The ancestor-as-index makes committed additions and
                // untracked files look alike; the real index tells them
                // apart.
                (Summary::Added, _) => {
                    let status = if tracked.entry_by_path(item.rela_path()).is_some() {
                        FileStatus::Added
                    } else {
                        FileStatus::Untracked
                    };
                    (status, None)
                }
                (Summary::IntentToAdd, _) => (FileStatus::Added, None),
                _ => (FileStatus::Modified, None),
            };
            files.push(ChangedFile {
                status,
                path,
                old_path,
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    fn file_diff(&self, cmp: &Comparison, file: &ChangedFile) -> Result<FileDiff, VcsError> {
        let old = match file.status {
            FileStatus::Untracked => None,
            _ => {
                let old_path = file.old_path.as_deref().unwrap_or(&file.path);
                self.ancestor_blob(&cmp.ancestor, old_path)
            }
        };
        let new = std::fs::read(self.root.join(&file.path)).ok();
        if is_binary(old.as_deref()) || is_binary(new.as_deref()) {
            return Ok(FileDiff::Binary);
        }
        let old_text = old
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let new_text = new
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();

        Ok(compute_file_diff(&old_text, &new_text))
    }

    fn file_at_ancestor(&self, cmp: &Comparison, file: &ChangedFile) -> Option<String> {
        if file.status == FileStatus::Untracked {
            return None;
        }
        let old_path = file.old_path.as_deref().unwrap_or(&file.path);
        let blob = self.ancestor_blob(&cmp.ancestor, old_path)?;
        Some(String::from_utf8_lossy(&blob).into_owned())
    }

    fn branches(&self) -> Result<Vec<String>, VcsError> {
        let platform = self.repo.references().map_err(tool)?;
        let mut branches: Vec<(String, i64)> = Vec::new();
        for prefix in ["refs/heads/", "refs/remotes/"] {
            let iter = platform.prefixed(prefix).map_err(tool)?;
            for reference in iter.flatten() {
                // Symbolic refs (origin/HEAD) aren't real branches.
                if matches!(reference.target(), gix::refs::TargetRef::Symbolic(_)) {
                    continue;
                }
                let name = reference.name().shorten().to_str_lossy().into_owned();
                if branches.iter().any(|(b, _)| *b == name) {
                    continue;
                }
                let time = reference
                    .id()
                    .object()
                    .ok()
                    .and_then(|o| o.peel_to_commit().ok())
                    .and_then(|c| c.time().ok())
                    .map_or(0, |t| t.seconds);
                branches.push((name, time));
            }
        }
        branches.sort_by_key(|(_, time)| -time);
        Ok(branches.into_iter().map(|(name, _)| name).collect())
    }
}

fn tool(err: impl std::fmt::Display) -> VcsError {
    VcsError::Tool(err.to_string())
}

/// Context lines around each change, matching git's default.
const CONTEXT: u32 = 3;

/// Structured hunks straight from imara-diff's line ranges — no unified
/// text round-trip (imara 0.2's text writer emits hunk headers that
/// disagree with its own content when leading context is present).
fn compute_file_diff(old_text: &str, new_text: &str) -> FileDiff {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let input = InternedInput::new(old_text, new_text);
    let diff = Diff::compute(Algorithm::Histogram, &input);

    // Group changes whose context would overlap, like git does.
    let mut groups: Vec<Vec<imara_diff::Hunk>> = Vec::new();
    for hunk in diff.hunks() {
        match groups.last_mut() {
            Some(group)
                if hunk
                    .before
                    .start
                    .saturating_sub(group.last().unwrap().before.end)
                    <= 2 * CONTEXT =>
            {
                group.push(hunk);
            }
            _ => groups.push(vec![hunk]),
        }
    }

    let hunks = groups
        .into_iter()
        .map(|group| build_hunk(&group, &old_lines, &new_lines))
        .collect();
    FileDiff::Text { hunks }
}

/// One model hunk from a group of change segments plus shared context.
/// imara ranges are 0-based exclusive line indices.
fn build_hunk(group: &[imara_diff::Hunk], old_lines: &[&str], new_lines: &[&str]) -> Hunk {
    let first = group.first().expect("groups are non-empty");
    let last = group.last().expect("groups are non-empty");
    let lead = first.before.start.min(first.after.start).min(CONTEXT);
    let trail = CONTEXT
        .min(old_lines.len() as u32 - last.before.end)
        .min(new_lines.len() as u32 - last.after.end);
    let (old_from, old_to) = (first.before.start - lead, last.before.end + trail);
    let (new_from, new_to) = (first.after.start - lead, last.after.end + trail);

    let mut lines = Vec::new();
    let mut old_at = old_from;
    let mut new_at = new_from;
    let mut segments = group.iter().peekable();
    while old_at < old_to || new_at < new_to {
        if let Some(segment) = segments.peek()
            && old_at == segment.before.start
            && new_at == segment.after.start
        {
            for index in segment.before.clone() {
                lines.push(DiffLine {
                    kind: LineKind::Removed,
                    old_lineno: Some(index + 1),
                    new_lineno: None,
                    content: old_lines[index as usize].to_string(),
                });
            }
            for index in segment.after.clone() {
                lines.push(DiffLine {
                    kind: LineKind::Added,
                    old_lineno: None,
                    new_lineno: Some(index + 1),
                    content: new_lines[index as usize].to_string(),
                });
            }
            old_at = segment.before.end;
            new_at = segment.after.end;
            segments.next();
            continue;
        }
        lines.push(DiffLine {
            kind: LineKind::Context,
            old_lineno: Some(old_at + 1),
            new_lineno: Some(new_at + 1),
            content: new_lines[new_at as usize].to_string(),
        });
        old_at += 1;
        new_at += 1;
    }

    // Git's header convention: a zero-count side starts at the line
    // *before* the gap (0-based index doubles as that).
    let range = |from: u32, to: u32| {
        let count = to - from;
        (if count == 0 { from } else { from + 1 }, count)
    };
    Hunk {
        old_range: range(old_from, old_to),
        new_range: range(new_from, new_to),
        header: String::new(),
        lines,
    }
}

/// Git's heuristic: a NUL byte in the first 8000 bytes means binary.
fn is_binary(content: Option<&[u8]>) -> bool {
    content.is_some_and(|bytes| bytes[..bytes.len().min(8000)].contains(&0))
}
