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
- Split `args` on the first whitespace. The first token is the count, the remainder is the prompt.
- Count token must parse as a decimal integer; if the token is empty or non-numeric → `InvalidSpawnCount(token.to_string())`.
- Count must be ≥ 1; `0` is rejected with `InvalidSpawnCount("0")` explicitly — the `≥ 1` rule is enforced after parsing, not before.
- `prompt` is all remaining text after the count token; if empty (no text after count) → `EmptySpawnPrompt`.
- No abbreviation alias; `spawn` only.

New `AutonomyParseError` variants:

```rust
/// `/agents spawn` where the count token is missing, non-numeric, or zero.
InvalidSpawnCount(String),
/// `/agents spawn <N>` with no prompt text after the count.
EmptySpawnPrompt,
```

### Dispatch (`src/store.rs`, `dispatch_agents_command`)

`dispatch_agents_command` returns `Option<AppUiCommand>` (not `SlashDispatchOutcome`; that wrapping happens one level up in `dispatch_autonomy_slash`). The `Spawn` arm:

```rust
AgentsCommand::Spawn { count, prompt } => {
    if !self.require_appui_method(APPUI_METHOD_AGENT_LIST) {
        return None;
    }
    // Status bar feedback.
    self.state.status = t!("status.spawning_agents",
        count = count, prompt = prompt).into_owned();
    // Queue an agent/list refresh to fire after the turn is dispatched.
    self.state.enqueue_autonomy_hydration(AppUiCommand::ListAgents(
        crate::model::AgentListParams {
            session_id: session_id.clone(),
            parent_agent_id: None,
        },
    ));
    // Emit the turn that asks the LLM to spawn agents.
    // Construct TurnStartParams using the same pattern as the existing
    // SubmitPrompt call in compose_command (session_id, turn_id, input as
    // vec![InputItem::Text { text: … }], media: vec![], topic: None, …).
    Some(AppUiCommand::SubmitPrompt(TurnStartParams {
        // input carries the message text, not a top-level `text` field:
        input: vec![InputItem::Text {
            text: t!("status.spawn_agents_turn",
                count = count, prompt = prompt).into_owned(),
        }],
        // … remaining fields (session_id, turn_id, media, topic, reasoning_effort, …)
        // filled identically to the existing SubmitPrompt path in compose_command
    }))
}
```

**Active-turn behavior:** `/agents spawn` feeds `SubmitPrompt` into the same `compose_command` path as a normal chat message. If a turn is already in flight, the FIFO staging logic in `compose_command` (the `pending_messages` / staged-submit path) applies automatically — the spawn turn is queued and fires when the active turn settles. No special guard is needed in `dispatch_agents_command`.

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
- Add `AgentsCommand::Spawn` arm in `dispatch_agents_command` (see pseudocode above)
- Gate on `APPUI_METHOD_AGENT_LIST`
- Call `self.state.enqueue_autonomy_hydration(AppUiCommand::ListAgents(AgentListParams { … }))`
- Return `Some(AppUiCommand::SubmitPrompt(…))`

### `src/menu/registry.rs`
- No new `CommandSpec` registration is needed. `/agents spawn` resolves through the existing `"agents"` `CommandSpec`. The `should_record_in_history` guard already excludes it from command history because the `invocation.args` field is non-empty (the guard requires `args.trim().is_empty()` for a slash command to be recorded). No `history_safe` field exists on `CommandSpec`; history exclusion is derived from the args guard.

### `locales/en.yml` + `locales/zh.yml`

All keys go under the existing `status:` top-level section (matching the pattern of `status.activity_agent_list`, `status.goal_objective_empty`, etc.). Interpolation uses `%{var}` syntax.

```yaml
status:
  spawning_agents: "Spawning %{count} agent(s): %{prompt}"
  spawn_agents_turn: "Spawn %{count} agents to accomplish in parallel: %{prompt}"
  spawn_count_invalid: "spawn count must be a positive integer, got \"%{raw}\""
  spawn_prompt_empty: "/agents spawn requires a prompt after the count"
```

---

## Error Handling & Capability Gating

| Scenario | Outcome |
|---|---|
| Server does not advertise `agent/list` | `None` returned from dispatch; existing "not available" status message set by `require_appui_method` |
| Count token missing, non-numeric, or zero | `InvalidSpawnCount(token)` parse error → status bar `status.spawn_count_invalid` |
| No prompt text after count | `EmptySpawnPrompt` parse error → status bar `status.spawn_prompt_empty` |
| No active session | `Rejected` via `require_active_session()` guard before `dispatch_agents_command` is reached |
| Turn already active | Turn is FIFO-staged by normal `compose_command` staging path; fires after active turn settles |
| `SubmitPrompt` RPC fails | Backend sends `TurnError`; existing handler renders it. Enqueued `ListAgents` still fires (may return 0 agents — acceptable). |
| `/agents spawn 3 secret-prompt` | `should_record_in_history` returns `false` because `invocation.args` is non-empty; prompt never written to history |

---

## Testing

### Parser unit tests (inline `src/autonomy.rs`)

| Input | Expected |
|---|---|
| `/agents spawn 3 run all tests` | `Spawn { count: 3, prompt: "run all tests" }` |
| `/agents spawn 1 fix lint` | `Spawn { count: 1, prompt: "fix lint" }` |
| `/agents spawn 0 anything` | `InvalidSpawnCount("0")` — zero rejected by `≥ 1` rule after numeric parse |
| `/agents spawn abc prompt` | `InvalidSpawnCount("abc")` — non-numeric token |
| `/agents spawn 3` | `EmptySpawnPrompt` — count parsed, no remaining text |
| `/agents spawn` | `InvalidSpawnCount("")` — first token is empty string, fails numeric parse |

### Dispatch tests (inline `src/store.rs`, within the `mod tests` block)

- `spawn_dispatches_submit_prompt_with_formatted_text` — `TurnStart` text contains count and prompt
- `spawn_enqueues_agent_list_refresh` — `state.dequeue_autonomy_hydration()` yields `ListAgents` after spawn dispatch
- `spawn_rejected_when_agent_list_not_advertised` — capability gate returns `None`
- `spawn_not_recorded_in_history` — call `should_record_in_history("/agents spawn 3 run tests")` directly (it is a private `fn` in the same module) and assert it returns `false`; note this test must live in the inline `mod tests` block inside `src/store.rs` where `should_record_in_history` is visible

### Contract test (`tests/m15_autonomy_dispatch_contract.rs`)

- `agents_spawn_dispatches_turn_and_queues_list_refresh` — compose `/agents spawn 2 do work`, assert `SubmitPrompt` returned + `ListAgents` in hydration queue via `state.dequeue_autonomy_hydration()`
