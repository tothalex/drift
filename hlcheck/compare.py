#!/usr/bin/env python3
"""Compare a neovim highlight dump (dump.lua) against a drift dump
(examples/dump_colors) character by character.

Usage: compare.py <nvim.tsv> <drift.tsv>

An uncaptured character in drift renders as the terminal's default
foreground, which is onedark's #abb2bf, so `-` is normalized to that
before comparing. Mismatches are split into two buckets:

  LSP-only     neovim's color came from a language server semantic token;
               drift has no LSP, so these are unreachable by design.
  tree-sitter  neovim and drift disagree on a purely tree-sitter color —
               these are the ones worth fixing.
"""
import sys

FG = "#abb2bf"  # onedark_dark default foreground


def norm(color):
    return FG if color == "-" else color


def load(path, width):
    rows = {}
    for line in open(path):
        parts = line.rstrip("\n").split("\t")
        if len(parts) < width:
            continue
        rows[(parts[0], parts[1])] = parts
    return rows


def main():
    nvim = load(sys.argv[1], 5)
    drift = load(sys.argv[2], 4)

    # If neovim colored almost nothing, its tree-sitter parser for this
    # language isn't installed — comparing against plain text is
    # meaningless, so skip rather than report a bogus mismatch rate.
    colored = sum(1 for r in nvim.values() if r[3] not in ("-", FG))
    if colored < 3:
        print("  skipped: neovim produced no highlighting (parser not installed?)")
        return

    keys = sorted(set(nvim) | set(drift), key=lambda k: (int(k[0]), int(k[1])))
    total = match = lsp_gap = ts_gap = 0
    mismatches = []
    for k in keys:
        nrow = nvim.get(k)
        drow = drift.get(k)
        ch = (nrow or drow)[2]
        if ch.strip() == "":
            continue  # whitespace carries no color
        total += 1
        ncol = norm(nrow[3]) if nrow else FG
        dcol = norm(drow[3]) if drow else FG
        src = nrow[4] if nrow else "ts"
        if ncol == dcol:
            match += 1
        elif src == "lsp":
            lsp_gap += 1
            mismatches.append((k, ch, ncol, dcol, "LSP-only (unreachable)"))
        else:
            ts_gap += 1
            mismatches.append((k, ch, ncol, dcol, "tree-sitter diff"))

    pct = lambda n: 100 * n // max(total, 1)
    print(f"  positions (non-space):  {total}")
    print(f"  exact match:            {match}  ({pct(match)}%)")
    print(f"  match ignoring LSP:     {match + lsp_gap}  ({pct(match + lsp_gap)}%)")
    print(f"  tree-sitter diffs:      {ts_gap}")
    for (r, c), ch, ncol, dcol, why in mismatches[:80]:
        print(f"    {r}:{c} {ch!r:>4}  nvim={ncol} drift={dcol}  [{why}]")
    # Non-zero exit if any fixable (non-LSP) difference remains.
    sys.exit(1 if ts_gap else 0)


if __name__ == "__main__":
    main()
