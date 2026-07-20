# hlcheck — syntax-highlighting parity check

Verifies that drift colors code the same way neovim does, character by
character, instead of eyeballing screenshots.

## How it works

1. `dump.lua` runs headless in neovim against your real config and writes
   the resolved foreground color of every character, flagging which ones
   came from an LSP semantic token.
2. `examples/dump_colors` (in drift) writes the same thing for drift, using
   the default theme.
3. `compare.py` joins the two and reports the match rate, splitting
   mismatches into **tree-sitter diffs** (worth fixing) and **LSP-only**
   differences (unreachable — drift has no language server).

## Run

```sh
# uses ~/.config/nvim/init.lua by default
./hlcheck/run.sh

# or point at a specific config
NVIM_INIT=~/dotfiles/nvim/init.lua ./hlcheck/run.sh
```

Files whose language has no parser installed in the target neovim are
skipped (neovim would render them plain, so there is nothing to compare).

This is a measurement and investigation tool, not a pass/fail gate — some
gaps below are unreachable by design, so a non-empty diff is expected.

## Why 100% parity is not reachable

The checker makes the ceiling concrete rather than a guess:

- **LSP semantic tokens** — neovim recolors identifiers via the language
  server (a `defaultLibrary` member turns yellow, a plain reference red).
  drift has only tree-sitter, so it can't reproduce these. Reported
  separately and never counted as a fixable diff.
- **Context-dependent colors** — onedarkpro varies a color by syntactic
  position within one language (Rust call-argument parens are purple
  while its other brackets are orange). drift's `[theme.<lang>]`
  sections cover per-language differences (Go's purple brackets vs the
  orange default), but not per-position ones.
- **Query richness** — nvim-treesitter ships richer queries than the
  grammar-bundled ones drift compiles, so some tokens (`void`, Rust `use`
  path segments, a few operators) land on a different capture.
