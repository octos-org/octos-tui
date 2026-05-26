#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
octos_repo="${OCTOS_REPO:-$(cd "$repo_root/../octos" 2>/dev/null && pwd || true)}"

run_id="${OCTOS_TUI_SOAK_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
artifact_root="${OCTOS_TUI_SOAK_ARTIFACT_ROOT:-$repo_root/e2e/test-results-tui-onboarding}"
artifact_dir="${OCTOS_TUI_SOAK_ARTIFACT_DIR:-$artifact_root/$run_id}"
runtime_root="${OCTOS_TUI_SOAK_RUNTIME_ROOT:-/tmp/octos-tui-onboarding-$run_id}"
workspace="${OCTOS_TUI_SOAK_WORKSPACE:-$runtime_root/workspace}"
data_dir="${OCTOS_TUI_SOAK_DATA_DIR:-$runtime_root/data}"
logs_dir="${OCTOS_TUI_SOAK_LOGS_DIR:-$runtime_root/logs}"

octos_bin="${OCTOS_BIN:-${octos_repo:+$octos_repo/target/debug/octos}}"
octos_tui_bin="${OCTOS_TUI_BIN:-$repo_root/target/debug/octos-tui}"
transport="${OCTOS_TUI_SOAK_TRANSPORT:-ws}"
if [ "$transport" = "stdio" ]; then
  default_solo_probe_data_dir="$runtime_root/solo-probe-data"
else
  default_solo_probe_data_dir="$data_dir"
fi
solo_probe_data_dir="${OCTOS_TUI_SOAK_SOLO_PROBE_DATA_DIR:-$default_solo_probe_data_dir}"
solo_probe_server_log="${OCTOS_TUI_SOAK_SOLO_PROBE_SERVER_LOG:-$logs_dir/solo-probe-server.log}"
host="${OCTOS_TUI_SOAK_HOST:-127.0.0.1}"
port="${OCTOS_TUI_SOAK_PORT:-50179}"
auth_token="${OCTOS_TUI_SOAK_AUTH_TOKEN:-octos-tui-onboarding-soak-token}"
profile_id="${OCTOS_TUI_SOAK_PROFILE:-coding}"
session_id="${OCTOS_TUI_SOAK_SESSION:-$profile_id:local:onboarding#$run_id}"
open_session="${OCTOS_TUI_SOAK_OPEN_SESSION:-auto}"
theme="${OCTOS_TUI_SOAK_THEME:-codex}"
serve_args="${OCTOS_TUI_SOAK_SERVE_ARGS:-}"
server_session="${OCTOS_TUI_SOAK_SERVER_SESSION:-octos-onboard-server-$run_id}"
tui_session="${OCTOS_TUI_SOAK_TUI_SESSION:-octos-onboard-tui-$run_id}"
fake_openai="${OCTOS_TUI_SOAK_FAKE_OPENAI:-0}"
fake_openai_host="${OCTOS_TUI_SOAK_FAKE_OPENAI_HOST:-127.0.0.1}"
fake_openai_port="${OCTOS_TUI_SOAK_FAKE_OPENAI_PORT:-50180}"
fake_openai_session="${OCTOS_TUI_SOAK_FAKE_OPENAI_SESSION:-octos-onboard-fake-openai-$run_id}"
fake_openai_delay_secs="${OCTOS_TUI_SOAK_FAKE_OPENAI_DELAY_SECS:-0}"
provider_env_vars="${OCTOS_TUI_SOAK_PROVIDER_ENV_VARS:-OPENAI_API_KEY ANTHROPIC_API_KEY DEEPSEEK_API_KEY OPENROUTER_API_KEY MOONSHOT_API_KEY KIMI_API_KEY AUTODL_API_KEY}"
require_live_provider="${OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER:-1}"
transport_parity_mode="${OCTOS_TUI_SOAK_TRANSPORT_PARITY_MODE:-sequence}"
first_launch_capture="${OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE:-0}"
endpoint="ws://$host:$port/api/ui-protocol/ws"

usage() {
  cat <<'USAGE'
Usage: scripts/run-onboarding-tmux-soak.sh <preflight-live|start|restart-server|drive-onboard|drive-solo|drive-permissions|drive-provider-missing|drive-approval-denial|drive-multiline-composer|drive-runtime-menus|drive-task-subagent-tree|drive-task-subagent-reconnect|drive-task-subagent-old-server-fallback|drive-autonomy-live|drive-autonomy-reconnect|drive-dropped-completion-backpressure|drive-interrupt-reconnect|drive-validator-cycle|drive-long-output|drive-narrow-terminal|drive-diff-artifact|drive-tool-denial|drive-tool-success|capture|send-turn|verify|verify-onboard|verify-solo|verify-solo-closure|verify-solo-transport-closure|verify-first-launch|verify-provider-missing|verify-permissions|verify-approval-denial|verify-multiline-composer|verify-runtime-menus|verify-task-subagent-tree|verify-task-subagent-reconnect|verify-task-subagent-old-server-fallback|verify-task-subagent-closure|verify-backpressure|verify-interrupt-reconnect|verify-validator-cycle|verify-long-output|verify-narrow-terminal|verify-diff-artifact|verify-tool-denial|verify-tool-success|verify-autonomy-live|verify-autonomy-reconnect|verify-autonomy-closure|verify-transport-parity|verify-ux-run|api-parity|self-test|solo-self-test|stop|help>

Environment:
  OCTOS_REPO                     Path to sibling octos checkout.
  OCTOS_BIN                      octos backend binary.
  OCTOS_TUI_BIN                  octos-tui binary.
  OCTOS_TUI_SOAK_TRANSPORT       ws or stdio, default ws.
  OCTOS_TUI_SOAK_RUN_ID          Stable run id for repeated capture/verify.
  OCTOS_TUI_SOAK_RUNTIME_ROOT    Runtime workspace/data/log root used by tmux children, default /tmp/octos-tui-onboarding-$run_id.
  OCTOS_TUI_SOAK_PORT            Backend port, default 50179.
  OCTOS_TUI_SOAK_PROFILE         Profile id, default coding.
  OCTOS_TUI_SOAK_OPEN_SESSION    1, 0, or auto. auto skips session/open until a profile JSON exists.
  OCTOS_TUI_SOAK_SERVE_ARGS      Extra octos serve args.
  OCTOS_TUI_SOAK_EXPECT_FAMILY   Optional family_id expected in profile JSON.
  OCTOS_TUI_SOAK_EXPECT_MODEL    Optional model_id expected in redacted profile JSON.
  OCTOS_TUI_SOAK_EXPECT_ROUTE    Optional route.route_id expected in profile JSON.
  OCTOS_TUI_SOAK_EXPECT_BASE_URL Optional route.base_url expected in profile JSON.
  OCTOS_TUI_SOAK_API_KEY         Optional secret string checked for capture leaks.
  OCTOS_TUI_SOAK_PROVIDER_ENV_VARS Space/comma-separated provider key env vars
                                 accepted by preflight-live.
  OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER Set to 0 for provider-free dry-run preflight.
  OCTOS_TUI_SOAK_INIT_PROFILE_LLM Set to 1 to pre-seed profile JSON before backend bootstraps.
  OCTOS_TUI_SOAK_TENANT_NEGATIVE Set to 1 to also run tenant/cloud dangerous-mode negative probe.
  OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE Set to 1 during verify-solo to require
                                 a passed tenant/cloud dangerous-mode rejection row.
  OCTOS_TUI_SOAK_SOLO_PROBE_DATA_DIR Optional separate data dir for stdio solo probe.
  OCTOS_TUI_SOAK_FAKE_OPENAI     Set to 1 to start scripts/fake-openai-server.py in tmux.
  OCTOS_TUI_SOAK_FAKE_OPENAI_PORT Local fake OpenAI-compatible port, default 50180.
  OCTOS_TUI_SOAK_FAKE_OPENAI_DELAY_SECS Optional fake API response delay for progress captures.
  OCTOS_TUI_SOAK_MULTILINE_PROMPT Optional multiline composer text used by
                                 drive-multiline-composer.
  OCTOS_TUI_SOAK_INTERRUPT_PROMPT Optional long-running prompt used by
                                 drive-interrupt-reconnect.
  OCTOS_TUI_SOAK_VALIDATOR_PROMPT Optional prompt used by
                                 drive-validator-cycle.
  OCTOS_TUI_SOAK_LONG_OUTPUT_PROMPT Optional prompt used by
                                 drive-long-output.
  OCTOS_TUI_SOAK_NARROW_COLS    Narrow terminal columns, default 80.
  OCTOS_TUI_SOAK_NARROW_ROWS    Narrow terminal rows, default 24.
  OCTOS_TUI_SOAK_DIFF_ARTIFACT_PROMPT Optional prompt used by
                                 drive-diff-artifact.
  OCTOS_TUI_SOAK_TOOL_DENIAL_PROMPT Optional prompt used by
                                 drive-tool-denial.
  OCTOS_TUI_SOAK_TOOL_SUCCESS_PROMPT Optional prompt used by
                                 drive-tool-success.
  OCTOS_TUI_SOAK_AUTONOMY_GOAL Optional /goal objective used by
                                 drive-autonomy-live.
  OCTOS_TUI_SOAK_AUTONOMY_LOOP_ID Optional loop id used by
                                 drive-autonomy-live for fire-now/pause/resume.
  OCTOS_TUI_SOAK_AUTONOMY_AGENT_ID Optional agent id used by
                                 drive-autonomy-live for status/output/artifacts.
  OCTOS_TUI_M15_UX_OUTPUT_DIR    Optional live M15 evidence directory copied
                                 into the retained artifact bundle.
  OCTOS_TUI_SOAK_WS_ARTIFACT_DIR WebSocket artifact dir for transport parity
                                 and verify-solo-transport-closure.
  OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR Stdio artifact dir for transport parity
                                 and verify-solo-transport-closure.
  OCTOS_TUI_SOAK_TRANSPORT_PARITY_MODE sequence or set, default sequence.
  OCTOS_TUI_SOAK_TASK_RECONNECT_ARTIFACT_DIR Optional reconnect artifact dir
                                 used by verify-task-subagent-closure.
  OCTOS_TUI_SOAK_TASK_OLD_SERVER_ARTIFACT_DIR Optional old-server fallback
                                 artifact dir used by verify-task-subagent-closure.
  OCTOS_TUI_SOAK_AUTONOMY_RECONNECT_ARTIFACT_DIR Optional reconnect artifact
                                 dir used by verify-autonomy-closure.
  OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE Set to 1 to launch without a preselected
                                 profile/session and save tui-capture-first-launch.txt.
  OCTOS_TUI_SOAK_REQUIRE_PROFILE Set to 0 to allow verify without profile JSON.
  OCTOS_TUI_SOAK_SOLO_STRICT     Set to 1 to fail when M12-A/C capability blockers remain.
                                 Also requires MCP/tool fixture mutations to pass
                                 when the backend advertises those methods.
  OCTOS_TUI_SOAK_REQUIRED_SOLO_CASES Space/comma-separated case names that
                                 must be status=ok in verify-solo.
  OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR Optional multiline artifact dir used
                                 by verify-solo-closure.

Interactive flow after start:
  1. Attach: tmux attach -t "$OCTOS_TUI_SOAK_TUI_SESSION"
  2. For M12 local solo no-OTP evidence use:
       scripts/run-onboarding-tmux-soak.sh drive-solo
  3. For legacy provider onboarding, run /onboard. For automated smoke use:
       scripts/run-onboarding-tmux-soak.sh drive-onboard
  4. Complete OTP if the server is not already authenticated by token.
  5. Select a dashboard-catalog provider route and save it as primary.
  6. Run /model and verify it renders server-returned profile models/catalog.
  7. Ask a short prompt, then run verify.
USAGE
}

die() {
  echo "$*" >&2
  exit 1
}

case "$transport" in
  ws|stdio) ;;
  *) die "OCTOS_TUI_SOAK_TRANSPORT must be ws or stdio, got: $transport" ;;
esac

require_bin() {
  local name="$1"
  local value="$2"
  if [ -z "$value" ] || [ ! -x "$value" ]; then
    die "$name is not executable: ${value:-<unset>}"
  fi
}

require_octos_serve() {
  require_bin OCTOS_BIN "$octos_bin"
  if ! "$octos_bin" serve --help >/dev/null 2>&1; then
    die "OCTOS_BIN does not expose 'serve'; build octos-cli with the api feature or set OCTOS_BIN to an API-enabled binary"
  fi
}

profile_has_provider_secret() {
  local profile_path="$data_dir/profiles/$profile_id.json"
  [ -f "$profile_path" ] || return 1
  if command -v jq >/dev/null 2>&1; then
    jq -e '
      (.config.env_vars // .env_vars // {})
      | type == "object"
      and any(to_entries[]?; (.value | type == "string" and length > 0))
    ' "$profile_path" >/dev/null 2>&1
  else
    grep -E '"env_vars"[[:space:]]*:' "$profile_path" >/dev/null 2>&1 \
      && grep -E '"[^"]+"[[:space:]]*:[[:space:]]*"[^"]+"' "$profile_path" >/dev/null 2>&1
  fi
}

git_commit_for_dir() {
  local dir="$1"
  [ -n "$dir" ] || return 0
  git -C "$dir" rev-parse HEAD 2>/dev/null || true
}

provider_credential_source() {
  if [ -n "${OCTOS_TUI_SOAK_API_KEY:-}" ]; then
    printf 'OCTOS_TUI_SOAK_API_KEY\n'
    return 0
  fi

  local env_names="${provider_env_vars//,/ }"
  local env_name
  for env_name in $env_names; do
    if [ -n "${!env_name:-}" ]; then
      printf '%s\n' "$env_name"
      return 0
    fi
  done

  if profile_has_provider_secret; then
    printf 'profile env_vars for %s\n' "$profile_id"
    return 0
  fi

  return 1
}

write_live_preflight_json() {
  local status="$1"
  local failure="$2"
  local provider_source="$3"
  local tmux_check="$4"
  local octos_check="$5"
  local tui_check="$6"
  local tmux_version="$7"
  local tmux_version_status="$8"
  local octos_version="$9"
  local octos_version_status="${10}"
  local octos_tui_version="${11}"
  local octos_tui_version_status="${12}"
  local octos_repo_commit="${13}"
  local octos_tui_repo_commit="${14}"
  local host_name="${15}"
  local os_release="${16}"
  mkdir -p "$artifact_dir"
  {
    printf '{\n'
    write_json_string_field schema "octos-tui.live-preflight.v1"
    write_json_string_field run_id "$run_id"
    write_json_string_field status "$status"
    write_json_string_field transport "$transport"
    write_json_string_field artifact_dir "$artifact_dir"
    write_json_string_field profile_id "$profile_id"
    write_json_string_field session_id "$session_id"
    write_json_string_field open_session "$open_session"
    write_json_string_field runtime_root "$runtime_root"
    write_json_string_field workspace "$workspace"
    write_json_string_field data_dir "$data_dir"
    write_json_string_field host "$host_name"
    write_json_string_field os "$os_release"
    write_json_string_field tmux "$tmux_check"
    write_json_string_field tmux_version "$tmux_version"
    write_json_string_field tmux_version_status "$tmux_version_status"
    write_json_string_field octos_serve "$octos_check"
    write_json_string_field octos_bin "$octos_bin"
    write_json_string_field octos_version "$octos_version"
    write_json_string_field octos_version_status "$octos_version_status"
    write_json_string_field octos_repo_commit "$octos_repo_commit"
    write_json_string_field octos_tui "$tui_check"
    write_json_string_field octos_tui_bin "$octos_tui_bin"
    write_json_string_field octos_tui_version "$octos_tui_version"
    write_json_string_field octos_tui_version_status "$octos_tui_version_status"
    write_json_string_field octos_tui_repo_commit "$octos_tui_repo_commit"
    write_json_string_field provider_credential "$provider_source"
    write_json_string_field provider_env_vars_checked "$provider_env_vars"
    write_json_string_field require_live_provider "$require_live_provider"
    write_json_string_field failure "$failure"
    write_json_string_field generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" ""
    printf '}\n'
  } > "$artifact_dir/live-preflight.json"
}

preflight_live() {
  local status="passed"
  local failure=""
  local tmux_check="passed"
  local octos_check="passed"
  local tui_check="passed"
  local tmux_version=""
  local tmux_version_status=""
  local octos_version=""
  local octos_version_status=""
  local octos_tui_version=""
  local octos_tui_version_status=""
  local octos_repo_commit=""
  local octos_tui_repo_commit=""
  local host_name=""
  local os_release=""
  local provider_source=""

  host_name="$(hostname 2>/dev/null || true)"
  os_release="$(uname -a 2>/dev/null || true)"
  octos_repo_commit="$(git_commit_for_dir "$octos_repo")"
  octos_tui_repo_commit="$(git_commit_for_dir "$repo_root")"

  if ! command -v tmux >/dev/null 2>&1; then
    tmux_check="missing"
    tmux_version_status="missing"
    status="failed"
    failure="tmux is required for live soak"
  elif tmux_version="$(tmux -V 2>/dev/null)"; then
    tmux_version_status="passed"
  else
    tmux_version="unsupported"
    tmux_version_status="unsupported"
  fi

  if [ -z "$octos_bin" ] || [ ! -x "$octos_bin" ]; then
    octos_check="not executable"
    octos_version_status="not executable"
    status="failed"
    [ -n "$failure" ] || failure="OCTOS_BIN is not executable: ${octos_bin:-<unset>}"
  elif ! "$octos_bin" serve --help >/dev/null 2>&1; then
    octos_check="missing serve"
    octos_version_status="missing serve"
    status="failed"
    [ -n "$failure" ] || failure="OCTOS_BIN does not expose 'serve'; build octos-cli with the api feature or set OCTOS_BIN to an API-enabled binary"
  elif octos_version="$("$octos_bin" --version 2>/dev/null)"; then
    octos_version_status="passed"
  else
    octos_version="unsupported"
    octos_version_status="unsupported"
  fi

  if [ -z "$octos_tui_bin" ] || [ ! -x "$octos_tui_bin" ]; then
    tui_check="not executable"
    octos_tui_version_status="not executable"
    status="failed"
    [ -n "$failure" ] || failure="OCTOS_TUI_BIN is not executable: ${octos_tui_bin:-<unset>}"
  elif octos_tui_version="$("$octos_tui_bin" --version 2>/dev/null)"; then
    octos_tui_version_status="passed"
  else
    octos_tui_version="unsupported"
    octos_tui_version_status="unsupported"
  fi

  if [ "$require_live_provider" != "0" ]; then
    provider_source="$(provider_credential_source || true)"
    if [ -z "$provider_source" ]; then
      provider_source="missing"
      status="failed"
      [ -n "$failure" ] || failure="no provider credential found. Set OCTOS_TUI_SOAK_API_KEY, one of OCTOS_TUI_SOAK_PROVIDER_ENV_VARS, or pre-seed $data_dir/profiles/$profile_id.json with profile env_vars. Set OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER=0 only for provider-free dry runs."
    fi
  else
    provider_source="not required"
  fi

  write_live_preflight_json "$status" "$failure" "$provider_source" "$tmux_check" "$octos_check" "$tui_check" "$tmux_version" "$tmux_version_status" "$octos_version" "$octos_version_status" "$octos_tui_version" "$octos_tui_version_status" "$octos_repo_commit" "$octos_tui_repo_commit" "$host_name" "$os_release"

  if [ "$status" != "passed" ]; then
    die "Live closure preflight failed: $failure (artifact: $artifact_dir/live-preflight.json)"
  fi

  printf 'Live soak preflight passed\n'
  printf 'transport=%s\n' "$transport"
  printf 'host=%s\n' "$host_name"
  printf 'os=%s\n' "$os_release"
  printf 'tmux_version=%s\n' "$tmux_version"
  printf 'tmux_version_status=%s\n' "$tmux_version_status"
  printf 'octos_bin=%s\n' "$octos_bin"
  printf 'octos_version=%s\n' "$octos_version"
  printf 'octos_version_status=%s\n' "$octos_version_status"
  printf 'octos_repo_commit=%s\n' "$octos_repo_commit"
  printf 'octos_tui_bin=%s\n' "$octos_tui_bin"
  printf 'octos_tui_version=%s\n' "$octos_tui_version"
  printf 'octos_tui_version_status=%s\n' "$octos_tui_version_status"
  printf 'octos_tui_repo_commit=%s\n' "$octos_tui_repo_commit"
  printf 'provider_credential=%s\n' "$provider_source"
  printf 'provider_env_vars_checked=%s\n' "$provider_env_vars"
  printf 'artifact=%s\n' "$artifact_dir/live-preflight.json"
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

write_json_string_field() {
  local name="$1"
  local value="$2"
  local suffix=","
  if [ "$#" -ge 3 ]; then
    suffix="$3"
  fi
  printf '  "%s": "%s"%s\n' "$name" "$(json_escape "$value")" "$suffix"
}

shell_quote() {
  printf '%q' "$1"
}

have_tmux() {
  command -v tmux >/dev/null 2>&1
}

capture_pane() {
  local session="$1"
  local out="$2"
  mkdir -p "$(dirname "$out")"
  if ! have_tmux; then
    printf 'tmux unavailable; capture skipped for session: %s\n' "$session" > "$out"
  elif tmux has-session -t "$session" 2>/dev/null; then
    tmux capture-pane -t "$session" -p -J -S -300 > "$out"
  else
    printf 'tmux session not running: %s\n' "$session" > "$out"
  fi
}

wait_for_tui_text() {
  local pattern="$1"
  local timeout_secs="${2:-20}"
  local deadline=$((SECONDS + timeout_secs))
  local snapshot="$artifact_dir/tui-capture.txt"
  # The pattern may be a single fixed string OR a `|`-separated alternation
  # like "Welcome to Octos|Set Up LLM Provider|AppUI capabilities refreshed"
  # — this matters for ratatui alt-screen apps where the picker overlay can
  # transition between states before tmux capture-pane catches any single
  # one. `grep --fixed-strings -F` treats EACH line of the pattern as its
  # own literal needle, so we feed the alternation in line-separated form.
  local pattern_lines
  pattern_lines="$(printf '%s\n' "$pattern" | tr '|' '\n')"
  while [ "$SECONDS" -le "$deadline" ]; do
    if ! tmux has-session -t "$tui_session" 2>/dev/null; then
      die "TUI tmux session exited while waiting for: $pattern"
    fi
    capture_pane "$tui_session" "$snapshot"
    if printf '%s\n' "$pattern_lines" | grep --fixed-strings --file=- "$snapshot" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

submit_composer_prompt() {
  local prompt="$1"
  local buffer="octos-tui-soak-prompt-$run_id"
  local tmp
  tmp="$(mktemp "${TMPDIR:-/tmp}/octos-tui-prompt.XXXXXX")"
  printf '%s' "$prompt" > "$tmp"
  tmux send-keys -t "$tui_session" Escape
  sleep 0.2
  tmux load-buffer -b "$buffer" "$tmp"
  rm -f "$tmp"
  tmux paste-buffer -p -t "$tui_session" -b "$buffer"
  tmux delete-buffer -b "$buffer" >/dev/null 2>&1 || true
  sleep "${OCTOS_TUI_SOAK_PROMPT_SETTLE_SECS:-0.35}"
  tmux send-keys -t "$tui_session" Enter
}

capture_scrolled_transcript_until_text() {
  local pattern="$1"
  local out="$2"
  local max_pages="${3:-6}"
  local page=1
  while [ "$page" -le "$max_pages" ]; do
    tmux send-keys -t "$tui_session" PageUp
    sleep 0.2
    capture_pane "$tui_session" "$out"
    if grep --fixed-strings -- "$pattern" "$out" >/dev/null 2>&1; then
      return 0
    fi
    page=$((page + 1))
  done
  return 1
}

server_socket_ready() {
  (exec 3<>"/dev/tcp/$host/$port") >/dev/null 2>&1
}

wait_for_server_ready() {
  local timeout_secs="${1:-20}"
  local deadline=$((SECONDS + timeout_secs))
  local ready_line="Listening: http://$host:$port"
  while [ "$SECONDS" -le "$deadline" ]; do
    if grep --fixed-strings -- "$ready_line" "$logs_dir/server.log" >/dev/null 2>&1 \
      && server_socket_ready; then
      sleep 0.2
      if ! tmux has-session -t "$server_session" 2>/dev/null; then
        capture_pane "$server_session" "$artifact_dir/server-pane.txt"
        die "Backend tmux session exited after reporting readiness: $server_session"
      fi
      return 0
    fi
    if grep -E 'Address already in use|bind error|panicked at|error binding|failed to bind' \
      "$logs_dir/server.log" >/dev/null 2>&1; then
      capture_pane "$server_session" "$artifact_dir/server-pane.txt"
      die "Backend server failed before readiness; see $logs_dir/server.log"
    fi
    if ! tmux has-session -t "$server_session" 2>/dev/null; then
      capture_pane "$server_session" "$artifact_dir/server-pane.txt"
      die "Backend tmux session exited before WebSocket server became ready: $server_session"
    fi
    sleep 1
  done
  capture_pane "$server_session" "$artifact_dir/server-pane.txt"
  die "Timed out waiting for WebSocket server readiness line: $ready_line"
}

redact_profile() {
  local input="$1"
  local output="$2"
  if [ ! -f "$input" ]; then
    return 0
  fi
  mkdir -p "$(dirname "$output")"
  if command -v jq >/dev/null 2>&1; then
    jq 'if .config.env_vars and (.config.env_vars | type == "object") then .config.env_vars |= with_entries(.value = "<redacted>") else . end' \
      "$input" > "$output.tmp"
    mv "$output.tmp" "$output"
  else
    awk '
      BEGIN { in_env = 0; depth = 0; comma = "" }
      /"env_vars"[[:space:]]*:[[:space:]]*\{/ {
        in_env = 1
        comma = ($0 ~ /\}[[:space:]]*,/) ? "," : ""
        depth = gsub(/\{/, "{") - gsub(/\}/, "}")
        if (depth <= 0) {
          print "    \"env_vars\": {\"_redacted\":\"jq unavailable\"}" comma
          in_env = 0
        }
        next
      }
      in_env {
        comma = ($0 ~ /\}[[:space:]]*,/) ? "," : ""
        depth += gsub(/\{/, "{") - gsub(/\}/, "}")
        if (depth <= 0) {
          print "    \"env_vars\": {\"_redacted\":\"jq unavailable\"}" comma
          in_env = 0
        }
        next
      }
      { print }
    ' "$input" > "$output.tmp"
    mv "$output.tmp" "$output"
  fi
}

profile_value() {
  local file="$1"
  local field="$2"
  if [ ! -f "$file" ]; then
    return 1
  fi
  if command -v jq >/dev/null 2>&1; then
    case "$field" in
      family_id) jq -r '.config.llm.primary.family_id // .llm.primary.family_id // .primary.family_id // empty' "$file" ;;
      model_id) jq -r '.config.llm.primary.model_id // .llm.primary.model_id // .primary.model_id // empty' "$file" ;;
      route_id) jq -r '.config.llm.primary.route.route_id // .llm.primary.route.route_id // .primary.route.route_id // empty' "$file" ;;
      base_url) jq -r '.config.llm.primary.route.base_url // .llm.primary.route.base_url // .primary.route.base_url // empty' "$file" ;;
      *) return 1 ;;
    esac
  else
    case "$field" in
      family_id|model_id|route_id|base_url)
        sed -n -E "s/.*\"$field\"[[:space:]]*:[[:space:]]*\"([^\"]*)\".*/\1/p" "$file" | head -n 1
        ;;
      *) return 1 ;;
    esac
  fi
}

