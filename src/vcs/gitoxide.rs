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
    ChangedFile, CommitInfo, Comparison, DiffLine, FileDiff, FileStatus, Hunk, LineKind,
    RevisionId, Scope,
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

    fn blob_at(&self, rev: &RevisionId, path: &Path) -> Option<Vec<u8>> {
        let id = gix::ObjectId::from_hex(rev.0.as_bytes()).ok()?;
        let commit = self.repo.find_object(id).ok()?.peel_to_commit().ok()?;
        let entry = commit.tree().ok()?.lookup_entry_by_path(path).ok()??;
        Some(entry.object().ok()?.detach().data)
    }

    fn find_commit(&self, rev: &RevisionId) -> Result<gix::Commit<'_>, VcsError> {
        let id = gix::ObjectId::from_hex(rev.0.as_bytes())
            .map_err(|_| VcsError::RevisionNotFound(rev.0.clone()))?;
        self.repo
            .find_object(id)
            .map_err(|_| VcsError::RevisionNotFound(rev.0.clone()))?
            .peel_to_commit()
            .map_err(|_| VcsError::RevisionNotFound(rev.0.clone()))
    }

    fn first_parent(&self, rev: &RevisionId) -> Option<RevisionId> {
        let parent = self.find_commit(rev).ok()?.parent_ids().next()?;
        Some(RevisionId(parent.detach().to_string()))
    }

    /// The file's content on the old side of the scoped comparison: the
    /// ancestor, or the commit's first parent under a commit scope.
    fn old_side(&self, cmp: &Comparison, file: &ChangedFile) -> Option<Vec<u8>> {
        if file.status == FileStatus::Untracked {
            return None;
        }
        let old_path = file.old_path.as_deref().unwrap_or(&file.path);
        match &cmp.scope {
            Scope::Commit(rev) => self.blob_at(&self.first_parent(rev)?, old_path),
            _ => self.blob_at(&cmp.ancestor, old_path),
        }
    }

    /// The files a single commit changed, against its first parent (the
    /// empty tree for a root commit).
    fn commit_changed_files(&self, rev: &RevisionId) -> Result<Vec<ChangedFile>, VcsError> {
        use gix::object::tree::diff::{Action, Change};

        let commit = self.find_commit(rev)?;
        let new_tree = commit.tree().map_err(tool)?;
        let old_tree = match commit.parent_ids().next() {
            Some(parent) => parent
                .object()
                .map_err(tool)?
                .peel_to_commit()
                .map_err(tool)?
                .tree()
                .map_err(tool)?,
            None => self.repo.empty_tree(),
        };

        let mut files = Vec::new();
        old_tree
            .changes()
            .map_err(tool)?
            .options(|opts| {
                opts.track_rewrites(Some(gix::diff::Rewrites {
                    copies: None,
                    percentage: Some(0.5),
                    limit: 1000,
                    track_empty: false,
                }));
            })
            .for_each_to_obtain_tree(&new_tree, |change| {
                let file = match change {
                    Change::Addition {
                        location,
                        entry_mode,
                        ..
                    } => entry_mode.is_blob().then(|| ChangedFile {
                        status: FileStatus::Added,
                        path: PathBuf::from(location.to_str_lossy().into_owned()),
                        old_path: None,
                    }),
                    Change::Deletion {
                        location,
                        entry_mode,
                        ..
                    } => entry_mode.is_blob().then(|| ChangedFile {
                        status: FileStatus::Deleted,
                        path: PathBuf::from(location.to_str_lossy().into_owned()),
                        old_path: None,
                    }),
                    Change::Modification {
                        location,
                        entry_mode,
                        ..
                    } => entry_mode.is_blob().then(|| ChangedFile {
                        status: FileStatus::Modified,
                        path: PathBuf::from(location.to_str_lossy().into_owned()),
                        old_path: None,
                    }),
                    Change::Rewrite {
                        location,
                        source_location,
                        entry_mode,
                        copy,
                        ..
                    } => entry_mode.is_blob().then(|| ChangedFile {
                        status: if copy {
                            FileStatus::Copied
                        } else {
                            FileStatus::Renamed
                        },
                        path: PathBuf::from(location.to_str_lossy().into_owned()),
                        old_path: Some(PathBuf::from(source_location.to_str_lossy().into_owned())),
                    }),
                };
                files.extend(file);
                Ok::<_, std::convert::Infallible>(Action::Continue(()))
            })
            .map_err(tool)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
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
            scope: Scope::default(),
        })
    }

    fn changed_files(&self, cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError> {
        if let Scope::Commit(rev) = &cmp.scope {
            return self.commit_changed_files(rev);
        }
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
        if cmp.scope == Scope::Untracked {
            files.retain(|file| file.status == FileStatus::Untracked);
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    fn file_diff(&self, cmp: &Comparison, file: &ChangedFile) -> Result<FileDiff, VcsError> {
        let old = self.old_side(cmp, file);
        let new = match &cmp.scope {
            // A commit's new side is its own tree, not the working copy.
            Scope::Commit(rev) => self.blob_at(rev, &file.path),
            _ => std::fs::read(self.root.join(&file.path)).ok(),
        };
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
        let blob = self.old_side(cmp, file)?;
        Some(String::from_utf8_lossy(&blob).into_owned())
    }

    fn file_at_revision(&self, rev: &RevisionId, path: &Path) -> Option<String> {
        let blob = self.blob_at(rev, path)?;
        Some(String::from_utf8_lossy(&blob).into_owned())
    }

    fn unignored(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let Ok(index) = self.repo.index_or_empty() else {
            return paths;
        };
        let Ok(mut stack) = self.repo.excludes(
            &index,
            None,
            gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
        ) else {
            return paths;
        };
        paths
            .into_iter()
            .filter(|path| match stack.at_path(path, None) {
                Ok(platform) => !platform.is_excluded(),
                Err(_) => true,
            })
            .collect()
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

    fn commits(&self, cmp: &Comparison) -> Result<Vec<CommitInfo>, VcsError> {
        let head = self
            .repo
            .head_id()
            .map_err(|_| VcsError::RevisionNotFound("HEAD".to_string()))?;
        let ancestor = gix::ObjectId::from_hex(cmp.ancestor.0.as_bytes())
            .map_err(|err| VcsError::Tool(format!("bad ancestor id: {err}")))?;
        let walk = self
            .repo
            .rev_walk([head.detach()])
            .with_hidden([ancestor])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                Default::default(),
            ))
            .all()
            .map_err(tool)?;

        let mut commits = Vec::new();
        for info in walk {
            let info = info.map_err(tool)?;
            let commit = info.object().map_err(tool)?;
            let summary = commit
                .message()
                .map(|message| message.summary().to_str_lossy().into_owned())
                .unwrap_or_default();
            commits.push(CommitInfo {
                id: RevisionId(info.id.to_string()),
                short_id: commit.id().shorten_or_id().to_string(),
                summary,
            });
        }
        Ok(commits)
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
