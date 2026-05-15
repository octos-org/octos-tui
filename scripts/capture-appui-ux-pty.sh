#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

run_id="$(date -u +%Y%m%dT%H%M%SZ)"
artifact_dir="${OCTOS_TUI_UX_CAPTURE_DIR:-e2e/test-results-tui-ux-capture/$run_id}"
fixture="fixtures/appui_ux_parity/coding_session_short.json"
raw_capture="$artifact_dir/appui-ux-fixture.pty.txt"
summary="$artifact_dir/summary.env"

mkdir -p "$artifact_dir"

run_cmd="cargo test --test appui_ux_fixture -- --nocapture"
pty_capture=0
if command -v script >/dev/null 2>&1; then
  pty_capture=1
  if script --version >/dev/null 2>&1; then
    script -q -e -c "$run_cmd" "$raw_capture"
  else
    script -q "$raw_capture" /bin/sh -lc "$run_cmd"
  fi
else
  /bin/sh -lc "$run_cmd" 2>&1 | tee "$raw_capture"
fi

cp "$fixture" "$artifact_dir/coding_session_short.json"

: >"$summary"
emit() {
  printf '%s=%s\n' "$1" "$2" >>"$summary"
}

failures=0
require_marker() {
  local key="$1"
  local pattern="$2"
  local path="${3:-$fixture}"

  if rg -q "$pattern" "$path"; then
    emit "$key" 1
  else
    emit "$key" 0
    failures=$((failures + 1))
  fi
}

emit provider_free 1
emit pty_capture "$pty_capture"
emit artifact_dir "$artifact_dir"

require_marker test_result_ok 'test result: ok' "$raw_capture"
require_marker websocket_stdio_parity 'websocket_and_stdio_records_normalize_to_same_semantics' "$raw_capture"
require_marker runtime_policy_stamp_seen '"runtime_policy_stamp"'
require_marker tool_timeline_seen '"event": "activity.tool.progress"'
require_marker typed_approval_seen '"typed_kind": "diff"'
require_marker typed_denial_seen '"event": "tool.denied"'
require_marker validator_failed_seen '"event": "validator.failed"'
require_marker validator_passed_seen '"event": "validator.passed"'
require_marker diff_ready_seen '"event": "diff.preview.ready"'
require_marker artifact_ready_seen '"event": "artifact.ready"'
require_marker interrupt_seen '"event": "turn.interrupt.request"'
require_marker reconnect_seen '"cursor_after_seq": 28'
require_marker long_output_folded '"long_output": true'
require_marker narrow_layout_ok '"narrow_layout_ok": true'
require_marker approval_prompt '"event": "approval.requested"'
require_marker approval_blocks_until_decision '"approval_must_block_until_decision": true'
require_marker inline_diff_ready '"event": "diff.preview.ready"'
require_marker long_diff_folded '"long_diff": true'
require_marker command_output_delta '"event": "task.output_delta"'
require_marker status_update '"event": "status.update"'
require_marker tool_card_labels '"activity_label": "Testing"'

if [[ "${OCTOS_TUI_CAPTURE_SELF_TEST:-0}" == "1" ]]; then
  if rg -q '__octos_missing_marker_self_test__' "$fixture"; then
    emit self_test_detected_missing_marker 0
    failures=$((failures + 1))
  else
    emit self_test_detected_missing_marker 1
  fi
fi

cat "$summary"

if (( failures > 0 )); then
  echo "AppUI UX PTY capture failed marker validation; see $summary" >&2
  exit 1
fi

echo "Captured AppUI UX PTY fixture artifacts in $artifact_dir"
