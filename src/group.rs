//! Review groups: the changeset split by what each file is — the code
//! first, then the tests, docs, config and dependency manifests around
//! it, generated output last — so a review reads the change before its
//! paperwork. Path rules only, language-agnostic; the first rule that
//! matches wins.

use std::path::Path;

use crate::tree::VIRTUAL_PREFIX;
use crate::vcs::model::ChangedFile;

/// What a changed file is, in review order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Code,
    Tests,
    Docs,
    Config,
    Deps,
    Generated,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Code,
        Kind::Tests,
        Kind::Docs,
        Kind::Config,
        Kind::Deps,
        Kind::Generated,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Code => "code",
            Kind::Tests => "tests",
            Kind::Docs => "docs",
            Kind::Config => "config",
            Kind::Deps => "dependencies",
            Kind::Generated => "generated",
        }
    }
}

/// One group of the tree's grouped layout: a header over its files.
pub struct Group {
    pub label: String,
    /// Indices into the changed-files list.
    pub files: Vec<usize>,
}

/// The changeset grouped by [`Kind`], empty groups left out. Virtual
/// entries (the PR conversation, a commit's message) belong to no
/// group: the tree pins them above all of them.
pub fn by_kind(files: &[ChangedFile], generated: impl Fn(usize) -> bool) -> Vec<Group> {
    let mut groups: Vec<Group> = Kind::ALL
        .iter()
        .map(|kind| Group {
            label: kind.label().to_string(),
            files: Vec::new(),
        })
        .collect();
    for (index, file) in files.iter().enumerate() {
        if file.path.to_string_lossy().starts_with(VIRTUAL_PREFIX) {
            continue;
        }
        let kind = kind_of(&file.path, generated(index));
        groups[Kind::ALL.iter().position(|k| *k == kind).expect("in ALL")]
            .files
            .push(index);
    }
    groups.retain(|group| !group.files.is_empty());
    groups
}

/// Base names (before the first dot, any case) of project paperwork.
const DOC_STEMS: &[&str] = &[
    "README",
    "CHANGELOG",
    "CHANGES",
    "HISTORY",
    "LICENSE",
    "LICENCE",
    "COPYING",
    "NOTICE",
    "AUTHORS",
    "CONTRIBUTING",
    "CODE_OF_CONDUCT",
    "SECURITY",
];
const DOC_EXTENSIONS: &[&str] = &["md", "mdx", "markdown", "rst", "adoc", "asciidoc"];
const DOC_DIRS: &[&str] = &["docs", "doc"];

const TEST_DIRS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "specs",
    "e2e",
    "fixtures",
    "__fixtures__",
    "__snapshots__",
    "__mocks__",
    "testdata",
];

/// Manifests: where dependencies are declared.
const DEP_NAMES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pnpm-workspace.yaml",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "Pipfile",
    "Gemfile",
    "composer.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "Package.swift",
    "pubspec.yaml",
    "mix.exs",
    "deno.json",
    "vcpkg.json",
];

const CONFIG_NAMES: &[&str] = &[
    "Dockerfile",
    "Containerfile",
    "Makefile",
    "makefile",
    "justfile",
    "Justfile",
    "Procfile",
    "Vagrantfile",
    "rust-toolchain",
];
/// Data formats that, at the repository root, are almost always some
/// tool's configuration.
const ROOT_CONFIG_EXTENSIONS: &[&str] = &["toml", "yaml", "yml", "json", "ini", "cfg"];

