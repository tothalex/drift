# Launching drift in a repo with working changes: the tree grouped by
# kind, with Nerd Font icons; walking a change and its scope; checking a
# file off, then an "agent edit" landing on it (it comes back marked);
# flipping the tree to plain directories and back.
# Regenerate:  sh assets/tapes/setup-demo.sh && vhs assets/tapes/launch.tape
# (expects a drift binary on PATH or DRIFT=/path/to/drift, and Symbols
# Nerd Font Mono installed for the icons)

Output assets/launch.gif

Set Shell zsh
Set FontSize 15
Set FontFamily "Menlo, Symbols Nerd Font Mono"
Set Width 1200
Set Height 640
Set Padding 12
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

# `drift` runs with --icons, and starts the agent edit's clock: about
# 13s after launch it lands on src/cart.ts — checked off by then.
Hide
Type "cd /tmp/drift-demo && drift() { (sleep 13.5; sh .claude/agent-edit.sh) &! command ${DRIFT:-drift} --icons; } && clear"
Enter
Show

Sleep 600ms
Type@120ms "drift"
Sleep 500ms
Enter
Sleep 1.2s
# Widen the tree (4 × 2%) so grouped names and icons fit.
Type "4>"
Sleep 1.3s

# Walk the change, widen the scope and narrow it again.
Type "l"
Sleep 600ms
Type@350ms "jjj"
Sleep 1s
Type "]"
Sleep 1.5s
Type "["
Sleep 1s

# Check cart.ts off; H/L hop file to file, across the groups.
Type "h"
Sleep 500ms
Type "x"
Sleep 1.5s
Type "L"
Sleep 1.5s
Type "L"
Sleep 1.5s

# The agent edits cart.ts: its check drops and it is marked ↻.
Sleep 3.5s

# f flips the tree to plain directories, and back to groups.
Type "f"
Sleep 2s
Type "f"
Sleep 2.5s
