# Moving Image — Launch Banner Reveal Animation

**Date:** 2026-06-26
**Branch:** moving_image
**Status:** Approved

---

## Overview

Add a row-by-row typing reveal animation to the OCTOS figlet wordmark in the launch banner (the box shown when a session has no messages). Each of the 6 figlet rows appears sequentially at 120 ms intervals — matching the existing spinner cadence — giving a ~720 ms reveal. The banner box height stays fixed throughout so layout does not shift.

---

## Architecture

A single `Option<Instant>` field (`banner_reveal_start`) added to `AppState` drives the animation. The event loop sets this timestamp the first time it detects the banner is active; `render_launch_banner` reads elapsed time to compute how many rows to expose. When the banner deactivates (session receives a message) the timestamp is cleared so the next empty session re-animates.

No external state machine, no extra event variants, no timers. The pattern mirrors the existing `run_state_started_at: Option<Instant>` field.

---

## Components

### `src/model.rs` — `AppState`

Add one field after the existing `run_state_started_at`:

```rust
/// Timestamp of when the launch banner first became visible in this
/// session. `None` until the banner activates; cleared when it
/// deactivates so the next empty session re-animates.
pub banner_reveal_start: Option<std::time::Instant>,
```

Default: `None`.

### `src/event_loop.rs` — animation loop

Two additions to the main `loop`:

1. **Start timestamp** — after existing `turn_active` detection, before the dirty check:
   ```rust
   let banner_active = crate::app::launch_banner_active(&store.state);
   if banner_active && store.state.banner_reveal_start.is_none() {
       store.state.banner_reveal_start = Some(Instant::now());
       dirty = true;
   } else if !banner_active {
       store.state.banner_reveal_start = None;
   }
   ```

2. **Animation firing** — extend the existing guard from `turn_active` to also include banner animation in progress:
   ```rust
   let banner_animating = store.state.banner_reveal_start
       .is_some_and(|t| t.elapsed() < BANNER_REVEAL_DURATION);
   if (turn_active || banner_animating) && last_animation.elapsed() >= ANIMATION_INTERVAL {
       dirty = true;
       last_animation = Instant::now();
   }
   ```
   Add constant alongside `ANIMATION_INTERVAL`:
   ```rust
   /// Total duration for the launch-banner row-by-row reveal (6 rows × 120 ms).
   const BANNER_REVEAL_DURATION: Duration = Duration::from_millis(720);
   ```
   Also extend the poll timeout to use `ANIMATION_INTERVAL` while banner is animating (same as for `turn_active`).

3. `launch_banner_active` must be made `pub(crate)` in `app.rs` so the event loop can call it.

### `src/app.rs` — `render_launch_banner`

Replace the unconditional figlet block with a reveal-aware version:

```rust
if show_figlet {
    let visible_rows = banner_visible_rows(app.banner_reveal_start);
    let fig_w = /* existing width calc */;
    lines.push(centered(vec![], 0));                 // blank padding row
    for (i, art) in ONBOARDING_LOGO_ART.lines().enumerate() {
        if i < visible_rows {
            lines.push(centered(
                vec![Span::styled(format!("{art:<fig_w$}"), accent)],
                fig_w,
            ));
        } else {
            lines.push(centered(vec![], 0));         // blank placeholder
        }
    }
    lines.push(centered(vec![], 0));                 // blank padding row
}
```

Add helper alongside `spinner_frame`:

```rust
/// Number of figlet rows to reveal based on elapsed time since the banner
/// first became active. Returns the full row count once the animation
/// completes, and 0 before the first timestamp is recorded.
fn banner_visible_rows(start: Option<std::time::Instant>) -> usize {
    const ROW_INTERVAL_MS: u128 = 120;
    // Derived from ONBOARDING_LOGO_ART at call time to stay in sync if the
    // art is ever updated. Not const because str::lines() is not const-stable.
    let total_rows = ONBOARDING_LOGO_ART.lines().count();
    match start {
        None => 0,
        Some(t) => ((t.elapsed().as_millis() / ROW_INTERVAL_MS) as usize + 1)
            .min(total_rows),
    }
}
```

---

## Data Flow

```
event_loop tick
  └─ launch_banner_active? ──yes──► banner_reveal_start is None?
                                         yes → set Some(Instant::now()), dirty=true
                                         no  → (already running)
  └─ banner_animating? ──yes──► dirty=true (at ANIMATION_INTERVAL cadence)

render_launch_banner
  └─ banner_visible_rows(app.banner_reveal_start)
       └─ returns 0..=6 based on elapsed / 120ms
  └─ renders visible rows as styled spans; remaining rows as blank lines
```

---

## Error Handling

- `Instant::elapsed()` is infallible.
- Narrow terminals (`show_figlet = false`) skip the figlet block entirely; reveal logic is inside `if show_figlet`, so compact banners are unaffected.
- Terminal resize mid-animation: the visible-row count is time-based, not render-count-based, so a resize during animation simply re-renders with the correct row count at that moment.

---

## Testing

- Existing tests that assert the full wordmark renders should set `banner_reveal_start: Some(Instant::now() - Duration::from_secs(10))` to pre-complete the animation.
- New unit test: `render_launch_banner_reveals_rows_progressively` — exercises `banner_visible_rows` at t=0, t=60ms, t=120ms, t=480ms, t=720ms and asserts correct row counts (1, 1, 2, 5, 6). Note: the formula `(elapsed_ms / 120) + 1` means t=0 already shows row 1 and t=480ms shows 5 rows.
- Existing test `render_launch_banner_shows_box_logo_and_greeting_on_empty_session` (app.rs) must be updated: set `banner_reveal_start: Some(Instant::now() - Duration::from_secs(10))` so the animation is pre-completed and all 6 figlet rows render.
- New unit test: `banner_reveal_start_cleared_when_banner_inactive` — verifies the timestamp is reset when a session gains a message.

---

## Out of Scope

- Animating the onboarding header (`render_onboarding_header`) — separate decision.
- Color-cycle or shimmer effects.
- Character-by-character reveal within a row.

## Known Limitations

**Session-switch between two empty sessions:** `banner_reveal_start` is a single scalar on `AppState`, not keyed per session. Switching from one empty session to another does not reset it — the second session resumes the animation from wherever the first left off. Fixing this would require clearing `banner_reveal_start` in the session-selection handlers (`select_next_session` / `select_prev_session` in `src/model.rs`). This is out of scope for the initial implementation; the effect is minor (animation may be partially or fully skipped on the second session).
