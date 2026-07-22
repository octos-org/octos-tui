//! `transcript_build` — extracted from `app.rs` (#365 step 2). Items keep their
//! original names; `app.rs` glob-re-exports them so every call site is
//! unchanged. `use super::*` reaches the app module's remaining items.
use super::*;

/// Height (rows) the live inline viewport needs for the current chat state:
/// the live transcript tail + menu + indicators + composer + status. Bounded so
/// it never consumes the whole screen (history must stay visible in scrollback).
pub fn live_ui_height(app: &AppState, width: u16, height: u16) -> u16 {
    live_ui_height_with_finalization(app, width, height, None)
}

pub fn live_ui_height_with_finalization(
    app: &AppState,
    width: u16,
    height: u16,
    live_finalization: Option<&LiveTurnFinalization>,
) -> u16 {
    let composer_height = composer_height_for_size(app, width, height);
    let active_menu = active_menu_surface(app);
    let menu_height = menu_height_hint(active_menu.as_ref(), width, height);
    let autonomy_height = autonomy_indicator_height(app, width);
    let harness_height = harness_status_height(app);
    // The sub-agent selector strip renders between the composer and the status
    // row (see `render_viewport_with_finalization`); reserve its row here too or
    // the live viewport is oversubscribed by one row whenever agents exist and
    // Ratatui compresses a fixed row at the tail floor. Height-gated on the same
    // `height` the render pass uses, so reservation and layout never disagree.
    let agent_strip_height = agent_strip_height(app, height);
    // #407: the Peer Dock renders between the agent strip and the status row
    // (see `render_viewport_with_finalization`); reserve its rows here too or the
    // live viewport is oversubscribed whenever peers exist. Unlike the one-row
    // strips above, the expanded dock (the default) is several rows, so omitting
    // it under-reserves by up to `peer_strip_height` — Ratatui then compresses a
    // fixed row at the tail floor, clipping the composer / stranding scrollback
    // ghosts. Height-gated on the same `height` the render pass uses, so
    // reservation and layout never disagree.
    let peer_strip_height = peer_strip_height(app, height);
    // The parked-decision watchdog banner reserves one row above the composer
    // (see `render_viewport_with_finalization`); reserve it here too or the live
    // viewport is oversubscribed by one row while the escalation is showing.
    let decision_height = decision_banner_height(app);
    let chrome = menu_height
        + autonomy_height
        + harness_height
        + decision_height
        + agent_strip_height
        + peer_strip_height
        + composer_height
        + 1; // +1 status

    let tail_height = live_tail_height_with_finalization(app, width, height, live_finalization);
    // The live-tail pane is laid out with `Constraint::Min(1)`, so it always
    // occupies at least one row even when there is no in-flight content. Reserve
    // that floor here too, otherwise an empty tail under-reserves by a row and
    // the layout steals it from the composer (clipping the last input line).
    let total = chrome.saturating_add(tail_height.max(1));

    // Never let the live UI eat the whole screen: leave at least a few rows of
    // scrollback visible above it (so the user always sees prior output and can
    // start a selection there). Always at least the chrome — but HARD-capped
    // at height-2 (#232 #3, codex fold 4): a full-screen viewport has no
    // scroll region above it, and a ONE-row region is equally unusable
    // (DECSTBM requires top < bottom; xterm ignores `CSI 1;1r`), so flushed
    // history lines had nowhere to go and were silently repainted over on
    // tiny panes. Two rows above keep the DECSTBM region valid; the
    // degenerate 1–2-row terminals fall back to insert_history's full-screen
    // streaming path.
    let max_live = height.saturating_sub(2).max(1);
    let cap = height
        .saturating_sub(LIVE_VIEWPORT_MIN_SCROLLBACK)
        .max(chrome.min(max_live));
    total.clamp(chrome.min(max_live).max(1), cap.max(1))
}

/// Desired height of the live transcript tail (in-flight / uncommitted content
/// shown inside the viewport). Bounded; the bulk of history lives in scrollback.
pub(super) fn live_tail_height_with_finalization(
    app: &AppState,
    width: u16,
    height: u16,
    live_finalization: Option<&LiveTurnFinalization>,
) -> u16 {
    if launch_banner_active(app) {
        // The empty-session launch banner wants a generous block.
        return height.saturating_sub(8).clamp(6, 16);
    }
    let wrap_width = crate::model::transcript_wrap_width_for(width);
    let lines = live_tail_lines_with_finalization(
        app,
        Palette::for_theme(app.theme),
        wrap_width,
        live_finalization,
    );
    let transcript_rows = transcript_visual_rows(&lines, wrap_width) as u16;
    // The STREAMING transcript is always capped at half the viewport so a long
    // in-flight turn can't dominate the screen — the rest stays in scrollback.
    let half = (height / 2).max(3);
    let capped_transcript = transcript_rows.min(half);
    // The `/btw` aside draws as a floating overlay OVER the tail's top rows
    // (`render_btw_overlay`) and adds no flow lines of its own — reserve its
    // rows here or a settled/short tail starves the overlay's 3-row minimum and
    // the pane becomes invisible while still answering (codex P1). Unlike the
    // transcript, a `/btw` aside is a focused reading surface the user
    // explicitly opened, so ITS reservation may exceed the half mark to fit the
    // whole answer rather than stranding its tail behind a scroll. Merging AFTER
    // the transcript cap keeps a long stream half-capped even while an aside is
    // open (only the aside's own height grows). The outer `live_ui_height` clamp
    // still reserves `LIVE_VIEWPORT_MIN_SCROLLBACK` rows of scrollback, and the
    // overlay scrolls whatever still doesn't fit on a small terminal.
    let btw_hint = btw_overlay_height_hint(app, width);
    capped_transcript.max(btw_hint).min(height)
}

pub(crate) fn live_tail_has_guarded_sections(
    app: &AppState,
    live_finalization: Option<&LiveTurnFinalization>,
) -> bool {
    !app.pending_messages.is_empty() || live_tail_has_activity_section(app, live_finalization)
}

pub(super) fn live_tail_has_activity_section(
    app: &AppState,
    live_finalization: Option<&LiveTurnFinalization>,
) -> bool {
    let mut flow_activity = flow_activity_items(app);
    if let Some(finalization) = active_live_finalization(app, live_finalization) {
        flow_activity = flow_activity
            .into_iter()
            .enumerate()
            .filter(|(idx, item)| {
                !finalization
                    .activity_flushed_keys
                    .contains(&activity_finalization_key(item, *idx))
            })
            .map(|(_, item)| item)
            .collect();
    }
    !flow_activity.is_empty()
}

pub(super) fn live_tail_lines_with_finalization(
    app: &AppState,
    palette: Palette,
    wrap_width: usize,
    live_finalization: Option<&LiveTurnFinalization>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(session) = app.active_session() else {
        return lines;
    };
    let active_finalization = active_live_finalization(app, live_finalization);

    // `should_show_turn_flow` already covers the visible-approval and
    // visible-question cases (it ORs them in), so a single branch suffices.
    if should_show_turn_flow(app, session) {
        let interactive_context_visible = app
            .approval
            .as_ref()
            .is_some_and(|approval| approval.visible)
            || app
                .user_question
                .as_ref()
                .is_some_and(|picker| picker.visible);
        // The recent-user-context pin is only needed while an interactive overlay
        // (approval / question) is visible — there it shows which prompt you're
        // acting on. Otherwise the committed prompt is already in native scrollback
        // just above the live tail, so pinning it again duplicates it (bug 2A: most
        // visibly for a mid-turn-submitted prompt whose turn hasn't replied yet —
        // the pin and the scrollback copy both sit on screen). The old
        // `!has_flushed_content` clause showed the pin for every just-started turn,
        // which is exactly the redundant case.
        let show_recent_context = interactive_context_visible;
        if show_recent_context
            && let Some(prompt) = latest_user_message(session)
                .filter(|prompt| !pending_messages_contains(&app.pending_messages, prompt))
        {
            push_recent_user_context(&mut lines, palette, prompt, wrap_width);
        }
        push_turn_flow(
            &mut lines,
            palette,
            app,
            session,
            wrap_width,
            active_finalization,
        );
    }

    if !app.pending_messages.is_empty() {
        push_pending_messages_block(&mut lines, palette, &app.pending_messages, wrap_width);
    }

    // Collapse interior multi-blank runs (recent-context → turn-flow →
    // pending-messages each guard only their own separator, so their seams can
    // stack) before trimming the trailing spacer rows below. Both run on the
    // shared builder, so the height calc and the render stay in lock-step.
    collapse_blank_runs(&mut lines);

    // Trailing spacer rows inflate the inline viewport height; once the turn
    // settles and the tail shrinks they become permanent blank rows in the
    // append-only scrollback (the "scar"). Trimming here shrinks the viewport
    // to hug real content, and since BOTH the height calc and the render read
    // this same builder, the two stay in lock-step (no off-by gap).
    trim_trailing_blank_lines(&mut lines);

    lines
}

/// The finalized transcript lines to push into scrollback: committed
/// `session.messages` plus anchored completed turn activity logs. Append-only as
/// long as messages are append-only; the scrollback tracker detects
/// discontinuities (session switch / hydrate replace / late activity-log
/// archive) and re-flushes from scratch.
pub fn finalized_history_lines(
    app: &AppState,
    palette: Palette,
    wrap_width: usize,
) -> Vec<Line<'static>> {
    finalized_history_lines_range(app, palette, wrap_width, 0)
}

/// Like [`finalized_history_lines`] but only renders committed messages from
/// index `start` onward — used to flush *just the newly committed* messages to
/// scrollback without re-emitting the whole history every turn.
pub fn finalized_history_lines_range(
    app: &AppState,
    palette: Palette,
    wrap_width: usize,
    start: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(session) = app.active_session() else {
        return lines;
    };
    let anchored_activity_logs = anchored_turn_activity_logs(app, session);
    for (idx, message) in session.messages.iter().enumerate().skip(start) {
        push_message_block(
            &mut lines,
            palette,
            message.role.as_str(),
            &message.content,
            wrap_width,
        );
        // Committed reasoning renders here only when the session opted in via
        // the `/thinking` display toggle (off = codex-style quiet default).
        push_reasoning_block(
            &mut lines,
            palette,
            message.reasoning_content.as_deref(),
            app.reasoning_display_enabled(&session.id),
            app.expanded_tool_outputs,
        );
        if let Some(tool_call_id) = message.tool_call_id.as_deref() {
            lines.push(Line::from(vec![
                Span::styled("         tool_call ", palette.muted()),
                Span::styled(tool_call_id.to_string(), palette.text()),
            ]));
        }

        for (_, log) in anchored_activity_logs
            .iter()
            .filter(|(anchor_idx, _)| *anchor_idx == idx)
        {
            push_turn_activity_log_section(&mut lines, palette, log, app, false, true, wrap_width);
        }
    }
    // Scrollback content must render on the terminal's NATIVE background, not the
    // theme surface. Message blocks bake a `surface` / `diff_context_bg`
    // background into their spans, and completed activity logs can inherit the
    // live tail's `surface_alt` background if they are promoted into scrollback.
    // codex writes finalized history on the default background; mirror that by
    // dropping every finalized line/span background here (the live viewport
    // render path is untouched, so it still shows the theme surface).
    strip_lines_background(&mut lines);
    lines
}

/// Render newly committed messages while skipping active-turn content that was
/// already streamed into scrollback before the turn settled.
pub fn finalized_history_lines_range_dedup_live(
    app: &AppState,
    palette: Palette,
    wrap_width: usize,
    start: usize,
    live_coverages: &[LiveTurnFinalization],
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(session) = app.active_session() else {
        return lines;
    };
    let anchored_activity_logs = anchored_turn_activity_logs(app, session);
    let mut used_reply_coverages = vec![false; live_coverages.len()];
    for (idx, message) in session.messages.iter().enumerate().skip(start) {
        let reply_coverage_idx =
            live_coverages
                .iter()
                .enumerate()
                .find_map(|(coverage_idx, coverage)| {
                    (!used_reply_coverages[coverage_idx]
                        && live_reply_coverage_matches_message(
                            app, session, idx, message, coverage,
                        ))
                    .then_some(coverage_idx)
                });
        if let Some(coverage_idx) = reply_coverage_idx {
            used_reply_coverages[coverage_idx] = true;
            let coverage = &live_coverages[coverage_idx];
            let suffix = &message.content[coverage.reply_flushed_text.len()..];
            let prefix_ends_blank =
                live_reply_prefix_ends_blank(palette, &coverage.reply_flushed_text, wrap_width);
            // A trailing Session Summary in the suffix must render as a card
            // here too — this live-flushed-prefix branch is the normal
            // long-running-tool partial-completion case and otherwise emits
            // flat markdown (codex P2 on #292). Split the prose suffix from the
            // summary; render the prose seeded (no bullet), then the card.
            if let Some(start) = session_summary_block_start(suffix) {
                let body = suffix[..start].trim_end();
                if !body.is_empty() {
                    push_live_reply_block_seeded(
                        &mut lines,
                        palette,
                        body,
                        wrap_width,
                        false,
                        true,
                        prefix_ends_blank,
                    );
                }
                let bg = chat_message_bg(palette, "assistant");
                push_session_summary_card(&mut lines, palette, &suffix[start..], bg, wrap_width);
            } else {
                // Continuation of a reply whose prefix is already in scrollback
                // (coverage matched only when non-empty) — never re-issue the
                // bullet, but seed blank handling from the streamed prefix so a
                // separator split across commit still renders like one document.
                push_live_reply_block_seeded(
                    &mut lines,
                    palette,
                    suffix,
                    wrap_width,
                    false,
                    true,
                    prefix_ends_blank,
                );
            }
        } else if message.role.as_str() == "assistant" {
            let boundaries =
                committed_reply_segment_boundaries_for_message(app, session, idx, &message.content);
            if boundaries.is_empty() {
                push_message_block(
                    &mut lines,
                    palette,
                    message.role.as_str(),
                    &message.content,
                    wrap_width,
                );
            } else {
                push_committed_assistant_reply_segments(
                    &mut lines,
                    palette,
                    &message.content,
                    wrap_width,
                    &boundaries,
                );
            }
        } else {
            push_message_block(
                &mut lines,
                palette,
                message.role.as_str(),
                &message.content,
                wrap_width,
            );
        }
        // Committed reasoning renders here only when the session opted in via
        // the `/thinking` display toggle (off = codex-style quiet default).
        push_reasoning_block(
            &mut lines,
            palette,
            message.reasoning_content.as_deref(),
            app.reasoning_display_enabled(&session.id),
            app.expanded_tool_outputs,
        );
        if let Some(tool_call_id) = message.tool_call_id.as_deref() {
            lines.push(Line::from(vec![
                Span::styled("         tool_call ", palette.muted()),
                Span::styled(tool_call_id.to_string(), palette.text()),
            ]));
        }

        for (_, log) in anchored_activity_logs
            .iter()
            .filter(|(anchor_idx, _)| *anchor_idx == idx)
        {
            if let Some(coverage) = live_coverages
                .iter()
                .find(|coverage| coverage.matches_turn(&log.session_id, &log.turn_id))
            {
                push_turn_activity_log_section_unflushed(
                    &mut lines, palette, log, app, coverage, wrap_width,
                );
            } else {
                push_turn_activity_log_section(
                    &mut lines, palette, log, app, false, true, wrap_width,
                );
            }
        }
    }
    strip_lines_background(&mut lines);
    lines
}

