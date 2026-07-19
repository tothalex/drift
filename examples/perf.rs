//! Times each stage of a file selection, per changed file.
//! Usage: cargo run --example perf -- <repo-path>

use std::path::Path;
use std::time::Instant;

fn ms(from: Instant, to: Instant) -> f64 {
    (to - from).as_secs_f64() * 1000.0
}

fn main() -> anyhow::Result<()> {
    let repo = std::env::args().nth(1).expect("usage: perf <repo-path>");
    let vcs = drift::vcs::detect(Path::new(&repo))?;
    let cmp = vcs.comparison(None)?;
    let files = vcs.changed_files(&cmp)?;

    println!(
        "{:<24} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "file", "diff", "read", "show", "process", "TOTAL"
    );
    for file in &files {
        let t0 = Instant::now();
        let diff = vcs.file_diff(&cmp, file)?;
        let t1 = Instant::now();
        let source = std::fs::read_to_string(vcs.root().join(&file.path)).ok();
        let t2 = Instant::now();
        let old = vcs.file_at_ancestor(&cmp, file);
        let t3 = Instant::now();
        let _view = drift::processor::process(
            &file.path,
            &diff,
            source.as_deref(),
            old.as_deref(),
            drift::processor::ViewOptions::default(),
        );
        let t4 = Instant::now();
        println!(
            "{:<24} {:>7.1}ms {:>7.1}ms {:>7.1}ms {:>7.1}ms {:>7.1}ms",
            file.path.display().to_string(),
            ms(t0, t1),
            ms(t1, t2),
            ms(t2, t3),
            ms(t3, t4),
            ms(t0, t4),
        );
    }
    Ok(())
}
