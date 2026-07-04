# /agents spawn Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `/agents spawn <N> <prompt>` slash command that sends a formatted turn to the LLM requesting N parallel sub-agents, then auto-refreshes the agent list.

**Architecture:** Parse the new command in `autonomy.rs`, dispatch it in `store.rs` using the existing `start_prompt_turn` helper plus `enqueue_autonomy_hydration` for the list refresh, and gate it on `APPUI_METHOD_AGENT_LIST`.

**Tech Stack:** Rust, `rust-i18n` (`t!` macro), `octos-core` UI protocol types, inline `#[test]` + integration tests in `tests/`.

---

## Chunk 1: Parser — new variants + parse logic + tests

**Files:**
- Modify: `src/autonomy.rs:35-54` (add `Spawn` variant to `AgentsCommand`)
- Modify: `src/autonomy.rs:157-210` (add error variants + `Display` arms)
- Modify: `src/autonomy.rs:253-278` (add `"spawn"` arm in `parse_agents`)
- Modify: `src/autonomy.rs:543+` (add tests in `mod tests`)

---

### Task 1: Write failing parser tests

**Files:**
- Modify: `src/autonomy.rs` (append to `mod tests`)

- [ ] **Step 1: Add failing tests to `mod tests` at the bottom of `src/autonomy.rs`**

Append inside the `mod tests { … }` block (after the `leading_slash_is_optional` test, before the closing `}`):

```rust
    #[test]
    fn agents_spawn_parses_count_and_prompt() {
        assert_eq!(
            parse_autonomy_slash("/agents spawn 3 run all tests").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::Spawn {
                count: 3,
                prompt: "run all tests".into(),
            }))
        );
    }

    #[test]
    fn agents_spawn_count_one_is_valid() {
        assert_eq!(
            parse_autonomy_slash("/agents spawn 1 fix lint").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::Spawn {
                count: 1,
                prompt: "fix lint".into(),
            }))
        );
    }

    #[test]
    fn agents_spawn_zero_count_is_rejected() {
        assert_eq!(
            parse_autonomy_slash("/agents spawn 0 anything").unwrap_err(),
            AutonomyParseError::InvalidSpawnCount("0".into())
        );
    }

    #[test]
    fn agents_spawn_non_numeric_count_is_rejected() {
        assert_eq!(
            parse_autonomy_slash("/agents spawn abc prompt").unwrap_err(),
            AutonomyParseError::InvalidSpawnCount("abc".into())
        );
    }

    #[test]
    fn agents_spawn_missing_prompt_is_rejected() {
        assert_eq!(
            parse_autonomy_slash("/agents spawn 3").unwrap_err(),
            AutonomyParseError::EmptySpawnPrompt
        );
    }

    #[test]
    fn agents_spawn_empty_args_is_rejected() {
        // "/agents spawn" with no tokens at all: first token is empty → InvalidSpawnCount
        assert_eq!(
            parse_autonomy_slash("/agents spawn").unwrap_err(),
            AutonomyParseError::InvalidSpawnCount("".into())
        );
    }

    #[test]
    fn agents_spawn_overflow_count_is_rejected() {
        // u32::MAX + 1 = 4294967296 overflows u32, so parse::<u32>() returns Err.
        assert_eq!(
            parse_autonomy_slash("/agents spawn 4294967296 prompt").unwrap_err(),
            AutonomyParseError::InvalidSpawnCount("4294967296".into())
        );
    }
```

- [ ] **Step 2: Run the failing tests**

```bash
cargo test -p octos-tui agents_spawn 2>&1 | head -40
```

Expected: compile error — `AgentsCommand::Spawn` does not exist yet.

---

### Task 2: Add `AgentsCommand::Spawn` variant

**Files:**
- Modify: `src/autonomy.rs:53` (after `Close(String)`)

- [ ] **Step 3: Add the variant to `AgentsCommand`**

In `src/autonomy.rs`, after line 53 (`Close(String),`), add:

```rust
    /// `/agents spawn <N> <prompt>` — request the LLM spawn N parallel agents.
    Spawn { count: u32, prompt: String },
```

- [ ] **Step 4: Add error variants to `AutonomyParseError`**

In `src/autonomy.rs`, after `InvalidInterval(String)` (currently last variant, ~line 181), add:

```rust
    /// `/agents spawn` where the count token is missing, non-numeric, or zero.
    InvalidSpawnCount(String),
    /// `/agents spawn <N>` with no prompt text after the count.
    EmptySpawnPrompt,
```

