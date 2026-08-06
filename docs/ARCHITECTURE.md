# octoscode Architecture

## Scope

`octoscode` is a standalone terminal client for the Octos UI Protocol.
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
octoscode
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
octoscode Store -> AppState -> ratatui render
```

`octoscode` and `octos-app` should both depend on the AppUI contract, not on
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

WebSocket is the current deployed transport, not the Octos UI Protocol itself. The
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
`octoscode`.

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

Every variant of `AppUiCommand` (`src/model.rs`) and the wire method
`AppUiCommand::method()` maps it to. The transport turns each into a JSON-RPC
request and records the result kind it expects back.

This list is COMPLETE and kept that way by `tests/docs_drift.rs`, which fails
if a variant is added to the enum without being documented here. Do not prune
it by hand.

### `profile/`

| AppUI command | Wire method |
|---|---|
| `ListModels` | `profile/llm/list` |
| `ProfileLlmCatalog` | `profile/llm/catalog` |
| `ProfileLlmDelete` | `profile/llm/delete` |
| `ProfileLlmFetchModels` | `profile/llm/fetch_models` |
| `ProfileLlmList` | `profile/llm/list` |
| `ProfileLlmSelect` | `profile/llm/select` |
| `ProfileLlmTest` | `profile/llm/test` |
| `ProfileLlmUpsert` | `profile/llm/upsert` |
| `ProfileLocalCreate` | `profile/local/create` |
| `ProfileSkillsInstall` | `profile/skills/install` |
| `ProfileSkillsList` | `profile/skills/list` |
| `ProfileSkillsRegistrySearch` | `profile/skills/registry/search` |
| `ProfileSkillsRemove` | `profile/skills/remove` |
| `ProfileSubProvidersList` | `profile/sub_providers/list` |
| `ProfileSubProvidersRemove` | `profile/sub_providers/remove` |
| `ProfileSubProvidersUpsert` | `profile/sub_providers/upsert` |
| `SelectModel` | `profile/llm/select` |

### `session/`

| AppUI command | Wire method |
|---|---|
| `ClearSessionGoal` | `session/goal/clear` |
| `CompactContext` | `session/compact` |
| `GetSessionGoal` | `session/goal/get` |
| `HydrateSession` | `session/hydrate` |
| `ListSessions` | `session/list` |
| `OpenSession` | `session/open` |
| `ReadSessionStatus` | `session/status/read` |
| `SessionBtw` | `session/btw` |
| `SessionRollback` | `session/rollback` |
| `SetCompactionMode` | `session/compact/mode/set` |
| `SetSessionGoal` | `session/goal/set` |

### `agent/`

| AppUI command | Wire method |
|---|---|
| `CloseAgent` | `agent/close` |
| `InterruptAgent` | `agent/interrupt` |
| `ListAgentArtifacts` | `agent/artifact/list` |
| `ListAgents` | `agent/list` |
| `ReadAgentArtifact` | `agent/artifact/read` |
| `ReadAgentOutput` | `agent/output/read` |
| `ReadAgentStatus` | `agent/status/read` |

### `mcp/`

| AppUI command | Wire method |
|---|---|
| `DeleteMcpConfig` | `mcp/config/delete` |
| `ListMcpConfig` | `mcp/config/list` |
| `ListMcpStatus` | `mcp/status/list` |
| `SetMcpConfigEnabled` | `mcp/config/set_enabled` |
| `TestMcpConfig` | `mcp/config/test` |
| `UpsertMcpConfig` | `mcp/config/upsert` |

### `tool/`

| AppUI command | Wire method |
|---|---|
| `DeleteToolConfig` | `tool/config/delete` |
| `ListToolConfig` | `tool/config/list` |
| `ListToolStatus` | `tool/status/list` |
| `SetToolConfigEnabled` | `tool/config/set_enabled` |
| `TestToolConfig` | `tool/config/test` |
| `UpsertToolConfig` | `tool/config/upsert` |

### `loop/`

| AppUI command | Wire method |
|---|---|
| `CreateLoop` | `loop/create` |
| `DeleteLoop` | `loop/delete` |
| `FireLoopNow` | `loop/fire_now` |
| `ListLoops` | `loop/list` |
| `PauseLoop` | `loop/pause` |
| `ResumeLoop` | `loop/resume` |

### `task/`

| AppUI command | Wire method |
|---|---|
| `CancelTask` | `task/cancel` |
| `ListTasks` | `task/list` |
| `ReadTaskArtifact` | `task/artifact/read` |
| `ReadTaskOutput` | `task/output/read` |
| `RestartTaskFromNode` | `task/restart_from_node` |

### `auth/`

| AppUI command | Wire method |
|---|---|
| `AuthLogout` | `auth/logout` |
| `AuthMe` | `auth/me` |
| `AuthSendCode` | `auth/send_code` |
| `AuthStatus` | `auth/status` |
| `AuthVerify` | `auth/verify` |

### `turn/`

| AppUI command | Wire method |
|---|---|
| `GetTurnState` | `turn/state/get` |
| `InterruptTurn` | `turn/interrupt` |
| `SubmitPrompt` | `turn/start` |
| `TurnSteer` | `turn/steer` |

### `approval/`

| AppUI command | Wire method |
|---|---|
| `ListApprovalScopes` | `approval/scopes/list` |
| `RespondApproval` | `approval/respond` |

### `permission/`

| AppUI command | Wire method |
|---|---|
| `ListPermissionProfiles` | `permission/profile/list` |
| `SetPermissionProfile` | `permission/profile/set` |

### `snapshot/`

| AppUI command | Wire method |
|---|---|
| `SnapshotList` | `snapshot/list` |
| `SnapshotRestore` | `snapshot/restore` |

### `peer/`

| AppUI command | Wire method |
|---|---|
| `PeerGather` | `peer/gather` |
| `PeerPrepare` | `peer/prepare` |

### `user_question/`

| AppUI command | Wire method |
|---|---|
| `RespondUserQuestion` | `user_question/respond` |

### `diff/`

| AppUI command | Wire method |
|---|---|
| `GetDiffPreview` | `diff/preview/get` |

### `launch/`

| AppUI command | Wire method |
|---|---|
| `LaunchResolve` | `launch/resolve` |

### `thread/`

| AppUI command | Wire method |
|---|---|
| `GetThreadGraph` | `thread/graph/get` |

### `review/`

| AppUI command | Wire method |
|---|---|
| `StartReview` | `review/start` |

### `config/`

| AppUI command | Wire method |
|---|---|
| `ListConfigCapabilities` | `config/capabilities/list` |

### `local/`

| AppUI command | Wire method |
|---|---|
| `LocalShellExec` | `local/shell_exec` |

## Protocol Notifications

Every `UiNotification` variant the pinned `octos-core` defines, and the wire
method it arrives as. `Store::apply_notification` currently handles all of
them — the match is exhaustive on purpose, so a variant added to `octos-core`
becomes a compile error here rather than silently unhandled.

An unknown or future notification must never crash the UI. Where it cannot be
rendered it should degrade to a visible warning or status item.

This list is kept complete by `tests/docs_drift.rs`.

### `session/`

| Notification | Wire method |
|---|---|
| `SessionEventBridged` | `session/event` |
| `SessionGoalCleared` | `session/goal/cleared` |
| `SessionGoalUpdated` | `session/goal/updated` |
| `SessionOpened` | `session/open` |
| `SessionOrchestration` | `session/orchestration` |

### `turn/`

| Notification | Wire method |
|---|---|
| `TurnCompleted` | `turn/completed` |
| `TurnError` | `turn/error` |
| `TurnSpawnComplete` | `turn/spawn_complete` |
| `TurnStarted` | `turn/started` |

### `approval/`

| Notification | Wire method |
|---|---|
| `ApprovalAutoResolved` | `approval/auto_resolved` |
| `ApprovalCancelled` | `approval/cancelled` |
| `ApprovalDecided` | `approval/decided` |
| `ApprovalRequested` | `approval/requested` |

### `visual/`

| Notification | Wire method |
|---|---|
| `VisualFailed` | `visual/failed` |
| `VisualGenerating` | `visual/generating` |
| `VisualSucceeded` | `visual/succeeded` |

### `tool/`

| Notification | Wire method |
|---|---|
| `ToolCompleted` | `tool/completed` |
| `ToolProgress` | `tool/progress` |
| `ToolStarted` | `tool/started` |

### `agent/`

| Notification | Wire method |
|---|---|
| `AgentArtifactUpdated` | `agent/artifact/updated` |
| `AgentOutputDelta` | `agent/output/delta` |
| `AgentUpdated` | `agent/updated` |

### `loop/`

| Notification | Wire method |
|---|---|
| `LoopCompleted` | `loop/completed` |
| `LoopFired` | `loop/fired` |
| `LoopUpdated` | `loop/updated` |

### `context/`

| Notification | Wire method |
|---|---|
| `ContextCompactionCompleted` | `context/compaction_completed` |
| `ContextCompactionStarted` | `context/compaction_started` |
| `ContextNormalizationReported` | `context/normalization_reported` |

### `message/`

| Notification | Wire method |
|---|---|
| `MessageDelta` | `message/delta` |
| `ReasoningDelta` | `message/reasoning_delta` |

### `voice/`

| Notification | Wire method |
|---|---|
| `VoiceAudioChunk` | `voice/audio_chunk` |
| `VoiceExit` | `voice/exit` |

### `task/`

| Notification | Wire method |
|---|---|
| `TaskOutputDelta` | `task/output/delta` |
| `TaskUpdated` | `task/updated` |

### `router/`

| Notification | Wire method |
|---|---|
| `RouterFailover` | `router/failover` |
| `RouterStatus` | `router/status` |

### `projection/`

| Notification | Wire method |
|---|---|
| `Envelope` | `projection/envelope` |
| `EnvelopeV2` | `projection/envelope` |
| `PeerClosed` | `peer/closed` |
| `SkillActionJobUpdated` | `skill/action_job_updated` |

### `user_question/`

| Notification | Wire method |
|---|---|
| `UserQuestionRequested` | `user_question/requested` |

### `plan/`

| Notification | Wire method |
|---|---|
| `PlanUpdated` | `plan/updated` |

### `progress/`

| Notification | Wire method |
|---|---|
| `ProgressUpdated` | `progress/updated` |

### `warning/`

| Notification | Wire method |
|---|---|
| `Warning` | `warning` |

### `protocol/`

| Notification | Wire method |
|---|---|
| `ReplayLossy` | `protocol/replay_lossy` |

### `file/`

| Notification | Wire method |
|---|---|
| `FileAttached` | `file/attached` |

### `queue/`

| Notification | Wire method |
|---|---|
| `QueueState` | `queue/state` |

### `peer/`

| Notification | Wire method |
|---|---|
| `PeerStaged` | `peer/staged` |

## Client Layers

One row per file under `src/`. `tests/docs_drift.rs` fails if a source file is
added without a row here.

### Entry and configuration

| File | Lines | Responsibility |
|---|---:|---|
| `src/cli.rs` | 1053 | `--config` JSON launch defaults plus CLI overrides. Must not own provider/model settings; those stay in Octos server config. |
| `src/cmd/config.rs` | 152 | `octoscode config`: read-only inspection of the client's startup config |
| `src/cmd/doctor.rs` | 2460 | `octoscode doctor` — flutter-doctor-style diagnostics (design §B). |
| `src/cmd/github.rs` | 155 | Minimal GitHub Releases client for `update --check` and `doctor`. |
| `src/cmd/install_method.rs` | 746 | Install-method detection for `octoscode update`/`doctor` (design §A.3). |
| `src/cmd/mod.rs` | 273 | `octoscode` subcommands: `update` and `doctor` (design doc). |
| `src/cmd/update.rs` | 606 | `octoscode update` — install-method-aware updater (design §A). |
| `src/lib.rs` | 317 | Crate root — module declarations and the shared public surface. |
| `src/main.rs` | 53 | Binary entry point: subcommand dispatch, then `event_loop::run`. |

### Core loop and state

| File | Lines | Responsibility |
|---|---:|---|
| `src/client_event.rs` | 377 | Decoded RPC results and autonomy results, in the shape the store reduces. |
| `src/event_loop.rs` | 6328 | Terminal raw mode, alternate screen, draw loop, keyboard dispatch, backend polling, send-error handling. |
| `src/model.rs` | 11612 | `AppState`, the TUI view models, `AppUiCommand`, and the mapping from AppUI snapshots/tasks/messages into renderable state. |
| `src/store.rs` | 37021 | The AppUI reducer: snapshots, RPC results, notifications, local commands, approvals, diffs, task output and queued prompts folded into `AppState`. |

### Transport and backend

| File | Lines | Responsibility |
|---|---:|---|
| `src/backend_ensure.rs` | 1140 | Auto-provision the `octos` server backend so a fresh octoscode install |
| `src/profiles.rs` | 544 | Phase 3 startup profile discovery. |
| `src/transport.rs` | 10721 | `AppUiBackend`, the mock and protocol backends, WebSocket/stdio framing, auth, reconnect status and in-memory cursors. |

### Rendering

| File | Lines | Responsibility |
|---|---:|---|
| `src/app.rs` | 5505 | ratatui surfaces — transcript, composer, docks, approvals, diff preview, status bar — and the re-exports for the `app/*` submodules. |
| `src/app/activity_nav.rs` | 558 | `activity_nav` — extracted from `app.rs` (#365 step 2). Items keep their |
| `src/app/markdown_highlight.rs` | 516 | Style-only markdown highlighting for the composer draft. |
| `src/app/render.rs` | 2172 | `render` — extracted from `app.rs` (#365 step 2). Items keep their |
| `src/app/tests.rs` | 12480 | Test module for [`crate::app`] (#365): moved out of `app.rs`, which was |
| `src/app/transcript_build.rs` | 3615 | `transcript_build` — extracted from `app.rs` (#365 step 2). Items keep their |
| `src/highlight.rs` | 172 | Fenced-code-block syntax highlighting for the transcript renderer |
| `src/insert_history.rs` | 1603 | Insert finalized history lines into the terminal's **normal scrollback**, |
| `src/sanitize.rs` | 160 | Terminal control-sequence sanitisation for server-supplied text. |
| `src/terminal_probe.rs` | 243 | Terminal detection and color adaptation for octoscode. |
| `src/theme.rs` | 204 | Terminal-aware palettes and theme-specific colors. |
| `src/tui_terminal.rs` | 1171 | Inline-viewport terminal — ported and trimmed from codex-rs `tui/src/custom_terminal.rs`. |
| `src/viewport.rs` | 576 | Inline-viewport driver: owns the scrollback-flush bookkeeping that turns |

### Menu framework

| File | Lines | Responsibility |
|---|---:|---|
| `src/menu/availability.rs` | 695 | Capability gating — which menus the connected server advertises. |
| `src/menu/mod.rs` | 21 | Menu framework model and generic render surfaces. |
| `src/menu/multi_select_view.rs` | 487 | Multi-select menu surface. |
| `src/menu/providers.rs` | 11355 | Local and capability-backed menu providers for the M9.34 framework. |
| `src/menu/registry.rs` | 1583 | The canonical slash-command registry: names, aliases and capability gating. |
| `src/menu/render.rs` | 427 | Generic menu renderer shared by every provider. |
| `src/menu/selection_view.rs` | 551 | Single-select menu surface. |
| `src/menu/types.rs` | 890 | `MenuSpec` / `MenuItem` / `MenuAction` — the framework core types. |
| `src/menu/wizard.rs` | 417 | First-run setup wizard step model. |

### Composer and input

| File | Lines | Responsibility |
|---|---:|---|
| `src/autonomy.rs` | 1097 | M15-E autonomy command parsing for `/agents`, `/goal`, and `/loop`. |
| `src/clipboard.rs` | 378 | Clipboard copy support for the TUI. |
| `src/file_picker.rs` | 254 | `@` composer file picker (#363, v1: path insert only). |
| `src/history.rs` | 743 | Composer command-history navigation (codex / claude-code style). |
| `src/keymap.rs` | 1 | The status-bar key-hint string. |

## Menu Framework

Codex-style menus should be implemented as a reusable TUI framework, not as
one-off slash handlers. The milestone plan lives in
`docs/M9_34_MENU_FRAMEWORK_MILESTONE.md`.

The intended boundary is:

- generic command registry, slash popup, selection views, and menu stack live in
  `octoscode`
- local menus such as `/theme`, `/statusline`, `/title`, and `/keymap` remain
  local TUI concerns
- server-backed menus such as `/model`, `/status`, `/permissions`, and `/mcp`
  must use AppUI capabilities and typed `AppUiCommand` values
- menu content providers must plug into the framework without changing generic
  renderer or composer logic

## Protocol Startup Flow

1. `octoscode` parses CLI launch preferences.
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
| UI | Local CLI/TUI process. | `octoscode` or `octos-app`. |
| Runtime owner | Mostly the local CLI process, with model service calls. | `octos serve`. |
| Tool execution | Local CLI sandbox/tool runner. | Server-side Octos runtime/tool system. |
| Approval policy | Local CLI approval flow. | Server-owned approval requests plus client rendering/response. |
| Durable replay | Local CLI/session behavior. | Server ledger and replay cursors. |
| Client/server contract | CLI implementation boundary. | Stable Octos UI Protocol boundary. |

For Octos, the product goal is to keep Codex-quality coding UX while preserving
a cleaner split: the terminal app is replaceable, and all clients speak the
same AppUI API to the Octos server.

## Architectural Invariants

- `octoscode` must not call Octos runtime internals directly.
- `octoscode` must not rely on M9-specific server internals outside AppUI.
- `octoscode` must treat the server as authoritative for tasks, approvals,
  diffs, tool results, cwd policy, sandbox policy, and replay.
- The server must not require TUI-specific behavior for protocol correctness.
- New client-visible runtime features should land in `octos-core` AppUI/UI
  Protocol types before TUI-specific rendering.
- Prompt shaping for better coding UX belongs in the server profile or harness
  prompt contract, documented separately in
  `docs/CODING_UX_PROMPT_CONTRACT.md`.
