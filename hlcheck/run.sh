#!/bin/sh
# Compare drift's syntax highlighting against neovim's, per character, for
# every file in samples/. Requires `nvim` on PATH; point NVIM_INIT at the
# neovim config whose colorscheme you want to match (defaults to the
# standard location).
#
#   ./hlcheck/run.sh
#   NVIM_INIT=~/dotfiles/nvim/init.lua ./hlcheck/run.sh
set -eu

NVIM_INIT="${NVIM_INIT:-$HOME/.config/nvim/init.lua}"
DIR=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$DIR/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

command -v nvim >/dev/null || { echo "nvim not found on PATH" >&2; exit 2; }
[ -f "$NVIM_INIT" ] || { echo "no neovim config at $NVIM_INIT (set NVIM_INIT)" >&2; exit 2; }

cargo build --quiet --example dump_colors --manifest-path "$ROOT/Cargo.toml"
DUMP="$ROOT/target/debug/examples/dump_colors"

status=0
for file in "$DIR"/samples/*; do
    name=$(basename "$file")
    nvim --headless -u "$NVIM_INIT" -l "$DIR/dump.lua" "$file" "$TMP/nvim.tsv" 2>/dev/null
    "$DUMP" "$file" >"$TMP/drift.tsv"
    echo "== $name =="
    python3 "$DIR/compare.py" "$TMP/nvim.tsv" "$TMP/drift.tsv" || status=1
done
exit "$status"
