use anyhow::Result;
use clap::Parser;

use drift::app::App;
use drift::cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.init_config {
        let path = drift::config::write_default()?;
        println!("wrote {}", path.display());
        return Ok(());
    }

    // Fail before touching the terminal so errors print normally.
    let config = drift::config::load()?;
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
