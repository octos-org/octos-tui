//! Inline-viewport driver: owns the scrollback-flush bookkeeping that turns
//! octos-tui's "rebuild everything every frame" model into codex's "finalized
//! history → scrollback, live UI → inline viewport" model.
//!
//! The event loop ([`crate::event_loop`]) calls [`ScrollbackTracker::sync`] each
//! time it is about to draw. The tracker compares the committed message history
//! to what it has already pushed into the terminal's scrollback and returns the
//! *new* finalized lines to insert (and whether the prior scrollback must be
//! reset first, e.g. on a session switch or a hydrate that replaced history).
//!
//! Keeping this state in one small, unit-tested type — separate from the
//! escape-sequence emitter and the render code — is what makes the rearchitecture
//! reviewable: the "what is finalized" decision lives here, the "how to draw the
//! live UI" decision lives in [`crate::app`], and the "how to write scrollback"
//! mechanism lives in [`crate::insert_history`].

use ratatui::text::Line;

use crate::app::{self, CommittedFingerprint, LiveTurnFinalization};
use crate::model::AppState;
use crate::theme::Palette;

/// Tracks how much committed history has been flushed to terminal scrollback so
/// that each draw only appends the *newly finalized* lines.
#[derive(Debug, Default)]
pub struct ScrollbackTracker {
    /// Fingerprint of the committed history we last flushed.
    last: CommittedFingerprint,
    /// Number of committed messages already flushed for `last.session_id`.
    flushed_messages: usize,
    /// Active-turn content already streamed to scrollback while still live.
    active_live: Option<LiveTurnFinalization>,
    /// Recently completed live-turn watermarks kept long enough to dedupe the
    /// eventual committed assistant message / archived activity log.
    completed_live: Vec<LiveTurnFinalization>,
    /// Whether the last line pushed to scrollback was blank. Reply chunks stream
    /// in across many flushes; without carrying this, a chunk that ends on a
    /// blank followed by one that opens on a blank stacks into a 2-line gap at
    /// the seam (per-flush collapse can't see across flushes). Seeds the next
    /// flush's blank-run collapse so cross-flush seams close to one blank.
    last_flushed_ends_blank: bool,
    /// Whether the previous live tail had guarded sections (activity or pending
    /// messages) whose separator rows can become orphaned when that tail settles
    /// and shrinks away.
    live_tail_had_guarded_sections: bool,
}

/// What the event loop should do with scrollback before drawing the viewport.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScrollbackUpdate {
    /// Lines to insert into scrollback above the inline viewport, in order.
    pub lines_to_insert: Vec<Line<'static>>,
    /// When true, the previously flushed scrollback is stale (session switch or
    /// a hydrate that replaced history). The caller cannot un-write real
    /// scrollback, but it should treat `lines_to_insert` as a fresh full
    /// re-flush of the (now-current) committed history rather than an append.
    pub reset: bool,
    /// Watermark to use when rendering the inline live tail for this draw.
    pub live_tail_finalization: Option<LiveTurnFinalization>,
}