pub(super) fn committed_reply_segment_boundaries_for_message(
    app: &AppState,
    session: &SessionView,
    message_idx: usize,
    content: &str,
) -> Vec<usize> {
    let mut boundaries = app
        .live_reply_segment_boundaries
        .iter()
        .filter(|((session_id, _), _)| session_id == &session.id)
        .filter(|((session_id, turn_id), _)| {
            let coverage = LiveTurnFinalization {
                session_id: session_id.0.clone(),
                turn_id: turn_id.0.to_string(),
                ..Default::default()
            };
            committed_reply_index_for_live_finalization(app, session, &coverage)
                == Some(message_idx)
        })
        .flat_map(|(_, boundaries)| boundaries.iter().copied())
        .filter(|boundary| {
            *boundary > 0
                && *boundary < content.len()
                && content.is_char_boundary(*boundary)
                && boundary_is_word_safe(content, *boundary)
        })
        .collect::<Vec<_>>();
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

pub(super) fn push_committed_assistant_reply_segments(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    wrap_width: usize,
    boundaries: &[usize],
) {
    // A trailing "Session Summary" block (appended after a partial reply) must
    // render as a card here too — this segmented native-scrollback path is
    // used for tool-backed replies and otherwise bypasses `push_message_block`'s
    // summary detection (codex P2 on #292). Split the prose body from the
    // summary, render the body's segments (boundaries within it), then the
    // card. Recursion terminates because the body has no summary block.
    if let Some(start) = session_summary_block_start(content) {
        let body = content[..start].trim_end();
        let summary = &content[start..];
        if !body.is_empty() {
            let body_boundaries: Vec<usize> = boundaries
                .iter()
                .copied()
                .filter(|boundary| *boundary < body.len())
                .collect();
            push_committed_assistant_reply_segments(
                lines,
                palette,
                body,
                wrap_width,
                &body_boundaries,
            );
        }
        let bg = chat_message_bg(palette, "assistant");
        push_session_summary_card(lines, palette, summary, bg, wrap_width);
        return;
    }

    let mut cursor = 0;
    let mut first = true;
    let mut previous_reply_has_output = false;
    let mut previous_reply_ends_blank = false;

    for boundary in boundaries {
        if *boundary > cursor {
            let chunk = &content[cursor..*boundary];
            push_live_reply_block_seeded(
                lines,
                palette,
                chunk,
                wrap_width,
                first,
                previous_reply_has_output,
                previous_reply_ends_blank,
            );
            if !chunk.trim().is_empty() {
                first = false;
            }
            cursor = *boundary;
            previous_reply_has_output = !content[..cursor].trim().is_empty();
            previous_reply_ends_blank =
                live_reply_prefix_ends_blank(palette, &content[..cursor], wrap_width);
        }

        if *boundary < content.len() {
            push_live_reply_segment_separator(
                lines,
                previous_reply_has_output,
                previous_reply_ends_blank,
            );
            previous_reply_has_output = false;
            previous_reply_ends_blank = true;
            first = false;
        }
    }

    if cursor < content.len() {
        push_live_reply_block_seeded(
            lines,
            palette,
            &content[cursor..],
            wrap_width,
            first,
            previous_reply_has_output,
            previous_reply_ends_blank,
        );
    }
}

/// Render the delta between two active-turn watermarks for insertion into
/// native scrollback.
pub fn finalized_live_turn_lines_between(
    app: &AppState,
    palette: Palette,
    wrap_width: usize,
    previous: &LiveTurnFinalization,
    next: &LiveTurnFinalization,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some((session_id, turn_id)) = app.active_turn() else {
        return lines;
    };
    if !next.matches_turn(session_id, turn_id) {
        return lines;
    }

    if next
        .reply_flushed_text
        .starts_with(previous.reply_flushed_text.as_str())
    {
        push_live_reply_delta_seeded(
            &mut lines, app, session_id, turn_id, palette, wrap_width, previous, next,
        );
    }

    let previous_activity = previous
        .activity_flushed_keys
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let new_activity = flow_activity_items(app)
        .into_iter()
        .enumerate()
        .filter(|(idx, item)| {
            let key = activity_finalization_key(item, *idx);
            next.activity_flushed_keys.contains(&key) && !previous_activity.contains(&key)
        })
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    if !new_activity.is_empty() {
        // Each scrollback delta flush builds a fresh buffer, so a pure-activity
        // delta (a sub-agent completing with no reply text ahead of it) reaches
        // the finalized section with an EMPTY buffer — which defeats that
        // section's own `!lines.is_empty()` leading-blank guard and packs
        // consecutive agent-task cards together in native scrollback. Seed the
        // separator here so every flushed card stays blank-separated from the
        // previous scrollback block. (A reply-delta-then-activity flush leaves
        // `lines` non-empty, so the section's guard handles that case and this
        // never double-blanks.)
        if lines.is_empty() {
            lines.push(Line::from(""));
        }
        push_finalized_activity_items_section(
            &mut lines,
            palette,
            app,
            Some(turn_id),
            &new_activity,
            wrap_width,
        );
    }

    strip_lines_background(&mut lines);
    lines
}

pub(super) fn push_live_reply_delta_seeded(
    lines: &mut Vec<Line<'static>>,
    app: &AppState,
    session_id: &SessionKey,
    turn_id: &octos_core::ui_protocol::TurnId,
    palette: Palette,
    wrap_width: usize,
    previous: &LiveTurnFinalization,
    next: &LiveTurnFinalization,
) {
    let previous_len = previous.reply_flushed_text.len();
    let next_len = next.reply_flushed_text.len();
    let boundaries = live_reply_segment_boundaries_in_delta(
        app,
        session_id,
        turn_id,
        previous_len,
        next_len,
        &next.reply_flushed_text,
    );
    let mut cursor = previous_len;
    let mut first = previous.reply_flushed_text.is_empty();
    let mut previous_reply_has_output = !previous.reply_flushed_text.trim().is_empty();
    let mut previous_reply_ends_blank =
        live_reply_prefix_ends_blank(palette, &previous.reply_flushed_text, wrap_width);

    for boundary in boundaries {
        if boundary > cursor {
            let chunk = &next.reply_flushed_text[cursor..boundary];
            push_live_reply_block_seeded(
                lines,
                palette,
                chunk,
                wrap_width,
                first,
                previous_reply_has_output,
                previous_reply_ends_blank,
            );
            if !chunk.trim().is_empty() {
                first = false;
            }
            cursor = boundary;
            previous_reply_has_output = !next.reply_flushed_text[..cursor].trim().is_empty();
            previous_reply_ends_blank = live_reply_prefix_ends_blank(
                palette,
                &next.reply_flushed_text[..cursor],
                wrap_width,
            );
        }

        if boundary < next_len {
            push_live_reply_segment_separator(
                lines,
                previous_reply_has_output,
                previous_reply_ends_blank,
            );
            previous_reply_has_output = false;
            previous_reply_ends_blank = true;
            first = false;
        }
    }

    if cursor < next_len {
        push_live_reply_block_seeded(
            lines,
            palette,
            &next.reply_flushed_text[cursor..next_len],
            wrap_width,
            first,
            previous_reply_has_output,
            previous_reply_ends_blank,
        );
    }
}

pub(super) fn live_reply_segment_boundaries_in_delta(
    app: &AppState,
    session_id: &SessionKey,
    turn_id: &octos_core::ui_protocol::TurnId,
    previous_len: usize,
    next_len: usize,
    flushed_text: &str,
) -> Vec<usize> {
    let mut boundaries = app
        .live_reply_segment_boundaries
        .get(&(session_id.clone(), turn_id.clone()))
        .into_iter()
        .flatten()
        .copied()
        .filter(|boundary| {
            (previous_len..next_len).contains(boundary)
                && flushed_text.is_char_boundary(*boundary)
                && boundary_is_word_safe(flushed_text, *boundary)
        })
        .collect::<Vec<_>>();
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

pub(super) fn push_live_reply_segment_separator(
    lines: &mut Vec<Line<'static>>,
    previous_reply_has_output: bool,
    previous_reply_ends_blank: bool,
) {
    if lines.last().is_some_and(|line| line_is_blank(Some(line))) {
        return;
    }
    if !lines.is_empty() || (previous_reply_has_output && !previous_reply_ends_blank) {
        lines.push(Line::from(""));
    }
}

/// Render late archived activity for turns whose live activity rows were
/// already streamed to scrollback. This handles the common race where the final
/// assistant message commits first and `turn_activity_logs` catches up on a
/// later frame.
pub fn finalized_late_activity_lines_for_coverages(
    app: &AppState,
    palette: Palette,
    wrap_width: usize,
    live_coverages: &[LiveTurnFinalization],
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(session) = app.active_session() else {
        return lines;
    };
    for log in app
        .turn_activity_logs
        .iter()
        .filter(|log| log.session_id == session.id)
    {
        if let Some(coverage) = live_coverages
            .iter()
            .find(|coverage| coverage.matches_turn(&log.session_id, &log.turn_id))
        {
            push_turn_activity_log_section_unflushed(
                &mut lines, palette, log, app, coverage, wrap_width,
            );
        }
    }
    strip_lines_background(&mut lines);
    lines
}

pub fn committed_activity_keys_for_live_finalization(
    app: &AppState,
    coverage: &LiveTurnFinalization,
) -> Option<Vec<String>> {
    app.turn_activity_logs
        .iter()
        .find(|log| {
            log.session_id.0 == coverage.session_id && log.turn_id.0.to_string() == coverage.turn_id
        })
        .map(|log| {
            log.items
                .iter()
                .enumerate()
                .map(|(idx, item)| activity_finalization_key(item, idx))
                .collect()
        })
}

pub fn committed_reply_matches_live_finalization(
    app: &AppState,
    start: usize,
    coverage: &LiveTurnFinalization,
) -> bool {
    !coverage.reply_flushed_text.is_empty()
        && app.active_session().is_some_and(|session| {
            session
                .messages
                .iter()
                .enumerate()
                .skip(start)
                .any(|(idx, message)| {
                    live_reply_coverage_matches_message(app, session, idx, message, coverage)
                })
        })
}

/// A stable fingerprint of the committed messages already flushed to scrollback,
/// used by the event loop's scrollback tracker to decide whether new committed
/// messages are an append-only extension (flush the tail) or a discontinuity
/// (session switch / hydrate replace → reset + re-flush).
pub fn committed_messages_fingerprint(app: &AppState) -> CommittedFingerprint {
    let Some(session) = app.active_session() else {
        return CommittedFingerprint::default();
    };
    let anchored_activity_logs = anchored_turn_activity_logs(app, session);
    CommittedFingerprint {
        session_id: session.id.0.clone(),
        message_count: session.messages.len(),
        activity_log_count: anchored_activity_logs
            .iter()
            .filter(|(_, log)| !log.items.is_empty())
            .count(),
        // A cheap content hash of the committed messages so a hydrate that
        // *replaces* history (same count, different content) is detected. It
        // also covers archived activity logs, which can arrive after the
        // corresponding assistant message was already flushed.
        content_hash: committed_content_hash(session, &anchored_activity_logs),
    }
}

pub(super) fn committed_content_hash(
    session: &SessionView,
    anchored_activity_logs: &[(usize, &TurnActivityLog)],
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for message in &session.messages {
        message.role.as_str().hash(&mut hasher);
        message.content.hash(&mut hasher);
        // reasoning_content is NOT hashed: the /thinking display toggle applies
        // to turns committed AFTER it flips (a terminal cannot retroactively
        // redraw scrolled-off history — re-flushing would duplicate it). Past
        // turns stay as flushed; the Tab inspector always shows full reasoning
        // regardless of the toggle. So a reasoning change must not force a
        // full re-flush of unchanged visible history.
        message.tool_call_id.hash(&mut hasher);
    }
    for (render_index, log) in anchored_activity_logs {
        if log.items.is_empty() {
            continue;
        }
        render_index.hash(&mut hasher);
        log.session_id.0.hash(&mut hasher);
        log.turn_id.0.to_string().hash(&mut hasher);
        log.request.hash(&mut hasher);
        for item in &log.items {
            item.kind.label().hash(&mut hasher);
            item.title.hash(&mut hasher);
            item.status.hash(&mut hasher);
            item.detail.hash(&mut hasher);
            item.output_preview.hash(&mut hasher);
            item.success.hash(&mut hasher);
            item.duration_ms.hash(&mut hasher);
            item.tool_call_id.hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub(super) fn transcript_render_model(
    app: &AppState,
    palette: Palette,
    area: Rect,
) -> TranscriptRenderModel {
    let mut lines = Vec::new();
    let mut approval_context_start = None;
    let wrap_width = transcript_wrap_width(area);

    if let Some(session) = app.active_session() {
        let approval_visible = app
            .approval
            .as_ref()
            .is_some_and(|approval| approval.visible);
        let turn_flow_visible = should_show_turn_flow(app, session);
        let latest_user_index = session
            .messages
            .iter()
            .rposition(|message| message.role.as_str() == "user");
        let anchored_activity_logs = anchored_turn_activity_logs(app, session);
        let mut turn_flow_rendered = false;

        for (idx, message) in session.messages.iter().enumerate() {
            let message_start = lines.len();
            push_message_block(
                &mut lines,
                palette,
                message.role.as_str(),
                &message.content,
                wrap_width,
            );
            // Codex-style: the verbose committed `reasoning_content` is
            // intentionally NOT rendered into scrollback. The data is kept on the
            // message for a future /thinking reveal; we just don't push it here.
            if let Some(tool_call_id) = message.tool_call_id.as_deref() {
                lines.push(Line::from(vec![
                    Span::styled("         tool_call ", palette.muted()),
                    Span::styled(tool_call_id.to_string(), palette.text()),
                ]));
            }

            for (_, log) in anchored_activity_logs
                .iter()
                .filter(|(anchor_idx, _)| *anchor_idx == idx)
            {
                push_turn_activity_log_section(
                    &mut lines, palette, log, app, true, false, wrap_width,
                );
            }

            if turn_flow_visible && Some(idx) == latest_user_index {
                approval_context_start = Some(message_start);
                push_turn_flow(&mut lines, palette, app, session, wrap_width, None);
                turn_flow_rendered = true;
            }
        }

        if !turn_flow_rendered
            && approval_visible
            && let Some(prompt) = latest_user_message(session)
        {
            approval_context_start = Some(lines.len());
            push_recent_user_context(&mut lines, palette, prompt, wrap_width);
            push_turn_flow(&mut lines, palette, app, session, wrap_width, None);
        } else if !turn_flow_rendered {
            push_turn_flow(&mut lines, palette, app, session, wrap_width, None);
        }

        if !app.pending_messages.is_empty() {
            push_pending_messages_block(&mut lines, palette, &app.pending_messages, wrap_width);
        }
    } else {
        lines.push(Line::from(Span::styled(
            t!("app.empty.no_session").to_string(),
            palette.muted(),
        )));
    }

    collapse_blank_runs(&mut lines);

    let visible_height = transcript_visible_height(area);
    let total_rows = transcript_visual_rows(&lines, wrap_width);
    let max_scroll = total_rows.saturating_sub(visible_height);
    let scroll_from_bottom = app.transcript_scroll.min(max_scroll);
    let metrics = TranscriptScrollMetrics {
        visible_rows: visible_height,
        total_rows,
        scroll_from_bottom,
        max_scroll_from_bottom: max_scroll,
    };
    let mut scroll_top = max_scroll.saturating_sub(scroll_from_bottom);
    if scroll_from_bottom == 0
        && let Some(context_start) = approval_context_start
    {
        let context_row = transcript_visual_rows(&lines[..context_start], wrap_width);
        let context_tail_rows = total_rows.saturating_sub(context_row);
        if context_tail_rows <= visible_height {
            scroll_top = scroll_top.min(context_row);
        }
    }
    let scroll_top = u16::try_from(scroll_top).unwrap_or(u16::MAX);

    // In the pager the transcript blends with the terminal's DEFAULT
    // background, exactly like the inline live tail: pinned-mode wheel
    // scrolling enters the pager seamlessly, and painting `surface_alt` here
    // would flip the whole screen to the theme color mid-scroll (the
    // user-reported "screen went black"). Other full-screen surfaces
    // (inspector, detail-modal backdrops) keep `surface_alt`.
    let block_style = if app.transcript_pager_active {
        // Span-level backgrounds (message-block "bubbles") must go too:
        // committed history in native scrollback renders without them, so
        // keeping them here paints text-shaped theme-color stripes over the
        // terminal background the moment the user scrolls into the pager.
        for line in &mut lines {
            line.style.bg = None;
            for span in &mut line.spans {
                span.style.bg = None;
            }
        }
        Style::default().fg(palette.text)
    } else {
        Style::default().fg(palette.text).bg(palette.surface_alt)
    };

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .style(block_style)
                .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    TranscriptRenderModel { paragraph, metrics }
}

/// Visible content rows of the transcript surfaces. Both callers — the inline
/// live tail and the fullscreen `transcript_render_model` path — render a
/// BORDERLESS Paragraph (`Block::default().style(..).border_style(..)` draws
/// no border glyphs without `.borders()`), so every area row is a content row.
/// The old `-2` "border allowance" was phantom: with the live tail sized
/// exactly to its content it forced `max_scroll = 2`, permanently scrolling
/// the top 2 tail rows out of the area and leaving 2 dead rows at the bottom.
/// (The bordered detail modals compute their own `-2` next to their
/// `titled_block(..)` calls, where a border really exists.)
pub(super) fn transcript_visible_height(area: Rect) -> usize {
    usize::from(area.height).max(1)
}

pub(super) fn transcript_wrap_width(area: Rect) -> usize {
    crate::model::transcript_wrap_width_for(area.width)
}

pub(super) fn transcript_visual_rows(lines: &[Line<'static>], wrap_width: usize) -> usize {
    lines
        .iter()
        .map(|line| transcript_line_visual_rows(line, wrap_width))
        .sum()
}

pub(super) fn transcript_line_visual_rows(line: &Line<'static>, wrap_width: usize) -> usize {
    let width = line
        .spans
        .iter()
        .map(|span| span.content.as_ref().width())
        .sum::<usize>();
    width.max(1).div_ceil(wrap_width.max(1))
}

pub(super) fn live_reply_coverage_matches_message(
    app: &AppState,
    session: &SessionView,
    message_idx: usize,
    message: &Message,
    coverage: &LiveTurnFinalization,
) -> bool {
    if coverage.reply_flushed_text.is_empty()
        || message.role.as_str() != "assistant"
        || !message
            .content
            .starts_with(coverage.reply_flushed_text.as_str())
    {
        return false;
    }

    committed_reply_index_for_live_finalization(app, session, coverage)
        .is_none_or(|reply_idx| reply_idx == message_idx)
}

pub(super) fn committed_reply_index_for_live_finalization(
    app: &AppState,
    session: &SessionView,
    coverage: &LiveTurnFinalization,
) -> Option<usize> {
    let prompt_idx = app
        .turn_prompt_anchors
        .iter()
        .rev()
        .find(|anchor| {
            anchor.session_id == session.id
                && anchor.turn_id.0.to_string() == coverage.turn_id
                && anchor.session_id.0 == coverage.session_id
        })
        .and_then(|anchor| resolve_turn_prompt_anchor_for_render(session, anchor))
        .or_else(|| {
            app.turn_activity_logs
                .iter()
                .rev()
                .find(|log| {
                    log.session_id == session.id
                        && log.turn_id.0.to_string() == coverage.turn_id
                        && log.session_id.0 == coverage.session_id
                })
                .and_then(|log| log.anchor_index)
                .filter(|idx| user_message_at(session, *idx))
        })?;

    let reply_idx = activity_log_render_index(session, prompt_idx);
    session
        .messages
        .get(reply_idx)
        .is_some_and(|message| message.role.as_str() == "assistant")
        .then_some(reply_idx)
}

pub(super) fn push_turn_flow(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    session: &SessionView,
    width: usize,
    live_finalization: Option<&LiveTurnFinalization>,
) {
    if let Some(comp) = app.live_compaction.get(&session.id) {
        push_live_compaction_block(
            lines,
            palette,
            comp,
            app.session_context_window.get(&session.id).copied(),
        );
    }

    // The pending approval / AskUserQuestion card renders LAST in the turn flow
    // (see `push_pending_decision_cards` at the end of this fn) so it sits at the
    // BOTTOM of the height-clipped live tail — the always-visible region. It used
    // to render HERE (first), so on a turn with lots of streamed output the card
    // scrolled off the top while still pending and still owning the keyboard,
    // leaving the user with a bare "Waiting" and no visible prompt.

    // `/btw` aside renders as a floating overlay pinned to the TOP of the live
    // viewport (see `render_btw_overlay`), not inline here — otherwise it
    // mingles with the streaming reply/activity below it.

    // Live reasoning for the active turn: codex-style, we DON'T render the
    // verbose "thinking" text. The deltas still accumulate in `live_reasoning`
    // (so a future /thinking toggle can reveal them and commit_live_reply can
    // hand them to the message's reasoning_content); we only surface a single
    // dimmed swimming-octopus indicator, and ONLY while the model is still
    // reasoning — once the answer has started streaming (`live_reply.text` has
    // non-empty content for the active turn) we drop the indicator too. The
    // status-bar "Thinking" label is gated on the SAME predicate
    // (`active_turn_is_thinking`) so the octopus and the label never disagree.
    if active_turn_is_thinking(app) {
        push_thinking_indicator(lines, palette, width);
    }

    if let Some(live_reply) = &session.live_reply {
        let reply_text = if let Some(finalization) = live_finalization {
            live_reply
                .text
                .strip_prefix(finalization.reply_flushed_text.as_str())
                .unwrap_or(live_reply.text.as_str())
        } else {
            live_reply.text.as_str()
        };
        if !reply_text.trim().is_empty() {
            // The live-tail view shows the not-yet-flushed remainder; the
            // bullet belongs to it only while nothing was flushed yet.
            let first = live_finalization
                .is_none_or(|finalization| finalization.reply_flushed_text.is_empty());
            push_live_reply_block(lines, palette, reply_text, width, first);
        }
    }

    // While a reserved-row menu (slash/command popup, model picker, …) is open
    // it squeezes the live tail down to its `Constraint::Min(1)` floor, so the
    // in-flight activity chip renders as a single truncated "◜ Orchestrating…"
    // HEADER row (no sub-agent child line). On the menu-open viewport GROW the
    // terminal scrolls that squeezed header up into REAL scrollback, where the
    // menu-close `clear_visible_screen` (`CSI 2J`) cannot reclaim it — leaving a
    // frozen, one-spinner-frame-behind duplicate stranded above the fresh live
    // chip (user report: "duplicated orchestrating after slash commands"; the
    // two chips carry the same turn id but different spinner glyphs). The scroll
    // itself is a deliberately conservative invariant (see
    // `viewport_growth_after_width_reset_scrolls_full_deficit` / #232 #267), so
    // the fix is here, not in the scroll geometry: don't paint the chip while a
    // menu holds focus. With no squeezed header there is nothing to strand, and
    // the full chip returns the instant the menu closes.
    if app.active_menu.is_none() {
        push_activity_section_with_finalization(lines, palette, app, live_finalization, width);
    }

    if live_turn_diff_preview_visible(app) {
        push_inline_diff_preview(
            lines,
            palette,
            &app.diff_preview,
            app.expanded_tool_outputs,
            width,
        );
    }

    // Pinned LAST: a parked decision's card is the bottom-most turn-flow content,
    // so the height-clipped live tail (which keeps its bottom rows) always shows
    // it even when the streamed reply/activity above it overflows the viewport.
    push_pending_decision_cards(lines, palette, app, width);
}

/// Render the pending approval / AskUserQuestion card into the live tail. Called
/// last in [`push_turn_flow`] so the card is the bottom-most content and cannot
/// scroll off the top of the height-clipped tail while the turn is parked on the
/// operator (the "Waiting, can't type, no visible prompt" trap). Streamed
/// reply/activity above it clips first. Gated on `visible` exactly as before, so
/// keyboard ownership (`modal_owns_keyboard`) and rendering stay coupled.
pub(super) fn push_pending_decision_cards(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    width: usize,
) {
    if let Some(approval) = app.approval.as_ref().filter(|approval| approval.visible) {
        push_inline_approval_card(lines, palette, approval);
    }

    if let Some(picker) = app.user_question.as_ref().filter(|picker| picker.visible) {
        push_inline_user_question_card(lines, palette, picker, width);
    }
}

pub(super) fn live_turn_diff_preview_visible(app: &AppState) -> bool {
    if !app.diff_preview.active {
        return false;
    }
    let Some(diff_turn_id) = app.diff_preview.turn_id.as_ref() else {
        return true;
    };
    app.active_turn()
        .is_some_and(|(_, active_turn_id)| active_turn_id == diff_turn_id)
}

pub(super) fn push_recent_user_context(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    _width: usize,
) {
    push_user_message_block(lines, palette, content);
}

/// User input gets the strongest, most terminal-portable contrast in the
/// transcript: a **bright bold** accent `▌` gutter plus a **reverse-video**
/// (SGR 7) body bar on every logical line. Scanning for "what did I say" is
/// the most common review motion, so the user's turns are the single most
/// important visual anchor.
///
/// Reverse video is deliberate over an RGB background shade: it is a basic
/// terminal attribute every terminal renders — including plain SSH sessions
/// that don't advertise truecolor, where `Rgb()` backgrounds silently vanish —
/// and it works identically in the live viewport and in native scrollback. A
/// themed `surface_alt` shade was tried first and was invisible on both counts
/// (dropped over SSH, and a near-invisible ~10/255 lift even when rendered).
/// A single space of padding on each side frames the text so the highlight
/// reads as a bar rather than tight-wrapped text.
pub(super) fn push_user_message_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    // Bright bold accent gutter — a colored gutter always renders (the user
    // already sees the ▌), so it stays the role marker.
    let gutter = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    // Body is a REVERSE-VIDEO (SGR 7) + bold bar. Reverse video is a basic
    // terminal attribute every terminal supports, so it renders identically
    // over SSH and in native scrollback — unlike an RGB `surface_alt` shade,
    // which silently vanishes on sessions that don't advertise truecolor and
    // is a near-invisible ~10/255 lift even when it does. The gutter owns the
    // only separating space so rendered prompt text has no extra padding.
    let body = Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED);
    if content.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("▌ ", gutter),
            Span::styled("<empty>", body),
        ]));
        return;
    }
    for raw_line in content.lines() {
        let text = raw_line.trim_end();
        lines.push(Line::from(vec![
            Span::styled("▌ ", gutter),
            Span::styled(text.to_string(), body),
        ]));
    }
}

