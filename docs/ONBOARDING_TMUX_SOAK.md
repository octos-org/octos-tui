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

The command surface is grouped as:

```sh
scripts/run-onboarding-tmux-soak.sh preflight-live
scripts/run-onboarding-tmux-soak.sh start
scripts/run-onboarding-tmux-soak.sh restart-server
scripts/run-onboarding-tmux-soak.sh stop

scripts/run-onboarding-tmux-soak.sh drive-onboard
scripts/run-onboarding-tmux-soak.sh drive-solo
scripts/run-onboarding-tmux-soak.sh drive-permissions
scripts/run-onboarding-tmux-soak.sh drive-provider-missing
scripts/run-onboarding-tmux-soak.sh drive-approval-denial
scripts/run-onboarding-tmux-soak.sh drive-multiline-composer
scripts/run-onboarding-tmux-soak.sh drive-runtime-menus
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-tree
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-reconnect
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-old-server-fallback
scripts/run-onboarding-tmux-soak.sh drive-autonomy-live
scripts/run-onboarding-tmux-soak.sh drive-autonomy-reconnect
scripts/run-onboarding-tmux-soak.sh drive-dropped-completion-backpressure
scripts/run-onboarding-tmux-soak.sh drive-interrupt-reconnect
scripts/run-onboarding-tmux-soak.sh drive-validator-cycle
scripts/run-onboarding-tmux-soak.sh drive-long-output
scripts/run-onboarding-tmux-soak.sh drive-narrow-terminal
scripts/run-onboarding-tmux-soak.sh drive-diff-artifact
scripts/run-onboarding-tmux-soak.sh drive-tool-denial
scripts/run-onboarding-tmux-soak.sh drive-tool-success

scripts/run-onboarding-tmux-soak.sh capture
scripts/run-onboarding-tmux-soak.sh send-turn

scripts/run-onboarding-tmux-soak.sh verify
scripts/run-onboarding-tmux-soak.sh verify-onboard
scripts/run-onboarding-tmux-soak.sh verify-solo
scripts/run-onboarding-tmux-soak.sh verify-solo-closure
scripts/run-onboarding-tmux-soak.sh verify-solo-transport-closure
scripts/run-onboarding-tmux-soak.sh verify-first-launch
scripts/run-onboarding-tmux-soak.sh verify-provider-missing
scripts/run-onboarding-tmux-soak.sh verify-permissions
scripts/run-onboarding-tmux-soak.sh verify-approval-denial
scripts/run-onboarding-tmux-soak.sh verify-multiline-composer
scripts/run-onboarding-tmux-soak.sh verify-runtime-menus
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-tree
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-reconnect
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-old-server-fallback
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-closure
scripts/run-onboarding-tmux-soak.sh verify-backpressure
scripts/run-onboarding-tmux-soak.sh verify-interrupt-reconnect
scripts/run-onboarding-tmux-soak.sh verify-validator-cycle
scripts/run-onboarding-tmux-soak.sh verify-long-output
scripts/run-onboarding-tmux-soak.sh verify-narrow-terminal
scripts/run-onboarding-tmux-soak.sh verify-diff-artifact
scripts/run-onboarding-tmux-soak.sh verify-tool-denial
scripts/run-onboarding-tmux-soak.sh verify-tool-success
scripts/run-onboarding-tmux-soak.sh verify-autonomy-live
scripts/run-onboarding-tmux-soak.sh verify-autonomy-reconnect
scripts/run-onboarding-tmux-soak.sh verify-autonomy-closure
scripts/run-onboarding-tmux-soak.sh verify-transport-parity
scripts/run-onboarding-tmux-soak.sh verify-ux-run

scripts/run-onboarding-tmux-soak.sh api-parity
scripts/run-onboarding-tmux-soak.sh self-test
scripts/run-onboarding-tmux-soak.sh solo-self-test
scripts/run-onboarding-tmux-soak.sh help
```

`self-test` is local and synthetic. It does not start the backend; it creates
temporary tmux panes and a temporary profile JSON, runs `verify`, checks
required artifact creation and redaction, then proves verification fails if
`OCTOS_TUI_SOAK_API_KEY` appears in any artifact.

`solo-self-test` delegates to the backend M12 fixture probe. It validates the
solo artifact schema and proves the retained AppUI transcript contains no OTP
method traffic.

## Live Preflight

Run the live preflight before validation-only closure attempts for M12/M13/M15
tmux evidence:

