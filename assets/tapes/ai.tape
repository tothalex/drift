# Sending a selection from drift to a Claude Code session beside it.
# Needs a running claude agent pane in herdr; pass its target as
# DEMO_AGENT_TARGET and the drift binary as DRIFT. Regenerate:
#   sh assets/tapes/setup-demo.sh
#   DRIFT=target/release/drift DEMO_AGENT_TARGET=<pane> vhs assets/tapes/ai.tape

Output assets/agent.gif

Set Shell zsh
Set FontSize 14
Set Width 1400
Set Height 720
Set Padding 10
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

Hide
Type "tmux kill-session -t drift-demo 2>/dev/null; tmux new-session -d -s drift-demo -x 214 -y 48"
Enter
Type "tmux set -t drift-demo status off; tmux set -t drift-demo pane-border-style fg=colour235; tmux set -t drift-demo pane-active-border-style fg=colour235"
Enter
Type "tmux split-window -h -p 45 -t drift-demo"
Enter
Type "tmux send-keys -t drift-demo:0.0 'cd /tmp/drift-demo && clear && XDG_CONFIG_HOME=/tmp/drift-demo-xdg $DRIFT' Enter"
Enter
Type "tmux send-keys -t drift-demo:0.1 'clear && herdr agent attach $DEMO_AGENT_TARGET' Enter"
Enter
Type "tmux select-pane -t drift-demo:0.0"
Enter
Sleep 3s
Type "clear && tmux attach -t drift-demo"
Enter
Sleep 2s
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
Sleep 15s

Hide
Type "q"
Type "tmux kill-session -t drift-demo"
Enter