pub(super) fn push_live_compaction_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    comp: &crate::model::LiveCompaction,
    context_window: Option<u64>,
) {
    if let Some(completed_at) = comp.completed_at {
        // Settled: dwell for a short window, then render nothing (the entry
        // itself is bounded by turn-terminal sweeps / the next Started).
        if completed_at.elapsed().as_secs() >= LIVE_COMPACTION_SETTLED_DISPLAY_SECS {
            return;
        }
        let after = comp
            .token_estimate_after
            .unwrap_or(comp.token_estimate_before);
        lines.push(Line::from(vec![Span::styled(
            format!(
                "✶ {} ({} → {} tokens)",
                t!("status.activity_context_compacted"),
                humanize_token_count(comp.token_estimate_before),
                humanize_token_count(after),
            ),
            Style::default().fg(palette.accent),
        )]));
        lines.push(Line::from(""));
        return;
    }
    let elapsed = comp.started_at.elapsed().as_secs();
    let denominator = context_window
        .filter(|w| *w > 0)
        .unwrap_or_else(|| comp.threshold_tokens.max(1));
    let frac = comp.token_estimate_before as f64 / denominator as f64;
    lines.push(Line::from(vec![Span::styled(
        format!(
            "✶ {} ({}s · {} tokens)",
            t!("status.compacting_context"),
            elapsed,
            humanize_token_count(comp.token_estimate_before),
        ),
        Style::default().fg(palette.accent),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  {} {:>3}%",
            progress_bar(frac, 40),
            (frac.clamp(0.0, 1.0) * 100.0).round() as u64
        ),
        Style::default().fg(palette.muted),
    )]));
    lines.push(Line::from(""));
}

