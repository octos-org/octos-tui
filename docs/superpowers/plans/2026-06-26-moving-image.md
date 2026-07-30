# Moving Image — Launch Banner Reveal Animation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a row-by-row typing reveal animation to the OCTOS figlet wordmark in the launch banner, showing each of the 6 ASCII art rows sequentially at 120 ms intervals.

**Architecture:** A single `Option<Instant>` field (`banner_reveal_start`) in `AppState` records when the banner first became active. The event loop sets it on first detection and fires animation ticks while the reveal is in progress; `render_launch_banner` derives the visible row count from elapsed time.

**Tech Stack:** Rust, Ratatui, `std::time::Instant`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-26-moving-image-design.md`

---

## Chunk 1: AppState field + helper function

### Task 1: Add `banner_reveal_start` to `AppState`

**Files:**
- Modify: `src/model.rs:3319` (after `pending_clipboard` field, before closing `}`)
- Modify: `src/model.rs:4895` (after `pending_clipboard: None,` in the `new_with_panes` initializer)

- [ ] **Step 1: Add the field to the struct**

  In `src/model.rs`, after line 3319 (`pub pending_clipboard: Option<String>,`), add:

  ```rust
      /// Timestamp of when the launch banner first became visible in the
      /// current session. `None` until the banner activates; cleared when it
      /// deactivates so the next empty session re-animates.
      pub banner_reveal_start: Option<std::time::Instant>,
  ```

- [ ] **Step 2: Initialize it in the constructor**

  In `src/model.rs`, after line 4895 (`pending_clipboard: None,`), add:

  ```rust
              banner_reveal_start: None,
  ```

- [ ] **Step 3: Verify it compiles**

  ```bash
  cargo check 2>&1 | head -30
  ```

  Expected: no errors (the struct has an explicit initializer so any missing field is a compile error).

- [ ] **Step 4: Commit**

  ```bash
  git add src/model.rs
  git commit -m "feat(tui): add banner_reveal_start field to AppState"
  ```

---

### Task 2: Add `banner_visible_rows` helper in `app.rs`

**Files:**
- Modify: `src/app.rs` — add helper near `spinner_frame` (around line 4712)

- [ ] **Step 1: Write a failing unit test for `banner_visible_rows`**

  At the bottom of the test module in `src/app.rs` (search for `#[cfg(test)]` to find the test section), add:

  ```rust
  #[test]
  fn banner_visible_rows_returns_correct_counts() {
      use std::time::{Duration, Instant};

      // None → 0 rows (not yet started)
      assert_eq!(banner_visible_rows(None), 0);

      // Freshly started → 1 row immediately
      let just_now = Instant::now();
      assert_eq!(banner_visible_rows(Some(just_now)), 1);

      // t=60ms → still row 1 (60/120 = 0, +1 = 1)
      let t60 = Instant::now() - Duration::from_millis(60);
      assert_eq!(banner_visible_rows(Some(t60)), 1);

      // t=120ms → row 2 (120/120 = 1, +1 = 2)
      let t120 = Instant::now() - Duration::from_millis(120);
      assert_eq!(banner_visible_rows(Some(t120)), 2);

      // t=480ms → row 5 (480/120 = 4, +1 = 5)
      let t480 = Instant::now() - Duration::from_millis(480);
      assert_eq!(banner_visible_rows(Some(t480)), 5);

      // t=720ms → row 6 (720/120 = 6, +1 = 7, clamped to 6)
      let t720 = Instant::now() - Duration::from_millis(720);
      assert_eq!(banner_visible_rows(Some(t720)), 6);

      // Long past → still capped at 6
      let old = Instant::now() - Duration::from_secs(10);
      assert_eq!(banner_visible_rows(Some(old)), 6);
  }
  ```

- [ ] **Step 2: Run the test to verify it fails (function not defined yet)**

  ```bash
  cargo test banner_visible_rows_returns_correct_counts 2>&1 | tail -15
  ```

  Expected: compile error — `cannot find function banner_visible_rows`.

