# Launching drift in a repo with working changes.
# Regenerate:  sh assets/tapes/setup-demo.sh && vhs assets/tapes/launch.tape
# (expects a drift binary on PATH or DRIFT=/path/to/drift)

Output assets/launch.gif

Set Shell zsh
Set FontSize 15
Set Width 1200
Set Height 640
Set Padding 12
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

Hide
Type "cd /tmp/drift-demo && alias drift=${DRIFT:-drift} && clear"
Enter
Show

Sleep 600ms
Type@120ms "drift"
Sleep 500ms
Enter
Sleep 2.5s

# Walk the change, widen the scope, hop files.
Type@350ms "jjj"
Sleep 1s
Type "]"
Sleep 1.5s
Type "["
Sleep 1s
Type "L"
Sleep 1.8s
Type "L"
Sleep 1.8s
Type "H"
Sleep 1s
Type "x"
Sleep 1.5s
Sleep 2s
