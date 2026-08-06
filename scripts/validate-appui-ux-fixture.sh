#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo test --test appui_ux_fixture

scripts/validate-tmux-ux-capture.sh fixtures/tui_ux_captures/reported_bugs_good.txt
if scripts/validate-tmux-ux-capture.sh \
  fixtures/tui_ux_captures/reported_bugs_bad_stuck.txt \
  fixtures/tui_ux_captures/reported_bugs_bad_server.log >/tmp/octoscode-ux-bad-fixture.out 2>&1; then
  cat /tmp/octoscode-ux-bad-fixture.out >&2
  echo "bad tmux UX fixture unexpectedly passed" >&2
  exit 1
else
  echo "Bad tmux UX fixture correctly failed validator"
fi

if [[ -n "${OCTOSCODE_UX_CAPTURE_FILE:-}" ]]; then
  if [[ -n "${OCTOSCODE_UX_SERVER_LOG:-}" ]]; then
    scripts/validate-tmux-ux-capture.sh "$OCTOSCODE_UX_CAPTURE_FILE" "$OCTOSCODE_UX_SERVER_LOG"
  else
    scripts/validate-tmux-ux-capture.sh "$OCTOSCODE_UX_CAPTURE_FILE"
  fi
fi

if [[ "${OCTOSCODE_UX_LIVE_SOAK:-0}" == "1" ]]; then
  if [[ -z "${OCTOSCODE_PROTOCOL_ENDPOINT:-}" ]]; then
    echo "OCTOSCODE_PROTOCOL_ENDPOINT is required for live soak" >&2
    exit 2
  fi

  run_id="$(date -u +%Y%m%dT%H%M%SZ)"
  artifact_dir="${OCTOSCODE_UX_ARTIFACT_DIR:-e2e/test-results-tui-coding-ux/$run_id}"
  mkdir -p \
    "$artifact_dir/transcripts" \
    "$artifact_dir/logs" \
    "$artifact_dir/policy" \
    "$artifact_dir/timeline" \
    "$artifact_dir/approvals" \
    "$artifact_dir/validators" \
    "$artifact_dir/diffs" \
    "$artifact_dir/artifacts" \
    "$artifact_dir/captures"
  cat >"$artifact_dir/README.txt" <<EOF
Manual AppUI UX live soak placeholder

Duration target: 60 minutes
Endpoint: $OCTOSCODE_PROTOCOL_ENDPOINT
Issues covered: octoscode#21, octoscode#22, octoscode#24

Required live matrix:
- WebSocket short reconnect lane with runtime policy stamp, tool timeline, diff, artifact, and typed approval markers.
- stdio short reconnect lane with the same normalized markers.
- long lane that exercises reconnect after replay loss and an in-flight interrupt.
- safety lane that emits a typed approval and a typed tool_denied policy result.
- validator lane that records at least one failed validator followed by a passing rerun.
- narrow terminal lane, 80x24 or smaller, with no overlap in cockpit/timeline/safety panes.

Run the parent octos tmux harness against this octoscode checkout and retain:
- transcripts/appui-transcript.jsonl
- logs/server.log
- policy/runtime-policy-stamp.json
- timeline/tool-timeline.jsonl
- approvals/approval-events.jsonl
- approvals/denial-events.jsonl
- validators/validator-events.jsonl
- diffs/diff-ready.json
- artifacts/artifact-ready.json
- captures/tui-capture.txt
- captures/tui-capture-clean.txt
- scripts/validate-tmux-ux-capture.sh captures/tui-capture-clean.txt logs/server.log
- validation.log
- git-status.txt
- worktree-diff.patch
- summary.env with all capture-appui-ux-pty marker names
EOF
  echo "Prepared live soak artifact dir: $artifact_dir"
  echo "Use the parent octos tmux harness for the one-hour live run."
fi
