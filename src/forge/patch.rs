//! Split a multi-file patch (`gh pr diff` output) into per-file diffs.
//!
//! Only the splitting and header reading live here; hunk parsing is the
//! shared `vcs::unidiff` parser, which already ignores header lines and
//! detects binary markers.

use std::path::PathBuf;

use crate::forge::model::PrFile;
use crate::vcs::model::{ChangedFile, FileStatus};
use crate::vcs::unidiff;

pub fn split(patch: &str) -> Vec<PrFile> {
    let mut files = Vec::new();
    let mut chunk: Vec<&str> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("diff --git ") && !chunk.is_empty() {
            files.extend(parse_chunk(&chunk));
            chunk.clear();
        }
        if !chunk.is_empty() || line.starts_with("diff --git ") {
            chunk.push(line);
        }
    }
    if !chunk.is_empty() {
        files.extend(parse_chunk(&chunk));
    }
    files.sort_by(|a, b| a.changed.path.cmp(&b.changed.path));
    files
}

/// One `diff --git` chunk → a changed file. Paths come from the most
/// reliable header available: `rename from/to`, then `---`/`+++`, then
/// the `diff --git` line itself.
fn parse_chunk(lines: &[&str]) -> Option<PrFile> {
    let mut added = false;
    let mut deleted = false;
    let mut rename_from = None;
    let mut rename_to = None;
    let mut copy = false;
    let mut minus = None; // `---` path, already unprefixed; None for /dev/null
    let mut plus = None;

    for line in lines {
        if line.starts_with("@@ ") {
            break; // headers end at the first hunk
        }
        if line.starts_with("new file mode") {
            added = true;
        } else if line.starts_with("deleted file mode") {
            deleted = true;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            rename_from = Some(unquote(path));
        } else if let Some(path) = line.strip_prefix("rename to ") {
            rename_to = Some(unquote(path));
        } else if let Some(path) = line.strip_prefix("copy from ") {
            rename_from = Some(unquote(path));
            copy = true;
        } else if let Some(path) = line.strip_prefix("copy to ") {
            rename_to = Some(unquote(path));
            copy = true;
        } else if let Some(path) = line.strip_prefix("--- ") {
            minus = strip_side(path);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            plus = strip_side(path);
        }
    }

    let header_paths = lines.first().and_then(|first| git_line_paths(first));
    let old = rename_from
        .clone()
        .or(minus)
        .or_else(|| header_paths.clone().map(|(a, _)| a));
    let new = rename_to
        .clone()
        .or(plus)
        .or_else(|| header_paths.map(|(_, b)| b));

    let (status, path, old_path) = if rename_to.is_some() {
        let status = if copy {
            FileStatus::Copied
        } else {
            FileStatus::Renamed
        };
        (status, new?, old)
    } else if added || old.is_none() {
        (FileStatus::Added, new?, None)
    } else if deleted || new.is_none() {
        (FileStatus::Deleted, old?, None)
    } else {
        (FileStatus::Modified, new?, None)
    };

    let text = lines.join("\n");
    Some(PrFile {
        changed: ChangedFile {
            status,
            path: PathBuf::from(path),
            old_path: old_path.map(PathBuf::from),
        },
        diff: unidiff::parse(&text),
    })
}

/// `a/src/x.rs` → `src/x.rs`; `/dev/null` → None. Quotes are stripped
/// first: git quotes paths with special characters.
fn strip_side(path: &str) -> Option<String> {
    let path = unquote(path);
    if path == "/dev/null" {
        return None;
    }
    let stripped = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(&path);
    Some(stripped.to_string())
}

/// Best-effort paths from `diff --git a/old b/new`. Ambiguous for paths
/// with spaces — later headers win over this.
fn git_line_paths(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?;
    let (a, b) = rest.split_once(" b/")?;
    let old = unquote(a.strip_prefix("a/").unwrap_or(a));
    Some((old, unquote(b)))
}

/// Strip git's quoting: surrounding double quotes and the common
/// backslash escapes. Lenient — unknown escapes pass through.
fn unquote(path: &str) -> String {
    let path = path.trim_end();
    let Some(inner) = path
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return path.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::FileDiff;

    const PATCH: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
 pub mod app;
-pub mod old;
+pub mod new;
diff --git a/src/old_name.rs b/src/new_name.rs
similarity index 90%
rename from src/old_name.rs
rename to src/new_name.rs
index 3333333..4444444 100644
--- a/src/old_name.rs
+++ b/src/new_name.rs
@@ -1 +1 @@
-old
+new
diff --git a/added.txt b/added.txt
new file mode 100644
index 0000000..5555555
--- /dev/null
+++ b/added.txt
@@ -0,0 +1 @@
+hello
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 6666666..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/logo.png b/logo.png
index 7777777..8888888 100644
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn splits_statuses_and_paths() {
        let files = split(PATCH);
        let summary: Vec<(char, &str)> = files
            .iter()
            .map(|f| (f.changed.status.letter(), f.changed.path.to_str().unwrap()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ('A', "added.txt"),
                ('D', "gone.txt"),
                ('M', "logo.png"),
                ('M', "src/lib.rs"),
                ('R', "src/new_name.rs"),
            ]
        );
        let renamed = files
            .iter()
            .find(|f| f.changed.status == FileStatus::Renamed)
            .unwrap();
        assert_eq!(
            renamed
                .changed
                .old_path
                .as_deref()
                .unwrap()
                .to_str()
                .unwrap(),
            "src/old_name.rs"
        );
        let binary = files
            .iter()
            .find(|f| f.changed.path.ends_with("logo.png"))
            .unwrap();
        assert!(matches!(binary.diff, FileDiff::Binary));
    }

    #[test]
    fn parses_hunks_through_unidiff() {
        let files = split(PATCH);
        let lib = files
            .iter()
            .find(|f| f.changed.path.ends_with("lib.rs"))
            .unwrap();
        let FileDiff::Text { hunks } = &lib.diff else {
            panic!("expected text diff");
        };
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines.len(), 3);
        assert_eq!(hunks[0].lines[2].content, "pub mod new;");
        assert_eq!(hunks[0].lines[2].new_lineno, Some(2));
    }

    #[test]
    fn quoted_paths_are_unescaped() {
        let patch = "\
diff --git \"a/sp ace.txt\" \"b/sp ace.txt\"
index 1111111..2222222 100644
--- \"a/sp ace.txt\"
+++ \"b/sp ace.txt\"
@@ -1 +1 @@
-x
+y
";
        let files = split(patch);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].changed.path.to_str().unwrap(), "sp ace.txt");
    }

    #[test]
    fn empty_patch_yields_no_files() {
        assert!(split("").is_empty());
        assert!(split("\n").is_empty());
    }
}
