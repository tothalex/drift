use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Review your work like a pull request: everything changed since the
/// base branch, committed or not.
#[derive(Parser, Debug)]
#[command(name = "drift", version, about)]
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