- [ ] **Step 5: Add `Display` arms for the new error variants**

In the `impl std::fmt::Display for AutonomyParseError` block, insert the two new arms **before the closing `}` of the `match self { … }` block** (i.e. after the `InvalidInterval` arm body at ~line 208, but before the `}` that closes the match). The structure looks like:

```
match self {
    ...
    Self::InvalidInterval(raw) => { ... }
    // ← insert here
}   // ← closing brace of match — do NOT insert after this
```

Add:

```rust
            Self::InvalidSpawnCount(raw) => write!(
                f,
                "spawn count must be a positive integer, got \"{raw}\""
            ),
            Self::EmptySpawnPrompt => {
                f.write_str("/agents spawn requires a prompt after the count")
            }
```

- [ ] **Step 6: Add `PartialEq` derives** — verify both new variants are covered by the existing `#[derive(Debug, Clone, PartialEq, Eq)]` on `AutonomyParseError` (they will be automatically, since `String` is `PartialEq`). No extra work needed.

---

### Task 3: Add `"spawn"` arm in `parse_agents` + helper

**Files:**
- Modify: `src/autonomy.rs:275` (before `other =>` catch-all)

- [ ] **Step 7: Add the `"spawn"` arm and helper function**

In `parse_agents`, replace the `other =>` catch-all line with:

```rust
        "spawn" => parse_agents_spawn(args),
        other => Err(AutonomyParseError::UnknownAgentsVerb(other.to_string())),
```

Then add this new private function after `parse_agents` (e.g. after line 278, before `parse_agent_artifact_read`):

```rust
fn parse_agents_spawn(args: &str) -> Result<AgentsCommand, AutonomyParseError> {
    let (count_token, prompt_rest) = split_head(args);
    // Empty token means bare "/agents spawn" with no count at all.
    if count_token.is_empty() {
        return Err(AutonomyParseError::InvalidSpawnCount(String::new()));
    }
    // Parse and range-check the count.
    let count: u32 = count_token
        .parse()
        .map_err(|_| AutonomyParseError::InvalidSpawnCount(count_token.to_string()))?;
    if count == 0 {
        return Err(AutonomyParseError::InvalidSpawnCount(count_token.to_string()));
    }
    // Require a non-empty prompt after the count.
    let prompt = prompt_rest.trim().to_string();
    if prompt.is_empty() {
        return Err(AutonomyParseError::EmptySpawnPrompt);
    }
    Ok(AgentsCommand::Spawn { count, prompt })
}
```

- [ ] **Step 8: Run the parser tests and confirm they pass**

```bash
cargo test -p octos-tui agents_spawn 2>&1 | tail -20
```

Expected: all 6 `agents_spawn_*` tests pass.

- [ ] **Step 9: Run the full autonomy test suite to check for regressions**

```bash
cargo test -p octos-tui --lib autonomy 2>&1 | tail -20
```

Expected: all existing autonomy tests still pass.

- [ ] **Step 10: Commit**

```bash
git add src/autonomy.rs
git commit -m "feat(autonomy): add /agents spawn <N> <prompt> parser"
```

---

## Chunk 2: Locale keys

**Files:**
- Modify: `locales/en.yml:452` (after `refreshing_agent_list`)
- Modify: `locales/zh.yml:909` (after `refreshing_agent_list`)

---

### Task 4: Add locale keys to `en.yml`

- [ ] **Step 1: Insert four keys after `refreshing_agent_list` in `locales/en.yml`**

After line 452 (`refreshing_agent_list: "Refreshing agent list"`), insert:

```yaml
  spawning_agents: "Spawning %{count} agent(s): %{prompt}"
  spawn_agents_turn: "Spawn %{count} agents to accomplish in parallel: %{prompt}"
  spawn_count_invalid: "spawn count must be a positive integer, got \"%{raw}\""
  spawn_prompt_empty: "/agents spawn requires a prompt after the count"
```

- [ ] **Step 2: Insert translated keys after `refreshing_agent_list` in `locales/zh.yml`**

After line 909 (`refreshing_agent_list: "正在刷新智能体列表"`), insert:

```yaml
  spawning_agents: "正在派遣 %{count} 个智能体：%{prompt}"
  spawn_agents_turn: "派遣 %{count} 个智能体并行完成：%{prompt}"
  spawn_count_invalid: "派遣数量必须为正整数，收到 \"%{raw}\""
  spawn_prompt_empty: "/agents spawn 需要在数量后面提供提示语"
```

