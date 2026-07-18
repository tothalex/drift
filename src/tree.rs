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
    pub depth: usize,
    pub kind: NodeKind,
}

pub enum NodeKind {
    Dir { children: Vec<usize>, expanded: bool },
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
        tree.roots = tree.convert(root, 0);
        tree.recompute_visible();
        tree
    }

    /// Convert a temp dir's children to nodes (dirs first, both sorted);
    /// returns their ids.
    fn convert(&mut self, dir: TmpDir, depth: usize) -> Vec<usize> {
        let mut ids = Vec::new();
        for (mut label, mut sub) in dir.dirs {
            // Compact chains of single-child directories: src/vcs/git.
            while sub.files.is_empty() && sub.dirs.len() == 1 {
                let (next_label, next) = sub.dirs.pop_first().expect("len checked");
                label = format!("{label}/{next_label}");
                sub = next;
            }
            let id = self.nodes.len();
            self.nodes.push(Node {
                label,
                depth,
                kind: NodeKind::Dir { children: Vec::new(), expanded: true },
            });
            let children = self.convert(sub, depth + 1);
            let NodeKind::Dir { children: slot, .. } = &mut self.nodes[id].kind else {
                unreachable!();
            };
            *slot = children;
            ids.push(id);
        }
        let mut files = dir.files;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (label, index, status) in files {
            let id = self.nodes.len();
            self.nodes.push(Node {
                label,
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
                if let NodeKind::Dir { children, expanded: true } = &nodes[id].kind {
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
    fn file_at_maps_to_original_indices() {
        let tree = FileTree::build(&[changed("src/b.rs"), changed("a.rs")]);
        // Rows: src, src/b.rs, a.rs — original indices 0 and 1.
        assert_eq!(tree.file_at(0), None);
        assert_eq!(tree.file_at(1), Some(0));
        assert_eq!(tree.file_at(2), Some(1));
        assert_eq!(tree.first_file_row(), Some(1));
    }
}
