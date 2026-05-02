#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo test --test appui_ux_fixture

if [[ "${OCTOS_TUI_UX_LIVE_SOAK:-0}" == "1" ]]; then
  if [[ -z "${OCTOS_TUI_PROTOCOL_ENDPOINT:-}" ]]; then
    echo "OCTOS_TUI_PROTOCOL_ENDPOINT is required for live soak" >&2
    exit 2
  fi

  run_id="$(date -u +%Y%m%dT%H%M%SZ)"
  artifact_dir="${OCTOS_TUI_UX_ARTIFACT_DIR:-e2e/test-results-tui-coding-ux/$run_id}"
  mkdir -p "$artifact_dir"
  cat >"$artifact_dir/README.txt" <<EOF
Manual AppUI UX live soak placeholder

Duration target: 60 minutes
Endpoint: $OCTOS_TUI_PROTOCOL_ENDPOINT

Run the parent octos tmux harness against this octos-tui checkout and retain:
- raw and cleaned octos-tui capture
- server log
- worktree diff and git status
- cargo/test validation log
- state-matrix assertion summary
EOF
  echo "Prepared live soak artifact dir: $artifact_dir"
  echo "Use the parent octos tmux harness for the one-hour live run."
fi