impl ScrollbackTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the COMMITTED flush watermark only, so the next [`Self::sync`]
    /// re-emits the entire committed history — while PRESERVING the live-turn
    /// watermarks (`active_live` / `completed_live`). Used when a `/btw`
    /// aside is dismissed: the viewport shrink strands a blank band that a
    /// committed re-flush fills, but the main turn may still be STREAMING —
    /// wiping the live watermarks would re-emit its already-streamed rows
    /// (they survive on screen; only the old viewport region was cleared)
    /// and duplicate them in scrollback (codex P2 on #288). The preserved
    /// `completed_live` also keeps the committed re-flush deduped against
    /// content that already streamed live.
    pub fn mark_committed_flush_stale(&mut self) {
        self.last = CommittedFingerprint::default();
        self.flushed_messages = 0;
        self.last_flushed_ends_blank = false;
    }

    /// Forget everything already flushed, so the next [`Self::sync`] re-emits
    /// the ENTIRE committed history (plus any already-streamed live-turn
    /// content) as a fresh first flush.
    ///
    /// Used when the terminal takes the full viewport reset path on a resize
    /// (width change either direction, or terminal-height shrink — see
    /// `Terminal::resize_viewport_to`): that reset clears the whole visible
    /// screen, erasing the transcript rows this tracker had flushed there.
    /// Without a re-flush the chat visually vanishes — a bare composer on an
    /// empty screen — with the pre-resize copy reachable only by scrolling
    /// real scrollback, wrapped at the old width.
    pub fn mark_flushed_stale(&mut self) {
        *self = Self::new();
    }

    /// Reconcile the tracker against the current app state and return the lines
    /// to push into scrollback. `wrap_width` is the inline-viewport width.
    pub fn sync(
        &mut self,
        app: &AppState,
        palette: Palette,
        wrap_width: usize,
    ) -> ScrollbackUpdate {
        let fingerprint = app::committed_messages_fingerprint(app);
        let previous_live_tail_had_guarded_sections = self.live_tail_had_guarded_sections;
        let (previous_live, next_live) = self.reconcile_active_live(app);
        let mut lines_to_insert = Vec::new();
        let mut reset = false;

        // No active session, or no committed messages yet → no committed
        // history to flush. Live-turn deltas below may still insert lines.
        if fingerprint.message_count == 0 {
            // Keep the session id so a later first message is treated as an
            // append, not a reset (avoids a spurious reset on the first flush).
            self.last = fingerprint;
            self.flushed_messages = 0;
        } else {
            // A fresh tracker (nothing flushed yet) treats its first flush as an
            // append of the whole current history, not a reset — there is no
            // prior scrollback to invalidate.
            let first_flush = self.flushed_messages == 0 && self.last.session_id.is_empty();
            let is_extension = first_flush
                || (fingerprint.session_id == self.last.session_id
                    && fingerprint.message_count >= self.flushed_messages
                    && is_prefix_preserved(&self.last, &fingerprint, self.flushed_messages));
            let covered_late_activity = self.covered_late_activity_arrived(app, &fingerprint);

            if is_extension {
                // Append only the messages we have not flushed yet. If a live
                // turn was already streamed into scrollback, skip the covered
                // prefix when the committed assistant/log catches up.
                let committed_start = self.flushed_messages;
                lines_to_insert.extend(app::finalized_history_lines_range_dedup_live(
                    app,
                    palette,
                    wrap_width,
                    committed_start,
                    &self.completed_live,
                ));
                self.flushed_messages = fingerprint.message_count;
                self.last = fingerprint;
                self.refresh_completed_live_coverage(app, Some(committed_start));
            } else if covered_late_activity {
                lines_to_insert.extend(app::finalized_late_activity_lines_for_coverages(
                    app,
                    palette,
                    wrap_width,
                    &self.completed_live,
                ));
                self.last = fingerprint;
                self.refresh_completed_live_coverage(app, None);
            } else {
                // Discontinuity: session switch or hydrate replaced history. We
                // cannot remove already-written scrollback, but we re-flush the
                // full current history so the up-to-date content is selectable
                // below the (now-stale) prior block. Rare (reconnect / session
                // switch).
                lines_to_insert.extend(app::finalized_history_lines(app, palette, wrap_width));
                self.flushed_messages = fingerprint.message_count;
                self.last = fingerprint;
                self.completed_live.clear();
                reset = true;
            }
        }

        if let Some(next) = next_live.as_ref() {
            let baseline = previous_live
                .clone()
                .unwrap_or_else(|| LiveTurnFinalization {
                    session_id: next.session_id.clone(),
                    turn_id: next.turn_id.clone(),
                    reply_flushed_text: String::new(),
                    activity_flushed_items: 0,
                    activity_flushed_keys: Vec::new(),
                });
            lines_to_insert.extend(app::finalized_live_turn_lines_between(
                app, palette, wrap_width, &baseline, next,
            ));
        }
        self.active_live = next_live.filter(LiveTurnFinalization::has_flushed_content);
        self.live_tail_had_guarded_sections =
            app::live_tail_has_guarded_sections(app, self.active_live.as_ref());

        // A single flush concatenates committed history + live-turn deltas, each
        // of which guards only its own separators; their seam can stack into a
        // multi-line gap. Collapse runs on the combined buffer — seeded with
        // whether the previously flushed scrollback line was blank, so blanks
        // stacked across the many small reply-streaming flushes also close to a
        // single blank. On a reset the prior scrollback is stale, so don't carry
        // the seam across it.
        let seam_seed = !reset && self.last_flushed_ends_blank;
        let drop_orphaned_leading_blank_run = !reset
            && previous_live_tail_had_guarded_sections
            && !self.live_tail_had_guarded_sections;
        self.last_flushed_ends_blank = app::collapse_blank_runs_seeded_orphan_guard(
            &mut lines_to_insert,
            seam_seed,
            drop_orphaned_leading_blank_run,
        );

        ScrollbackUpdate {
            lines_to_insert,
            reset,
            live_tail_finalization: self.active_live.clone(),
        }
    }

    fn reconcile_active_live(
        &mut self,
        app: &AppState,
    ) -> (Option<LiveTurnFinalization>, Option<LiveTurnFinalization>) {
        let previous = self.active_live.take();
        let matching_previous = match (previous, app.active_turn()) {
            (Some(previous), Some((session_id, turn_id)))
                if previous.matches_turn(session_id, turn_id) =>
            {
                Some(previous)
            }
            (Some(previous), _) => {
                self.archive_completed_live(previous);
                None
            }
            (None, _) => None,
        };
        let next = app::next_live_turn_finalization(app, matching_previous.as_ref());
        (matching_previous, next)
    }

    fn archive_completed_live(&mut self, finalization: LiveTurnFinalization) {
        if !finalization.has_flushed_content() {
            return;
        }
        if let Some(existing) = self.completed_live.iter_mut().find(|existing| {
            existing.session_id == finalization.session_id
                && existing.turn_id == finalization.turn_id
        }) {
            *existing = finalization;
        } else {
            self.completed_live.push(finalization);
        }
        const MAX_COMPLETED_LIVE_TURNS: usize = 8;
        if self.completed_live.len() > MAX_COMPLETED_LIVE_TURNS {
            let excess = self.completed_live.len() - MAX_COMPLETED_LIVE_TURNS;
            self.completed_live.drain(0..excess);
        }
    }

    fn covered_late_activity_arrived(
        &self,
        app: &AppState,
        fingerprint: &CommittedFingerprint,
    ) -> bool {
        fingerprint.session_id == self.last.session_id
            && fingerprint.message_count == self.flushed_messages
            && fingerprint.message_count == self.last.message_count
            && fingerprint.activity_log_count > self.last.activity_log_count
            && self.completed_live.iter().any(|coverage| {
                app::committed_activity_keys_for_live_finalization(app, coverage).is_some()
            })
    }

    fn refresh_completed_live_coverage(&mut self, app: &AppState, reply_start: Option<usize>) {
        for coverage in &mut self.completed_live {
            if let Some(start) = reply_start
                && app::committed_reply_matches_live_finalization(app, start, coverage)
            {
                coverage.reply_flushed_text.clear();
            }
            if let Some(committed_keys) =
                app::committed_activity_keys_for_live_finalization(app, coverage)
            {
                for key in committed_keys {
                    if !coverage.activity_flushed_keys.contains(&key) {
                        coverage.activity_flushed_keys.push(key);
                    }
                }
                coverage.activity_flushed_items = coverage.activity_flushed_keys.len();
            }
        }
    }
}

