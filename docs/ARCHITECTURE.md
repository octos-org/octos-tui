# octos-tui Architecture

## Scope

`octos-tui` is a standalone terminal client for the Octos AppUI/UI Protocol.
In protocol mode it does not run the Octos agent, execute tools, approve
commands, maintain the durable ledger, or own provider/model configuration.
Those responsibilities belong to the `octos serve` process.

The TUI owns:

- terminal rendering and keyboard handling
- local view state, focus, scroll, expansion, composer draft, and staged input
- optimistic display of the user's submitted prompt
- local slash commands such as `/ps`, `/stop`, and `/help`
- translation between user interactions and stable `AppUiCommand` values

The server owns:

- session creation and session cwd validation
- agent/runtime execution
- shell/tool execution and sandbox policy
- approval requests, approval decisions, and approval scopes
- task supervisor state and background task registry
- durable UI event ledger, replay, and `protocol/replay_lossy` reporting
- diff preview and task output data sources

## Runtime Topology

```text
User keyboard
  |
  v
octos-tui
  src/event_loop.rs       terminal draw/read/send loop
  src/store.rs            AppUI reducer and follow-up command builder
  src/app.rs              ratatui panes, markdown, tasks, diffs, approvals
  src/transport.rs        mock or protocol backend
  |
  | AppUiCommand -> JSON-RPC over WebSocket
  v
ws://HOST:PORT/api/ui-protocol/ws
  |
  v
octos serve
  crates/octos-cli/src/api/ui_protocol.rs
  crates/octos-core/src/app_ui.rs
  crates/octos-core/src/ui_protocol.rs
  |
  v
Octos runtime
  sessions, agent turns, tools, approvals, task supervisor, ledger
  |
  | UiNotification / UiProgressEvent / RPC results
  v
octos-tui Store -> AppState -> ratatui render
```

`octos-tui` and `octos-app` should both depend on the AppUI contract, not on
M9 or future M10 implementation details. As long as the AppUI API remains
compatible, client behavior should survive server-internal milestone changes.

## Server Endpoints

The AppUI endpoint is:

```text
/api/ui-protocol/ws
```

That route is implemented in the Octos repo under
`crates/octos-cli/src/api/ui_protocol.rs`. It accepts JSON-RPC messages over a
WebSocket and translates protocol commands into runtime actions.

WebSocket is the current deployed transport, not the AppUI protocol itself. The
transport refactor milestone is documented in the parent Octos repo at
`api/APPUI_TRANSPORT_PROTOCOL_REFACTOR_MILESTONE.md`. The intended long-term
shape is that the same `AppUiCommand` and `AppUiEvent` contract can run over
WebSocket, stdio, Unix sockets, local TCP streams, named pipes, or in-process
test channels.

The older endpoint:

```text
/api/ws
```

is the legacy web chat/gateway WebSocket. It is not the AppUI contract used by
`octos-tui`.

## Shared API Types

The client consumes shared Rust types from the sibling Octos repo:

```text
../octos/crates/octos-core/src/app_ui.rs
../octos/crates/octos-core/src/ui_protocol.rs
```

`app_ui.rs` is the app-facing API layer. `ui_protocol.rs` is the JSON-RPC wire
protocol layer. The TUI should prefer the `AppUi*` types at its boundary and
keep wire-specific details inside `src/transport.rs`.

## Protocol Commands

The stable command surface currently includes:

| AppUI command | Wire method | Purpose |
|---|---|---|
| `OpenSession` | `session/open` | Open or resume a session, request a cwd, and replay after a cursor. |
| `SubmitPrompt` | `turn/start` | Start a user turn with one or more input items. |
| `InterruptTurn` | `turn/interrupt` | Interrupt the active turn. |
| `RespondApproval` | `approval/respond` | Approve or deny a pending approval with an optional scope. |
| `ListApprovalScopes` | `approval/scopes/list` | Discover server-supported approval scopes. |
| `GetDiffPreview` | `diff/preview/get` | Fetch the server-authoritative diff preview for a preview id. |
| `ReadTaskOutput` | `task/output/read` | Fetch a task output snapshot. |