- [ ] **Step 3: Verify locale compiles**

```bash
cargo build -p octos-tui 2>&1 | grep -E "error|warning.*unused" | head -20
```

Expected: clean build (no missing locale key errors — the `t!` macro panics at runtime for missing keys, not compile time, but any typo in YAML structure shows as a build warning).

- [ ] **Step 4: Commit**

```bash
git add locales/en.yml locales/zh.yml
git commit -m "feat(i18n): add /agents spawn locale keys"
```

---

## Chunk 3: Dispatch — store.rs arm + inline tests

**Files:**
- Modify: `src/store.rs:748-758` (add `Spawn` arm before closing `}` of `dispatch_agents_command`)
- Modify: `src/store.rs` (add 4 tests in `mod tests`)

---

### Task 5: Write failing dispatch tests

- [ ] **Step 1: Add failing dispatch tests to `src/store.rs` inline `mod tests`**

Find the `agents_list_dispatches_agent_list_rpc` test (~line 19125) and append the new tests after the existing agent-dispatch tests:

```rust
    #[test]
    fn spawn_dispatches_submit_prompt_with_formatted_text() {
        let mut store = protocol_store_with_autonomy();
        store.state.set_composer_text("/agents spawn 3 run integration tests");
        let command = store.compose_command().expect("spawn emits a command");
        match command {
            AppUiCommand::SubmitPrompt(params) => {
                let text = match params.input.first().expect("input has text") {
                    octos_core::ui_protocol::InputItem::Text { text } => text.clone(),
                    other => panic!("expected Text input item, got {other:?}"),
                };
                assert!(
                    text.contains("3"),
                    "turn text must contain the count, got: {text}"
                );
                assert!(
                    text.contains("run integration tests"),
                    "turn text must contain the prompt, got: {text}"
                );
            }
            other => panic!("expected SubmitPrompt, got {other:?}"),
        }
    }

    #[test]
    fn spawn_enqueues_agent_list_refresh() {
        let mut store = protocol_store_with_autonomy();
        store.state.set_composer_text("/agents spawn 2 do work");
        let _command = store.compose_command();
        // The hydration queue must contain a ListAgents RPC.
        let queued: Vec<_> =
            std::iter::from_fn(|| store.state.dequeue_autonomy_hydration()).collect();
        assert!(
            queued.iter().any(|c| matches!(c, AppUiCommand::ListAgents(_))),
            "spawn must enqueue a ListAgents refresh, got: {queued:?}"
        );
    }

    #[test]
    fn spawn_rejected_when_agent_list_not_advertised() {
        use crate::menu::CapabilitySet;
        let session = crate::model::SessionView {
            id: octos_core::SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        let mut store = Store {
            state: crate::model::AppState::new(
                vec![session],
                0,
                "ready".into(),
                Some("ws://example.test/ui-protocol".into()),
                false,
            ),
        };
        // Advertise autonomy feature but NOT agent/list method.
        store.state.capabilities = Some(CapabilitySet::from_methods_and_features(
            [],
            [crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1],
        ));
        store.state.set_composer_text("/agents spawn 2 do work");
        let command = store.compose_command();
        assert!(
            command.is_none(),
            "spawn must be rejected when agent/list is not advertised"
        );
        // Confirm it was the capability guard (not a session guard) that fired:
        // require_appui_method sets "Octos UI method `agent/list` is not advertised".
        assert!(
            store.state.status.contains("agent/list"),
            "status must name the missing method, got: {:?}",
            store.state.status
        );
    }

    #[test]
    fn spawn_not_recorded_in_history() {
        // should_record_in_history is private to this module — test it directly.
        assert!(
            !should_record_in_history("/agents spawn 3 run tests"),
            "/agents spawn has non-empty args so it must never be recorded"
        );
        assert!(
            !should_record_in_history("/agents spawn 1 fix lint errors"),
            "single-agent spawn with a long prompt must not be recorded"
        );
    }
```

- [ ] **Step 2: Run the failing tests**

```bash
cargo test -p octos-tui "spawn_dispatches\|spawn_enqueues\|spawn_rejected\|spawn_not_recorded" 2>&1 | head -40
```

Expected: compile errors — `AgentsCommand::Spawn` arm missing in `dispatch_agents_command`.

---