/// Whether the already-flushed prefix is preserved in the new fingerprint. We
/// only have a hash of the *whole* committed list, so when the message count is
/// unchanged we can compare hashes directly; when it grew we optimistically
/// treat it as an append (the common streaming/commit case). A hydrate that
/// rewrites earlier messages while also growing the list is the one case this
/// can miss; it is rare and self-heals on the next count-stable frame.
fn is_prefix_preserved(
    last: &CommittedFingerprint,
    next: &CommittedFingerprint,
    flushed: usize,
) -> bool {
    if next.message_count == last.message_count {
        // Same length: only an append-noop if the content is identical.
        return next.content_hash == last.content_hash;
    }
    // Grew: treat as append as long as we had flushed a prefix of it.
    next.message_count >= flushed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ThemeName;
    use crate::model::{ActivityItem, ActivityKind, AppState, TurnActivityLog};
    use octos_core::Message;
    use octos_core::SessionKey;
    use octos_core::app_ui::AppUiSession;
    use octos_core::ui_protocol::TurnId;

    fn palette() -> Palette {
        Palette::for_theme(ThemeName::Slate)
    }

    fn session_with(messages: Vec<Message>) -> AppUiSession {
        AppUiSession {
            id: SessionKey("local:test".into()),
            title: "t".into(),
            profile_id: None,
            messages,
            tasks: Vec::new(),
            live_reply: None,
        }
    }

    fn state(messages: Vec<Message>) -> AppState {
        AppState::new(vec![session_with(messages)], 0, "ready".into(), None, false)
    }

    #[test]
    fn first_flush_emits_all_committed_messages() {
        let app = state(vec![Message::user("hi"), Message::assistant("hello there")]);
        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, palette(), 60);
        assert!(!update.reset);
        assert!(
            !update.lines_to_insert.is_empty(),
            "expected committed lines to flush"
        );
    }

    #[test]
    fn mark_committed_flush_stale_preserves_live_watermarks() {
        // A /btw dismissal re-flushes committed history while the main turn
        // may still be streaming: the live watermarks must survive so (a)
        // already-streamed live rows are not re-emitted (they survive on
        // screen — only the old viewport region is cleared on shrink) and
        // (b) the committed re-flush stays deduped against content that
        // already streamed live.
        let mut tracker = ScrollbackTracker::new();
        let app = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let first = tracker.sync(&app, palette(), 60);
        assert!(!first.lines_to_insert.is_empty());

        let live = LiveTurnFinalization {
            session_id: "local:test".into(),
            turn_id: "turn-1".into(),
            reply_flushed_text: "streamed so far".into(),
            activity_flushed_items: 2,
            activity_flushed_keys: vec!["k1".into(), "k2".into()],
        };
        tracker.active_live = Some(live.clone());
        tracker.completed_live = vec![live.clone()];
        tracker.live_tail_had_guarded_sections = true;

        tracker.mark_committed_flush_stale();

        assert_eq!(
            tracker.active_live.as_ref().map(|l| l.turn_id.as_str()),
            Some("turn-1"),
            "active live watermark must survive"
        );
        assert_eq!(
            tracker.completed_live.len(),
            1,
            "completed live dedup watermarks must survive"
        );
        assert!(tracker.live_tail_had_guarded_sections);
        assert_eq!(tracker.flushed_messages, 0, "committed watermark reset");
    }

    #[test]
    fn mark_flushed_stale_reflushes_the_whole_transcript() {
        // The width-change full viewport reset clears the visible screen,
        // erasing the transcript rows already flushed there. After
        // mark_flushed_stale the next sync must re-emit the ENTIRE committed
        // history (as a plain flush, not a reset — the screen was already
        // cleared by the terminal), so the chat reappears freshly wrapped.
        let mut tracker = ScrollbackTracker::new();
        let app = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let first = tracker.sync(&app, palette(), 60);
        assert!(!first.lines_to_insert.is_empty());

        let settled = tracker.sync(&app, palette(), 60);
        assert!(
            settled.lines_to_insert.is_empty(),
            "no growth -> nothing to flush"
        );

        tracker.mark_flushed_stale();
        let reflushed = tracker.sync(&app, palette(), 50);
        assert!(!reflushed.reset, "the terminal reset already cleared");
        assert_eq!(
            reflushed.lines_to_insert.len(),
            app::finalized_history_lines(&app, palette(), 50).len(),
            "must re-emit the full committed history at the new width"
        );

        // ...and the tracker keeps working incrementally afterwards.
        let after = tracker.sync(&app, palette(), 50);
        assert!(after.lines_to_insert.is_empty());
    }

    #[test]
    fn appending_a_message_flushes_only_the_new_one() {
        let mut tracker = ScrollbackTracker::new();
        let app1 = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let first = tracker.sync(&app1, palette(), 60);
        let first_count = first.lines_to_insert.len();
        assert!(first_count > 0);

        let app2 = state(vec![
            Message::user("hi"),
            Message::assistant("a1"),
            Message::user("again"),
            Message::assistant("a2"),
        ]);
        let second = tracker.sync(&app2, palette(), 60);
        assert!(!second.reset, "append should not reset");
        assert!(
            !second.lines_to_insert.is_empty(),
            "expected the new messages to flush"
        );
        // The second flush is only the 2 new messages, so it is smaller than a
        // full re-flush of all 4 messages would be.
        let full = app::finalized_history_lines(&app2, palette(), 60);
        assert!(
            second.lines_to_insert.len() < full.len(),
            "append flush ({}) should be smaller than full ({})",
            second.lines_to_insert.len(),
            full.len()
        );
    }

    #[test]
    fn no_new_messages_flushes_nothing() {
        let mut tracker = ScrollbackTracker::new();
        let app = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let _ = tracker.sync(&app, palette(), 60);
        let again = tracker.sync(&app, palette(), 60);
        assert!(again.lines_to_insert.is_empty());
        assert!(!again.reset);
    }

    #[test]
    fn late_activity_log_archive_triggers_reflush() {
        let mut tracker = ScrollbackTracker::new();
        let mut app = state(vec![
            Message::user("build the site"),
            Message::assistant("done"),
        ]);
        let _ = tracker.sync(&app, palette(), 60);

        let session_id = app.sessions[0].id.clone();
        let turn_id = TurnId::new();
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo test")
                    .with_success(true),
            ],
        });

        let update = tracker.sync(&app, palette(), 60);
        let text = update
            .lines_to_insert
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(update.reset, "late activity log changes finalized history");
        assert!(
            text.contains("Agent task completed") && text.contains("Bash($ cargo test"),
            "reflush should include archived activity log: {text:?}"
        );
    }

    #[test]
    fn session_switch_triggers_reset_and_full_reflush() {
        let mut tracker = ScrollbackTracker::new();
        let app1 = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let _ = tracker.sync(&app1, palette(), 60);

        let other = AppUiSession {
            id: SessionKey("local:other".into()),
            title: "o".into(),
            profile_id: None,
            messages: vec![Message::user("q"), Message::assistant("a")],
            tasks: Vec::new(),
            live_reply: None,
        };
        let app2 = AppState::new(vec![other], 0, "ready".into(), None, false);
        let update = tracker.sync(&app2, palette(), 60);
        assert!(update.reset, "switching sessions should reset scrollback");
        assert!(!update.lines_to_insert.is_empty());
    }

    #[test]
    fn empty_session_flushes_nothing() {
        let mut tracker = ScrollbackTracker::new();
        let app = state(vec![]);
        let update = tracker.sync(&app, palette(), 60);
        assert!(update.lines_to_insert.is_empty());
        assert!(!update.reset);
    }
}
