//! Demo file for exercising drift's pull-request view. Never merged.

/// Sums the first `n` natural numbers.
fn sum_to(n: u64) -> u64 {
    let mut total = 0;
    for i in 0..n {
        total += i;
    }
    total
}

fn main() {
    println!("sum_to(10) = {}", sum_to(10));
}
