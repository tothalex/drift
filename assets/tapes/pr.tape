# Reviewing a pull request: threads, replies, the whole conversation.
# Records against tothalex/gitdiff-demo PR #1 (private demo repo);
# needs an authenticated gh. Regenerate:
#   DRIFT=target/release/drift vhs assets/tapes/pr.tape

Output assets/pr.gif

Set Shell zsh
Set FontSize 15
Set FontFamily "Menlo, Symbols Nerd Font Mono"
Set Width 1200
Set Height 640
Set Padding 12
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

Hide
Type "cd ~/Code/gitdiff-demo && alias drift='${DRIFT:-drift} --icons' && clear"
Enter
Show

Sleep 600ms
Type@110ms "drift --pr 1"
Sleep 500ms
Enter
Sleep 5.5s
# Widen the tree (4 × 2%) so grouped names and icons fit.
Type "4>"
Sleep 1.5s

# Read the description, then walk to the change with the review thread.
Sleep 1.5s
Type "L"
Sleep 1.2s
Type "L"
Sleep 2s
# Into the code pane, onto the lines under the review thread.
Type "l"
Sleep 500ms
Type@300ms "jj"
Sleep 4s
