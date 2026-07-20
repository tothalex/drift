//! Dev probe: print every highlight token drift assigns to a snippet, to
//! sanity-check capture→color mapping. `cargo run --example dump_tokens`

use std::path::Path;

fn main() {
    let source = r#"import { CartItem } from "./models";
export class Cart {
    private items: CartItem[] = [];
    add(item: CartItem): void {
        const existing = this.items.find((i) => i.sku === item.sku);
        existing.quantity = Math.min(existing.quantity + item.quantity, 99);
    }
    checkout(discount?: Discount): Receipt {
        const shipping = taxable > 100 ? 0 : 9.99;
        const receiptId = `R-${Date.now()}`;
        if (discount.kind === "percent") {}
    }
}
"#;
    let highlights =
        drift::processor::highlight::highlight(Path::new("x.ts"), source).expect("ts supported");
    for (lineno, line) in source.lines().enumerate() {
        let spans = highlights.spans_for(lineno as u32 + 1);
        if spans.is_empty() {
            continue;
        }
        println!("L{}: {line}", lineno + 1);
        let mut prev_end = 0;
        for span in spans {
            let text = &line[span.start..span.end.min(line.len())];
            let overlap = if span.start < prev_end {
                "  <<OVERLAP"
            } else {
                ""
            };
            prev_end = span.end;
            println!(
                "    [{:>3}..{:<3}] {text:<20} {:?}{overlap}",
                span.start, span.end, span.token
            );
        }
    }
}
