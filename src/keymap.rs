//! Keybindings as data: defaults overlaid with the user's `[keys]` config,
//! resolved by the dispatcher and rendered by the `?` overlay from the
//! same table, so the two can never drift apart.

use std::collections::HashMap;

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Help,
    NextFile,
    PrevFile,
    ToggleDir,
    CursorDown,
    CursorUp,
    JumpDown,
    JumpUp,
    JumpTop,
    JumpBottom,
    ScopeWiden,
    ScopeNarrow,
    Search,
    NextMatch,
    PrevMatch,
    ToggleCollapse,
    ToggleCommentFold,
    ToggleThread,
    CheckFile,
    UncheckLast,
    Visual,
    Yank,
    CopyPath,
    GrowTree,
    ShrinkTree,
    PickBase,
    PickPr,
    Comment,
    CommentGeneral,
    Refresh,
    OpenEditor,
}

/// Config name and default keys per action — the single source for both
/// the runtime defaults and `--init-config`.
pub const KEY_DEFAULTS: &[(&str, Action, &[&str])] = &[
    ("quit", Action::Quit, &["q", "ctrl-c"]),
    ("help", Action::Help, &["?"]),
    ("next_file", Action::NextFile, &["l", "right"]),
    ("prev_file", Action::PrevFile, &["h", "left"]),
    ("toggle_dir", Action::ToggleDir, &["o", "enter", "space"]),
    ("cursor_down", Action::CursorDown, &["j", "down"]),
    ("cursor_up", Action::CursorUp, &["k", "up"]),
    ("jump_down", Action::JumpDown, &["d", "pagedown"]),
    ("jump_up", Action::JumpUp, &["u", "pageup"]),
    ("jump_top", Action::JumpTop, &["g"]),
    ("jump_bottom", Action::JumpBottom, &["G"]),
    ("scope_widen", Action::ScopeWiden, &["]"]),
    ("scope_narrow", Action::ScopeNarrow, &["["]),
    ("search", Action::Search, &["/"]),
    ("next_match", Action::NextMatch, &["n"]),
    ("prev_match", Action::PrevMatch, &["N"]),
    ("toggle_collapse", Action::ToggleCollapse, &["z"]),
    ("toggle_comment_fold", Action::ToggleCommentFold, &["C"]),
    ("toggle_thread", Action::ToggleThread, &["t"]),
    ("copy_path", Action::CopyPath, &["c"]),
    ("check_file", Action::CheckFile, &["x"]),
    ("uncheck_last", Action::UncheckLast, &["X"]),
    ("visual", Action::Visual, &["v"]),
    ("yank", Action::Yank, &["y"]),
    ("grow_tree", Action::GrowTree, &[">"]),
    ("shrink_tree", Action::ShrinkTree, &["<"]),
    ("pick_base", Action::PickBase, &["b"]),
    ("pick_pr", Action::PickPr, &["p", "P"]),
    ("comment", Action::Comment, &["a"]),
    ("comment_general", Action::CommentGeneral, &["A"]),
    ("refresh", Action::Refresh, &["r"]),
    ("open_editor", Action::OpenEditor, &["e"]),
];

struct Binding {
    display: String,
    code: KeyCode,
    ctrl: bool,
    action: Action,
}

pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    /// Defaults overlaid with the user's `[keys]` entries. A configured
    /// action replaces all of its default keys.
    pub fn from_overrides(overrides: &HashMap<String, Vec<String>>) -> Result<Keymap> {
        for name in overrides.keys() {
            if !KEY_DEFAULTS.iter().any(|(known, ..)| known == name) {
                bail!(
                    "unknown key action '{name}' (known: {})",
                    KEY_DEFAULTS
                        .iter()
                        .map(|(n, ..)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        let mut bindings = Vec::new();
        for (name, action, defaults) in KEY_DEFAULTS {
            let specs: Vec<String> = match overrides.get(*name) {
                Some(user) if !user.is_empty() => user.clone(),
                Some(_) => bail!("key action '{name}' has no keys"),
                None => defaults.iter().map(|s| s.to_string()).collect(),
            };
            for spec in specs {
                let (code, ctrl) = parse_key(&spec)?;
                if let Some(other) = bindings
                    .iter()
                    .find(|b: &&Binding| b.code == code && b.ctrl == ctrl)
                {
                    bail!(
                        "key '{spec}' is bound to both {:?} and {:?}",
                        other.action,
                        action
                    );
                }
                bindings.push(Binding {
                    display: display_of(&spec),
                    code,
                    ctrl,
                    action: *action,
                });
            }
        }
        Ok(Keymap { bindings })
    }

    pub fn action_for(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        self.bindings
            .iter()
            .find(|b| b.code == code && b.ctrl == ctrl)
            .map(|b| b.action)
    }

    fn primary_key(&self, action: Action) -> &str {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .map_or("", |b| &b.display)
    }

    /// (keys, description) rows for the help overlay.
    pub fn help_rows(&self) -> Vec<(String, &'static str)> {
        HELP.iter()
            .map(|(actions, description)| {
                let keys = actions
                    .iter()
                    .map(|a| self.primary_key(*a))
                    .collect::<Vec<_>>()
                    .join(" / ");
                (keys, *description)
            })
            .collect()
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::from_overrides(&HashMap::new()).expect("defaults parse")
    }
}

/// Help rows: which actions share a row, and the description.
const HELP: &[(&[Action], &str)] = &[
    (
        &[Action::NextFile, Action::PrevFile],
        "next / previous file (skips reviewed)",
    ),
    (&[Action::CheckFile], "check file as reviewed, go to next"),
    (&[Action::UncheckLast], "uncheck the last review, jump back"),
    (&[Action::ToggleDir], "open / close folder"),
    (&[Action::Search], "search the file tree"),
    (
        &[Action::NextMatch, Action::PrevMatch],
        "next / previous match",
    ),
    (
        &[Action::CursorDown, Action::CursorUp],
        "move the code cursor",
    ),
    (&[Action::JumpDown, Action::JumpUp], "jump 15 lines"),
    (
        &[Action::JumpTop, Action::JumpBottom],
        "jump to top / bottom (10G: line 10)",
    ),
    (&[Action::Yank], "copy line / selection"),
    (&[Action::CopyPath], "copy the file path"),
    (&[Action::Visual], "select lines (visual mode)"),
    (
        &[Action::ScopeWiden, Action::ScopeNarrow],
        "widen / narrow block scope",
    ),
    (
        &[Action::ToggleCollapse],
        "collapse / expand unchanged lines",
    ),
    (&[Action::ToggleCommentFold], "fold / unfold comment blocks"),
    (
        &[Action::ToggleThread],
        "fold / unfold the review thread (PR)",
    ),
    (
        &[Action::GrowTree, Action::ShrinkTree],
        "resize panes (or drag the gap)",
    ),
    (&[Action::OpenEditor], "open the file in your editor"),
    (&[Action::PickBase], "choose base branch, then review scope"),
    (&[Action::PickPr], "open a pull request from the forge"),
    (
        &[Action::Comment],
        "comment on the line / reply to the thread (PR)",
    ),
    (&[Action::CommentGeneral], "general PR comment"),
    (&[Action::Refresh], "refresh"),
    (&[Action::Quit], "quit"),
];

/// Parse a key spec: a single character ("g", "G", "<"), a named key
/// ("enter", "space", "up", "pagedown"), optionally prefixed "ctrl-".
/// Digits and esc are reserved (counts and cancel).
fn parse_key(spec: &str) -> Result<(KeyCode, bool)> {
    let (ctrl, rest) = match spec.strip_prefix("ctrl-") {
        Some(rest) => (true, rest),
        None => (false, spec),
    };
    let code = match rest.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "esc" | "escape" => bail!("'esc' is reserved (it cancels)"),
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_digit() => {
                    bail!("digits are reserved for count prefixes")
                }
                (Some(c), None) => KeyCode::Char(c),
                _ => bail!("unknown key '{spec}'"),
            }
        }
    };
    Ok((code, ctrl))
}

/// Short display name for the help overlay.
fn display_of(spec: &str) -> String {
    match spec.to_ascii_lowercase().as_str() {
        "enter" | "return" => "⏎".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "pageup" => "⇞".to_string(),
        "pagedown" => "⇟".to_string(),
        _ => spec.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve_and_ctrl_distinguishes() {
        let map = Keymap::default();
        assert_eq!(
            map.action_for(KeyCode::Char('c'), KeyModifiers::NONE),
            Some(Action::CopyPath)
        );
        assert_eq!(
            map.action_for(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
        assert_eq!(
            map.action_for(KeyCode::Char('z'), KeyModifiers::NONE),
            Some(Action::ToggleCollapse)
        );
    }

    #[test]
    fn overrides_replace_defaults() {
        let overrides = HashMap::from([("next_file".to_string(), vec!["tab".to_string()])]);
        let map = Keymap::from_overrides(&overrides).unwrap();
        assert_eq!(
            map.action_for(KeyCode::Tab, KeyModifiers::NONE),
            Some(Action::NextFile)
        );
        // The default 'l' no longer maps to next_file.
        assert_eq!(map.action_for(KeyCode::Char('l'), KeyModifiers::NONE), None);
    }

    #[test]
    fn conflicts_unknown_names_and_reserved_keys_error() {
        let conflict = HashMap::from([("next_file".to_string(), vec!["q".to_string()])]);
        assert!(Keymap::from_overrides(&conflict).is_err());
        let unknown = HashMap::from([("nope".to_string(), vec!["z".to_string()])]);
        assert!(Keymap::from_overrides(&unknown).is_err());
        let digit = HashMap::from([("yank".to_string(), vec!["5".to_string()])]);
        assert!(Keymap::from_overrides(&digit).is_err());
    }

    #[test]
    fn every_help_action_is_bound_by_default() {
        let map = Keymap::default();
        for (actions, _) in HELP {
            for action in *actions {
                assert!(
                    !map.primary_key(*action).is_empty(),
                    "{action:?} appears in help but has no binding"
                );
            }
        }
    }
}
