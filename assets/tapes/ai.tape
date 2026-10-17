# Sending a selection from drift to a Claude Code session beside it: drift
# and claude side by side in a throwaway tmux server (its own socket,
# never the user's tmux), where drift finds the agent by its process.
# Needs tmux and the claude CLI on PATH, and the typescript language
# plugin in the demo config dir. Regenerate from the repo root:
#   sh assets/tapes/setup-demo.sh
#   XDG_CONFIG_HOME=/tmp/drift-demo-xdg target/release/drift lang install typescript
#   DRIFT=$PWD/target/release/drift sh assets/tapes/render.sh ai

Output assets/agent.gif

Set Shell zsh
Set FontSize 14
Set FontFamily "Menlo, Symbols Nerd Font Mono"
Set Width 1400
Set Height 720
Set Padding 10
Set Theme { "name": "onedark-dark", "background": "#000000", "foreground": "#abb2bf", "cursor": "#89ca78", "black": "#1e2127", "red": "#ef596f", "green": "#89ca78", "yellow": "#e5c07b", "blue": "#61afef", "magenta": "#d55fde", "cyan": "#2bbac5", "white": "#abb2bf", "brightBlack": "#5c6370", "brightRed": "#ef596f", "brightGreen": "#89ca78", "brightYellow": "#e5c07b", "brightBlue": "#61afef", "brightMagenta": "#d55fde", "brightCyan": "#2bbac5", "brightWhite": "#dcdfe4" }

# Plain sh in both panes, and no tmux config: a login shell's own setup
# can start a multiplexer of its own (an interactive fish outside herdr
# attaching the user's herdr, say), which would swallow every key meant
# for the demo panes.
Hide
Type "tmux -L drift-demo kill-server 2>/dev/null; tmux -L drift-demo -f /dev/null new-session -d -s drift-demo -x 214 -y 48 -c /tmp/drift-demo sh"
Enter
Type "tmux -L drift-demo set -t drift-demo status off; tmux -L drift-demo set -t drift-demo pane-border-style fg=colour235; tmux -L drift-demo set -t drift-demo pane-active-border-style fg=colour235"
Enter
Type "tmux -L drift-demo split-window -h -p 45 -t drift-demo -c /tmp/drift-demo sh"
Enter
# Claude on the right, read-only inspection pre-allowed so no permission
# prompt stalls the answer. A first start in the fixture asks whether to
# trust it, cursor on "No, exit": Down+Enter answers the other option,
# sent only when that question is what the pane shows.
Type `tmux -L drift-demo send-keys -t drift-demo:0.1 'clear && DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 claude --allowedTools "Read Grep Glob Bash(git:*)"' Enter`
Enter
Sleep 8s
Type "tmux -L drift-demo capture-pane -p -t drift-demo:0.1 | grep -q 'Yes, I trust this folder' && tmux -L drift-demo send-keys -t drift-demo:0.1 Down Enter"
Enter
Sleep 4s
Type "tmux -L drift-demo send-keys -t drift-demo:0.0 'clear && XDG_CONFIG_HOME=/tmp/drift-demo-xdg $DRIFT --icons' Enter"
Enter
Type "tmux -L drift-demo select-pane -t drift-demo:0.0"
Enter
Sleep 5s
Type "clear && tmux -L drift-demo attach -t drift-demo"
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
Type "tmux -L drift-demo kill-server"
Enter
