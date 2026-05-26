# Onboarding Tmux Soak

This is the live interactive acceptance test for OTP login and dashboard-parity
LLM provider onboarding in octos-tui.

For M12 solo mode, the same runner also has a no-OTP local onboarding lane that
drives AppUI directly through the live tmux environment and captures the runtime
permission evidence required by M12-D/G.

## Runner

```sh
scripts/run-onboarding-tmux-soak.sh start
```

The runner starts two tmux sessions:

- `octos-onboard-server-<run-id>`
- `octos-onboard-tui-<run-id>`

It writes artifacts under:

```text
e2e/test-results-tui-onboarding/<run-id>/
```

The command surface is:

```sh
scripts/run-onboarding-tmux-soak.sh start
scripts/run-onboarding-tmux-soak.sh drive-onboard
scripts/run-onboarding-tmux-soak.sh drive-solo
scripts/run-onboarding-tmux-soak.sh capture
scripts/run-onboarding-tmux-soak.sh send-turn
scripts/run-onboarding-tmux-soak.sh verify
scripts/run-onboarding-tmux-soak.sh verify-solo
scripts/run-onboarding-tmux-soak.sh verify-first-launch
scripts/run-onboarding-tmux-soak.sh verify-provider-missing
scripts/run-onboarding-tmux-soak.sh verify-permissions
scripts/run-onboarding-tmux-soak.sh verify-approval-denial
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-tree
scripts/run-onboarding-tmux-soak.sh verify-ux-run
scripts/run-onboarding-tmux-soak.sh api-parity
scripts/run-onboarding-tmux-soak.sh self-test
scripts/run-onboarding-tmux-soak.sh solo-self-test
scripts/run-onboarding-tmux-soak.sh stop
```

`self-test` is local and synthetic. It does not start the backend; it creates
temporary tmux panes and a temporary profile JSON, runs `verify`, checks
required artifact creation and redaction, then proves verification fails if
`OCTOS_TUI_SOAK_API_KEY` appears in any artifact.

`solo-self-test` delegates to the backend M12 fixture probe. It validates the
solo artifact schema and proves the retained AppUI transcript contains no OTP
method traffic.

## M12 Solo No-OTP Flow

The solo lane calls `profile/local/create` with display name, username, and
email metadata, then opens a cwd-bound session, probes permission profiles, and
drives the MCP/tool config fixture through AppUI:

- `workspace-write`
- `approval_policy=never` with sandbox still active
- `danger-full-access` plus approval-never
- tenant/cloud dangerous rejection, when the backend exposes the policy method
- `mcp/status/list` and `tool/status/list` for configured state
- `mcp/config/upsert` for stdio and websocket fixture servers
- `mcp/config/set_enabled` for disable/enable semantics
- `tool/config/set_enabled` for per-tool disable/enable semantics
- `mcp/config/test` progress and result for stdio and websocket fixtures
- `mcp/config/delete` followed by status refresh showing server truth

Current backends may not have M12-A/C fully wired. In that case the runner keeps
the evidence files and records blockers in `soak-summary.json`; set
`OCTOS_TUI_SOAK_SOLO_STRICT=1` when the backend is ready to require a pass.
MCP/tool config blockers are recorded the same way until the backend advertises
`mcp/config/*`, `mcp/config/test`, and `tool/config/set_enabled`.
Live transports also require `OCTOS_BIN` to point at an API-enabled `octos`
binary that exposes `serve`.

Provider-free stdio dry-run:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=solo-stdio-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh drive-solo

OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-solo
```

WebSocket tmux run:

```sh
OCTOS_TUI_SOAK_TRANSPORT=ws scripts/run-onboarding-tmux-soak.sh start
OCTOS_TUI_SOAK_TRANSPORT=ws OCTOS_TUI_SOAK_RUN_ID=<run-id> scripts/run-onboarding-tmux-soak.sh drive-solo
OCTOS_TUI_SOAK_TRANSPORT=ws OCTOS_TUI_SOAK_RUN_ID=<run-id> scripts/run-onboarding-tmux-soak.sh verify-solo
```

First-launch splash capture:

```sh
OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE=1 \
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=first-launch-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-first-launch
```

This mode is intentionally opt-in. It starts the TUI without a preselected
profile or session, refuses to reuse an existing profile JSON, waits for the
`Welcome to Octos` onboarding surface, and writes
`tui-capture-first-launch.txt` beside the regular `tui-capture.txt`.
`verify-first-launch` checks the retained capture for the local profile splash,
the `OCTOS` wordmark, and absence of OTP/setup-provider text. Use this lane
when collecting M22/M19 evidence for the first-launch splash; keep the default
launch shape for provider-missing and coding-session lanes.

Missing-provider recovery capture:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=provider-missing-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-provider-missing

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-provider-missing
```

