//! Sidebar file-tree state: builds a VS Code-style tree from the flat
//! changed-file list — directories first, single-child directory chains
//! compacted (`src/vcs/git`), directories collapsible (expanded by default).
//! Virtual entries — `#`-prefixed root paths like the PR conversation —
//! pin above everything: they're about the whole changeset, not a file.
//! The grouped layout puts a header per review group on top, each over
//! its own files' directory tree.

use std::collections::BTreeMap;

use crate::group::{Group, Kind};
use crate::vcs::model::{ChangedFile, FileStatus};

/// Root paths starting with this are virtual entries (about the whole
/// changeset, not a file) and pin above everything — the contract the
/// PR session's conversation entry relies on.
pub const VIRTUAL_PREFIX: char = '#';

/// How the tree arranges the changed files: under review-group headers
/// (code, tests, docs…), or as plain directories. `f` flips between
/// them; `[tree] layout` picks the one drift starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Groups,
    Directories,
}

impl Layout {
    /// The layout `f` flips to.
    pub fn other(self) -> Layout {
        match self {
            Layout::Groups => Layout::Directories,
            Layout::Directories => Layout::Groups,
        }
    }

    /// The `[tree] layout` config value naming this layout.
    pub fn name(self) -> &'static str {
        match self {
            Layout::Groups => "groups",
            Layout::Directories => "directories",
        }
    }

    /// Parse the config value; `None` for anything else.
    pub fn parse(name: &str) -> Option<Layout> {
        [Layout::Groups, Layout::Directories]
            .into_iter()
            .find(|layout| layout.name() == name)
    }
}

pub struct FileTree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// Node ids in display order, honoring collapsed directories.
    visible: Vec<usize>,
}

pub struct Node {
    pub label: String,
    /// Full slash-joined path from the repo root — the node's stable
    /// identity across rebuilds (labels repeat, compacted chains don't).
    pub path: String,
    pub depth: usize,
    pub kind: NodeKind,
}

pub enum NodeKind {
    Dir {
        children: Vec<usize>,
        expanded: bool,
    },
    /// A review group's header; collapses like a directory.
    Group {
        children: Vec<usize>,
        expanded: bool,
        /// Indices into the changed-files list, for its progress.
        files: Vec<usize>,
        kind: Kind,
    },
    /// The blank row above a group header, setting the groups apart.
    /// Never a cursor stop.
    Gap,
    /// Index into the changed-files list.
    File { index: usize, status: FileStatus },
}

#[derive(Default)]
struct TmpDir {
    dirs: BTreeMap<String, TmpDir>,
    files: Vec<(String, usize, FileStatus)>,
}

impl TmpDir {
    fn insert(&mut self, index: usize, file: &ChangedFile) {
        let components: Vec<String> = file
            .path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some((name, dirs)) = components.split_last() else {
            return;
        };
        let mut cursor = self;
        for dir in dirs {
            cursor = cursor.dirs.entry(dir.clone()).or_default();
        }
        cursor.files.push((name.clone(), index, file.status));
    }
}

impl FileTree {
    pub fn build(files: &[ChangedFile]) -> FileTree {
        let mut root = TmpDir::default();
        for (index, file) in files.iter().enumerate() {
            root.insert(index, file);
        }
        let mut tree = FileTree::empty();
        tree.pin_virtuals(&mut root);
        let rest = tree.convert(root, 0, "", "");
        tree.roots.extend(rest);
        tree.recompute_visible();
        tree
    }

    /// The grouped layout: virtual entries pinned on top as always, then
    /// each group's header over the directory tree of its files. Paths
    /// of files stay their own — the cursor finds its file across a
    /// layout switch — while a group's directories get scoped paths, so
    /// `src` under one group collapses apart from `src` under another.
    /// A single group would only head the plain tree, so it doesn't.
    pub fn build_grouped(files: &[ChangedFile], groups: &[Group]) -> FileTree {
        if groups.len() < 2 {
            return FileTree::build(files);
        }
        let mut root = TmpDir::default();
        for (index, file) in files.iter().enumerate() {
            root.insert(index, file);
        }
        let mut tree = FileTree::empty();
        tree.pin_virtuals(&mut root);
        for group in groups {
            if !tree.roots.is_empty() {
                tree.nodes.push(Node {
                    label: String::new(),
                    path: format!("{VIRTUAL_PREFIX}gap:{}", group.label),
                    depth: 0,
                    kind: NodeKind::Gap,
                });
                tree.roots.push(tree.nodes.len() - 1);
            }
            let mut sub = TmpDir::default();
            for &index in &group.files {
                sub.insert(index, &files[index]);
            }
            let path = format!("{VIRTUAL_PREFIX}group:{}", group.label);
            let id = tree.nodes.len();
            tree.nodes.push(Node {
                label: group.label.clone(),
                path: path.clone(),
                depth: 0,
                kind: NodeKind::Group {
                    children: Vec::new(),
                    expanded: true,
                    files: group.files.clone(),
                    kind: group.kind,
                },
            });
            let children = tree.convert(sub, 1, "", &format!("{path}/"));
            let NodeKind::Group { children: slot, .. } = &mut tree.nodes[id].kind else {
                unreachable!();
            };
            *slot = children;
            tree.roots.push(id);
        }
        tree.recompute_visible();
        tree
    }

