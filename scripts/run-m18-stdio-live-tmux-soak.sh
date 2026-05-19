#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
octos_repo="${OCTOS_REPO:-$(cd "$repo_root/../octos" 2>/dev/null && pwd || true)}"

run_id="${OCTOS_TUI_M18_STDIO_RUN_ID:-m18-stdio-live-$(date -u +%Y%m%dT%H%M%SZ)}"
artifact_root="${OCTOS_TUI_M18_STDIO_ARTIFACT_ROOT:-$repo_root/e2e/test-results-m18-stdio-live-tmux}"
artifact_dir="${OCTOS_TUI_M18_STDIO_RUN_ARTIFACT_DIR:-$artifact_root/$run_id}"
runtime_root="${OCTOS_TUI_M18_STDIO_RUNTIME_ROOT:-/tmp/octos-tui-m18-stdio-$run_id}"
workspace="${OCTOS_TUI_M18_STDIO_WORKSPACE:-$runtime_root/workspace}"
data_dir="${OCTOS_TUI_M18_STDIO_DATA_DIR:-$runtime_root/data}"
octos_bin="${OCTOS_BIN:-${octos_repo:+$octos_repo/target/debug/octos}}"
octos_tui_bin="${OCTOS_TUI_BIN:-$repo_root/target/debug/octos-tui}"
profile_id="${OCTOS_TUI_M18_STDIO_PROFILE:-coding}"
session_id="${OCTOS_TUI_M18_STDIO_SESSION:-$profile_id:local:m18-stdio#$run_id}"
tmux_session="${OCTOS_TUI_M18_STDIO_TMUX_SESSION:-octos-m18-stdio-$run_id}"
ready_wait_secs="${OCTOS_TUI_M18_STDIO_READY_WAIT_SECS:-25}"
prompt_wait_secs="${OCTOS_TUI_M18_STDIO_PROMPT_WAIT_SECS:-45}"
repeat_count="${OCTOS_TUI_M18_STDIO_REPEAT_COUNT:-1}"
failure_budget="${OCTOS_TUI_M18_STDIO_FAILURE_BUDGET:-0}"
run_command="${OCTOS_TUI_M18_STDIO_RUN_COMMAND:-}"

usage() {
  cat <<'USAGE'
Usage: scripts/run-m18-stdio-live-tmux-soak.sh <run-once|repeat|self-test|help>

Commands:
  run-once   Launch octos-tui in tmux against real `octos serve --stdio`.
  repeat     Run run-once, or OCTOS_TUI_M18_STDIO_RUN_COMMAND, N times and write a flake-budget report.
  self-test  Exercise repeat-report accounting with synthetic child commands only.

Environment:
  OCTOS_BIN                                  Backend binary. Default: ../octos/target/debug/octos.
  OCTOS_TUI_BIN                              TUI binary. Default: target/debug/octos-tui.
  OCTOS_TUI_M18_STDIO_REPEAT_COUNT           Repeat count for repeat. Default: 1.
  OCTOS_TUI_M18_STDIO_FAILURE_BUDGET         Allowed failed runs before repeat exits nonzero. Default: 0.
  OCTOS_TUI_M18_STDIO_ARTIFACT_ROOT          Report/artifact root. Default: e2e/test-results-m18-stdio-live-tmux.
  OCTOS_TUI_M18_STDIO_RUN_COMMAND            Optional command used by repeat instead of run-once.
  OCTOS_TUI_M18_STDIO_PROMPT                 Optional prompt to submit during run-once.
  OCTOS_TUI_M18_STDIO_KEEP_SESSION           Set to 1 to leave tmux session running after run-once.

repeat exports these per child:
  OCTOS_TUI_M18_STDIO_RUN_ID
  OCTOS_TUI_M18_STDIO_RUN_INDEX
  OCTOS_TUI_M18_STDIO_RUN_ARTIFACT_DIR
USAGE
}

die() {
  echo "$*" >&2
  exit 1
}

