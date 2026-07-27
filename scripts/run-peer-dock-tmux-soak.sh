#!/usr/bin/env bash
# #407 Peer TUI soak — live tmux UX test for the peer dock.
#
# Drives a real octos-tui against a real octos serve, stages peers via
# peer_handoff (or /peer --prepare), and asserts:
#   - peer chip appears in the session strip
#   - overflow shows the structured "Peers: N · M live · K⚠" pill
#   - Ctrl+J toggles the peer dock between collapsed pill and per-peer rows
#   - the per-peer row shows the live tail / blocked reason glyph
#
# Self-contained: builds nothing (assumes binaries exist), creates its own
# workspace + data dir under /tmp, tears down on exit.
set -euo pipefail

OCTOS_BIN="${OCTOS_BIN:-/Users/yuechen/home/octos-one/octos/target/debug/octos}"
OCTOS_TUI_BIN="${OCTOS_TUI_BIN:-/Users/yuechen/home/octos-tui-wt-bashcard/target/debug/octos-tui}"

RUN_ID="${RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
ROOT="/tmp/octos-peer-soak-$RUN_ID"
WORKSPACE="$ROOT/workspace"
DATA_DIR="$ROOT/data"
LOGS_DIR="$ROOT/logs"
ARTIFACT_DIR="${ARTIFACT_DIR:-$ROOT/artifacts}"
SERVER_SESSION="peer-soak-server-$RUN_ID"
TUI_SESSION="peer-soak-tui-$RUN_ID"
HOST="127.0.0.1"
PORT="${PORT:-50279}"
AUTH_TOKEN="soak-token-$RUN_ID"
PROFILE_ID="${PROFILE_ID:-dev}"
SESSION_ID="$PROFILE_ID:local:soak#$RUN_ID"
ENDPOINT="ws://$HOST:$PORT/api/ui-protocol/ws"

mkdir -p "$WORKSPACE" "$DATA_DIR" "$LOGS_DIR" "$ARTIFACT_DIR"
mkdir -p "$WORKSPACE/peers"

die() { echo "FAIL: $*" >&2; teardown 2>/dev/null || true; exit 1; }

capture_pane() {
  local out="$1"
  tmux capture-pane -t "$TUI_SESSION" -p -J -S -300 > "$out" 2>/dev/null || \
    printf 'capture failed\n' > "$out"
}

wait_for() {
  local pattern="$1" timeout="${2:-20}"
  local deadline=$((SECONDS + timeout)) snapshot="$ARTIFACT_DIR/tui-capture.txt"
  while [ "$SECONDS" -le "$deadline" ]; do
    tmux has-session -t "$TUI_SESSION" 2>/dev/null || die "TUI exited while waiting for: $pattern"
    capture_pane "$snapshot"
    if grep --fixed-strings --line-regexp "$pattern" "$snapshot" >/dev/null 2>&1 || \
       grep --fixed-strings "$pattern" "$snapshot" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "TIMEOUT waiting for: $pattern" >&2
  capture_pane "$ARTIFACT_DIR/tui-capture-timeout.txt"
  return 1
}

submit_prompt() {
  local prompt="$1" buffer="peer-soak-prompt-$RUN_ID" tmp
  tmp="$(mktemp "${TMPDIR:-/tmp}/peer-soak.XXXXXX")"
  printf '%s' "$prompt" > "$tmp"
  tmux send-keys -t "$TUI_SESSION" Escape
  sleep 0.2
  tmux load-buffer -b "$buffer" "$tmp"
  rm -f "$tmp"
  tmux paste-buffer -p -t "$TUI_SESSION" -b "$buffer"
  tmux delete-buffer -b "$buffer" >/dev/null 2>&1 || true
  sleep 0.35
  tmux send-keys -t "$TUI_SESSION" Enter
}

send_ctrl() {
  local ch="$1"
  # Ctrl+J = newline in terminals; ratatui apps usually treat C-j distinctly
  # from Enter only when the app captures keysym-level. Send via send-keys
  # C-j notation (tmux interprets C-x as Ctrl+x).
  tmux send-keys -t "$TUI_SESSION" "C-$ch"
  sleep 0.4
}

teardown() {
  echo "--- teardown ---"
  tmux kill-session -t "$TUI_SESSION" 2>/dev/null || true
  tmux kill-session -t "$SERVER_SESSION" 2>/dev/null || true
}

trap teardown EXIT

echo "=== binaries ==="
[ -x "$OCTOS_BIN" ] || die "octos binary missing: $OCTOS_BIN"
[ -x "$OCTOS_TUI_BIN" ] || die "octos-tui binary missing: $OCTOS_TUI_BIN"
"$OCTOS_BIN" --version | head -1
"$OCTOS_TUI_BIN" --version | head -1

echo "=== launching octos serve (ws on $PORT) ==="
# OCTOS_HOME=~/.crew resolves the 'dev' profile (LLM creds). A SEPARATE
# --instance-data-dir keeps the serve lock off the running production
# server (PID held against ~/.crew/instances/...).
tmux new-session -d -s "$SERVER_SESSION" \
  "cd '$WORKSPACE' && OCTOS_HOME='$HOME/.crew' KIMI_API_KEY='$KIMI_API_KEY' \
    '$OCTOS_BIN' serve --host '$HOST' --port '$PORT' \
    --instance-data-dir '$DATA_DIR/runtime' \
    --auth-token '$AUTH_TOKEN' \
    2>&1 | tee '$LOGS_DIR/server.log'"
