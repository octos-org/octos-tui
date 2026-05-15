#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: scripts/validate-tmux-ux-capture.sh <tmux-capture.txt> [server.log]

Validates real terminal capture text for UX bugs reported during live tmux
soaking. This is intentionally screen-oriented: it checks rendered rows and
server evidence, not just AppUI semantic events.
EOF
}

if [[ $# -lt 1 || $# -gt 2 ]]; then
  usage
  exit 2
fi

capture="$1"
server_log="${2:-}"

if [[ ! -f "$capture" ]]; then
  echo "capture file not found: $capture" >&2
  exit 2
fi
if [[ -n "$server_log" && ! -f "$server_log" ]]; then
  echo "server log not found: $server_log" >&2
  exit 2
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

# Strip common ANSI control sequences and carriage returns while preserving
# Unicode border/cursor glyphs from ratatui/tmux captures.
perl -pe 's/\e\[[0-9;?]*[ -\/]*[@-~]//g; s/\r//g' "$capture" >"$tmp"

failures=0
fail() {
  echo "UX_CAPTURE_FAIL $*" >&2
  failures=$((failures + 1))
}

line_no() {
  local pattern="$1"
  rg -n "$pattern" "$tmp" | head -n 1 | cut -d: -f1 || true
}

composer_start="$(line_no '^┌Composer')"

if [[ -z "$composer_start" ]]; then
  fail "missing composer pane"
fi

if rg -q '^┌(Work|Progress)' "$tmp"; then
  fail "split work/progress pane should not render in normal chat layout"
fi

if rg -q 'Queued messages \(([0-9]+)\)' "$tmp"; then
  if ! rg -q 'queued [0-9]+ message(s)? after active turn' "$tmp"; then
    fail "queued composer messages are not listed in chat history"
  fi
fi

if rg -q 'Plan rounds|Current round:|Is this a path within the current project/workspace|Or is it a system path outside the workspace|Did you mean a different directory' "$tmp"; then
  fail "turn plan/clarifying rows leaked into split routing surface"
fi

if rg -q '^ state .*[◐◑◒◓]' "$tmp"; then
  fail "bottom state line must not animate a spinner"
fi

if rg -q '^┌(Work|Progress).*›|^┌Wor ›|^┌Progress.*›' "$tmp"; then
  fail "input text overlaps removed work/progress pane border"
fi

if rg -q '• ####|What I \*can\* access|\[x\] Point me|\[x\] Or share' "$tmp"; then
  fail "markdown markers leaked into rendered assistant text"
fi

if rg -q 'Task Working|Progress .*Thinking|state .*Working' "$tmp" && [[ -n "$server_log" ]]; then
  if rg -q 'lifecycle notification not delivered.*turn/completed|writer channel full for lifecycle frame|lifecycle ws send failed; aborting connection' "$server_log"; then
    fail "UI shows running after backend dropped turn/completed under backpressure"
  fi
fi

if [[ -n "$server_log" ]] && rg -q 'lifecycle notification not delivered.*turn/completed|writer channel full for lifecycle frame' "$server_log"; then
  fail "server log contains dropped turn/completed lifecycle notification"
fi

if (( failures > 0 )); then
  echo "UX capture validation failed: $capture" >&2
  exit 1
fi

echo "UX capture validation passed: $capture"
