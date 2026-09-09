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
    BranchInfo, ChangedFile, CommitInfo, Comparison, DiffLine, FileDiff, FileStatus, Hunk,
    LineKind, RevisionId, Scope, WorktreeInfo,
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

    /// The short name of the remote-tracking ref for the checked-out
    /// branch (e.g. `origin/feature`), if the branch has an upstream.
    fn upstream_of_head(&self) -> Option<String> {
        let head = self.repo.head_ref().ok()??;
        let name = head
            .remote_tracking_ref_name(gix::remote::Direction::Fetch)?
            .ok()?;
        let short = name.as_bstr().strip_prefix(b"refs/remotes/")?;
        Some(short.to_str_lossy().into_owned())
    }

    fn merge_base_with_head(
        &self,
        base: &str,
        head: gix::ObjectId,
    ) -> Result<gix::ObjectId, VcsError> {
        let base_id = self
            .rev_commit_id(base)
            .ok_or_else(|| VcsError::RevisionNotFound(base.to_string()))?;
        self.repo
            .merge_base(base_id, head)
            .map(|id| id.detach())
            .map_err(|_| VcsError::NoCommonAncestor {
                base: base.to_string(),
                work: "HEAD".to_string(),
            })
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

    /// HEAD as a revision id — the old side of the uncommitted scope.
    fn head_rev(&self) -> Result<RevisionId, VcsError> {
        let head = self
            .repo
            .head_id()
            .map_err(|_| VcsError::RevisionNotFound("HEAD".to_string()))?;
        Ok(RevisionId(head.detach().to_string()))
    }

    /// The tip of the work side: the branch being reviewed off the
    /// board, or this worktree's HEAD when the review is the live one.
    fn work_rev(&self, cmp: &Comparison) -> Result<RevisionId, VcsError> {
        match &cmp.work {
            Some(rev) => Ok(rev.clone()),
            None => self.head_rev(),
        }
    }

    /// The file's content on the old side of the scoped comparison: the
    /// ancestor, HEAD under the uncommitted scope, or the commit's first
    /// parent under a commit scope.
    fn old_side(&self, cmp: &Comparison, file: &ChangedFile) -> Option<Vec<u8>> {
        if file.status == FileStatus::Untracked {
            return None;
        }
        let old_path = file.old_path.as_deref().unwrap_or(&file.path);
        match &cmp.scope {
            Scope::Commit(rev) => self.blob_at(&self.first_parent(rev)?, old_path),
            Scope::Uncommitted => self.blob_at(&self.work_rev(cmp).ok()?, old_path),
            _ => self.blob_at(&cmp.ancestor, old_path),
        }
    }

    /// The files a single commit changed, against its first parent (the
    /// empty tree for a root commit).
    fn commit_changed_files(&self, rev: &RevisionId) -> Result<Vec<ChangedFile>, VcsError> {
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
        self.tree_changed_files(old_tree, new_tree)
    }

    /// A revision's tree.
    fn tree_of(&self, rev: &RevisionId) -> Result<gix::Tree<'_>, VcsError> {
        self.find_commit(rev)?.tree().map_err(tool)
    }

    /// The files differing between two trees — commit and committed
    /// scopes, where the working copy plays no part.
    fn tree_changed_files(
        &self,
        old_tree: gix::Tree<'_>,
        new_tree: gix::Tree<'_>,
    ) -> Result<Vec<ChangedFile>, VcsError> {
        use gix::object::tree::diff::{Action, Change};

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
            .map_err(|_| VcsError::RevisionNotFound("HEAD".to_string()))?
            .detach();
        let (base, ancestor) = match self.merge_base_with_head(&base, head) {
            Ok(ancestor) => (base, ancestor),
            // An orphan branch shares nothing with the detected default
            // branch; its own upstream is the only meaningful base left.
            // An explicit base is the user's choice, so it still errors.
            Err(err @ VcsError::NoCommonAncestor { .. }) if base_override.is_none() => {
                let fallback = self
                    .upstream_of_head()
                    .filter(|upstream| *upstream != base)
                    .and_then(|upstream| {
                        let ancestor = self.merge_base_with_head(&upstream, head).ok()?;
                        Some((upstream, ancestor))
                    });
                fallback.ok_or(err)?
            }
            Err(err) => return Err(err),
        };
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
            work: None,
        })
    }

    fn comparison_at(
        &self,
        base_override: Option<&str>,
        work: &str,
    ) -> Result<Comparison, VcsError> {
        let base = match base_override {
            Some(base) => base.to_string(),
            None => self.default_base()?,
        };
        let base_id = self
            .rev_commit_id(&base)
            .ok_or_else(|| VcsError::RevisionNotFound(base.clone()))?;
        let tip = self
            .rev_commit_id(work)
            .ok_or_else(|| VcsError::RevisionNotFound(work.to_string()))?;
        let ancestor =
            self.repo
                .merge_base(base_id, tip)
                .map_err(|_| VcsError::NoCommonAncestor {
                    base: base.clone(),
                    work: work.to_string(),
                })?;
        Ok(Comparison {
            base_label: base,
            ancestor: RevisionId(ancestor.to_string()),
            work_label: work.to_string(),
            // Committed, always: a branch has no working copy, and
            // every other scope would claim to show one.
            scope: Scope::Committed,
            work: Some(RevisionId(tip.to_string())),
        })
    }

    fn changed_files(&self, cmp: &Comparison) -> Result<Vec<ChangedFile>, VcsError> {
        match &cmp.scope {
            Scope::Commit(rev) => return self.commit_changed_files(rev),
            // Committed work is a pure tree diff — the working copy
            // plays no part.
            Scope::Committed => {
                let old = self.tree_of(&cmp.ancestor)?;
                let new = self.tree_of(&self.work_rev(cmp)?)?;
                return self.tree_changed_files(old, new);
            }
            // A branch reviewed off the board has no working copy: its
            // whole changeset is the tree diff to its tip, and nothing
            // about it is uncommitted.
            _ if cmp.work.is_some() => {
                if cmp.scope == Scope::Uncommitted {
                    return Ok(Vec::new());
                }
                let old = self.tree_of(&cmp.ancestor)?;
                let new = self.tree_of(&self.work_rev(cmp)?)?;
                return self.tree_changed_files(old, new);
            }
            _ => {}
        }
        // The uncommitted scope diffs the working copy against HEAD —
        // exactly the slice `git status` reports.
        let old_rev = match cmp.scope {
            Scope::Uncommitted => self.head_rev()?,
            _ => cmp.ancestor.clone(),
        };
        let ancestor_id = gix::ObjectId::from_hex(old_rev.0.as_bytes())
            .map_err(|err| VcsError::Tool(format!("bad ancestor id: {err}")))?;
        let tree_id = self
            .repo
            .find_object(ancestor_id)
            .map_err(tool)?
            .peel_to_commit()
            .map_err(tool)?
            .tree_id()
            .map_err(tool)?;
        let mut ancestor_index = self
            .repo
            .index_from_tree(&tree_id)
            .map_err(|err| VcsError::Tool(format!("index from tree: {err}")))?;
        // The real index tells committed additions apart from untracked
        // files (both are absent from the ancestor index).
        let tracked = self.repo.index_or_empty().map_err(tool)?;
        // An index synthesized from a tree has no stat data, so the
        // status walk would open and hash every tracked file to prove it
        // unchanged — the whole scan cost in large repos. Grafting the
        // real index's stat blocks onto entries with the same blob lets
        // those files pass the stat shortcut, exactly like `git status`.
        // (Same content at a matching stat ⇒ the worktree still holds
        // this blob; entries left zeroed just fall back to hashing.)
        let stats: Vec<(usize, gix::index::entry::Stat)> = ancestor_index
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                let real = tracked.entry_by_path(entry.path(&ancestor_index))?;
                (real.id == entry.id && real.mode == entry.mode).then_some((i, real.stat))
            })
            .collect();
        for (i, stat) in stats {
            ancestor_index.entries_mut()[i].stat = stat;
        }

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
        let old = self.old_side(cmp, file);
        let new = match &cmp.scope {
            // A commit's new side is its own tree, not the working copy;
            // committed work ends at HEAD.
            Scope::Commit(rev) => self.blob_at(rev, &file.path),
            Scope::Committed => self
                .work_rev(cmp)
                .ok()
                .and_then(|tip| self.blob_at(&tip, &file.path)),
            // The working copy is the new side only when the review is
            // this worktree's own; a branch's ends at its tip.
            _ => match &cmp.work {
                Some(tip) => self.blob_at(tip, &file.path),
                None => std::fs::read(self.root.join(&file.path)).ok(),
            },
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

    fn file_at_head(&self, path: &Path) -> Option<String> {
        self.file_at_revision(&self.head_rev().ok()?, path)
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

    fn worktrees(&self) -> Result<Vec<WorktreeInfo>, VcsError> {
        // gix lists linked worktrees only, so the main one is added by
        // hand — from the board's point of view it is just another row.
        let mut out = Vec::new();
        if let Ok(main) = self.repo.main_repo()
            && let Some(info) = worktree_info(&main)
        {
            out.push(info);
        }
        for proxy in self.repo.worktrees().map_err(tool)? {
            // A worktree whose directory was deleted without `git
            // worktree prune` still has its private git dir; skip it
            // rather than offering a row that cannot be opened.
            if let Ok(repo) = proxy.into_repo()
                && let Some(info) = worktree_info(&repo)
            {
                out.push(info);
            }
        }
        out.sort_by_key(|info| -info.last_commit);
        Ok(out)
    }

    fn work_tip(&self, cmp: &Comparison) -> Option<RevisionId> {
        self.work_rev(cmp).ok()
    }

    fn branch_tips(&self) -> Result<Vec<BranchInfo>, VcsError> {
        let platform = self.repo.references().map_err(tool)?;
        let mut branches: Vec<BranchInfo> = Vec::new();
        for prefix in ["refs/heads/", "refs/remotes/"] {
            let iter = platform.prefixed(prefix).map_err(tool)?;
            for reference in iter.flatten() {
                // Symbolic refs (origin/HEAD) aren't real branches.
                if matches!(reference.target(), gix::refs::TargetRef::Symbolic(_)) {
                    continue;
                }
                let name = reference.name().shorten().to_str_lossy().into_owned();
                if branches.iter().any(|branch| branch.name == name) {
                    continue;
                }
                let Some(commit) = reference
                    .id()
                    .object()
                    .ok()
                    .and_then(|object| object.peel_to_commit().ok())
                else {
                    continue;
                };
                branches.push(BranchInfo {
                    name,
                    tip: RevisionId(commit.id.to_string()),
                    last_commit: commit.time().map_or(0, |time| time.seconds),
                });
            }
        }
        branches.sort_by_key(|branch| -branch.last_commit);
        Ok(branches)
    }

    fn commits(&self, cmp: &Comparison) -> Result<Vec<CommitInfo>, VcsError> {
        let tip = gix::ObjectId::from_hex(self.work_rev(cmp)?.0.as_bytes())
            .map_err(|err| VcsError::Tool(format!("bad work id: {err}")))?;
        let ancestor = gix::ObjectId::from_hex(cmp.ancestor.0.as_bytes())
            .map_err(|err| VcsError::Tool(format!("bad ancestor id: {err}")))?;
        // On the base itself (merge-base == tip) hiding the ancestor
        // would hide everything; offer recent history instead, capped so
        // a long-lived repo doesn't stall the picker.
        let on_base = tip == ancestor;
        let mut walk =
            self.repo
                .rev_walk([tip])
                .sorting(gix::revision::walk::Sorting::ByCommitTime(
                    Default::default(),
                ));
        if !on_base {
            walk = walk.with_hidden([ancestor]);
        }
        let walk = walk.all().map_err(tool)?;
        let limit = if on_base { RECENT_COMMITS } else { usize::MAX };

        let mut commits = Vec::new();
        for info in walk.take(limit) {
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

/// How much history the scope picker offers when the work side is the
/// base itself and there are no branch commits to list.
const RECENT_COMMITS: usize = 100;

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

/// Board row for one already-opened worktree: name, branch, tip time.
/// `None` for a bare or worktree-less repository.
fn worktree_info(repo: &gix::Repository) -> Option<WorktreeInfo> {
    // Canonical, because the main worktree's workdir is however drift
    // was invoked ("." when launched in place) — the board needs a real
    // name to show and a real path to match agent directories against.
    let path = repo.workdir()?.canonicalize().ok()?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .and_then(|head| head.referent_name())
        .map(|name| name.shorten().to_str_lossy().into_owned());
    // An unborn branch has no tip: it sorts last rather than dropping
    // the worktree off the board.
    let last_commit = repo
        .head_id()
        .ok()
        .and_then(|id| id.object().ok())
        .and_then(|object| object.peel_to_commit().ok())
        .and_then(|commit| commit.time().ok())
        .map_or(0, |time| time.seconds);
    Some(WorktreeInfo {
        name,
        path,
        branch,
        last_commit,
    })
}
