# DeepSeek-TUI Terminal Tactics For Octos-TUI

Issues: #13, #14, #15, #16, #17, #18, #19

This document captures what Octos-TUI should learn from DeepSeek-TUI while keeping Octos runtime ownership in `octos` and the AppUI protocol. The goal is Rust terminal implementation quality, not moving model routing, memory, sandboxing, tool registration, or approval policy into the TUI.

## Ownership Boundary

Octos-TUI should own:

- terminal rendering and input ergonomics
- capability-aware command/menu presentation
- approval, tool, diff, output, and statusline layout
- deterministic terminal capture fixtures

Octos/AppUI should own:

- model portfolios and QoE policy
- per-profile sandbox and permission policy
- tool registry and auto-deferral behavior
- per-session memory and durable workflow state
- AppUI wire compatibility across WebSocket and stdio

## Tactics To Apply

| Area | DeepSeek-TUI tactic | Octos-TUI application | Issue |
| --- | --- | --- | --- |
| Tool cards | Keep tool execution visually grouped with command, status, preview, and result. | Render AppUI tool start/progress/complete events as structured cards with stable activity labels and folded long output. | #14 |
| Approvals | Make blocked actions visually distinct and keep the decision target close to the relevant command/diff. | Render typed approval cards with risk, scope, decision state, and diff preview linkage. | #15 |
| Diffs and output | Fold noisy content by default while preserving fast expand/read paths. | Keep long command output and long diffs collapsed, with deterministic thresholds covered by fixtures. | #16 |
| Statusline | Make runtime state glanceable rather than buried in transcript text. | Use AppUI capabilities to show profile, model/workspace/runtime availability, readonly state, and transport state. | #17 |
| Terminal input | Treat paste, resize, mouse, focus, and cancellation as first-class terminal state. | Harden crossterm handling for paste/focus/resize/mouse without letting layout shift or lose pending input. | #18 |
| Regression loop | Capture terminal behavior at the cell/PTY layer, not only data-model unit tests. | Add PTY capture artifacts and marker checks for approval, diff, output, status, and parity scenarios. | #19 |

## Implementation Rules

- Prefer AppUI capability checks over local feature guesses.
- Keep command dispatch transport-neutral; WebSocket and stdio must normalize to the same semantic transcript.
- Bound all client-side queues and pending request maps so stalled transports cannot grow memory unbounded.
- Fold long outputs and diffs by deterministic thresholds; never depend on a live provider to validate folding.
- Keep the fixture provider-free by default. Live DeepSeek V4 Pro soaks are separate evidence and must not be required for CI.

## Verification Loop

1. Run `cargo test` for compile and reducer coverage.
2. Run `scripts/validate-appui-ux-fixture.sh` for semantic WebSocket-vs-stdio parity.
3. Run `scripts/capture-appui-ux-pty.sh` for PTY capture artifacts and marker summaries.
4. For live soaks, use the parent `octos` harness to drive a real AppUI endpoint and retain raw terminal captures, server logs, state summaries, and model metadata.

## Non-Goals

- Do not import DeepSeek-TUI provider ownership into Octos-TUI.
- Do not bypass AppUI for tools, memory, sandbox, profile state, or approvals.
- Do not make live-provider output required for normal regression tests.
