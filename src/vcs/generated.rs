//! Machine-written files — lockfiles, minified bundles, codegen output —
//! that a review steps over by default. The repository has the final
//! say through `.gitattributes` (`linguist-generated`, the attribute
//! GitHub collapses diffs on, `=false` included); the built-in names
//! only answer for files it leaves unspecified.

use std::path::Path;

/// Lockfile names that don't end in `.lock`.
const NAMES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "bun.lockb",
    "go.sum",
    "Package.resolved",
    "packages.lock.json",
    "gradle.lockfile",
];

/// File-name endings of generated output. `.lock` covers Cargo, yarn,
/// poetry, uv, bundler, composer, CocoaPods, mix, pub, deno, flakes.
const SUFFIXES: &[&str] = &[".lock", ".min.js", ".min.css", ".pb.go", "_pb2.py"];

/// Whether `path` is generated: the repository's `linguist-generated`
/// verdict when it gives one, otherwise the built-in names.
pub fn is_generated(path: &Path, attribute: Option<bool>) -> bool {
    attribute.unwrap_or_else(|| {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        NAMES.contains(&name)
            || name.contains(".generated.")
            || SUFFIXES
                .iter()
                .any(|suffix| name.len() > suffix.len() && name.ends_with(suffix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated(path: &str) -> bool {
        is_generated(Path::new(path), None)
    }

    #[test]
    fn lockfiles_and_build_output_are_generated() {
        for path in [
            "Cargo.lock",
            "web/yarn.lock",
            "package-lock.json",
            "pnpm-lock.yaml",
            "go.sum",
            "dist/app.min.js",
            "api/v1/user.pb.go",
            "src/schema.generated.ts",
        ] {
            assert!(generated(path), "{path}");
        }
    }

    #[test]
    fn source_and_manifests_are_not() {
        for path in [
            "Cargo.toml",
            "package.json",
            "src/lock.rs",
            "src/.lock",
            "go.mod",
            "docs/locking.md",
        ] {
            assert!(!generated(path), "{path}");
        }
    }

    #[test]
    fn the_repository_attribute_overrides_the_names() {
        assert!(!is_generated(Path::new("Cargo.lock"), Some(false)));
        assert!(is_generated(Path::new("src/parser.c"), Some(true)));
    }
}
