# /agents spawn — Design Spec

**Date:** 2026-07-04
**Branch:** fix_agent_list
**Status:** Approved

---

## Overview

Add `/agents spawn <N> <prompt>` as a new autonomy slash command. It sends a turn to the active session's LLM requesting that it spawn `N` parallel sub-agents to accomplish `prompt`, then immediately enqueues an `agent/list` refresh so newly spawned agents appear in the activity feed.

The backend owns agent creation (no `agent/create` RPC exists in the Octos UI Protocol). The command works by crafting a specially formatted chat turn; the LLM schedules the actual sub-agent calls via its tool use.

---

## Architecture & Data Flow

### Parser (`src/autonomy.rs`)

New `AgentsCommand` variant:

```rust
Spawn { count: u32, prompt: String },
```

Parse rules for `/agents spawn <N> <prompt>`:
- `N` must be a decimal integer ≥ 1; anything else → `AutonomyParseError::InvalidSpawnCount(String)`
- `prompt` is all remaining text after `N`; empty → `AutonomyParseError::EmptySpawnPrompt`
- No abbreviation alias; `spawn` only

New `AutonomyParseError` variants:
```rust
InvalidSpawnCount(String),
EmptySpawnPrompt,
```

### Dispatch (`src/store.rs`, `dispatch_agents_command`)

```
AgentsCommand::Spawn { count, prompt } =>
  1. require_appui_method(APPUI_METHOD_AGENT_LIST)   — gate on agent control feature
  2. set status: "Spawning {count} agent(s): {prompt}"
  3. enqueue_autonomy_hydration(ListAgents { session_id, parent_agent_id: None })
  4. return Accepted(Some(SubmitPrompt(TurnStartParams {
       text: "Spawn {count} agents to accomplish in parallel: {prompt}",
       …
     })))
```

### Event Flow

```
User: /agents spawn 3 run tests
  → compose_command → SubmitPrompt(TurnStart) emitted to transport
  → enqueue_autonomy_hydration(ListAgents)          ← queued immediately
  → event loop drains hydration queue → ListAgents RPC
  → AgentList result → activity chips (fix_agent_list code)
  → AgentUpdated push events → progress rows update live
```

No new `AppUiCommand` variant or `APPUI_METHOD_*` constant is needed.

---

## Components

### `src/autonomy.rs`
- Add `AgentsCommand::Spawn { count: u32, prompt: String }`
- Add `AutonomyParseError::InvalidSpawnCount(String)` and `EmptySpawnPrompt`
- Add `Display` arms for new error variants
- Add `parse_agents_spawn(args)` helper called from `parse_agents`

### `src/store.rs`
- Add `AgentsCommand::Spawn` arm in `dispatch_agents_command`
- Gate on `APPUI_METHOD_AGENT_LIST`
- Call `self.state.enqueue_autonomy_hydration(ListAgents { … })`
- Return `Accepted(Some(SubmitPrompt(…)))`

### `src/menu/registry.rs`
- Register `/agents spawn` as a known command with `history_safe: false`

### `locales/en.yml` + `locales/zh.yml`

```yaml
status:
  spawning_agents: "Spawning {count} agent(s): {prompt}"

activity:
  spawn_agents_turn: "Spawn {count} agents to accomplish in parallel: {prompt}"

error:
  spawn_count_invalid: "spawn count must be a positive integer, got \"{raw}\""
  spawn_prompt_empty: "/agents spawn requires a prompt after the count"
```

---

## Error Handling & Capability Gating

| Scenario | Outcome |
|---|---|
| Server does not advertise `agent/list` | `Rejected`; standard "not available" status |
| `N` is not a positive integer | `InvalidSpawnCount` → status bar error message |
| No prompt after `N` | `EmptySpawnPrompt` → status bar error message |
| No active session | `Rejected` via `require_active_session()` guard |
| `TurnStart` RPC fails | Backend sends `TurnError`; existing handler renders it. Enqueued `ListAgents` still fires (returns 0 agents — acceptable). |

`history_safe: false` — raw prompt is never written to command history.

---

## Testing

### Parser unit tests (inline `src/autonomy.rs`)

| Input | Expected |
|---|---|
| `/agents spawn 3 run all tests` | `Spawn { count: 3, prompt: "run all tests" }` |
| `/agents spawn 1 fix lint` | `Spawn { count: 1, prompt: "fix lint" }` |
| `/agents spawn 0 anything` | `InvalidSpawnCount("0")` |
| `/agents spawn abc prompt` | `InvalidSpawnCount("abc")` |
| `/agents spawn 3` | `EmptySpawnPrompt` |
| `/agents spawn` | `InvalidSpawnCount("")` |

### Dispatch tests (inline `src/store.rs`)

- `spawn_dispatches_submit_prompt_with_formatted_text` — `TurnStart` text contains count and prompt
- `spawn_enqueues_agent_list_refresh` — `dequeue_autonomy_hydration()` yields `ListAgents` after spawn
- `spawn_rejected_when_agent_list_not_advertised` — capability gate returns `Rejected`
- `spawn_not_recorded_in_history` — `should_record_in_history("/agents spawn 3 ...")` is `false`

### Contract test (`tests/m15_autonomy_dispatch_contract.rs`)

- `agents_spawn_dispatches_turn_and_queues_list_refresh` — compose `/agents spawn 2 do work`, assert `SubmitPrompt` returned + `ListAgents` in hydration queue
