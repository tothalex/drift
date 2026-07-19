//! Tab expansion for display. Terminal backends give `\t` zero width, so
//! tab-indented files would render flush-left. Everything the processor
//! sees — sources and hunk contents — is expanded up front, keeping
//! highlight and emphasis byte ranges consistent with the rendered text.

use std::borrow::Cow;

use crate::vcs::model::FileDiff;

/// Rendered width of a tab stop.
const TAB_STOP: usize = 4;

/// Expand tabs to spaces, advancing to the next tab stop (column-aware,
/// so mid-line tabs keep their alignment).
pub fn expand_tabs(text: &str) -> Cow<'_, str> {
    if !text.contains('\t') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 3 * TAB_STOP);
    let mut col = 0usize;
    for ch in text.chars() {
        match ch {
            '\t' => {
                let width = TAB_STOP - col % TAB_STOP;
                for _ in 0..width {
                    out.push(' ');
                }
                col += width;
            }
            '\n' => {
                out.push('\n');
                col = 0;
            }
            _ => {
                out.push(ch);
                col += 1;
            }
        }
    }
    Cow::Owned(out)
}

/// `expand_tabs` that reuses the allocation when nothing changes.
pub fn expand_tabs_owned(text: String) -> String {
    match expand_tabs(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(expanded) => expanded,
    }
}

/// Expand tabs in every hunk line of a diff, in place.
pub fn expand_diff(diff: &mut FileDiff) {
    if let FileDiff::Text { hunks } = diff {
        for hunk in hunks.iter_mut() {
            for line in hunk.lines.iter_mut() {
                if let Cow::Owned(expanded) = expand_tabs(&line.content) {
                    line.content = expanded;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_tabs_become_indent() {
        assert_eq!(expand_tabs("\t\tfoo"), "        foo");
    }

    #[test]
    fn mid_line_tab_advances_to_next_stop() {
        // "ab" ends at column 2 → tab fills 2 columns to reach stop 4.
        assert_eq!(expand_tabs("ab\tcd"), "ab  cd");
        // At an exact stop a tab is a full TAB_STOP wide.
        assert_eq!(expand_tabs("abcd\tef"), "abcd    ef");
    }

    #[test]
    fn newline_resets_the_column() {
        assert_eq!(expand_tabs("a\n\tb"), "a\n    b");
    }

    #[test]
    fn tab_free_text_is_borrowed() {
        assert!(matches!(expand_tabs("    foo"), Cow::Borrowed(_)));
    }
}