    fn empty() -> FileTree {
        FileTree {
            nodes: Vec::new(),
            roots: Vec::new(),
            visible: Vec::new(),
        }
    }

    /// Take the virtual entries out of the root and pin them first,
    /// above every group, directory and file.
    fn pin_virtuals(&mut self, root: &mut TmpDir) {
        let (virtuals, plain): (Vec<_>, Vec<_>) = std::mem::take(&mut root.files)
            .into_iter()
            .partition(|(name, ..)| name.starts_with(VIRTUAL_PREFIX));
        root.files = plain;
        for (label, index, status) in virtuals {
            let id = self.nodes.len();
            self.nodes.push(Node {
                path: label.clone(),
                label,
                depth: 0,
                kind: NodeKind::File { index, status },
            });
            self.roots.push(id);
        }
    }

    /// Convert a temp dir's children to nodes (dirs first, both sorted);
    /// returns their ids. `scope` prefixes directory paths only (see
    /// [`FileTree::build_grouped`]).
    fn convert(&mut self, dir: TmpDir, depth: usize, prefix: &str, scope: &str) -> Vec<usize> {
        let join = |label: &str| {
            if prefix.is_empty() {
                label.to_string()
            } else {
                format!("{prefix}/{label}")
            }
        };
        let mut ids = Vec::new();
        for (mut label, mut sub) in dir.dirs {
            // Compact chains of single-child directories: src/vcs/git.
            while sub.files.is_empty() && sub.dirs.len() == 1 {
                let (next_label, next) = sub.dirs.pop_first().expect("len checked");
                label = format!("{label}/{next_label}");
                sub = next;
            }
            let path = join(&label);
            let id = self.nodes.len();
            self.nodes.push(Node {
                label,
                path: format!("{scope}{path}"),
                depth,
                kind: NodeKind::Dir {
                    children: Vec::new(),
                    expanded: true,
                },
            });
            let children = self.convert(sub, depth + 1, &path, scope);
            let NodeKind::Dir { children: slot, .. } = &mut self.nodes[id].kind else {
                unreachable!();
            };
            *slot = children;
            ids.push(id);
        }
        let mut files = dir.files;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (label, index, status) in files {
            let path = join(&label);
            let id = self.nodes.len();
            self.nodes.push(Node {
                label,
                path,
                depth,
                kind: NodeKind::File { index, status },
            });
            ids.push(id);
        }
        ids
    }