`verify-provider-missing` checks `tui-capture-provider-missing.txt` for the
provider setup recovery screen, local profile readiness, provider catalog
action, and API-key row. It fails if the capture is still on the first-launch
splash, has already opened a coding session, or contains OTP/AppUI error text.

Permissions capture:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_INIT_PROFILE_LLM=1 \
OCTOS_TUI_SOAK_API_KEY=<secret-value> \
OCTOS_TUI_SOAK_RUN_ID=permissions-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-permissions

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-permissions
```

`verify-permissions` checks the open and applied permission pane captures for
the server-backed permission menu, `Workspace Write` update acknowledgement,
and a returned coding-session composer. It fails if the capture is still on
provider setup or contains AppUI error text.

## Approval Denial

For M9/M19 approval-denial evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=approval-denial-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-approval-denial

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-approval-denial
```

`verify-approval-denial` checks the retained request and denied captures for a
visible shell approval prompt, the deny action, the returned composer, and a
completed post-denial status. It fails if the final capture still shows a
blocked approval prompt.

## Task/Subagent Tree

For M13 supervised task inspection evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=task-subagent-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-tree

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-tree
```

`verify-task-subagent-tree` checks the running, final, and scrolled summary
captures for visible subagent/artifact output, the final review marker, and a
usable composer. It also checks the retained transcript/ledger evidence and
fails if the TUI issued client-owned `task/spawn`, `task/send`, or `task/join`
calls in the normal backend-supervised review flow.

For the WebSocket reconnect/hydration leg, keep the same run id and run:

```sh
OCTOS_TUI_SOAK_TRANSPORT=ws \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-reconnect

OCTOS_TUI_SOAK_TRANSPORT=ws \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-reconnect
```

`drive-task-subagent-reconnect` restarts the backend tmux session, waits for
the TUI to settle again, and saves
`tui-capture-task-subagent-tree-reconnect.txt`. The verifier checks the restart
capture, the post-reconnect composer/status line, and AppUI hydration method
evidence such as `session/open`, `agent/list`, `session/goal/get`,
`loop/list`, or `task/list`.

## M15 Autonomy Live Artifacts

For M15 production autonomy evidence, point the verifier at a retained live
artifact directory:

```sh
OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-autonomy-live
```

`verify-autonomy-live` checks the capture, AppUI transcript,
`runtime-policy-stamp.json`, `agent-ledger.jsonl`, `goal-ledger.jsonl`,
`loop-ledger.jsonl`, `task-ledger.jsonl`, and `artifact-index.json`. It fails
if production evidence is still deterministic fixture text, if agent/goal/loop
notifications appear to come from the TUI as client traffic, or if the capture
does not visibly show agent, goal, loop, and final summary state.

## Dropped Completion Backpressure

For replay-lossy and dropped-completion regression evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-dropped-completion-backpressure

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-backpressure
```

`verify-backpressure` checks `tui-capture-replay-lossy.txt`,
`tui-capture-backpressure-final.txt`, `server.log`, and
`appui-transcript.jsonl`. It reuses `scripts/validate-tmux-ux-capture.sh`, so
the retained capture fails if the UI remains falsely working after a dropped
`turn/completed` lifecycle notification or if the final composer is hidden.
It also requires a `protocol/replay_lossy` notification in the transcript.

## Interrupt And Reconnect

For live interrupt and reconnect evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-interrupt-reconnect

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-interrupt-reconnect
```

`drive-interrupt-reconnect` starts a long-running prompt, captures the active
turn, sends `Ctrl-C`, then captures the interrupted state and a post-interrupt
status/reconnect pane. In WebSocket mode it restarts the backend before the
final capture.

`verify-interrupt-reconnect` checks
`tui-capture-interrupt-running.txt`, `tui-capture-interrupt.txt`,
`tui-capture-interrupt-reconnect.txt`, and `appui-transcript.jsonl`. The
verifier requires an active-turn capture, a visible interrupt/cancel
acknowledgement, a usable composer after recovery, a client `turn/interrupt`
request, and session hydration/status evidence.

## Validator Fail/Pass Cycle

For validator evidence in a live coding run:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-validator-cycle

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-validator-cycle
```

`verify-validator-cycle` checks `tui-capture-validator-cycle.txt`,
`validator-results.jsonl`, and `appui-transcript.jsonl`. The verifier requires
visible failed and passed validator states, a usable composer after the rerun,
named failed and passed rows in `validator-results.jsonl`, and the failed row
must appear before the passing rerun.

