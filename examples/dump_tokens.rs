//! Dev probe: print every highlight token drift assigns to a snippet, to
//! sanity-check capture→color mapping. `cargo run --example dump_tokens`

use std::path::Path;

fn main() {
    let source = r#"import { StockLockTier } from "@qantrum/smartface";
export function parseTierThresholds(raw: string): number[] {
    parsed = JSON.parse(raw);
    if (!Array.isArray(parsed) || !parsed.every((value) => typeof value === "number")) {}
    return [...parsed].sort((a, b) => a - b);
}
const tier = Math.min(reached + 1, StockLockTier.Tier6);
"#;
    let highlights =
        drift::processor::highlight::highlight(Path::new("x.ts"), source).expect("ts supported");
    for (lineno, line) in source.lines().enumerate() {
        let spans = highlights.spans_for(lineno as u32 + 1);
        if spans.is_empty() {
            continue;
        }
        println!("L{}: {line}", lineno + 1);
        for span in spans {
            let text = &line[span.start..span.end.min(line.len())];
            println!("    {text:<24} {:?}", span.token);
        }
    }
}
