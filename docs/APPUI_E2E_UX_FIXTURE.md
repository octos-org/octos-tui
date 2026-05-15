# AppUI E2E UX Fixture

Issues:

- https://github.com/octos-org/octos-tui/issues/7
- https://github.com/octos-org/octos-tui/issues/21
- https://github.com/octos-org/octos-tui/issues/22
- https://github.com/octos-org/octos-tui/issues/24

This repo owns a deterministic short fixture for AppUI coding-session UX parity:

- Fixture: `fixtures/appui_ux_parity/coding_session_short.json`
- Validator: `tests/appui_ux_fixture.rs`
- Runner: `scripts/validate-appui-ux-fixture.sh`
- PTY capture runner: `scripts/capture-appui-ux-pty.sh`

The fixture stores one semantic scenario with WebSocket and future stdio wire
frames side by side. The validator normalizes away transport-only fields such as
JSON-RPC ids, WebSocket connection ids, and stdio line numbers, then compares the
resulting AppUI event sequence.

## Real Tmux Capture Lane

Run:

```bash
scripts/validate-tmux-ux-capture.sh <captures/tui-capture-clean.txt> <logs/server.log>
```

This lane validates the rendered terminal screen from a real tmux session. It is
designed to catch the UX bugs found during manual 249 soaking:

- the split Work/Progress pane must not render in the normal chat layout
- queued composer messages must also be listed in the chat history
- clarification questions and historical turn rounds must not render as a
  separate routing/plan panel
- the bottom status line must not animate a second progress spinner
- input text must not overlap any removed Work/Progress pane border
- markdown markers such as `####`, `[x]`, and emphasis markers must not leak in
  assistant prose
- a server log with dropped `turn/completed` lifecycle delivery fails the soak
  when the screen still shows the turn as working

The validator has deterministic fixtures:

- passing: `fixtures/tui_ux_captures/reported_bugs_good.txt`
- intentionally failing:
  `fixtures/tui_ux_captures/reported_bugs_bad_stuck.txt`
- matching failing log:
  `fixtures/tui_ux_captures/reported_bugs_bad_server.log`

To validate a live 249 capture, save the pane and pass the matching server log:

```bash
tmux capture-pane -t <session> -p -S -200 > /tmp/tui-capture-clean.txt
scripts/validate-tmux-ux-capture.sh /tmp/tui-capture-clean.txt /path/to/server.log
```

## CI Lane

Run:

```bash
scripts/validate-appui-ux-fixture.sh
```

The short lane is offline and must not call a model provider. It covers:

- session open and cwd/profile metadata
- runtime policy stamp metadata on `turn/started`
- user turn submission before assistant deltas
- sticky plan/status placement metadata
- markdown table presence in deterministic assistant text
- task creation, running output, output read, completion, cancellation, and
  reconnect after replay loss
- tool timeline ordering: started, progress, completed
- long tool output and long diff preview collapse expectations
- typed approval prompt blocking until a decision
- typed `tool_denied` policy result
- validator failed and validator passed timeline markers
- diff ready and artifact ready evidence markers
- activity labels for test and edit tool activity
- narrow-layout readiness marker for the later render snapshot lane
- real tmux capture checks for the reported sticky pane, queued prompt,
  markdown leakage, duplicate spinner, and dropped-completion bugs

## PTY Capture Lane

Run:

```bash
scripts/capture-appui-ux-pty.sh
```

The capture lane runs the same provider-free fixture under a terminal PTY when
the host provides the `script` command. It writes artifacts to
`e2e/test-results-tui-ux-capture/<run-id>/` by default:

- `appui-ux-fixture.pty.txt`: raw terminal capture of the fixture test run
- `coding_session_short.json`: copied fixture used for the run
- `summary.env`: machine-readable markers:
  `runtime_policy_stamp_seen`, `tool_timeline_seen`,
  `typed_approval_seen`, `typed_denial_seen`, `validator_failed_seen`,
  `validator_passed_seen`, `diff_ready_seen`, `artifact_ready_seen`,
  `interrupt_seen`, `reconnect_seen`, `long_output_folded`,
  `narrow_layout_ok`, plus legacy approval, diff, output, status, label, and
  WebSocket/stdio parity markers