### Task 6: Add dispatch arm

**Files:**
- Modify: `src/store.rs:757` (before closing `}` of `dispatch_agents_command`)

- [ ] **Step 3: Add `AgentsCommand::Spawn` arm in `dispatch_agents_command`**

In `src/store.rs`, inside `dispatch_agents_command`, add a new arm before the closing `}` (currently after `AgentsCommand::Close`, ~line 757):

```rust
            AgentsCommand::Spawn { count, prompt } => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_LIST) {
                    return None;
                }
                // Queue an agent/list refresh to fire after the turn is dispatched.
                self.state.enqueue_autonomy_hydration(AppUiCommand::ListAgents(
                    crate::model::AgentListParams {
                        session_id: session_id.clone(),
                        parent_agent_id: None,
                    },
                ));
                let text = t!(
                    "status.spawn_agents_turn",
                    count = count,
                    prompt = prompt
                )
                .into_owned();
                let status = t!(
                    "status.spawning_agents",
                    count = count,
                    prompt = prompt
                )
                .into_owned();
                self.start_prompt_turn(text, status)
            }
```

- [ ] **Step 4: Run the dispatch tests**

```bash
cargo test -p octos-tui "spawn_dispatches\|spawn_enqueues\|spawn_rejected\|spawn_not_recorded" 2>&1 | tail -20
```

Expected: all 4 tests pass.

- [ ] **Step 5: Run the full store test suite for regressions**

```bash
cargo test -p octos-tui --lib 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/store.rs
git commit -m "feat(store): dispatch /agents spawn — turn + enqueued list refresh"
```

---

## Chunk 4: Contract test

**Files:**
- Modify: `tests/m15_autonomy_dispatch_contract.rs` (add one test)

---

### Task 7: Add integration contract test

- [ ] **Step 1: Add `agents_spawn_dispatches_turn_and_queues_list_refresh` to the contract file**

Open `tests/m15_autonomy_dispatch_contract.rs` and append after the last test:

```rust
/// `/agents spawn N <prompt>` dispatches a `SubmitPrompt` turn and
/// simultaneously enqueues an `agent/list` refresh in the hydration queue.
/// The TUI must never emit a hypothetical generic `agent/spawn` RPC.
#[test]
fn agents_spawn_dispatches_turn_and_queues_list_refresh() {
    // AppUiCommand is already imported at module level — no local use needed.
    let mut store = store_with_autonomy_session();
    store.state.composer = "/agents spawn 2 do work".into(); // match file's direct-assignment style

    let command = store.compose_command().expect("spawn emits a command");

    // Primary dispatch: a SubmitPrompt turn.
    assert!(
        matches!(command, AppUiCommand::SubmitPrompt(_)),
        "expected SubmitPrompt, got {command:?}"
    );

    // Side-effect: a ListAgents refresh must be queued.
    let queued: Vec<_> =
        std::iter::from_fn(|| store.state.dequeue_autonomy_hydration()).collect();
    assert!(
        queued.iter().any(|c| matches!(c, AppUiCommand::ListAgents(_))),
        "spawn must enqueue ListAgents in the hydration queue, got: {queued:?}"
    );
}
```

- [ ] **Step 2: Check what `store_with_autonomy_session` exports**

The helper at the top of the file uses `AppState::new` directly and sets capabilities — `set_composer_text` must be available. Verify:

```bash
grep "set_composer_text\|pub fn set_composer" src/store.rs | head -5
```

If `set_composer_text` is not `pub`, use direct field assignment instead: `store.state.composer = "/agents spawn 2 do work".into();`

- [ ] **Step 3: Run the contract test**

```bash
cargo test --test m15_autonomy_dispatch_contract agents_spawn 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 4: Run the full contract test file for regressions**

```bash
cargo test --test m15_autonomy_dispatch_contract 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/m15_autonomy_dispatch_contract.rs
git commit -m "test(contract): agents_spawn dispatches turn + queues list refresh"
```

---

## Final verification

- [ ] **Run the full test suite**

```bash
cargo test -p octos-tui 2>&1 | tail -15
```

Expected: all tests pass, no warnings about missing locale keys.

- [ ] **Confirm the new command appears in the help output** (if a `/help` or slash-popup exists)

```bash
grep -n "spawn" src/menu/registry.rs src/autonomy.rs
```

Verify `spawn` is handled and not silently swallowed by the `UnknownAgentsVerb` catch-all.
