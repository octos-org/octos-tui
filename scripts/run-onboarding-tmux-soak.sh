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
first_launch_capture="${OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE:-0}"
endpoint="ws://$host:$port/api/ui-protocol/ws"

usage() {
  cat <<'USAGE'
Usage: scripts/run-onboarding-tmux-soak.sh <start|restart-server|drive-onboard|drive-solo|drive-permissions|drive-provider-missing|drive-approval-denial|drive-task-subagent-tree|capture|send-turn|verify|verify-solo|verify-first-launch|verify-provider-missing|verify-permissions|verify-approval-denial|verify-task-subagent-tree|verify-ux-run|api-parity|self-test|solo-self-test|stop|help>

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
  OCTOS_TUI_SOAK_INIT_PROFILE_LLM Set to 1 to pre-seed profile JSON before backend bootstraps.
  OCTOS_TUI_SOAK_TENANT_NEGATIVE Set to 1 to also run tenant/cloud dangerous-mode negative probe.
  OCTOS_TUI_SOAK_SOLO_PROBE_DATA_DIR Optional separate data dir for stdio solo probe.
  OCTOS_TUI_SOAK_FAKE_OPENAI     Set to 1 to start scripts/fake-openai-server.py in tmux.
  OCTOS_TUI_SOAK_FAKE_OPENAI_PORT Local fake OpenAI-compatible port, default 50180.
  OCTOS_TUI_SOAK_FAKE_OPENAI_DELAY_SECS Optional fake API response delay for progress captures.
  OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE Set to 1 to launch without a preselected
                                 profile/session and save tui-capture-first-launch.txt.
  OCTOS_TUI_SOAK_REQUIRE_PROFILE Set to 0 to allow verify without profile JSON.
  OCTOS_TUI_SOAK_SOLO_STRICT     Set to 1 to fail when M12-A/C capability blockers remain.
                                 Also requires MCP/tool fixture mutations to pass
                                 when the backend advertises those methods.

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

secret_leak_check() {
  local secret="${OCTOS_TUI_SOAK_API_KEY:-}"
  local file
  if [ -z "$secret" ]; then
    return 0
  fi
  if [ ! -d "$artifact_dir" ]; then
    return 0
  fi
  while IFS= read -r -d '' file; do
    if [ -f "$file" ] && grep --fixed-strings -- "$secret" "$file" >/dev/null 2>&1; then
      die "Secret leaked into soak artifact: $file"
    fi
  done < <(find "$artifact_dir" -type f -print0)
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
    stdio_cmd="cd $(shell_quote "$workspace") && ${env_prefix}$(shell_quote "$octos_bin") serve --stdio --data-dir $(shell_quote "$data_dir")"
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
  verify -> scripts/run-onboarding-tmux-soak.sh verify
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

verify() {
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

api_parity() {
  write_summary
  write_api_parity_checklist
  echo "Wrote API parity checklist to $artifact_dir/api-parity-checklist.json"
}

solo_probe_args() {
  local probe_transport="$1"
  local stdio_command="${2:-}"
  local local_name="${OCTOS_TUI_SOAK_LOCAL_NAME:-M12 Solo Soak}"
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
  local local_name="${OCTOS_TUI_SOAK_LOCAL_NAME:-M12 Solo Soak}"
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

verify_solo() {
  capture
  local required=(
    "$artifact_dir/tui-capture.txt"
    "$artifact_dir/server.log"
    "$artifact_dir/appui-transcript.jsonl"
    "$artifact_dir/runtime-policy-stamp.json"
    "$artifact_dir/tool-registry-snapshot.json"
    "$artifact_dir/mcp-config-before.redacted.json"
    "$artifact_dir/mcp-config-after.redacted.json"
    "$artifact_dir/mcp-status-list.json"
    "$artifact_dir/mcp-connection-test-result.json"
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
  if grep -E 'redacted-by-probe|Bearer redacted-by-probe' \
    "$artifact_dir/appui-transcript.jsonl" \
    "$artifact_dir/mcp-config-before.redacted.json" \
    "$artifact_dir/mcp-config-after.redacted.json" \
    "$artifact_dir/mcp-status-list.json" \
    "$artifact_dir/mcp-connection-test-result.json" >/dev/null 2>&1; then
    die "M12 MCP/tool artifacts contain unredacted fixture secrets"
  fi
  if [ "${OCTOS_TUI_SOAK_SOLO_STRICT:-0}" = "1" ]; then
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
  write_ux_validation "solo-onboarding" "passed" "M12 solo soak artifacts verified"
  secret_leak_check
  echo "Verified M12 solo soak artifacts in $artifact_dir"
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
    tmux kill-session -t "$self_test_server_session" >/dev/null 2>&1 || true
    tmux kill-session -t "$self_test_tui_session" >/dev/null 2>&1 || true
    rm -rf "$tmp_root"
  }
  trap cleanup_self_test EXIT
  mkdir -p "$tmp_root/data/profiles" "$tmp_root/workspace"
  tmux kill-session -t "$self_test_server_session" >/dev/null 2>&1 || true
  tmux kill-session -t "$self_test_tui_session" >/dev/null 2>&1 || true
  tmux new-session -d -s "$self_test_server_session" "printf 'Synthetic self-test server pane\n'; sleep 600"
  tmux new-session -d -s "$self_test_tui_session" "printf 'Ask Octos to change code\nOCTOS self-test\n'; sleep 600"
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
  env "${child_env[@]}" "$0" verify >/dev/null

  [ -f "$tmp_root/artifacts/summary.env" ] || die "self-test missing summary.env"
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
  start) start ;;
  restart-server) restart_server ;;
  drive-onboard) drive_onboard ;;
  drive-solo) drive_solo ;;
  drive-permissions) drive_permissions ;;
  drive-provider-missing) drive_provider_missing ;;
  drive-approval-denial) drive_approval_denial ;;
  drive-task-subagent-tree) drive_task_subagent_tree ;;
  drive-dropped-completion-backpressure) drive_dropped_completion_backpressure ;;
  capture) capture ;;
  send-turn) send_turn ;;
  verify) verify ;;
  verify-solo) verify_solo ;;
  verify-first-launch) verify_first_launch ;;
  verify-provider-missing) verify_provider_missing ;;
  verify-permissions) verify_permissions ;;
  verify-approval-denial) verify_approval_denial ;;
  verify-task-subagent-tree) verify_task_subagent_tree ;;
  verify-ux-run) verify_ux_run ;;
  api-parity) api_parity ;;
  self-test) self_test ;;
  solo-self-test) self_test_solo ;;
  stop) stop ;;
  help|-h|--help) usage ;;
  *) usage; exit 2 ;;
esac
