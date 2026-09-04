# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.26.1](https://github.com/tothalex/drift/compare/v0.26.0...v0.26.1) - 2026-09-04

### Fixed

- fall back to the upstream when the base shares no history with head

## [0.26.0](https://github.com/tothalex/drift/compare/v0.25.0...v0.26.0) - 2026-09-01

### Added

- replace tracked/untracked scopes with committed and uncommitted
- highlight embedded style/script blocks via language injections
- distinguish folders in the tree and add opt-in nerd font icons

### Fixed

- repin go/javascript/python grammars to reachable release commits

### Other

- reuse the real index's stat data in change scans
- document injections.scm in readme and site

## [0.25.0](https://github.com/tothalex/drift/compare/v0.24.0...v0.25.0) - 2026-08-20

### Added

- diagnose external CLI version mismatches, add drift doctor ([#23](https://github.com/tothalex/drift/pull/23))

### Fixed

- send prompts through herdr pane send-text ([#22](https://github.com/tothalex/drift/pull/22))

### Other

- weekly canary against the latest gh/glab/herdr ([#24](https://github.com/tothalex/drift/pull/24))

## [0.24.0](https://github.com/tothalex/drift/compare/v0.23.1...v0.24.0) - 2026-08-12

### Added

- add tracked-changes review scope ([#19](https://github.com/tothalex/drift/pull/19))

## [0.23.1](https://github.com/tothalex/drift/compare/v0.23.0...v0.23.1) - 2026-08-11

### Fixed

- highlight commit-scoped views from the commit's tree ([#17](https://github.com/tothalex/drift/pull/17))

## [0.23.0](https://github.com/tothalex/drift/compare/v0.22.1...v0.23.0) - 2026-08-11

### Added

- show the changelog in drift update ([#14](https://github.com/tothalex/drift/pull/14))

### Other

- bump minor for features even pre-1.0 ([#16](https://github.com/tothalex/drift/pull/16))
- automate release PRs and tagging with release-plz ([#13](https://github.com/tothalex/drift/pull/13))