/// Push a single line carrying only the swimming octopus — no text. The
/// octopus alone signals the thinking phase, traveling left↔right across the
/// line (see [`octopus_swim`]) in the palette accent so it stays visible
/// against the `reasoning` role's background from [`push_message_block`] /
/// [`chat_message_bg`]. `wrap_width` bounds the travel so the octopus never
/// runs past the transcript's wrap edge.
pub(super) fn push_thinking_indicator(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    wrap_width: usize,
) {
    use std::sync::OnceLock;
    use std::time::Instant;
    // Same process-lifetime clock pattern as the spinner. The event loop
    // redraws ~every 120ms during an active turn, so the elapsed-driven travel
    // animates smoothly.
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    let (offset, frame) = octopus_swim(elapsed, wrap_width);

    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, "reasoning");
    let style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD)
        .bg(bg);
    lines.push(chat_line(
        vec![Span::styled(
            format!("{}{}", " ".repeat(offset), frame),
            style,
        )],
        Some(bg),
    ));
}

pub(super) fn push_reasoning_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    reasoning: Option<&str>,
    display_on: bool,
    expanded: bool,
) {
    if !display_on {
        return;
    }
    let Some(reasoning) = reasoning.filter(|text| !text.trim().is_empty()) else {
        return;
    };
    let all: Vec<&str> = reasoning.lines().filter(|l| !l.trim().is_empty()).collect();
    let shown = if expanded {
        all.len()
    } else {
        all.len().min(REASONING_BLOCK_CAP)
    };
    lines.push(Line::from(Span::styled(
        "· reasoning".to_string(),
        palette.muted(),
    )));
    for line in all.iter().take(shown) {
        lines.push(Line::from(Span::styled(
            format!("· {line}"),
            palette.muted(),
        )));
    }
    if all.len() > shown {
        lines.push(Line::from(Span::styled(
            format!("·   … +{} more line(s) (Ctrl+O expand)", all.len() - shown),
            palette.muted(),
        )));
    }
}

pub(super) fn push_message_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    role: &str,
    content: &str,
    width: usize,
) {
    if role == "system" {
        return;
    }

    if role == "user" {
        push_user_message_block(lines, palette, content);
        return;
    }

    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, role);
    let indent = match role {
        "assistant" => ASSISTANT_BODY_INDENT,
        "tool" => "$ ",
        "reasoning" => "· ",
        "btw" => "· ",
        _ => "",
    };
    let prose_marker = match role {
        "assistant" => Some("• "),
        _ => None,
    };

    if content.is_empty() {
        lines.push(chat_line(
            vec![Span::styled("<empty>", palette.muted().bg(bg))],
            Some(bg),
        ));
        return;
    }

    // The synthesized turn-completion "Session Summary" block (failure /
    // no-answer / partial) renders as a distinct card: a colored + bold title
    // and bold field labels, instead of flat muted markdown that buries the
    // error. The block can be the whole message OR a suffix appended after a
    // partial live reply (`{prose}\n\n{summary}`) — render the prose above it
    // normally, then the card.
    if role == "assistant"
        && let Some(start) = session_summary_block_start(content)
    {
        let (prose, summary) = content.split_at(start);
        let prose = prose.trim_end();
        if !prose.is_empty() {
            push_formatted_body_marked(
                lines,
                palette,
                prose,
                indent,
                prose_marker,
                Some(bg),
                width,
            );
        }
        push_session_summary_card(lines, palette, summary, bg, width);
        return;
    }

    push_formatted_body_marked(
        lines,
        palette,
        content,
        indent,
        prose_marker,
        Some(bg),
        width,
    );
}

/// Render a "Session Summary" card: the title in a bold attention color, then
/// each `- Label: value` row with the label bolded so the Result / Error /
/// Activity fields stand out. The `- Error:` row's value is drawn in the
/// danger color so a failure reads as a failure at a glance. Every row is
/// clipped to `width` so a narrow pane cannot wrap a value to column 0.
pub(super) fn push_session_summary_card(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    bg: Color,
    width: usize,
) {
    let mut rows = content.lines();
    let title = rows.next().unwrap_or_default();
    // Title: `✦ Session Summary` in the highlight color, bold — a notice, not
    // an error glyph (the same card also covers no-answer / partial, not only
    // failure). The failure detail below carries the red.
    lines.push(chat_line(
        clip_line_spans(
            vec![Span::styled(
                format!("{ASSISTANT_BODY_INDENT}✦ {title}"),
                Style::default()
                    .fg(palette.highlight)
                    .add_modifier(Modifier::BOLD)
                    .bg(bg),
            )],
            width,
        ),
        Some(bg),
    ));

    // Budget the value text so `indent + "- " + label + sep + value` fits the
    // pane; the final `clip_line_spans` is the hard backstop so even a row
    // whose prefix alone exceeds `width` (e.g. a long label on a 24-col pane)
    // truncates rather than wrapping to column 0 (codex P2 on #292).
    let label_lead = ASSISTANT_BODY_INDENT.width() + 2; // indent + "- "
    let error_labels = localized_in_all_locales("status.summary_error_label");
    for row in rows {
        let row = row.strip_prefix("- ").unwrap_or(row);
        // Labels use `": "` (en) or the fullwidth `"："` (zh, no space).
        let split = row
            .split_once(": ")
            .map(|(label, value)| (label, value, ": "))
            .or_else(|| {
                row.split_once('：')
                    .map(|(label, value)| (label, value, "："))
            });
        let Some((label, value, sep)) = split else {
            // A label-less row: render as a plain muted line.
            let budget = width.saturating_sub(label_lead);
            lines.push(chat_line(
                clip_line_spans(
                    vec![
                        Span::styled(format!("{ASSISTANT_BODY_INDENT}- "), palette.muted().bg(bg)),
                        Span::styled(
                            truncate_to_display_width(row, budget),
                            palette.text().bg(bg),
                        ),
                    ],
                    width,
                ),
                Some(bg),
            ));
            continue;
        };
        // The Error row's value carries the danger color; every other value is
        // the normal text color. Locale-independent label match.
        let value_style = if error_labels.iter().any(|l| l == label) {
            Style::default().fg(palette.danger).bg(bg)
        } else {
            palette.text().bg(bg)
        };
        let label_with_sep = format!("{label}{sep}");
        let value_budget = width.saturating_sub(label_lead + label_with_sep.width());
        lines.push(chat_line(
            clip_line_spans(
                vec![
                    Span::styled(format!("{ASSISTANT_BODY_INDENT}- "), palette.muted().bg(bg)),
                    Span::styled(
                        label_with_sep,
                        palette.text().add_modifier(Modifier::BOLD).bg(bg),
                    ),
                    Span::styled(truncate_to_display_width(value, value_budget), value_style),
                ],
                width,
            ),
            Some(bg),
        ));
    }
}

/// Render a (chunk of a) streaming reply. `first` controls the `• ` prose
/// marker: a reply flushed across several scrollback batches must carry the
/// bullet exactly once — on its first batch — or the transcript reads as
/// several separate replies.
pub(super) fn push_live_reply_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    width: usize,
    first: bool,
) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, "assistant");
    let marker = first.then_some("• ");
    push_formatted_body_marked(
        lines,
        palette,
        content,
        ASSISTANT_BODY_INDENT,
        marker,
        Some(bg),
        width,
    );
}

pub(super) fn push_live_reply_block_seeded(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    width: usize,
    first: bool,
    previous_reply_has_output: bool,
    previous_reply_ends_blank: bool,
) {
    if !seeded_live_reply_content_can_emit(
        content,
        previous_reply_has_output,
        previous_reply_ends_blank,
    ) {
        return;
    }
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, "assistant");
    let marker = first.then_some("• ");
    push_formatted_body_marked_seeded(
        lines,
        palette,
        content,
        ASSISTANT_BODY_INDENT,
        marker,
        Some(bg),
        width,
        previous_reply_has_output,
        previous_reply_ends_blank,
    );
}

pub(super) fn live_reply_prefix_ends_blank(palette: Palette, content: &str, width: usize) -> bool {
    if content.trim().is_empty() {
        return false;
    }
    let mut lines = Vec::new();
    push_live_reply_block(&mut lines, palette, content, width, true);
    lines.last().is_some_and(|line| line_is_blank(Some(line)))
}

pub(super) fn push_pending_messages_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    pending: &[String],
    width: usize,
) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = palette.diff_context_bg;
    lines.push(chat_line(
        vec![
            Span::styled(
                format!("{} ", t!("app.transcript.queued_label")),
                palette.title().add_modifier(Modifier::BOLD).bg(bg),
            ),
            Span::styled(
                t!("app.transcript.queued_after_turn", count = pending.len()).into_owned(),
                palette.muted().bg(bg),
            ),
        ],
        Some(bg),
    ));

    for pending in pending.iter().take(3) {
        push_formatted_body(lines, palette, pending, "› ", Some(bg), width);
    }

    if pending.len() > 3 {
        lines.push(chat_line(
            vec![Span::styled(
                format!(
                    "› {}",
                    t!("app.transcript.more_queued", count = pending.len() - 3)
                ),
                palette.muted().bg(bg),
            )],
            Some(bg),
        ));
    }
}

/// Emit the framed body rows of a fenced code block, highlighted via the
/// memoizing block cache. `complete` marks a closed fence (cacheable);
/// still-streaming blocks render uncached. The fallback style is fg-only —
/// the row background stays line-level (`chat_line`) per the no-span-bg rule.
pub(super) fn push_code_block_lines(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    indent: &'static str,
    bg: Option<Color>,
    language: &str,
    body: &[String],
    complete: bool,
) {
    if code_block_is_unified_diff(language, body) {
        retitle_last_code_block_header_as_diff(lines);
        push_unified_diff_code_block_lines(lines, palette, indent, bg, body);
        return;
    }

    let rendered = crate::highlight::highlight_block(
        language,
        body,
        palette.muted(),
        complete,
        palette.code_theme,
    );
    for row in rendered.iter() {
        let mut spans = vec![
            Span::styled(indent, style_bg(palette.border(), bg)),
            Span::styled("│ ", style_bg(palette.border(), bg)),
        ];
        spans.extend(row.iter().cloned());
        lines.push(chat_line(spans, bg));
    }
}