```sh
OCTOS_BIN=/path/to/octos \
OCTOS_TUI_BIN=/path/to/octos-tui \
scripts/run-onboarding-tmux-soak.sh preflight-live
```

`preflight-live` checks for tmux, an API-enabled `octos serve`, an executable
`octos-tui`, and a provider credential source. By default the provider source
can be `OCTOS_TUI_SOAK_API_KEY`, one of `OCTOS_TUI_SOAK_PROVIDER_ENV_VARS`, or
pre-seeded profile `env_vars`. Set `OCTOS_TUI_SOAK_REQUIRE_LIVE_PROVIDER=0`
only for provider-free dry runs that cannot close #31, #40, or #44. The command
writes `live-preflight.json` into the retained artifact directory even when a
required check fails, so blocker comments can link the exact failed readiness
state without exposing provider secrets. The JSON includes host and OS details,
profile/session/runtime context, the tmux version, backend `octos --version`
output when available, and the `octos-tui --version` status for the executable
under test. It records the provider environment variable names checked, but
never their values. When the source checkouts are available, it also records the
`octos` and `octos-tui` Git commits used by the run.

## Live Closure Checklist

Use this checklist before posting closure evidence for #31, #40, or #44. A
provider-free run can prove harness readiness, but it is not closure evidence
for these issues because each acceptance row requires provider-backed live TUI
behavior.

For each issue, keep the retained artifact directories outside generated
workspace paths that should not be committed. Then post a GitHub issue comment
with:

- the `octos` and `octos-tui` commits;
- host, OS, tmux version, transport, and provider credential source without the
  secret value;
- exact commands;
- retained artifact paths;
- verifier result and any remaining gaps.

Minimum closeable bundle per issue:

- #31 solo/dangerous mode: one strict stdio run, one strict WebSocket run,
  tenant/cloud dangerous-mode rejection evidence, multiline composer evidence,
  and a passing `verify-solo-transport-closure`.
- #40 supervised task inspection: one stdio run, one WebSocket run,
  reconnect/hydration evidence, old-server fallback evidence, and a passing
  `verify-task-subagent-closure`.
- #44 production autonomy: one stdio run, one WebSocket run,
  production autonomy evidence, reconnect/hydration evidence, and a passing
  `verify-autonomy-closure`.

Run live preflight first:

```sh
OCTOS_BIN=/path/to/octos \
OCTOS_TUI_BIN=/path/to/octos-tui \
scripts/run-onboarding-tmux-soak.sh preflight-live
```

After closure verification, make sure `git status --short` does not include
retained artifacts such as `e2e/test-results*` before opening any PR.

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
Strict mode also requires `workspace-cwd-open`,
`approval-never-sandbox-active`, and `danger-full-access-approval-never` rows
to be `ok`. Override or extend this list with
`OCTOS_TUI_SOAK_REQUIRED_SOLO_CASES` for a specific closure bundle.
MCP/tool config blockers are recorded the same way until the backend advertises
`mcp/config/*`, `mcp/config/test`, and `tool/config/set_enabled`.
Set `OCTOS_TUI_SOAK_EXPECT_TENANT_NEGATIVE=1` during `verify-solo` when the
tenant/cloud dangerous-mode rejection row is part of the closure bundle; the
verifier then requires `tenant-danger-rejection` to pass with server-side
rejection evidence in `soak-summary.json`.
For the full M12-G closure bundle, run `verify-solo-closure`; it enables strict
solo verification, requires tenant-negative evidence, and also verifies the
multiline composer capture. If multiline evidence was retained in a separate
run, set `OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR=<path>`.
For final transport coverage, retain one strict stdio solo run and one strict
WebSocket solo run, then run `verify-solo-transport-closure`:

```sh
OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<solo-run-id> \
OCTOS_TUI_SOAK_MULTILINE_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<multiline-run-id> \
OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<stdio-run-id> \
OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<ws-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-solo-transport-closure
```

`verify-solo-transport-closure` runs the strict solo closure bundle, verifies
both retained transport artifacts, and compares their AppUI method sequence.
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

For the reconnect/hydration leg, keep the same run id and run one lane per
transport.

WebSocket:

```sh
OCTOS_TUI_SOAK_TRANSPORT=ws \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-reconnect
```