The transport maps each `AppUiCommand` into a JSON-RPC request and tracks the
result kind expected back from the server.

## Protocol Notifications

The TUI reducer must defensively handle known notifications and warnings:

- `session/opened`
- `turn/started`, `turn/completed`, `turn/error`
- `message/delta`
- `tool/started`, `tool/progress`, `tool/completed`
- `approval/requested`, `approval/auto_resolved`, `approval/decided`,
  `approval/cancelled`
- `task/updated`, `task/output/delta`
- `progress/updated`
- `warning`
- `protocol/replay_lossy`

Unknown or future notifications should not crash the UI. They should degrade to
a visible warning or status item when possible.

## Client Layers

| File | Responsibility |
|---|---|
| `src/cli.rs` | Parses `--config` JSON launch defaults plus CLI overrides such as `--mode`, `--endpoint`, `--stdio-command`, `--session`, `--profile-id`, `--cwd`, `--auth-token`, `--readonly`, `--no-readonly`, and `--theme`. It must not own provider/model settings; those stay in Octos server config. |
| `src/event_loop.rs` | Owns terminal raw mode, alternate screen, draw loop, keyboard dispatch, backend polling, and command send errors. |
| `src/store.rs` | Reduces snapshots, RPC results, notifications, local commands, approvals, diff previews, task output, and queued prompts into `AppState`. |
| `src/transport.rs` | Defines `AppUiBackend`, mock backend, protocol backend, WebSocket auth, JSON-RPC framing, reconnect status, in-memory cursors, and command/result routing. |
| `src/model.rs` | Defines TUI view models and maps AppUI snapshots/tasks/messages into renderable state. |
| `src/app.rs` | Renders the chat history, sticky work/plan area, activity cards, task list, diff preview, approvals, composer, and status bar with ratatui. |
| `src/theme.rs` | Defines terminal-aware palettes and theme-specific colors. |

## Menu Framework

Codex-style menus should be implemented as a reusable TUI framework, not as
one-off slash handlers. The milestone plan lives in
`docs/M9_34_MENU_FRAMEWORK_MILESTONE.md`.

The intended boundary is:

- generic command registry, slash popup, selection views, and menu stack live in
  `octos-tui`
- local menus such as `/theme`, `/statusline`, `/title`, and `/keymap` remain
  local TUI concerns
- server-backed menus such as `/model`, `/status`, `/permissions`, and `/mcp`
  must use AppUI capabilities and typed `AppUiCommand` values
- menu content providers must plug into the framework without changing generic
  renderer or composer logic

## Protocol Startup Flow

1. `octos-tui` parses CLI launch preferences.
2. `build_backend()` creates either `MockAppUiBackend` or
   `ProtocolAppUiBackend`.
3. In protocol mode, `bootstrap()` connects to `/api/ui-protocol/ws`.
4. If a session id is present, the TUI sends `session/open` with
   `session_id`, `profile_id`, requested `cwd`, and any known replay cursor.
5. The server validates the session and cwd against its policy, replays durable
   ledger events after the cursor, and sends the current session view.
6. `Store::from_snapshot()` hydrates local state and the renderer draws the
   first frame.

## Turn Flow

1. User presses Enter in the composer.
2. `Store::compose_command()` creates a new `turn_id`, appends the submitted
   user message locally, and returns `AppUiCommand::SubmitPrompt`.
3. The transport sends `turn/start`.
4. The server emits turn, message, tool, approval, task, progress, and warning
   events.
5. `Store::apply_client_event()` applies each event and may return a follow-up
   command, for example `diff/preview/get` after a diff approval request.
6. `src/app.rs` renders active work separately from completed activity so the
   user can see what is running and what already finished.

## Approval Flow

Approval decisions are server-owned. The TUI renders approval details and sends
one of the server-supported choices:

- approve this request
- approve an allowed scope, such as session or tool, when advertised
- deny this request

The TUI must stop the visible waiting state when the server emits
`approval/decided`, `approval/auto_resolved`, `approval/cancelled`, or a turn
interrupt/error that invalidates the approval.