pub(super) fn push_unified_diff_code_block_lines(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    indent: &'static str,
    bg: Option<Color>,
    body: &[String],
) {
    for raw_line in body {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let mut spans = vec![
            Span::styled(indent, style_bg(palette.border(), bg)),
            Span::styled("│ ", style_bg(palette.border(), bg)),
        ];

        if line.starts_with("@@") {
            spans.push(Span::styled(
                line.to_string(),
                diff_hunk_style(palette).remove_modifier(Modifier::BOLD),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if line.starts_with("+++ ") {
            spans.push(Span::styled(
                line.to_string(),
                Style::default()
                    .fg(palette.success)
                    .bg(palette.success_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if line.starts_with("--- ") {
            spans.push(Span::styled(
                line.to_string(),
                Style::default()
                    .fg(palette.danger)
                    .bg(palette.danger_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if line.starts_with("diff --git") || line.starts_with("index ") {
            spans.push(Span::styled(
                line.to_string(),
                style_bg(palette.selected().add_modifier(Modifier::BOLD), bg),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if let Some(content) = line.strip_prefix('+') {
            spans.push(Span::styled("+ ", diff_line_marker_style("added", palette)));
            spans.push(Span::styled(
                content.to_string(),
                diff_line_style("added", palette),
            ));
        } else if let Some(content) = line.strip_prefix('-') {
            spans.push(Span::styled(
                "- ",
                diff_line_marker_style("removed", palette),
            ));
            spans.push(Span::styled(
                content.to_string(),
                diff_line_style("removed", palette),
            ));
        } else if let Some(content) = line.strip_prefix(' ') {
            spans.push(Span::styled(
                "  ",
                diff_line_gutter_style("context", palette),
            ));
            spans.push(Span::styled(
                content.to_string(),
                diff_line_style("context", palette),
            ));
        } else {
            spans.push(Span::styled(
                line.to_string(),
                style_bg(palette.muted(), bg),
            ));
        }

        lines.push(chat_line(spans, bg));
    }
}

pub(super) fn push_formatted_body(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    indent: &'static str,
    bg: Option<Color>,
    width: usize,
) {
    push_formatted_body_marked(lines, palette, content, indent, None, bg, width);
}

pub(super) fn push_formatted_body_marked(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    indent: &'static str,
    prose_marker: Option<&'static str>,
    bg: Option<Color>,
    width: usize,
) {
    push_formatted_body_marked_seeded(
        lines,
        palette,
        content,
        indent,
        prose_marker,
        bg,
        width,
        false,
        false,
    );
}

pub(super) fn push_formatted_body_marked_seeded(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    indent: &'static str,
    prose_marker: Option<&'static str>,
    bg: Option<Color>,
    width: usize,
    previous_reply_has_output: bool,
    previous_reply_ends_blank: bool,
) {
    // `Some((language, collected body))` while inside a fenced block: the body
    // is rendered as ONE unit when the fence closes (or at end of input for a
    // still-streaming block) so highlighting can be memoized per block — the
    // pager re-renders all history every scroll frame.
    let mut in_code: Option<(String, Vec<String>)> = None;
    let mut last_blank = previous_reply_ends_blank;
    let mut prose = Vec::new();
    let mut table = Vec::new();
    let mut checkbox_index = 1usize;
    let body_start = lines.len();
    // Hanging (all-whitespace) indents keep their blank separators truly blank
    // — no trailing spaces in immutable scrollback; glyph gutters (`· `, `$ `)
    // keep marking their blank rows.
    let separator_indent = if indent.trim().is_empty() { "" } else { indent };
    let normalized = content.trim_matches(|ch: char| ch.is_whitespace() && ch != '\n');

    for raw_line in normalized.lines() {
        let line = if in_code.is_some() {
            raw_line
        } else {
            raw_line.trim()
        };
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            if let Some((language, body)) = in_code.take() {
                push_code_block_lines(lines, palette, indent, bg, &language, &body, true);
                lines.push(chat_line(
                    vec![
                        Span::styled(indent, style_bg(palette.border(), bg)),
                        Span::styled("└─", style_bg(palette.border(), bg)),
                    ],
                    bg,
                ));
            } else {
                let language = rest
                    .split_whitespace()
                    .next()
                    .filter(|language| !language.is_empty())
                    .unwrap_or("code")
                    .to_string();
                lines.push(chat_line(
                    vec![
                        Span::styled(indent, style_bg(palette.border(), bg)),
                        Span::styled("┌─ ", style_bg(palette.border(), bg)),
                        Span::styled(language.clone(), style_bg(palette.selected(), bg)),
                    ],
                    bg,
                ));
                in_code = Some((language, Vec::new()));
            }
            last_blank = false;
            continue;
        }

        if let Some((_, body)) = in_code.as_mut() {
            body.push(truncate_terminal_line(line, CODE_BLOCK_LINE_LIMIT));
            last_blank = false;
            continue;
        }

        if line.is_empty() {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            checkbox_index = 1;
            if !last_blank && (previous_reply_has_output || !lines.is_empty()) {
                lines.push(chat_line(
                    vec![Span::styled(
                        separator_indent,
                        style_bg(palette.border(), bg),
                    )],
                    bg,
                ));
                last_blank = true;
            }
            continue;
        }
        last_blank = false;

        if let Some(command) = shell_command_from_line(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            push_command_row(lines, palette, indent, command);
            continue;
        }

        if markdown_table_separator(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            continue;
        }

        if let Some(cells) = markdown_table_cells(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            table.push(cells.into_iter().map(str::to_owned).collect());
            continue;
        }

        if markdown_hr(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            let rule_width = width.saturating_sub(indent.chars().count()).clamp(1, 40);
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled("─".repeat(rule_width), style_bg(palette.muted(), bg)),
                ],
                bg,
            ));
            continue;
        }

        if let Some(heading) = markdown_heading(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            let mut spans = vec![Span::styled(indent, style_bg(palette.border(), bg))];
            spans.extend(inline_markdown_spans(
                heading,
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.selected(), bg),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if let Some((_checked, text)) = markdown_checkbox(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            let mut spans = vec![
                Span::styled(indent, style_bg(palette.border(), bg)),
                Span::styled(
                    format!("{checkbox_index}. "),
                    style_bg(palette.selected(), bg),
                ),
            ];
            checkbox_index += 1;
            spans.extend(inline_markdown_spans(
                text,
                style_bg(palette.text(), bg),
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.selected(), bg),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if let Some(text) = markdown_bullet(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            let mut spans = vec![
                Span::styled(indent, style_bg(palette.border(), bg)),
                Span::styled("- ", style_bg(palette.selected(), bg)),
            ];
            spans.extend(inline_markdown_spans(
                text,
                style_bg(palette.text(), bg),
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.selected(), bg),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if let Some((number, text)) = markdown_numbered(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            let mut spans = vec![
                Span::styled(indent, style_bg(palette.border(), bg)),
                Span::styled(format!("{number}. "), style_bg(palette.selected(), bg)),
            ];
            spans.extend(inline_markdown_spans(
                text,
                style_bg(palette.text(), bg),
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.selected(), bg),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        if let Some(text) = markdown_blockquote(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            // Render as a quoted line with a left bar + muted italics, instead of
            // leaking the literal `>` marker into prose.
            let mut spans = vec![
                Span::styled(indent, style_bg(palette.border(), bg)),
                Span::styled("▌ ", style_bg(palette.muted(), bg)),
            ];
            spans.extend(inline_markdown_spans(
                text,
                style_bg(palette.muted().add_modifier(Modifier::ITALIC), bg),
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.selected(), bg),
            ));
            lines.push(chat_line(spans, bg));
            continue;
        }

        flush_markdown_table(lines, palette, &mut table, indent, bg, width);
        prose.push(line.to_string());
    }

    if let Some((language, body)) = in_code.take() {
        // Fence still open at end of input (streaming): render it too so the
        // live tail shows the in-flight code — uncached, the body grows every
        // frame.
        push_code_block_lines(lines, palette, indent, bg, &language, &body, false);
    }
    flush_prose_paragraph(lines, palette, &mut prose, indent, bg);
    flush_markdown_table(lines, palette, &mut table, indent, bg, width);
    finish_hanging_body(lines, body_start, palette, indent, prose_marker, bg, width);
}

pub(super) fn flush_prose_paragraph(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    prose: &mut Vec<String>,
    indent: &'static str,
    bg: Option<Color>,
) {
    if prose.is_empty() {
        return;
    }

    let paragraph = prose.join(" ");
    let mut spans = vec![Span::styled(indent, style_bg(palette.border(), bg))];
    spans.extend(inline_markdown_spans(
        &paragraph,
        style_bg(palette.text(), bg),
        style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
        style_bg(palette.selected(), bg),
    ));
    lines.push(chat_line(spans, bg));
    prose.clear();
}

pub(super) fn flush_markdown_table(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    table: &mut Vec<Vec<String>>,
    indent: &'static str,
    bg: Option<Color>,
    width: usize,
) {
    if table.is_empty() {
        return;
    }
    let col_count = table.iter().map(Vec::len).max().unwrap_or(0);
    if col_count == 0 {
        table.clear();
        return;
    }

    // Natural (display-width) column sizes.
    let mut widths = vec![0usize; col_count];
    for row in table.iter() {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(table_cell_width(cell));
        }
    }

    // Fit within the pane: a bordered row is `│ c1 │ c2 │ … │`, so the
    // borders/padding cost 3*cols + 1 columns on top of the cell content.
    // Shrink the widest columns (cells get ellipsized) so the grid never wraps.
    let overhead = 3 * col_count + 1;
    let budget = width.saturating_sub(indent.width() + overhead);
    let mut current: usize = widths.iter().sum();
    while current > budget {
        let max_w = widths.iter().copied().max().unwrap_or(0);
        if max_w <= MIN_TABLE_COL {
            break;
        }
        if let Some(idx) = widths.iter().position(|w| *w == max_w) {
            widths[idx] -= 1;
            current -= 1;
        } else {
            break;
        }
    }

    let border = style_bg(palette.border(), bg);
    let bold = style_bg(palette.title().add_modifier(Modifier::BOLD), bg);
    let code = style_bg(palette.selected(), bg);
    let has_header = table.len() > 1;

    lines.push(table_border_line(
        indent, &widths, '┌', '┬', '┐', border, bg, width,
    ));
    for (row_idx, row) in table.iter().enumerate() {
        let header = has_header && row_idx == 0;
        let cell_style = if header {
            bold
        } else {
            style_bg(palette.text(), bg)
        };
        let mut spans = vec![Span::styled(indent, border), Span::styled("│", border)];
        for (idx, w) in widths.iter().enumerate() {
            let cell = row.get(idx).map(String::as_str).unwrap_or("");
            let (cell_spans, used) = fit_cell_spans(cell, *w, cell_style, bold, code);
            spans.push(Span::styled(" ", cell_style));
            spans.extend(cell_spans);
            spans.push(Span::styled(
                " ".repeat(w.saturating_sub(used) + 1),
                cell_style,
            ));
            spans.push(Span::styled("│", border));
        }
        // Last-resort clip: when even minimum-width columns plus borders exceed
        // the pane (e.g. a many-column table in a narrow transcript), hard-cut
        // the row so ratatui never wraps it into a broken grid.
        lines.push(chat_line(clip_line_spans(spans, width), bg));
        if header {
            lines.push(table_border_line(
                indent, &widths, '├', '┼', '┤', border, bg, width,
            ));
        }
    }
    lines.push(table_border_line(
        indent, &widths, '└', '┴', '┘', border, bg, width,
    ));
    table.clear();
}

pub(super) fn push_command_row(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    indent: &'static str,
    command: &str,
) {
    lines.push(Line::from(vec![
        Span::styled(indent, palette.border().bg(palette.surface)),
        Span::styled(
            format!("▸ {}  ", t!("app.tool.command_label")),
            palette.selected().bg(palette.surface),
        ),
        Span::styled("$ ", palette.selected().bg(palette.surface)),
        Span::styled(command.to_string(), palette.text().bg(palette.surface)),
    ]));
}

pub(super) fn push_inline_approval_card(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    approval: &ApprovalModalState,
) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled(
            t!("app.approval.title").to_string(),
            palette.title().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", t!("app.approval.inline")), palette.muted()),
    ]));
    for line in approval_modal_lines(approval, palette) {
        push_prefixed_line(lines, "    ", palette.muted(), line);
    }
    for action in approval_action_labels(approval) {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(action, palette.selected()),
        ]));
    }
    if approval.diff_preview_id().is_some() {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                t!("app.approval.action_diff").to_string(),
                palette.selected(),
            ),
        ]));
    }
}

/// UPCR-2026-023: render the pending AskUserQuestion picker inline, mirroring
/// [`push_inline_approval_card`]. Shows the mandatory `title`/`body` fallback,
/// the active structured question (1–4), each option as a radio/checkbox row,
/// and the always-present free-text "Other" row.
/// The `/btw` aside card: question echo, then `✽ Answering…` while the
/// out-of-band answer is in flight, then the answer as a dim `·` block (or a
/// failure line). Live-pane only — the aside is ephemeral by design.
pub(super) fn push_btw_aside_card(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    aside: &crate::model::BtwAside,
    width: usize,
) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled(format!("/btw {}", aside.question), palette.selected()),
    ]));
    match &aside.state {
        crate::model::BtwAsideState::Answering => {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(format!("✽ {}", t!("app.btw.answering")), palette.muted()),
            ]));
        }
        crate::model::BtwAsideState::Answered(answer) => {
            push_message_block(lines, palette, "btw", answer, width);
        }
        crate::model::BtwAsideState::Failed(message) => {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    format!("✽ {}", t!("app.btw.failed", error = message.clone())),
                    palette.muted(),
                ),
            ]));
        }
    }
}

