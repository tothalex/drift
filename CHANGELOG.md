# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.29.0](https://github.com/tothalex/drift/compare/v0.28.0...v0.29.0) - 2026-10-01

### Fixed

- place board agents by the worktree their latest turn changed ([#33](https://github.com/tothalex/drift/pull/33))

  The review board's agent column now follows what each session
  changes, not where its pane was started:

  - A session that stays in the main checkout but edits another
    worktree by path (`cd <worktree> && …`, or writing files there)
    shows on the worktree it is writing to. It used to show on the
    checkout its pane sits in.
  - Files a session wrote outrank checkouts its commands only mention,
    such as a baseline it compares against.
  - Several agents in one worktree are all counted: the row shows the
    busiest one and then a count (`● claude working +1`). Before, a
    working session could be hidden behind an idle one in the same
    checkout.
  - Subagents and workflow agents count toward the session that
    launched them.

## [0.28.0](https://github.com/tothalex/drift/compare/v0.27.0...v0.28.0) - 2026-09-09

### Added

- size the board to the terminal and fold rows with enter ([#31](https://github.com/tothalex/drift/pull/31))

## [0.27.0](https://github.com/tothalex/drift/compare/v0.26.0...v0.27.0) - 2026-09-09

### Added

- review branches on the board and follow agents into worktrees

### Fixed

- fall back to the upstream when the base shares no history with head

### Other

- document the review board and record its demo

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
