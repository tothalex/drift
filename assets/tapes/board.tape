# The review board: every worktree of the repo, then every branch —
# and drilling into one of them without leaving the panel.
# Regenerate:
#   sh assets/tapes/setup-demo.sh
#   DRIFT=$PWD/target/release/drift sh assets/tapes/render.sh board

Output assets/board.gif

Set Shell zsh
Set FontSize 15
Set FontFamily "Menlo, Symbols Nerd Font Mono"
Set Width 1200
Set Height 640
Set Padding 12
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

Hide
Type "cd /tmp/drift-demo && alias drift='${DRIFT:-drift} --icons' && clear"
Enter
Show

Sleep 600ms
Type@120ms "drift"
Sleep 400ms
Enter
Sleep 1.2s
# Widen the tree (2 × 2%) so grouped names and icons fit.
Type "2>"
Sleep 1.3s

# Every worktree, newest first, with what each one holds.
Type "b"
Sleep 3s

# `g` jumps to the top row — the worktree that just moved — and `l`
# opens it; two rows down is what is uncommitted in it.
Type "g"
Sleep 900ms
Type "l"
Sleep 2.5s
Type@320ms "jj"
Sleep 1.2s
Type "l"
Sleep 3.5s

# Back to the board — it reopens where it was left — then `b` again for
# the other axis: the repo's branches.
Type "b"
Sleep 2.5s
Type "b"
Sleep 3s

# `/` searches the rows, and a branch nothing has checked out is
# reviewed at its tip.
Type "/"
Sleep 500ms
Type@160ms "bump"
Sleep 1.8s
Enter
Sleep 900ms
Enter
Sleep 4s
