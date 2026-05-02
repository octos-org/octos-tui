# AppUI E2E UX Fixture

Issue: https://github.com/octos-org/octos-tui/issues/7

This repo owns a deterministic short fixture for AppUI coding-session UX parity:

- Fixture: `fixtures/appui_ux_parity/coding_session_short.json`
- Validator: `tests/appui_ux_fixture.rs`
- Runner: `scripts/validate-appui-ux-fixture.sh`

The fixture stores one semantic scenario with WebSocket and future stdio wire
frames side by side. The validator normalizes away transport-only fields such as
JSON-RPC ids, WebSocket connection ids, and stdio line numbers, then compares the
resulting AppUI event sequence.

## CI Lane

Run:

```bash
scripts/validate-appui-ux-fixture.sh
```

The short lane is offline and must not call a model provider. It covers:

- session open and cwd/profile metadata
- user turn submission before assistant deltas
- sticky plan/status placement metadata
- markdown table presence in deterministic assistant text
- task creation, running output, output read, completion, and cancellation
- long tool output and long diff preview collapse expectations
- approval prompt blocking until a decision
- activity labels for test and edit tool activity
- replay-lossy plus reconnect/open after cursor

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
capture the live TUI panes for one hour and retain raw/cleaned captures, server
logs, worktree diff, git status, validation logs, and state-matrix summaries.

## Known Gaps

No production hook is added here. Visual screenshot/tmux assertions for exact
sticky pane placement, rendered markdown cell alignment, and interactive
diff expand/collapse still need the parent `octos` harness or an exposed
render-snapshot hook from `octos-tui`.