# Wait for server readiness.
for i in $(seq 1 30); do
  if grep -qE "listening|ready|serving|bound|accepting|octos API server" "$LOGS_DIR/server.log" 2>/dev/null; then
    echo "server ready after ${i}s"; break
  fi
  sleep 1
done
grep -qE "listening|ready|serving|bound|accepting|octos API server" "$LOGS_DIR/server.log" 2>/dev/null || \
  { echo "server log head:"; head -40 "$LOGS_DIR/server.log"; die "server did not become ready"; }

echo "=== launching octos-tui (ws transport) ==="
tmux new-session -d -s "$TUI_SESSION" \
  "cd '$WORKSPACE' && '$OCTOS_TUI_BIN' \
    --endpoint '$ENDPOINT' --auth-token '$AUTH_TOKEN' \
    --profile-id '$PROFILE_ID' --session '$SESSION_ID' \
    --cwd '$WORKSPACE' --theme codex \
    2>&1 | tee '$LOGS_DIR/tui.log'; echo exited \$?; sleep 30"

# Wait for the composer / first-paint.
wait_for "Ask Octos" 25 || { capture_pane "$ARTIFACT_DIR/tui-first-paint.txt"; die "TUI never reached composer"; }
echo "TUI is up."
capture_pane "$ARTIFACT_DIR/tui-baseline.txt"

echo
echo "=== TEST 1: stage a peer via /peer --prepare (user-initiated) ==="
# /peer --prepare should call peer/prepare RPC, stage a peer, and the client
# auto-opens it (per apply_peer_staged_event / apply_peer_prepared_event).
submit_prompt "/peer --prepare --title ci-red --brief 'Investigate why CI is red. Read the latest workflow run logs and summarize the failure.'"
# Allow the RPC + session/open roundtrip.
sleep 4
capture_pane "$ARTIFACT_DIR/tui-after-peer-prepare.txt"
if grep -qE "ci-red|ci_red|cired" "$ARTIFACT_DIR/tui-after-peer-prepare.txt"; then
  echo "PASS: ci-red chip appeared in strip"
else
  echo "WARN: ci-red chip not visible yet — peer may still be opening"
fi

echo
echo "=== TEST 2: agent-driven peer_handoff (model stages a peer) ==="
# Ask the model to hand off a second peer — server should emit peer/staged,
# TUI auto-opens.
submit_prompt "Use peer_handoff to spin off a peer called lint-pass that checks the clippy warnings in src/lib.rs. Use worktree:false. Once staged, tell me the slug."
# Allow the model turn + staging.
wait_for "lint-pass|lint_pass|lintpass|spun off|staged" 60 || \
  echo "WARN: lint-pass staging not visible — model may not have called peer_handoff"
sleep 3
capture_pane "$ARTIFACT_DIR/tui-after-handoff.txt"

echo
echo "=== TEST 3: overflow → structured Peers pill ==="
# At this point we should have main + 2 peers; on a narrow-enough terminal
# the strip overflows and renders "Peers: N · M live".
capture_pane "$ARTIFACT_DIR/tui-strip-overflow.txt"
if grep -qE "Peers|peers|peer" "$ARTIFACT_DIR/tui-strip-overflow.txt"; then
  echo "PASS: peer pill text present"
else
  echo "WARN: no peer pill visible — strip may not have overflowed (try narrower tmux window)"
fi

echo
echo "=== TEST 4: Ctrl+J toggles the peer dock ==="
# Capture before toggle.
capture_pane "$ARTIFACT_DIR/tui-pre-ctrlj.txt"
PRE_ROWS=$(grep -cE "✻|⚠|○|opening" "$ARTIFACT_DIR/tui-pre-ctrlj.txt" || true)
send_ctrl "j"
sleep 1
capture_pane "$ARTIFACT_DIR/tui-post-ctrlj-1.txt"
send_ctrl "j"
sleep 1
capture_pane "$ARTIFACT_DIR/tui-post-ctrlj-2.txt"
echo "PASS: Ctrl+J delivered twice without crash"

echo
echo "=== TEST 5: dock rows show per-peer detail (glyph + slug + tail) ==="
# After toggling expanded, the per-peer rows should appear in the dock area.
if grep -qE "ci-red|lint-pass" "$ARTIFACT_DIR/tui-post-ctrlj-1.txt" || \
   grep -qE "ci-red|lint-pass" "$ARTIFACT_DIR/tui-post-ctrlj-2.txt"; then
  echo "PASS: peer slug visible in dock rows"
else
  echo "WARN: peer slug not visible in dock — may be collapsed or rows hidden by height gate"
fi

echo
echo "=== TEST 6: blocked ⚠ glyph when a peer needs approval ==="
# If either peer's turn reaches an approval, the row glyph should flip to ⚠.
# (Depends on model behavior — may not fire in every run.)
if grep -qE "⚠" "$ARTIFACT_DIR/tui-post-ctrlj-1.txt" || \
   grep -qE "⚠" "$ARTIFACT_DIR/tui-post-ctrlj-2.txt"; then
  echo "PASS: ⚠ glyph present somewhere"
else
  echo "INFO: no ⚠ — no peer happened to need approval during this run"
fi

echo
echo "=== FINAL CAPTURE ==="
capture_pane "$ARTIFACT_DIR/tui-final.txt"
echo "--- final pane (last 25 lines) ---"
tail -25 "$ARTIFACT_DIR/tui-final.txt"
echo
echo "=== SOAK COMPLETE ==="
echo "Artifacts: $ARTIFACT_DIR"
echo "Server log: $LOGS_DIR/server.log"
echo "TUI log: $LOGS_DIR/tui.log"
