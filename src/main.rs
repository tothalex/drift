use anyhow::Result;
use clap::Parser;

use drift::app::App;
use drift::cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Command::Lang { command }) = &cli.command {
        return drift::lang::cli::run(command);
    }

    if let Some(Command::Update { check }) = &cli.command {
        return drift::update::run(*check);
    }

    if let Some(Command::Doctor) = &cli.command {
        return drift::doctor::run();
    }

    if cli.init_config {
        let path = drift::config::write_default()?;
        println!("wrote {}", path.display());
        return Ok(());
    }

    // Plugin languages join the registry before the config parses:
    // `[theme.<lang>]` sections validate against the full registry.
    for warning in drift::lang::init_plugins() {
        eprintln!("warning: {warning}");
    }

    // Fail before touching the terminal so errors print normally.
    let mut config = drift::config::load()?;
    // The flag only ever turns the sync off, so it can't fight a config
    // that already has it off.
    config.viewed_sync &= !cli.no_viewed_sync;
    let vcs = drift::vcs::detect(&cli.path)?;
    // Base priority: --base flag, then config, then auto-detection.
    let base = cli.base.clone().or_else(|| config.base.clone());
    let mut app = App::new(vcs, base.as_deref(), config)?;
    if let Some(number) = cli.pr {
        app.open_pr_at_start(number);
    }

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let result = app.run(&mut terminal);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    result
}
