//! Parser for single-file unified diff text into structured hunks.
//!
//! Shared across providers: git, hg, and jj all emit this format. Providers
//! that get structured diffs natively (a future `gix` backend) skip it and
//! build `FileDiff` directly.

use crate::vcs::model::{DiffLine, FileDiff, Hunk, LineKind};

/// Parse the unified diff for a single file.
///
/// Header lines before the first hunk (`diff --git`, `index`, `---`, `+++`)
/// are ignored. A binary-change marker yields [`FileDiff::Binary`]. Empty
/// input yields `Text` with no hunks (e.g. a pure rename).
pub fn parse(text: &str) -> FileDiff {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    for raw in text.lines() {
        if hunks.is_empty() && (raw.starts_with("Binary files ") || raw == "GIT binary patch") {
            return FileDiff::Binary;
        }
        if let Some(hunk) = parse_hunk_header(raw) {
            old_no = hunk.old_range.0;
            new_no = hunk.new_range.0;
            hunks.push(hunk);
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue; // still in the file header
        };
        let (kind, old_lineno, new_lineno) = match raw.as_bytes().first() {
            Some(b'+') => {
                let no = new_no;
                new_no += 1;
                (LineKind::Added, None, Some(no))
            }
            Some(b'-') => {
                let no = old_no;
                old_no += 1;
                (LineKind::Removed, Some(no), None)
            }
            // A fully empty line is a context line whose trailing space
            // was stripped somewhere along the way; be lenient.
            Some(b' ') | None => {
                let nos = (old_no, new_no);
                old_no += 1;
                new_no += 1;
                (LineKind::Context, Some(nos.0), Some(nos.1))
            }
            // "\ No newline at end of file" and anything unrecognized.
            _ => continue,
        };
        hunk.lines.push(DiffLine {
            kind,
            old_lineno,
            new_lineno,
            content: raw.get(1..).unwrap_or("").to_string(),
        });
    }
    FileDiff::Text { hunks }
}

/// Parse `@@ -start[,count] +start[,count] @@ optional section header`.
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_spec, rest) = rest.split_once(" +")?;
    let (new_spec, rest) = rest.split_once(" @@")?;
    Some(Hunk {
        old_range: parse_range(old_spec)?,
        new_range: parse_range(new_spec)?,
        header: rest.strip_prefix(' ').unwrap_or(rest).to_string(),
        lines: Vec::new(),
    })
}

fn parse_range(spec: &str) -> Option<(u32, u32)> {
    match spec.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((spec.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modification_with_line_numbers() {
        let diff = "\
diff --git a/f.rs b/f.rs
index 111..222 100644
--- a/f.rs
+++ b/f.rs
@@ -1,3 +1,4 @@ fn main()
 context one
-removed line
+added one
+added two
 context two";
        let FileDiff::Text { hunks } = parse(diff) else {
            panic!("expected text diff");
        };
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!(hunk.header, "fn main()");
        assert_eq!(hunk.old_range, (1, 3));
        assert_eq!(hunk.new_range, (1, 4));

        let nos: Vec<_> = hunk
            .lines
            .iter()
            .map(|l| (l.kind, l.old_lineno, l.new_lineno))
            .collect();
        assert_eq!(
            nos,
            vec![
                (LineKind::Context, Some(1), Some(1)),
                (LineKind::Removed, Some(2), None),
                (LineKind::Added, None, Some(2)),
                (LineKind::Added, None, Some(3)),
                (LineKind::Context, Some(3), Some(4)),
            ]
        );
        assert_eq!(hunk.lines[1].content, "removed line");
    }

    #[test]
    fn parses_new_file() {
        let diff = "\
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,2 @@
+first
+second";
        let FileDiff::Text { hunks } = parse(diff) else {
            panic!("expected text diff");
        };
        assert_eq!(hunks[0].lines[0].new_lineno, Some(1));
        assert_eq!(hunks[0].lines[1].new_lineno, Some(2));
        assert!(hunks[0].lines.iter().all(|l| l.kind == LineKind::Added));
    }

    #[test]
    fn detects_binary() {
        assert!(matches!(
            parse("Binary files a/x.png and b/x.png differ"),
            FileDiff::Binary
        ));
    }

    #[test]
    fn skips_no_newline_marker() {
        let diff = "\
@@ -1 +1 @@
-old
\\ No newline at end of file
+new";
        let FileDiff::Text { hunks } = parse(diff) else {
            panic!("expected text diff");
        };
        assert_eq!(hunks[0].lines.len(), 2);
    }

    #[test]
    fn empty_input_is_contentless_text() {
        let FileDiff::Text { hunks } = parse("") else {
            panic!("expected text diff");
        };
        assert!(hunks.is_empty());
    }

    #[test]
    fn treats_blank_line_as_context() {
        let diff = "\
@@ -1,3 +1,3 @@
 a

-b
+c";
        let FileDiff::Text { hunks } = parse(diff) else {
            panic!("expected text diff");
        };
        assert_eq!(hunks[0].lines[1].kind, LineKind::Context);
        assert_eq!(hunks[0].lines[1].content, "");
        assert_eq!(hunks[0].lines[2].old_lineno, Some(3));
    }
}
