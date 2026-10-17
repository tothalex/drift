#!/bin/sh
# Runs inside the drift-demo herdr session's first pane (ai-herdr.tape types
# `sh /tmp/drift-demo-setup` after copying this file there): splits a
# shell pane to the right and starts a claude agent in it, widens drift's
# pane to 55%, waits for the agent to come up, then launches drift —
# stopping the session when it quits. The drift binary path is read
# from /tmp/drift-demo-bin because text typed into a herdr pane can't
# expand the recording shell's $DRIFT.
set -e

DRIFT=$(cat /tmp/drift-demo-bin)
# Keep updater/plugin chatter from flashing into the pane while it
# records.
PANE=$(herdr pane split --current --direction right --cwd /tmp/drift-demo --no-focus \
	--env DISABLE_AUTOUPDATER=1 --env CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 |
	jq -r '.result.pane.pane_id')
# Pre-allow read-only inspection so no permission prompt stalls the
# recording mid-answer. A first start in the fixture stops at Claude's
# trust question, its cursor on "No, exit" — answer the other option,
# and only when that question is what is on screen.
if ! herdr agent start demo --kind claude --pane "$PANE" --timeout 60000 -- \
	--allowedTools "Read Grep Glob Bash(git:*)" >/dev/null 2>&1; then
	if herdr agent read demo --source visible | grep -q "Yes, I trust this folder"; then
		herdr agent send-keys demo down enter >/dev/null
	fi
fi
herdr agent wait demo --until idle --timeout 90000 >/dev/null
herdr pane resize --direction right --amount 0.05 --current >/dev/null
clear
env XDG_CONFIG_HOME=/tmp/drift-demo-xdg "$DRIFT" --icons
# Quitting drift ends the demo: the whole throwaway session stops, its
# agent with it — the tape never types into a pane a live claude holds.
herdr session stop drift-demo >/dev/null