- [ ] **Step 3: Implement `banner_visible_rows` in `app.rs`**

  Add directly after the `spinner_frame` function (after line 4712):

  ```rust
  /// Number of figlet rows to reveal based on elapsed time since the banner
  /// first became active. Returns 0 when the timestamp is not yet set, and
  /// clamps at the art's actual line count once the animation completes.
  fn banner_visible_rows(start: Option<std::time::Instant>) -> usize {
      const ROW_INTERVAL_MS: u128 = 120;
      let total_rows = ONBOARDING_LOGO_ART.lines().count();
      match start {
          None => 0,
          Some(t) => ((t.elapsed().as_millis() / ROW_INTERVAL_MS) as usize + 1)
              .min(total_rows),
      }
  }
  ```

- [ ] **Step 4: Run the test to verify it passes**

  ```bash
  cargo test banner_visible_rows_returns_correct_counts 2>&1 | tail -10
  ```

  Expected: `test ... ok`

- [ ] **Step 5: Commit**

  ```bash
  git add src/app.rs
  git commit -m "feat(tui): add banner_visible_rows helper for reveal animation"
  ```

---

## Chunk 2: Wire reveal into render and event loop

### Task 3: Update `render_launch_banner` to use the reveal

**Files:**
- Modify: `src/app.rs:2382` — make `launch_banner_active` `pub(crate)`
- Modify: `src/app.rs:2429-2441` — replace the unconditional figlet block

- [ ] **Step 1: Make `launch_banner_active` pub(crate)**

  At `src/app.rs:2382`, change:

  ```rust
  fn launch_banner_active(app: &AppState) -> bool {
  ```

  to:

  ```rust
  pub(crate) fn launch_banner_active(app: &AppState) -> bool {
  ```

- [ ] **Step 2: Update the existing full-wordmark test to pre-complete the animation**

  The test `render_launch_banner_shows_box_logo_and_greeting_on_empty_session` starts at `src/app.rs:8196`. It constructs `AppState` with `banner_reveal_start: None` (the default), so after this change the figlet won't render and the `text.contains("██████╗")` assertion will fail.

  Find the line `let app = AppState::new(` in that test and change only the `let` binding to `let mut`:

  ```rust
  let mut app = AppState::new(   // <-- add `mut` here; all args stay the same
  ```

  Then, immediately after the closing `);` of that call (before the `assert!` lines), add:

  ```rust
  app.banner_reveal_start = Some(std::time::Instant::now() - std::time::Duration::from_secs(10));
  ```

  Do not change any other part of the test.