assert_profile_value() {
  local file="$1"
  local field="$2"
  local expected="$3"
  local actual
  actual="$(profile_value "$file" "$field" || true)"
  if [ "$actual" != "$expected" ]; then
    die "Expected $field=$expected in redacted profile JSON, got ${actual:-<missing>}"
  fi
}

secret_leak_check_dir() {
  local dir="$1"
  local label="$2"
  local secret="${OCTOS_TUI_SOAK_API_KEY:-}"
  local file
  if [ -z "$secret" ]; then
    return 0
  fi
  if [ ! -d "$dir" ]; then
    return 0
  fi
  while IFS= read -r -d '' file; do
    if [ -f "$file" ] && grep --fixed-strings -- "$secret" "$file" >/dev/null 2>&1; then
      die "Secret leaked into $label artifact: $file"
    fi
  done < <(find "$dir" -type f -print0)
}

secret_leak_check() {
  secret_leak_check_dir "$artifact_dir" "soak"
}

runtime_env_prefix() {
  local api_key_env="${OCTOS_TUI_SOAK_EXPECT_API_KEY_ENV:-AUTODL_API_KEY}"
  local api_key="${OCTOS_TUI_SOAK_API_KEY:-}"
  local prefix=""
  if [ -n "$api_key" ]; then
    prefix="$prefix $(shell_quote "$api_key_env=$api_key")"
  fi
  if [ -n "${OCTOS_M9_PROTOCOL_FIXTURES:-}" ]; then
    prefix="$prefix $(shell_quote "OCTOS_M9_PROTOCOL_FIXTURES=$OCTOS_M9_PROTOCOL_FIXTURES")"
  fi
  if [ -n "${OCTOS_M15_LIVE_SUBAGENT_FIXTURE:-}" ]; then
    prefix="$prefix $(shell_quote "OCTOS_M15_LIVE_SUBAGENT_FIXTURE=$OCTOS_M15_LIVE_SUBAGENT_FIXTURE")"
  fi
  if [ -n "${OCTOS_TUI_M15_UX_OUTPUT_DIR:-}" ]; then
    prefix="$prefix $(shell_quote "OCTOS_TUI_M15_UX_OUTPUT_DIR=$OCTOS_TUI_M15_UX_OUTPUT_DIR")"
  fi
  if [ -n "${OCTOS_TUI_M15_UX_WORKDIR:-}" ]; then
    prefix="$prefix $(shell_quote "OCTOS_TUI_M15_UX_WORKDIR=$OCTOS_TUI_M15_UX_WORKDIR")"
  fi
  if [ -n "${OCTOS_M15_LIVE_SUBAGENT_DELAY_SCALE:-}" ]; then
    prefix="$prefix $(shell_quote "OCTOS_M15_LIVE_SUBAGENT_DELAY_SCALE=$OCTOS_M15_LIVE_SUBAGENT_DELAY_SCALE")"
  fi
  if [ -n "$prefix" ]; then
    printf 'env%s ' "$prefix"
  fi
}

write_summary() {
  mkdir -p "$artifact_dir"
  {
    printf 'run_id=%s\n' "$run_id"
    printf 'artifact_dir=%s\n' "$artifact_dir"
    printf 'transport=%s\n' "$transport"
    printf 'server_session=%s\n' "$server_session"
    printf 'tui_session=%s\n' "$tui_session"
    printf 'fake_openai=%s\n' "$fake_openai"
    printf 'fake_openai_session=%s\n' "$fake_openai_session"
    printf 'fake_openai_base_url=http://%s:%s/v1\n' "$fake_openai_host" "$fake_openai_port"
    printf 'endpoint=%s\n' "$endpoint"
    printf 'runtime_root=%s\n' "$runtime_root"
    printf 'profile_id=%s\n' "$profile_id"
    printf 'session_id=%s\n' "$session_id"
    printf 'open_session=%s\n' "$open_session"
    printf 'first_launch_capture=%s\n' "$first_launch_capture"
    printf 'workspace=%s\n' "$workspace"
    printf 'data_dir=%s\n' "$data_dir"
    printf 'octos_repo_commit=%s\n' "$(git_commit_for_dir "$octos_repo")"
    printf 'octos_tui_repo_commit=%s\n' "$(git_commit_for_dir "$repo_root")"
    printf 'host=%s\n' "$host"
    printf 'port=%s\n' "$port"
  } > "$artifact_dir/summary.env"
}

init_profile_if_missing() {
  local profile_path="$1"
  if [ "${OCTOS_TUI_SOAK_INIT_PROFILE:-1}" = "0" ] || [ -f "$profile_path" ]; then
    return 0
  fi
  mkdir -p "$(dirname "$profile_path")"
  local now
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  local init_llm="${OCTOS_TUI_SOAK_INIT_PROFILE_LLM:-0}"
  local local_name="${OCTOS_TUI_SOAK_LOCAL_NAME:-$profile_id}"
  local local_username="${OCTOS_TUI_SOAK_LOCAL_USERNAME:-$profile_id}"
  local local_email="${OCTOS_TUI_SOAK_LOCAL_EMAIL:-$profile_id@example.invalid}"
  local family="${OCTOS_TUI_SOAK_EXPECT_FAMILY:-moonshot}"
  local model="${OCTOS_TUI_SOAK_EXPECT_MODEL:-kimi-k2.5}"
  local route="${OCTOS_TUI_SOAK_EXPECT_ROUTE:-autodl}"
  local base_url="${OCTOS_TUI_SOAK_EXPECT_BASE_URL:-https://www.autodl.art/api/v1}"
  local api_key_env="${OCTOS_TUI_SOAK_EXPECT_API_KEY_ENV:-AUTODL_API_KEY}"
  local api_key="${OCTOS_TUI_SOAK_API_KEY:-}"
  {
    printf '{\n'
    write_json_string_field id "$profile_id"
    write_json_string_field name "$local_name"
    write_json_string_field username "$local_username"
    write_json_string_field email "$local_email"
    printf '  "enabled": true,\n'
    printf '  "config": {\n'
    if [ "$init_llm" = "1" ]; then
      printf '    "llm": {\n'
      printf '      "primary": {\n'
      write_json_string_field family_id "$family"
      write_json_string_field model_id "$model"
      printf '        "route": {\n'
      write_json_string_field route_id "$route"
      write_json_string_field label "$route"
      write_json_string_field base_url "$base_url"
      write_json_string_field api_key_env "$api_key_env"
      write_json_string_field api_type "openai" ""
      printf '        }\n'
      printf '      },\n'
      printf '      "fallbacks": []\n'
      printf '    },\n'
    fi
    printf '    "channels": [],\n'
    printf '    "gateway": {},\n'
    if [ "$init_llm" = "1" ] && [ -n "$api_key" ]; then
      printf '    "env_vars": {\n'
      write_json_string_field "$api_key_env" "$api_key" ""
      printf '    },\n'
    else
      printf '    "env_vars": {},\n'
    fi
    printf '    "hooks": []\n'
    printf '  },\n'
    write_json_string_field created_at "$now"
    write_json_string_field updated_at "$now" ""
    printf '}\n'
  } > "$profile_path"
}

write_runtime_policy_stamp() {
  local profile_json="$1"
  local family=""
  local model=""
  local route=""
  local base_url=""

  family="$(profile_value "$profile_json" family_id || true)"
  model="$(profile_value "$profile_json" model_id || true)"
  route="$(profile_value "$profile_json" route_id || true)"
  base_url="$(profile_value "$profile_json" base_url || true)"

  {
    printf '{\n'
    write_json_string_field run_id "$run_id"
    write_json_string_field profile_id "$profile_id"
    write_json_string_field session_id "$session_id"
    write_json_string_field family_id "$family"
    write_json_string_field model_id "$model"
    write_json_string_field route_id "$route"
    write_json_string_field base_url "$base_url"
    write_json_string_field source "profile-json-after.json" ""
    printf '}\n'
  } > "$artifact_dir/runtime-policy-stamp.json"

  {
    printf 'run_id=%s\n' "$run_id"
    printf 'profile_id=%s\n' "$profile_id"
    printf 'session_id=%s\n' "$session_id"
    printf 'family_id=%s\n' "$family"
    printf 'model_id=%s\n' "$model"
    printf 'route_id=%s\n' "$route"
    printf 'base_url=%s\n' "$base_url"
  } > "$artifact_dir/runtime-policy-stamp.txt"
}

write_api_parity_checklist() {
  mkdir -p "$artifact_dir"
  {
    printf '{\n'
    write_json_string_field schema "octos-tui-onboarding-api-parity-checklist-v1"
    write_json_string_field purpose "Record equivalence expectations between dashboard profile patch and AppUI profile/llm/upsert."
    printf '  "cases": [\n'
    printf '    {"name":"moonshot-autodl","family_id":"moonshot","model_id":"kimi-k2.5","route_id":"autodl","base_url":"https://www.autodl.art/api/v1","expectation":"AppUI upsert and dashboard patch persist identical redacted config.llm primary selection and env_vars key presence."},\n'
    printf '    {"name":"minimax-wisemodel","family_id":"minimax","model_id":"MiniMax-M2.5-highspeed","route_id":"wisemodel","base_url":"https://open.ospreyai.cn/v1","expectation":"AppUI upsert and dashboard patch persist identical redacted config.llm primary selection and env_vars key presence."},\n'
    printf '    {"name":"custom-openai-compatible","family_id":"custom","model_id":"custom-model","route_id":"custom","base_url":"https://example.invalid/v1","expectation":"Custom family/model/route/base_url/api_type survive through both APIs with secrets redacted before comparison."}\n'
    printf '  ],\n'
    printf '  "comparison": {\n'
    printf '    "normalize": ["redact config.env_vars values", "ignore timestamps/order-only differences"],\n'
    printf '    "must_match": ["config.llm.primary.family_id", "config.llm.primary.model_id", "config.llm.primary.route.route_id", "config.llm.primary.route.base_url", "config.llm.primary.route.api_key_env", "config.llm.primary.route.api_type", "config.env_vars keys"],\n'
    printf '    "must_not_match_raw": ["api key values"]\n'
    printf '  }\n'
    printf '}\n'
  } > "$artifact_dir/api-parity-checklist.json"
}

summary_env_value() {
  local key="$1"
  local summary_file="$artifact_dir/summary.env"
  [ -f "$summary_file" ] || return 1
  sed -n -E "s/^${key}=(.*)$/\\1/p" "$summary_file" | head -n 1
}

summary_env_value_for_dir() {
  local dir="$1"
  local key="$2"
  local summary_file="$dir/summary.env"
  [ -f "$summary_file" ] || return 1
  sed -n -E "s/^${key}=(.*)$/\\1/p" "$summary_file" | head -n 1
}

write_ux_validation() {
  local scenario="$1"
  local status="$2"
  local summary="$3"
  local validation_run_id="$run_id"
  local validation_transport="$transport"
  validation_run_id="$(summary_env_value run_id || printf '%s' "$run_id")"
  validation_transport="$(summary_env_value transport || printf '%s' "$transport")"
  mkdir -p "$artifact_dir"
  {
    printf '{\n'
    write_json_string_field schema "octos-tui-onboarding-ux-validation-v1"
    write_json_string_field run_id "$validation_run_id"
    write_json_string_field scenario "$scenario"
    write_json_string_field status "$status"
    write_json_string_field transport "$validation_transport"
    write_json_string_field artifact_dir "$artifact_dir"
    write_json_string_field summary "$summary"
    write_json_string_field generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" ""
    printf '}\n'
  } > "$artifact_dir/ux-validation.json"
}

markdown_cell() {
  local value="$1"
  value=${value//$'\r'/ }
  value=${value//$'\n'/ }
  value=${value//|/\\|}
  printf '%s' "$value"
}

write_solo_summary_matrix() {
  local summary_json="$artifact_dir/soak-summary.json"
  local summary_matrix="$artifact_dir/summary-matrix.md"
  [ -f "$summary_json" ] || die "M12 solo summary missing: $summary_json"

  {
    printf '# M12 Solo Soak Summary\n\n'
    printf 'Run: `%s`\n\n' "$run_id"
    printf '| Case | Status | Notes |\n'
    printf '|---|---|---|\n'
    if command -v jq >/dev/null 2>&1; then
      jq -e '.cases | type == "array" and length > 0' "$summary_json" >/dev/null \
        || die "M12 solo summary has no case matrix: $summary_json"
      jq -r '
        .cases[]
        | [
            (.name // "unknown"),
            (.status // "unknown"),
            (
              .reason
              // .todo
              // .error.message
              // (if (.missing_required_tools? // []) | length > 0 then
                    "missing_required_tools=" + ((.missing_required_tools? // []) | join(","))
                  else
                    ""
                  end)
            )
          ]
        | @tsv
      ' "$summary_json" | while IFS=$'\t' read -r name status notes; do
        printf '| %s | %s | %s |\n' \
          "$(markdown_cell "$name")" \
          "$(markdown_cell "$status")" \
          "$(markdown_cell "$notes")"
      done
    else
      printf '| soak-summary | present | jq unavailable; inspect `soak-summary.json` for per-case status. |\n'
    fi
  } > "$summary_matrix"
}

verify_solo_tenant_negative_case() {
  local summary_json="$artifact_dir/soak-summary.json"
  [ -f "$summary_json" ] || die "M12 tenant-negative summary missing: $summary_json"

  if command -v jq >/dev/null 2>&1; then
    jq -e '
      any(.cases[]?;
        .name == "tenant-danger-rejection"
        and .status == "ok"
        and (
          .rejected == true
          or .result.rejected == true
          or .applied == false
          or .result.applied == false
        )
      )
    ' "$summary_json" >/dev/null \
      || die "M12 solo tenant/cloud dangerous-mode rejection row missing or not passed"
    return
  fi

  local compact
  compact="$(tr -d '\n[:space:]' < "$summary_json")"
  case "$compact" in
    *'"name":"tenant-danger-rejection"'*'"status":"ok"'*'"rejected":true'*|\
    *'"name":"tenant-danger-rejection"'*'"status":"ok"'*'"applied":false'*)
      ;;
    *)
      die "M12 solo tenant/cloud dangerous-mode rejection row missing or not passed"
      ;;
  esac
}

verify_solo_required_cases() {
  local summary_json="$artifact_dir/soak-summary.json"
  local required_cases="$1"
  [ -n "$required_cases" ] || return 0
  [ -f "$summary_json" ] || die "M12 solo summary missing: $summary_json"

  required_cases="${required_cases//,/ }"
  local case_name
  if command -v jq >/dev/null 2>&1; then
    for case_name in $required_cases; do
      jq -e --arg name "$case_name" '
        any(.cases[]?; .name == $name and .status == "ok")
      ' "$summary_json" >/dev/null \
        || die "M12 solo required case missing or not ok: $case_name"
    done
    return
  fi

  local compact
  compact="$(tr -d '\n[:space:]' < "$summary_json")"
  for case_name in $required_cases; do
    case "$compact" in
      *'"name":"'"$case_name"'"'*'"status":"ok"'*) ;;
      *) die "M12 solo required case missing or not ok: $case_name" ;;
    esac
  done
}

assert_capture_clean() {
  local file="$1"
  local label="$2"
  [ -f "$file" ] || die "$label capture missing: $file"
  grep -q '[^[:space:]]' "$file" || die "$label capture is empty: $file"
  if grep -E 'tmux unavailable|tmux session not running|Task Error|app-ui error|malformed_json|unsupported method|unavailable: AppUI capabilities|Traceback|panicked at' \
    "$file" >/dev/null 2>&1; then
    die "$label capture contains tmux/AppUI error text: $file"
  fi
}

first_existing_artifact() {
  local label="$1"
  shift
  local file
  for file in "$@"; do
    if [ -f "$file" ]; then
      printf '%s\n' "$file"
      return 0
    fi
  done
  die "$label missing"
}

appui_transcript_for_dir() {
  local label="$1"
  local dir="$2"
  first_existing_artifact "$label AppUI transcript" \
    "$dir/m15-evidence/appui-transcript.jsonl" \
    "$dir/appui-transcript.jsonl"
}

verify_transport_dir_kind() {
  local label="$1"
  local dir="$2"
  local expected="$3"
  local actual
  if ! actual="$(summary_env_value_for_dir "$dir" transport)"; then
    die "$label artifact dir missing summary.env transport: $dir/summary.env"
  fi
  [ "$actual" = "$expected" ] \
    || die "$label artifact dir has transport=$actual, expected $expected: $dir/summary.env"
}

extract_appui_method_sequence() {
  local transcript="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -Rr '
      def norm_dir($d):
        if $d == "client_to_server" then "tx"
        elif $d == "server_to_client" then "rx"
        elif $d == null or $d == "" then "unknown"
        else $d
        end;
      fromjson?
      | select(type == "object")
      | (.frame.method // .method // empty) as $method
      | select($method != "")
      | norm_dir(.direction // .frame.direction // .dir // null) + "\t" + $method
    ' "$transcript"
  else
    sed -n -E 's/.*"direction"[[:space:]]*:[[:space:]]*"([^"]+)".*"method"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1	\2/p' "$transcript" \
      | sed -e 's/^client_to_server	/tx	/' -e 's/^server_to_client	/rx	/'
  fi
}

json_scalar_value() {
  local file="$1"
  local key="$2"
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg key "$key" '.[$key] // empty' "$file" 2>/dev/null | head -n 1
  else
    sed -n -E "s/^[[:space:]]*\"$key\"[[:space:]]*:[[:space:]]*\"?([^\",}]*)\"?.*/\1/p" "$file" | head -n 1
  fi
}

start() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for start"
  require_bin OCTOS_BIN "$octos_bin"
  require_bin OCTOS_TUI_BIN "$octos_tui_bin"
  mkdir -p "$workspace" "$data_dir" "$logs_dir"
  write_summary
  write_api_parity_checklist

  local profile_path="$data_dir/profiles/$profile_id.json"
  if [ "$first_launch_capture" = "1" ]; then
    if [ -f "$profile_path" ]; then
      die "OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE=1 requires no existing profile JSON: $profile_path"
    fi
  else
    init_profile_if_missing "$profile_path"
  fi
  local launch_session_id="$session_id"
  local profile_family=""
  profile_family="$(profile_value "$profile_path" family_id || true)"
  if [ "$first_launch_capture" = "1" ] || [ "$open_session" = "0" ] || { [ "$open_session" = "auto" ] && { [ ! -f "$profile_path" ] || [ -z "$profile_family" ]; }; }; then
    launch_session_id=""
  fi
  if [ -f "$profile_path" ]; then
    redact_profile "$profile_path" "$artifact_dir/profile-json-before.json"
  fi

  tmux kill-session -t "$server_session" 2>/dev/null || true
  tmux kill-session -t "$tui_session" 2>/dev/null || true
  tmux kill-session -t "$fake_openai_session" 2>/dev/null || true

  if [ "$fake_openai" = "1" ]; then
    local fake_cmd
    fake_cmd="cd $(shell_quote "$repo_root") && python3 $(shell_quote "$script_dir/fake-openai-server.py") --host $(shell_quote "$fake_openai_host") --port $(shell_quote "$fake_openai_port") --content OK --delay-secs $(shell_quote "$fake_openai_delay_secs") 2>&1 | tee $(shell_quote "$logs_dir/fake-openai.log")"
    tmux new-session -d -s "$fake_openai_session" "$fake_cmd"
    sleep "${OCTOS_TUI_SOAK_FAKE_OPENAI_WAIT_SECS:-1}"
  fi

  local env_prefix
  env_prefix="$(runtime_env_prefix)"

  if [ "$transport" = "ws" ]; then
    local server_cmd
    server_cmd="cd $(shell_quote "$workspace") && ${env_prefix}$(shell_quote "$octos_bin") serve --host $(shell_quote "$host") --port $(shell_quote "$port") --data-dir $(shell_quote "$data_dir") --auth-token $(shell_quote "$auth_token")"
    if [ -n "$serve_args" ]; then
      server_cmd="$server_cmd $serve_args"
    fi
    server_cmd="$server_cmd 2>&1 | tee $(shell_quote "$logs_dir/server.log")"
    tmux new-session -d -s "$server_session" "$server_cmd"
    wait_for_server_ready "${OCTOS_TUI_SOAK_SERVER_WAIT_SECS:-20}"
  else
    : > "$logs_dir/server.log"
    tmux new-session -d -s "$server_session" "tail -n +1 -F $(shell_quote "$logs_dir/server.log")"
    sleep "${OCTOS_TUI_SOAK_SERVER_WAIT_SECS:-1}"
  fi

  local tui_cmd
  tui_cmd="cd $(shell_quote "$workspace") && "
  tui_cmd="${tui_cmd}${env_prefix}"
  tui_cmd="${tui_cmd}$(shell_quote "$octos_tui_bin") --mode protocol"
  if [ "$transport" = "ws" ]; then
    tui_cmd="$tui_cmd --endpoint $(shell_quote "$endpoint") --auth-token $(shell_quote "$auth_token")"
  else
    local stdio_cmd
    local stdio_pid_file="$logs_dir/stdio-backend.pid"
    stdio_cmd="cd $(shell_quote "$workspace") && printf '%s\n' \$\$ > $(shell_quote "$stdio_pid_file") && exec ${env_prefix}$(shell_quote "$octos_bin") serve --stdio --data-dir $(shell_quote "$data_dir")"
    if [ -n "$serve_args" ]; then
      stdio_cmd="$stdio_cmd $serve_args"
    fi
    stdio_cmd="$stdio_cmd 2>$(shell_quote "$logs_dir/server.log")"
    tui_cmd="$tui_cmd --stdio-command $(shell_quote "$stdio_cmd")"
  fi
  if [ -n "$launch_session_id" ]; then
    tui_cmd="$tui_cmd --session $(shell_quote "$launch_session_id")"
  fi
  if [ "$first_launch_capture" != "1" ]; then
    tui_cmd="$tui_cmd --profile-id $(shell_quote "$profile_id")"
  fi
  tui_cmd="$tui_cmd --cwd $(shell_quote "$workspace") --theme $(shell_quote "$theme")"
  tui_cmd="$tui_cmd 2>&1; exit_code=\$?; echo octos-tui exited with status \$exit_code; sleep ${OCTOS_TUI_SOAK_EXIT_HOLD_SECS:-30}"
  tmux new-session -d -s "$tui_session" "$tui_cmd"

  if [ "$first_launch_capture" = "1" ]; then
    wait_for_tui_text "Welcome to Octos" "${OCTOS_TUI_SOAK_FIRST_LAUNCH_WAIT_SECS:-20}" || \
      die "Timed out waiting for first-launch onboarding splash"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-first-launch.txt"
  else
    sleep "${OCTOS_TUI_SOAK_TUI_WAIT_SECS:-2}"
  fi
  capture

  cat <<EOF
Started onboarding tmux soak.
  server: tmux attach -t $server_session
  tui:    tmux attach -t $tui_session
  dir:    $artifact_dir

Manual checkpoints:
  /login -> complete OTP when needed
  /provider -> select catalog route, set masked key, save provider
  drive -> scripts/run-onboarding-tmux-soak.sh drive-onboard
  /model -> verify server-returned model list
  prompt -> "Reply with exactly OK."
  verify -> scripts/run-onboarding-tmux-soak.sh verify-onboard
EOF
}

restart_server() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for restart-server"
  require_bin OCTOS_BIN "$octos_bin"
  if [ "$transport" != "ws" ]; then
    die "restart-server is only supported for WebSocket transport"
  fi
  if ! tmux has-session -t "$server_session" 2>/dev/null; then
    die "Backend tmux session is not running before restart: $server_session"
  fi
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running before backend restart: $tui_session"
  fi

  mkdir -p "$workspace" "$data_dir" "$logs_dir" "$artifact_dir"
  tmux kill-session -t "$server_session" 2>/dev/null || true
  local shutdown_deadline=$((SECONDS + ${OCTOS_TUI_SOAK_SERVER_SHUTDOWN_WAIT_SECS:-10}))
  while tmux has-session -t "$server_session" 2>/dev/null && [ "$SECONDS" -le "$shutdown_deadline" ]; do
    sleep 0.2
  done
  if tmux has-session -t "$server_session" 2>/dev/null; then
    die "Backend tmux session did not exit after restart kill: $server_session"
  fi
  if [ -f "$logs_dir/server.log" ]; then
    cp "$logs_dir/server.log" "$artifact_dir/server-before-restart.log"
  fi
  sleep "${OCTOS_TUI_SOAK_SERVER_RESTART_DOWN_SECS:-1}"
  : > "$logs_dir/server.log"

  local env_prefix
  env_prefix="$(runtime_env_prefix)"
  local server_cmd
  server_cmd="cd $(shell_quote "$workspace") && ${env_prefix}$(shell_quote "$octos_bin") serve --host $(shell_quote "$host") --port $(shell_quote "$port") --data-dir $(shell_quote "$data_dir") --auth-token $(shell_quote "$auth_token")"
  if [ -n "$serve_args" ]; then
    server_cmd="$server_cmd $serve_args"
  fi
  server_cmd="$server_cmd 2>&1 | tee $(shell_quote "$logs_dir/server.log")"
  tmux new-session -d -s "$server_session" "$server_cmd"
  wait_for_server_ready "${OCTOS_TUI_SOAK_SERVER_WAIT_SECS:-20}"
  capture_pane "$server_session" "$artifact_dir/server-pane-after-restart.txt"
  capture
  echo "Restarted backend tmux server for $tui_session"
}

stdio_backend_pids() {
  local pid_file="$logs_dir/stdio-backend.pid"
  if [ -f "$pid_file" ]; then
    local pid
    while IFS= read -r pid; do
      case "$pid" in
        ''|*[!0-9]*) continue ;;
      esac
      if kill -0 "$pid" 2>/dev/null; then
        printf '%s\n' "$pid"
      fi
    done < "$pid_file"
    return 0
  fi

  command -v ps >/dev/null 2>&1 || die "ps is required to locate stdio backend processes"
  ps -ax -o pid= -o command= | awk \
    -v bin="$octos_bin" \
    -v data="$data_dir" '
      index($0, bin) &&
      index($0, "serve --stdio") &&
      index($0, data) &&
      index($0, "octos-tui") == 0 &&
      index($0, "--stdio-command") == 0 &&
      index($0, "run-onboarding-tmux-soak.sh") == 0 {
        pid = $1
        if (pid ~ /^[0-9]+$/) {
          print pid
        }
      }
    '
}

restart_stdio_child() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for stdio restart"
  require_bin OCTOS_BIN "$octos_bin"
  if [ "$transport" != "stdio" ]; then
    die "restart_stdio_child is only supported for stdio transport"
  fi
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running before stdio restart: $tui_session"
  fi

  mkdir -p "$workspace" "$data_dir" "$logs_dir" "$artifact_dir"
  local pids
  pids="$(stdio_backend_pids | sort -u)"
  if [ -z "$pids" ]; then
    die "No scoped stdio backend process matched OCTOS_BIN=$octos_bin and data_dir=$data_dir"
  fi

  if [ -f "$logs_dir/server.log" ]; then
    cp "$logs_dir/server.log" "$artifact_dir/server-before-restart.log"
  fi

  local pid
  for pid in $pids; do
    kill "$pid" 2>/dev/null || true
  done

  local shutdown_deadline=$((SECONDS + ${OCTOS_TUI_SOAK_STDIO_SHUTDOWN_WAIT_SECS:-10}))
  local remaining
  while [ "$SECONDS" -le "$shutdown_deadline" ]; do
    remaining="$(stdio_backend_pids | sort -u)"
    [ -z "$remaining" ] && break
    sleep 0.2
  done
  remaining="$(stdio_backend_pids | sort -u)"
  if [ -n "$remaining" ]; then
    die "Scoped stdio backend did not exit after SIGTERM: $remaining"
  fi

  {
    printf 'Terminated scoped stdio backend process(es): %s\n' "$pids"
    printf 'OCTOS_BIN=%s\n' "$octos_bin"
    printf 'data_dir=%s\n' "$data_dir"
  } > "$artifact_dir/server-pane-after-restart.txt"
  capture
  echo "Restarted stdio backend child for $tui_session"
}

