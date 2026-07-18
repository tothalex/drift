use std::path::PathBuf;

use clap::Parser;

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

    /// Write the default config to ~/.config/drift/config.toml and exit.
    #[arg(long)]
    pub init_config: bool,
}
