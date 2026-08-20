#!/usr/bin/env bash
# The external-CLI surface drift invokes, asserted against whatever
# versions are installed. canary.yml runs this weekly against the
# latest gh/glab/herdr releases: a red run means a tool changed under
# drift — adjust the calls (and the floors in src/cliver.rs) before a
# user finds out, the way herdr 0.7.5 removing `agent send` was found
# by a user (#21).
#
# The lists mirror what the code invokes: src/forge/gh.rs,
# src/forge/glab.rs, src/connect/herdr.rs.
set -u

fail=0

check() {
  desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    printf 'ok    %s\n' "$desc"
  else
    printf 'GONE  %s  (%s)\n' "$desc" "$*"
    fail=1
  fi
}

require() {
  if command -v "$1" >/dev/null 2>&1; then
    return 0
  fi
  printf 'GONE  %s is not installed in the canary environment\n' "$1"
  fail=1
  return 1
}

if require gh; then
  echo "== $(gh --version | head -1)"
  check "gh api"       gh api --help
  check "gh repo view" gh repo view --help
  check "gh pr list"   gh pr list --help
  check "gh pr view"   gh pr view --help
  check "gh pr diff"   gh pr diff --help
  # drift passes explicit --json field lists and --paginate.
  check "gh pr list --json"  sh -c "gh pr list --help 2>&1 | grep -q -- --json"
  check "gh api --paginate"  sh -c "gh api --help 2>&1 | grep -q -- --paginate"
fi
echo

if require glab; then
  echo "== $(glab --version | head -1)"
  check "glab api" glab api --help
  check "glab api --paginate" sh -c "glab api --help 2>&1 | grep -q -- --paginate"
fi
echo

if require herdr; then
  echo "== $(herdr --version)"
  check "herdr agent list"      herdr agent list --help
  check "herdr tab list"        herdr tab list --help
  check "herdr workspace list"  herdr workspace list --help
  check "herdr pane send-text"  herdr pane send-text --help
  check "herdr pane send-keys"  herdr pane send-keys --help
  # The schema bundled with the binary names every socket method the
  # CLI commands above resolve to — a removal shows up here even if a
  # deprecated CLI alias still answers --help.
  schema=$(herdr api schema --json 2>/dev/null || true)
  for method in agent.list tab.list workspace.list pane.send_text pane.send_keys; do
    if printf '%s' "$schema" | grep -q "\"$method\""; then
      printf 'ok    herdr schema method %s\n' "$method"
    else
      printf 'GONE  herdr schema method %s\n' "$method"
      fail=1
    fi
  done
fi

exit "$fail"
