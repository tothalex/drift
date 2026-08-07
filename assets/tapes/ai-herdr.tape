# Sending a selection from drift to a Claude Code session beside it —
# drift and the agent both live in panes of a herdr (herdr.dev) session.
# Creates and tears down a throwaway herdr session named drift-demo;
# needs herdr and the claude CLI on PATH, and the typescript language
# plugin in the demo config dir. Regenerate from the repo root:
#   sh assets/tapes/setup-demo.sh
#   XDG_CONFIG_HOME=/tmp/drift-demo-xdg target/release/drift lang install typescript
#   DRIFT=target/release/drift vhs assets/tapes/ai-herdr.tape

Output assets/agent-herdr.gif

Set Shell zsh
Set FontSize 14
Set Width 1400
Set Height 720
Set Padding 10
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

Hide
Type `herdr session stop drift-demo 2>/dev/null; herdr session delete drift-demo 2>/dev/null`
Enter
Type `realpath "$DRIFT" > /tmp/drift-demo-bin && cp assets/tapes/herdr-demo-pane.sh /tmp/drift-demo-setup`
Enter
# The fresh session's server inherits this shell's environment, and its
# panes inherit the server's — strip herdr's own vars (no nested-session
# confusion) and any Claude Code ones (claude in the demo pane must not
# think it is a child session).
Type `cd /tmp/drift-demo && env $(env | awk -F= '/^(HERDR_|CLAUDE|AI_AGENT)/{printf "-u %s ", $1}') herdr --session drift-demo`
Enter
Sleep 3s
Type "sh /tmp/drift-demo-setup"
Enter
Sleep 25s
Show

# Select the changed lines in cart.ts, send them to claude.
Sleep 1.5s
Type "l"
Sleep 800ms
Type "v"
Sleep 800ms
Type "j"
Sleep 1s
Type "s"
Sleep 1.6s
Type@110ms "explain this change"
Sleep 1.2s
Enter
Sleep 3s

# Claude answers on the right.
Sleep 24s

Hide
Type "q"
Sleep 1s
Type "herdr session stop drift-demo"
Enter
Sleep 2s
Type "herdr session delete drift-demo"
Enter
Sleep 1s
