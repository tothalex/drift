# drift — agent instructions

drift is a Rust terminal UI for reviewing working changes like a pull
request: everything since the base branch (committed, uncommitted,
untracked) in one live view, plus real GitHub/GitLab PR review and a
send-to-AI-agent integration.

## Commands

```sh
cargo build                 # debug binary at target/debug/drift
cargo test                  # unit + integration tests (tests/git_vcs.rs)
cargo test <name>           # one test by substring
cargo clippy --all-targets  # must be warning-free
cargo fmt                   # required before committing
```

## Architecture

- `src/app/` — the coordinator: event loop, key dispatch, overlay state
  (picker, compose, help). Fully synchronous — no async runtime;
  background work is `std::thread::spawn` + an `AppEvent` channel.
- `src/processor/` — tree-sitter block scoping, syntax highlighting,
  view flattening (`view.rs` is the canonical row model).
- `src/vcs/` — git via gitoxide; never shells out to `git`.
- `src/forge/` — PRs through the official `gh`/`glab` CLIs; drift never
  speaks HTTP itself. One submodule per forge behind the `Forge` trait.
- `src/connect/` — send-to-agent backends behind the `Bridge` trait;
  one submodule per multiplexer plus a `BACKENDS` registry table in
  `mod.rs`. Adding a backend touches only this directory: new file,
  new `Backend` variant, one registry row.
- `src/keymap.rs` — bindings as data: `KEY_DEFAULTS` + `HELP` tables;
  tests enforce that every action is bound and appears in help.
- `src/theme.rs` — every color drift paints; `THEME_DEFAULTS` is the
  single source (onedark_dark palette).
- `src/config.rs` — `~/.config/drift/config.toml`; each section is a
  serde struct with an `into_config()` validator, and `default_toml()`
  must stay parseable (a test round-trips it).

## Conventions

- Comments state constraints the code can't show — no narration, no
  change-log commentary. Match the existing density and tone.
- Tests are colocated `#[cfg(test)]` modules; parsing/formatting logic
  is factored into pure functions so it's testable from fixtures.
- Adding a key action requires: `Action` variant, `KEY_DEFAULTS` row,
  `HELP` row (tests fail otherwise), and a `handle_key` match arm.
- Commit messages: short, lowercase, imperative-ish, matching
  `git log --oneline`. **No AI co-author trailers.**
- Releases: push the version-bump commit and the tag only — the
  `release.yml` workflow builds binaries and creates the GitHub
  release itself. Never create releases by hand.

## Documentation sync (important)

`README.md` is canonical. When features, keys, config options, or CLI
flags change, update ALL of:

1. `README.md` — the canonical prose.
2. `site/index.html` — the landing page's feature sections and demos.
3. `site/docs.html` — the reference page (install, usage, keybindings,
   configuration, PR setup, AI agents).
4. `config.rs default_toml()` / `--init-config` docs when config
   changes, and the `?` help overlay via `keymap.rs HELP`.

The website (`site/`, deployed by `pages.yml` to GitHub Pages) is plain
HTML/CSS/JS with no dependencies. Every color on it must be a value
drift itself renders: the `theme.rs` palette, drift's ANSI-256 UI grays
(235/236/238 → #262626/#303030/#444444), or the One Dark mono ramp
(#dcdfe4 / #abb2bf / #828997 / #5c6370). The demos' token colors were
verified against real drift ANSI output — when highlighting rules
change, re-verify (`herdr pane read <pane> --format ansi` against a
fixture repo works well).