    fn recompute_visible(&mut self) {
        fn walk(nodes: &[Node], ids: &[usize], out: &mut Vec<usize>) {
            for &id in ids {
                out.push(id);
                if let NodeKind::Dir {
                    children,
                    expanded: true,
                }
                | NodeKind::Group {
                    children,
                    expanded: true,
                    ..
                } = &nodes[id].kind
                {
                    walk(nodes, children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &self.roots, &mut out);
        self.visible = out;
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn rows(&self) -> impl Iterator<Item = &Node> {
        self.visible.iter().map(|&id| &self.nodes[id])
    }

    /// The node at visible row `i`.
    pub fn row(&self, i: usize) -> Option<&Node> {
        self.visible.get(i).map(|&id| &self.nodes[id])
    }

    /// Changed-files index of the file at visible row `i`, if it's a file.
    pub fn file_at(&self, i: usize) -> Option<usize> {
        match self.row(i)?.kind {
            NodeKind::File { index, .. } => Some(index),
            NodeKind::Dir { .. } | NodeKind::Group { .. } | NodeKind::Gap => None,
        }
    }

    /// Whether visible row `i` is a [`NodeKind::Gap`].
    pub fn is_gap(&self, i: usize) -> bool {
        self.row(i)
            .is_some_and(|node| matches!(node.kind, NodeKind::Gap))
    }

    pub fn first_file_row(&self) -> Option<usize> {
        (0..self.visible.len()).find(|&i| self.file_at(i).is_some())
    }

    /// Set the expansion of the directory at visible row `i`; returns
    /// whether anything changed (false for files or no-op states).
    pub fn set_expanded(&mut self, i: usize, want: bool) -> bool {
        let Some(&id) = self.visible.get(i) else {
            return false;
        };
        if let NodeKind::Dir { expanded, .. } | NodeKind::Group { expanded, .. } =
            &mut self.nodes[id].kind
            && *expanded != want
        {
            *expanded = want;
            self.recompute_visible();
            return true;
        }
        false
    }

    /// Full path of the node at visible row `i`.
    pub fn row_path(&self, i: usize) -> Option<&str> {
        self.row(i).map(|node| node.path.as_str())
    }

    /// Visible row of the node with this path, if it's currently shown.
    pub fn row_of_path(&self, path: &str) -> Option<usize> {
        (0..self.visible.len()).find(|&i| self.row(i).is_some_and(|n| n.path == path))
    }

    /// Paths of all collapsed directories, for carrying the expansion
    /// state across a rebuild.
    pub fn collapsed_paths(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    NodeKind::Dir {
                        expanded: false,
                        ..
                    } | NodeKind::Group {
                        expanded: false,
                        ..
                    }
                )
            })
            .map(|node| node.path.clone())
            .collect()
    }

    /// Collapse every directory whose path is in `paths` (the counterpart
    /// of [`FileTree::collapsed_paths`] on the new tree).
    pub fn collapse_paths(&mut self, paths: &[String]) {
        let mut changed = false;
        for node in &mut self.nodes {
            if let NodeKind::Dir { expanded, .. } | NodeKind::Group { expanded, .. } =
                &mut node.kind
                && *expanded
                && paths.contains(&node.path)
            {
                *expanded = false;
                changed = true;
            }
        }
        if changed {
            self.recompute_visible();
        }
    }

    /// Expand/collapse the directory at visible row `i`.
    /// Returns false when the row is a file.
    pub fn toggle(&mut self, i: usize) -> bool {
        let Some(&id) = self.visible.get(i) else {
            return false;
        };
        if let NodeKind::Dir { expanded, .. } | NodeKind::Group { expanded, .. } =
            &mut self.nodes[id].kind
        {
            *expanded = !*expanded;
            self.recompute_visible();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            status: FileStatus::Modified,
            path: PathBuf::from(path),
            old_path: None,
        }
    }

    fn labels(tree: &FileTree) -> Vec<(usize, String)> {
        tree.rows().map(|n| (n.depth, n.label.clone())).collect()
    }

    #[test]
    fn dirs_first_then_files_sorted() {
        let tree = FileTree::build(&[
            changed("zzz.txt"),
            changed("src/main.rs"),
            changed("src/app.rs"),
            changed("assets/logo.png"),
        ]);
        assert_eq!(
            labels(&tree),
            vec![
                (0, "assets".into()),
                (1, "logo.png".into()),
                (0, "src".into()),
                (1, "app.rs".into()),
                (1, "main.rs".into()),
                (0, "zzz.txt".into()),
            ]
        );
    }

    #[test]
    fn virtual_entries_pin_above_directories_and_files() {
        let tree = FileTree::build(&[
            changed("zzz.txt"),
            changed("src/main.rs"),
            changed("#conversation"),
        ]);
        assert_eq!(
            labels(&tree),
            vec![
                (0, "#conversation".into()),
                (0, "src".into()),
                (1, "main.rs".into()),
                (0, "zzz.txt".into()),
            ]
        );
        // It maps back to its changed-files index like any file row.
        assert_eq!(tree.file_at(0), Some(2));
        assert_eq!(tree.first_file_row(), Some(0));
    }

    #[test]
    fn single_child_dir_chains_compact() {
        let tree = FileTree::build(&[
            changed("src/vcs/git/mod.rs"),
            changed("src/vcs/git/cli.rs"),
            changed("src/main.rs"),
        ]);
        assert_eq!(
            labels(&tree),
            vec![
                (0, "src".into()),
                (1, "vcs/git".into()),
                (2, "cli.rs".into()),
                (2, "mod.rs".into()),
                (1, "main.rs".into()),
            ]
        );
    }

    #[test]
    fn toggle_collapses_descendants() {
        let mut tree = FileTree::build(&[changed("src/a.rs"), changed("src/b.rs")]);
        assert_eq!(tree.visible_len(), 3);
        assert!(tree.toggle(0));
        assert_eq!(tree.visible_len(), 1);
        assert!(tree.toggle(0));
        assert_eq!(tree.visible_len(), 3);
    }

    #[test]
    fn toggle_on_file_is_noop() {
        let mut tree = FileTree::build(&[changed("a.rs")]);
        assert!(!tree.toggle(0));
        assert_eq!(tree.visible_len(), 1);
    }

    #[test]
    fn row_paths_are_full_and_stable() {
        let tree = FileTree::build(&[changed("src/vcs/git/mod.rs"), changed("src/main.rs")]);
        // Rows: src, vcs/git (compacted), mod.rs, main.rs
        assert_eq!(tree.row_path(0), Some("src"));
        assert_eq!(tree.row_path(1), Some("src/vcs/git"));
        assert_eq!(tree.row_path(2), Some("src/vcs/git/mod.rs"));
        assert_eq!(tree.row_path(3), Some("src/main.rs"));
        assert_eq!(tree.row_of_path("src/main.rs"), Some(3));
    }

    #[test]
    fn collapsed_state_round_trips_across_rebuild() {
        let mut tree = FileTree::build(&[changed("src/a.rs"), changed("docs/b.md")]);
        // Collapse "src" (docs, b.md, src, a.rs — src is row 2).
        assert_eq!(tree.row_path(2), Some("src"));
        assert!(tree.toggle(2));
        let collapsed = tree.collapsed_paths();
        assert_eq!(collapsed, vec!["src".to_string()]);

        let mut rebuilt = FileTree::build(&[
            changed("src/a.rs"),
            changed("src/c.rs"),
            changed("docs/b.md"),
        ]);
        rebuilt.collapse_paths(&collapsed);
        // src stays collapsed: docs, b.md, src.
        assert_eq!(rebuilt.visible_len(), 3);
        assert_eq!(rebuilt.row_of_path("src/a.rs"), None);
    }

    fn group(label: &str, files: &[usize]) -> Group {
        Group {
            kind: Kind::ALL
                .into_iter()
                .find(|kind| kind.label() == label)
                .unwrap_or(Kind::Code),
            label: label.to_string(),
            files: files.to_vec(),
        }
    }

    #[test]
    fn grouped_layout_heads_each_group_over_its_own_tree() {
        let files = [
            changed("#conversation"),
            changed("src/app.rs"),
            changed("src/app_test.rs"),
            changed("tests/cli.rs"),
        ];
        let tree = FileTree::build_grouped(&files, &[group("code", &[1]), group("tests", &[2, 3])]);
        assert_eq!(
            labels(&tree),
            vec![
                (0, "#conversation".into()),
                (0, "".into()),
                (0, "code".into()),
                (1, "src".into()),
                (2, "app.rs".into()),
                (0, "".into()),
                (0, "tests".into()),
                (1, "src".into()),
                (2, "app_test.rs".into()),
                (1, "tests".into()),
                (2, "cli.rs".into()),
            ]
        );
        // A gap sets each group apart from whatever comes before it.
        assert!(tree.is_gap(1) && tree.is_gap(5));
        assert!(!tree.is_gap(0) && !tree.is_gap(2));
        // Files keep their own paths; a group's directories are scoped.
        assert_eq!(tree.row_of_path("src/app_test.rs"), Some(8));
        assert_eq!(tree.row_path(3), Some("#group:code/src"));
        assert_eq!(tree.row_path(7), Some("#group:tests/src"));
        assert_eq!(tree.file_at(2), None);
    }

    #[test]
    fn a_lone_group_is_just_the_plain_tree() {
        let files = [changed("src/a.rs"), changed("src/b.rs")];
        let tree = FileTree::build_grouped(&files, &[group("code", &[0, 1])]);
        assert_eq!(
            labels(&tree),
            vec![(0, "src".into()), (1, "a.rs".into()), (1, "b.rs".into())]
        );
    }

    #[test]
    fn a_group_collapses_like_a_directory_and_stays_so_across_rebuild() {
        let files = [changed("src/a.rs"), changed("README.md")];
        let groups = [group("code", &[0]), group("docs", &[1])];
        let mut tree = FileTree::build_grouped(&files, &groups);
        assert!(tree.toggle(0));
        assert_eq!(
            labels(&tree),
            vec![
                (0, "code".into()),
                (0, "".into()),
                (0, "docs".into()),
                (1, "README.md".into())
            ]
        );

        let mut rebuilt = FileTree::build_grouped(&files, &groups);
        rebuilt.collapse_paths(&tree.collapsed_paths());
        assert_eq!(rebuilt.visible_len(), 4);
    }

    #[test]
    fn file_at_maps_to_original_indices() {
        let tree = FileTree::build(&[changed("src/b.rs"), changed("a.rs")]);
        // Rows: src, src/b.rs, a.rs — original indices 0 and 1.
        assert_eq!(tree.file_at(0), None);
        assert_eq!(tree.file_at(1), Some(0));
        assert_eq!(tree.file_at(2), Some(1));
        assert_eq!(tree.first_file_row(), Some(1));
    }
}