pub(super) fn push_inline_user_question_card(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    picker: &UserQuestionPickerState,
    width: usize,
) {
    lines.push(Line::from(""));
    let header = if picker.questions.len() > 1 {
        t!(
            "app.question.header_multi",
            n = picker.active + 1,
            total = picker.questions.len()
        )
        .to_string()
    } else {
        t!("app.question.header_single").to_string()
    };
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled(
            t!("app.question.card_title").to_string(),
            palette.title().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {header}"), palette.muted()),
    ]));

    // Mandatory generic fallback text keeps the card actionable even when the
    // structured `questions` field is empty or unparsed.
    if !picker.title.is_empty() {
        push_wrapped_card_text(
            lines,
            vec![Span::styled("    ", palette.muted())],
            "    ".to_string(),
            &picker.title,
            palette.text(),
            width.saturating_sub(4).max(1),
        );
    }
    if !picker.body.is_empty() {
        push_wrapped_card_text(
            lines,
            vec![Span::styled("    ", palette.muted())],
            "    ".to_string(),
            &picker.body,
            palette.muted(),
            width.saturating_sub(4).max(1),
        );
    }

    match picker.active_question() {
        Some(entry) => push_user_question_entry(lines, palette, entry, width),
        None => {
            // Garbled / protocol-violation fallback: no structured questions, so
            // there is nothing answerable. Render the title/body as an
            // INFORMATIONAL card only — do NOT offer a "Type your answer"
            // affordance, since any input would be discarded and a submit cannot
            // form a valid (count-matched) respond (DO-NOT-SHIP #2). The card
            // stays dismissible (Esc) and recoverable (Ctrl+R/Alt+a).
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(t!("app.question.no_options").to_string(), palette.muted()),
            ]));
        }
    }

    for action in user_question_action_labels(picker) {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(action, palette.selected()),
        ]));
    }
}

pub(super) fn push_user_question_entry(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    entry: &UserQuestionEntry,
    width: usize,
) {
    if !entry.header.is_empty() {
        push_prefixed_line(
            lines,
            "    ",
            palette.muted(),
            Line::from(Span::styled(
                entry.header.clone(),
                palette.title().add_modifier(Modifier::BOLD),
            )),
        );
    }
    push_wrapped_card_text(
        lines,
        vec![Span::styled("    ", palette.muted())],
        "    ".to_string(),
        &entry.question,
        palette.text(),
        width.saturating_sub(4).max(1),
    );

    for (idx, option) in entry.options.iter().enumerate() {
        let highlighted = idx == entry.cursor;
        let checked = entry.option_selected.get(idx).copied().unwrap_or(false);
        let mut text = option.label.clone();
        if !option.description.is_empty() {
            text.push_str(" — ");
            text.push_str(&option.description);
        }
        push_user_question_option_row(
            lines,
            palette,
            highlighted,
            checked,
            entry.multi_select,
            &text,
            width,
        );
    }

    // Always-present free-text "Other" row (server forces allow_free_text).
    let other_highlighted = entry.cursor >= entry.free_text_row();
    let editing = entry.editing_free_text;
    let has_text = !entry.free_text.trim().is_empty();
    let body = if entry.free_text.is_empty() {
        if editing {
            t!("app.question.type_answer").into_owned()
        } else {
            t!("app.question.free_text_row").to_string()
        }
    } else {
        entry.free_text.clone()
    };
    let other_prefix = t!("app.question.other_prefix").to_string();
    let text = format!("{other_prefix}: {body}");
    // "Other" counts as chosen when it has text (or is being edited).
    push_user_question_option_row(
        lines,
        palette,
        other_highlighted,
        has_text || editing,
        entry.multi_select,
        &text,
        width,
    );
}

/// Render one selectable option row (or the free-text "Other" row) for the
/// AskUserQuestion picker. A prominent left accent bar marks the highlighted
/// row; a filled/hollow marker (● / ○ for single-select, ▣ / ▢ for
/// multi-select) shows what's chosen. The label is bold+highlighted on the
/// active row, accent-coloured when chosen-but-not-active, plain otherwise —
/// so the current choice reads at a glance without arrow-hunting.
pub(super) fn push_user_question_option_row(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    highlighted: bool,
    chosen: bool,
    multi_select: bool,
    text: &str,
    width: usize,
) {
    let (bar, bar_style) = if highlighted {
        ("▌ ", palette.title().add_modifier(Modifier::BOLD))
    } else {
        ("  ", palette.muted())
    };
    let marker = match (multi_select, chosen) {
        (true, true) => "▣ ",
        (true, false) => "▢ ",
        (false, true) => "● ",
        (false, false) => "○ ",
    };
    let marker_style = if chosen {
        palette.title()
    } else {
        palette.muted()
    };
    let label_style = if highlighted {
        palette.selected().add_modifier(Modifier::BOLD)
    } else if chosen {
        palette.title()
    } else {
        palette.text()
    };
    // Budget = width minus the 4-space indent and the bar + marker prefixes
    // (2 cols each). Long labels WRAP with a hanging indent aligned under the
    // label start — truncation hid the tail of real questions (user report).
    let budget = width.saturating_sub(8).max(1);
    push_wrapped_card_text(
        lines,
        vec![
            Span::styled("    ", palette.muted()),
            Span::styled(bar, bar_style),
            Span::styled(marker, marker_style),
        ],
        "        ".to_string(),
        text,
        label_style,
        budget,
    );
}

pub(super) fn push_prefixed_line(
    lines: &mut Vec<Line<'static>>,
    prefix: &'static str,
    prefix_style: Style,
    mut line: Line<'static>,
) {
    let mut spans = vec![Span::styled(prefix, prefix_style)];
    spans.append(&mut line.spans);
    lines.push(Line::from(spans));
}

/// Word-wrap `text` into display-width-budgeted rows (unicode-width, so CJK
/// double-width glyphs count as 2 — the `fit_card_text` lesson). Breaks on
/// spaces; a single word wider than the budget hard-breaks mid-word rather
/// than overflowing. Never returns an empty vec (empty text → one empty row).
pub(super) fn wrap_display_width(text: &str, budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    let mut row_w = 0usize;
    for word in text.split(' ') {
        let word_w = word.width();
        let sep_w = if row.is_empty() { 0 } else { 1 };
        if row_w + sep_w + word_w <= budget {
            if sep_w == 1 {
                row.push(' ');
            }
            row.push_str(word);
            row_w += sep_w + word_w;
            continue;
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        if word_w <= budget {
            row.push_str(word);
            row_w = word_w;
        } else {
            // Hard-break an over-budget word by display columns.
            let mut piece = String::new();
            let mut piece_w = 0usize;
            for ch in word.chars() {
                let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if piece_w + ch_w > budget && !piece.is_empty() {
                    rows.push(std::mem::take(&mut piece));
                    piece_w = 0;
                }
                piece.push(ch);
                piece_w += ch_w;
            }
            row = piece;
            row_w = piece_w;
        }
    }
    rows.push(row);
    rows
}

/// Push `text` wrapped to the card budget: every row carries the 4-space card
/// indent; rows after the first get `extra_indent` more columns so
/// continuations align under the first row's content (option-marker hanging
/// indent). Replaces the old single-line `fit_card_text` truncation on the
/// question card — questions and options must WRAP, not clip (user report).
pub(super) fn push_wrapped_card_text(
    lines: &mut Vec<Line<'static>>,
    first_prefix_spans: Vec<Span<'static>>,
    continuation_prefix: String,
    text: &str,
    style: Style,
    budget: usize,
) {
    for (idx, row) in wrap_display_width(text, budget).into_iter().enumerate() {
        if idx == 0 {
            let mut spans = first_prefix_spans.clone();
            spans.push(Span::styled(row, style));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![
                Span::styled(continuation_prefix.clone(), Style::default()),
                Span::styled(row, style),
            ]));
        }
    }
}

pub(super) fn push_activity_section_with_finalization(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    live_finalization: Option<&LiveTurnFinalization>,
    wrap_width: usize,
) {
    let mut flow_activity = flow_activity_items(app);
    if let Some(finalization) = active_live_finalization(app, live_finalization) {
        flow_activity = flow_activity
            .into_iter()
            .enumerate()
            .filter(|(idx, item)| {
                !finalization
                    .activity_flushed_keys
                    .contains(&activity_finalization_key(item, *idx))
            })
            .map(|(_, item)| item)
            .collect();
    }
    if flow_activity.is_empty() {
        return;
    }
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    let shown_limit = if app.expanded_tool_outputs { 12 } else { 3 };
    let recent = flow_activity
        .iter()
        .rev()
        .take(shown_limit)
        .rev()
        .copied()
        .collect::<Vec<_>>();
    let pending_continuations = active_session_pending_continuations(app);
    // The header counts tally the FULL per-turn set (from `flow_activity`), not
    // the display-capped `group` — so a chip header agrees with the sibling
    // "... +N older action(s)" footer below.
    let full_group = |turn: Option<&octos_core::ui_protocol::TurnId>| -> Vec<&ActivityItem> {
        flow_activity
            .iter()
            .copied()
            .filter(|item| item.turn_id.as_ref() == turn)
            .collect()
    };
    let mut group: Vec<&ActivityItem> = Vec::new();
    let mut last_turn: Option<&octos_core::ui_protocol::TurnId> = None;
    for item in recent.iter().copied() {
        let turn_id = item.turn_id.as_ref();
        if last_turn != turn_id {
            if !group.is_empty() {
                push_agent_task_group(
                    lines,
                    palette,
                    last_turn,
                    &full_group(last_turn),
                    &group,
                    &running_subagent_titles_for_chip(app, last_turn),
                    pending_continuations,
                    is_active_group(app, last_turn),
                    app.expanded_tool_outputs,
                    true,
                    wrap_width,
                );
                group.clear();
            }
            last_turn = turn_id;
        }
        group.push(item);
    }
    if !group.is_empty() {
        push_agent_task_group(
            lines,
            palette,
            last_turn,
            &full_group(last_turn),
            &group,
            &running_subagent_titles_for_chip(app, last_turn),
            pending_continuations,
            is_active_group(app, last_turn),
            app.expanded_tool_outputs,
            true,
            wrap_width,
        );
    }
    if flow_activity.len() > recent.len() {
        lines.push(Line::from(vec![
            Span::styled("     ", palette.muted()),
            Span::styled(
                t!(
                    "app.activity.older_actions",
                    count = flow_activity.len() - recent.len()
                )
                .to_string(),
                palette.muted(),
            ),
        ]));
    }
}

pub(super) fn push_turn_activity_log_section(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    log: &TurnActivityLog,
    app: &AppState,
    collapse_settled: bool,
    finalized: bool,
    wrap_width: usize,
) {
    let summary = app.turn_summary_for(&log.turn_id);
    // A FINALIZED render targets IMMUTABLE scrollback, so it must record only
    // the turn's TERMINAL activity — a still-running item would freeze an
    // in-progress "Orchestrating… (N active)" chip there and strand a second
    // copy above the live chip (the same failure `push_finalized_activity_items_section`
    // guards). #342 stripped the volatile sub-agent titles here but still fed
    // the running item into the header counts. Drop running items on the
    // finalized path; the live/overlay render (`finalized == false`) keeps them.
    let items: Vec<&ActivityItem> = if finalized {
        log.items
            .iter()
            .filter(|item| !is_running_activity(item))
            .collect()
    } else {
        log.items.iter().collect()
    };
    // A tool-less turn (or a finalized turn whose only items are still running)
    // carries only a summary; still render its report. Nothing at all to show
    // only when both are absent.
    if items.is_empty() && summary.is_none() {
        return;
    }
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    if !items.is_empty() {
        let shown_limit = if app.expanded_tool_outputs { 12 } else { 3 };
        // Full uncapped set (header counts + footer tally both derive from this
        // via `task_group_counts`, so they cannot diverge).
        let full = items;
        let shown = full
            .iter()
            .rev()
            .take(shown_limit)
            .rev()
            .copied()
            .collect::<Vec<_>>();
        // A FINALIZED render targets IMMUTABLE scrollback, so it must record the
        // turn's OWN terminal outcome — never the volatile cross-turn sub-agent
        // status. A settled turn whose spawned sub-agents are still running would
        // otherwise be flushed as "Orchestrating… N running" and STRAND frozen
        // there (append-only scrollback can't be reclaimed): it keeps lying
        // "N sub-agent(s) running" after the sub-agent finished, and a menu-toggle
        // reflush strands a second such copy above the live chip (validated on
        // mini5). The live aggregate chip at the bottom of the viewport carries
        // the current sub-agent status; scrollback keeps only the parent turn's
        // terminal state.
        let live_subagent_titles = if finalized {
            Vec::new()
        } else {
            running_subagent_titles_for_chip(app, Some(&log.turn_id))
        };
        let pending_continuations = if finalized {
            0
        } else {
            active_session_pending_continuations(app)
        };
        let is_active = !finalized && is_active_group(app, Some(&log.turn_id));
        push_agent_task_group(
            lines,
            palette,
            Some(&log.turn_id),
            &full,
            &shown,
            &live_subagent_titles,
            pending_continuations,
            is_active,
            app.expanded_tool_outputs,
            collapse_settled,
            wrap_width,
        );
        if full.len() > shown.len() {
            let hidden = full.len() - shown.len();
            let (_, completed, active, _) = task_group_counts(&full);
            lines.push(Line::from(vec![
                Span::styled("     ", palette.muted()),
                Span::styled(
                    t!(
                        "app.activity.more_completed_active",
                        hidden = hidden,
                        completed = completed,
                        active = active
                    )
                    .into_owned(),
                    palette.muted(),
                ),
            ]));
        }
        if app.diff_preview.active && app.diff_preview.turn_id.as_ref() == Some(&log.turn_id) {
            push_inline_diff_preview(
                lines,
                palette,
                &app.diff_preview,
                app.expanded_tool_outputs,
                wrap_width,
            );
        }
    }
    push_turn_summary_line(lines, palette, app, &log.turn_id);
}

pub(super) fn push_turn_activity_log_section_unflushed(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    log: &TurnActivityLog,
    app: &AppState,
    coverage: &LiveTurnFinalization,
    wrap_width: usize,
) {
    let items = log
        .items
        .iter()
        .enumerate()
        .filter(|(idx, item)| {
            !coverage
                .activity_flushed_keys
                .contains(&activity_finalization_key(item, *idx))
        })
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    push_finalized_activity_items_section(
        lines,
        palette,
        app,
        Some(&log.turn_id),
        &items,
        wrap_width,
    );
    // The settling flush routes a still-covered log through this path, so emit
    // the committed turn summary here too (a no-op until the turn completes).
    push_turn_summary_line(lines, palette, app, &log.turn_id);
}

