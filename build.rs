fn main() {
    // The runtime grammar compiler (`drift lang build`) targets the triple
    // drift itself was built for; cargo only exposes it at build time.
    println!(
        "cargo:rustc-env=DRIFT_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
