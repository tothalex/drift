use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// The `--help` footer: the keys worth knowing before the `?` overlay is
/// reachable, where the config lives, and what a session usually starts
/// with. Keys here must match `keymap::KEY_DEFAULTS`; a test enforces it.
const AFTER_HELP: &str = "\
Keys:
  j / k    move down / up        x / X   check file off / undo
  L / H    next / prev file      a       comment on a line
  p        open a pull request   r       refresh
  ?        every keybinding      q       quit

Config:
  ~/.config/drift/config.toml (drift --init-config writes the defaults)

Examples:
  drift                    review your working changes
  drift --base develop     compare against develop instead
  drift --pr 42            open pull request #42";

/// Review your work like a pull request: everything changed since the
/// base branch, committed or not.
#[derive(Parser, Debug)]
#[command(name = "drift", version, about, after_help = AFTER_HELP)]
pub struct Cli {
    /// Repository path (defaults to the current directory).
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Base branch to compare against (defaults to origin/HEAD, main, or master).
    #[arg(short, long)]
    pub base: Option<String>,

    /// Open this pull request / merge request number right away
    /// (requires the gh or glab CLI, authenticated).
    #[arg(long, value_name = "NUMBER")]
    pub pr: Option<u64>,

    /// Keep review checks local: don't tick files off as "viewed" on the
    /// pull request (GitHub only; same as forge.viewed_sync = false).
    #[arg(long)]
    pub no_viewed_sync: bool,

    /// Write the default config to ~/.config/drift/config.toml and exit.
    #[arg(long)]
    pub init_config: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage language plugins (tree-sitter grammars and queries).
    Lang {
        #[command(subcommand)]
        command: LangCommand,
    },
    /// Update drift to the latest release. Requires curl and tar; refuses
    /// installs managed by cargo or homebrew.
    Update {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum LangCommand {
    /// Install a language: a curated name (see `list`) or a grammar
    /// repo git URL. Requires git and a C compiler.
    Install {
        /// Language name ("json") or git URL of a tree-sitter grammar.
        language: String,
        /// Commit to build, overriding the manifest's pinned rev.
        #[arg(long)]
        rev: Option<String>,
    },
    /// (Re)compile grammars: one language, or every installed one.
    /// Needed after a drift upgrade changes the tree-sitter ABI.
    Build {
        /// Language to rebuild; all installed languages when omitted.
        language: Option<String>,
    },
    /// List built-in, installed, and installable languages.
    List,
    /// Uninstall a plugin: its manifest, queries, sources and grammar.
    Remove {
        /// Installed plugin language name.
        language: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KEY_DEFAULTS;

    #[test]
    fn help_footer_quotes_the_real_keys() {
        // The footer is hand-written prose; drift-apart from the actual
        // bindings is exactly what a reader can't check.
        for (action, key) in [
            ("cursor_down", "j"),
            ("cursor_up", "k"),
            ("check_file", "x"),
            ("uncheck_last", "X"),
            ("next_file", "L"),
            ("prev_file", "H"),
            ("comment", "a"),
            ("pick_pr", "p"),
            ("refresh", "r"),
            ("help", "?"),
            ("quit", "q"),
        ] {
            let bound = KEY_DEFAULTS
                .iter()
                .find(|(name, _, _)| *name == action)
                .unwrap_or_else(|| panic!("{action} is not an action"))
                .2;
            assert!(
                bound.contains(&key),
                "--help says {key} for {action}, which is bound to {bound:?}"
            );
        }
    }

    #[test]
    fn help_footer_names_the_config_path() {
        assert!(AFTER_HELP.contains("config.toml"));
        assert!(AFTER_HELP.contains("--init-config"));
    }
}