capture() {
  mkdir -p "$artifact_dir"
  write_summary
  capture_pane "$server_session" "$artifact_dir/server-pane.txt"
  if [ "$fake_openai" = "1" ]; then
    capture_pane "$fake_openai_session" "$artifact_dir/fake-openai-pane.txt"
  fi
  capture_pane "$tui_session" "$artifact_dir/tui-capture.txt"
  if [ -f "$logs_dir/server.log" ]; then
    cp "$logs_dir/server.log" "$artifact_dir/server.log"
  else
    : > "$artifact_dir/server.log"
  fi
  if [ -f "$logs_dir/fake-openai.log" ]; then
    cp "$logs_dir/fake-openai.log" "$artifact_dir/fake-openai.log"
  fi
}

send_turn() {
  local prompt="${OCTOS_TUI_SOAK_PROMPT:-Reply with exactly OK.}"
  command -v tmux >/dev/null 2>&1 || die "tmux is required for send-turn"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi
  submit_composer_prompt "$prompt"
  sleep "${OCTOS_TUI_SOAK_TURN_WAIT_SECS:-20}"
  capture
}

send_tui_line() {
  local line="$1"
  tmux send-keys -t "$tui_session" Escape
  sleep 0.1
  tmux send-keys -t "$tui_session" -l "$line"
  sleep 0.1
  tmux send-keys -t "$tui_session" Escape
  sleep 0.1
  tmux send-keys -t "$tui_session" Enter
  sleep "${OCTOS_TUI_SOAK_COMMAND_WAIT_SECS:-1}"
}

drive_onboard() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-onboard"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local family="${OCTOS_TUI_SOAK_EXPECT_FAMILY:-moonshot}"
  local model="${OCTOS_TUI_SOAK_EXPECT_MODEL:-kimi-k2.5}"
  local route="${OCTOS_TUI_SOAK_EXPECT_ROUTE:-autodl}"
  local base_url="${OCTOS_TUI_SOAK_EXPECT_BASE_URL:-https://www.autodl.art/api/v1}"
  local api_key_env="${OCTOS_TUI_SOAK_EXPECT_API_KEY_ENV:-AUTODL_API_KEY}"
  local api_key="${OCTOS_TUI_SOAK_API_KEY:-octos-tui-soak-placeholder-key}"

  # M22-A polished onboarding (post-#67 / commit f142a86) auto-opens the
  # onboarding picker on first launch when profile/local/create is advertised.
  # The picker overlay redraws over the status line, so tmux capture-pane
  # can't reliably catch the legacy "AppUI capabilities refreshed: N methods"
  # banner. See octos-tui#27 mini5 sweep finding.
  #
  # Wait for ANY of three signals (`|`-separated alternation per the
  # extended wait_for_tui_text):
  #
  #   * "Welcome to Octos"          — fresh first-launch splash (no profile)
  #   * "Set Up LLM Provider"       — picker after profile/llm/list resolves
  #                                   (when a profile_id was passed to start
  #                                   and the picker auto-advances to the
  #                                   provider step). Codex P2 follow-up:
  #                                   if profile/llm/list returns before
  #                                   drive-onboard runs, the splash text
  #                                   has already been replaced — without
  #                                   this alternative the wait times out.
  #   * "AppUI capabilities refreshed" — legacy OTP path (auth/send_code +
  #                                   verify + me) which doesn't trigger
  #                                   the polished picker overlay, so the
  #                                   status banner remains visible.
  #
  # Operators driving a custom flow can override via OCTOS_TUI_SOAK_READY_TEXT
  # — values are also treated as `|`-separated alternations.
  local ready_text="${OCTOS_TUI_SOAK_READY_TEXT:-Welcome to Octos|Set Up LLM Provider|AppUI capabilities refreshed}"
  wait_for_tui_text "$ready_text" "${OCTOS_TUI_SOAK_CAPABILITIES_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI ready signal ('$ready_text') before driving onboarding commands"
  send_tui_line "/login status"
  send_tui_line "/login me"
  send_tui_line "/provider catalog"
  sleep "${OCTOS_TUI_SOAK_CATALOG_WAIT_SECS:-2}"
  send_tui_line "/provider select $family $model $route $base_url $api_key_env"
  send_tui_line "/provider key $api_key"
  send_tui_line "/provider save"
  sleep "${OCTOS_TUI_SOAK_SAVE_WAIT_SECS:-2}"
  send_tui_line "/provider list"
  if [ "${OCTOS_TUI_SOAK_DRIVE_FINISH:-1}" = "1" ]; then
    send_tui_line "/onboard profile $profile_id"
    send_tui_line "/onboard finish"
  fi
  send_tui_line "/provider"
  send_tui_line "/model"
  sleep "${OCTOS_TUI_SOAK_FINISH_WAIT_SECS:-2}"
  capture
  echo "Drove /onboard flow in $tui_session"
}

verify_onboard() {
  capture
  local profile_path="$data_dir/profiles/$profile_id.json"
  local redacted_profile="$artifact_dir/profile-json-after.json"

  if [ -f "$profile_path" ]; then
    redact_profile "$profile_path" "$redacted_profile"
  elif [ "${OCTOS_TUI_SOAK_REQUIRE_PROFILE:-1}" != "0" ]; then
    die "Profile JSON missing: $profile_path"
  else
    printf '{}\n' > "$redacted_profile"
  fi

  if [ -n "${OCTOS_TUI_SOAK_EXPECT_FAMILY:-}" ]; then
    assert_profile_value "$redacted_profile" family_id "$OCTOS_TUI_SOAK_EXPECT_FAMILY"
  fi
  if [ -n "${OCTOS_TUI_SOAK_EXPECT_MODEL:-}" ]; then
    assert_profile_value "$redacted_profile" model_id "$OCTOS_TUI_SOAK_EXPECT_MODEL"
  fi
  if [ -n "${OCTOS_TUI_SOAK_EXPECT_ROUTE:-}" ]; then
    assert_profile_value "$redacted_profile" route_id "$OCTOS_TUI_SOAK_EXPECT_ROUTE"
  fi
  if [ -n "${OCTOS_TUI_SOAK_EXPECT_BASE_URL:-}" ]; then
    assert_profile_value "$redacted_profile" base_url "$OCTOS_TUI_SOAK_EXPECT_BASE_URL"
  fi

  write_runtime_policy_stamp "$redacted_profile"
  write_api_parity_checklist

  if [ -f "$artifact_dir/tui-capture.txt" ]; then
    assert_capture_clean "$artifact_dir/tui-capture.txt" "TUI"
    if grep -E 'malformed_json|session\.workspace_cwd|requires protocol|provider is unavailable|Task Error|app-ui error|unavailable: AppUI capabilities' \
      "$artifact_dir/tui-capture.txt" >/dev/null 2>&1; then
      die "TUI capture contains AppUI/onboarding error text"
    fi
  fi

  if [ "$fake_openai" = "1" ]; then
    [ -f "$artifact_dir/fake-openai.log" ] || die "fake OpenAI log missing"
    if grep -E 'Traceback|OSError|Address already in use' "$artifact_dir/fake-openai.log" >/dev/null 2>&1; then
      die "fake OpenAI server failed; see $artifact_dir/fake-openai.log"
    fi
    if ! grep -E '"POST /v1/(chat/completions|responses) HTTP/1\.1" 200' \
      "$artifact_dir/fake-openai.log" >/dev/null 2>&1; then
      die "fake OpenAI log did not record a successful model API call"
    fi
  fi

  {
    printf '{\n'
    write_json_string_field run_id "$run_id"
    write_json_string_field profile_id "$profile_id"
    write_json_string_field session_id "$session_id"
    write_json_string_field artifact_dir "$artifact_dir"
    write_json_string_field expected_family "${OCTOS_TUI_SOAK_EXPECT_FAMILY:-}"
    write_json_string_field expected_model "${OCTOS_TUI_SOAK_EXPECT_MODEL:-}"
    write_json_string_field expected_route "${OCTOS_TUI_SOAK_EXPECT_ROUTE:-}"
    write_json_string_field expected_base_url "${OCTOS_TUI_SOAK_EXPECT_BASE_URL:-}"
    write_json_string_field api_parity_checklist "api-parity-checklist.json"
    write_json_string_field verified_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" ""
    printf '}\n'
  } > "$artifact_dir/soak-summary.json"

  write_ux_validation "provider-onboarding" "passed" "provider onboarding artifacts verified"
  secret_leak_check
  echo "Verified onboarding soak artifacts in $artifact_dir"
}

verify() {
  verify_onboard
}

api_parity() {
  write_summary
  write_api_parity_checklist
  echo "Wrote API parity checklist to $artifact_dir/api-parity-checklist.json"
}

solo_probe_args() {
  local probe_transport="$1"
  local stdio_command="${2:-}"
  local local_name="${OCTOS_TUI_SOAK_LOCAL_NAME:-$profile_id}"
  local probe="$octos_repo/scripts/m12-solo-appui-probe.mjs"
  [ -f "$probe" ] || die "M12 solo AppUI probe missing: $probe"
  local args=(
    "$probe"
    --transport "$probe_transport"
    --out-dir "$artifact_dir"
    --workspace "$workspace"
    --data-dir "$solo_probe_data_dir"
    --profile-id "$profile_id"
    --session-id "$session_id"
    --local-name "$local_name"
    --local-username "${OCTOS_TUI_SOAK_LOCAL_USERNAME:-$profile_id}"
    --local-email "${OCTOS_TUI_SOAK_LOCAL_EMAIL:-$profile_id@example.invalid}"
    --server-log "$solo_probe_server_log"
  )
  if [ "$probe_transport" = "ws" ]; then
    args+=(--endpoint "$endpoint" --auth-token "$auth_token")
  fi
  if [ "$probe_transport" = "stdio" ]; then
    args+=(--stdio-command "$stdio_command")
  fi
  if [ "${OCTOS_TUI_SOAK_SOLO_STRICT:-0}" = "1" ]; then
    args+=(--strict)
  fi
  if [ "${OCTOS_TUI_SOAK_TENANT_NEGATIVE:-0}" != "1" ]; then
    args+=(--no-tenant-negative)
  fi
  printf '%s\0' "${args[@]}"
}

drive_solo() {
  command -v node >/dev/null 2>&1 || die "node is required for drive-solo"
  require_octos_serve
  mkdir -p "$workspace" "$data_dir" "$solo_probe_data_dir" "$logs_dir" "$artifact_dir"
  write_summary
  local local_name="${OCTOS_TUI_SOAK_LOCAL_NAME:-$profile_id}"
  OCTOS_TUI_SOAK_INIT_PROFILE_LLM="${OCTOS_TUI_SOAK_INIT_PROFILE_LLM:-1}" \
    OCTOS_TUI_SOAK_LOCAL_NAME="$local_name" \
    init_profile_if_missing "$solo_probe_data_dir/profiles/$profile_id.json"
  capture

  local probe_transport="$transport"
  local stdio_command=""
  local env_prefix
  env_prefix="$(runtime_env_prefix)"
  if [ "$probe_transport" = "ws" ]; then
    if have_tmux && ! tmux has-session -t "$server_session" 2>/dev/null; then
      die "WS solo probe expects the server tmux session to be running; run start first or use OCTOS_TUI_SOAK_TRANSPORT=stdio"
    fi
  else
    # TODO(M12-A/C): once `octos serve` grows explicit solo/dangerous flags,
    # append them via OCTOS_TUI_SOAK_SERVE_ARGS instead of relying only on
    # AppUI capability negotiation.
    stdio_command="${env_prefix}$(shell_quote "$octos_bin") serve --stdio --data-dir $(shell_quote "$solo_probe_data_dir") --cwd $(shell_quote "$workspace")"
    if [ -n "$serve_args" ]; then
      stdio_command="$stdio_command $serve_args"
    fi
  fi

  local -a args=()
  while IFS= read -r -d '' arg; do
    args+=("$arg")
  done < <(solo_probe_args "$probe_transport" "$stdio_command")
  node "${args[@]}"
  if [ -f "$solo_probe_server_log" ]; then
    cp "$solo_probe_server_log" "$logs_dir/server.log"
  fi
  capture
  echo "Drove M12 solo no-OTP AppUI probe in $artifact_dir"
}

drive_permissions() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-permissions"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before opening permissions"
  send_tui_line "/permissions"
  wait_for_tui_text "Update Model Permissions" "${OCTOS_TUI_SOAK_PERMISSIONS_WAIT_SECS:-20}" || \
    die "Timed out waiting for /permissions menu"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-permissions-open.txt"

  tmux send-keys -t "$tui_session" j
  sleep 0.1
  tmux send-keys -t "$tui_session" j
  sleep 0.1
  tmux send-keys -t "$tui_session" j
  sleep 0.1
  tmux send-keys -t "$tui_session" Enter
  wait_for_tui_text "Permissions updated: Workspace Write" \
    "${OCTOS_TUI_SOAK_PERMISSIONS_APPLY_WAIT_SECS:-5}" || \
    die "Timed out waiting for workspace-write permission update"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-permissions-applied.txt"
  capture
  echo "Drove /permissions selection in $tui_session"
}

drive_provider_missing() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-provider-missing"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  wait_for_tui_text "Set Up LLM Provider" "${OCTOS_TUI_SOAK_PROVIDER_WAIT_SECS:-20}" || \
    die "Timed out waiting for missing-provider setup menu"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-provider-missing.txt"
  capture
  echo "Drove missing-provider recovery capture in $tui_session"
}

drive_approval_denial() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-approval-denial"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_APPROVAL_PROMPT:-M9 approval fixture: request approval for printf m19-approval-denial}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving approval denial"
  send_tui_line "$prompt"
  wait_for_tui_text "Approval Requested" "${OCTOS_TUI_SOAK_APPROVAL_WAIT_SECS:-40}" || \
    die "Timed out waiting for approval request in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-approval-request.txt"

  tmux send-keys -t "$tui_session" n
  wait_for_tui_text "Approval denied" "${OCTOS_TUI_SOAK_APPROVAL_DENIAL_WAIT_SECS:-40}" || \
    die "Timed out waiting for approval denial acknowledgement in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-approval-denied.txt"
  capture
  echo "Drove approval denial in $tui_session"
}

drive_multiline_composer() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-multiline-composer"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_MULTILINE_PROMPT:-}"
  if [ -z "$prompt" ]; then
    prompt=$'first instruction\nsecond instruction\nthird instruction'
  fi
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before multiline capture"
  tmux send-keys -t "$tui_session" Escape
  sleep 0.1
  local first_line=1
  while IFS= read -r line; do
    if [ "$first_line" = "1" ]; then
      first_line=0
    else
      tmux send-keys -t "$tui_session" Enter
    fi
    if [ -n "$line" ]; then
      tmux send-keys -t "$tui_session" -l "$line"
    fi
  done <<EOF
$prompt
EOF
  sleep "${OCTOS_TUI_SOAK_MULTILINE_SETTLE_SECS:-0.5}"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-multiline-composer.txt"
  capture
  echo "Drove multiline composer capture in $tui_session"
}

drive_runtime_menus() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-runtime-menus"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  wait_for_tui_text "Ask Octos to change code|state|AppUI capabilities refreshed" \
    "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI ready signal before runtime menu capture"
  send_tui_line "/status"
  tmux send-keys -t "$tui_session" Down Down Down Enter
  sleep "${OCTOS_TUI_SOAK_COMMAND_WAIT_SECS:-1}"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-runtime-status.txt"
  send_tui_line "/model"
  tmux send-keys -t "$tui_session" Enter
  sleep "${OCTOS_TUI_SOAK_COMMAND_WAIT_SECS:-1}"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-runtime-model.txt"
  send_tui_line "/mcp config"
  send_tui_line "/mcp status"
  send_tui_line "/mcp"
  sleep "${OCTOS_TUI_SOAK_COMMAND_WAIT_SECS:-1}"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-runtime-mcp.txt"
  capture
  echo "Drove runtime menu captures in $tui_session"
}