## Task and Output Flow

Task lifecycle is server-owned. The TUI renders task state from
`task/updated` and task snapshots. On-demand task details use
`task/output/read`; live output deltas use `task/output/delta` when the server
emits them.

The UX target is Codex-style task visibility:

- active work is sticky near the composer
- completed work appears in the transcript with past-tense labels such as
  `Ran`, `Explored`, and `Waited`
- long command output, diffs, and file/document previews collapse by default
- expanded cards expose as much useful command/file detail as the terminal can
  fit

## Durability and Replay

The durable source of truth is the server ledger, not TUI memory. The TUI keeps
only local presentation state and in-memory replay cursors.

Server durability requirements:

- append durable lifecycle events before sending them
- replay missed durable events after a client cursor
- never apply stale disk replay over a newer live session snapshot
- surface lossy delivery with `protocol/replay_lossy`
- keep terminal task states deliverable under backpressure

Client replay requirements:

- request replay using the latest known cursor when opening a session
- tolerate duplicate durable notifications
- treat `protocol/replay_lossy` as a signal to refresh/reopen rather than as a
  normal chat message
- never infer runtime truth from local optimistic UI state after reconnect

## Mock Mode

`--mode mock` is a deterministic local fixture backend for rendering,
keyboard, and harness tests. It does not represent the live Octos runtime and
must not be used to validate server policy, sandboxing, provider setup, ledger
durability, or tool execution behavior.

## Readonly Mode

`--readonly` opens a protocol session as a viewer. Mutating commands should be
blocked locally, and unavailable protocol connections may fall back to a
readonly offline snapshot for inspection.

## Codex-Style Reference Architecture

This section is an observable product-level reference model for comparison. It
is not a statement about private OpenAI implementation details.

The Codex CLI surface exposes an integrated local coding-agent process with
interactive TUI, non-interactive `exec`, code `review`, MCP support,
configurable sandbox policy, configurable approval policy, optional web search,
local working-directory selection, and optional remote/app-server modes.

A useful architecture model is:

```text
User terminal
  |
  v
Codex CLI/TUI process
  |-- renderer, composer, status line, transcript, tool cards
  |-- local session/config/history
  |-- approval policy and sandbox policy
  |-- local tool runner for shell/files/MCP
  |-- optional remote/app-server connection
  |
  | model requests and streamed responses
  v
model/provider service
  |
  | tool-call decisions, text deltas, plan/status updates
  v
Codex CLI executes approved local tools and updates the transcript
```

The important product difference is where the runtime lives:

| Area | Codex-style local CLI | Octos AppUI architecture |
|---|---|---|
| UI | Local CLI/TUI process. | `octos-tui` or `octos-app`. |
| Runtime owner | Mostly the local CLI process, with model service calls. | `octos serve`. |
| Tool execution | Local CLI sandbox/tool runner. | Server-side Octos runtime/tool system. |
| Approval policy | Local CLI approval flow. | Server-owned approval requests plus client rendering/response. |
| Durable replay | Local CLI/session behavior. | Server ledger and replay cursors. |
| Client/server contract | CLI implementation boundary. | Stable AppUI/UI Protocol boundary. |

For Octos, the product goal is to keep Codex-quality coding UX while preserving
a cleaner split: the terminal app is replaceable, and all clients speak the
same AppUI API to the Octos server.

## Architectural Invariants

- `octos-tui` must not call Octos runtime internals directly.
- `octos-tui` must not rely on M9-specific server internals outside AppUI.
- `octos-tui` must treat the server as authoritative for tasks, approvals,
  diffs, tool results, cwd policy, sandbox policy, and replay.
- The server must not require TUI-specific behavior for protocol correctness.
- New client-visible runtime features should land in `octos-core` AppUI/UI
  Protocol types before TUI-specific rendering.
- Prompt shaping for better coding UX belongs in the server profile or harness
  prompt contract, documented separately in
  `docs/CODING_UX_PROMPT_CONTRACT.md`.
