//! Sidebar file-tree state: builds a VS Code-style tree from the flat
//! changed-file list — directories first, single-child directory chains
//! compacted (`src/vcs/git`), directories collapsible (expanded by default).

use std::collections::BTreeMap;

use crate::vcs::model::{ChangedFile, FileStatus};

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
    /// Index into the changed-files list.
    File { index: usize, status: FileStatus },
}

#[derive(Default)]
struct TmpDir {
    dirs: BTreeMap<String, TmpDir>,
    files: Vec<(String, usize, FileStatus)>,
}

impl FileTree {
    pub fn build(files: &[ChangedFile]) -> FileTree {
        let mut root = TmpDir::default();
        for (index, file) in files.iter().enumerate() {
            let components: Vec<String> = file
                .path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let Some((name, dirs)) = components.split_last() else {
                continue;
            };
            let mut cursor = &mut root;
            for dir in dirs {
                cursor = cursor.dirs.entry(dir.clone()).or_default();
            }
            cursor.files.push((name.clone(), index, file.status));
        }

        let mut tree = FileTree {
            nodes: Vec::new(),
            roots: Vec::new(),
            visible: Vec::new(),
        };
        tree.roots = tree.convert(root, 0, "");
        tree.recompute_visible();
        tree
    }

    /// Convert a temp dir's children to nodes (dirs first, both sorted);
    /// returns their ids.
    fn convert(&mut self, dir: TmpDir, depth: usize, prefix: &str) -> Vec<usize> {
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
                path: path.clone(),
                depth,
                kind: NodeKind::Dir {
                    children: Vec::new(),
                    expanded: true,
                },
            });
            let children = self.convert(sub, depth + 1, &path);
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

    fn row(&self, i: usize) -> Option<&Node> {
        self.visible.get(i).map(|&id| &self.nodes[id])
    }

    /// Changed-files index of the file at visible row `i`, if it's a file.
    pub fn file_at(&self, i: usize) -> Option<usize> {
        match self.row(i)?.kind {
            NodeKind::File { index, .. } => Some(index),
            NodeKind::Dir { .. } => None,
        }
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
        if let NodeKind::Dir { expanded, .. } = &mut self.nodes[id].kind
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
            if let NodeKind::Dir { expanded, .. } = &mut node.kind
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
        if let NodeKind::Dir { expanded, .. } = &mut self.nodes[id].kind {
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