## Long Output Folding

For long-output folding evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-long-output

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-long-output
```

`verify-long-output` checks `tui-capture-long-output.txt` and
`appui-transcript.jsonl`. The verifier requires the rendered
`... N more line(s) hidden (Ctrl+O expand|collapse)` marker, visible tool/output
text, a usable composer after the run, and turn/output method evidence in the
transcript.

## Narrow Terminal

For narrow-terminal evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-narrow-terminal

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-narrow-terminal
```

`drive-narrow-terminal` resizes the TUI tmux window to 80x24 by default and
writes `tui-capture-narrow-terminal.txt` plus `terminal-size.json`.
`verify-narrow-terminal` requires geometry at or below 80x24, a visible
composer, a status line, and the shared tmux UX capture checks for hidden panes,
overlap, leaked markdown markers, and stale running state.

## M19 UX Run Bundle

For M19 runner-owned artifacts, set `OCTOS_TUI_SOAK_ARTIFACT_DIR` to the
scenario directory and run:

```sh
OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-ux/<run-id>/<scenario-id> \
scripts/run-onboarding-tmux-soak.sh verify-ux-run
```

`verify-ux-run` checks the M19 `scenario.json`, `summary.json`,
`validation.json`, `terminal-size.json`, `appui-transcript.jsonl`,
`runtime-policy-stamp.json`, `server.log`, and `tui-capture.txt` bundle. It
requires a passed real-tmux summary, passed validation, declared terminal
geometry with the M19 `screen_geometry_consistent` validator, core AppUI
session methods, and a visible composer/status line.

The solo lane writes these M12 artifacts into
`e2e/test-results-tui-onboarding/<run-id>/`:

- `tui-capture.txt`
- `tui-capture-first-launch.txt` when
  `OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE=1`
- `tui-capture-provider-missing.txt` when running `drive-provider-missing`
- `tui-capture-permissions-open.txt` and
  `tui-capture-permissions-applied.txt` when running `drive-permissions`
- `tui-capture-approval-request.txt` and
  `tui-capture-approval-denied.txt` when running `drive-approval-denial`
- `tui-capture-interrupt-running.txt`, `tui-capture-interrupt.txt`, and
  `tui-capture-interrupt-reconnect.txt` when running
  `drive-interrupt-reconnect`
- `tui-capture-validator-cycle.txt` and `validator-results.jsonl` when running
  `drive-validator-cycle`
- `tui-capture-long-output.txt` when running `drive-long-output`
- `tui-capture-narrow-terminal.txt` and `terminal-size.json` when running
  `drive-narrow-terminal`
- `tui-capture-task-subagent-tree-running.txt`,
  `tui-capture-task-subagent-tree-final.txt`, and
  `tui-capture-task-subagent-tree-summary.txt` when running
  `drive-task-subagent-tree`
- `tui-capture-task-subagent-tree-reconnect.txt` when running
  `drive-task-subagent-reconnect`
- `server.log`
- `appui-transcript.jsonl`
- `runtime-policy-stamp.json`
- `tool-registry-snapshot.json`
- `mcp-config-before.redacted.json`
- `mcp-config-after.redacted.json`
- `mcp-status-list.json`
- `mcp-connection-test-result.json`
- `approval-events.jsonl`
- `filesystem-probe.json`
- `soak-summary.json`

The transcript artifact is redacted for global capability method lists so the
no-OTP assertion is about actual onboarding traffic. The exact outbound AppUI
method names remain visible; the verifier fails if `auth/send_code` or
`auth/verify` appears.

On 249/mini hosts, pin the run id, port, and artifact root:

```sh
OCTOS_TUI_SOAK_RUN_ID="$(hostname)-solo-$(date -u +%Y%m%dT%H%M%SZ)" \
OCTOS_TUI_SOAK_PORT=50249 \
OCTOS_TUI_SOAK_ARTIFACT_ROOT="$PWD/e2e/test-results-tui-onboarding" \
OCTOS_TUI_SOAK_TRANSPORT=stdio \
  scripts/run-onboarding-tmux-soak.sh drive-solo
```

## Manual Flow

Attach to the TUI:

```sh
tmux attach -t octos-onboard-tui-<run-id>
```

Then complete the guided wizard:

1. `/onboard`
   - `/onboard profile <profile-id>`;
   - `/onboard email <address>` and `/onboard send-code` when OTP is required;
   - `/onboard code <otp>` and `/onboard verify`;
   - `/onboard catalog` to load the dashboard-owned provider schema;
   - choose a catalog route from the menu, or use
     `/onboard select <family> <model> <route> [base-url] [api-key-env]`;
   - `/onboard key <secret>`;
   - `/onboard save`;
   - `/onboard finish`.