json_escape() {
  local value="$1"
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  value=${value//$'\n'/\\n}
  value=${value//$'\r'/\\r}
  value=${value//$'\t'/\\t}
  printf '%s' "$value"
}

json_string() {
  printf '"%s"' "$(json_escape "$1")"
}

shell_quote() {
  printf '%q' "$1"
}

require_executable() {
  local name="$1"
  local path="$2"
  [ -n "$path" ] || die "$name is unset"
  [ -x "$path" ] || die "$name is not executable: $path"
}

capture_pane() {
  local out="$1"
  mkdir -p "$(dirname "$out")"
  if tmux has-session -t "$tmux_session" 2>/dev/null; then
    tmux capture-pane -t "$tmux_session" -p -J -S -400 > "$out"
  else
    printf 'tmux session not running: %s\n' "$tmux_session" > "$out"
  fi
}

wait_for_capture_text() {
  local pattern="$1"
  local seconds="$2"
  local deadline=$((SECONDS + seconds))
  local capture="$artifact_dir/tui-capture.txt"
  while [ "$SECONDS" -le "$deadline" ]; do
    capture_pane "$capture"
    if grep -E "$pattern" "$capture" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

write_run_summary() {
  local ok="$1"
  local reason="$2"
  local ended_at
  ended_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  cat > "$artifact_dir/run-summary.json" <<EOF
{
  "schema": "octos-tui-m18-stdio-live-run-v1",
  "ok": $ok,
  "reason": $(json_string "$reason"),
  "run_id": $(json_string "$run_id"),
  "ended_at": $(json_string "$ended_at"),
  "artifact_dir": $(json_string "$artifact_dir"),
  "session_id": $(json_string "$session_id"),
  "profile_id": $(json_string "$profile_id"),
  "tmux_session": $(json_string "$tmux_session"),
  "octos_bin": $(json_string "$octos_bin"),
  "octos_tui_bin": $(json_string "$octos_tui_bin"),
  "files": {
    "tui_capture": $(json_string "$artifact_dir/tui-capture.txt"),
    "server_log": $(json_string "$artifact_dir/server.log"),
    "tui_stderr": $(json_string "$artifact_dir/tui-stderr.log")
  }
}
EOF
}

run_once() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for run-once"
  require_executable OCTOS_BIN "$octos_bin"
  require_executable OCTOS_TUI_BIN "$octos_tui_bin"
  if ! "$octos_bin" serve --help >/dev/null 2>&1; then
    die "OCTOS_BIN does not expose 'serve': $octos_bin"
  fi

  mkdir -p "$artifact_dir" "$workspace" "$data_dir"
  local server_log="$artifact_dir/server.log"
  local tui_stderr="$artifact_dir/tui-stderr.log"
  local stdio_command
  stdio_command="bash -lc $(shell_quote "exec $(shell_quote "$octos_bin") serve --stdio --data-dir $(shell_quote "$data_dir") --cwd $(shell_quote "$workspace") 2>$(shell_quote "$server_log")")"
  local tui_cmd
  tui_cmd="cd $(shell_quote "$repo_root") && RUST_LOG=off exec $(shell_quote "$octos_tui_bin") --mode protocol --stdio-command $(shell_quote "$stdio_command") --session $(shell_quote "$session_id") --profile-id $(shell_quote "$profile_id") --cwd $(shell_quote "$workspace") 2>$(shell_quote "$tui_stderr")"

  tmux kill-session -t "$tmux_session" 2>/dev/null || true
  tmux new-session -d -s "$tmux_session" "$tui_cmd"

  local ok=true
  local reason="ready"
  if ! wait_for_capture_text 'Protocol backend connected|Opened .*local|Ask Octos to change code|Sessions' "$ready_wait_secs"; then
    ok=false
    reason="timed out waiting for stdio TUI readiness"
  fi

  if [ "$ok" = true ] && [ -n "${OCTOS_TUI_M18_STDIO_PROMPT:-}" ]; then
    tmux send-keys -t "$tmux_session" Escape
    tmux send-keys -t "$tmux_session" -l "$OCTOS_TUI_M18_STDIO_PROMPT"
    tmux send-keys -t "$tmux_session" Enter
    if ! wait_for_capture_text 'Done|Session Summary|turn completed|error|Error' "$prompt_wait_secs"; then
      ok=false
      reason="timed out waiting for prompted turn to settle"
    fi
  fi

  capture_pane "$artifact_dir/tui-capture.txt"
  if grep -E 'thread .* panicked|panic|stack backtrace' "$artifact_dir/tui-capture.txt" "$tui_stderr" >/dev/null 2>&1; then
    ok=false
    reason="panic marker found in TUI capture or stderr"
  fi

  if [ "${OCTOS_TUI_M18_STDIO_KEEP_SESSION:-0}" != "1" ]; then
    tmux kill-session -t "$tmux_session" 2>/dev/null || true
  fi

  if [ "$ok" = true ]; then
    write_run_summary true "$reason"
    echo "M18 stdio live tmux run passed: $artifact_dir"
  else
    write_run_summary false "$reason"
    echo "M18 stdio live tmux run failed: $reason; artifacts: $artifact_dir" >&2
    return 1
  fi
}

repeat_runs() {
  case "$repeat_count" in
    ''|*[!0-9]*) die "OCTOS_TUI_M18_STDIO_REPEAT_COUNT must be a positive integer" ;;
  esac
  case "$failure_budget" in
    ''|*[!0-9]*) die "OCTOS_TUI_M18_STDIO_FAILURE_BUDGET must be a non-negative integer" ;;
  esac
  [ "$repeat_count" -ge 1 ] || die "OCTOS_TUI_M18_STDIO_REPEAT_COUNT must be >= 1"

  mkdir -p "$artifact_root"
  local report="$artifact_root/$run_id-repeat-report.json"
  local runs_json="$artifact_root/$run_id-repeat-runs.tmp"
  local started_epoch
  local started_at
  started_epoch="$(date +%s)"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  : > "$runs_json"

  local pass_count=0
  local fail_count=0
  local command_string="$run_command"
  if [ -z "$command_string" ]; then
    command_string="$(shell_quote "$script_dir/run-m18-stdio-live-tmux-soak.sh") run-once"
  fi

  local index
  for index in $(seq 1 "$repeat_count"); do
    local child_run_id
    local child_dir
    local child_log
    local started
    local ended
    local duration
    local exit_code=0
    child_run_id="$run_id-run-$(printf '%02d' "$index")"
    child_dir="$artifact_root/$child_run_id"
    child_log="$artifact_root/$child_run_id-repeat.log"
    mkdir -p "$child_dir"
    started="$(date +%s)"
    if env \
      OCTOS_TUI_M18_STDIO_RUN_ID="$child_run_id" \
      OCTOS_TUI_M18_STDIO_RUN_INDEX="$index" \
      OCTOS_TUI_M18_STDIO_RUN_ARTIFACT_DIR="$child_dir" \
      OCTOS_TUI_M18_STDIO_ARTIFACT_ROOT="$artifact_root" \
      bash -lc "$command_string" > "$child_log" 2>&1; then
      pass_count=$((pass_count + 1))
    else
      exit_code=$?
      fail_count=$((fail_count + 1))
    fi
    ended="$(date +%s)"
    duration=$((ended - started))
    if [ "$index" -gt 1 ]; then
      printf ',\n' >> "$runs_json"
    fi
    cat >> "$runs_json" <<EOF
    {
      "index": $index,
      "run_id": $(json_string "$child_run_id"),
      "ok": $([ "$exit_code" -eq 0 ] && echo true || echo false),
      "exit_code": $exit_code,
      "duration_seconds": $duration,
      "artifact_dir": $(json_string "$child_dir"),
      "log": $(json_string "$child_log")
    }
EOF
  done

  local ended_at
  local total_duration
  local ok=false
  ended_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  total_duration=$(($(date +%s) - started_epoch))
  if [ "$fail_count" -le "$failure_budget" ]; then
    ok=true
  fi

  cat > "$report" <<EOF
{
  "schema": "octos-tui-m18-stdio-live-flake-budget-v1",
  "ok": $ok,
  "run_id": $(json_string "$run_id"),
  "started_at": $(json_string "$started_at"),
  "ended_at": $(json_string "$ended_at"),
  "duration_seconds": $total_duration,
  "repeat_count": $repeat_count,
  "pass_count": $pass_count,
  "fail_count": $fail_count,
  "failure_budget": $failure_budget,
  "command": $(json_string "$command_string"),
  "artifact_root": $(json_string "$artifact_root"),
  "runs": [
$(cat "$runs_json")
  ]
}
EOF
  rm -f "$runs_json"

  echo "M18 stdio repeat report: $report"
  if [ "$ok" = true ]; then
    return 0
  fi
  return 1
}

self_test() {
  local tmp_root
  tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/octos-tui-m18-stdio-repeat.XXXXXX")"
  local command='mkdir -p "$OCTOS_TUI_M18_STDIO_RUN_ARTIFACT_DIR"; printf "synthetic capture %s\n" "$OCTOS_TUI_M18_STDIO_RUN_INDEX" > "$OCTOS_TUI_M18_STDIO_RUN_ARTIFACT_DIR/tui-capture.txt"; if [ "$OCTOS_TUI_M18_STDIO_RUN_INDEX" = "2" ]; then exit 7; fi'
  OCTOS_TUI_M18_STDIO_RUN_ID=selftest \
    OCTOS_TUI_M18_STDIO_ARTIFACT_ROOT="$tmp_root" \
    OCTOS_TUI_M18_STDIO_REPEAT_COUNT=3 \
    OCTOS_TUI_M18_STDIO_FAILURE_BUDGET=1 \
    OCTOS_TUI_M18_STDIO_RUN_COMMAND="$command" \
    "$0" repeat >/dev/null

  local report="$tmp_root/selftest-repeat-report.json"
  [ -f "$report" ] || die "self-test missing repeat report"
  grep -F '"pass_count": 2' "$report" >/dev/null || die "self-test pass_count mismatch"
  grep -F '"fail_count": 1' "$report" >/dev/null || die "self-test fail_count mismatch"
  grep -F '"ok": true' "$report" >/dev/null || die "self-test budget ok mismatch"
  [ -f "$tmp_root/selftest-run-02/tui-capture.txt" ] || die "self-test failed run artifact missing"
  rm -rf "$tmp_root"
  echo "Self-test passed"
}

case "${1:-help}" in
  run-once) run_once ;;
  repeat) repeat_runs ;;
  self-test) self_test ;;
  help|-h|--help) usage ;;
  *) usage; exit 2 ;;
esac