Stdio:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-reconnect
```

Then verify the retained artifact directory:

```sh
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-reconnect
```

`drive-task-subagent-reconnect` restarts the WebSocket backend or terminates the
scoped stdio child process so the TUI exercises its relaunch path. It waits for
the TUI to settle again and saves
`tui-capture-task-subagent-tree-reconnect.txt`. The verifier checks the restart
capture, the post-reconnect composer/status line, and AppUI hydration method
evidence such as `session/open`, `agent/list`, `session/goal/get`,
`loop/list`, or `task/list`.

For the old-server fallback leg, retain a capture and transcript from a backend
that does not advertise supervised task inspection capabilities, then run:

```sh
OCTOS_TUI_SOAK_RUN_ID=<old-server-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-task-subagent-old-server-fallback

OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-old-server-fallback
```

The verifier checks that the fallback capture still has a usable composer/status
line, does not expose task/subagent inspection controls, and does not probe
`review/start`, `task/list`, or `task/artifact/*` methods.

For the full #40 closure bundle, retain the main task/subagent run, the
restart/reconnect run, the old-server fallback run, and one WebSocket plus one
stdio transcript pair. Then run:

```sh
OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<task-run-id> \
OCTOS_TUI_SOAK_TASK_RECONNECT_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<reconnect-run-id> \
OCTOS_TUI_SOAK_TASK_OLD_SERVER_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<old-server-run-id> \
OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<ws-run-id> \
OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<stdio-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-task-subagent-closure
```

`verify-task-subagent-closure` runs the task/subagent tree, reconnect,
old-server fallback, and transport-parity verifiers as one fail-closed bundle.

## M15 Autonomy Live Artifacts

For M15 production autonomy evidence, start the normal tmux harness against a
production-capable backend and drive the autonomy path:

```sh
OCTOS_TUI_SOAK_RUN_ID=autonomy-live-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-autonomy-live
```

`drive-autonomy-live` sends `/goal`, fixed/self-paced/maintenance `/loop`
commands, `/loop list`, a supervised review prompt, and `/agents list`. Set
`OCTOS_TUI_SOAK_AUTONOMY_LOOP_ID=<loop-id>` after a loop is visible to also
drive `/loop fire-now`, `/loop pause`, and `/loop resume`. Set
`OCTOS_TUI_SOAK_AUTONOMY_AGENT_ID=<agent-id>` after an agent is visible to also
drive `/agents status`, `/agents output`, and `/agents artifacts`.

If the backend writes M15 JSON evidence outside the TUI artifact directory, set
`OCTOS_TUI_M15_UX_OUTPUT_DIR=<path>` before `drive-autonomy-live`; the driver
copies it under `m15-evidence/` for the verifier.

The driver keeps per-step captures and also writes
`tui-capture-autonomy-live.txt` as the aggregate capture used by
`verify-autonomy-live`.

Then point the verifier at the retained live artifact directory:

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

For the reconnect/hydration leg, retain `server-pane-after-restart.txt`,
`tui-capture-autonomy-reconnect.txt`, the AppUI transcript, and agent/goal/loop
ledgers, then run one lane per transport.

WebSocket:

```sh
OCTOS_TUI_SOAK_TRANSPORT=ws \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-autonomy-reconnect
```

Stdio:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-autonomy-reconnect
```

Then verify the retained artifact directory:

```sh
OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-autonomy-reconnect
```

`drive-autonomy-reconnect` restarts the WebSocket backend or terminates the
scoped stdio child process so the TUI exercises its relaunch path. It then
re-requests `/agents list`, `/goal`, and `/loop list`, and writes
`tui-capture-autonomy-reconnect.txt` as an aggregate of the hydration captures.
`verify-autonomy-reconnect` checks that the restarted run visibly rehydrates
agent, goal, and loop state; that the transcript issues `session/open`,
`agent/list`, `session/goal/get`, and `loop/list`; and that agent/goal/loop
notifications are not client-owned timer traffic.

For the full #44 closure bundle, retain the main production autonomy run, the
restart/reconnect run, and one WebSocket plus one stdio transcript pair. Then
run:

```sh
OCTOS_TUI_SOAK_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<autonomy-run-id> \
OCTOS_TUI_SOAK_AUTONOMY_RECONNECT_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<reconnect-run-id> \
OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<ws-run-id> \
OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<stdio-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-autonomy-closure
```

`verify-autonomy-closure` runs the autonomy live, reconnect, and
transport-parity verifiers as one fail-closed bundle.

## Transport Parity

After retaining one WebSocket artifact directory and one stdio artifact
directory for the same live scenario, compare their AppUI method traffic:

```sh
OCTOS_TUI_SOAK_WS_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<ws-run-id> \
OCTOS_TUI_SOAK_STDIO_ARTIFACT_DIR=e2e/test-results-tui-onboarding/<stdio-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-transport-parity
```

The verifier first checks each directory's `summary.env` transport metadata:
the WebSocket directory must record `transport=ws`, and the stdio directory
must record `transport=stdio`. It then reads `appui-transcript.jsonl` from each
directory, including the `m15-evidence/` subdirectory when present, normalizes
`client_to_server`/`tx` and `server_to_client`/`rx`, then compares the
direction + method sequence. Set `OCTOS_TUI_SOAK_TRANSPORT_PARITY_MODE=set`
only when the issue acceptance requires method-set parity rather than ordering
parity.

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

## Diff And Artifact Readiness

For diff/artifact readiness evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-diff-artifact

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-diff-artifact
```

`verify-diff-artifact` checks `tui-capture-diff-artifact.txt`,
`artifact-index.json`, and `appui-transcript.jsonl`. The verifier requires
visible diff preview text, visible artifact text, a usable composer, an
artifact-index `artifacts` array, and diff/artifact readiness evidence in the
transcript.

## Denied Tool Policy

For denied-tool / blocked-policy evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-tool-denial

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-tool-denial
```

`verify-tool-denial` checks `tui-capture-tool-denial.txt` and
`appui-transcript.jsonl`. The verifier requires visible denied-policy text, a
usable composer after the blocked tool path, `tool/denied` transcript evidence,
and no `approval/requested` frame for that retained bundle.

## Successful Tool Call

For normal successful tool-call evidence:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh drive-tool-success

OCTOS_TUI_SOAK_RUN_ID=<run-id> \
scripts/run-onboarding-tmux-soak.sh verify-tool-success
```

`verify-tool-success` checks `tui-capture-tool-success.txt` and
`appui-transcript.jsonl`. The verifier requires visible successful tool/output
text, a usable composer after the turn, turn/tool-output transcript evidence,
and no denied-tool event in that retained bundle.

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
- `tui-capture-diff-artifact.txt` and `artifact-index.json` when running
  `drive-diff-artifact`
- `tui-capture-tool-denial.txt` when running `drive-tool-denial`
- `tui-capture-tool-success.txt` when running `drive-tool-success`
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

`drive-onboard` drives the live tmux TUI through the provider-onboarding smoke:
it refreshes login/provider state, selects and saves the configured provider,
finishes onboarding when requested, opens `/provider` and `/model`, then
captures the pane. The retained artifacts must show masked secrets only.

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
scripts/run-onboarding-tmux-soak.sh verify-onboard
```

For MiniMax WiseModel:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=minimax \
OCTOS_TUI_SOAK_EXPECT_MODEL=MiniMax-M2.5-highspeed \
OCTOS_TUI_SOAK_EXPECT_ROUTE=wisemodel \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=https://open.ospreyai.cn/v1 \
scripts/run-onboarding-tmux-soak.sh verify-onboard
```

For a custom OpenAI-compatible route:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=<custom-family-id> \
OCTOS_TUI_SOAK_EXPECT_MODEL=<custom-model-id> \
OCTOS_TUI_SOAK_EXPECT_ROUTE=<custom-route-id> \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=<custom-base-url> \
scripts/run-onboarding-tmux-soak.sh verify-onboard
```

`verify` remains a backwards-compatible alias for `verify-onboard`.

To check secret redaction, pass the test key as an environment variable only:

```sh
OCTOS_TUI_SOAK_API_KEY=<secret-value> scripts/run-onboarding-tmux-soak.sh verify-onboard
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
artifact directory, source checkout commits, status, and timestamp. Treat it as
the machine-readable summary for the retained pane captures and JSONL evidence;
keep `soak-summary.json` for provider/profile-specific details.
`summary.env` records the run id, transport, runtime paths, and the source
checkout commits for both `octos` and `octos-tui`.
Strict closure verifiers fail unless each retained closure artifact directory
has valid `octos_repo_commit` and `octos_tui_repo_commit` fields.
They also reject mixed-revision closure bundles; old-server fallback artifacts
may use a different `octos` backend commit, but must still use the same
`octos-tui` commit as the closure run under test.
Strict closure verifiers also require the primary closure artifact directory to
retain a passed, provider-backed `live-preflight.json`.

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