2. `/model`
   - verify the menu renders only server-returned provider/model state;
   - verify it does not contain hard-coded TUI model defaults.
3. Prompt:
   - send `Reply with exactly OK.`;
   - verify the response completes and the runtime policy stamp uses the
     configured family/model/route.

For deterministic local smoke without hand typing:

```sh
OCTOS_TUI_SOAK_API_KEY=<secret-value> \
scripts/run-onboarding-tmux-soak.sh drive-onboard
```

`drive-onboard` sends only `/onboard` commands through the live tmux TUI and
then captures the pane. The retained artifacts must show masked secrets only.

## Runtime Menus

For M12 runtime-cockpit evidence, capture the server-backed status, model, and
MCP menu surfaces:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-runtime-menus

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-runtime-menus
```

`verify-runtime-menus` checks `tui-capture-runtime-status.txt`,
`tui-capture-runtime-model.txt`, `tui-capture-runtime-mcp.txt`, and
`appui-transcript.jsonl`. The verifier requires visible profile/model/MCP
surfaces plus outbound `session/status/read`, `profile/llm/list`, and
`mcp/status/list` or `mcp/config/list` requests, so a capture cannot pass only
because the TUI rendered local placeholder text.

## Verify

For Moonshot AutoDL:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=moonshot \
OCTOS_TUI_SOAK_EXPECT_MODEL=kimi-k2.5 \
OCTOS_TUI_SOAK_EXPECT_ROUTE=autodl \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=https://www.autodl.art/api/v1 \
scripts/run-onboarding-tmux-soak.sh verify
```

For MiniMax WiseModel:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=minimax \
OCTOS_TUI_SOAK_EXPECT_MODEL=MiniMax-M2.5-highspeed \
OCTOS_TUI_SOAK_EXPECT_ROUTE=wisemodel \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=https://open.ospreyai.cn/v1 \
scripts/run-onboarding-tmux-soak.sh verify
```

For a custom OpenAI-compatible route:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=<custom-family-id> \
OCTOS_TUI_SOAK_EXPECT_MODEL=<custom-model-id> \
OCTOS_TUI_SOAK_EXPECT_ROUTE=<custom-route-id> \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=<custom-base-url> \
scripts/run-onboarding-tmux-soak.sh verify
```

To check secret redaction, pass the test key as an environment variable only:

```sh
OCTOS_TUI_SOAK_API_KEY=<secret-value> scripts/run-onboarding-tmux-soak.sh verify
```

The verifier fails if that value appears in any retained evidence artifact
listed below. `profile-json-after.json` is always redacted before the leak
check. Raw backend state under the run data directory is not treated as retained
evidence because it may legitimately contain the provider secret while the live
server is running. If a run is only validating captures and no profile file is
expected, set
`OCTOS_TUI_SOAK_REQUIRE_PROFILE=0`; provider expectation variables still require
profile JSON.

Each verifier writes `ux-validation.json` with the run id, scenario, transport,
artifact directory, status, and timestamp. Treat it as the machine-readable
summary for the retained pane captures and JSONL evidence; keep
`soak-summary.json` for provider/profile-specific details.

Verifier-backed pane captures must be non-empty and must not contain tmux
capture failures, hidden task errors, malformed AppUI frames, unsupported-method
spam, or panic/traceback text.

## Required Artifacts

- `summary.env`
- `ux-validation.json`
- `server.log`
- `server-pane.txt`
- `tui-capture.txt`
- `profile-json-before.json` when present
- `profile-json-after.json` with `config.env_vars` redacted
- `runtime-policy-stamp.txt`
- `runtime-policy-stamp.json`
- `soak-summary.json`

The runner also writes `api-parity-checklist.json` as lightweight evidence for
the profile API parity contract below.

## API Contract Checks

The live run must be backed by automated API tests in `octos`. This repository
records the expected equivalence checklist via:

```sh
scripts/run-onboarding-tmux-soak.sh api-parity
```

The checklist covers these cases:

- catalog API returns the dashboard provider catalog shape;
- upsert API persists Moonshot AutoDL route exactly;
- upsert API persists MiniMax WiseModel route exactly;
- upsert API persists a custom OpenAI-compatible family/model/route exactly;
- provider test API does not return or log secrets;
- dashboard REST/profile patch and AppUI upsert produce equivalent profile JSON.

For parity comparison, normalize by redacting `config.env_vars` values and
ignoring timestamp/order-only differences. The values that must match are
`config.llm.primary.family_id`, `model_id`, `route.route_id`,
`route.base_url`, `route.api_key_env`, `route.api_type`, and the env var keys.