/// Emit the committed per-turn status report line for `turn_id`, if one was
/// captured. Shared by the flushed and unflushed (still-covered) activity-log
/// section renderers so an orchestrated turn — whose log is still covered by the
/// live tail at the settling flush — still gets its report in scrollback. Keeps
/// one blank separator so the line reads as the turn's footer.
pub(super) fn push_turn_summary_line(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    turn_id: &octos_core::ui_protocol::TurnId,
) {
    let Some(summary) = app.turn_summary_for(turn_id) else {
        return;
    };
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        turn_summary_text(summary),
        palette.muted(),
    )));
}

pub(super) fn push_finalized_activity_items_section(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    turn_id: Option<&octos_core::ui_protocol::TurnId>,
    items: &[&ActivityItem],
    wrap_width: usize,
) {
    // This section targets IMMUTABLE scrollback, which can never be reclaimed
    // (`CSI 2J` clears only the visible screen). So it must record only
    // TERMINAL items — never one that is still RUNNING. A running item flushed
    // here makes the chip header read "Orchestrating… (N active)" with its
    // spinner frozen at flush time, and that copy strands one frame behind the
    // LIVE aggregate chip below: two "Orchestrating" lines for the same turn,
    // different spinner glyphs (the third face of the "duplicated
    // orchestrating" bug, after #339/#342). It reaches here via the covered
    // late-activity flush (`finalized_late_activity_lines_for_coverages` /
    // `push_turn_activity_log_section_unflushed`), whose UNFLUSHED item set is
    // NOT pre-filtered by run state. Drop running items: they stay in the live
    // tail (repainted every frame) and are flushed only once they settle.
    // (`finalized_live_turn_lines_between` already pre-filters to non-running
    // items, so this is a no-op on that path.)
    let terminal_items: Vec<&ActivityItem> = items
        .iter()
        .copied()
        .filter(|item| !is_running_activity(item))
        .collect();
    if terminal_items.is_empty() {
        return;
    }
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    push_agent_task_group(
        lines,
        palette,
        turn_id,
        &terminal_items,
        &terminal_items,
        &[],
        0,
        false,
        app.expanded_tool_outputs,
        // Scrollback flush path: the archive never collapses.
        false,
        wrap_width,
    );
}

/// Render an agent-task-group chip: a header (title + count metadata) plus the
/// display-capped `items` as children.
///
/// `full_items` is the UNCAPPED turn activity set used for the HEADER counts;
/// `items` is the display-capped slice (last N rows) actually rendered as
/// children. The header tallies `full_items` via [`task_group_counts`] — the
/// SAME helper the sibling "... +N older" footer uses — so the header and
/// footer numbers cannot diverge (render-cap bug: a 66-action turn previously
/// read "3 action(s) · 3 completed" because the header counted the cap).
#[allow(clippy::too_many_arguments)]
pub(super) fn push_agent_task_group(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    turn_id: Option<&octos_core::ui_protocol::TurnId>,
    full_items: &[&ActivityItem],
    items: &[&ActivityItem],
    subagent_titles: &[String],
    pending_continuations: u32,
    is_active_group: bool,
    expanded: bool,
    collapse_settled: bool,
    wrap_width: usize,
) {
    let active_subagents = subagent_titles.len();
    if items.is_empty() && subagent_titles.is_empty() {
        return;
    }
    // Header counts tally the FULL turn set, not the display-capped `items`.
    let (total, completed, active, failed) = task_group_counts(full_items);
    // `spawn` returns immediately, so the parent's tool-call rollup can be all
    // "completed" while the sub-agents it launched are still running (tracked
    // separately in `session.tasks`). Treat the turn as still orchestrating
    // while any of its sub-agents are live, so the chip never says "completed"
    // with work outstanding.
    let in_progress = active > 0 || active_subagents > 0;
    let title = agent_task_group_title(in_progress, failed, pending_continuations, is_active_group);
    let mut metadata = vec![t!("app.activity.action_count", count = total).into_owned()];
    if active > 0 {
        metadata.push(t!("app.activity.active_count", count = active).into_owned());
    }
    if completed > 0 {
        metadata.push(t!("app.activity.completed_count", count = completed).into_owned());
    }
    if failed > 0 {
        metadata.push(t!("app.activity.failed_count", count = failed).into_owned());
    }
    if active_subagents > 0 {
        metadata.push(t!("app.activity.subagents_running", count = active_subagents).into_owned());
    }
    if let Some(turn_id) = turn_id {
        metadata.push(
            t!(
                "app.activity.turn_label",
                id = short_id(&turn_id.0.to_string())
            )
            .into_owned(),
        );
    }

    // While orchestrating, show the animated octopus "tentacle pulse" spinner;
    // a settled chip keeps the static bullet. Both are 1 col wide so the title
    // stays aligned whether running or done.
    let icon = if in_progress { spinner_frame() } else { "•" };
    // Role-contrast: runtime/tool activity is the LOW tier of the transcript's
    // visual hierarchy — muted header (bold kept for grouping), status icons
    // keep their state colors (spinner/✓/✗ carry information).
    let spans = vec![
        Span::styled(format!("{icon} "), palette.selected()),
        Span::styled(title, palette.muted().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" ({})", metadata.join(" · ")), palette.muted()),
    ];
    lines.push(Line::from(spans));

    // Settled groups collapse to their one-line summary in the repainting
    // views (Ctrl+O expands); the scrollback flush path never collapses (the
    // archive stays complete). A group is NOT settled while it is the active
    // turn OR while it is still in progress on its own — a finished turn with
    // sub-agents still running keeps its spinner AND its children visible.
    if collapse_settled && !is_active_group && !in_progress && !expanded {
        return;
    }

    for (idx, item) in items.iter().enumerate() {
        push_agent_task_child(
            lines,
            palette,
            item,
            idx == 0,
            expanded,
            wrap_width,
            !collapse_settled,
        );
    }

    // List this turn's running sub-agents (from session.tasks, attributed by
    // turn) as children, so their live progress shows under THIS chip instead
    // of forming a separate turn-less "Orchestrating" chip (mini5 soak: folds
    // the phantom second chip into the orchestrating turn's chip).
    for (idx, title) in subagent_titles.iter().enumerate() {
        let first = items.is_empty() && idx == 0;
        let prefix = if first { "  ⎿  " } else { "     " };
        // Clip to `wrap_width` like every other child row so a long sub-agent
        // title cannot overflow and wrap to column 0.
        let spans = clip_line_spans(
            vec![
                Span::styled(prefix, palette.border()),
                Span::styled("◻ ", palette.selected()),
                Span::styled(title.clone(), palette.muted()),
                Span::styled(format!("  {}", t!("app.activity.running")), palette.muted()),
            ],
            wrap_width,
        );
        lines.push(Line::from(spans));
    }
}

/// Claude-Code-style tool-card header: `  ⏺ Bash(cmd)` (indented as a group
/// child). The invocation (shell command, spawn task, file path, …) renders
/// in parens with raw JSON and the call-id stripped; multi-line commands
/// indent to align under `(`. Every emitted line is budgeted + clipped to
/// `wrap_width` display columns so it can never overflow and wrap to column 0
/// (the indent-not-honored bug).
pub(super) fn push_tool_card_header(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    wrap_width: usize,
) {
    let (bullet, bullet_style) = tool_card_bullet(item, palette);
    let name = tool_display_name(&item.title);
    let indent = TOOL_CARD_CHILD_INDENT;
    let indent_cols = indent.width();
    let duration = item
        .duration_ms
        .map(|ms| format!("  {}", format_duration_ms(ms)))
        .unwrap_or_default();

    let Some(invocation) = tool_invocation_text(item).filter(|text| !text.trim().is_empty()) else {
        // No arguments to show: `  ⏺ Bash`.
        let mut spans = vec![
            Span::styled(indent, palette.muted()),
            Span::styled(bullet, bullet_style),
            Span::styled(" ", palette.muted()),
            Span::styled(name, palette.muted()),
        ];
        if !duration.is_empty() {
            spans.push(Span::styled(duration, palette.muted()));
        }
        lines.push(Line::from(clip_line_spans(spans, wrap_width)));
        return;
    };

    // Shell-family invocations keep the `$ ` prompt inside the parens —
    // `  ⏺ Bash($ cargo test)` — the command-row marker #276 established; the
    // prompt is part of the budgeted text so the width math stays exact.
    let invocation = if is_shell_family_tool(&item.title) {
        format!("$ {invocation}")
    } else {
        invocation
    };

    // Continuation lines align under the first char after `(`, INCLUDING the
    // leading child indent so multi-line commands stay under the card.
    let cont_indent =
        " ".repeat(indent_cols + bullet.chars().count() + 1 + name.chars().count() + 1);
    let cmd_lines: Vec<&str> = invocation.lines().collect();
    let max_lines = 10usize;
    let shown = cmd_lines.len().min(max_lines).max(1);
    let clipped = cmd_lines.len() > shown;
    // Budget the command text so indent + lead-in + text + `)` + duration fit
    // within `wrap_width` (unicode-width, so CJK commands stay exact).
    let lead_width = cont_indent.width();
    let text_budget = wrap_width
        .saturating_sub(lead_width)
        .saturating_sub(duration.width() + 2)
        .max(8);

    for idx in 0..shown {
        let raw = cmd_lines.get(idx).copied().unwrap_or_default();
        let last = idx + 1 == shown;
        let mut text = truncate_to_display_width(raw, text_budget);
        if last {
            if clipped {
                text.push('…');
            }
            text.push(')');
        }
        let mut spans = Vec::new();
        if idx == 0 {
            spans.push(Span::styled(indent, palette.muted()));
            spans.push(Span::styled(bullet.clone(), bullet_style));
            spans.push(Span::styled(" ", palette.muted()));
            spans.push(Span::styled(format!("{name}("), palette.muted()));
        } else {
            spans.push(Span::styled(cont_indent.clone(), palette.muted()));
        }
        spans.push(Span::styled(text, palette.muted()));
        if last && !duration.is_empty() {
            spans.push(Span::styled(duration.clone(), palette.muted()));
        }
        lines.push(Line::from(clip_line_spans(spans, wrap_width)));
    }
}

pub(super) fn push_agent_task_child(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    first: bool,
    expanded: bool,
    wrap_width: usize,
    in_scrollback: bool,
) {
    // Tool calls render as Claude-Code-style `⏺ Tool(arg)` cards; other
    // activity rows (file mutations, progress) keep the `⎿ ✓ …` tree form.
    if item.kind == ActivityKind::Tool {
        push_tool_card_header(lines, palette, item, wrap_width);
        push_compact_tool_preview(lines, palette, item, expanded, wrap_width, in_scrollback);
        return;
    }

    let (icon, icon_style) = activity_status_icon(item, palette);
    let prefix = if first { "  ⎿  " } else { "     " };
    // Display width consumed by the fixed lead-in (prefix + icon + one space);
    // the content spans get the remaining budget so the whole row fits within
    // `wrap_width` and ratatui never wraps it to column 0 (the indent-not-honored
    // bug). Measured with unicode-width so CJK/emoji prefixes stay exact.
    let lead_width = prefix.width() + icon.width() + 1;
    let content_budget = wrap_width.saturating_sub(lead_width);
    let mut spans = vec![
        Span::styled(prefix, palette.border()),
        Span::styled(icon, icon_style),
        Span::styled(" ", palette.muted()),
    ];
    spans.extend(compact_activity_spans(item, palette, content_budget));
    // Backstop: hard-clip the assembled row to `wrap_width` display columns so
    // no child line can EVER exceed the transcript width (and wrap to column 0),
    // even if a branch left an unbudgeted variable part (e.g. a long
    // recovery-suggestion status). A budgeted row already fits, so this is a
    // no-op there; it only bites pathological cases.
    let spans = clip_line_spans(spans, wrap_width);
    lines.push(Line::from(spans));
}

pub(super) fn push_compact_metadata_spans(
    spans: &mut Vec<Span<'static>>,
    palette: Palette,
    item: &ActivityItem,
) {
    if let Some(duration_ms) = item.duration_ms {
        spans.push(Span::styled(
            format!("  {}", format_duration_ms(duration_ms)),
            palette.muted(),
        ));
    }
    // No `call <tool_call_id>` span: #267 established "no call-id" for CC-style
    // activity cards. The `tool_call_id` FIELD is retained (used for keying /
    // reconciliation); only the noisy display suffix is dropped.
}