/// Which group a path belongs to. Generated files go last whatever
/// their path says; docs are tested before tests, so a README under
/// `tests/` reads as the docs it is.
pub fn kind_of(path: &Path, generated: bool) -> Kind {
    if generated {
        return Kind::Generated;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dirs: Vec<String> = path
        .parent()
        .map(|parent| {
            parent
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let (base, ext) = match name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() => (base, ext.to_ascii_lowercase()),
        _ => (name.as_str(), String::new()),
    };
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let in_dir = |set: &[&str]| dirs.iter().any(|dir| set.contains(&dir.as_str()));

    if DOC_EXTENSIONS.contains(&ext.as_str())
        || DOC_STEMS.contains(&stem.as_str())
        || stem.starts_with("LICENSE-")
        || in_dir(DOC_DIRS)
    {
        return Kind::Docs;
    }
    if in_dir(TEST_DIRS)
        || base.ends_with("_test")
        || base.ends_with("_spec")
        || base.ends_with(".test")
        || base.ends_with(".spec")
        || base.contains(".stories")
        || (base.starts_with("test_") && ext == "py")
        || name == "conftest.py"
        || name == "tests.rs"
        || ext == "snap"
    {
        return Kind::Tests;
    }
    if DEP_NAMES.contains(&name.as_str())
        || (base.starts_with("requirements") && ext == "txt")
        || ext == "csproj"
        || ext == "gemspec"
    {
        return Kind::Deps;
    }
    if name.starts_with('.')
        || dirs.iter().any(|dir| dir.starts_with('.'))
        || CONFIG_NAMES.contains(&name.as_str())
        || name.starts_with("Dockerfile")
        || name.starts_with("docker-compose")
        || name.contains(".config.")
        || (base.starts_with("tsconfig") && ext == "json")
        || (dirs.is_empty() && ROOT_CONFIG_EXTENSIONS.contains(&ext.as_str()))
    {
        return Kind::Config;
    }
    Kind::Code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::model::FileStatus;
    use std::path::PathBuf;

    fn kind(path: &str) -> Kind {
        kind_of(Path::new(path), false)
    }

    fn assert_kind(expected: Kind, paths: &[&str]) {
        for path in paths {
            assert_eq!(kind(path), expected, "{path}");
        }
    }

    #[test]
    fn source_is_code() {
        assert_kind(
            Kind::Code,
            &[
                "src/main.rs",
                "src/app/mod.rs",
                "web/src/App.vue",
                "lib/parser.py",
                "site/index.html",
                "languages/rust.toml",
                "src/readme_parser.rs",
                "src/test_helpers.rs",
            ],
        );
    }

    #[test]
    fn tests_by_directory_and_by_name() {
        assert_kind(
            Kind::Tests,
            &[
                "tests/git_vcs.rs",
                "src/app/tests.rs",
                "pkg/server/handler_test.go",
                "web/Button.test.tsx",
                "web/Button.spec.ts",
                "web/Button.stories.tsx",
                "app/test_models.py",
                "conftest.py",
                "src/__snapshots__/view.snap",
                "packages/core/test/fixtures/pr.json",
            ],
        );
    }

    #[test]
    fn docs_by_extension_name_and_directory() {
        assert_kind(
            Kind::Docs,
            &[
                "README.md",
                "CHANGELOG.md",
                "LICENSE",
                "LICENSE-MIT",
                "docs/architecture.html",
                "tests/README.md",
                ".github/CONTRIBUTING.md",
            ],
        );
    }

    #[test]
    fn manifests_are_dependencies() {
        assert_kind(
            Kind::Deps,
            &[
                "Cargo.toml",
                "crates/core/Cargo.toml",
                "package.json",
                "go.mod",
                "requirements-dev.txt",
                "App.csproj",
            ],
        );
    }

    #[test]
    fn tooling_is_config() {
        assert_kind(
            Kind::Config,
            &[
                ".github/workflows/ci.yml",
                ".gitignore",
                ".gitattributes",
                ".cargo/config.toml",
                "Dockerfile",
                "Dockerfile.dev",
                "docker-compose.yml",
                "Makefile",
                "vite.config.ts",
                "tsconfig.node.json",
                "release-plz.toml",
                "rustfmt.toml",
            ],
        );
    }

    #[test]
    fn generated_wins_over_the_path() {
        assert_eq!(
            kind_of(Path::new("tests/fixtures.lock"), true),
            Kind::Generated
        );
        assert_eq!(kind_of(Path::new("src/parser.c"), true), Kind::Generated);
    }

    #[test]
    fn groups_come_in_review_order_without_empty_ones_or_virtual_entries() {
        let files: Vec<ChangedFile> = ["#conversation", "README.md", "src/a.rs", "Cargo.lock"]
            .iter()
            .map(|path| ChangedFile {
                status: FileStatus::Modified,
                path: PathBuf::from(path),
                old_path: None,
            })
            .collect();

        let groups = by_kind(&files, |index| index == 3);

        let shape: Vec<(&str, &[usize])> = groups
            .iter()
            .map(|g| (g.label.as_str(), g.files.as_slice()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("code", &[2][..]),
                ("docs", &[1][..]),
                ("generated", &[3][..])
            ]
        );
    }
}