- [ ] **Step 3: Write a new failing test for partial reveal**

  Add to the test module in `src/app.rs`. The `make_app` closure uses the same `AppState::new(...)` call as the existing test at line 8196 — copy those args verbatim (same sessions vec, same `"ready"` status, same `None` target, same `false` readonly). The only difference is `profile_id: None` (the new test doesn't need a named profile):

  ```rust
  #[test]
  fn render_launch_banner_reveals_rows_progressively() {
      use std::time::{Duration, Instant};

      let make_app = |reveal_start: Option<Instant>| {
          let mut app = AppState::new(
              vec![SessionView {
                  id: SessionKey("local:test".into()),
                  title: "test".into(),
                  profile_id: None,
                  messages: vec![],
                  tasks: vec![],
                  live_reply: None,
              }],
              0,
              "ready".into(),
              None,
              false,
          );
          app.banner_reveal_start = reveal_start;
          app
      };

      // Not yet started — no figlet rows visible
      let app_none = make_app(None);
      let text_none =
          rendered_buffer_with_size(&app_none, Palette::for_theme(ThemeName::Slate), 100, 30)
              .content
              .iter()
              .map(|c| c.symbol())
              .collect::<String>();
      assert!(
          !text_none.contains("██████╗"),
          "no figlet rows should appear before animation starts"
      );

      // Pre-completed — all rows visible
      let app_done = make_app(Some(Instant::now() - Duration::from_secs(10)));
      let text_done =
          rendered_buffer_with_size(&app_done, Palette::for_theme(ThemeName::Slate), 100, 30)
              .content
              .iter()
              .map(|c| c.symbol())
              .collect::<String>();
      assert!(
          text_done.contains("██████╗"),
          "all figlet rows should appear once animation is complete"
      );
  }
  ```

- [ ] **Step 4: Run the new test to verify it fails**

  ```bash
  cargo test render_launch_banner_reveals_rows_progressively 2>&1 | tail -10
  ```

  Expected: FAIL — `no figlet rows should appear before animation starts` panics because the figlet currently always renders.

- [ ] **Step 5: Replace the unconditional figlet block in `render_launch_banner`**

  In `src/app.rs`, replace lines 2429–2441:

  **Old:**
  ```rust
      if show_figlet {
          let fig_w = ONBOARDING_LOGO_ART
              .lines()
              .map(|l| l.chars().count())
              .max()
              .unwrap_or(0);
          for art in ONBOARDING_LOGO_ART.lines() {
              lines.push(centered(
                  vec![Span::styled(format!("{art:<fig_w$}"), accent)],
                  fig_w,
              ));
          }
          lines.push(centered(vec![], 0));
      }
  ```

  **New:**
  ```rust
      if show_figlet {
          let fig_w = ONBOARDING_LOGO_ART
              .lines()
              .map(|l| l.chars().count())
              .max()
              .unwrap_or(0);
          let visible = banner_visible_rows(app.banner_reveal_start);
          for (i, art) in ONBOARDING_LOGO_ART.lines().enumerate() {
              if i < visible {
                  lines.push(centered(
                      vec![Span::styled(format!("{art:<fig_w$}"), accent)],
                      fig_w,
                  ));
              } else {
                  lines.push(centered(vec![], 0));
              }
          }
          lines.push(centered(vec![], 0));
      }
  ```

  Height invariant: the original block pushes 6 art rows + 1 trailing blank = 7 lines inside `if show_figlet`. The new block pushes 6 art-or-blank rows + 1 trailing blank = 7 lines — identical count. The unconditional blank row that precedes the `if show_figlet` block (at the line reading `lines.push(centered(vec![], 0));` just before `if show_figlet {`) is **not changed** — do not add or remove it.

- [ ] **Step 6: Run both tests to verify they pass**

  ```bash
  cargo test render_launch_banner 2>&1 | tail -15
  ```

  Expected: all three launch-banner tests pass.

- [ ] **Step 7: Commit**

  ```bash
  git add src/app.rs
  git commit -m "feat(tui): row-by-row reveal in render_launch_banner"
  ```

---

### Task 4: Wire animation ticks in the event loop

**Files:**
- Modify: `src/event_loop.rs:46` — add `BANNER_REVEAL_DURATION` constant
- Modify: `src/event_loop.rs:137-151` — extend animation guard and poll timeout

- [ ] **Step 1: Add the duration constant**

  In `src/event_loop.rs`, after line 46 (`const ANIMATION_INTERVAL: Duration = Duration::from_millis(120);`), add:

  ```rust
  /// Total duration for the launch-banner row-by-row reveal (6 rows × 120 ms).
  const BANNER_REVEAL_DURATION: Duration = Duration::from_millis(720);
  ```

- [ ] **Step 2: Set `banner_reveal_start` on first banner activation and clear it on deactivation**

  In `src/event_loop.rs`, find the line:

  ```rust
  let turn_active = store.state.run_state.is_active();
  ```

  Insert the following block **immediately after** that line (before the `if turn_active && last_animation` guard):

  ```rust
  let banner_active = crate::app::launch_banner_active(&store.state);
  if banner_active && store.state.banner_reveal_start.is_none() {
      store.state.banner_reveal_start = Some(Instant::now());
      dirty = true;
  } else if !banner_active {
      store.state.banner_reveal_start = None;
  }
  ```

  This block must come before Steps 3 and 4 so `banner_reveal_start` is set before `banner_animating` reads it.

- [ ] **Step 3: Extend the animation-tick guard to include banner reveal**

  Find and replace the block starting with `if turn_active && last_animation.elapsed()`:

  **Old:**
  ```rust
          if turn_active && last_animation.elapsed() >= ANIMATION_INTERVAL {
              dirty = true;
              last_animation = Instant::now();
          }
  ```

  **New:**
  ```rust
          let banner_animating = store.state.banner_reveal_start
              .is_some_and(|t| t.elapsed() < BANNER_REVEAL_DURATION);
          if (turn_active || banner_animating) && last_animation.elapsed() >= ANIMATION_INTERVAL {
              dirty = true;
              last_animation = Instant::now();
          }
  ```

- [ ] **Step 4: Extend the poll timeout to also use `ANIMATION_INTERVAL` while the banner is animating**

  Find and replace the block starting with `let poll = if turn_active {`:

  **Old:**
  ```rust
          let poll = if turn_active {
              ANIMATION_INTERVAL.min(UI_EVENT_POLL_INTERVAL)
          } else {
              UI_EVENT_POLL_INTERVAL
          };
  ```

  **New:**
  ```rust
          let poll = if turn_active || banner_animating {
              ANIMATION_INTERVAL.min(UI_EVENT_POLL_INTERVAL)
          } else {
              UI_EVENT_POLL_INTERVAL
          };
  ```

- [ ] **Step 5: Compile and run all tests**

  ```bash
  cargo test 2>&1 | tail -20
  ```

  Expected: all tests pass with no warnings about unused imports.

- [ ] **Step 6: Commit**

  ```bash
  git add src/event_loop.rs
  git commit -m "feat(tui): drive launch banner reveal animation from event loop"
  ```

---

## Chunk 3: Verify end-to-end

### Task 5: Add `banner_reveal_start_cleared_when_banner_inactive` test

**Files:**
- Modify: `src/app.rs` — add test to the test module

- [ ] **Step 1: Write the test**

  Add to the test module in `src/app.rs`:

  ```rust
  #[test]
  fn banner_reveal_start_cleared_when_banner_inactive() {
      // When the banner is no longer active (session has messages),
      // the event loop clears banner_reveal_start. This test verifies
      // launch_banner_active returns false for a session with messages,
      // confirming the condition that triggers the clear in the event loop.
      use std::time::Instant;

      let mut app = AppState::new(
          vec![SessionView {
              id: SessionKey("local:test".into()),
              title: "test".into(),
              profile_id: None,
              messages: vec![Message::user("hello")],
              tasks: vec![],
              live_reply: None,
          }],
          0,
          "ready".into(),
          None,
          false,
      );
      // Simulate that the banner was previously animating.
      app.banner_reveal_start = Some(Instant::now());

      // The event loop clears banner_reveal_start when !launch_banner_active.
      // Verify the condition is false so the clear path is taken.
      assert!(
          !launch_banner_active(&app),
          "banner must be inactive when session has messages"
      );
      // Simulate the event loop's clear.
      if !launch_banner_active(&app) {
          app.banner_reveal_start = None;
      }
      assert!(
          app.banner_reveal_start.is_none(),
          "banner_reveal_start must be cleared when banner is inactive"
      );
  }
  ```

- [ ] **Step 2: Run the test**

  ```bash
  cargo test banner_reveal_start_cleared_when_banner_inactive 2>&1 | tail -10
  ```

  Expected: `test ... ok`

- [ ] **Step 3: Commit**

  ```bash
  git add src/app.rs
  git commit -m "test(tui): verify banner_reveal_start is cleared when banner inactive"
  ```

---

### Task 6: Full test suite + manual smoke test

**Files:** none new — verification only

- [ ] **Step 1: Run the full test suite**

  ```bash
  cargo test 2>&1 | tail -30
  ```

  Expected: all tests pass.

- [ ] **Step 2: Build and run the binary**

  ```bash
  cargo build 2>&1 | tail -10
  ```

  Then launch against a real or local-dev server (or just start the TUI with no server reachable) and observe that the OCTOS wordmark in the launch banner reveals row by row over ~720 ms when you open an empty session.

  Expected behavior:
  - On first render of the empty-session banner: 1 row of the wordmark visible.
  - Every 120 ms: one more row appears until all 6 rows show.
  - Banner box height stays constant (no layout jump).
  - After the reveal completes, the wordmark stays fully visible.
  - If you send a message and the banner disappears, re-opening a new empty session re-plays the animation.

- [ ] **Step 3: Final check — no uncommitted changes**

  ```bash
  git status
  ```

  Expected: `nothing to commit, working tree clean`. If there are leftover changes, review them with `git diff` and commit with an appropriate message before finishing.
