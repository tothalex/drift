#!/bin/sh
# Runs inside the drift-demo herdr session's first pane (ai-herdr.tape types
# `sh /tmp/drift-demo-setup` after copying this file there): starts a
# claude agent in a right split, widens drift's pane to 55%, waits for
# the agent to come up, then launches drift. The drift binary path is
# read from /tmp/drift-demo-bin because text typed into a herdr pane
# can't expand the recording shell's $DRIFT.
set -e

DRIFT=$(cat /tmp/drift-demo-bin)
# Pre-allow read-only inspection so no permission prompt stalls the
# recording mid-answer, and keep updater/plugin chatter from flashing
# into the pane while it records.
herdr agent start claude --cwd /tmp/drift-demo --tab "$HERDR_TAB_ID" --split right --no-focus \
	--env DISABLE_AUTOUPDATER=1 --env CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 -- \
	claude --allowedTools "Read Grep Glob Bash(git:*)" >/dev/null
herdr pane resize --direction right --amount 0.05 --current >/dev/null
herdr agent wait claude --status idle --timeout 90000 >/dev/null
clear
exec env XDG_CONFIG_HOME=/tmp/drift-demo-xdg "$DRIFT"