For fixture self-checking, run:

```bash
OCTOS_TUI_CAPTURE_SELF_TEST=1 scripts/capture-appui-ux-pty.sh
```

The self-test verifies the marker validator notices a deliberately absent marker
without requiring a live model provider.

## M12 Solo Permission Soak Fixture

M12-D/G evidence is captured by the onboarding tmux runner's solo lane:

```bash
scripts/run-onboarding-tmux-soak.sh solo-self-test
OCTOS_TUI_SOAK_TRANSPORT=stdio scripts/run-onboarding-tmux-soak.sh drive-solo
OCTOS_TUI_SOAK_TRANSPORT=stdio OCTOS_TUI_SOAK_RUN_ID=<run-id> scripts/run-onboarding-tmux-soak.sh verify-solo
```

This lane is provider-free. It records `profile/local/create` local onboarding,
asserts the retained transcript has no OTP method traffic, drives the MCP/tool
config fixture, and captures:

- `appui-transcript.jsonl`
- `runtime-policy-stamp.json`
- `tool-registry-snapshot.json`
- `mcp-config-before.redacted.json`
- `mcp-config-after.redacted.json`
- `mcp-status-list.json`
- `mcp-connection-test-result.json`
- `approval-events.jsonl`
- `filesystem-probe.json`
- `tui-capture.txt`
- `server.log`

Before M12-A/C backend support lands, `soak-summary.json` may report
`"status": "blocked"` with explicit capability blockers. Use
`OCTOS_TUI_SOAK_SOLO_STRICT=1` once the backend advertises and implements
`profile/local/create`, `permission/profile/*`, `mcp/config/*`,
`mcp/config/test`, and `tool/config/set_enabled`.

## One-Hour Live Soak

The live soak is manual and should be driven by the parent `octos` tmux harness,
because that harness owns server startup and terminal captures.

Minimum environment:

```bash
OCTOS_TUI_UX_LIVE_SOAK=1 \
OCTOS_TUI_PROTOCOL_ENDPOINT=ws://127.0.0.1:7777/ui \
scripts/validate-appui-ux-fixture.sh
```

The script prepares the artifact directory, but the parent harness still needs to
capture the live TUI panes for one hour.

Required live matrix:

- WebSocket short reconnect lane with runtime policy stamp, tool timeline,
  typed approval, diff ready, artifact ready, and typed denial markers.
- stdio short reconnect lane with the same normalized markers.
- long lane that proves reconnect after replay loss and an in-flight interrupt.
- safety lane that emits one typed approval and one typed `tool_denied` policy
  result.
- validator lane with at least one failed validator followed by a passing rerun.
- narrow terminal lane, 80x24 or smaller, with no overlapping cockpit,
  timeline, approval, denial, diff, artifact, or composer text.

Required live artifacts:

- `transcripts/appui-transcript.jsonl`
- `logs/server.log`
- `policy/runtime-policy-stamp.json`
- `timeline/tool-timeline.jsonl`
- `approvals/approval-events.jsonl`
- `approvals/denial-events.jsonl`
- `validators/validator-events.jsonl`
- `diffs/diff-ready.json`
- `artifacts/artifact-ready.json`
- `captures/tui-capture.txt`
- `captures/tui-capture-clean.txt`
- `scripts/validate-tmux-ux-capture.sh captures/tui-capture-clean.txt logs/server.log`
- `validation.log`
- `git-status.txt`
- `worktree-diff.patch`
- `summary.env`

## Remaining Gaps

The local lane now captures terminal output and semantic markers, but it still
does not replace a full live soak. Exact rendered cell alignment, interactive
expand/collapse, narrow layout overlap checks, and model-backed UX comparison
still need the parent `octos` tmux harness or a future render-snapshot hook from
`octos-tui`.