pub(super) fn push_compact_tool_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    expanded: bool,
    wrap_width: usize,
    in_scrollback: bool,
) {
    // The preview prefix `    ⎿ ` is 6 display columns — the 2-col child
    // indent of the tool card (`TOOL_CARD_CHILD_INDENT`) plus `  ⎿ ` — so the
    // output nests under the indented `⏺` bullet. Budget the content so a
    // preview line fits within `wrap_width` and never wraps to column 0.
    const PREVIEW_PREFIX_COLS: usize = 6;
    let preview_budget = wrap_width.saturating_sub(PREVIEW_PREFIX_COLS);
    let Some(output_preview) = item
        .output_preview
        .as_deref()
        .filter(|output| !output.trim().is_empty())
    else {
        return;
    };
    let meaningful = meaningful_output_lines(output_preview);
    let preview_lines = if meaningful.is_empty() {
        output_preview.lines().collect::<Vec<_>>()
    } else {
        meaningful
    };
    let total = preview_lines.len();
    // Frozen scrollback can't be repainted, so the Ctrl+O affordance is dead
    // there: render the full output and drop the hint. Only the live viewport
    // (which the toggle genuinely repaints) collapses to a preview.
    let line_limit = if in_scrollback {
        total
    } else if expanded {
        EXPANDED_TOOL_PREVIEW_LINES
    } else {
        COLLAPSED_TOOL_PREVIEW_LINES
    };
    let shown = total.min(line_limit);
    for line in preview_lines.iter().take(shown) {
        lines.push(Line::from(vec![
            Span::styled("    ⎿ ", palette.border()),
            Span::styled(
                truncate_to_display_width(line, preview_budget),
                palette.text(),
            ),
        ]));
    }
    if in_scrollback {
        // Full output already shown; no un-actionable "(Ctrl+O expand)" hint.
        return;
    }
    if total > shown {
        let action = if expanded {
            t!("app.hint.ctrl_o_collapse").into_owned()
        } else {
            t!("app.hint.ctrl_o_expand").into_owned()
        };
        lines.push(Line::from(clip_line_spans(
            vec![
                Span::styled("    ⎿ ", palette.border()),
                Span::styled(
                    t!(
                        "app.activity.more_lines_hidden",
                        count = total - shown,
                        action = action
                    )
                    .into_owned(),
                    palette.muted(),
                ),
            ],
            wrap_width,
        )));
    } else if expanded && total > COLLAPSED_TOOL_PREVIEW_LINES {
        lines.push(Line::from(clip_line_spans(
            vec![
                Span::styled("    ⎿ ", palette.border()),
                Span::styled(
                    t!("app.activity.expanded_hint").into_owned(),
                    palette.muted(),
                ),
            ],
            wrap_width,
        )));
    }
}

pub(super) fn push_inline_diff_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    diff: &DiffPreviewPaneState,
    expanded: bool,
    wrap_width: usize,
) {
    // C6: when there is no usable line diff ("line diff unavailable for this
    // mutation"), hide the box entirely instead of rendering an empty preview
    // with a dead "[/] select hunk | c stage" UI. Loading/error stay visible.
    if !diff.has_renderable_diff() {
        return;
    }
    // Side-by-side needs room for two readable columns; below the minimum the
    // render AUTO-FALLS-BACK to unified (and the footer hint explains why).
    let side_by_side_available = wrap_width >= crate::model::DIFF_SIDE_BY_SIDE_MIN_WIDTH;
    let side_by_side = diff.side_by_side && side_by_side_available;
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled(
            t!("app.diff.title").to_string(),
            palette.title().add_modifier(Modifier::BOLD),
        ),
    ]));

    if let Some(preview) = &diff.preview {
        let mut header = vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                preview
                    .title
                    .clone()
                    .unwrap_or_else(|| t!("app.diff.inline_patch").to_string()),
                palette.text().add_modifier(Modifier::BOLD),
            ),
        ];
        // Only surface `status`/`source` when they carry information. Today both
        // are single-variant protocol constants ("ready" / "pending_store")
        // that are pure noise — and "pending_store" is an internal server
        // implementation detail — so the defaults are suppressed and the row
        // shows just the operation + path (e.g. "modify …"). An unrecognized
        // future value is still shown so genuinely new states aren't swallowed.
        if let Some(status) = diff
            .status
            .as_deref()
            .filter(|status| !is_default_diff_status(status))
        {
            header.push(Span::styled("  ", palette.muted()));
            header.push(Span::styled(status.to_string(), palette.muted()));
        }
        if let Some(source) = diff
            .source
            .as_deref()
            .filter(|source| !is_default_diff_source(source))
        {
            header.push(Span::styled("  ", palette.muted()));
            header.push(Span::styled(source.to_string(), palette.muted()));
        }
        lines.push(Line::from(header));

        if preview.files.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(t!("app.empty.no_file_changes").to_string(), palette.muted()),
            ]));
        }

        if !preview.files.is_empty() {
            // Footer hint: hunk navigation/staging, plus the view-mode toggle
            // — or, when the transcript is too narrow to split, why `v` is
            // disabled.
            let mut hint = t!("app.diff.select_stage_hint").into_owned();
            hint.push_str(" | ");
            if side_by_side_available {
                hint.push_str(&if diff.side_by_side {
                    t!("app.diff.toggle_unified_hint")
                } else {
                    t!("app.diff.toggle_side_by_side_hint")
                });
            } else {
                hint.push_str(&t!(
                    "app.diff.side_by_side_too_narrow",
                    min = crate::model::DIFF_SIDE_BY_SIDE_MIN_WIDTH
                ));
            }
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(hint, palette.selected()),
            ]));
        }

        if !preview.files.is_empty() {
            let file_idx = diff
                .selected_file
                .min(preview.files.len().saturating_sub(1));
            if let Some(file) = preview.files.get(file_idx) {
                push_diff_file_lines(
                    lines,
                    palette,
                    file_idx,
                    diff.selected_file,
                    diff.selected_hunk,
                    file,
                    expanded,
                    wrap_width,
                    side_by_side,
                );
            }
        }
        if preview.files.len() > 1 {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    t!(
                        "app.diff.more_files_hidden",
                        count = preview.files.len() - 1
                    )
                    .into_owned(),
                    palette.muted(),
                ),
            ]));
        }
    } else if diff.loading {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(t!("app.diff.loading").to_string(), palette.selected()),
        ]));
    } else if let Some(error) = &diff.error {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(error.clone(), Style::default().fg(palette.danger)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(t!("app.empty.no_diff").to_string(), palette.muted()),
        ]));
    }
}

pub(super) fn push_diff_content_line(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    line: &crate::model::DiffPreviewLine,
) {
    let sign = diff_line_sign(&line.kind);
    let old_line = line
        .old_line
        .map(|line| line.to_string())
        .unwrap_or_else(|| "-".into());
    let new_line = line
        .new_line
        .map(|line| line.to_string())
        .unwrap_or_else(|| "-".into());
    let marker_style = diff_line_marker_style(&line.kind, palette);
    let gutter_style = diff_line_gutter_style(&line.kind, palette);
    let body_style = diff_line_style(&line.kind, palette);
    lines.push(Line::from(vec![
        Span::styled("    ", gutter_style),
        Span::styled(format!("{sign} "), marker_style),
        Span::styled(format!("{old_line:>4} {new_line:>4} "), gutter_style),
        Span::styled(line.content.clone(), body_style),
    ]));
}

/// Render one half of a side-by-side row: line number, sign, and the content
/// cell, styled by the line's kind — or a blank filler half when this side has
/// no line.
pub(super) fn push_diff_half_spans(
    spans: &mut Vec<Span<'static>>,
    palette: Palette,
    line: Option<&crate::model::DiffPreviewLine>,
    line_no: fn(&crate::model::DiffPreviewLine) -> Option<u32>,
    gutter_width: usize,
    cell: usize,
    leading_indent: bool,
) {
    match line {
        Some(line) => {
            let number = line_no(line)
                .map(|number| number.to_string())
                .unwrap_or_else(|| "-".into());
            let gutter = diff_line_gutter_style(&line.kind, palette);
            if leading_indent {
                spans.push(Span::styled("    ", gutter));
            }
            spans.push(Span::styled(format!("{number:>gutter_width$} "), gutter));
            spans.push(Span::styled(
                format!("{} ", diff_line_sign(&line.kind)),
                diff_line_marker_style(&line.kind, palette),
            ));
            spans.push(Span::styled(
                fit_diff_cell(&line.content, cell),
                diff_line_style(&line.kind, palette),
            ));
        }
        None => {
            let blank = palette.muted().bg(palette.surface_alt);
            if leading_indent {
                spans.push(Span::styled("    ", blank));
            }
            spans.push(Span::styled(" ".repeat(gutter_width + 3 + cell), blank));
        }
    }
}

/// One aligned old|new row: `NNNN - old-cell │ NNNN + new-cell`. Long content
/// truncates with `…` inside its cell — v1 has no horizontal scroll.
pub(super) fn push_diff_side_by_side_row(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    row: SideBySideRow<'_>,
    gutter_width: usize,
    wrap_width: usize,
) {
    let (left, right) = row;
    let cell = (wrap_width.saturating_sub(side_by_side_chrome_cols(gutter_width)) / 2).max(8);
    let mut spans = Vec::with_capacity(9);
    push_diff_half_spans(
        &mut spans,
        palette,
        left,
        |line| line.old_line,
        gutter_width,
        cell,
        true,
    );
    spans.push(Span::styled(" │ ", palette.muted()));
    push_diff_half_spans(
        &mut spans,
        palette,
        right,
        |line| line.new_line,
        gutter_width,
        cell,
        false,
    );
    lines.push(Line::from(spans));
}

/// Render one hunk's body. Unified: one row per `DiffPreviewLine`.
/// Side-by-side: lines are paired into aligned old/new rows first
/// (`side_by_side_rows`). `max_rows` caps the output (the collapsed inline
/// view); returns how many rows the cap hid.
pub(super) fn push_diff_hunk_body(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    hunk_lines: &[crate::model::DiffPreviewLine],
    side_by_side: bool,
    wrap_width: usize,
    max_rows: Option<usize>,
) -> usize {
    if side_by_side {
        let rows = side_by_side_rows(hunk_lines);
        let gutter_width = side_by_side_gutter_width(hunk_lines);
        let shown = max_rows.unwrap_or(rows.len()).min(rows.len());
        for row in rows.iter().take(shown) {
            push_diff_side_by_side_row(lines, palette, *row, gutter_width, wrap_width);
        }
        rows.len() - shown
    } else {
        let shown = max_rows.unwrap_or(hunk_lines.len()).min(hunk_lines.len());
        for line in hunk_lines.iter().take(shown) {
            push_diff_content_line(lines, palette, line);
        }
        hunk_lines.len() - shown
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_diff_file_lines(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    file_idx: usize,
    selected_file: usize,
    selected_hunk: usize,
    file: &crate::model::DiffPreviewFile,
    expanded: bool,
    wrap_width: usize,
    side_by_side: bool,
) {
    let path = match &file.old_path {
        Some(old_path) if old_path != &file.path => format!("{old_path} -> {}", file.path),
        _ => file.path.clone(),
    };
    let (added, removed) = diff_file_line_counts(file);
    let badge = diff_file_type_badge(&file.path);
    lines.push(Line::from(vec![
        Span::styled("    ", palette.muted()),
        Span::styled(
            format!(" {badge:<5} "),
            diff_file_badge_style(badge, palette),
        ),
        Span::styled(" ", palette.muted()),
        Span::styled(
            file.status.clone(),
            diff_file_status_style(&file.status, palette),
        ),
        Span::styled("  ", palette.muted()),
        Span::styled(format!("+{added} "), Style::default().fg(palette.success)),
        Span::styled(format!("-{removed} "), Style::default().fg(palette.danger)),
        Span::styled(" ", palette.muted()),
        Span::styled(path, palette.text().add_modifier(Modifier::BOLD)),
    ]));

    if file.hunks.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                t!("app.diff.line_unavailable").into_owned(),
                palette.muted(),
            ),
        ]));
    }

    let hunk_idx = selected_hunk.min(file.hunks.len().saturating_sub(1));

    if expanded {
        // Ctrl+O review mode for staging: show EVERY hunk header so the diff
        // structure stays navigable, and the SELECTED hunk's COMPLETE body so
        // the user can see exactly what they are about to stage (the collapsed
        // view caps each hunk at 4 lines, which is the "can't see the diff"
        // complaint). Non-selected hunks stay header-only to keep the inline
        // view bounded; navigate with the hunk keys to expand another.
        for (idx, hunk) in file.hunks.iter().enumerate() {
            let selected = file_idx == selected_file && idx == selected_hunk;
            let marker = if selected { "  › " } else { "  ├ " };
            lines.push(Line::from(vec![
                Span::styled(marker, palette.selected()),
                Span::styled(hunk.header.clone(), diff_hunk_style(palette)),
            ]));
            if selected {
                push_diff_hunk_body(lines, palette, &hunk.lines, side_by_side, wrap_width, None);
            }
        }
        return;
    }

    if hunk_idx > 0 {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                t!("app.diff.more_hunks_hidden", count = hunk_idx).into_owned(),
                palette.muted(),
            ),
        ]));
    }
    for (rendered_hunk_idx, hunk) in file.hunks.iter().enumerate().skip(hunk_idx).take(1) {
        let hunk_idx = rendered_hunk_idx;
        let selected = file_idx == selected_file && hunk_idx == selected_hunk;
        let marker = if selected { "  › " } else { "  ├ " };
        lines.push(Line::from(vec![
            Span::styled(marker, palette.selected()),
            Span::styled(hunk.header.clone(), diff_hunk_style(palette)),
        ]));
        let hidden = push_diff_hunk_body(
            lines,
            palette,
            &hunk.lines,
            side_by_side,
            wrap_width,
            Some(4),
        );
        if hidden > 0 {
            // Side-by-side hides PAIRED ROWS (one row holds up to two
            // unified lines), so the notice counts rows — the unified
            // "line(s)" label would understate what's hidden.
            let notice = if side_by_side {
                t!("app.diff.more_rows_hidden", count = hidden)
            } else {
                t!("app.diff.more_lines_hidden", count = hidden)
            };
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(notice.into_owned(), palette.muted()),
            ]));
        }
    }
    if file.hunks.len() > 1 {
        let hidden_after = file.hunks.len().saturating_sub(hunk_idx.saturating_add(1));
        if hidden_after == 0 {
            return;
        }
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                t!("app.diff.more_hunks_hidden", count = hidden_after).into_owned(),
                palette.muted(),
            ),
        ]));
    }
}

pub(super) fn push_optional_field(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    label: impl Into<String>,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        push_field(lines, palette, label, value.to_string());
    }
}

pub(super) fn push_field(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    label: impl Into<String>,
    value: String,
) {
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", label.into()), palette.muted()),
        Span::styled(value, palette.text()),
    ]));
}