drive_task_subagent_tree() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-task-subagent-tree"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_TASK_SUBAGENT_PROMPT:-Run M15 code review with live subagent orchestration through octos serve --stdio. Use supervised subagents and produce the final marker.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving task/subagent tree"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "Agent task" "${OCTOS_TUI_SOAK_TASK_SUBAGENT_RUNNING_WAIT_SECS:-10}" || \
    die "Timed out waiting for visible agent task tree"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-task-subagent-tree-running.txt"

  wait_for_tui_text "M15CODEREVIEWFINALLINE" "${OCTOS_TUI_SOAK_TASK_SUBAGENT_DONE_WAIT_SECS:-80}" || \
    die "Timed out waiting for M15 final marker in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-task-subagent-tree-final.txt"
  capture_scrolled_transcript_until_text \
    "Review Summary" \
    "$artifact_dir/tui-capture-task-subagent-tree-summary.txt" \
    "${OCTOS_TUI_SOAK_TASK_SUBAGENT_SUMMARY_PAGEUP_COUNT:-6}" || \
    die "Timed out waiting for visible code-review summary heading after scrolling task/subagent output"
  local page_down=1
  while [ "$page_down" -le "${OCTOS_TUI_SOAK_TASK_SUBAGENT_SUMMARY_PAGEUP_COUNT:-6}" ]; do
    tmux send-keys -t "$tui_session" PageDown
    sleep 0.05
    page_down=$((page_down + 1))
  done
  if [ -n "${OCTOS_TUI_M15_UX_OUTPUT_DIR:-}" ] && [ -d "$OCTOS_TUI_M15_UX_OUTPUT_DIR" ]; then
    local m15_source_abs
    local m15_dest_abs
    m15_source_abs="$(cd "$OCTOS_TUI_M15_UX_OUTPUT_DIR" && pwd -P)"
    mkdir -p "$artifact_dir/m15-evidence"
    m15_dest_abs="$(cd "$artifact_dir/m15-evidence" && pwd -P)"
    case "$m15_dest_abs/" in
      "$m15_source_abs"/*|"$m15_source_abs/")
        die "Refusing recursive M15 evidence copy from $m15_source_abs to $m15_dest_abs"
        ;;
    esac
    cp -R "$m15_source_abs"/. "$m15_dest_abs"/
  fi
  capture
  echo "Drove task/subagent tree in $tui_session"
}

drive_task_subagent_reconnect() {
  if [ "$transport" = "ws" ]; then
    restart_server
  else
    restart_stdio_child
  fi
  wait_for_tui_text "Ask Octos to change code|UI protocol reconnected|stdio child exited|relaunch|state" \
    "${OCTOS_TUI_SOAK_RECONNECT_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI to settle after backend restart"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-task-subagent-tree-reconnect.txt"
  capture
  echo "Drove task/subagent reconnect capture in $tui_session"
}

drive_task_subagent_old_server_fallback() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-task-subagent-old-server-fallback"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  wait_for_tui_text "Ask Octos to change code|state|AppUI capabilities refreshed" \
    "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI ready signal before old-server fallback capture"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-task-subagent-old-server-fallback.txt"
  capture
  echo "Drove task/subagent old-server fallback capture in $tui_session"
}

drive_autonomy_live() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-autonomy-live"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local goal="${OCTOS_TUI_SOAK_AUTONOMY_GOAL:-Keep the production autonomy soak moving until the final joined answer is visible.}"
  local loop_fixed="${OCTOS_TUI_SOAK_AUTONOMY_LOOP_FIXED_PROMPT:-check child-agent progress and report backend truth}"
  local loop_self="${OCTOS_TUI_SOAK_AUTONOMY_LOOP_SELF_PROMPT:-continue the autonomy soak when the backend decides it is idle}"
  local loop_maintenance="${OCTOS_TUI_SOAK_AUTONOMY_LOOP_MAINTENANCE_PROMPT:-prune stale autonomy artifacts after the soak}"
  local review_prompt="${OCTOS_TUI_SOAK_AUTONOMY_REVIEW_PROMPT:-Run a production autonomy review with supervised child agents. Produce model-generated per-child progress summaries, a model-generated final joined answer, goal continuation updates, and loop fire evidence.}"
  local loop_id="${OCTOS_TUI_SOAK_AUTONOMY_LOOP_ID:-}"
  local agent_id="${OCTOS_TUI_SOAK_AUTONOMY_AGENT_ID:-}"

  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving M15 autonomy"

  send_tui_line "/goal $goal"
  wait_for_tui_text "Goal|goal|session/goal" "${OCTOS_TUI_SOAK_AUTONOMY_GOAL_WAIT_SECS:-20}" || \
    die "Timed out waiting for goal runtime evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-goal.txt"

  send_tui_line "/loop 5m $loop_fixed"
  wait_for_tui_text "Loop|loop|loop/create" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
    die "Timed out waiting for fixed loop evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-fixed.txt"

  send_tui_line "/loop $loop_self"
  wait_for_tui_text "Loop|loop|loop/create" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
    die "Timed out waiting for self-paced loop evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-self-paced.txt"

  send_tui_line "/loop maintenance $loop_maintenance"
  wait_for_tui_text "Loop|loop|loop/create" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
    die "Timed out waiting for maintenance loop evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-maintenance.txt"

  send_tui_line "/loop list"
  wait_for_tui_text "Loop|loop|loop/list" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
    die "Timed out waiting for loop list evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-list.txt"

  if [ -n "$loop_id" ]; then
    send_tui_line "/loop fire-now $loop_id"
    wait_for_tui_text "Loop|loop|fire|fired|completed" "${OCTOS_TUI_SOAK_AUTONOMY_FIRE_WAIT_SECS:-40}" || \
      die "Timed out waiting for loop fire-now evidence in TUI"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-fire-now.txt"

    send_tui_line "/loop pause $loop_id"
    wait_for_tui_text "Loop|loop|pause|paused" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
      die "Timed out waiting for loop pause evidence in TUI"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-paused.txt"

    send_tui_line "/loop resume $loop_id"
    wait_for_tui_text "Loop|loop|resume|resumed" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
      die "Timed out waiting for loop resume evidence in TUI"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-loop-resumed.txt"
  fi

  submit_composer_prompt "$review_prompt"
  wait_for_tui_text "Agent|agent|Goal|goal|Loop|loop|summary|final|completed|joined answer" \
    "${OCTOS_TUI_SOAK_AUTONOMY_REVIEW_WAIT_SECS:-120}" || \
    die "Timed out waiting for production autonomy review evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-review.txt"

  send_tui_line "/agents list"
  wait_for_tui_text "Agent|agent|agent/list" "${OCTOS_TUI_SOAK_AUTONOMY_AGENT_WAIT_SECS:-20}" || \
    die "Timed out waiting for agent list evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-agents.txt"

  if [ -n "$agent_id" ]; then
    send_tui_line "/agents status $agent_id"
    wait_for_tui_text "Agent|agent|status" "${OCTOS_TUI_SOAK_AUTONOMY_AGENT_WAIT_SECS:-20}" || \
      die "Timed out waiting for agent status evidence in TUI"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-agent-status.txt"

    send_tui_line "/agents output $agent_id"
    wait_for_tui_text "Agent|agent|output|summary|final" "${OCTOS_TUI_SOAK_AUTONOMY_AGENT_WAIT_SECS:-20}" || \
      die "Timed out waiting for agent output evidence in TUI"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-agent-output.txt"

    send_tui_line "/agents artifacts $agent_id"
    wait_for_tui_text "Agent|agent|Artifacts|artifacts" "${OCTOS_TUI_SOAK_AUTONOMY_AGENT_WAIT_SECS:-20}" || \
      die "Timed out waiting for agent artifact evidence in TUI"
    capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-agent-artifacts.txt"
  fi

  local aggregate_capture="$artifact_dir/tui-capture-autonomy-live.txt"
  : > "$aggregate_capture"
  local step_capture
  for step_capture in \
    "$artifact_dir/tui-capture-autonomy-goal.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-fixed.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-self-paced.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-maintenance.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-list.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-fire-now.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-paused.txt" \
    "$artifact_dir/tui-capture-autonomy-loop-resumed.txt" \
    "$artifact_dir/tui-capture-autonomy-review.txt" \
    "$artifact_dir/tui-capture-autonomy-agents.txt" \
    "$artifact_dir/tui-capture-autonomy-agent-status.txt" \
    "$artifact_dir/tui-capture-autonomy-agent-output.txt" \
    "$artifact_dir/tui-capture-autonomy-agent-artifacts.txt"
  do
    [ -f "$step_capture" ] || continue
    printf '\n== %s ==\n' "$(basename "$step_capture")" >> "$aggregate_capture"
    cat "$step_capture" >> "$aggregate_capture"
  done
  [ -s "$aggregate_capture" ] || die "M15 autonomy aggregate capture was not written"

  if [ -n "${OCTOS_TUI_M15_UX_OUTPUT_DIR:-}" ] && [ -d "$OCTOS_TUI_M15_UX_OUTPUT_DIR" ]; then
    local m15_source_abs
    local m15_dest_abs
    m15_source_abs="$(cd "$OCTOS_TUI_M15_UX_OUTPUT_DIR" && pwd -P)"
    mkdir -p "$artifact_dir/m15-evidence"
    m15_dest_abs="$(cd "$artifact_dir/m15-evidence" && pwd -P)"
    case "$m15_dest_abs/" in
      "$m15_source_abs"/*|"$m15_source_abs/")
        die "Refusing recursive M15 evidence copy from $m15_source_abs to $m15_dest_abs"
        ;;
    esac
    cp -R "$m15_source_abs"/. "$m15_dest_abs"/
  fi

  capture
  echo "Drove M15 autonomy live flow in $tui_session"
}

drive_autonomy_reconnect() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-autonomy-reconnect"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  if [ "$transport" = "ws" ]; then
    restart_server
  else
    restart_stdio_child
  fi
  wait_for_tui_text "UI protocol reconnected|stdio child exited|relaunch|Ask Octos to change code|state" \
    "${OCTOS_TUI_SOAK_RECONNECT_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI to settle after autonomy backend restart"

  send_tui_line "/agents list"
  wait_for_tui_text "Agent|agent|agent/list" "${OCTOS_TUI_SOAK_AUTONOMY_AGENT_WAIT_SECS:-20}" || \
    die "Timed out waiting for reconnect agent hydration evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-reconnect-agents.txt"

  send_tui_line "/goal"
  wait_for_tui_text "Goal|goal|session/goal" "${OCTOS_TUI_SOAK_AUTONOMY_GOAL_WAIT_SECS:-20}" || \
    die "Timed out waiting for reconnect goal hydration evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-reconnect-goal.txt"

  send_tui_line "/loop list"
  wait_for_tui_text "Loop|loop|loop/list" "${OCTOS_TUI_SOAK_AUTONOMY_LOOP_WAIT_SECS:-20}" || \
    die "Timed out waiting for reconnect loop hydration evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-autonomy-reconnect-loops.txt"

  local aggregate_capture="$artifact_dir/tui-capture-autonomy-reconnect.txt"
  : > "$aggregate_capture"
  local step_capture
  for step_capture in \
    "$artifact_dir/tui-capture-autonomy-reconnect-agents.txt" \
    "$artifact_dir/tui-capture-autonomy-reconnect-goal.txt" \
    "$artifact_dir/tui-capture-autonomy-reconnect-loops.txt"
  do
    [ -f "$step_capture" ] || continue
    printf '\n== %s ==\n' "$(basename "$step_capture")" >> "$aggregate_capture"
    cat "$step_capture" >> "$aggregate_capture"
  done
  [ -s "$aggregate_capture" ] || die "M15 autonomy reconnect aggregate capture was not written"

  if [ -n "${OCTOS_TUI_M15_UX_OUTPUT_DIR:-}" ] && [ -d "$OCTOS_TUI_M15_UX_OUTPUT_DIR" ]; then
    local m15_source_abs
    local m15_dest_abs
    m15_source_abs="$(cd "$OCTOS_TUI_M15_UX_OUTPUT_DIR" && pwd -P)"
    mkdir -p "$artifact_dir/m15-evidence"
    m15_dest_abs="$(cd "$artifact_dir/m15-evidence" && pwd -P)"
    case "$m15_dest_abs/" in
      "$m15_source_abs"/*|"$m15_source_abs/")
        die "Refusing recursive M15 evidence copy from $m15_source_abs to $m15_dest_abs"
        ;;
    esac
    cp -R "$m15_source_abs"/. "$m15_dest_abs"/
  fi

  capture
  echo "Drove M15 autonomy reconnect capture in $tui_session"
}

drive_dropped_completion_backpressure() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-dropped-completion-backpressure"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_BACKPRESSURE_PROMPT:-M9 replay-lossy fixture for M18 reconnect-style replay.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving replay-lossy backpressure"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "Replay lossy" "${OCTOS_TUI_SOAK_BACKPRESSURE_WAIT_SECS:-30}" || \
    die "Timed out waiting for replay-lossy status in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-replay-lossy.txt"
  wait_for_tui_text "Done" "${OCTOS_TUI_SOAK_BACKPRESSURE_DONE_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI to settle after replay-lossy fixture"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-backpressure-final.txt"
  capture
  echo "Drove replay-lossy backpressure recovery in $tui_session"
}

drive_interrupt_reconnect() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-interrupt-reconnect"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_INTERRUPT_PROMPT:-M12 interrupt/reconnect fixture: start a long response, then interrupt and resume.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving interrupt/reconnect"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "Working|Thinking|Agent task|Running" "${OCTOS_TUI_SOAK_INTERRUPT_RUNNING_WAIT_SECS:-20}" || \
    die "Timed out waiting for active turn before interrupt"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-interrupt-running.txt"

  tmux send-keys -t "$tui_session" C-c
  wait_for_tui_text "interrupt|cancel|Ask Octos to change code|Done" \
    "${OCTOS_TUI_SOAK_INTERRUPT_DONE_WAIT_SECS:-30}" || \
    die "Timed out waiting for interrupt acknowledgement in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-interrupt.txt"

  if [ "$transport" = "ws" ]; then
    restart_server
    wait_for_tui_text "UI protocol reconnected|Ask Octos to change code|state" \
      "${OCTOS_TUI_SOAK_RECONNECT_WAIT_SECS:-20}" || \
      die "Timed out waiting for TUI to settle after interrupt reconnect"
  else
    send_tui_line "/status"
    wait_for_tui_text "Status|Ask Octos to change code|state" \
      "${OCTOS_TUI_SOAK_STATUS_WAIT_SECS:-10}" || \
      die "Timed out waiting for post-interrupt status capture"
  fi
  capture_pane "$tui_session" "$artifact_dir/tui-capture-interrupt-reconnect.txt"
  capture
  echo "Drove interrupt/reconnect capture in $tui_session"
}

drive_validator_cycle() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-validator-cycle"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_VALIDATOR_PROMPT:-M12 validator fixture: make a tiny change, show one failing validator, fix it, then show the passing validator rerun.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving validator cycle"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "validator|Validator|failed|passed" \
    "${OCTOS_TUI_SOAK_VALIDATOR_WAIT_SECS:-80}" || \
    die "Timed out waiting for validator evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-validator-cycle.txt"
  capture
  echo "Drove validator cycle capture in $tui_session"
}

drive_long_output() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-long-output"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_LONG_OUTPUT_PROMPT:-M12 long-output fixture: run a shell command that prints 40 unique output-line-NN rows so the TUI folds the tool output preview.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving long-output capture"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "more line(s) hidden|Ctrl+O expand|Ctrl+O collapse|output-line-" \
    "${OCTOS_TUI_SOAK_LONG_OUTPUT_WAIT_SECS:-80}" || \
    die "Timed out waiting for long-output folding evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-long-output.txt"
  capture
  echo "Drove long-output folding capture in $tui_session"
}

drive_narrow_terminal() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-narrow-terminal"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local cols="${OCTOS_TUI_SOAK_NARROW_COLS:-80}"
  local rows="${OCTOS_TUI_SOAK_NARROW_ROWS:-24}"
  case "$cols" in ''|*[!0-9]*) die "OCTOS_TUI_SOAK_NARROW_COLS must be numeric: ${cols:-<empty>}" ;; esac
  case "$rows" in ''|*[!0-9]*) die "OCTOS_TUI_SOAK_NARROW_ROWS must be numeric: ${rows:-<empty>}" ;; esac
  [ "$cols" -le 80 ] || die "Narrow terminal cols must be <= 80, got $cols"
  [ "$rows" -le 24 ] || die "Narrow terminal rows must be <= 24, got $rows"

  tmux resize-window -t "$tui_session" -x "$cols" -y "$rows"
  wait_for_tui_text "Ask Octos to change code|state|AppUI capabilities refreshed" \
    "${OCTOS_TUI_SOAK_NARROW_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI ready signal after narrow resize"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-narrow-terminal.txt"
  {
    printf '{\n'
    write_json_string_field schema "octos-tui.narrow-terminal.v1"
    printf '  "cols": %s,\n' "$cols"
    printf '  "rows": %s\n' "$rows"
    printf '}\n'
  } > "$artifact_dir/terminal-size.json"
  capture
  echo "Drove narrow terminal capture in $tui_session at ${cols}x${rows}"
}

drive_diff_artifact() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-diff-artifact"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_DIFF_ARTIFACT_PROMPT:-M12 diff/artifact fixture: make a tiny patch, show the diff preview, and publish an artifact summary.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving diff/artifact capture"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "Diff Preview|diff preview|artifact ready|Artifacts|artifact" \
    "${OCTOS_TUI_SOAK_DIFF_ARTIFACT_WAIT_SECS:-80}" || \
    die "Timed out waiting for diff/artifact evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-diff-artifact.txt"
  capture
  echo "Drove diff/artifact capture in $tui_session"
}

drive_tool_denial() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-tool-denial"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_TOOL_DENIAL_PROMPT:-M12 denied-tool fixture: attempt a policy-blocked shell command and show the tool/denied event in the TUI.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving tool denial"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "tool denied|Tool denied|tool_denied|denied by policy|policy denied" \
    "${OCTOS_TUI_SOAK_TOOL_DENIAL_WAIT_SECS:-80}" || \
    die "Timed out waiting for denied-tool evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-tool-denial.txt"
  capture
  echo "Drove denied-tool capture in $tui_session"
}

drive_tool_success() {
  command -v tmux >/dev/null 2>&1 || die "tmux is required for drive-tool-success"
  if ! tmux has-session -t "$tui_session" 2>/dev/null; then
    die "TUI tmux session is not running: $tui_session"
  fi

  local prompt="${OCTOS_TUI_SOAK_TOOL_SUCCESS_PROMPT:-M12 normal-tool fixture: run a safe shell command and show a successful tool card in the TUI.}"
  wait_for_tui_text "Ask Octos to change code" "${OCTOS_TUI_SOAK_TUI_READY_WAIT_SECS:-20}" || \
    die "Timed out waiting for TUI composer before driving successful tool call"
  submit_composer_prompt "$prompt"
  wait_for_tui_text "Tool|tool|shell|complete|succeeded|Done" \
    "${OCTOS_TUI_SOAK_TOOL_SUCCESS_WAIT_SECS:-80}" || \
    die "Timed out waiting for successful tool-call evidence in TUI"
  capture_pane "$tui_session" "$artifact_dir/tui-capture-tool-success.txt"
  capture
  echo "Drove successful tool-call capture in $tui_session"
}

verify_solo() {
  local required=(
    "$artifact_dir/tui-capture.txt"
    "$artifact_dir/server.log"
    "$artifact_dir/appui-transcript.jsonl"
    "$artifact_dir/runtime-policy-stamp.json"
    "$artifact_dir/tool-registry-snapshot.json"
    "$artifact_dir/approval-events.jsonl"
    "$artifact_dir/filesystem-probe.json"
    "$artifact_dir/soak-summary.json"
  )
  local file
  for file in "${required[@]}"; do
    [ -f "$file" ] || die "M12 solo artifact missing: $file"
  done
  assert_capture_clean "$artifact_dir/tui-capture.txt" "M12 solo"
  if grep -E 'auth/(send_code|verify)' "$artifact_dir/appui-transcript.jsonl" >/dev/null 2>&1; then
    die "M12 solo transcript contains OTP method traffic"
  fi
  if grep -E '"method":"approval/requested"|"method": "approval/requested"' "$artifact_dir/approval-events.jsonl" >/dev/null 2>&1; then
    die "M12 solo approval-never evidence contains approval/requested"
  fi
  local leak_check_files=("$artifact_dir/appui-transcript.jsonl")
  for file in \
    "$artifact_dir/mcp-config-before.redacted.json" \
    "$artifact_dir/mcp-config-after.redacted.json" \
    "$artifact_dir/mcp-status-list.json" \
    "$artifact_dir/mcp-connection-test-result.json"
  do
    if [ -f "$file" ]; then
      leak_check_files+=("$file")
    fi
  done
  if grep -E 'redacted-by-probe|Bearer redacted-by-probe' "${leak_check_files[@]}" >/dev/null 2>&1; then
    die "M12 MCP/tool artifacts contain unredacted fixture secrets"
  fi
  if [ "${OCTOS_TUI_SOAK_SOLO_STRICT:-0}" = "1" ] && [ -f "$artifact_dir/mcp-config-after.redacted.json" ]; then
    if grep -q '"id": "fixture-stdio"' "$artifact_dir/mcp-config-after.redacted.json"; then
      die "M12 MCP strict verification expected deleted fixture-stdio to be absent"
    fi
    if ! grep -q '"id": "fixture-websocket"' "$artifact_dir/mcp-config-after.redacted.json"; then
      die "M12 MCP strict verification expected websocket parity fixture to remain"
    fi
  fi
  if [ "${OCTOS_TUI_SOAK_SOLO_STRICT:-0}" = "1" ]; then
    if ! grep -q '"status": "passed"' "$artifact_dir/soak-summary.json"; then
      die "M12 solo strict verification requires passed soak-summary.json"
    fi
  fi
  local required_solo_cases="${OCTOS_TUI_SOAK_REQUIRED_SOLO_CASES:-}"
  if [ "${OCTOS_TUI_SOAK_SOLO_STRICT:-0}" = "1" ] && [ -z "$required_solo_cases" ]; then
    required_solo_cases="workspace-cwd-open approval-never-sandbox-active danger-full-access-approval-never"
  fi
  verify_solo_required_cases "$required_solo_cases"
  if [ "${OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE:-${OCTOS_TUI_SOAK_TENANT_NEGATIVE:-0}}" = "1" ]; then
    verify_solo_tenant_negative_case
  fi
  write_solo_summary_matrix
  [ -f "$artifact_dir/summary-matrix.md" ] || die "M12 solo summary matrix missing"
  write_ux_validation "solo-onboarding" "passed" "M12 solo soak artifacts verified"
  secret_leak_check
  echo "Verified M12 solo soak artifacts in $artifact_dir"
}

verify_solo_strict_bundle() {
  local expect_tenant_negative="${1:-0}"
  local original_strict="${OCTOS_TUI_SOAK_SOLO_STRICT:-}"
  local original_tenant_negative="${OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE:-}"
  OCTOS_TUI_SOAK_SOLO_STRICT=1
  OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE="$expect_tenant_negative"
  verify_solo
  OCTOS_TUI_SOAK_SOLO_STRICT="$original_strict"
  OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE="$original_tenant_negative"
}

verify_solo_closure() {
  verify_solo_strict_bundle 1

  local multiline_dir="${OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR:-$artifact_dir}"
  local original_artifact_dir="$artifact_dir"
  artifact_dir="$multiline_dir"
  verify_multiline_composer
  artifact_dir="$original_artifact_dir"

  write_ux_validation "solo-closure" "passed" "M12 solo closure bundle verified"
  echo "Verified M12 solo closure bundle in $original_artifact_dir with multiline artifacts in $multiline_dir"
}

verify_solo_transport_closure() {
  local original_artifact_dir="$artifact_dir"
  local stdio_dir="${OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR:-}"
  local ws_dir="${OCTOS_TUI_SOAK_WS_ARTIFACT_DIR:-}"
  [ -n "$stdio_dir" ] || die "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR is required for verify-solo-transport-closure"
  [ -n "$ws_dir" ] || die "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR is required for verify-solo-transport-closure"
  [ -d "$stdio_dir" ] || die "stdio artifact dir missing: $stdio_dir"
  [ -d "$ws_dir" ] || die "WebSocket artifact dir missing: $ws_dir"

  verify_solo_closure

  artifact_dir="$stdio_dir"
  verify_solo_strict_bundle 0

  artifact_dir="$ws_dir"
  verify_solo_strict_bundle 0

  artifact_dir="$original_artifact_dir"
  verify_transport_parity

  write_ux_validation "solo-transport-closure" "passed" "M12 solo transport closure bundle verified"
  echo "Verified M12 solo transport closure bundle with stdio artifacts in $stdio_dir and WebSocket artifacts in $ws_dir"
}

verify_first_launch() {
  local capture_file="$artifact_dir/tui-capture-first-launch.txt"
  assert_capture_clean "$capture_file" "first-launch"

  if [ -f "$artifact_dir/summary.env" ] \
    && ! grep --fixed-strings -- "first_launch_capture=1" "$artifact_dir/summary.env" >/dev/null 2>&1; then
    die "summary.env does not record first_launch_capture=1"
  fi

  for required_text in \
    "Welcome to Octos" \
    "Create your local Octos profile" \
    "OCTOS"
  do
    grep --fixed-strings -- "$required_text" "$capture_file" >/dev/null 2>&1 \
      || die "first-launch capture missing required text: $required_text"
  done

  if grep --fixed-strings -- "Set Up LLM Provider" "$capture_file" >/dev/null 2>&1; then
    die "first-launch capture advanced past the local profile splash"
  fi

  if grep -E 'auth/(send_code|verify)|Email OTP' "$capture_file" >/dev/null 2>&1; then
    die "first-launch capture contains OTP onboarding text"
  fi
  if [ -f "$artifact_dir/server.log" ] \
    && grep -E 'auth/(send_code|verify)' "$artifact_dir/server.log" >/dev/null 2>&1; then
    die "first-launch server log contains OTP method traffic"
  fi

  write_ux_validation "first-launch" "passed" "first-launch onboarding splash verified"
  secret_leak_check
  echo "Verified first-launch onboarding splash in $artifact_dir"
}

verify_provider_missing() {
  local capture_file="$artifact_dir/tui-capture-provider-missing.txt"
  assert_capture_clean "$capture_file" "provider-missing"

  for required_text in \
    "Set Up LLM Provider" \
    "Profile:" \
    "Local profile ready" \
    "API key"
  do
    grep --fixed-strings -- "$required_text" "$capture_file" >/dev/null 2>&1 \
      || die "provider-missing capture missing required text: $required_text"
  done

  grep -E '(Load|Reload) provider catalog' "$capture_file" >/dev/null 2>&1 \
    || die "provider-missing capture missing provider catalog action"

  if grep --fixed-strings -- "Welcome to Octos" "$capture_file" >/dev/null 2>&1; then
    die "provider-missing capture is still on the first-launch splash"
  fi
  if grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1; then
    die "provider-missing capture already opened a coding session"
  fi
  if grep -E 'auth/(send_code|verify)|Email OTP|Task Error|app-ui error|malformed_json' \
    "$capture_file" >/dev/null 2>&1; then
    die "provider-missing capture contains onboarding or AppUI error text"
  fi

  write_ux_validation "provider-missing" "passed" "missing-provider recovery capture verified"
  secret_leak_check
  echo "Verified provider-missing onboarding recovery in $artifact_dir"
}

verify_permissions() {
  local open_capture="$artifact_dir/tui-capture-permissions-open.txt"
  local applied_capture="$artifact_dir/tui-capture-permissions-applied.txt"
  assert_capture_clean "$open_capture" "permissions-open"
  assert_capture_clean "$applied_capture" "permissions-applied"

  for required_text in \
    "Update Model Permissions" \
    "Default" \
    "Read Only" \
    "Workspace Write"
  do
    grep --fixed-strings -- "$required_text" "$open_capture" >/dev/null 2>&1 \
      || die "permissions-open capture missing required text: $required_text"
  done

  for required_text in \
    "Permissions updated: Workspace Write" \
    "Workspace Write, Never Ask" \
    "Ask Octos to change code"
  do
    grep --fixed-strings -- "$required_text" "$applied_capture" >/dev/null 2>&1 \
      || die "permissions-applied capture missing required text: $required_text"
  done

  if grep --fixed-strings -- "Set Up LLM Provider" "$applied_capture" >/dev/null 2>&1; then
    die "permissions capture is still on provider setup"
  fi
  if grep -E 'Task Error|app-ui error|malformed_json|unsupported method|unavailable: AppUI capabilities' \
    "$open_capture" "$applied_capture" >/dev/null 2>&1; then
    die "permissions capture contains AppUI error text"
  fi

  write_ux_validation "permissions" "passed" "permissions selection captures verified"
  secret_leak_check
  echo "Verified permissions onboarding selection in $artifact_dir"
}

verify_approval_denial() {
  local request_capture="$artifact_dir/tui-capture-approval-request.txt"
  local denied_capture="$artifact_dir/tui-capture-approval-denied.txt"
  assert_capture_clean "$request_capture" "approval-request"
  assert_capture_clean "$denied_capture" "approval-denied"

  for required_text in \
    "Approval Requested" \
    "tool shell" \
    "kind command" \
    "n = deny it"
  do
    grep --fixed-strings -- "$required_text" "$request_capture" >/dev/null 2>&1 \
      || die "approval request capture missing required text: $required_text"
  done

  for required_text in \
    "approval denied" \
    "decision  deny" \
    "Ask Octos to change code"
  do
    grep --fixed-strings -- "$required_text" "$denied_capture" >/dev/null 2>&1 \
      || die "approval denied capture missing required text: $required_text"
  done
  grep --fixed-strings -- "state" "$denied_capture" >/dev/null 2>&1 \
    && grep --fixed-strings -- "Done" "$denied_capture" >/dev/null 2>&1 \
    || die "approval denied capture missing Done status"

  if grep -E 'state ! Blocked|Approval Requested' "$denied_capture" >/dev/null 2>&1; then
    die "approval denied capture still shows a blocked approval prompt"
  fi
  if [ -f "$artifact_dir/validation.json" ] \
    && ! grep --fixed-strings -- '"status": "passed"' "$artifact_dir/validation.json" >/dev/null 2>&1; then
    die "approval-denial validation.json is not passed"
  fi

  write_ux_validation "approval-denial" "passed" "approval denial captures verified"
  secret_leak_check
  echo "Verified approval denial artifacts in $artifact_dir"
}

verify_multiline_composer() {
  local capture_file="$artifact_dir/tui-capture-multiline-composer.txt"
  assert_capture_clean "$capture_file" "multiline-composer"

  for required_text in \
    "Composer" \
    "first instruction" \
    "second instruction" \
    "third instruction"
  do
    grep --fixed-strings -- "$required_text" "$capture_file" >/dev/null 2>&1 \
      || die "multiline composer capture missing required text: $required_text"
  done

  if grep -E 'state !.*Blocked|Approval Requested|Task Error|app-ui error|malformed_json' \
    "$capture_file" >/dev/null 2>&1; then
    die "multiline composer capture contains blocked or AppUI error text"
  fi

  write_ux_validation "multiline-composer" "passed" "multiline composer capture verified"
  secret_leak_check
  echo "Verified multiline composer capture in $artifact_dir"
}

verify_runtime_menus() {
  local status_capture="$artifact_dir/tui-capture-runtime-status.txt"
  local model_capture="$artifact_dir/tui-capture-runtime-model.txt"
  local mcp_capture="$artifact_dir/tui-capture-runtime-mcp.txt"
  local transcript
  transcript="$(first_existing_artifact "runtime menu AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$status_capture" "runtime-status"
  assert_capture_clean "$model_capture" "runtime-model"
  assert_capture_clean "$mcp_capture" "runtime-mcp"

  for required_text in \
    "Status" \
    "Profile" \
    "Model"
  do
    grep --fixed-strings --ignore-case -- "$required_text" "$status_capture" "$model_capture" >/dev/null 2>&1 \
      || die "runtime menu captures missing required text: $required_text"
  done
  grep -Ei 'provider|route|configured|profile/llm' "$model_capture" >/dev/null 2>&1 \
    || die "runtime model capture missing server-backed provider/model surface"
  grep -E 'MCP|mcp/status/list|mcp/config/list|Configured MCP|No MCP|server' "$mcp_capture" >/dev/null 2>&1 \
    || die "runtime MCP capture missing MCP status/config surface"

  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"session/status/read"' \
    "$transcript" >/dev/null 2>&1 \
    || die "runtime menu transcript missing session/status/read request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"profile/llm/list"' \
    "$transcript" >/dev/null 2>&1 \
    || die "runtime menu transcript missing profile/llm/list request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"mcp/(status/list|config/list)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "runtime menu transcript missing MCP status/config request"

  write_ux_validation "runtime-menus" "passed" "runtime status/model/MCP menu captures verified"
  secret_leak_check
  echo "Verified runtime menu captures in $artifact_dir"
}

verify_backpressure() {
  local replay_capture="$artifact_dir/tui-capture-replay-lossy.txt"
  local final_capture="$artifact_dir/tui-capture-backpressure-final.txt"
  local server_log="$artifact_dir/server.log"
  local transcript
  transcript="$(first_existing_artifact "backpressure AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$replay_capture" "backpressure-replay-lossy"
  assert_capture_clean "$final_capture" "backpressure-final"
  [ -f "$server_log" ] || die "backpressure server log missing: $server_log"

  grep --fixed-strings -- "Replay lossy" "$replay_capture" "$final_capture" >/dev/null 2>&1 \
    || die "backpressure captures missing Replay lossy status"
  grep --fixed-strings -- "Ask Octos to change code" "$final_capture" >/dev/null 2>&1 \
    || die "backpressure final capture missing usable composer"
  grep --fixed-strings -- "Done" "$final_capture" >/dev/null 2>&1 \
    || die "backpressure final capture did not settle to Done"

  "$script_dir/validate-tmux-ux-capture.sh" "$final_capture" "$server_log" >/dev/null

  grep -E '"direction"[[:space:]]*:[[:space:]]*"(server_to_client|rx)".*"method"[[:space:]]*:[[:space:]]*"protocol/replay_lossy"' \
    "$transcript" >/dev/null 2>&1 \
    || die "backpressure transcript missing protocol/replay_lossy notification"
  if grep -E 'lifecycle notification not delivered.*turn/completed|writer channel full for lifecycle frame|lifecycle ws send failed; aborting connection' \
    "$server_log" >/dev/null 2>&1; then
    die "backpressure server log contains dropped turn/completed lifecycle notification"
  fi

  write_ux_validation "dropped-completion-backpressure" "passed" "dropped-completion backpressure captures verified"
  secret_leak_check
  echo "Verified dropped-completion backpressure artifacts in $artifact_dir"
}

verify_interrupt_reconnect() {
  local running_capture="$artifact_dir/tui-capture-interrupt-running.txt"
  local interrupt_capture="$artifact_dir/tui-capture-interrupt.txt"
  local reconnect_capture="$artifact_dir/tui-capture-interrupt-reconnect.txt"
  local transcript
  transcript="$(first_existing_artifact "interrupt/reconnect AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$running_capture" "interrupt-running"
  assert_capture_clean "$interrupt_capture" "interrupt"
  assert_capture_clean "$reconnect_capture" "interrupt-reconnect"

  grep -Ei 'Working|Thinking|Agent task|Running' "$running_capture" >/dev/null 2>&1 \
    || die "interrupt running capture missing active turn state"
  grep -Ei 'interrupt|cancel' "$interrupt_capture" >/dev/null 2>&1 \
    || die "interrupt capture missing interrupt/cancel acknowledgement"
  grep --fixed-strings -- "Ask Octos to change code" "$interrupt_capture" "$reconnect_capture" >/dev/null 2>&1 \
    || die "interrupt/reconnect captures missing usable composer"
  grep -E 'UI protocol reconnected|Status|state' "$reconnect_capture" >/dev/null 2>&1 \
    || die "interrupt reconnect capture missing reconnect/status evidence"

  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"turn/interrupt"' \
    "$transcript" >/dev/null 2>&1 \
    || die "interrupt transcript missing turn/interrupt request"
  grep -E '"method"[[:space:]]*:[[:space:]]*"(session/open|session/status/read|session/snapshot|session/hydrate|config/capabilities/list|protocol/replay_lossy)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "interrupt/reconnect transcript missing session hydration/status evidence"
  if grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"(turn/completed|protocol/replay_lossy)"' \
    "$transcript" >/dev/null 2>&1; then
    die "interrupt/reconnect transcript shows client-owned lifecycle/replay notification traffic"
  fi

  write_ux_validation "interrupt-reconnect" "passed" "interrupt and reconnect capture verified"
  secret_leak_check
  echo "Verified interrupt/reconnect artifacts in $artifact_dir"
}

verify_validator_cycle() {
  local capture_file="$artifact_dir/tui-capture-validator-cycle.txt"
  local validator_results="$artifact_dir/validator-results.jsonl"
  local transcript
  transcript="$(first_existing_artifact "validator cycle AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$capture_file" "validator-cycle"
  [ -s "$validator_results" ] || die "validator-results artifact missing or empty: $validator_results"

  grep -Ei 'validator|validation|check' "$capture_file" >/dev/null 2>&1 \
    || die "validator capture missing validator/check text"
  grep -Ei 'failed|fail' "$capture_file" >/dev/null 2>&1 \
    || die "validator capture missing failed validator state"
  grep -Ei 'passed|pass' "$capture_file" >/dev/null 2>&1 \
    || die "validator capture missing passed validator state"
  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "validator capture missing usable composer"

  grep -E '"status"[[:space:]]*:[[:space:]]*"failed"' "$validator_results" >/dev/null 2>&1 \
    || die "validator-results.jsonl missing failed status"
  grep -E '"status"[[:space:]]*:[[:space:]]*"passed"' "$validator_results" >/dev/null 2>&1 \
    || die "validator-results.jsonl missing passed status"
  grep -E '"name"[[:space:]]*:' "$validator_results" >/dev/null 2>&1 \
    || die "validator-results.jsonl missing validator name"

  if grep -E '"status"[[:space:]]*:[[:space:]]*"(failed|passed)"' "$validator_results" \
    | awk '/"failed"/ {failed=NR} /"passed"/ {passed=NR; exit} END {exit !(failed && passed && failed < passed)}'; then
    :
  else
    die "validator-results.jsonl must record failed status before passed rerun"
  fi

  grep -E '"method"[[:space:]]*:[[:space:]]*"(turn/start|task/updated|agent/updated)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "validator cycle transcript missing turn/task evidence"

  write_ux_validation "validator-cycle" "passed" "validator fail/pass cycle verified"
  secret_leak_check
  echo "Verified validator cycle artifacts in $artifact_dir"
}

verify_long_output() {
  local capture_file="$artifact_dir/tui-capture-long-output.txt"
  local transcript
  transcript="$(first_existing_artifact "long-output AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$capture_file" "long-output"

  grep -E '[0-9]+ more line\(s\) hidden \(Ctrl\+O (expand|collapse)\)' "$capture_file" >/dev/null 2>&1 \
    || die "long-output capture missing folded-output marker"
  grep -Ei 'output-line|tool|shell|command|stdout|preview' "$capture_file" >/dev/null 2>&1 \
    || die "long-output capture missing command/tool output text"
  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "long-output capture missing usable composer"
  if grep -E '^┌(Work|Progress).*›|^┌Wor ›|^┌Progress.*›' "$capture_file" >/dev/null 2>&1; then
    die "long-output capture shows input overlap with removed work/progress pane"
  fi

  grep -E '"method"[[:space:]]*:[[:space:]]*"(turn/start|task/output/delta|task/output/read|agent/output/delta|agent/output/read)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "long-output transcript missing turn/output method evidence"

  write_ux_validation "long-output" "passed" "long-output folding capture verified"
  secret_leak_check
  echo "Verified long-output folding artifacts in $artifact_dir"
}

verify_narrow_terminal() {
  local capture_file="$artifact_dir/tui-capture-narrow-terminal.txt"
  local terminal_size_json="$artifact_dir/terminal-size.json"
  local server_log="$artifact_dir/server.log"
  local cols
  local rows

  assert_capture_clean "$capture_file" "narrow-terminal"
  [ -f "$terminal_size_json" ] || die "narrow-terminal size artifact missing: $terminal_size_json"
  cols="$(json_scalar_value "$terminal_size_json" cols)"
  rows="$(json_scalar_value "$terminal_size_json" rows)"
  case "$cols" in ''|*[!0-9]*) die "narrow terminal cols invalid: ${cols:-<missing>}" ;; esac
  case "$rows" in ''|*[!0-9]*) die "narrow terminal rows invalid: ${rows:-<missing>}" ;; esac
  [ "$cols" -le 80 ] || die "narrow terminal cols must be <= 80, got $cols"
  [ "$rows" -le 24 ] || die "narrow terminal rows must be <= 24, got $rows"

  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "narrow-terminal capture missing usable composer"
  grep --fixed-strings -- "state" "$capture_file" >/dev/null 2>&1 \
    || die "narrow-terminal capture missing status line"
  if [ -f "$server_log" ]; then
    "$script_dir/validate-tmux-ux-capture.sh" "$capture_file" "$server_log" >/dev/null
  else
    "$script_dir/validate-tmux-ux-capture.sh" "$capture_file" >/dev/null
  fi

  write_ux_validation "narrow-terminal" "passed" "narrow terminal capture verified"
  secret_leak_check
  echo "Verified narrow terminal artifacts in $artifact_dir"
}

verify_diff_artifact() {
  local capture_file="$artifact_dir/tui-capture-diff-artifact.txt"
  local artifact_index
  local transcript
  artifact_index="$(first_existing_artifact "diff/artifact artifact index" \
    "$artifact_dir/m15-evidence/artifact-index.json" \
    "$artifact_dir/artifact-index.json")"
  transcript="$(first_existing_artifact "diff/artifact AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$capture_file" "diff-artifact"
  [ -s "$artifact_index" ] || die "diff/artifact index missing or empty: $artifact_index"

  grep -Ei 'Diff Preview|diff preview|patch|modify|apply_patch' "$capture_file" >/dev/null 2>&1 \
    || die "diff/artifact capture missing diff preview text"
  grep -Ei 'Artifacts?|artifact ready|summary.env|artifact-index' "$capture_file" >/dev/null 2>&1 \
    || die "diff/artifact capture missing artifact text"
  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "diff/artifact capture missing usable composer"
  grep -E '"artifacts"[[:space:]]*:[[:space:]]*\[' "$artifact_index" >/dev/null 2>&1 \
    || die "diff/artifact index missing artifacts array"
  grep -Ei 'diff preview ready|diff.preview.ready|artifact ready|artifact.ready|task/updated|artifact/updated|task/artifact' \
    "$transcript" >/dev/null 2>&1 \
    || die "diff/artifact transcript missing diff or artifact readiness evidence"

  write_ux_validation "diff-artifact" "passed" "diff/artifact readiness capture verified"
  secret_leak_check
  echo "Verified diff/artifact artifacts in $artifact_dir"
}

verify_tool_denial() {
  local capture_file="$artifact_dir/tui-capture-tool-denial.txt"
  local transcript
  transcript="$(first_existing_artifact "tool-denial AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$capture_file" "tool-denial"
  grep -Ei 'tool denied|tool_denied|denied by policy|policy denied|blocked' "$capture_file" >/dev/null 2>&1 \
    || die "tool-denial capture missing denied-policy text"
  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "tool-denial capture missing usable composer"
  grep -E '"method"[[:space:]]*:[[:space:]]*"tool/denied"|tool[.]denied|tool_denied' \
    "$transcript" >/dev/null 2>&1 \
    || die "tool-denial transcript missing tool/denied evidence"
  if grep -E '"method"[[:space:]]*:[[:space:]]*"approval/requested"' "$transcript" >/dev/null 2>&1; then
    die "tool-denial transcript contains approval/requested; expected direct blocked-policy evidence"
  fi

  write_ux_validation "tool-denial" "passed" "denied-tool capture verified"
  secret_leak_check
  echo "Verified denied-tool artifacts in $artifact_dir"
}

verify_tool_success() {
  local capture_file="$artifact_dir/tui-capture-tool-success.txt"
  local transcript
  transcript="$(first_existing_artifact "tool-success AppUI transcript" \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl")"

  assert_capture_clean "$capture_file" "tool-success"
  grep -Ei 'tool|shell|command|exec|stdout|complete|succeeded|success' "$capture_file" >/dev/null 2>&1 \
    || die "tool-success capture missing successful tool-call text"
  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "tool-success capture missing usable composer"
  grep -E '"method"[[:space:]]*:[[:space:]]*"(turn/start|task/output/delta|task/output/read|agent/output/delta|agent/output/read|tool/status/list)"|activity[.]tool[.](progress|complete)|"tool"[[:space:]]*:' \
    "$transcript" >/dev/null 2>&1 \
    || die "tool-success transcript missing turn/tool output evidence"
  if grep -E '"method"[[:space:]]*:[[:space:]]*"tool/denied"|tool[.]denied|tool_denied' "$transcript" >/dev/null 2>&1; then
    die "tool-success transcript contains denied-tool evidence"
  fi

  write_ux_validation "tool-success" "passed" "successful tool-call capture verified"
  secret_leak_check
  echo "Verified successful tool-call artifacts in $artifact_dir"
}

verify_task_subagent_tree() {
  local running_capture="$artifact_dir/tui-capture-task-subagent-tree-running.txt"
  local final_capture="$artifact_dir/tui-capture-task-subagent-tree-final.txt"
  local summary_capture="$artifact_dir/tui-capture-task-subagent-tree-summary.txt"
  assert_capture_clean "$running_capture" "task-subagent-running"
  assert_capture_clean "$final_capture" "task-subagent-final"
  assert_capture_clean "$summary_capture" "task-subagent-summary"

  for required_text in \
    "Subagents" \
    "Artifacts"
  do
    grep --fixed-strings -- "$required_text" "$running_capture" "$final_capture" >/dev/null 2>&1 \
      || die "task-subagent captures missing required text: $required_text"
  done

  for required_text in \
    "M15CODEREVIEWFINALLINE" \
    "Ask Octos to change code"
  do
    grep --fixed-strings -- "$required_text" "$final_capture" >/dev/null 2>&1 \
      || die "task-subagent final capture missing required text: $required_text"
  done

  tr '\n' ' ' < "$summary_capture" | grep -E 'Code[[:space:]]+Review[[:space:]]+Summary' >/dev/null 2>&1 \
    || die "task-subagent summary capture missing Code Review Summary"

  local transcript
  local task_ledger
  local artifact_index
  transcript="$(first_existing_artifact "task-subagent AppUI transcript" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl" \
    "$artifact_dir/appui-transcript.jsonl")"
  task_ledger="$(first_existing_artifact "task-subagent task ledger" \
    "$artifact_dir/m15-evidence/task-ledger.jsonl" \
    "$artifact_dir/task-ledger.jsonl")"
  artifact_index="$(first_existing_artifact "task-subagent artifact index" \
    "$artifact_dir/m15-evidence/artifact-index.json" \
    "$artifact_dir/artifact-index.json")"

  grep -E '"event"[[:space:]]*:[[:space:]]*"task_started"' "$task_ledger" >/dev/null 2>&1 \
    || die "task-subagent task ledger missing task_started event"
  grep -E '"event"[[:space:]]*:[[:space:]]*"task_completed"' "$task_ledger" >/dev/null 2>&1 \
    || die "task-subagent task ledger missing task_completed event"
  grep -E '"artifacts"[[:space:]]*:' "$artifact_index" >/dev/null 2>&1 \
    || die "task-subagent artifact index missing artifacts array"

  if grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"task/(spawn|send|join)"' \
    "$transcript" >/dev/null 2>&1; then
    die "task-subagent transcript contains client-owned task control methods"
  fi
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"(turn/start|review/start)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "task-subagent transcript missing turn/start or review/start request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(server_to_client|rx)".*"method"[[:space:]]*:[[:space:]]*"(task/updated|agent/updated)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "task-subagent transcript missing backend task/agent update notification"

  write_ux_validation "task-subagent-tree" "passed" "task/subagent tree captures verified"
  secret_leak_check
  echo "Verified task/subagent tree artifacts in $artifact_dir"
}

verify_task_subagent_reconnect() {
  local server_restart_capture="$artifact_dir/server-pane-after-restart.txt"
  local reconnect_capture
  reconnect_capture="$(first_existing_artifact "task-subagent reconnect capture" \
    "$artifact_dir/tui-capture-task-subagent-tree-reconnect.txt" \
    "$artifact_dir/tui-capture.txt")"
  local transcript
  transcript="$(first_existing_artifact "task-subagent reconnect AppUI transcript" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl" \
    "$artifact_dir/appui-transcript.jsonl")"

  assert_capture_clean "$server_restart_capture" "task-subagent-server-after-restart"
  assert_capture_clean "$reconnect_capture" "task-subagent-reconnect"

  grep --fixed-strings -- "Ask Octos to change code" "$reconnect_capture" >/dev/null 2>&1 \
    || die "task-subagent reconnect capture missing usable composer"
  grep --fixed-strings -- "state" "$reconnect_capture" >/dev/null 2>&1 \
    || die "task-subagent reconnect capture missing status line"

  grep -E '"method"[[:space:]]*:[[:space:]]*"(session/open|session/status/read|agent/list|session/goal/get|loop/list|task/list)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "task-subagent reconnect transcript missing reconnect/hydration method evidence"

  write_ux_validation "task-subagent-reconnect" "passed" "task/subagent reconnect capture verified"
  secret_leak_check
  echo "Verified task/subagent reconnect artifacts in $artifact_dir"
}

verify_task_subagent_old_server_fallback() {
  local capture_file
  capture_file="$(first_existing_artifact "task-subagent old-server fallback capture" \
    "$artifact_dir/tui-capture-task-subagent-old-server-fallback.txt" \
    "$artifact_dir/tui-capture.txt")"
  local transcript
  transcript="$(first_existing_artifact "task-subagent old-server fallback AppUI transcript" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl" \
    "$artifact_dir/appui-transcript.jsonl")"

  assert_capture_clean "$capture_file" "task-subagent old-server fallback"
  grep --fixed-strings -- "Ask Octos to change code" "$capture_file" >/dev/null 2>&1 \
    || die "task-subagent old-server fallback capture missing usable composer"
  grep --fixed-strings -- "state" "$capture_file" >/dev/null 2>&1 \
    || die "task-subagent old-server fallback capture missing status line"

  if grep -Ei 'Subagents|Task[[:space:]]+Inspector|task/artifact|artifact/list|review/start' \
    "$capture_file" >/dev/null 2>&1; then
    die "task-subagent old-server fallback capture shows hidden inspection controls"
  fi
  if grep -E 'harness[.]task_supervision_inspection[.]v1|harness[.]task_artifacts[.]v1' \
    "$transcript" >/dev/null 2>&1; then
    die "task-subagent old-server fallback transcript advertises supervised task capabilities"
  fi
  if grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"(review/start|task/list|task/artifact/(list|read))"' \
    "$transcript" >/dev/null 2>&1; then
    die "task-subagent old-server fallback transcript probes inspection methods"
  fi

  write_ux_validation "task-subagent-old-server-fallback" "passed" "old-server task/subagent fallback capture verified"
  secret_leak_check
  echo "Verified task/subagent old-server fallback artifacts in $artifact_dir"
}

verify_task_subagent_closure() {
  local original_artifact_dir="$artifact_dir"
  verify_task_subagent_tree

  local reconnect_dir="${OCTOS_TUI_SOAK_TASK_RECONNECT_ARTIFACT_DIR:-$original_artifact_dir}"
  artifact_dir="$reconnect_dir"
  verify_task_subagent_reconnect

  local old_server_dir="${OCTOS_TUI_SOAK_TASK_OLD_SERVER_ARTIFACT_DIR:-$original_artifact_dir}"
  artifact_dir="$old_server_dir"
  verify_task_subagent_old_server_fallback

  artifact_dir="$original_artifact_dir"
  verify_transport_parity

  write_ux_validation "task-subagent-closure" "passed" "M13 task/subagent closure bundle verified"
  echo "Verified M13 task/subagent closure bundle in $original_artifact_dir"
}

verify_autonomy_live() {
  local capture_file
  capture_file="$(first_existing_artifact "M15 autonomy TUI capture" \
    "$artifact_dir/tui-capture-autonomy-live.txt" \
    "$artifact_dir/tui-capture.txt")"
  local transcript
  transcript="$(first_existing_artifact "M15 autonomy AppUI transcript" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl" \
    "$artifact_dir/appui-transcript.jsonl")"
  local runtime_policy
  runtime_policy="$(first_existing_artifact "M15 autonomy runtime policy stamp" \
    "$artifact_dir/m15-evidence/runtime-policy-stamp.json" \
    "$artifact_dir/runtime-policy-stamp.json")"
  local agent_ledger
  agent_ledger="$(first_existing_artifact "M15 autonomy agent ledger" \
    "$artifact_dir/m15-evidence/agent-ledger.jsonl" \
    "$artifact_dir/agent-ledger.jsonl")"
  local goal_ledger
  goal_ledger="$(first_existing_artifact "M15 autonomy goal ledger" \
    "$artifact_dir/m15-evidence/goal-ledger.jsonl" \
    "$artifact_dir/goal-ledger.jsonl")"
  local loop_ledger
  loop_ledger="$(first_existing_artifact "M15 autonomy loop ledger" \
    "$artifact_dir/m15-evidence/loop-ledger.jsonl" \
    "$artifact_dir/loop-ledger.jsonl")"
  local task_ledger
  task_ledger="$(first_existing_artifact "M15 autonomy task ledger" \
    "$artifact_dir/m15-evidence/task-ledger.jsonl" \
    "$artifact_dir/task-ledger.jsonl")"
  local artifact_index
  artifact_index="$(first_existing_artifact "M15 autonomy artifact index" \
    "$artifact_dir/m15-evidence/artifact-index.json" \
    "$artifact_dir/artifact-index.json")"

  local required_file
  for required_file in \
    "$capture_file" \
    "$transcript" \
    "$runtime_policy" \
    "$agent_ledger" \
    "$goal_ledger" \
    "$loop_ledger" \
    "$task_ledger" \
    "$artifact_index"
  do
    [ -s "$required_file" ] || die "M15 autonomy artifact missing or empty: $required_file"
  done

  assert_capture_clean "$capture_file" "M15 autonomy"
  for required_text in \
    "Agent" \
    "Goal" \
    "Loop" \
    "Ask Octos to change code"
  do
    grep --fixed-strings -- "$required_text" "$capture_file" >/dev/null 2>&1 \
      || die "M15 autonomy capture missing required text: $required_text"
  done
  grep -Ei 'summary|final|joined answer|completed' "$capture_file" >/dev/null 2>&1 \
    || die "M15 autonomy capture missing visible summary/final content"

  if grep -E 'm15-fixture-appui-backend|fixture-only|deterministic fixture|fake-openai|M15_CODE_REVIEW_FINAL_LINE|M15CODEREVIEWFINALLINE' \
    "$capture_file" \
    "$transcript" \
    "$runtime_policy" \
    "$agent_ledger" \
    "$goal_ledger" \
    "$loop_ledger" \
    "$task_ledger" >/dev/null 2>&1; then
    die "M15 autonomy artifacts contain fixture-only or deterministic marker text"
  fi

  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"(turn/start|review/start)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing turn/start or review/start request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"agent/(list|status/read|output/read|artifact/list)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing agent inspection request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"session/goal/(get|set|clear)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing goal runtime request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"loop/(list|create|fire_now|pause|resume|delete)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing loop runtime request"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(server_to_client|rx)".*"method"[[:space:]]*:[[:space:]]*"agent/(updated|output/delta|artifact/updated)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing backend agent notification"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(server_to_client|rx)".*"method"[[:space:]]*:[[:space:]]*"session/goal/(updated|cleared)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing backend goal notification"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(server_to_client|rx)".*"method"[[:space:]]*:[[:space:]]*"loop/(updated|fired|completed)"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy transcript missing backend loop notification"

  if grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"(agent/updated|agent/output/delta|agent/artifact/updated|session/goal/updated|session/goal/cleared|loop/updated|loop/fired|loop/completed)"' \
    "$transcript" >/dev/null 2>&1; then
    die "M15 autonomy transcript shows TUI-owned notification/timer traffic"
  fi

  grep -E '"event"[[:space:]]*:[[:space:]]*"agent_started"' "$agent_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy agent ledger missing agent_started"
  grep -E '"event"[[:space:]]*:[[:space:]]*"agent_completed"' "$agent_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy agent ledger missing agent_completed"
  grep -E '"event"[[:space:]]*:[[:space:]]*"(goal_started|goal_continuation|goal_updated)"' "$goal_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy goal ledger missing goal event"
  grep -E '"event"[[:space:]]*:[[:space:]]*"(loop_iteration|loop_fired|loop_completed|loop_updated)"' "$loop_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy loop ledger missing loop event"
  grep -E '"event"[[:space:]]*:[[:space:]]*"task_started"' "$task_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy task ledger missing task_started"
  grep -E '"event"[[:space:]]*:[[:space:]]*"task_completed"' "$task_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy task ledger missing task_completed"
  grep -E '"artifacts"[[:space:]]*:[[:space:]]*\[' "$artifact_index" >/dev/null 2>&1 \
    || die "M15 autonomy artifact index missing artifacts array"

  write_ux_validation "autonomy-live" "passed" "M15 autonomy production artifact bundle verified"
  secret_leak_check
  echo "Verified M15 autonomy artifacts in $artifact_dir"
}

verify_autonomy_reconnect() {
  local server_restart_capture="$artifact_dir/server-pane-after-restart.txt"
  local reconnect_capture="$artifact_dir/tui-capture-autonomy-reconnect.txt"
  local transcript
  transcript="$(first_existing_artifact "M15 autonomy reconnect AppUI transcript" \
    "$artifact_dir/m15-evidence/appui-transcript.jsonl" \
    "$artifact_dir/appui-transcript.jsonl")"
  local agent_ledger
  agent_ledger="$(first_existing_artifact "M15 autonomy reconnect agent ledger" \
    "$artifact_dir/m15-evidence/agent-ledger.jsonl" \
    "$artifact_dir/agent-ledger.jsonl")"
  local goal_ledger
  goal_ledger="$(first_existing_artifact "M15 autonomy reconnect goal ledger" \
    "$artifact_dir/m15-evidence/goal-ledger.jsonl" \
    "$artifact_dir/goal-ledger.jsonl")"
  local loop_ledger
  loop_ledger="$(first_existing_artifact "M15 autonomy reconnect loop ledger" \
    "$artifact_dir/m15-evidence/loop-ledger.jsonl" \
    "$artifact_dir/loop-ledger.jsonl")"

  local required_file
  for required_file in \
    "$server_restart_capture" \
    "$reconnect_capture" \
    "$transcript" \
    "$agent_ledger" \
    "$goal_ledger" \
    "$loop_ledger"
  do
    [ -s "$required_file" ] || die "M15 autonomy reconnect artifact missing or empty: $required_file"
  done

  assert_capture_clean "$server_restart_capture" "M15 autonomy server after restart"
  assert_capture_clean "$reconnect_capture" "M15 autonomy reconnect"
  for required_text in \
    "Agent" \
    "Goal" \
    "Loop" \
    "Ask Octos to change code"
  do
    grep --fixed-strings -- "$required_text" "$reconnect_capture" >/dev/null 2>&1 \
      || die "M15 autonomy reconnect capture missing required text: $required_text"
  done
  grep --fixed-strings -- "state" "$reconnect_capture" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect capture missing status line"

  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"session/open"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect transcript missing session/open hydration"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"agent/list"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect transcript missing agent/list hydration"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"session/goal/get"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect transcript missing session/goal/get hydration"
  grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"loop/list"' \
    "$transcript" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect transcript missing loop/list hydration"
  if grep -E '"direction"[[:space:]]*:[[:space:]]*"(client_to_server|tx)".*"method"[[:space:]]*:[[:space:]]*"(agent/updated|agent/output/delta|agent/artifact/updated|session/goal/updated|session/goal/cleared|loop/updated|loop/fired|loop/completed)"' \
    "$transcript" >/dev/null 2>&1; then
    die "M15 autonomy reconnect transcript shows TUI-owned notification/timer traffic"
  fi

  grep -E '"event"[[:space:]]*:[[:space:]]*"agent_(started|completed|hydrated|replayed)"' "$agent_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect agent ledger missing durable agent event"
  grep -E '"event"[[:space:]]*:[[:space:]]*"(goal_started|goal_continuation|goal_updated|goal_hydrated|goal_replayed)"' "$goal_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect goal ledger missing durable goal event"
  grep -E '"event"[[:space:]]*:[[:space:]]*"(loop_iteration|loop_fired|loop_completed|loop_updated|loop_hydrated|loop_replayed)"' "$loop_ledger" >/dev/null 2>&1 \
    || die "M15 autonomy reconnect loop ledger missing durable loop event"

  write_ux_validation "autonomy-reconnect" "passed" "M15 autonomy reconnect hydration verified"
  secret_leak_check
  echo "Verified M15 autonomy reconnect artifacts in $artifact_dir"
}

verify_autonomy_closure() {
  local original_artifact_dir="$artifact_dir"
  verify_autonomy_live

  local reconnect_dir="${OCTOS_TUI_SOAK_AUTONOMY_RECONNECT_ARTIFACT_DIR:-$original_artifact_dir}"
  artifact_dir="$reconnect_dir"
  verify_autonomy_reconnect

  artifact_dir="$original_artifact_dir"
  verify_transport_parity

  write_ux_validation "autonomy-closure" "passed" "M15 autonomy closure bundle verified"
  echo "Verified M15 autonomy closure bundle in $original_artifact_dir"
}

verify_transport_parity() {
  local ws_dir="${OCTOS_TUI_SOAK_WS_ARTIFACT_DIR:-}"
  local stdio_dir="${OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR:-}"
  [ -n "$ws_dir" ] || die "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR is required for verify-transport-parity"
  [ -n "$stdio_dir" ] || die "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR is required for verify-transport-parity"
  [ -d "$ws_dir" ] || die "WebSocket artifact dir missing: $ws_dir"
  [ -d "$stdio_dir" ] || die "stdio artifact dir missing: $stdio_dir"

  local ws_transcript
  local stdio_transcript
  ws_transcript="$(appui_transcript_for_dir "WebSocket transport parity" "$ws_dir")"
  stdio_transcript="$(appui_transcript_for_dir "stdio transport parity" "$stdio_dir")"
  verify_transport_dir_kind "WebSocket transport parity" "$ws_dir" ws
  verify_transport_dir_kind "stdio transport parity" "$stdio_dir" stdio
  secret_leak_check_dir "$ws_dir" "WebSocket transport parity"
  secret_leak_check_dir "$stdio_dir" "stdio transport parity"

  local tmp_dir
  tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/octos-tui-transport-parity.XXXXXX")"
  trap 'rm -rf "$tmp_dir"; trap - RETURN' RETURN

  extract_appui_method_sequence "$ws_transcript" > "$tmp_dir/ws.methods"
  extract_appui_method_sequence "$stdio_transcript" > "$tmp_dir/stdio.methods"
  [ -s "$tmp_dir/ws.methods" ] || die "WebSocket transcript contains no AppUI methods: $ws_transcript"
  [ -s "$tmp_dir/stdio.methods" ] || die "stdio transcript contains no AppUI methods: $stdio_transcript"

  case "$transport_parity_mode" in
    sequence) ;;
    set)
      sort -u "$tmp_dir/ws.methods" -o "$tmp_dir/ws.methods"
      sort -u "$tmp_dir/stdio.methods" -o "$tmp_dir/stdio.methods"
      ;;
    *) die "OCTOS_TUI_SOAK_TRANSPORT_PARITY_MODE must be sequence or set, got: $transport_parity_mode" ;;
  esac

  if ! diff -u "$tmp_dir/ws.methods" "$tmp_dir/stdio.methods" > "$tmp_dir/diff"; then
    cat "$tmp_dir/diff" >&2
    die "transport parity mismatch between WebSocket and stdio AppUI method sequences"
  fi

  echo "Verified $transport_parity_mode transport parity between $ws_dir and $stdio_dir"
}

verify_ux_run() {
  local scenario_json="$artifact_dir/scenario.json"
  local summary_json="$artifact_dir/summary.json"
  local validation_json="$artifact_dir/validation.json"
  local terminal_size_json="$artifact_dir/terminal-size.json"
  local capture_file="$artifact_dir/tui-capture.txt"
  local transcript="$artifact_dir/appui-transcript.jsonl"
  local runtime_policy="$artifact_dir/runtime-policy-stamp.json"
  local server_log="$artifact_dir/server.log"
  local required
  for required in \
    "$scenario_json" \
    "$summary_json" \
    "$validation_json" \
    "$terminal_size_json" \
    "$capture_file" \
    "$transcript" \
    "$runtime_policy" \
    "$server_log"
  do
    [ -f "$required" ] || die "UX run artifact missing: $required"
  done
  assert_capture_clean "$capture_file" "UX run"

  local scenario_id
  local scenario_transport
  local summary_status
  local validation_status
  local cols
  local rows
  scenario_id="$(json_scalar_value "$scenario_json" scenario_id)"
  scenario_transport="$(json_scalar_value "$scenario_json" transport)"
  summary_status="$(json_scalar_value "$summary_json" status)"
  validation_status="$(json_scalar_value "$validation_json" status)"
  cols="$(json_scalar_value "$terminal_size_json" cols)"
  rows="$(json_scalar_value "$terminal_size_json" rows)"

  [ -n "$scenario_id" ] || die "UX run scenario_id missing"
  [ -n "$scenario_transport" ] || die "UX run transport missing"
  [ "$summary_status" = "passed" ] || die "UX run summary status is not passed: ${summary_status:-<missing>}"
  [ "$validation_status" = "passed" ] || die "UX run validation status is not passed: ${validation_status:-<missing>}"
  grep --fixed-strings -- '"real_tmux_launched": true' "$summary_json" >/dev/null 2>&1 \
    || die "UX run summary does not prove real tmux launch"
  grep --fixed-strings -- '"placeholder_artifacts": false' "$summary_json" >/dev/null 2>&1 \
    || die "UX run summary still marks placeholder artifacts"
  grep --fixed-strings -- '"schema": "octos.ux.validation.v1"' "$validation_json" >/dev/null 2>&1 \
    || die "UX run validation schema mismatch"

  if [ -n "${OCTOS_TUI_SOAK_EXPECT_SCENARIO:-}" ] && [ "$scenario_id" != "$OCTOS_TUI_SOAK_EXPECT_SCENARIO" ]; then
    die "Expected UX scenario $OCTOS_TUI_SOAK_EXPECT_SCENARIO, got $scenario_id"
  fi
  if [ -n "${OCTOS_TUI_SOAK_EXPECT_TRANSPORT:-}" ] && [ "$scenario_transport" != "$OCTOS_TUI_SOAK_EXPECT_TRANSPORT" ]; then
    die "Expected UX transport $OCTOS_TUI_SOAK_EXPECT_TRANSPORT, got $scenario_transport"
  fi

  case "$cols" in
    ''|*[!0-9]*) die "UX run terminal cols invalid: ${cols:-<missing>}" ;;
  esac
  case "$rows" in
    ''|*[!0-9]*) die "UX run terminal rows invalid: ${rows:-<missing>}" ;;
  esac
  [ "$cols" -gt 0 ] && [ "$rows" -gt 0 ] || die "UX run terminal size must be positive"
  grep --fixed-strings -- '"id": "screen_geometry_consistent"' "$validation_json" >/dev/null 2>&1 \
    || die "UX run validation missing screen_geometry_consistent check"

  for required in \
    "Ask Octos to change code" \
    "state"
  do
    grep --fixed-strings -- "$required" "$capture_file" >/dev/null 2>&1 \
      || die "UX run capture missing required text: $required"
  done
  for required in \
    '"method":"config/capabilities/list"' \
    '"method":"session/open"' \
    '"method":"session/status/read"'
  do
    grep --fixed-strings -- "$required" "$transcript" >/dev/null 2>&1 \
      || die "UX run transcript missing required method: $required"
  done

  write_ux_validation "$scenario_id" "passed" "M19 UX run artifacts verified"
  secret_leak_check
  echo "Verified M19 UX run artifacts in $artifact_dir"
}

self_test_solo() {
  local probe="$octos_repo/scripts/m12-solo-appui-soak.sh"
  [ -x "$probe" ] || die "M12 solo soak wrapper missing or not executable: $probe"
  "$probe" self-test
}

self_test() {
  local tmp_root
  tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/octos-tui-soak-self-test.XXXXXX")"
  local self_test_server_session="octos-tui-soak-selftest-server-$$"
  local self_test_tui_session="octos-tui-soak-selftest-tui-$$"
  command -v tmux >/dev/null 2>&1 || die "tmux is required for self-test"
  local child_env=(
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/artifacts"
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/data"
    "OCTOS_TUI_SOAK_WORKSPACE=$tmp_root/workspace"
    "OCTOS_TUI_SOAK_RUN_ID=selftest"
    "OCTOS_TUI_SOAK_SERVER_SESSION=$self_test_server_session"
    "OCTOS_TUI_SOAK_TUI_SESSION=$self_test_tui_session"
    "OCTOS_TUI_SOAK_PROFILE=coding"
    "OCTOS_TUI_SOAK_REQUIRE_PROFILE=1"
    "OCTOS_TUI_SOAK_EXPECT_FAMILY=moonshot"
    "OCTOS_TUI_SOAK_EXPECT_MODEL=kimi-k2.5"
    "OCTOS_TUI_SOAK_EXPECT_ROUTE=autodl"
    "OCTOS_TUI_SOAK_EXPECT_BASE_URL=https://www.autodl.art/api/v1"
    "OCTOS_TUI_SOAK_API_KEY=selftest-secret"
  )
  cleanup_self_test() {
    if [ -n "${self_test_server_session:-}" ]; then
      tmux kill-session -t "$self_test_server_session" >/dev/null 2>&1 || true
    fi
    if [ -n "${self_test_tui_session:-}" ]; then
      tmux kill-session -t "$self_test_tui_session" >/dev/null 2>&1 || true
    fi
    if [ -n "${tmp_root:-}" ]; then
      rm -rf "$tmp_root"
    fi
  }
  trap cleanup_self_test EXIT
  mkdir -p "$tmp_root/data/profiles" "$tmp_root/workspace"
  tmux kill-session -t "$self_test_server_session" >/dev/null 2>&1 || true
  tmux kill-session -t "$self_test_tui_session" >/dev/null 2>&1 || true
  tmux new-session -d -s "$self_test_server_session" "printf 'Synthetic self-test server pane\n'; sleep 600"
  tmux new-session -d -s "$self_test_tui_session" "printf 'Ask Octos to change code\nOCTOS self-test\n'; sleep 600"
  local fake_octos_bin="$tmp_root/fake-octos-preflight"
  local fake_tui_bin="$tmp_root/fake-octos-tui-preflight"
  cat > "$fake_octos_bin" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "serve" ] && [ "${2:-}" = "--help" ]; then
  printf 'serve help\n'
  exit 0
fi
if [ "${1:-}" = "--version" ]; then
  printf 'octos 0.0.0-self-test\n'
  exit 0
fi
exit 0
SH
  cat > "$fake_tui_bin" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "--version" ]; then
  printf 'octos-tui 0.0.0-self-test\n'
  exit 0
fi
exit 0
SH
  chmod +x "$fake_octos_bin" "$fake_tui_bin"
  env \
    "OCTOS_BIN=$fake_octos_bin" \
    "OCTOS_TUI_BIN=$fake_tui_bin" \
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/preflight-empty-data" \
    "OCTOS_TUI_SOAK_ARTIFACT_ROOT=$tmp_root/preflight-artifacts" \
    "OCTOS_TUI_SOAK_RUN_ID=preflight-provider-ok" \
    "OCTOS_TUI_SOAK_API_KEY=selftest-secret" \
    "$0" preflight-live >/dev/null
  [ -f "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" ] \
    || die "self-test expected provider-backed preflight artifact"
  grep -F '"status": "passed"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected provider-backed preflight to pass"
  grep -F '"host": "' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected host field in preflight artifact"
  grep -F '"os": "' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected os field in preflight artifact"
  grep -F '"profile_id": "coding"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected profile id in preflight artifact"
  grep -F '"session_id": "coding:local:onboarding#preflight-provider-ok"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected session id in preflight artifact"
  grep -F '"octos_version": "octos 0.0.0-self-test"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected octos version in preflight artifact"
  grep -F '"octos_version_status": "passed"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected octos version status in preflight artifact"
  grep -F '"octos_tui_version": "octos-tui 0.0.0-self-test"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected octos-tui version in preflight artifact"
  grep -F '"octos_tui_version_status": "passed"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected octos-tui version status in preflight artifact"
  grep -F '"octos_repo_commit": "' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected octos repo commit field in preflight artifact"
  grep -F '"octos_tui_repo_commit": "' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected octos-tui repo commit field in preflight artifact"
  grep -F '"provider_env_vars_checked": "OPENAI_API_KEY ANTHROPIC_API_KEY DEEPSEEK_API_KEY OPENROUTER_API_KEY MOONSHOT_API_KEY KIMI_API_KEY AUTODL_API_KEY"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected provider env names in preflight artifact"
  grep -F '"tmux_version": "tmux ' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected tmux version in preflight artifact"
  grep -F '"tmux_version_status": "passed"' "$tmp_root/preflight-artifacts/preflight-provider-ok/live-preflight.json" >/dev/null \
    || die "self-test expected tmux version status in preflight artifact"
  local fake_bin_dir="$tmp_root/fake-bin"
  mkdir -p "$fake_bin_dir"
  cat > "$fake_bin_dir/tmux" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "-V" ]; then
  exit 2
fi
exit 0
SH
  chmod +x "$fake_bin_dir/tmux"
  env \
    "PATH=$fake_bin_dir:$PATH" \
    "OCTOS_BIN=$fake_octos_bin" \
    "OCTOS_TUI_BIN=$fake_tui_bin" \
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/preflight-empty-data" \
    "OCTOS_TUI_SOAK_ARTIFACT_ROOT=$tmp_root/preflight-artifacts" \
    "OCTOS_TUI_SOAK_RUN_ID=preflight-tmux-version-unsupported" \
    "OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER=0" \
    "$0" preflight-live >/dev/null
  grep -F '"tmux_version": "unsupported"' "$tmp_root/preflight-artifacts/preflight-tmux-version-unsupported/live-preflight.json" >/dev/null \
    || die "self-test expected unsupported tmux version in preflight artifact"
  grep -F '"tmux_version_status": "unsupported"' "$tmp_root/preflight-artifacts/preflight-tmux-version-unsupported/live-preflight.json" >/dev/null \
    || die "self-test expected unsupported tmux version status in preflight artifact"
  if env \
    "OCTOS_BIN=$fake_octos_bin" \
    "OCTOS_TUI_BIN=$fake_tui_bin" \
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/preflight-empty-data" \
    "OCTOS_TUI_SOAK_ARTIFACT_ROOT=$tmp_root/preflight-artifacts" \
    "OCTOS_TUI_SOAK_RUN_ID=preflight-provider-missing" \
    "$0" preflight-live >/dev/null 2>&1; then
    die "self-test expected live preflight to fail without provider credentials"
  fi
  [ -f "$tmp_root/preflight-artifacts/preflight-provider-missing/live-preflight.json" ] \
    || die "self-test expected failed preflight artifact"
  grep -F '"provider_credential": "missing"' "$tmp_root/preflight-artifacts/preflight-provider-missing/live-preflight.json" >/dev/null \
    || die "self-test expected missing-provider preflight artifact"
  env \
    "OCTOS_BIN=$fake_octos_bin" \
    "OCTOS_TUI_BIN=$fake_tui_bin" \
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/preflight-empty-data" \
    "OCTOS_TUI_SOAK_ARTIFACT_ROOT=$tmp_root/preflight-artifacts" \
    "OCTOS_TUI_SOAK_RUN_ID=preflight-provider-free" \
    "OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER=0" \
    "$0" preflight-live >/dev/null
  [ -f "$tmp_root/preflight-artifacts/preflight-provider-free/live-preflight.json" ] \
    || die "self-test expected provider-free preflight artifact"
  grep -F '"provider_credential": "not required"' "$tmp_root/preflight-artifacts/preflight-provider-free/live-preflight.json" >/dev/null \
    || die "self-test expected provider-free preflight artifact"
  local fake_octos_no_version="$tmp_root/fake-octos-no-version"
  cat > "$fake_octos_no_version" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "serve" ] && [ "${2:-}" = "--help" ]; then
  printf 'serve help\n'
  exit 0
fi
if [ "${1:-}" = "--version" ]; then
  exit 2
fi
exit 0
SH
  chmod +x "$fake_octos_no_version"
  env \
    "OCTOS_BIN=$fake_octos_no_version" \
    "OCTOS_TUI_BIN=$fake_tui_bin" \
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/preflight-empty-data" \
    "OCTOS_TUI_SOAK_ARTIFACT_ROOT=$tmp_root/preflight-artifacts" \
    "OCTOS_TUI_SOAK_RUN_ID=preflight-octos-version-unsupported" \
    "OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER=0" \
    "$0" preflight-live >/dev/null
  grep -F '"octos_version": "unsupported"' "$tmp_root/preflight-artifacts/preflight-octos-version-unsupported/live-preflight.json" >/dev/null \
    || die "self-test expected unsupported octos version in preflight artifact"
  grep -F '"octos_version_status": "unsupported"' "$tmp_root/preflight-artifacts/preflight-octos-version-unsupported/live-preflight.json" >/dev/null \
    || die "self-test expected unsupported octos version status in preflight artifact"
  local fake_tui_no_version="$tmp_root/fake-octos-tui-no-version"
  cat > "$fake_tui_no_version" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "--version" ]; then
  exit 2
fi
exit 0
SH
  chmod +x "$fake_tui_no_version"
  env \
    "OCTOS_BIN=$fake_octos_bin" \
    "OCTOS_TUI_BIN=$fake_tui_no_version" \
    "OCTOS_TUI_SOAK_DATA_DIR=$tmp_root/preflight-empty-data" \
    "OCTOS_TUI_SOAK_ARTIFACT_ROOT=$tmp_root/preflight-artifacts" \
    "OCTOS_TUI_SOAK_RUN_ID=preflight-tui-version-unsupported" \
    "OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER=0" \
    "$0" preflight-live >/dev/null
  grep -F '"octos_tui_version": "unsupported"' "$tmp_root/preflight-artifacts/preflight-tui-version-unsupported/live-preflight.json" >/dev/null \
    || die "self-test expected unsupported octos-tui version in preflight artifact"
  grep -F '"octos_tui_version_status": "unsupported"' "$tmp_root/preflight-artifacts/preflight-tui-version-unsupported/live-preflight.json" >/dev/null \
    || die "self-test expected unsupported octos-tui version status in preflight artifact"
  cat > "$tmp_root/data/profiles/coding.json" <<'JSON'
{
  "id": "coding",
  "config": {
    "llm": {
      "primary": {
        "family_id": "moonshot",
        "model_id": "kimi-k2.5",
        "route": {
          "route_id": "autodl",
          "label": "AutoDL",
          "base_url": "https://www.autodl.art/api/v1",
          "api_key_env": "AUTODL_API_KEY",
          "api_type": "openai"
        }
      }
    },
    "env_vars": {
      "AUTODL_API_KEY": "selftest-secret"
    }
  }
}
JSON
  env "${child_env[@]}" "$0" verify-onboard >/dev/null

  [ -f "$tmp_root/artifacts/summary.env" ] || die "self-test missing summary.env"
  grep -F 'octos_repo_commit=' "$tmp_root/artifacts/summary.env" >/dev/null \
    || die "self-test missing octos_repo_commit in summary.env"
  grep -F 'octos_tui_repo_commit=' "$tmp_root/artifacts/summary.env" >/dev/null \
    || die "self-test missing octos_tui_repo_commit in summary.env"
  [ -f "$tmp_root/artifacts/server.log" ] || die "self-test missing server.log"
  [ -f "$tmp_root/artifacts/server-pane.txt" ] || die "self-test missing server-pane.txt"
  [ -f "$tmp_root/artifacts/tui-capture.txt" ] || die "self-test missing tui-capture.txt"
  [ -f "$tmp_root/artifacts/profile-json-after.json" ] || die "self-test missing profile-json-after.json"
  [ -f "$tmp_root/artifacts/runtime-policy-stamp.txt" ] || die "self-test missing runtime-policy-stamp.txt"
  [ -f "$tmp_root/artifacts/runtime-policy-stamp.json" ] || die "self-test missing runtime-policy-stamp.json"
  [ -f "$tmp_root/artifacts/soak-summary.json" ] || die "self-test missing soak-summary.json"
  [ -f "$tmp_root/artifacts/api-parity-checklist.json" ] || die "self-test missing api-parity-checklist.json"
  [ -f "$tmp_root/artifacts/ux-validation.json" ] || die "self-test missing ux-validation.json"
  printf 'first_launch_capture=1\n' >> "$tmp_root/artifacts/summary.env"
  cat > "$tmp_root/artifacts/tui-capture-first-launch.txt" <<'CAPTURE'
Welcome to Octos
Set up a local solo profile to continue.
> Create your local Octos profile - This stays on this machine; no email OTP is sent.
OCTOS
Welcome to Octos - local solo onboarding
CAPTURE
  env "${child_env[@]}" "$0" verify-first-launch >/dev/null

  mkdir -p "$tmp_root/bad-first-launch"
  printf 'first_launch_capture=1\n' > "$tmp_root/bad-first-launch/summary.env"
  printf 'Set Up LLM Provider\n' > "$tmp_root/bad-first-launch/tui-capture-first-launch.txt"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-first-launch" "$0" verify-first-launch >/dev/null 2>&1; then
    die "self-test expected bad first-launch capture verification to fail"
  fi

  cat > "$tmp_root/artifacts/tui-capture-provider-missing.txt" <<'CAPTURE'
Set Up LLM Provider
> Profile: coding - Local profile ready
Load provider catalog - Load dashboard model families and provider routes
API key: not set
CAPTURE
  env "${child_env[@]}" "$0" verify-provider-missing >/dev/null

  mkdir -p "$tmp_root/bad-provider-missing"
  printf 'Welcome to Octos\n' > "$tmp_root/bad-provider-missing/tui-capture-provider-missing.txt"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-provider-missing" "$0" verify-provider-missing >/dev/null 2>&1; then
    die "self-test expected bad provider-missing capture verification to fail"
  fi

  cat > "$tmp_root/artifacts/tui-capture-permissions-open.txt" <<'CAPTURE'
Update Model Permissions
Default - Workspace edits; ask for network/outside.
Read Only - No writes without approval.
Workspace Write - Read/write inside workspace.
CAPTURE
  cat > "$tmp_root/artifacts/tui-capture-permissions-applied.txt" <<'CAPTURE'
Permissions updated: Workspace Write, network blocked
Workspace Write, Never Ask - Read/write inside workspace; deny approval-gated actions.
Ask Octos to change code...
CAPTURE
  env "${child_env[@]}" "$0" verify-permissions >/dev/null

  mkdir -p "$tmp_root/bad-permissions"
  printf 'Update Model Permissions\n' > "$tmp_root/bad-permissions/tui-capture-permissions-open.txt"
  printf 'Set Up LLM Provider\n' > "$tmp_root/bad-permissions/tui-capture-permissions-applied.txt"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-permissions" "$0" verify-permissions >/dev/null 2>&1; then
    die "self-test expected bad permissions capture verification to fail"
  fi

  mkdir -p "$tmp_root/empty-capture"
  : > "$tmp_root/empty-capture/tui-capture-first-launch.txt"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/empty-capture" "$0" verify-first-launch >/dev/null 2>&1; then
    die "self-test expected empty first-launch capture verification to fail"
  fi

  mkdir -p "$tmp_root/error-capture"
  printf 'Update Model Permissions\nunsupported method: profile/set\n' > "$tmp_root/error-capture/tui-capture-permissions-open.txt"
  printf 'Permissions updated: Workspace Write\nAsk Octos to change code...\n' > "$tmp_root/error-capture/tui-capture-permissions-applied.txt"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/error-capture" "$0" verify-permissions >/dev/null 2>&1; then
    die "self-test expected unsupported-method capture verification to fail"
  fi

  mkdir -p "$tmp_root/solo-core"
  cat > "$tmp_root/solo-core/summary.env" <<'SUMMARY'
run_id=solo-core-selftest
transport=stdio
SUMMARY
  cat > "$tmp_root/solo-core/tui-capture.txt" <<'CAPTURE'
Agent task completed
Ask Octos to change code...
state Done | approval gated | coding
CAPTURE
  cp "$tmp_root/solo-core/tui-capture.txt" "$tmp_root/solo-core/tui-capture.before"
  printf 'synthetic server log\n' > "$tmp_root/solo-core/server.log"
  cat > "$tmp_root/solo-core/appui-transcript.jsonl" <<'JSONL'
{"direction":"tx","frame":{"method":"config/capabilities/list"}}
{"direction":"tx","frame":{"method":"session/open"}}
{"direction":"tx","frame":{"method":"session/status/read"}}
JSONL
  printf '{"runtime_mode":"solo","permission_profile":"danger_full_access"}\n' > "$tmp_root/solo-core/runtime-policy-stamp.json"
  printf '{"coding_tool_contract":{"status":"ready","missing_required_tools":[]}}\n' > "$tmp_root/solo-core/tool-registry-snapshot.json"
  printf '{"total":0,"requested":0}\n' > "$tmp_root/solo-core/approval-events.jsonl"
  printf '{"workspace_write":true}\n' > "$tmp_root/solo-core/filesystem-probe.json"
  cat > "$tmp_root/solo-core/soak-summary.json" <<'JSON'
{
  "schema": "octos-m12-solo-appui-soak-v1",
  "status": "passed",
  "transport": "stdio",
  "cases": [
    {"name": "workspace-cwd-open", "status": "ok"},
    {"name": "approval-never-sandbox-active", "status": "ok"},
    {"name": "danger-full-access-approval-never", "status": "ok"},
    {"name": "coding-tool-contract-ready", "status": "ok", "contract_status": "ready", "missing_required_tools": []}
  ]
}
JSON
  env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/solo-core" \
    "OCTOS_TUI_SOAK_SOLO_STRICT=1" \
    "OCTOS_TUI_SOAK_API_KEY=selftest-secret" \
    "$0" verify-solo >/dev/null
  cmp -s "$tmp_root/solo-core/tui-capture.before" "$tmp_root/solo-core/tui-capture.txt" \
    || die "self-test expected verify-solo to preserve retained tui-capture.txt"
  [ -f "$tmp_root/solo-core/summary-matrix.md" ] || die "self-test missing solo summary matrix"
  grep --fixed-strings -- '"scenario": "solo-onboarding"' "$tmp_root/solo-core/ux-validation.json" >/dev/null 2>&1 \
    || die "self-test missing solo-onboarding ux validation"

  cp -R "$tmp_root/solo-core" "$tmp_root/bad-solo-required-case"
  cat > "$tmp_root/bad-solo-required-case/soak-summary.json" <<'JSON'
{
  "schema": "octos-m12-solo-appui-soak-v1",
  "status": "passed",
  "transport": "stdio",
  "cases": [
    {"name": "workspace-cwd-open", "status": "ok"},
    {"name": "approval-never-sandbox-active", "status": "ok"},
    {"name": "danger-full-access-approval-never", "status": "blocked"}
  ]
}
JSON
  if env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-solo-required-case" \
    "OCTOS_TUI_SOAK_SOLO_STRICT=1" \
    "$0" verify-solo >/dev/null 2>&1; then
    die "self-test expected required solo case verification to fail"
  fi

  cp -R "$tmp_root/solo-core" "$tmp_root/solo-tenant-negative"
  cat > "$tmp_root/solo-tenant-negative/soak-summary.json" <<'JSON'
{
  "schema": "octos-m12-solo-appui-soak-v1",
  "status": "passed",
  "transport": "stdio",
  "cases": [
    {"name": "workspace-cwd-open", "status": "ok"},
    {"name": "tenant-danger-rejection", "status": "ok", "result": {"rejected": true, "applied": false}}
  ]
}
JSON
  env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/solo-tenant-negative" \
    "OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE=1" \
    "$0" verify-solo >/dev/null

  cp -R "$tmp_root/solo-core" "$tmp_root/bad-solo-tenant-negative"
  cat > "$tmp_root/bad-solo-tenant-negative/soak-summary.json" <<'JSON'
{
  "schema": "octos-m12-solo-appui-soak-v1",
  "status": "failed",
  "transport": "stdio",
  "cases": [
    {"name": "workspace-cwd-open", "status": "ok"},
    {"name": "tenant-danger-rejection", "status": "failed", "result": {"rejected": false, "applied": true}}
  ]
}
JSON
  if env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-solo-tenant-negative" \
    "OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE=1" \
    "$0" verify-solo >/dev/null 2>&1; then
    die "self-test expected tenant-negative solo verification to fail"
  fi

  mkdir -p "$tmp_root/approval-denial"
  cat > "$tmp_root/approval-denial/tui-capture-approval-request.txt" <<'CAPTURE'
Approval Requested inline
tool shell
kind command risk low
n = deny it
CAPTURE
  cat > "$tmp_root/approval-denial/tui-capture-approval-denied.txt" <<'CAPTURE'
approval denied
decision  deny  decided by coding
Ask Octos to change code...
state Done
CAPTURE
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/approval-denial" "$0" verify-approval-denial >/dev/null

  mkdir -p "$tmp_root/multiline-composer"
  cat > "$tmp_root/multiline-composer/tui-capture-multiline-composer.txt" <<'CAPTURE'
Composer  Enter send | Tab inspector
> first instruction
  second instruction
  third instruction
state Done
CAPTURE
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/multiline-composer" "$0" verify-multiline-composer >/dev/null

  cp -R "$tmp_root/solo-core" "$tmp_root/solo-closure"
  cat > "$tmp_root/solo-closure/soak-summary.json" <<'JSON'
{
  "schema": "octos-m12-solo-appui-soak-v1",
  "status": "passed",
  "transport": "stdio",
  "cases": [
    {"name": "workspace-cwd-open", "status": "ok"},
    {"name": "approval-never-sandbox-active", "status": "ok"},
    {"name": "danger-full-access-approval-never", "status": "ok"},
    {"name": "tenant-danger-rejection", "status": "ok", "result": {"rejected": true, "applied": false}}
  ]
}
JSON
  env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/solo-closure" \
    "OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR=$tmp_root/multiline-composer" \
    "$0" verify-solo-closure >/dev/null
  grep --fixed-strings -- '"scenario": "solo-closure"' "$tmp_root/solo-closure/ux-validation.json" >/dev/null 2>&1 \
    || die "self-test missing solo-closure ux validation"

  cp -R "$tmp_root/solo-closure" "$tmp_root/solo-transport-stdio"
  cp -R "$tmp_root/solo-closure" "$tmp_root/solo-transport-ws"
  cat > "$tmp_root/solo-transport-stdio/summary.env" <<'SUMMARY'
run_id=solo-transport-stdio-selftest
transport=stdio
SUMMARY
  cat > "$tmp_root/solo-transport-ws/summary.env" <<'SUMMARY'
run_id=solo-transport-ws-selftest
transport=ws
SUMMARY
  cat > "$tmp_root/solo-transport-stdio/appui-transcript.jsonl" <<'JSONL'
{"direction":"tx","frame":{"method":"config/capabilities/list"}}
{"direction":"tx","frame":{"method":"session/open"}}
{"direction":"tx","frame":{"method":"session/status/read"}}
JSONL
  cat > "$tmp_root/solo-transport-ws/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"config/capabilities/list"}}
{"direction":"client_to_server","frame":{"method":"session/open"}}
{"direction":"client_to_server","frame":{"method":"session/status/read"}}
JSONL
  env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/solo-closure" \
    "OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR=$tmp_root/multiline-composer" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/solo-transport-stdio" \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/solo-transport-ws" \
    "$0" verify-solo-transport-closure >/dev/null
  grep --fixed-strings -- '"scenario": "solo-transport-closure"' "$tmp_root/solo-closure/ux-validation.json" >/dev/null 2>&1 \
    || die "self-test missing solo-transport-closure ux validation"

  if env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/solo-closure" \
    "OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR=$tmp_root/multiline-composer" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/solo-transport-stdio" \
    "$0" verify-solo-transport-closure >/dev/null 2>&1; then
    die "self-test expected solo transport closure verification to fail without WebSocket artifacts"
  fi

  if env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/solo-closure" \
    "$0" verify-solo-closure >/dev/null 2>&1; then
    die "self-test expected solo closure verification to fail without multiline artifact"
  fi

  mkdir -p "$tmp_root/bad-multiline-composer"
  cat > "$tmp_root/bad-multiline-composer/tui-capture-multiline-composer.txt" <<'CAPTURE'
Composer  Enter send | Tab inspector
> first instruction
  third instruction
CAPTURE
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-multiline-composer" "$0" verify-multiline-composer >/dev/null 2>&1; then
    die "self-test expected incomplete multiline composer verification to fail"
  fi

  mkdir -p "$tmp_root/runtime-menus"
  cat > "$tmp_root/runtime-menus/tui-capture-runtime-status.txt" <<'CAPTURE'
Status
Profile coding
Model server-only-model via local-provider
CAPTURE
  cat > "$tmp_root/runtime-menus/tui-capture-runtime-model.txt" <<'CAPTURE'
Model
Configured provider route from profile/llm/list
server-only-model
CAPTURE
  cat > "$tmp_root/runtime-menus/tui-capture-runtime-mcp.txt" <<'CAPTURE'
MCP
mcp/status/list server fixture-stdio connected
CAPTURE
  cat > "$tmp_root/runtime-menus/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"session/status/read"}}
{"direction":"client_to_server","frame":{"method":"profile/llm/list"}}
{"direction":"client_to_server","frame":{"method":"mcp/status/list"}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/runtime-menus" "$0" verify-runtime-menus >/dev/null

  mkdir -p "$tmp_root/bad-runtime-menus"
  cp "$tmp_root/runtime-menus/tui-capture-runtime-status.txt" "$tmp_root/bad-runtime-menus/"
  cp "$tmp_root/runtime-menus/tui-capture-runtime-model.txt" "$tmp_root/bad-runtime-menus/"
  cp "$tmp_root/runtime-menus/tui-capture-runtime-mcp.txt" "$tmp_root/bad-runtime-menus/"
  cat > "$tmp_root/bad-runtime-menus/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"session/status/read"}}
{"direction":"client_to_server","frame":{"method":"profile/llm/list"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-runtime-menus" "$0" verify-runtime-menus >/dev/null 2>&1; then
    die "self-test expected runtime menu MCP transcript verification to fail"
  fi

  mkdir -p "$tmp_root/backpressure"
  cat > "$tmp_root/backpressure/tui-capture-replay-lossy.txt" <<'CAPTURE'
Replay lossy: 3 dropped (last durable seq 42); reconnect to rehydrate
state Working
CAPTURE
  cat > "$tmp_root/backpressure/tui-capture-backpressure-final.txt" <<'CAPTURE'
Replay lossy: 3 dropped (last durable seq 42); reconnect to rehydrate
Assistant response recovered after replay refresh.
┌Composer
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/backpressure/appui-transcript.jsonl" <<'JSONL'
{"direction":"server_to_client","frame":{"method":"protocol/replay_lossy"}}
JSONL
  printf 'Listening\n' > "$tmp_root/backpressure/server.log"
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/backpressure" "$0" verify-backpressure >/dev/null

  mkdir -p "$tmp_root/bad-backpressure"
  cp "$tmp_root/backpressure/tui-capture-replay-lossy.txt" "$tmp_root/bad-backpressure/"
  cat > "$tmp_root/bad-backpressure/tui-capture-backpressure-final.txt" <<'CAPTURE'
Replay lossy: 3 dropped (last durable seq 42); reconnect to rehydrate
┌Composer
Ask Octos to change code...
state Working
CAPTURE
  cp "$tmp_root/backpressure/appui-transcript.jsonl" "$tmp_root/bad-backpressure/"
  printf 'writer channel full for lifecycle frame turn/completed\n' > "$tmp_root/bad-backpressure/server.log"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-backpressure" "$0" verify-backpressure >/dev/null 2>&1; then
    die "self-test expected dropped turn/completed backpressure verification to fail"
  fi

  mkdir -p "$tmp_root/interrupt-reconnect"
  cat > "$tmp_root/interrupt-reconnect/tui-capture-interrupt-running.txt" <<'CAPTURE'
Assistant is streaming a long answer
state Working
CAPTURE
  cat > "$tmp_root/interrupt-reconnect/tui-capture-interrupt.txt" <<'CAPTURE'
Turn interrupted by client request
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/interrupt-reconnect/tui-capture-interrupt-reconnect.txt" <<'CAPTURE'
UI protocol reconnected to ws://127.0.0.1:50179/api/ui-protocol/ws.
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/interrupt-reconnect/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/interrupt"}}
{"direction":"client_to_server","frame":{"method":"session/status/read"}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/interrupt-reconnect" "$0" verify-interrupt-reconnect >/dev/null

  mkdir -p "$tmp_root/bad-interrupt-reconnect"
  cp "$tmp_root/interrupt-reconnect/tui-capture-interrupt-running.txt" "$tmp_root/bad-interrupt-reconnect/"
  cp "$tmp_root/interrupt-reconnect/tui-capture-interrupt.txt" "$tmp_root/bad-interrupt-reconnect/"
  cp "$tmp_root/interrupt-reconnect/tui-capture-interrupt-reconnect.txt" "$tmp_root/bad-interrupt-reconnect/"
  cat > "$tmp_root/bad-interrupt-reconnect/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"direction":"client_to_server","frame":{"method":"session/status/read"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-interrupt-reconnect" "$0" verify-interrupt-reconnect >/dev/null 2>&1; then
    die "self-test expected interrupt/reconnect verification to fail without turn/interrupt"
  fi

  mkdir -p "$tmp_root/validator-cycle"
  cat > "$tmp_root/validator-cycle/tui-capture-validator-cycle.txt" <<'CAPTURE'
validator cargo fmt --check failed on attempt 1
validator cargo fmt --check passed on attempt 2
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/validator-cycle/validator-results.jsonl" <<'JSONL'
{"name":"cargo fmt --check","status":"failed","attempt":1,"exit_code":1}
{"name":"cargo fmt --check","status":"passed","attempt":2,"exit_code":0}
JSONL
  cat > "$tmp_root/validator-cycle/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"direction":"server_to_client","frame":{"method":"task/updated"}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/validator-cycle" "$0" verify-validator-cycle >/dev/null

  mkdir -p "$tmp_root/bad-validator-cycle"
  cp "$tmp_root/validator-cycle/tui-capture-validator-cycle.txt" "$tmp_root/bad-validator-cycle/"
  cp "$tmp_root/validator-cycle/appui-transcript.jsonl" "$tmp_root/bad-validator-cycle/"
  cat > "$tmp_root/bad-validator-cycle/validator-results.jsonl" <<'JSONL'
{"name":"cargo fmt --check","status":"passed","attempt":1,"exit_code":0}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-validator-cycle" "$0" verify-validator-cycle >/dev/null 2>&1; then
    die "self-test expected validator-cycle verification to fail without failed result"
  fi

  mkdir -p "$tmp_root/long-output"
  cat > "$tmp_root/long-output/tui-capture-long-output.txt" <<'CAPTURE'
tool shell complete
     │ output-line-01-unique
     │ output-line-02-unique
     │ ... 16 more line(s) hidden (Ctrl+O collapse)
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/long-output/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"direction":"server_to_client","frame":{"method":"task/output/delta"}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/long-output" "$0" verify-long-output >/dev/null

  mkdir -p "$tmp_root/bad-long-output"
  cat > "$tmp_root/bad-long-output/tui-capture-long-output.txt" <<'CAPTURE'
tool shell complete
     │ output-line-01-unique
     │ output-line-02-unique
Ask Octos to change code...
state Done
CAPTURE
  cp "$tmp_root/long-output/appui-transcript.jsonl" "$tmp_root/bad-long-output/"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-long-output" "$0" verify-long-output >/dev/null 2>&1; then
    die "self-test expected long-output verification to fail without folded-output marker"
  fi

  mkdir -p "$tmp_root/narrow-terminal"
  cat > "$tmp_root/narrow-terminal/tui-capture-narrow-terminal.txt" <<'CAPTURE'
┌Composer
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/narrow-terminal/terminal-size.json" <<'JSON'
{"schema":"octos-tui.narrow-terminal.v1","cols":80,"rows":24}
JSON
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/narrow-terminal" "$0" verify-narrow-terminal >/dev/null

  mkdir -p "$tmp_root/bad-narrow-terminal"
  cp "$tmp_root/narrow-terminal/tui-capture-narrow-terminal.txt" "$tmp_root/bad-narrow-terminal/"
  cat > "$tmp_root/bad-narrow-terminal/terminal-size.json" <<'JSON'
{"schema":"octos-tui.narrow-terminal.v1","cols":120,"rows":40}
JSON
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-narrow-terminal" "$0" verify-narrow-terminal >/dev/null 2>&1; then
    die "self-test expected narrow-terminal verification to fail for wide geometry"
  fi

  mkdir -p "$tmp_root/diff-artifact"
  cat > "$tmp_root/diff-artifact/tui-capture-diff-artifact.txt" <<'CAPTURE'
Diff Preview
modify src/app.rs | diff preview ready
Artifacts
artifact ready: summary.env
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/diff-artifact/artifact-index.json" <<'JSON'
{"artifacts":[{"id":"summary.env","title":"summary.env"}]}
JSON
  cat > "$tmp_root/diff-artifact/appui-transcript.jsonl" <<'JSONL'
{"event":"diff.preview.ready","message":"modify src/app.rs | diff preview ready"}
{"event":"artifact.ready","message":"artifact ready: summary.env"}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/diff-artifact" "$0" verify-diff-artifact >/dev/null

  mkdir -p "$tmp_root/bad-diff-artifact"
  cp "$tmp_root/diff-artifact/tui-capture-diff-artifact.txt" "$tmp_root/bad-diff-artifact/"
  cp "$tmp_root/diff-artifact/appui-transcript.jsonl" "$tmp_root/bad-diff-artifact/"
  cat > "$tmp_root/bad-diff-artifact/artifact-index.json" <<'JSON'
{"items":[]}
JSON
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-diff-artifact" "$0" verify-diff-artifact >/dev/null 2>&1; then
    die "self-test expected diff/artifact verification to fail without artifact index"
  fi

  mkdir -p "$tmp_root/tool-denial"
  cat > "$tmp_root/tool-denial/tui-capture-tool-denial.txt" <<'CAPTURE'
Tool denied by policy
code tool_denied
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/tool-denial/appui-transcript.jsonl" <<'JSONL'
{"direction":"server_to_client","frame":{"method":"tool/denied","params":{"code":"tool_denied"}}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/tool-denial" "$0" verify-tool-denial >/dev/null

  mkdir -p "$tmp_root/bad-tool-denial"
  cp "$tmp_root/tool-denial/tui-capture-tool-denial.txt" "$tmp_root/bad-tool-denial/"
  cat > "$tmp_root/bad-tool-denial/appui-transcript.jsonl" <<'JSONL'
{"direction":"server_to_client","frame":{"method":"approval/requested"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-tool-denial" "$0" verify-tool-denial >/dev/null 2>&1; then
    die "self-test expected tool-denial verification to fail without tool/denied evidence"
  fi

  mkdir -p "$tmp_root/tool-success"
  cat > "$tmp_root/tool-success/tui-capture-tool-success.txt" <<'CAPTURE'
tool shell complete
stdout: normal tool call completed
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/tool-success/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"event":"activity.tool.complete","tool":"shell","status":"success"}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/tool-success" "$0" verify-tool-success >/dev/null

  mkdir -p "$tmp_root/bad-tool-success"
  cp "$tmp_root/tool-success/tui-capture-tool-success.txt" "$tmp_root/bad-tool-success/"
  cat > "$tmp_root/bad-tool-success/appui-transcript.jsonl" <<'JSONL'
{"direction":"server_to_client","frame":{"method":"tool/denied","params":{"code":"tool_denied"}}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-tool-success" "$0" verify-tool-success >/dev/null 2>&1; then
    die "self-test expected tool-success verification to fail with denied-tool evidence"
  fi

  mkdir -p "$tmp_root/bad-approval-denial"
  cp "$tmp_root/approval-denial/tui-capture-approval-request.txt" "$tmp_root/bad-approval-denial/"
  cat > "$tmp_root/bad-approval-denial/tui-capture-approval-denied.txt" <<'CAPTURE'
Approval Requested inline
approval denied
state ! Blocked
CAPTURE
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-approval-denial" "$0" verify-approval-denial >/dev/null 2>&1; then
    die "self-test expected blocked approval denial verification to fail"
  fi

  mkdir -p "$tmp_root/task-subagent/m15-evidence"
  cat > "$tmp_root/task-subagent/tui-capture-task-subagent-tree-running.txt" <<'CAPTURE'
Subagents
1. Ada Lovelace (reviewer-api) completed: true
Artifacts
- reviewer-api-notes
CAPTURE
  cat > "$tmp_root/task-subagent/tui-capture-task-subagent-tree-final.txt" <<'CAPTURE'
Subagents
1. Ada Lovelace (reviewer-api) completed: true
Artifacts
- reviewer-api-notes
M15_CODE_REVIEW_FINAL_LINE M15CODEREVIEWFINALLINE
Ask Octos to change code...
CAPTURE
  cat > "$tmp_root/task-subagent/tui-capture-task-subagent-tree-summary.txt" <<'CAPTURE'
Code Review Summary
Findings
Subagents
Artifacts
CAPTURE
  cat > "$tmp_root/task-subagent/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"direction":"server_to_client","frame":{"method":"task/updated"}}
JSONL
  cat > "$tmp_root/task-subagent/m15-evidence/task-ledger.jsonl" <<'JSONL'
{"event":"task_started","task_id":"task-1"}
{"event":"task_completed","task_id":"task-1"}
JSONL
  cat > "$tmp_root/task-subagent/m15-evidence/artifact-index.json" <<'JSON'
{"artifacts":[{"id":"reviewer-api-notes"}]}
JSON
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/task-subagent" "$0" verify-task-subagent-tree >/dev/null

  mkdir -p "$tmp_root/bad-task-subagent/m15-evidence"
  cp "$tmp_root/task-subagent/tui-capture-task-subagent-tree-running.txt" "$tmp_root/bad-task-subagent/"
  cp "$tmp_root/task-subagent/tui-capture-task-subagent-tree-final.txt" "$tmp_root/bad-task-subagent/"
  cp "$tmp_root/task-subagent/tui-capture-task-subagent-tree-summary.txt" "$tmp_root/bad-task-subagent/"
  cp "$tmp_root/task-subagent/m15-evidence/task-ledger.jsonl" "$tmp_root/bad-task-subagent/m15-evidence/"
  cp "$tmp_root/task-subagent/m15-evidence/artifact-index.json" "$tmp_root/bad-task-subagent/m15-evidence/"
  cat > "$tmp_root/bad-task-subagent/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"direction":"client_to_server","frame":{"method":"task/spawn"}}
{"direction":"server_to_client","frame":{"method":"task/updated"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-task-subagent" "$0" verify-task-subagent-tree >/dev/null 2>&1; then
    die "self-test expected task-subagent client task-control verification to fail"
  fi

  mkdir -p "$tmp_root/task-subagent-reconnect/m15-evidence"
  cat > "$tmp_root/task-subagent-reconnect/server-pane-after-restart.txt" <<'CAPTURE'
Listening: http://127.0.0.1:50179
CAPTURE
  cat > "$tmp_root/task-subagent-reconnect/tui-capture-task-subagent-tree-reconnect.txt" <<'CAPTURE'
UI protocol reconnected to ws://127.0.0.1:50179/api/ui-protocol/ws.
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/task-subagent-reconnect/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"session/open"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/task-subagent-reconnect" "$0" verify-task-subagent-reconnect >/dev/null

  local fake_task_stdio_bin="$tmp_root/fake-octos-task-stdio"
  local fake_task_stdio_data="$tmp_root/task-stdio-drive-data"
  local fake_task_stdio_logs="$tmp_root/task-stdio-drive-logs"
  local fake_task_stdio_artifacts="$tmp_root/task-subagent-stdio-drive"
  mkdir -p "$fake_task_stdio_data" "$fake_task_stdio_logs" "$fake_task_stdio_artifacts"
  cat > "$fake_task_stdio_bin" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "serve" ] && [ "${2:-}" = "--stdio" ]; then
  while true; do
    sleep 5
  done
fi
exit 0
SH
  chmod +x "$fake_task_stdio_bin"
  "$fake_task_stdio_bin" serve --stdio --data-dir "$fake_task_stdio_data" &
  local fake_task_stdio_pid=$!
  printf '%s\n' "$fake_task_stdio_pid" > "$fake_task_stdio_logs/stdio-backend.pid"
  sleep 0.3
  tmux kill-session -t "$self_test_tui_session" >/dev/null 2>&1 || true
  tmux new-session -d -s "$self_test_tui_session" "printf 'UI protocol reconnected\nAsk Octos to change code...\nstate Done\n'; sleep 600"
  env \
    "${child_env[@]}" \
    "OCTOS_BIN=$fake_task_stdio_bin" \
    "OCTOS_TUI_SOAK_TRANSPORT=stdio" \
    "OCTOS_TUI_SOAK_DATA_DIR=$fake_task_stdio_data" \
    "OCTOS_TUI_SOAK_LOGS_DIR=$fake_task_stdio_logs" \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$fake_task_stdio_artifacts" \
    "$0" drive-task-subagent-reconnect >/dev/null
  wait "$fake_task_stdio_pid" 2>/dev/null || true
  if kill -0 "$fake_task_stdio_pid" 2>/dev/null; then
    die "self-test expected stdio task/subagent reconnect driver to terminate scoped backend"
  fi
  [ -s "$fake_task_stdio_artifacts/server-pane-after-restart.txt" ] \
    || die "self-test missing stdio task/subagent reconnect restart artifact"
  [ -s "$fake_task_stdio_artifacts/tui-capture-task-subagent-tree-reconnect.txt" ] \
    || die "self-test missing stdio task/subagent reconnect capture"

  mkdir -p "$tmp_root/bad-task-subagent-reconnect/m15-evidence"
  cp "$tmp_root/task-subagent-reconnect/server-pane-after-restart.txt" "$tmp_root/bad-task-subagent-reconnect/"
  cp "$tmp_root/task-subagent-reconnect/tui-capture-task-subagent-tree-reconnect.txt" "$tmp_root/bad-task-subagent-reconnect/"
  cat > "$tmp_root/bad-task-subagent-reconnect/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-task-subagent-reconnect" "$0" verify-task-subagent-reconnect >/dev/null 2>&1; then
    die "self-test expected task-subagent reconnect verification to fail"
  fi

  mkdir -p "$tmp_root/task-subagent-old-server"
  cat > "$tmp_root/task-subagent-old-server/tui-capture-task-subagent-old-server-fallback.txt" <<'CAPTURE'
Assistant response rendered without supervised task controls.
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/task-subagent-old-server/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"config/capabilities/list"}}
{"direction":"client_to_server","frame":{"method":"session/open"}}
{"direction":"client_to_server","frame":{"method":"turn/start"}}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/task-subagent-old-server" "$0" verify-task-subagent-old-server-fallback >/dev/null

  mkdir -p "$tmp_root/bad-task-subagent-old-server"
  cat > "$tmp_root/bad-task-subagent-old-server/tui-capture-task-subagent-old-server-fallback.txt" <<'CAPTURE'
Task Inspector
Subagents
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/bad-task-subagent-old-server/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"config/capabilities/list"}}
{"direction":"client_to_server","frame":{"method":"task/artifact/list"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-task-subagent-old-server" "$0" verify-task-subagent-old-server-fallback >/dev/null 2>&1; then
    die "self-test expected task-subagent old-server fallback verification to fail"
  fi

  mkdir -p "$tmp_root/task-subagent-parity-ws/m15-evidence" "$tmp_root/task-subagent-parity-stdio/m15-evidence"
  printf 'transport=ws\n' > "$tmp_root/task-subagent-parity-ws/summary.env"
  printf 'transport=stdio\n' > "$tmp_root/task-subagent-parity-stdio/summary.env"
  cat > "$tmp_root/task-subagent-parity-ws/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"turn/start"}}
{"direction":"server_to_client","frame":{"method":"task/updated"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
JSONL
  cat > "$tmp_root/task-subagent-parity-stdio/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"tx","frame":{"method":"turn/start"}}
{"direction":"rx","frame":{"method":"task/updated"}}
{"direction":"tx","frame":{"method":"agent/list"}}
JSONL
  env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/task-subagent" \
    "OCTOS_TUI_SOAK_TASK_RECONNECT_ARTIFACT_DIR=$tmp_root/task-subagent-reconnect" \
    "OCTOS_TUI_SOAK_TASK_OLD_SERVER_ARTIFACT_DIR=$tmp_root/task-subagent-old-server" \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/task-subagent-parity-ws" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/task-subagent-parity-stdio" \
    "$0" verify-task-subagent-closure >/dev/null
  grep --fixed-strings -- '"scenario": "task-subagent-closure"' "$tmp_root/task-subagent/ux-validation.json" >/dev/null 2>&1 \
    || die "self-test missing task-subagent-closure ux validation"

  if env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/task-subagent" \
    "OCTOS_TUI_SOAK_TASK_RECONNECT_ARTIFACT_DIR=$tmp_root/task-subagent-reconnect" \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/task-subagent-parity-ws" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/task-subagent-parity-stdio" \
    "$0" verify-task-subagent-closure >/dev/null 2>&1; then
    die "self-test expected task-subagent closure verification to fail without old-server artifacts"
  fi

  mkdir -p "$tmp_root/autonomy-live/m15-evidence"
  cat > "$tmp_root/autonomy-live/tui-capture-autonomy-live.txt" <<'CAPTURE'
Agent reviewer-api summary generated by model
Goal active continuation rendered
Loop fire_now completed from backend
Final joined answer completed
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/autonomy-live/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"review/start"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
{"direction":"client_to_server","frame":{"method":"session/goal/get"}}
{"direction":"client_to_server","frame":{"method":"loop/fire_now"}}
{"direction":"server_to_client","frame":{"method":"agent/updated"}}
{"direction":"server_to_client","frame":{"method":"session/goal/updated"}}
{"direction":"server_to_client","frame":{"method":"loop/fired"}}
JSONL
  cat > "$tmp_root/autonomy-live/m15-evidence/runtime-policy-stamp.json" <<'JSON'
{"scenario":"production_autonomy","runtime":"octos-serve","tool_policy_id":"coding-autonomy-v1"}
JSON
  cat > "$tmp_root/autonomy-live/m15-evidence/agent-ledger.jsonl" <<'JSONL'
{"event":"agent_started","agent_id":"reviewer-api"}
{"event":"agent_completed","agent_id":"reviewer-api","summary_kind":"model_generated"}
JSONL
  cat > "$tmp_root/autonomy-live/m15-evidence/goal-ledger.jsonl" <<'JSONL'
{"event":"goal_started","goal_id":"goal-1"}
{"event":"goal_continuation","goal_id":"goal-1","status":"active"}
JSONL
  cat > "$tmp_root/autonomy-live/m15-evidence/loop-ledger.jsonl" <<'JSONL'
{"event":"loop_iteration","loop_id":"loop-1","status":"started"}
{"event":"loop_fired","loop_id":"loop-1","status":"completed"}
JSONL
  cat > "$tmp_root/autonomy-live/m15-evidence/task-ledger.jsonl" <<'JSONL'
{"event":"task_started","task_id":"task-1"}
{"event":"task_completed","task_id":"task-1"}
JSONL
  cat > "$tmp_root/autonomy-live/m15-evidence/artifact-index.json" <<'JSON'
{"artifacts":[{"id":"joined-answer"}]}
JSON
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/autonomy-live" "$0" verify-autonomy-live >/dev/null

  mkdir -p "$tmp_root/bad-autonomy-live/m15-evidence"
  cp "$tmp_root/autonomy-live/tui-capture-autonomy-live.txt" "$tmp_root/bad-autonomy-live/"
  cp "$tmp_root/autonomy-live/m15-evidence/runtime-policy-stamp.json" "$tmp_root/bad-autonomy-live/m15-evidence/"
  cp "$tmp_root/autonomy-live/m15-evidence/agent-ledger.jsonl" "$tmp_root/bad-autonomy-live/m15-evidence/"
  cp "$tmp_root/autonomy-live/m15-evidence/goal-ledger.jsonl" "$tmp_root/bad-autonomy-live/m15-evidence/"
  cp "$tmp_root/autonomy-live/m15-evidence/loop-ledger.jsonl" "$tmp_root/bad-autonomy-live/m15-evidence/"
  cp "$tmp_root/autonomy-live/m15-evidence/task-ledger.jsonl" "$tmp_root/bad-autonomy-live/m15-evidence/"
  cp "$tmp_root/autonomy-live/m15-evidence/artifact-index.json" "$tmp_root/bad-autonomy-live/m15-evidence/"
  cat > "$tmp_root/bad-autonomy-live/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"review/start"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
{"direction":"client_to_server","frame":{"method":"session/goal/get"}}
{"direction":"client_to_server","frame":{"method":"loop/fire_now"}}
{"direction":"client_to_server","frame":{"method":"loop/fired"}}
{"direction":"server_to_client","frame":{"method":"agent/updated"}}
{"direction":"server_to_client","frame":{"method":"session/goal/updated"}}
{"direction":"server_to_client","frame":{"method":"loop/fired"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-autonomy-live" "$0" verify-autonomy-live >/dev/null 2>&1; then
    die "self-test expected autonomy live client notification verification to fail"
  fi

  mkdir -p "$tmp_root/fixture-autonomy-live/m15-evidence"
  cp "$tmp_root/autonomy-live/tui-capture-autonomy-live.txt" "$tmp_root/fixture-autonomy-live/"
  cp "$tmp_root/autonomy-live/m15-evidence/"*.json "$tmp_root/fixture-autonomy-live/m15-evidence/"
  cp "$tmp_root/autonomy-live/m15-evidence/"*.jsonl "$tmp_root/fixture-autonomy-live/m15-evidence/"
  printf 'M15CODEREVIEWFINALLINE\n' >> "$tmp_root/fixture-autonomy-live/tui-capture-autonomy-live.txt"
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/fixture-autonomy-live" "$0" verify-autonomy-live >/dev/null 2>&1; then
    die "self-test expected autonomy live fixture-marker verification to fail"
  fi

  mkdir -p "$tmp_root/autonomy-reconnect/m15-evidence"
  cat > "$tmp_root/autonomy-reconnect/server-pane-after-restart.txt" <<'CAPTURE'
Listening: http://127.0.0.1:50179
CAPTURE
  cat > "$tmp_root/autonomy-reconnect/tui-capture-autonomy-reconnect.txt" <<'CAPTURE'
Agent reviewer-api completed and hydrated
Goal active continuation hydrated
Loop fire_now schedule hydrated
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/autonomy-reconnect/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"session/open"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
{"direction":"client_to_server","frame":{"method":"session/goal/get"}}
{"direction":"client_to_server","frame":{"method":"loop/list"}}
JSONL
  cat > "$tmp_root/autonomy-reconnect/m15-evidence/agent-ledger.jsonl" <<'JSONL'
{"event":"agent_completed","agent_id":"reviewer-api"}
JSONL
  cat > "$tmp_root/autonomy-reconnect/m15-evidence/goal-ledger.jsonl" <<'JSONL'
{"event":"goal_continuation","goal_id":"goal-1","status":"active"}
JSONL
  cat > "$tmp_root/autonomy-reconnect/m15-evidence/loop-ledger.jsonl" <<'JSONL'
{"event":"loop_fired","loop_id":"loop-1","status":"completed"}
JSONL
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/autonomy-reconnect" "$0" verify-autonomy-reconnect >/dev/null

  local fake_stdio_bin="$tmp_root/fake-octos-stdio"
  local fake_stdio_data="$tmp_root/stdio-drive-data"
  local fake_stdio_logs="$tmp_root/stdio-drive-logs"
  local fake_stdio_artifacts="$tmp_root/autonomy-stdio-drive"
  mkdir -p "$fake_stdio_data" "$fake_stdio_logs" "$fake_stdio_artifacts"
  cat > "$fake_stdio_bin" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "serve" ] && [ "${2:-}" = "--stdio" ]; then
  while true; do
    sleep 5
  done
fi
exit 0
SH
  chmod +x "$fake_stdio_bin"
  "$fake_stdio_bin" serve --stdio --data-dir "$fake_stdio_data" &
  local fake_stdio_pid=$!
  printf '%s\n' "$fake_stdio_pid" > "$fake_stdio_logs/stdio-backend.pid"
  sleep 0.3
  tmux kill-session -t "$self_test_tui_session" >/dev/null 2>&1 || true
  tmux new-session -d -s "$self_test_tui_session" "printf 'UI protocol reconnected\nAgent reviewer-api hydrated\nGoal active continuation hydrated\nLoop fire_now schedule hydrated\nAsk Octos to change code...\nstate Done\n'; sleep 600"
  env \
    "${child_env[@]}" \
    "OCTOS_BIN=$fake_stdio_bin" \
    "OCTOS_TUI_SOAK_TRANSPORT=stdio" \
    "OCTOS_TUI_SOAK_DATA_DIR=$fake_stdio_data" \
    "OCTOS_TUI_SOAK_LOGS_DIR=$fake_stdio_logs" \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$fake_stdio_artifacts" \
    "$0" drive-autonomy-reconnect >/dev/null
  wait "$fake_stdio_pid" 2>/dev/null || true
  if kill -0 "$fake_stdio_pid" 2>/dev/null; then
    die "self-test expected stdio autonomy reconnect driver to terminate scoped backend"
  fi
  [ -s "$fake_stdio_artifacts/server-pane-after-restart.txt" ] \
    || die "self-test missing stdio reconnect restart artifact"
  [ -s "$fake_stdio_artifacts/tui-capture-autonomy-reconnect.txt" ] \
    || die "self-test missing stdio reconnect aggregate capture"

  mkdir -p "$tmp_root/bad-autonomy-reconnect/m15-evidence"
  cp "$tmp_root/autonomy-reconnect/server-pane-after-restart.txt" "$tmp_root/bad-autonomy-reconnect/"
  cp "$tmp_root/autonomy-reconnect/tui-capture-autonomy-reconnect.txt" "$tmp_root/bad-autonomy-reconnect/"
  cp "$tmp_root/autonomy-reconnect/m15-evidence/agent-ledger.jsonl" "$tmp_root/bad-autonomy-reconnect/m15-evidence/"
  cp "$tmp_root/autonomy-reconnect/m15-evidence/goal-ledger.jsonl" "$tmp_root/bad-autonomy-reconnect/m15-evidence/"
  cp "$tmp_root/autonomy-reconnect/m15-evidence/loop-ledger.jsonl" "$tmp_root/bad-autonomy-reconnect/m15-evidence/"
  cat > "$tmp_root/bad-autonomy-reconnect/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"session/open"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
{"direction":"client_to_server","frame":{"method":"session/goal/get"}}
{"direction":"client_to_server","frame":{"method":"loop/fired"}}
JSONL
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-autonomy-reconnect" "$0" verify-autonomy-reconnect >/dev/null 2>&1; then
    die "self-test expected autonomy reconnect verification to fail"
  fi

  mkdir -p "$tmp_root/parity-ws/m15-evidence" "$tmp_root/parity-stdio/m15-evidence"
  printf 'transport=ws\n' > "$tmp_root/parity-ws/summary.env"
  printf 'transport=stdio\n' > "$tmp_root/parity-stdio/summary.env"
  cat > "$tmp_root/parity-ws/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"client_to_server","frame":{"method":"session/open"}}
{"direction":"client_to_server","frame":{"method":"agent/list"}}
{"direction":"server_to_client","frame":{"method":"agent/updated"}}
{"direction":"client_to_server","frame":{"method":"loop/list"}}
JSONL
  cat > "$tmp_root/parity-stdio/m15-evidence/appui-transcript.jsonl" <<'JSONL'
{"direction":"tx","frame":{"method":"session/open"}}
{"direction":"tx","frame":{"method":"agent/list"}}
{"direction":"rx","frame":{"method":"agent/updated"}}
{"direction":"tx","frame":{"method":"loop/list"}}
JSONL
  env \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/parity-ws" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/parity-stdio" \
    "$0" verify-transport-parity >/dev/null

  env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/autonomy-live" \
    "OCTOS_TUI_SOAK_AUTONOMY_RECONNECT_ARTIFACT_DIR=$tmp_root/autonomy-reconnect" \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/parity-ws" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/parity-stdio" \
    "$0" verify-autonomy-closure >/dev/null
  grep --fixed-strings -- '"scenario": "autonomy-closure"' "$tmp_root/autonomy-live/ux-validation.json" >/dev/null 2>&1 \
    || die "self-test missing autonomy-closure ux validation"

  if env \
    "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/autonomy-live" \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/parity-ws" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/parity-stdio" \
    "$0" verify-autonomy-closure >/dev/null 2>&1; then
    die "self-test expected autonomy closure verification to fail without reconnect artifacts"
  fi

  mkdir -p "$tmp_root/bad-parity-stdio/m15-evidence"
  cp "$tmp_root/parity-stdio/m15-evidence/appui-transcript.jsonl" "$tmp_root/bad-parity-stdio/m15-evidence/"
  cp "$tmp_root/parity-stdio/summary.env" "$tmp_root/bad-parity-stdio/summary.env"
  printf '{"direction":"tx","frame":{"method":"session/goal/get"}}\n' \
    >> "$tmp_root/bad-parity-stdio/m15-evidence/appui-transcript.jsonl"
  if env \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/parity-ws" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/bad-parity-stdio" \
    "$0" verify-transport-parity >/dev/null 2>&1; then
    die "self-test expected transport parity verification to fail"
  fi

  mkdir -p "$tmp_root/bad-parity-wrong-kind/m15-evidence"
  cp "$tmp_root/parity-ws/m15-evidence/appui-transcript.jsonl" "$tmp_root/bad-parity-wrong-kind/m15-evidence/"
  printf 'transport=stdio\n' > "$tmp_root/bad-parity-wrong-kind/summary.env"
  if env \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/bad-parity-wrong-kind" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/parity-stdio" \
    "$0" verify-transport-parity >/dev/null 2>&1; then
    die "self-test expected transport parity verification to fail on wrong transport kind"
  fi

  mkdir -p "$tmp_root/bad-parity-secret/m15-evidence"
  cp "$tmp_root/parity-ws/m15-evidence/appui-transcript.jsonl" "$tmp_root/bad-parity-secret/m15-evidence/"
  cp "$tmp_root/parity-ws/summary.env" "$tmp_root/bad-parity-secret/summary.env"
  printf 'retained secret: selftest-secret\n' > "$tmp_root/bad-parity-secret/leak.txt"
  if env \
    "OCTOS_TUI_SOAK_API_KEY=selftest-secret" \
    "OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=$tmp_root/bad-parity-secret" \
    "OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=$tmp_root/parity-stdio" \
    "$0" verify-transport-parity >/dev/null 2>&1; then
    die "self-test expected transport parity verification to fail on secret leak"
  fi

  mkdir -p "$tmp_root/ux-run"
  cat > "$tmp_root/ux-run/scenario.json" <<'JSON'
{"schema":"octos.ux.scenario.v1","scenario_id":"narrow-layout","transport":"stdio"}
JSON
  cat > "$tmp_root/ux-run/summary.json" <<'JSON'
{"schema":"octos.ux.summary.v1","status":"passed","ok":true,"real_tmux_launched": true,"placeholder_artifacts": false}
JSON
  cat > "$tmp_root/ux-run/validation.json" <<'JSON'
{"schema": "octos.ux.validation.v1","status":"passed","checks":[{"id": "screen_geometry_consistent","status":"passed"}]}
JSON
  cat > "$tmp_root/ux-run/terminal-size.json" <<'JSON'
{"schema":"octos.ux.terminal_size.v1","cols":80,"rows":24}
JSON
  cat > "$tmp_root/ux-run/tui-capture.txt" <<'CAPTURE'
Agent task completed
Ask Octos to change code...
state Done
CAPTURE
  cat > "$tmp_root/ux-run/appui-transcript.jsonl" <<'JSONL'
{"direction":"tx","method":"config/capabilities/list"}
{"direction":"tx","method":"session/open"}
{"direction":"tx","method":"session/status/read"}
JSONL
  printf '{}\n' > "$tmp_root/ux-run/runtime-policy-stamp.json"
  printf 'Listening\n' > "$tmp_root/ux-run/server.log"
  env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/ux-run" \
    "OCTOS_TUI_SOAK_EXPECT_SCENARIO=narrow-layout" \
    "OCTOS_TUI_SOAK_EXPECT_TRANSPORT=stdio" \
    "$0" verify-ux-run >/dev/null

  mkdir -p "$tmp_root/bad-ux-run"
  cp "$tmp_root/ux-run"/scenario.json "$tmp_root/bad-ux-run/"
  cp "$tmp_root/ux-run"/summary.json "$tmp_root/bad-ux-run/"
  cp "$tmp_root/ux-run"/terminal-size.json "$tmp_root/bad-ux-run/"
  cp "$tmp_root/ux-run"/tui-capture.txt "$tmp_root/bad-ux-run/"
  cp "$tmp_root/ux-run"/appui-transcript.jsonl "$tmp_root/bad-ux-run/"
  cp "$tmp_root/ux-run"/runtime-policy-stamp.json "$tmp_root/bad-ux-run/"
  cp "$tmp_root/ux-run"/server.log "$tmp_root/bad-ux-run/"
  cat > "$tmp_root/bad-ux-run/validation.json" <<'JSON'
{"schema": "octos.ux.validation.v1","status":"failed","checks":[]}
JSON
  if env "OCTOS_TUI_SOAK_ARTIFACT_DIR=$tmp_root/bad-ux-run" "$0" verify-ux-run >/dev/null 2>&1; then
    die "self-test expected failed UX run verification to fail"
  fi

  while IFS= read -r -d '' file; do
    if grep --fixed-strings -- "selftest-secret" "$file" >/dev/null 2>&1; then
      die "self-test secret leaked into artifacts: $file"
    fi
  done < <(find "$tmp_root/artifacts" -type f -print0)

  printf 'leaked selftest-secret\n' > "$tmp_root/artifacts/profile-json-before.json"
  if env "${child_env[@]}" "$0" verify >/dev/null 2>&1; then
    die "self-test expected leak verification to fail"
  fi
  cleanup_self_test
  trap - EXIT
  echo "Self-test passed"
}

stop() {
  if have_tmux; then
    tmux kill-session -t "$tui_session" 2>/dev/null || true
    tmux kill-session -t "$server_session" 2>/dev/null || true
    tmux kill-session -t "$fake_openai_session" 2>/dev/null || true
  fi
  echo "Stopped $server_session, $tui_session, and $fake_openai_session"
}

case "${1:-help}" in
  preflight-live) preflight_live ;;
  start) start ;;
  restart-server) restart_server ;;
  drive-onboard) drive_onboard ;;
  drive-solo) drive_solo ;;
  drive-permissions) drive_permissions ;;
  drive-provider-missing) drive_provider_missing ;;
  drive-approval-denial) drive_approval_denial ;;
  drive-multiline-composer) drive_multiline_composer ;;
  drive-runtime-menus) drive_runtime_menus ;;
  drive-task-subagent-tree) drive_task_subagent_tree ;;
  drive-task-subagent-reconnect) drive_task_subagent_reconnect ;;
  drive-task-subagent-old-server-fallback) drive_task_subagent_old_server_fallback ;;
  drive-autonomy-live) drive_autonomy_live ;;
  drive-autonomy-reconnect) drive_autonomy_reconnect ;;
  drive-dropped-completion-backpressure) drive_dropped_completion_backpressure ;;
  drive-interrupt-reconnect) drive_interrupt_reconnect ;;
  drive-validator-cycle) drive_validator_cycle ;;
  drive-long-output) drive_long_output ;;
  drive-narrow-terminal) drive_narrow_terminal ;;
  drive-diff-artifact) drive_diff_artifact ;;
  drive-tool-denial) drive_tool_denial ;;
  drive-tool-success) drive_tool_success ;;
  capture) capture ;;
  send-turn) send_turn ;;
  verify) verify ;;
  verify-onboard) verify_onboard ;;
  verify-solo) verify_solo ;;
  verify-solo-closure) verify_solo_closure ;;
  verify-solo-transport-closure) verify_solo_transport_closure ;;
  verify-first-launch) verify_first_launch ;;
  verify-provider-missing) verify_provider_missing ;;
  verify-permissions) verify_permissions ;;
  verify-approval-denial) verify_approval_denial ;;
  verify-multiline-composer) verify_multiline_composer ;;
  verify-runtime-menus) verify_runtime_menus ;;
  verify-task-subagent-tree) verify_task_subagent_tree ;;
  verify-task-subagent-reconnect) verify_task_subagent_reconnect ;;
  verify-task-subagent-old-server-fallback) verify_task_subagent_old_server_fallback ;;
  verify-task-subagent-closure) verify_task_subagent_closure ;;
  verify-backpressure) verify_backpressure ;;
  verify-interrupt-reconnect) verify_interrupt_reconnect ;;
  verify-validator-cycle) verify_validator_cycle ;;
  verify-long-output) verify_long_output ;;
  verify-narrow-terminal) verify_narrow_terminal ;;
  verify-diff-artifact) verify_diff_artifact ;;
  verify-tool-denial) verify_tool_denial ;;
  verify-tool-success) verify_tool_success ;;
  verify-autonomy-live) verify_autonomy_live ;;
  verify-autonomy-reconnect) verify_autonomy_reconnect ;;
  verify-autonomy-closure) verify_autonomy_closure ;;
  verify-transport-parity) verify_transport_parity ;;
  verify-ux-run) verify_ux_run ;;
  api-parity) api_parity ;;
  self-test) self_test ;;
  solo-self-test) self_test_solo ;;
  stop) stop ;;
  help|-h|--help) usage ;;
  *) usage; exit 2 ;;
esac
