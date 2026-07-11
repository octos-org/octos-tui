use ratatui::{
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, LineGauge, List, ListItem, ListState, Paragraph, StatefulWidget,
        Wrap,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use octos_core::{
    Message, SessionKey, TaskId, ui_protocol::TaskRuntimeState, ui_protocol::approval_kinds,
};

use crate::{
    menu::render as menu_render,
    model::{
        ActivityItem, ActivityKind, ActivityNavigatorFilter, AppState, ApprovalModalState,
        ArtifactDetailState, ComposerPresentation, DiffPreviewPaneState, FocusPane,
        PlanStep as RenderedPlanStep, SessionAutonomyState, SessionRunState, SessionView,
        TaskOutputDetailState, TaskView, ThreadGraphDetailState, TurnActivityLog, TurnPromptAnchor,
        TurnStateDetailState, UserQuestionEntry, UserQuestionPickerState, extract_plan_steps,
        task_state_label,
    },
    theme::Palette,
    tui_terminal::FrameLike,
};

pub fn render(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    if app.activity_navigator.active {
        render_activity_navigator_overlay(frame, app, palette);
        return;
    } else if inspector_visible(app) {
        render_inspector_layout(frame, app, palette);
    } else {
        render_chat_layout(frame, app, palette);
    }

    if app.task_output.active {
        render_task_output_modal(frame, &app.task_output, palette);
    }
    if app.artifact_detail.active {
        render_artifact_detail_modal(frame, &app.artifact_detail, palette);
    }
    if app.thread_graph_detail.active {
        render_thread_graph_detail_modal(frame, &app.thread_graph_detail, palette);
    }
    if app.turn_state_detail.active {
        render_turn_state_detail_modal(frame, &app.turn_state_detail, palette);
    }
}

/// Full-screen overlay render for the alt-screen path (inspector, onboarding,
/// detail modals). Identical layout to [`render`]; named separately so the event
/// loop's alt-screen branch reads clearly against the inline-viewport branch.
pub fn render_inline_overlay(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    render(frame, app, palette);
}

fn inspector_visible(app: &AppState) -> bool {
    matches!(
        app.focus,
        FocusPane::Sessions
            | FocusPane::Tasks
            | FocusPane::Artifacts
            | FocusPane::Workspace
            | FocusPane::Git
    )
}

// ===========================================================================
// Inline-viewport rendering (codex-style scrollback model).
//
// The event loop keeps the live UI (live transcript tail + menus + indicators +
// composer + status) in a small ratatui inline viewport pinned to the bottom of
// the screen, and writes *finalized* transcript history into the terminal's
// normal scrollback (via `insert_history`). The terminal then owns that
// scrollback, so the user can natively mouse-select, wheel-scroll, and copy
// prior output (incl. through tmux) with no app mode key.
//
// `render_viewport` is the live-UI draw; `finalized_history_lines` produces the
// committed-only lines flushed to scrollback. Full-screen overlays (inspector,
// onboarding, modals) fall back to the legacy `render` path under alt-screen —
// see `wants_fullscreen_overlay`.
// ===========================================================================

/// True when the current state needs the legacy full-screen render (alt-screen),
/// rather than the inline-viewport + scrollback chat flow. Mirrors codex using
/// alt-screen only for transient overlays (transcript pager, resume picker).
pub fn wants_fullscreen_overlay(app: &AppState) -> bool {
    app.activity_navigator.active
        || inspector_visible(app)
        || onboarding_first_launch_active(app)
        || app.transcript_pager_active
        || app.task_output.active
        || app.artifact_detail.active
        || app.thread_graph_detail.active
        || app.turn_state_detail.active
}

/// Mouse capture policy. In the default `native` scroll-mode, capture is on
/// ONLY while the transcript pager is up, so the wheel scrolls the pager and
/// the inline chat flow keeps native terminal selection/copy untouched. In
/// `pinned` scroll-mode the user explicitly trades native selection for a
/// wheel that always scrolls the app (composer pinned), so capture stays on.
pub fn wants_mouse_capture(app: &AppState) -> bool {
    app.transcript_pager_active || app.pinned_scroll
}

/// Watermarks for active-turn content that has already been written into native
/// scrollback while the turn is still running. The inline viewport uses this to
/// hide the same stable prefix so spinner ticks only repaint the live tail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTurnFinalization {
    pub session_id: String,
    pub turn_id: String,
    pub reply_flushed_text: String,
    pub activity_flushed_items: usize,
    pub activity_flushed_keys: Vec<String>,
}

impl LiveTurnFinalization {
    fn new(session_id: &SessionKey, turn_id: &octos_core::ui_protocol::TurnId) -> Self {
        Self {
            session_id: session_id.0.clone(),
            turn_id: turn_id.0.to_string(),
            reply_flushed_text: String::new(),
            activity_flushed_items: 0,
            activity_flushed_keys: Vec::new(),
        }
    }

    pub(crate) fn matches_turn(
        &self,
        session_id: &SessionKey,
        turn_id: &octos_core::ui_protocol::TurnId,
    ) -> bool {
        self.session_id == session_id.0 && self.turn_id == turn_id.0.to_string()
    }

    pub(crate) fn has_flushed_content(&self) -> bool {
        !self.reply_flushed_text.is_empty()
            || self.activity_flushed_items > 0
            || !self.activity_flushed_keys.is_empty()
    }
}

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
    let autonomy_height = autonomy_indicator_height(app);
    let harness_height = harness_status_height(app);
    let chrome = menu_height + autonomy_height + harness_height + composer_height + 1; // +1 status

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

/// Minimum rows of scrollback to keep visible above the inline viewport.
const LIVE_VIEWPORT_MIN_SCROLLBACK: u16 = 4;

/// Desired height of the live transcript tail (in-flight / uncommitted content
/// shown inside the viewport). Bounded; the bulk of history lives in scrollback.
fn live_tail_height_with_finalization(
    app: &AppState,
    width: u16,
    height: u16,
    live_finalization: Option<&LiveTurnFinalization>,
) -> u16 {
    if launch_banner_active(app) {
        // The empty-session launch banner wants a generous block.
        return height.saturating_sub(8).clamp(6, 16);
    }
    let wrap_width = usize::from(width.saturating_sub(2)).max(1);
    let lines = live_tail_lines_with_finalization(
        app,
        Palette::for_theme(app.theme),
        wrap_width,
        live_finalization,
    );
    let rows = transcript_visual_rows(&lines, wrap_width) as u16;
    // The `/btw` aside draws as a floating overlay OVER the tail's top rows
    // (`render_btw_overlay`) and adds no flow lines of its own — reserve its
    // rows here or a settled/short tail starves the overlay's 3-row minimum
    // and the pane becomes invisible while still answering (codex P1).
    let rows = rows.max(btw_overlay_height_hint(app, width));
    // Cap the tail so it can't dominate the viewport; the rest stays in
    // scrollback. Scale with the terminal (at most half its height) rather than
    // a fixed 18 — a tall terminal shouldn't strand the live tail at 18 while a
    // short one over-reserves. The outer `live_ui_height` clamp still guarantees
    // `LIVE_VIEWPORT_MIN_SCROLLBACK` rows of scrollback remain visible.
    let max_tail = (height / 2).max(3);
    rows.clamp(0, max_tail)
}

pub(crate) fn live_tail_has_guarded_sections(
    app: &AppState,
    live_finalization: Option<&LiveTurnFinalization>,
) -> bool {
    !app.pending_messages.is_empty() || live_tail_has_activity_section(app, live_finalization)
}

fn live_tail_has_activity_section(
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

/// Render the live UI into the inline viewport (`frame.area()` is the viewport).
/// Mirrors `render_chat_layout` but the top pane shows only the live transcript
/// tail (finalized history is in scrollback, not here).
pub fn render_viewport(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    let terminal_height = frame.area().height;
    render_viewport_with_finalization(frame, app, palette, terminal_height, None);
}

pub fn render_viewport_with_finalization(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
    terminal_height: u16,
    live_finalization: Option<&LiveTurnFinalization>,
) {
    let area = frame.area();
    // The composer cap must be derived from the FULL terminal height — the same
    // basis `live_ui_height` used to RESERVE the composer rows. `area.height`
    // here is only the inline viewport region (it already excludes scrollback),
    // so deriving the cap from it would shrink the composer below what was
    // reserved (cap floors at 3), clipping multi-line input. Everything else
    // legitimately lays out within `area`.
    let composer_height = composer_height_for_size(app, area.width, terminal_height);
    let active_menu = active_menu_surface(app);
    let menu_height = menu_height_for_viewport(
        active_menu.as_ref(),
        area.width,
        area.height.saturating_sub(composer_height + 1),
    );
    let autonomy_height = autonomy_indicator_height(app);
    let harness_height = harness_status_height(app);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(menu_height),
            Constraint::Length(autonomy_height),
            Constraint::Length(harness_height),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);

    if launch_banner_active(app) {
        render_launch_banner(frame, app, palette, root[0]);
    } else {
        frame.render_widget(
            render_live_tail_with_finalization(app, palette, root[0], live_finalization),
            root[0],
        );
        // `/btw` aside floats over the top of the live tail so it reads as a
        // distinct top pane instead of mingling with the streaming reply.
        render_btw_overlay(frame, app, palette, root[0]);
    }
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, root[1], menu, palette);
    }
    if autonomy_height > 0 {
        frame.render_widget(render_autonomy_indicator(app, palette), root[2]);
    }
    if harness_height > 0 {
        render_harness_status_row(frame, app, palette, root[3]);
    }
    frame.render_widget(render_composer(app, palette, root[4]), root[4]);
    set_composer_cursor(frame, app, root[4]);
    frame.render_widget(render_status(app, palette), root[5]);
}

/// The live (uncommitted / in-flight) transcript tail rendered inside the
/// viewport: recent-user context, turn-flow, the streaming reply, activity, and
/// pending messages. Committed messages are NOT here — they are in scrollback.
fn render_live_tail_with_finalization(
    app: &AppState,
    palette: Palette,
    area: Rect,
    live_finalization: Option<&LiveTurnFinalization>,
) -> Paragraph<'static> {
    let wrap_width = transcript_wrap_width(area);
    let lines = live_tail_lines_with_finalization(app, palette, wrap_width, live_finalization);

    let visible_height = transcript_visible_height(area);
    let total_rows = transcript_visual_rows(&lines, wrap_width);
    let max_scroll = total_rows.saturating_sub(visible_height);
    let scroll_from_bottom = app.transcript_scroll.min(max_scroll);
    let scroll_top =
        u16::try_from(max_scroll.saturating_sub(scroll_from_bottom)).unwrap_or(u16::MAX);

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                // The inline live tail sits directly above the terminal's native
                // scrollback (where finalized history is written on the DEFAULT
                // background). Painting the whole tail with `surface_alt` made it
                // a solid theme-colored rectangle that reads as "brown blocks all
                // over the screen" against that native scrollback — the
                // user-reported bug. Render the tail on the default background so
                // it blends with scrollback and the terminal, matching codex's
                // inline rendering. (The fullscreen-overlay `render_transcript`
                // path keeps `surface_alt` — it has no terminal scrollback behind
                // it.) Interactive cards and the composer/status set their own
                // backgrounds on their own spans.
                .style(Style::default().fg(palette.text))
                .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false })
}

/// Build the live-tail lines (everything that is NOT finalized committed
/// history): recent-user context pinned for the active turn, turn-flow
/// (approvals / questions / streaming reply / activity / diff preview), and
/// pending queued messages.
fn active_live_finalization<'a>(
    app: &AppState,
    live_finalization: Option<&'a LiveTurnFinalization>,
) -> Option<&'a LiveTurnFinalization> {
    let (session_id, turn_id) = app.active_turn()?;
    live_finalization.filter(|finalization| finalization.matches_turn(session_id, turn_id))
}

fn live_tail_lines_with_finalization(
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

/// Drop blank lines from the end of a line set (a line is blank when every
/// span is whitespace). Interior blanks — paragraph separators — are kept.
fn trim_trailing_blank_lines(lines: &mut Vec<Line<'static>>) {
    while lines.last().is_some_and(|line| line_is_blank(Some(line))) {
        lines.pop();
    }
}

/// Collapse any run of two-or-more consecutive blank lines down to a single
/// blank, keeping the first of each run. The block builders
/// (`push_message_block`, `push_live_reply_block`, `push_formatted_body_marked`,
/// the activity-log/tool-call sections) each guard their *own* leading/trailing
/// separator, but a single flush concatenates several of them into one buffer
/// (committed history + live-turn deltas in `viewport.rs`), so a block that ends
/// in a blank followed by one that opens with a blank sums to a multi-line gap.
/// Applied once at the assembly endpoints, this guarantees at most one blank
/// between blocks regardless of how the pieces were produced. It never tightens
/// a single blank or fuses two non-blank blocks (a run of one stays one), so it
/// can only remove excess vertical space, never introduce it.
pub fn collapse_blank_runs(lines: &mut Vec<Line<'static>>) {
    collapse_blank_runs_seeded(lines, false);
}

/// [`collapse_blank_runs`] that also closes the seam against content already
/// emitted before this batch. `prev_ends_blank` is whether the line immediately
/// preceding these — e.g. the last line already in scrollback from an earlier
/// flush — was blank; when it was, a leading blank here is dropped. Reply text
/// streams to scrollback across many small flushes, so without this a chunk
/// ending on a blank and the next chunk opening on a blank stack into a 2-line
/// gap that per-batch collapse can't see. Returns whether the batch now ends on
/// a blank (feed back as the next call's `prev_ends_blank`).
pub fn collapse_blank_runs_seeded(lines: &mut Vec<Line<'static>>, prev_ends_blank: bool) -> bool {
    let mut prev_blank = prev_ends_blank;
    lines.retain(|line| {
        let blank = line_is_blank(Some(line));
        let keep = !(blank && prev_blank);
        prev_blank = blank;
        keep
    });
    match lines.last() {
        Some(line) => line_is_blank(Some(line)),
        // Batch contributed nothing (all dropped) → seam state is unchanged.
        None => prev_ends_blank,
    }
}

pub fn collapse_blank_runs_seeded_orphan_guard(
    lines: &mut Vec<Line<'static>>,
    prev_ends_blank: bool,
    drop_orphaned_leading_blank_run: bool,
) -> bool {
    if drop_orphaned_leading_blank_run {
        let leading_blank_run = lines
            .iter()
            .take_while(|line| line_is_blank(Some(line)))
            .count();
        if leading_blank_run > 1 {
            lines.drain(0..leading_blank_run);
        }
    }
    collapse_blank_runs_seeded(lines, prev_ends_blank)
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
            push_turn_activity_log_section(&mut lines, palette, log, app, false, wrap_width);
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
            // Continuation of a reply whose prefix is already in scrollback
            // (coverage is only matched when non-empty) — never re-issue the
            // bullet, but do seed blank handling from the streamed prefix so a
            // separator split across commit still renders like one document.
            push_live_reply_block_seeded(
                &mut lines,
                palette,
                suffix,
                wrap_width,
                false,
                true,
                live_reply_prefix_ends_blank(palette, &coverage.reply_flushed_text, wrap_width),
            );
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
                push_turn_activity_log_section(&mut lines, palette, log, app, false, wrap_width);
            }
        }
    }
    strip_lines_background(&mut lines);
    lines
}

/// A recorded segment boundary is "word-safe" when it does NOT fall inside a
/// word/token — i.e. not (the char before AND the char at the offset are both
/// word chars). `message/persisted` can sample the live buffer mid-word
/// ("anim|ate"); splitting or flushing there breaks words in immutable
/// scrollback. Boundaries adjacent to a delimiter (whitespace, punctuation, line
/// end, or buffer edge) pass — `ToolStarted` boundaries normally sit after
/// sentence punctuation and pass anyway.
fn boundary_is_word_safe(text: &str, boundary: usize) -> bool {
    if boundary > text.len() || !text.is_char_boundary(boundary) {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let before = text[..boundary].chars().next_back().is_some_and(is_word);
    let after = text[boundary..].chars().next().is_some_and(is_word);
    !(before && after)
}

fn committed_reply_segment_boundaries_for_message(
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

fn push_committed_assistant_reply_segments(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    wrap_width: usize,
    boundaries: &[usize],
) {
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

/// Return the next active-turn watermark by extending the previous one with any
/// newly settled live reply lines and any non-running activity rows.
pub fn next_live_turn_finalization(
    app: &AppState,
    previous: Option<&LiveTurnFinalization>,
) -> Option<LiveTurnFinalization> {
    let (session_id, turn_id) = app.active_turn()?;
    let session = app.active_session()?;
    let mut next = previous
        .filter(|finalization| finalization.matches_turn(session_id, turn_id))
        .cloned()
        .unwrap_or_else(|| LiveTurnFinalization::new(session_id, turn_id));

    if let Some(live_reply) = session
        .live_reply
        .as_ref()
        .filter(|live_reply| &live_reply.turn_id == turn_id)
        && live_reply
            .text
            .starts_with(next.reply_flushed_text.as_str())
    {
        // A completed content segment (the text before a tool call) is stable and
        // flushable even without a trailing blank line. Without this, an agentic
        // turn whose narration segments are glued ("…step 1.step 2:") never
        // advances the blank-line watermark, so the whole growing reply stays in
        // the height-limited live tail and clips to its bottom — the user sees a
        // mid-reply fragment ("intermediate truncated") while the committed render
        // is correct. Flush through the last completed segment boundary so the
        // live tail holds only the in-progress segment.
        let last_completed_segment = app
            .live_reply_segment_boundaries
            .get(&(session_id.clone(), turn_id.clone()))
            .into_iter()
            .flatten()
            .copied()
            .filter(|b| {
                *b <= live_reply.text.len()
                    && live_reply.text.is_char_boundary(*b)
                    && boundary_is_word_safe(&live_reply.text, *b)
            })
            .max()
            .unwrap_or(0);
        // A completed segment is flushable UNLESS it ends inside an unclosed code
        // fence (a tool call mid-```block```), which stable_live_reply_prefix_len
        // deliberately pins behind — never flush an unbalanced fence into immutable
        // scrollback. Plain-text narration segments (the glued case this targets)
        // carry no fence and stay flushable.
        let segment_end = if last_completed_segment > 0
            && live_reply.text[..last_completed_segment]
                .lines()
                .filter(|line| line.trim_start().starts_with("```"))
                .count()
                % 2
                == 0
        {
            last_completed_segment
        } else {
            0
        };
        let stable_end = stable_live_reply_prefix_len(&live_reply.text).max(segment_end);
        if stable_end > next.reply_flushed_text.len() {
            next.reply_flushed_text = live_reply.text[..stable_end].to_string();
        }
    }

    let mut existing_activity = next
        .activity_flushed_keys
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    for (idx, item) in flow_activity_items(app).iter().enumerate() {
        let key = activity_finalization_key(item, idx);
        if !existing_activity.contains(&key) && !is_running_activity(item) {
            existing_activity.insert(key.clone());
            next.activity_flushed_keys.push(key);
        }
    }
    next.activity_flushed_items = next.activity_flushed_keys.len();

    Some(next)
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

fn push_live_reply_delta_seeded(
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

fn live_reply_segment_boundaries_in_delta(
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

fn push_live_reply_segment_separator(
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

fn activity_finalization_key(item: &ActivityItem, ordinal: usize) -> String {
    if let Some(tool_call_id) = item.tool_call_id.as_deref() {
        return format!("tool:{tool_call_id}");
    }
    if let Some(turn_id) = item.turn_id.as_ref() {
        return format!(
            "turn:{}:{ordinal}:{}:{}",
            turn_id.0,
            item.kind.label(),
            item.title
        );
    }
    format!("activity:{ordinal}:{}:{}", item.kind.label(), item.title)
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

/// Largest prefix of the streaming reply that is safe to flush into the
/// IMMUTABLE terminal scrollback (codex's markdown-stream model): the cut may
/// only land on a *completed block* boundary — a closed code fence, or a blank
/// line ending a paragraph/table/list run. Completed blocks are self-contained,
/// so rendering each flushed batch as an independent document stays correct;
/// an unclosed fence or a still-accumulating paragraph is held back (it keeps
/// re-rendering in the live tail) rather than written out half-parsed and
/// frozen wrong forever. The state machine is line-oriented: only lines
/// terminated by `\n` are considered at all.
fn stable_live_reply_prefix_len(text: &str) -> usize {
    let mut safe_end = 0;
    let mut offset = 0;
    let mut in_fence = false;
    let mut fence_start = 0;
    for segment in text.split_inclusive('\n') {
        if !segment.ends_with('\n') {
            // Trailing partial line: never flushable.
            break;
        }
        let line_start = offset;
        offset += segment.len();
        let trimmed = segment.trim();
        if trimmed.starts_with("```") {
            if in_fence {
                // Fence closed → the whole fenced block just completed.
                in_fence = false;
                safe_end = offset;
            } else {
                in_fence = true;
                fence_start = line_start;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.is_empty() {
            // Blank line ends any open paragraph / table / list run.
            safe_end = offset;
        }
    }
    if in_fence {
        // An unclosed fence pins the watermark before the fence opener, even
        // if blank lines were seen inside the fence body.
        safe_end = safe_end.min(fence_start);
    }
    safe_end
}

fn strip_lines_background(lines: &mut [Line<'static>]) {
    for line in lines {
        strip_line_background(line);
    }
}

/// Reset the background of a finalized scrollback line (and every span) to the
/// terminal default, so history written into real scrollback blends with the
/// terminal's native background instead of painting the theme surface. Only the
/// background is cleared; foreground colors and text attributes are preserved.
fn strip_line_background(line: &mut Line<'static>) {
    line.style.bg = None;
    for span in &mut line.spans {
        span.style.bg = None;
    }
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

/// Identity of the committed history flushed to scrollback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommittedFingerprint {
    pub session_id: String,
    pub message_count: usize,
    pub activity_log_count: usize,
    pub content_hash: u64,
}

fn committed_content_hash(
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

fn render_chat_layout(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    if onboarding_first_launch_active(app) {
        render_onboarding_first_launch_layout(frame, app, palette);
        return;
    }

    let active_menu = active_menu_surface(app);
    let areas = chat_layout_areas_for_menu(app, frame.area(), active_menu.as_ref());

    if launch_banner_active(app) {
        render_launch_banner(frame, app, palette, areas.transcript);
    } else {
        let transcript = transcript_render_model(app, palette, areas.transcript);
        let metrics = transcript.metrics;
        frame.render_widget(transcript.paragraph, areas.transcript);
        if app.transcript_pager_active {
            render_pager_scrollbar(frame, metrics, areas.transcript, palette);
        }
        // `/btw` aside floats over the top of the transcript as a distinct pane.
        render_btw_overlay(frame, app, palette, areas.transcript);
    }
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, areas.menu, menu, palette);
    }
    if areas.autonomy.height > 0 {
        frame.render_widget(render_autonomy_indicator(app, palette), areas.autonomy);
    }
    if areas.harness.height > 0 {
        render_harness_status_row(frame, app, palette, areas.harness);
    }
    frame.render_widget(
        render_composer(app, palette, areas.composer),
        areas.composer,
    );
    set_composer_cursor(frame, app, areas.composer);
    frame.render_widget(render_status(app, palette), areas.status);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatLayoutAreas {
    pub transcript: Rect,
    pub menu: Rect,
    pub autonomy: Rect,
    pub harness: Rect,
    pub composer: Rect,
    pub status: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptScrollMetrics {
    pub visible_rows: usize,
    pub total_rows: usize,
    pub scroll_from_bottom: usize,
    pub max_scroll_from_bottom: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarThumb {
    pub top: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintBarMode {
    StatusbarKeys,
    Menu,
    Onboarding,
    Approval,
    UserQuestion,
    PagerKeys,
    PagerReviewing,
    ActivityNavigator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintBarModel {
    pub mode: HintBarMode,
}

pub fn hint_bar_model(app: &AppState) -> HintBarModel {
    let mode = if app.activity_navigator.active {
        HintBarMode::ActivityNavigator
    } else if app
        .approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
    {
        HintBarMode::Approval
    } else if app
        .user_question
        .as_ref()
        .is_some_and(|question| question.visible)
    {
        HintBarMode::UserQuestion
    } else if onboarding_first_launch_active(app) {
        HintBarMode::Onboarding
    } else if app.menu_stack.is_active() {
        HintBarMode::Menu
    } else if app.transcript_pager_active && app.transcript_scroll > 0 {
        HintBarMode::PagerReviewing
    } else if app.transcript_pager_active {
        HintBarMode::PagerKeys
    } else {
        HintBarMode::StatusbarKeys
    };
    HintBarModel { mode }
}

pub fn scrollbar_thumb(metrics: TranscriptScrollMetrics, track: Rect) -> Option<ScrollbarThumb> {
    if track.height == 0 || metrics.max_scroll_from_bottom == 0 || metrics.visible_rows == 0 {
        return None;
    }

    let track_height = usize::from(track.height);
    let thumb_height = metrics
        .visible_rows
        .saturating_mul(track_height)
        .div_ceil(metrics.total_rows.max(1))
        .clamp(1, track_height);
    let max_top_offset = track_height.saturating_sub(thumb_height);
    let scrolled_from_top = metrics
        .max_scroll_from_bottom
        .saturating_sub(metrics.scroll_from_bottom);
    let top_offset = if max_top_offset == 0 {
        0
    } else {
        scrolled_from_top
            .saturating_mul(max_top_offset)
            .div_ceil(metrics.max_scroll_from_bottom)
            .min(max_top_offset)
    };

    Some(ScrollbarThumb {
        top: track.y.saturating_add(top_offset as u16),
        height: thumb_height as u16,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityNavigatorRowKind {
    Session,
    Message,
    Orchestration,
    Task,
    FileChange,
    Activity,
    Approval,
}

impl ActivityNavigatorRowKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Message => "message",
            Self::Orchestration => "orchestration",
            Self::Task => "task",
            Self::FileChange => "change",
            Self::Activity => "activity",
            Self::Approval => "approval",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityNavigatorStatus {
    Running,
    Blocked,
    Failed,
    Done,
}

impl ActivityNavigatorStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityNavigatorCounts {
    pub all: usize,
    pub running: usize,
    pub blocked: usize,
    pub failed: usize,
    pub done: usize,
    pub changes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityNavigatorRow {
    pub kind: ActivityNavigatorRowKind,
    pub status: ActivityNavigatorStatus,
    pub title: String,
    pub subtitle: String,
    pub detail_lines: Vec<String>,
    pub session_id: Option<SessionKey>,
    pub task_id: Option<TaskId>,
    pub turn_id: Option<String>,
    search_text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ActivityNavigatorRowLinks {
    session_id: Option<SessionKey>,
    task_id: Option<TaskId>,
    turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityNavigatorModel {
    pub rows: Vec<ActivityNavigatorRow>,
    pub counts: ActivityNavigatorCounts,
    pub selected: usize,
    pub query: String,
    pub filter: ActivityNavigatorFilter,
    pub search_active: bool,
}

impl ActivityNavigatorModel {
    pub fn selected_row(&self) -> Option<&ActivityNavigatorRow> {
        self.rows.get(self.selected)
    }
}

pub fn activity_navigator_model(app: &AppState) -> ActivityNavigatorModel {
    let mut rows = activity_navigator_all_rows(app);
    let query = app.activity_navigator.query.trim().to_ascii_lowercase();
    if !query.is_empty() {
        rows.retain(|row| row.search_text.contains(&query));
    }
    let counts = activity_navigator_counts(&rows);
    rows.retain(|row| activity_navigator_filter_matches(app.activity_navigator.filter, row.status));
    let selected = app
        .activity_navigator
        .selected
        .min(rows.len().saturating_sub(1));

    ActivityNavigatorModel {
        rows,
        counts,
        selected,
        query: app.activity_navigator.query.clone(),
        filter: app.activity_navigator.filter,
        search_active: app.activity_navigator.search_active,
    }
}

pub fn selected_activity_navigator_session(app: &AppState) -> Option<SessionKey> {
    activity_navigator_model(app)
        .selected_row()
        .and_then(|row| row.session_id.clone())
}

fn activity_navigator_filter_matches(
    filter: ActivityNavigatorFilter,
    status: ActivityNavigatorStatus,
) -> bool {
    match filter {
        ActivityNavigatorFilter::All => true,
        ActivityNavigatorFilter::Running => status == ActivityNavigatorStatus::Running,
        ActivityNavigatorFilter::Blocked => status == ActivityNavigatorStatus::Blocked,
        ActivityNavigatorFilter::Failed => status == ActivityNavigatorStatus::Failed,
        ActivityNavigatorFilter::Done => status == ActivityNavigatorStatus::Done,
    }
}

fn activity_navigator_counts(rows: &[ActivityNavigatorRow]) -> ActivityNavigatorCounts {
    let mut counts = ActivityNavigatorCounts {
        all: rows.len(),
        ..ActivityNavigatorCounts::default()
    };
    for row in rows {
        match row.status {
            ActivityNavigatorStatus::Running => counts.running += 1,
            ActivityNavigatorStatus::Blocked => counts.blocked += 1,
            ActivityNavigatorStatus::Failed => counts.failed += 1,
            ActivityNavigatorStatus::Done => counts.done += 1,
        }
        if row.kind == ActivityNavigatorRowKind::FileChange {
            counts.changes += 1;
        }
    }
    counts
}

fn activity_navigator_all_rows(app: &AppState) -> Vec<ActivityNavigatorRow> {
    let mut rows = Vec::new();
    if let Some(row) = activity_navigator_run_state_row(app) {
        rows.push(row);
    }
    if let Some(row) = activity_navigator_approval_row(app) {
        rows.push(row);
    }
    if let Some(row) = activity_navigator_question_row(app) {
        rows.push(row);
    }

    for session_idx in activity_navigator_session_order(app) {
        let Some(session) = app.sessions.get(session_idx) else {
            continue;
        };
        if let Some(orchestration) = app.orchestration.get(&session.id)
            && orchestration.active
        {
            rows.push(activity_navigator_row(
                ActivityNavigatorRowKind::Orchestration,
                ActivityNavigatorStatus::Running,
                session.title.clone(),
                "orchestration active".to_string(),
                vec![
                    format!("session: {}", session.id.0),
                    format!(
                        "phase: {}",
                        orchestration.phase.as_deref().unwrap_or("active")
                    ),
                    format!("running agents: {}", orchestration.running_agents),
                    format!(
                        "pending continuations: {}",
                        orchestration.pending_continuations
                    ),
                ],
                ActivityNavigatorRowLinks {
                    session_id: Some(session.id.clone()),
                    ..ActivityNavigatorRowLinks::default()
                },
            ));
        }
        for task in &session.tasks {
            rows.push(activity_navigator_task_row(session, task));
        }
        rows.extend(
            app.activity
                .iter()
                .filter(|item| activity_belongs_to_session(app, item, &session.id))
                .map(|item| activity_navigator_activity_row(session, item, None)),
        );
        for log in app
            .turn_activity_logs
            .iter()
            .filter(|log| log.session_id == session.id)
        {
            for item in &log.items {
                rows.push(activity_navigator_activity_row(
                    session,
                    item,
                    Some(log.turn_id.0.to_string()),
                ));
            }
        }
        rows.extend(
            session
                .messages
                .iter()
                .enumerate()
                .filter(|(_, message)| message.role.as_str() != "system")
                .map(|(idx, message)| activity_navigator_message_row(session, idx, message)),
        );
    }

    rows
}

fn activity_navigator_session_order(app: &AppState) -> Vec<usize> {
    let mut order = Vec::with_capacity(app.sessions.len());
    if app.selected_session < app.sessions.len() {
        order.push(app.selected_session);
    }
    order.extend((0..app.sessions.len()).filter(|idx| *idx != app.selected_session));
    order
}

fn activity_navigator_run_state_row(app: &AppState) -> Option<ActivityNavigatorRow> {
    let (status, title) = match &app.run_state {
        SessionRunState::Idle => return None,
        SessionRunState::InProgress => (ActivityNavigatorStatus::Running, "session running"),
        SessionRunState::Blocked { .. } => (ActivityNavigatorStatus::Blocked, "session blocked"),
        SessionRunState::Success => (ActivityNavigatorStatus::Done, "session done"),
        SessionRunState::Error { .. } => (ActivityNavigatorStatus::Failed, "session error"),
    };
    let session = app.active_session();
    let detail = app.run_state.detail().unwrap_or(app.status.as_str());
    Some(activity_navigator_row(
        ActivityNavigatorRowKind::Session,
        status,
        title.to_string(),
        session
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "no active session".to_string()),
        vec![
            format!("state: {}", app.run_state.label()),
            format!("status: {}", app.status),
            format!("detail: {detail}"),
        ],
        ActivityNavigatorRowLinks {
            session_id: session.map(|session| session.id.clone()),
            ..ActivityNavigatorRowLinks::default()
        },
    ))
}

fn activity_navigator_approval_row(app: &AppState) -> Option<ActivityNavigatorRow> {
    let approval = app.approval.as_ref().filter(|approval| approval.visible)?;
    let session = app.active_session();
    Some(activity_navigator_row(
        ActivityNavigatorRowKind::Approval,
        ActivityNavigatorStatus::Blocked,
        approval.title.clone(),
        "approval required".to_string(),
        vec![
            format!("tool: {}", approval.tool_name),
            format!(
                "kind: {}",
                approval.approval_kind.as_deref().unwrap_or("unknown")
            ),
            format!("body: {}", approval.body),
        ],
        ActivityNavigatorRowLinks {
            session_id: session.map(|session| session.id.clone()),
            ..ActivityNavigatorRowLinks::default()
        },
    ))
}

fn activity_navigator_question_row(app: &AppState) -> Option<ActivityNavigatorRow> {
    let question = app
        .user_question
        .as_ref()
        .filter(|question| question.visible)?;
    let session = app.active_session();
    Some(activity_navigator_row(
        ActivityNavigatorRowKind::Approval,
        ActivityNavigatorStatus::Blocked,
        question.title.clone(),
        "question pending".to_string(),
        vec![
            format!("question id: {}", question.question_id.0),
            format!("questions: {}", question.questions.len()),
        ],
        ActivityNavigatorRowLinks {
            session_id: session.map(|session| session.id.clone()),
            ..ActivityNavigatorRowLinks::default()
        },
    ))
}

fn activity_navigator_message_row(
    session: &SessionView,
    idx: usize,
    message: &Message,
) -> ActivityNavigatorRow {
    let role = message.role.as_str();
    let content = message.content.trim();
    let title = if content.is_empty() {
        format!("{role}: empty message")
    } else {
        format!(
            "{role}: {}",
            truncate_terminal_line(&content.replace('\n', " "), 80)
        )
    };
    let mut detail = vec![
        format!("session: {}", session.id.0),
        format!("message: {}", idx + 1),
        format!("role: {role}"),
    ];
    if !content.is_empty() {
        detail.push("content:".to_string());
        detail.extend(content.lines().take(10).map(|line| format!("  {line}")));
    }
    if let Some(reasoning) = message.reasoning_content.as_deref() {
        detail.push("reasoning:".to_string());
        detail.extend(reasoning.lines().take(6).map(|line| format!("  {line}")));
    }
    if let Some(tool_call_id) = message.tool_call_id.as_deref() {
        detail.push(format!("tool call: {tool_call_id}"));
    }

    activity_navigator_row(
        ActivityNavigatorRowKind::Message,
        ActivityNavigatorStatus::Done,
        title,
        format!("{} · message {}", session.title, idx + 1),
        detail,
        ActivityNavigatorRowLinks {
            session_id: Some(session.id.clone()),
            ..ActivityNavigatorRowLinks::default()
        },
    )
}

fn activity_navigator_task_row(session: &SessionView, task: &TaskView) -> ActivityNavigatorRow {
    let status = match task.state {
        TaskRuntimeState::Pending | TaskRuntimeState::Running => ActivityNavigatorStatus::Running,
        TaskRuntimeState::Completed => ActivityNavigatorStatus::Done,
        TaskRuntimeState::Failed | TaskRuntimeState::Cancelled => ActivityNavigatorStatus::Failed,
    };
    let mut detail = vec![
        format!("session: {}", session.id.0),
        format!("task: {}", task.id.0),
        format!("state: {}", task_state_label(task.state)),
    ];
    if let Some(runtime_detail) = task.runtime_detail.as_ref() {
        detail.push(format!("detail: {runtime_detail}"));
    }
    if !task.output_tail.trim().is_empty() {
        detail.push("output tail:".to_string());
        detail.extend(
            task.output_tail
                .lines()
                .take(8)
                .map(|line| format!("  {line}")),
        );
    }

    activity_navigator_row(
        ActivityNavigatorRowKind::Task,
        status,
        task.title.clone(),
        format!("{} · {}", session.title, task_state_label(task.state)),
        detail,
        ActivityNavigatorRowLinks {
            session_id: Some(session.id.clone()),
            task_id: Some(task.id.clone()),
            turn_id: task.turn_id.as_ref().map(|turn| turn.0.to_string()),
        },
    )
}

fn activity_navigator_activity_row(
    session: &SessionView,
    item: &ActivityItem,
    archived_turn_id: Option<String>,
) -> ActivityNavigatorRow {
    if let Some(mutation) = FileMutationActivity::from_item(item) {
        return activity_navigator_file_change_row(session, item, mutation, archived_turn_id);
    }

    let status = activity_navigator_activity_status(item);
    let turn_id = archived_turn_id.or_else(|| item.turn_id.as_ref().map(|turn| turn.0.to_string()));
    let mut detail = vec![
        format!("session: {}", session.id.0),
        format!("kind: {}", item.kind.label()),
        format!("status: {}", item.status),
    ];
    if let Some(turn_id) = turn_id.as_ref() {
        detail.push(format!("turn: {turn_id}"));
    }
    if let Some(tool_call_id) = item.tool_call_id.as_ref() {
        detail.push(format!("tool call: {tool_call_id}"));
    }
    if let Some(item_detail) = item.detail.as_ref() {
        detail.push(format!("detail: {item_detail}"));
    }
    if let Some(output) = item
        .output_preview
        .as_ref()
        .filter(|output| !output.is_empty())
    {
        detail.push("output preview:".to_string());
        detail.extend(output.lines().take(8).map(|line| format!("  {line}")));
    }

    activity_navigator_row(
        ActivityNavigatorRowKind::Activity,
        status,
        item.title.clone(),
        format!("{} · {}", session.title, item.status),
        detail,
        ActivityNavigatorRowLinks {
            session_id: Some(session.id.clone()),
            task_id: None,
            turn_id,
        },
    )
}

fn activity_navigator_file_change_row(
    session: &SessionView,
    item: &ActivityItem,
    mutation: FileMutationActivity,
    archived_turn_id: Option<String>,
) -> ActivityNavigatorRow {
    let status = activity_navigator_activity_status(item);
    let turn_id = archived_turn_id.or_else(|| item.turn_id.as_ref().map(|turn| turn.0.to_string()));
    let badge = diff_file_type_badge(&mutation.path);
    let preview = if mutation.preview_ready {
        "diff preview ready"
    } else {
        "diff preview pending"
    };
    let mut detail = vec![
        format!("session: {}", session.id.0),
        format!("file: {}", mutation.path),
        format!("type: {badge}"),
        format!("operation: {}", mutation.operation),
        format!("preview: {preview}"),
        format!("status: {}", item.status),
    ];
    if let Some(turn_id) = turn_id.as_ref() {
        detail.push(format!("turn: {turn_id}"));
    }
    if let Some(item_detail) = item.detail.as_ref() {
        detail.push(format!("detail: {item_detail}"));
    }

    activity_navigator_row(
        ActivityNavigatorRowKind::FileChange,
        status,
        format!(
            "{badge} {} {}",
            mutation.operation,
            compact_file_path(&mutation.path)
        ),
        format!("{badge} · {} · {preview}", mutation.operation),
        detail,
        ActivityNavigatorRowLinks {
            session_id: Some(session.id.clone()),
            task_id: None,
            turn_id,
        },
    )
}

fn activity_navigator_activity_status(item: &ActivityItem) -> ActivityNavigatorStatus {
    let status = item.status.to_ascii_lowercase();
    if item.kind == ActivityKind::Approval {
        ActivityNavigatorStatus::Blocked
    } else if item.kind == ActivityKind::Error
        || item.success == Some(false)
        || matches!(
            status.as_str(),
            "failed" | "error" | "cancelled" | "canceled"
        )
    {
        ActivityNavigatorStatus::Failed
    } else if crate::model::activity_status_is_running(&item.status) {
        ActivityNavigatorStatus::Running
    } else {
        ActivityNavigatorStatus::Done
    }
}

fn activity_belongs_to_session(
    app: &AppState,
    item: &ActivityItem,
    session_id: &SessionKey,
) -> bool {
    if item.session_id.as_ref() == Some(session_id) {
        return true;
    }
    if let Some(turn_id) = item.turn_id.as_ref() {
        return app
            .turn_activity_logs
            .iter()
            .any(|log| &log.session_id == session_id && log.turn_id == *turn_id)
            || app
                .active_session()
                .is_some_and(|session| &session.id == session_id);
    }
    app.active_session()
        .is_some_and(|session| &session.id == session_id)
}

fn activity_navigator_row(
    kind: ActivityNavigatorRowKind,
    status: ActivityNavigatorStatus,
    title: String,
    subtitle: String,
    detail_lines: Vec<String>,
    links: ActivityNavigatorRowLinks,
) -> ActivityNavigatorRow {
    let mut search_text = format!("{} {} {} {}", kind.label(), status.label(), title, subtitle);
    for detail in &detail_lines {
        search_text.push(' ');
        search_text.push_str(detail);
    }
    if let Some(session_id) = links.session_id.as_ref() {
        search_text.push(' ');
        search_text.push_str(&session_id.0);
    }
    if let Some(task_id) = links.task_id.as_ref() {
        search_text.push(' ');
        search_text.push_str(&task_id.0.to_string());
    }
    if let Some(turn_id) = links.turn_id.as_ref() {
        search_text.push(' ');
        search_text.push_str(turn_id);
    }
    search_text = search_text.to_ascii_lowercase();

    ActivityNavigatorRow {
        kind,
        status,
        title,
        subtitle,
        detail_lines,
        session_id: links.session_id,
        task_id: links.task_id,
        turn_id: links.turn_id,
        search_text: search_text.to_ascii_lowercase(),
    }
}

pub fn chat_layout_areas(app: &AppState, area: Rect) -> ChatLayoutAreas {
    let active_menu = active_menu_surface(app);
    chat_layout_areas_for_menu(app, area, active_menu.as_ref())
}

fn chat_layout_areas_for_menu(
    app: &AppState,
    area: Rect,
    active_menu: Option<&menu_render::MenuSurface>,
) -> ChatLayoutAreas {
    let composer_height = composer_height_for_size(app, area.width, area.height);
    let desired_menu_height = menu_height_hint(active_menu, area.width, area.height);
    let autonomy_height = autonomy_indicator_height(app);
    let harness_height = harness_status_height(app);
    let surface_budget = area.height.saturating_sub(
        min_transcript_height(area.height) + composer_height + autonomy_height + harness_height + 1,
    );
    let menu_height = desired_menu_height.min(surface_budget);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(menu_height),
            Constraint::Length(autonomy_height),
            Constraint::Length(harness_height),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);

    ChatLayoutAreas {
        transcript: root[0],
        menu: root[1],
        autonomy: root[2],
        harness: root[3],
        composer: root[4],
        status: root[5],
    }
}

/// OCTOS figlet wordmark shown in the MAIN window on the first-launch
/// onboarding entry screen (it used to live in a right-side preview pane).
const ONBOARDING_LOGO_ART: &str = "\
 ██████╗  ██████╗████████╗ ██████╗ ███████╗
██╔═══██╗██╔════╝╚══██╔══╝██╔═══██╗██╔════╝
██║   ██║██║        ██║   ██║   ██║███████╗
██║   ██║██║        ██║   ██║   ██║╚════██║
╚██████╔╝╚██████╗   ██║   ╚██████╔╝███████║
 ╚═════╝  ╚═════╝   ╚═╝    ╚═════╝ ╚══════╝";

/// Display width of the figlet wordmark (max over its lines), measured with
/// `unicode-width` so the box-drawing glyphs are counted by display columns.
fn onboarding_logo_art_width() -> usize {
    ONBOARDING_LOGO_ART
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

/// UX2 A.1: rows to spend on the OCTOS banner HEADER across the top of every
/// onboarding step. Taken ONLY from the surplus above what the menu itself
/// needs (`menu_needed`) so the step list, its inputs, and the explanation pane
/// are never clipped on short terminals. Full bordered figlet box when there is
/// room, else a compact one-line bordered tagline box, else nothing.
///
/// Layout (full): top border + blank + 6 art rows + blank + tagline + bottom
/// border = 11 rows. Compact: top border + tagline + bottom border = 3 rows.
fn onboarding_header_height(area_height: u16, area_width: u16, menu_needed: u16) -> u16 {
    let art_width = onboarding_logo_art_width() as u16;
    let surplus = area_height.saturating_sub(menu_needed);
    if area_width >= art_width + 4 && surplus >= 11 {
        11
    } else if surplus >= 3 {
        3
    } else {
        0
    }
}

/// UX2 A.1: render the OCTOS wordmark as a bordered window/header spanning the
/// top of the onboarding screen. `height >= 11` draws the full figlet; a
/// shorter box draws just the tagline. The box content is centered using
/// `unicode-width` column math so the CJK tagline and the box-drawing art stay
/// aligned. Mirrors `render_launch_banner`'s centering primitive.
fn render_onboarding_header(area: Rect, palette: Palette) -> Paragraph<'static> {
    let width = area.width as usize;
    if width < 4 {
        return Paragraph::new(Text::default());
    }
    let inner_w = width - 2;
    let border = Style::default().fg(palette.frame);
    let accent = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let highlight = Style::default().fg(palette.highlight);

    // `│` + centered content (display width `content_w`) + `│`.
    let centered = |content: Vec<Span<'static>>, content_w: usize| -> Line<'static> {
        let pad = inner_w.saturating_sub(content_w);
        let left = pad / 2;
        let right = pad - left;
        let mut spans = vec![Span::styled("│", border), Span::raw(" ".repeat(left))];
        spans.extend(content);
        spans.push(Span::raw(" ".repeat(right)));
        spans.push(Span::styled("│", border));
        Line::from(spans)
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("╭", border),
        Span::styled(format!("{}╮", "─".repeat(inner_w)), border),
    ]));
    let show_figlet = area.height >= 11 && inner_w >= onboarding_logo_art_width();
    if show_figlet {
        let fig_w = onboarding_logo_art_width();
        lines.push(centered(vec![], 0));
        for art in ONBOARDING_LOGO_ART.lines() {
            // Pad each art line to the wordmark width so all rows align inside
            // the box regardless of trailing-space trimming.
            let pad_cols = fig_w.saturating_sub(art.width());
            lines.push(centered(
                vec![Span::styled(
                    format!("{art}{}", " ".repeat(pad_cols)),
                    accent,
                )],
                fig_w,
            ));
        }
        lines.push(centered(vec![], 0));
    }
    let tagline = t!("app.banner.title").into_owned();
    let tagline_width = tagline.width();
    lines.push(centered(
        vec![Span::styled(tagline, highlight)],
        tagline_width,
    ));
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(inner_w)),
        border,
    )));
    Paragraph::new(Text::from(lines))
}

fn render_onboarding_first_launch_layout(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
) {
    let composer_height = composer_height_for_size(app, frame.area().width, frame.area().height);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let menu = active_menu_surface(app);
    // UX2 A.1: three-region onboarding layout. TOP = the OCTOS banner header
    // (shown on EVERY step, not just the welcome screen); MAIN = the wizard menu
    // (the numbered step list + the active step's inputs/rows on the left); RIGHT
    // = the per-step explanation/teaching panel, carried as the menu's preview so
    // the selection view renders it beside the items on wide terminals. Header
    // rows come only from the surplus above the menu's own needs, so the steps
    // and the explanation pane are never clipped on short terminals.
    let menu_needed = menu
        .as_ref()
        .map_or(0, |m| menu_render::height_hint(m, root[0].width));
    let header_height = onboarding_header_height(root[0].height, root[0].width, menu_needed);
    let menu_area = if header_height > 0 {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(header_height), Constraint::Min(0)])
            .split(root[0]);
        frame.render_widget(render_onboarding_header(split[0], palette), split[0]);
        split[1]
    } else {
        root[0]
    };

    if let Some(menu) = menu.as_ref() {
        menu_render::render_menu_surface(frame, menu_area, menu, palette);
    }
    frame.render_widget(render_composer(app, palette, root[1]), root[1]);
    set_composer_cursor(frame, app, root[1]);
    frame.render_widget(render_status(app, palette), root[2]);
}

fn onboarding_first_launch_active(app: &AppState) -> bool {
    app.sessions.is_empty()
        && app.menu_stack.active().is_some_and(|frame| {
            matches!(
                frame.id.as_str(),
                crate::menu::registry::MENU_ONBOARD
                    | crate::menu::registry::MENU_ONBOARD_LANGUAGE
                    | crate::menu::registry::MENU_ONBOARD_FAMILY
                    | crate::menu::registry::MENU_ONBOARD_MODEL
                    | crate::menu::registry::MENU_ONBOARD_ROUTE
                    | crate::menu::registry::MENU_ONBOARD_WORKSPACE
            )
        })
}

fn min_transcript_height(terminal_height: u16) -> u16 {
    if terminal_height < 30 { 8 } else { 12 }
}

fn render_inspector_layout(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    let composer_height = composer_height_for_size(app, frame.area().width, frame.area().height);
    let active_menu = active_menu_surface(app);
    let menu_height = menu_height_hint(
        active_menu.as_ref(),
        frame.area().width,
        frame.area().height,
    );
    let autonomy_height = autonomy_indicator_height(app);
    let harness_height = harness_status_height(app);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(12),
            Constraint::Length(menu_height),
            Constraint::Length(autonomy_height),
            Constraint::Length(harness_height),
            Constraint::Length(composer_height),
            Constraint::Length(4),
        ])
        .split(frame.area());

    let upper = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(32),
            Constraint::Min(44),
            Constraint::Length(38),
        ])
        .split(root[0]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(32),
            Constraint::Percentage(36),
            Constraint::Percentage(32),
        ])
        .split(upper[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(44),
            Constraint::Percentage(30),
        ])
        .split(upper[2]);

    frame.render_widget(render_sessions(app, palette), left[0]);
    frame.render_widget(render_tasks(app, palette), left[1]);
    frame.render_widget(render_artifacts(app, palette), left[2]);
    frame.render_widget(render_transcript(app, palette, upper[1]), upper[1]);
    frame.render_widget(render_plan(app, palette), right[0]);
    frame.render_widget(render_workspace(app, palette, right[1].height), right[1]);
    frame.render_widget(render_git(app, palette, right[2].height), right[2]);
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, root[1], menu, palette);
    }
    if autonomy_height > 0 {
        frame.render_widget(render_autonomy_indicator(app, palette), root[2]);
    }
    if harness_height > 0 {
        render_harness_status_row(frame, app, palette, root[3]);
    }
    frame.render_widget(render_composer(app, palette, root[4]), root[4]);
    set_composer_cursor(frame, app, root[4]);
    frame.render_widget(render_status(app, palette), root[5]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityNavigatorAreas {
    pub toolbar: Rect,
    pub list: Rect,
    pub detail: Rect,
    pub hint: Rect,
}

pub fn activity_navigator_areas(area: Rect) -> ActivityNavigatorAreas {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(vertical[1]);

    ActivityNavigatorAreas {
        toolbar: vertical[0],
        list: body[0],
        detail: body[1],
        hint: vertical[2],
    }
}

fn render_activity_navigator_overlay(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    let areas = activity_navigator_areas(frame.area());
    let model = activity_navigator_model(app);
    frame.render_widget(Clear, frame.area());
    frame.render_widget(
        render_activity_navigator_toolbar(&model, palette),
        areas.toolbar,
    );
    let mut list_state = ListState::default().with_selected(Some(model.selected));
    StatefulWidget::render(
        render_activity_navigator_list(&model, palette),
        areas.list,
        frame.buffer_mut(),
        &mut list_state,
    );
    frame.render_widget(
        render_activity_navigator_detail(&model, palette),
        areas.detail,
    );
    frame.render_widget(
        Paragraph::new(hint_bar_text(HintBarModel {
            mode: HintBarMode::ActivityNavigator,
        }))
        .style(Style::default().fg(palette.text).bg(palette.surface_alt)),
        areas.hint,
    );
}

fn render_activity_navigator_toolbar(
    model: &ActivityNavigatorModel,
    palette: Palette,
) -> Paragraph<'static> {
    let search_label = if model.search_active {
        "search*: "
    } else {
        "query: "
    };
    let query = if model.query.is_empty() {
        "(empty)".to_string()
    } else {
        model.query.clone()
    };
    let counts = format!(
        "all {} | changes {} | running {} | blocked {} | failed {} | done {}",
        model.counts.all,
        model.counts.changes,
        model.counts.running,
        model.counts.blocked,
        model.counts.failed,
        model.counts.done
    );
    Paragraph::new(Text::from(vec![
        Line::from(vec![
            Span::styled("Activity", palette.title()),
            Span::styled(" navigator", palette.text()),
            Span::styled(
                format!("  filter: {}", model.filter.label()),
                palette.muted(),
            ),
        ]),
        Line::from(vec![
            Span::styled(search_label, palette.muted()),
            Span::styled(query, palette.text()),
            Span::styled("  |  ", palette.muted()),
            Span::styled(counts, palette.muted()),
        ]),
    ]))
    .block(Block::default().style(Style::default().bg(palette.surface_alt)))
}

fn render_activity_navigator_list(
    model: &ActivityNavigatorModel,
    palette: Palette,
) -> List<'static> {
    let items = if model.rows.is_empty() {
        let detail = if model.query.trim().is_empty() {
            format!("filter: {}", model.filter.label())
        } else {
            format!("query: {}  filter: {}", model.query, model.filter.label())
        };
        vec![ListItem::new(Text::from(vec![
            Line::from(Span::styled("No activity rows match", palette.muted())),
            Line::from(Span::styled(detail, palette.muted())),
        ]))]
    } else {
        model
            .rows
            .iter()
            .enumerate()
            .map(|(idx, row)| {
                let selected = idx == model.selected;
                let style = if selected {
                    palette.selected()
                } else {
                    palette.text()
                };
                let marker = if selected { "›" } else { " " };
                let kind_style = if row.kind == ActivityNavigatorRowKind::FileChange {
                    palette.selected()
                } else {
                    palette.muted()
                };
                ListItem::new(Text::from(vec![
                    Line::from(vec![
                        Span::styled(format!("{marker} "), style),
                        Span::styled(
                            format!("[{}] ", row.status.label()),
                            status_style(row.status, palette),
                        ),
                        Span::styled(row.title.clone(), style),
                    ]),
                    Line::from(vec![
                        Span::styled("  ", palette.muted()),
                        Span::styled(row.kind.label(), kind_style),
                        Span::styled(" · ", palette.muted()),
                        Span::styled(row.subtitle.clone(), palette.muted()),
                    ]),
                ]))
            })
            .collect()
    };

    List::new(items).highlight_style(Style::default()).block(
        titled_block(
            "Results".to_string(),
            palette,
            true,
            Some("j/k".to_string()),
        )
        .border_style(palette.border()),
    )
}

fn render_activity_navigator_detail(
    model: &ActivityNavigatorModel,
    palette: Palette,
) -> Paragraph<'static> {
    let lines = if let Some(row) = model.selected_row() {
        let mut lines = vec![
            Line::from(Span::styled(row.title.clone(), palette.title())),
            Line::from(vec![
                Span::styled(row.kind.label(), palette.muted()),
                Span::styled(" · ", palette.muted()),
                Span::styled(row.status.label(), status_style(row.status, palette)),
            ]),
            Line::from(Span::raw("")),
        ];
        lines.extend(
            row.detail_lines
                .iter()
                .map(|line| Line::from(Span::styled(line.clone(), palette.text()))),
        );
        lines
    } else {
        vec![Line::from(Span::styled(
            "No activity selected",
            palette.muted(),
        ))]
    };

    Paragraph::new(Text::from(lines))
        .block(
            titled_block("Detail".to_string(), palette, false, None).border_style(palette.border()),
        )
        .wrap(Wrap { trim: false })
}

fn status_style(status: ActivityNavigatorStatus, palette: Palette) -> Style {
    match status {
        ActivityNavigatorStatus::Running => palette.selected(),
        ActivityNavigatorStatus::Blocked => Style::default().fg(palette.highlight),
        ActivityNavigatorStatus::Failed => Style::default().fg(palette.danger),
        ActivityNavigatorStatus::Done => Style::default().fg(palette.success),
    }
}

fn active_menu_surface(app: &AppState) -> Option<menu_render::MenuSurface> {
    let frame = app.menu_stack.active();
    let stack_path = app
        .menu_stack
        .frames()
        .iter()
        .map(|frame| frame.id.to_string())
        .collect::<Vec<_>>();
    match app.active_menu.as_ref()? {
        crate::menu::MenuBuildResult::Ready(spec) => {
            Some(menu_render::MenuSurface::from_spec(spec, frame, stack_path))
        }
        crate::menu::MenuBuildResult::Loading(status)
        | crate::menu::MenuBuildResult::Unavailable(status)
        | crate::menu::MenuBuildResult::Error(status) => {
            Some(menu_render::MenuSurface::from_status(status, stack_path))
        }
    }
}

fn menu_height_hint(
    menu: Option<&menu_render::MenuSurface>,
    terminal_width: u16,
    terminal_height: u16,
) -> u16 {
    let Some(menu) = menu else {
        return 0;
    };
    let max_height = terminal_height.saturating_sub(15);
    if max_height == 0 {
        return 0;
    }
    menu_render::height_hint(menu, terminal_width)
        .min(max_height)
        .max(4.min(max_height))
}

/// Menu height for the INLINE VIEWPORT render pass. `menu_height_hint` budgets
/// against the full TERMINAL height (its `-15` heuristic reserves scrollback
/// rows) and sizes the viewport accordingly; re-applying that heuristic to the
/// viewport's own (much smaller) height collapsed the menu to zero rows — the
/// slash popup's space was reserved but rendered blank once the activity
/// collapse made viewports short. Here the menu simply takes its desired
/// height, clamped to the room the viewport actually has.
fn menu_height_for_viewport(
    menu: Option<&menu_render::MenuSurface>,
    viewport_width: u16,
    available: u16,
) -> u16 {
    let Some(menu) = menu else {
        return 0;
    };
    if available == 0 {
        return 0;
    }
    menu_render::height_hint(menu, viewport_width)
        .min(available)
        .max(4.min(available))
}

const COMPOSER_CHROME_ROWS: u16 = 4;
const COMPOSER_MIN_HEIGHT: u16 = 5;
const COMPOSER_MAX_INPUT_ROWS: u16 = 12;
const COMPOSER_SIDE_COLUMNS: u16 = 6;

#[cfg(test)]
fn composer_height(app: &AppState) -> u16 {
    composer_height_for_size(app, 120, 42)
}

fn composer_height_for_size(app: &AppState, terminal_width: u16, terminal_height: u16) -> u16 {
    match app.composer_presentation() {
        ComposerPresentation::Inline(text) => {
            COMPOSER_CHROME_ROWS
                + composer_visible_input_rows(&text, terminal_width, terminal_height)
        }
        ComposerPresentation::Empty | ComposerPresentation::Collapsed(_) => COMPOSER_MIN_HEIGHT,
    }
}

fn composer_input_row_cap(terminal_height: u16) -> u16 {
    terminal_height
        .saturating_sub(12)
        .saturating_div(2)
        .clamp(3, COMPOSER_MAX_INPUT_ROWS)
}

fn composer_text_width(terminal_width: u16) -> usize {
    usize::from(terminal_width.saturating_sub(COMPOSER_SIDE_COLUMNS).max(1))
}

fn composer_visible_input_rows(text: &str, terminal_width: u16, terminal_height: u16) -> u16 {
    let width = composer_text_width(terminal_width);
    let rows = text
        .split('\n')
        .map(|line| visual_rows_for_text(line, width))
        .sum::<usize>()
        .max(1);
    rows.min(usize::from(composer_input_row_cap(terminal_height))) as u16
}

fn visual_rows_for_text(text: &str, width: usize) -> usize {
    // Derived from the wrap so the rows reserved here always equal the rows
    // actually drawn by render_composer (and the rows the cursor math counts).
    wrap_composer_line(text, width).len()
}

/// Split one logical composer line into the visual sub-lines it occupies, each
/// fitting within `width` display columns. The `Paragraph` that draws the
/// composer has no soft-wrap, so without this the overflow of a long line is
/// clipped at the pane edge and its reserved continuation row renders blank
/// ("dark/invisible").
///
/// Packing is by grapheme cluster measured with `UnicodeWidthStr::width` (the
/// same primitive `str::width()` uses), so a multi-codepoint glyph (CJK, emoji
/// ZWJ/modifier/variation sequences) is never split across a row boundary, and
/// the chunk count is the authoritative visual-row count (`visual_rows_for_text`
/// delegates here) — keeping reserved height, rendered rows, and cursor row in
/// agreement for every input. Always returns at least one (possibly empty)
/// chunk so an empty logical line still occupies a row.
fn wrap_composer_line(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    for grapheme in text.graphemes(true) {
        let g_w = grapheme.width();
        if current_w + g_w > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_w = 0;
        }
        current.push_str(grapheme);
        current_w += g_w;
    }
    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

const CODE_BLOCK_LINE_LIMIT: usize = 120;
const COLLAPSED_TOOL_PREVIEW_LINES: usize = 1;
const EXPANDED_TOOL_PREVIEW_LINES: usize = 24;

/// True while an activity is genuinely in-flight. Thin wrapper over the shared
/// [`crate::model::activity_status_is_running`] running-status set so the
/// renderer's chip "active" count and the orphan activity-chip self-heal in
/// [`crate::model::AppState::capture_completed_turn_activity`] stay in lockstep.
/// Sub-agent liveness is tracked separately via the task count
/// ([`running_subagent_titles_for_chip`]).
fn is_running_activity(item: &ActivityItem) -> bool {
    crate::model::activity_status_is_running(&item.status)
}

fn render_sessions(app: &AppState, palette: Palette) -> List<'static> {
    let items = app
        .sessions
        .iter()
        .enumerate()
        .map(|(idx, session)| {
            let marker = if idx == app.selected_session {
                "›"
            } else {
                " "
            };
            let profile = session.profile_id.as_deref().unwrap_or("default");
            let style = if idx == app.selected_session {
                palette.selected()
            } else {
                palette.text()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(session.title.clone(), style),
                Span::styled(format!("  [{profile}]"), palette.muted()),
            ]))
        })
        .collect::<Vec<_>>();

    List::new(items).block(
        titled_block(
            t!("app.pane.sessions").to_string(),
            palette,
            app.focus == FocusPane::Sessions,
            Some("Tab".to_string()),
        )
        .border_style(palette.border()),
    )
}

fn render_tasks(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if let Some(session) = app.active_session() {
        if session.tasks.is_empty() {
            lines.push(Line::from(Span::styled(
                t!("app.empty.no_tasks").to_string(),
                palette.muted(),
            )));
        } else {
            for (idx, task) in session.tasks.iter().enumerate() {
                let marker = if idx == app.selected_task { "›" } else { " " };
                let style = if idx == app.selected_task {
                    palette.selected()
                } else {
                    palette.text()
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{marker} "), style),
                    Span::styled(task.title.clone(), style),
                    Span::styled(
                        format!("  [{}]", task_state_label(task.state)),
                        palette.muted(),
                    ),
                ]));
                if idx == app.selected_task {
                    if let Some(detail) = &task.runtime_detail {
                        lines.push(Line::from(Span::styled(
                            format!("    {detail}"),
                            palette.muted(),
                        )));
                    }
                    for tail_line in task
                        .output_tail
                        .lines()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        lines.push(Line::from(Span::styled(
                            format!("    {tail_line}"),
                            palette.muted(),
                        )));
                    }
                }
            }
        }
    }

    Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.tasks").to_string(),
                palette,
                app.focus == FocusPane::Tasks,
                Some(t!("app.hint.list_nav").into_owned()),
            )
            .border_style(palette.border()),
        )
        .wrap(Wrap { trim: false })
}

fn render_artifacts(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mut lines = Vec::new();

    if app.artifacts.items.is_empty() {
        lines.push(Line::from(Span::styled(
            t!("app.empty.no_artifacts").to_string(),
            palette.muted(),
        )));
    } else {
        for (idx, item) in app.artifacts.items.iter().enumerate() {
            let marker = if idx == app.artifacts.selected {
                "›"
            } else {
                " "
            };
            let style = if idx == app.artifacts.selected {
                palette.selected()
            } else {
                palette.text()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(item.title.clone(), style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(format!("    {}", item.kind), palette.muted()),
                Span::styled("  ", palette.muted()),
                Span::styled(item.status.clone(), palette.muted()),
            ]));
            if idx == app.artifacts.selected {
                lines.push(Line::from(Span::styled(
                    format!("    {}", t!("app.artifact.from", source = item.source)),
                    palette.muted(),
                )));
            }
        }
    }

    Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.artifacts").to_string(),
                palette,
                app.focus == FocusPane::Artifacts,
                Some("j/k".to_string()),
            )
            .border_style(palette.border()),
        )
        .wrap(Wrap { trim: false })
}

/// True for a fresh session that has no messages yet — where we show the launch
/// banner at the top of the transcript area (it scrolls away on the first turn).
fn launch_banner_active(app: &AppState) -> bool {
    app.pending_messages.is_empty()
        && app
            .active_session()
            .is_some_and(|session| session.messages.is_empty() && session.live_reply.is_none())
}

/// Claude-Code-style launch banner: a rounded box with the OCTOS logo, a
/// greeting, and the workspace path. No right-hand panel (per product call).
/// Rendered at the TOP of the transcript area for an empty session.
fn render_launch_banner(frame: &mut impl FrameLike, app: &AppState, palette: Palette, area: Rect) {
    let width = area.width as usize;
    if width < 12 || area.height < 6 {
        return;
    }
    let inner_w = width - 2;
    let show_figlet = area.width >= 48 && area.height >= 14;
    let border = Style::default().fg(palette.frame);
    let accent = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let highlight = Style::default()
        .fg(palette.highlight)
        .add_modifier(Modifier::BOLD);

    // A content row: `│` + centered content (display width `content_w`) + `│`.
    let centered = |content: Vec<Span<'static>>, content_w: usize| -> Line<'static> {
        let pad = inner_w.saturating_sub(content_w);
        let left = pad / 2;
        let right = pad - left;
        let mut spans = vec![Span::styled("│", border), Span::raw(" ".repeat(left))];
        spans.extend(content);
        spans.push(Span::raw(" ".repeat(right)));
        spans.push(Span::styled("│", border));
        Line::from(spans)
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    // Top border with an embedded title.
    let title = "─ octos ─";
    let top_dashes = inner_w.saturating_sub(title.chars().count());
    lines.push(Line::from(vec![
        Span::styled("╭", border),
        Span::styled(title, accent),
        Span::styled(format!("{}╮", "─".repeat(top_dashes)), border),
    ]));
    lines.push(centered(vec![], 0));
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
    let greeting = match app
        .active_session()
        .and_then(|session| session.profile_id.as_deref())
    {
        Some(profile) => t!("app.banner.greeting_named", profile = profile).to_string(),
        None => t!("app.banner.greeting_default").to_string(),
    };
    let greeting_w = greeting.width();
    lines.push(centered(
        vec![Span::styled(greeting, highlight)],
        greeting_w,
    ));
    let cwd = short_path(app.workspace.root.as_str());
    let cwd_w = cwd.width();
    lines.push(centered(vec![Span::styled(cwd, palette.muted())], cwd_w));
    lines.push(centered(vec![], 0));
    let hint = t!("app.banner.hint").to_string();
    let hint_w = hint.width();
    lines.push(centered(vec![Span::styled(hint, palette.muted())], hint_w));
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(inner_w)),
        border,
    )));

    let banner_height = (lines.len() as u16).min(area.height);
    let banner_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: banner_height,
    };
    frame.render_widget(Paragraph::new(Text::from(lines)), banner_area);
}

struct TranscriptRenderModel {
    paragraph: Paragraph<'static>,
    metrics: TranscriptScrollMetrics,
}

fn render_transcript(app: &AppState, palette: Palette, area: Rect) -> Paragraph<'static> {
    transcript_render_model(app, palette, area).paragraph
}

fn transcript_render_model(app: &AppState, palette: Palette, area: Rect) -> TranscriptRenderModel {
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
                push_turn_activity_log_section(&mut lines, palette, log, app, true, wrap_width);
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

const PAGER_SCROLLBAR_TRACK: &str = "│";
const PAGER_SCROLLBAR_THUMB: &str = "█";

fn render_pager_scrollbar(
    frame: &mut impl FrameLike,
    metrics: TranscriptScrollMetrics,
    area: Rect,
    palette: Palette,
) {
    let Some(track) = pager_scrollbar_track(area) else {
        return;
    };
    let Some(thumb) = scrollbar_thumb(metrics, track) else {
        return;
    };

    let buffer = frame.buffer_mut();
    let thumb_bottom = thumb.top.saturating_add(thumb.height);
    for y in track.y..track.y.saturating_add(track.height) {
        let in_thumb = y >= thumb.top && y < thumb_bottom;
        let cell = &mut buffer[(track.x, y)];
        if in_thumb {
            cell.set_symbol(PAGER_SCROLLBAR_THUMB);
            cell.set_style(palette.title());
        } else {
            cell.set_symbol(PAGER_SCROLLBAR_TRACK);
            cell.set_style(palette.muted());
        }
    }
}

fn pager_scrollbar_track(area: Rect) -> Option<Rect> {
    if area.width < 2 || area.height == 0 {
        return None;
    }

    Some(Rect::new(
        area.x + area.width.saturating_sub(1),
        area.y,
        1,
        area.height,
    ))
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
fn transcript_visible_height(area: Rect) -> usize {
    usize::from(area.height).max(1)
}

fn transcript_wrap_width(area: Rect) -> usize {
    usize::from(area.width.saturating_sub(2)).max(1)
}

fn transcript_visual_rows(lines: &[Line<'static>], wrap_width: usize) -> usize {
    lines
        .iter()
        .map(|line| transcript_line_visual_rows(line, wrap_width))
        .sum()
}

fn transcript_line_visual_rows(line: &Line<'static>, wrap_width: usize) -> usize {
    let width = line
        .spans
        .iter()
        .map(|span| span.content.as_ref().width())
        .sum::<usize>();
    width.max(1).div_ceil(wrap_width.max(1))
}

fn latest_user_message(session: &SessionView) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role.as_str() == "user")
        .map(|message| message.content.as_str())
        .filter(|content| !content.trim().is_empty())
}

fn pending_messages_contains(pending: &[String], content: &str) -> bool {
    pending.iter().any(|pending| pending == content)
}

fn anchored_turn_activity_logs<'a>(
    app: &'a AppState,
    session: &'a SessionView,
) -> Vec<(usize, &'a TurnActivityLog)> {
    app.turn_activity_logs
        .iter()
        .filter(|log| log.session_id == session.id)
        .filter_map(|log| {
            let anchor_index = log
                .anchor_index
                .filter(|idx| user_message_at(session, *idx))
                .or_else(|| {
                    log.request.as_ref().and_then(|request| {
                        session.messages.iter().rposition(|message| {
                            message.role.as_str() == "user" && message.content == *request
                        })
                    })
                })?;
            Some((activity_log_render_index(session, anchor_index), log))
        })
        .collect()
}

fn activity_log_render_index(session: &SessionView, anchor_index: usize) -> usize {
    session
        .messages
        .iter()
        .enumerate()
        .skip(anchor_index.saturating_add(1))
        .take_while(|(_, message)| message.role.as_str() != "user")
        .find(|(_, message)| message.role.as_str() == "assistant")
        .map(|(idx, _)| idx)
        .unwrap_or(anchor_index)
}

fn user_message_at(session: &SessionView, idx: usize) -> bool {
    session
        .messages
        .get(idx)
        .is_some_and(|message| message.role.as_str() == "user")
}

fn live_reply_coverage_matches_message(
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

fn committed_reply_index_for_live_finalization(
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

fn resolve_turn_prompt_anchor_for_render(
    session: &SessionView,
    anchor: &TurnPromptAnchor,
) -> Option<usize> {
    if session
        .messages
        .get(anchor.anchor_index)
        .is_some_and(|message| message.role.as_str() == "user" && message.content == anchor.content)
    {
        return Some(anchor.anchor_index);
    }

    session
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role.as_str() == "user" && message.content == anchor.content)
        .nth(anchor.prior_matching_user_count)
        .map(|(idx, _)| idx)
}

fn should_pin_recent_user_context(app: &AppState, session: &SessionView) -> bool {
    session.live_reply.is_some()
        || live_turn_diff_preview_visible(app)
        || app.active_turn().is_some()
        || app.run_state.is_active()
}

fn should_show_turn_flow(app: &AppState, session: &SessionView) -> bool {
    app.approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
        || app
            .user_question
            .as_ref()
            .is_some_and(|picker| picker.visible)
        // NB: a `/btw` aside no longer forces the turn flow — it renders as a
        // floating top overlay (`render_btw_overlay`) so it doesn't mingle.
        || should_pin_recent_user_context(app, session)
}

fn push_turn_flow(
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

    if let Some(approval) = app.approval.as_ref().filter(|approval| approval.visible) {
        push_inline_approval_card(lines, palette, approval);
    }

    if let Some(picker) = app.user_question.as_ref().filter(|picker| picker.visible) {
        push_inline_user_question_card(lines, palette, picker, width);
    }

    // `/btw` aside renders as a floating overlay pinned to the TOP of the live
    // viewport (see `render_btw_overlay`), not inline here — otherwise it
    // mingles with the streaming reply/activity below it.

    // Live reasoning for the active turn: codex-style, we DON'T render the
    // verbose "thinking" text. The deltas still accumulate in `live_reasoning`
    // (so a future /thinking toggle can reveal them and commit_live_reply can
    // hand them to the message's reasoning_content); we only surface a single
    // dimmed swimming-octopus indicator, and ONLY while the model is still
    // reasoning — once the answer has started streaming (`live_reply.text` has
    // non-empty content for the active turn) we drop the indicator too.
    if let Some((session_id, turn_id)) = app.active_turn()
        && app
            .live_reasoning
            .get(&(session_id.clone(), turn_id.clone()))
            .is_some_and(|reasoning| !reasoning.trim().is_empty())
        && session
            .live_reply
            .as_ref()
            .is_none_or(|live_reply| live_reply.text.trim().is_empty())
    {
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

    push_activity_section_with_finalization(lines, palette, app, live_finalization, width);

    if live_turn_diff_preview_visible(app) {
        push_inline_diff_preview(lines, palette, &app.diff_preview, app.expanded_tool_outputs);
    }
}

fn live_turn_diff_preview_visible(app: &AppState) -> bool {
    if !app.diff_preview.active {
        return false;
    }
    let Some(diff_turn_id) = app.diff_preview.turn_id.as_ref() else {
        return true;
    };
    app.active_turn()
        .is_some_and(|(_, active_turn_id)| active_turn_id == diff_turn_id)
}

fn push_recent_user_context(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    _width: usize,
) {
    push_user_message_block(lines, palette, content);
}

/// User input gets the role-contrast treatment: an accent-colored `▌` gutter
/// on every logical line plus a bold body. It is the single strongest visual
/// anchor in the transcript (scanning for "what did I say" is the most common
/// review motion), works without any background color (backgrounds are
/// unreliable in the pager, the terminal theme, and native scrollback), and
/// echoes the input verbatim — user text is a quote, not a markdown document.
fn push_user_message_block(lines: &mut Vec<Line<'static>>, palette: Palette, content: &str) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    let gutter = Style::default().fg(palette.accent);
    let body = palette.text().add_modifier(Modifier::BOLD);
    if content.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("▌ ", gutter),
            Span::styled("<empty>", palette.muted()),
        ]));
        return;
    }
    for raw_line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("▌ ", gutter),
            Span::styled(raw_line.trim_end().to_string(), body),
        ]));
    }
}

/// A horizontal ASCII octopus that "swims" across the thinking line: a `[⇔]`
/// head flanked by the tilted-line glyphs `彡`/`ミ` (one arm per side). The two
/// frames are alternating paddle *strokes* — the arms flip mirror-image every
/// column step while the octopus ping-pongs left↔right (see [`octopus_swim`]),
/// so it visibly paddles the whole way instead of holding one pose per leg.
///
///   `彡[⇔]ミ` ⇄ `ミ[⇔]彡`
const OCTOPUS_SWIM_FRAMES: [&str; 2] = ["彡[⇔]ミ", "ミ[⇔]彡"];

/// One-way sweep duration: the octopus crosses edge-to-edge in this time
/// REGARDLESS of terminal width. The previous fixed ms-per-column pace made
/// the sweep ~21s one-way on a 146-column pane, so typical thinking phases
/// ended with the octopus visibly stuck around mid-screen ("only went half
/// of the page"). 4s matches the pace the capped sweep used to have.
const OCTOPUS_SWEEP_ONE_WAY_MS: u128 = 4_000;

/// Paddle-stroke cadence — the arms flip mirror-image at this interval,
/// independent of travel position (~3 strokes/sec reads as swimming, not a
/// strobe).
const OCTOPUS_STROKE_MS: u128 = 150;

/// Pure elapsed→(leading-space offset, frame) mapping for the swimming octopus.
///
/// The octopus travels horizontally as a triangle wave: the leading-space
/// offset climbs `0 → MAX` in [`OCTOPUS_SWEEP_ONE_WAY_MS`] then falls back,
/// forever, sweeping the FULL `wrap_width` — `MAX` keeps the octopus plus a
/// one-column right margin inside it, measured in display *columns* via
/// `unicode-width` (the CJK arm glyphs are double-width). Position is
/// time-proportional, so it reaches the far edge every sweep on any width.
/// The paddle frame alternates every [`OCTOPUS_STROKE_MS`] independent of
/// travel. On a terminal too narrow to travel, `MAX` is 0 and the octopus
/// paddles in place at the left margin rather than panicking. All arithmetic
/// is overflow-safe: `offset` is bounded by `MAX`, so the caller's
/// `" ".repeat(offset)` can never run away.
fn octopus_swim(elapsed_ms: u128, wrap_width: usize) -> (usize, &'static str) {
    let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
    let frame = OCTOPUS_SWIM_FRAMES[((elapsed_ms / OCTOPUS_STROKE_MS) % 2) as usize];
    let max = wrap_width.saturating_sub(octopus_width + 1);
    if max == 0 {
        return (0, frame);
    }
    // Triangle wave in TIME (u128 end-to-end so a huge uptime can't
    // truncate): phase ∈ [0, 2·SWEEP), folded to one_way ∈ [0, SWEEP],
    // then scaled onto [0, MAX].
    let cycle_ms = 2 * OCTOPUS_SWEEP_ONE_WAY_MS;
    let phase = elapsed_ms % cycle_ms;
    let one_way = phase.min(cycle_ms - phase);
    let offset = ((one_way * max as u128) / OCTOPUS_SWEEP_ONE_WAY_MS) as usize;
    (offset, frame)
}

/// `▰▰▰▰▱▱▱▱` fixed-width fraction bar for the compaction/context UX.
pub(crate) fn progress_bar(frac: f64, width: usize) -> String {
    let filled = ((frac.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "▰".repeat(filled), "▱".repeat(width - filled))
}

/// The in-progress compaction block (UPCR-2026-026):
/// ```text
/// ✶ Compacting conversation… (12s · 87.4k tokens)
///   ▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱ 49%
/// ```
/// The percentage is honest: pre-compaction tokens over the session's
/// context window (threshold as the fallback denominator).
/// How long the settled "context compacted" block dwells after completion.
/// The server pass is synchronous — started/completed land in one drain batch
/// and draws only follow the batch — so without this dwell the block would
/// paint zero frames, ever.
const LIVE_COMPACTION_SETTLED_DISPLAY_SECS: u64 = 4;

fn push_live_compaction_block(
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
fn push_thinking_indicator(lines: &mut Vec<Line<'static>>, palette: Palette, wrap_width: usize) {
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

/// Push the committed `reasoning_content` as a capped "· reasoning" block,
/// gated on the active session's `/thinking` display toggle. Off by default
/// (codex-style quiet). Capped to the first `REASONING_BLOCK_CAP` lines unless
/// `expanded` (Ctrl+O), with a "+N more" affordance — the same convention as
/// tool output. A no-op when display is off or there is no reasoning.
const REASONING_BLOCK_CAP: usize = 6;

fn push_reasoning_block(
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

fn push_message_block(
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

/// Render a (chunk of a) streaming reply. `first` controls the `• ` prose
/// marker: a reply flushed across several scrollback batches must carry the
/// bullet exactly once — on its first batch — or the transcript reads as
/// several separate replies.
fn push_live_reply_block(
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
    push_formatted_body_marked(lines, palette, content, "", marker, Some(bg), width);
}

fn push_live_reply_block_seeded(
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
        "",
        marker,
        Some(bg),
        width,
        previous_reply_has_output,
        previous_reply_ends_blank,
    );
}

fn seeded_live_reply_content_can_emit(
    content: &str,
    previous_reply_has_output: bool,
    previous_reply_ends_blank: bool,
) -> bool {
    !content.trim().is_empty()
        || (previous_reply_has_output
            && !previous_reply_ends_blank
            && content.contains('\n')
            && content.lines().any(|line| line.trim().is_empty()))
}

fn live_reply_prefix_ends_blank(palette: Palette, content: &str, width: usize) -> bool {
    if content.trim().is_empty() {
        return false;
    }
    let mut lines = Vec::new();
    push_live_reply_block(&mut lines, palette, content, width, true);
    lines.last().is_some_and(|line| line_is_blank(Some(line)))
}

fn push_pending_messages_block(
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
fn push_code_block_lines(
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

fn code_block_is_unified_diff(language: &str, body: &[String]) -> bool {
    let language = language.trim().to_ascii_lowercase();
    if matches!(
        language.as_str(),
        "diff" | "patch" | "udiff" | "unidiff" | "gitdiff"
    ) {
        return true;
    }

    if !language.is_empty() && language != "code" {
        return false;
    }

    let mut has_hunk_or_file_header = false;
    let mut has_added = false;
    let mut has_removed = false;

    for line in body {
        let trimmed = line.trim_start();
        if trimmed.starts_with("@@")
            || trimmed.starts_with("diff --git")
            || trimmed.starts_with("index ")
            || trimmed.starts_with("--- ")
            || trimmed.starts_with("+++ ")
        {
            has_hunk_or_file_header = true;
        }
        if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
            has_added = true;
        } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
            has_removed = true;
        }
    }

    has_hunk_or_file_header && (has_added || has_removed)
}

fn retitle_last_code_block_header_as_diff(lines: &mut [Line<'static>]) {
    let Some(line) = lines.last_mut() else {
        return;
    };
    let Some(label) = line.spans.last_mut() else {
        return;
    };
    if label.content.as_ref() == "code" {
        label.content = "diff".into();
    }
}

fn push_unified_diff_code_block_lines(
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

fn chat_message_bg(palette: Palette, role: &str) -> Color {
    match role {
        "user" => palette.diff_context_bg,
        "assistant" => palette.surface,
        "reasoning" => palette.surface,
        "btw" => palette.surface,
        "tool" => palette.surface,
        _ => palette.surface,
    }
}

fn push_formatted_body(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    indent: &'static str,
    bg: Option<Color>,
    width: usize,
) {
    push_formatted_body_marked(lines, palette, content, indent, None, bg, width);
}

fn push_formatted_body_marked(
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

fn push_formatted_body_marked_seeded(
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
    let normalized = content.trim_matches(|ch: char| ch.is_whitespace() && ch != '\n');

    for raw_line in normalized.lines() {
        let line = if in_code.is_some() {
            raw_line
        } else {
            raw_line.trim()
        };
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            checkbox_index = 1;
            if !last_blank && (previous_reply_has_output || !lines.is_empty()) {
                lines.push(chat_line(
                    vec![Span::styled(indent, style_bg(palette.border(), bg))],
                    bg,
                ));
                last_blank = true;
            }
            continue;
        }
        last_blank = false;

        if let Some(command) = shell_command_from_line(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            push_command_row(lines, palette, indent, command);
            continue;
        }

        if markdown_table_separator(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            continue;
        }

        if let Some(cells) = markdown_table_cells(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            table.push(cells.into_iter().map(str::to_owned).collect());
            continue;
        }

        if markdown_hr(line) {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
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
    flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
    flush_markdown_table(lines, palette, &mut table, indent, bg, width);
}

fn flush_prose_paragraph(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    prose: &mut Vec<String>,
    indent: &'static str,
    prose_marker: Option<&'static str>,
    bg: Option<Color>,
) {
    if prose.is_empty() {
        return;
    }

    let paragraph = prose.join(" ");
    let mut spans = vec![Span::styled(indent, style_bg(palette.border(), bg))];
    if let Some(marker) = prose_marker {
        spans.push(Span::styled(marker, style_bg(palette.selected(), bg)));
    }
    spans.extend(inline_markdown_spans(
        &paragraph,
        style_bg(palette.text(), bg),
        style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
        style_bg(palette.selected(), bg),
    ));
    lines.push(chat_line(spans, bg));
    prose.clear();
}

/// Minimum width a table column is allowed to shrink to (just an `…`). Columns
/// shrink this far before the per-line clip (below) becomes the last resort, so
/// even many-column tables fit the pane whenever the column count allows.
const MIN_TABLE_COL: usize = 1;

fn flush_markdown_table(
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

#[allow(clippy::too_many_arguments)]
fn table_border_line(
    indent: &'static str,
    widths: &[usize],
    left: char,
    mid: char,
    right: char,
    border: Style,
    bg: Option<Color>,
    width: usize,
) -> Line<'static> {
    let mut s = String::new();
    s.push(left);
    for (idx, w) in widths.iter().enumerate() {
        if idx > 0 {
            s.push(mid);
        }
        for _ in 0..(w + 2) {
            s.push('─');
        }
    }
    s.push(right);
    let spans = vec![Span::styled(indent, border), Span::styled(s, border)];
    chat_line(clip_line_spans(spans, width), bg)
}

/// Hard-cut a fully-built line's spans to `max_width` display columns (no
/// ellipsis) so an over-wide table row is clipped rather than wrapped.
fn clip_line_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= max_width {
            break;
        }
        let span_w = span.content.as_ref().width();
        if used + span_w <= max_width {
            used += span_w;
            out.push(span);
        } else {
            let mut clipped = String::new();
            for ch in span.content.chars() {
                let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + ch_w > max_width {
                    break;
                }
                clipped.push(ch);
                used += ch_w;
            }
            if !clipped.is_empty() {
                out.push(Span::styled(clipped, span.style));
            }
            break;
        }
    }
    out
}

/// Render an inline-markdown table cell as styled spans, truncating to `max_w`
/// display columns (with an `…`) so the bordered grid stays aligned. Returns the
/// spans and the display width they occupy (`<= max_w`).
fn fit_cell_spans(
    cell: &str,
    max_w: usize,
    normal: Style,
    bold: Style,
    code: Style,
) -> (Vec<Span<'static>>, usize) {
    let spans = inline_markdown_spans(cell, normal, bold, code);
    let total: usize = spans.iter().map(|span| span.content.as_ref().width()).sum();
    if total <= max_w {
        return (spans, total);
    }
    if max_w == 0 {
        return (Vec::new(), 0);
    }
    let budget = max_w - 1; // leave room for the ellipsis
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let span_w = span.content.as_ref().width();
        if used + span_w <= budget {
            used += span_w;
            out.push(span);
        } else {
            let mut clipped = String::new();
            for ch in span.content.chars() {
                let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + ch_w > budget {
                    break;
                }
                clipped.push(ch);
                used += ch_w;
            }
            if !clipped.is_empty() {
                out.push(Span::styled(clipped, span.style));
            }
            break;
        }
    }
    out.push(Span::styled("…", normal));
    (out, used + 1)
}

fn table_cell_width(cell: &str) -> usize {
    // Column padding must match the terminal's *display* width, not the char
    // count — emoji/CJK render at width 2 but are a single char, so
    // chars().count() under-pads their columns and misaligns the table.
    restore_streamed_sentence_spacing(&plain_inline_markdown(cell))
        .as_str()
        .width()
}

fn chat_line(spans: Vec<Span<'static>>, bg: Option<Color>) -> Line<'static> {
    let line = Line::from(spans);
    match bg {
        Some(bg) => line.style(Style::default().bg(bg)),
        None => line,
    }
}

fn style_bg(style: Style, bg: Option<Color>) -> Style {
    match bg {
        Some(bg) => style.bg(bg),
        None => style,
    }
}

fn truncate_terminal_line(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }

    let keep = max_chars.saturating_sub(4);
    let mut preview = text.chars().take(keep).collect::<String>();
    preview.push_str(" ...");
    preview
}

/// Truncate `text` to at most `max_cols` terminal *display* columns
/// (unicode-width aware), appending a `…` when it overflows. Unlike
/// [`truncate_terminal_line`] this counts double-width CJK/emoji glyphs as 2
/// columns, so a row built from the result can never exceed its column budget
/// and wrap. Never splits a char and never byte-slices, so it cannot panic on
/// a multibyte boundary. The returned string's display width is `<= max_cols`.
fn truncate_to_display_width(text: &str, max_cols: usize) -> String {
    if text.width() <= max_cols {
        return text.to_string();
    }
    if max_cols == 0 {
        return String::new();
    }
    // Reserve one column for the ellipsis marker.
    let budget = max_cols - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_w > budget {
            break;
        }
        out.push(ch);
        used += ch_w;
    }
    out.push('…');
    out
}

fn line_is_blank(line: Option<&Line<'static>>) -> bool {
    line.map(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        .unwrap_or(false)
}

/// True when a line is a thematic break (`---`, `***`, `___`): ≥3 of a single
/// marker char once spaces are removed. Table separators (which contain `|`)
/// are handled earlier and never reach here.
fn markdown_hr(line: &str) -> bool {
    let stripped: String = line
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if stripped.len() < 3 {
        return false;
    }
    let mut chars = stripped.chars();
    let first = chars.next().unwrap();
    matches!(first, '-' | '*' | '_') && chars.all(|ch| ch == first)
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hash_count = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hash_count) {
        return None;
    }
    let heading = trimmed.get(hash_count..)?.strip_prefix(' ')?;
    (!heading.trim().is_empty()).then_some(heading.trim())
}

fn markdown_checkbox(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    if let Some(text) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        return Some((true, text.trim()));
    }
    trimmed
        .strip_prefix("- [ ] ")
        .map(|text| (false, text.trim()))
}

fn markdown_emphasis_segment(rest: &str) -> Option<(&str, usize)> {
    let delimiter = rest.chars().next()?;
    if !matches!(delimiter, '*' | '_') {
        return None;
    }
    let after_open = &rest[delimiter.len_utf8()..];
    if after_open.starts_with(delimiter) {
        return None;
    }
    let close = after_open.find(delimiter)?;
    let emphasized = &after_open[..close];
    if emphasized.is_empty() || emphasized.chars().all(char::is_whitespace) {
        return None;
    }
    Some((
        emphasized,
        delimiter.len_utf8() + close + delimiter.len_utf8(),
    ))
}

fn markdown_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .filter(|text| !text.trim().is_empty())
        .map(str::trim)
}

fn markdown_blockquote(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("> ")
        .or_else(|| trimmed.strip_prefix('>'))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn markdown_numbered(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let dot = trimmed.find(". ")?;
    let number = &trimmed[..dot];
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let text = trimmed[dot + 2..].trim();
    (!text.is_empty()).then_some((number, text))
}

fn markdown_table_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if !(trimmed.starts_with('|') && trimmed.ends_with('|')) {
        return None;
    }
    let cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    (cells.len() >= 2 && cells.iter().any(|cell| !cell.is_empty())).then_some(cells)
}

fn markdown_table_separator(line: &str) -> bool {
    markdown_table_cells(line).is_some_and(|cells| {
        cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
    })
}

/// Parse a markdown link `[text](url)` at the start of `s`, requiring
/// non-empty text AND url. Returns `(link_text, url, consumed_bytes)`, or `None`
/// to fall through to the plain-text path. Shared by the span renderer and the
/// width-measurement path (`plain_inline_markdown`) so the two cannot drift —
/// a link in a table cell measures exactly what it renders.
fn parse_markdown_link(s: &str) -> Option<(&str, &str, usize)> {
    let after_lb = s.strip_prefix('[')?;
    let mid = after_lb.find("](")?;
    let rel_close = after_lb[mid + 2..].find(')')?;
    let link_text = &after_lb[..mid];
    let url = &after_lb[mid + 2..mid + 2 + rel_close];
    if link_text.is_empty() || url.is_empty() {
        return None;
    }
    // '[' + link_text + "](" + url + ')'
    Some((link_text, url, 1 + mid + 2 + rel_close + 1))
}

/// Parse `~~text~~` at the start of `s`, requiring NON-WHITESPACE content
/// between the markers. Returns `(struck_text, consumed_bytes)`, or `None` for
/// degenerate forms (`~~~~`, `~~ ~~`) so they fall through to the plain-text
/// path and the literal tildes survive instead of being silently eaten. Shared
/// by the span renderer and `plain_inline_markdown` so width matches render.
fn parse_markdown_strikethrough(s: &str) -> Option<(&str, usize)> {
    let after_open = s.strip_prefix("~~")?;
    let close = after_open.find("~~")?;
    let struck = &after_open[..close];
    if struck.trim().is_empty() {
        return None;
    }
    // "~~" + struck + "~~"
    Some((struck, 2 + close + 2))
}

fn inline_markdown_spans(
    text: &str,
    normal_style: Style,
    bold_style: Style,
    code_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;

    while !rest.is_empty() {
        // Link `[text](url)`: text in the highlight (code) style, url appended
        // dimmed. NOT a real OSC 8 hyperlink — ratatui renders cell-by-cell, so
        // a raw escape would be counted as width and corrupt the layout.
        // The url is rendered IN FULL and unbroken (no truncation) so the
        // terminal's native URL detector can linkify it for cmd/ctrl+click in
        // the native-scrollback flow. (When the link text already IS the url,
        // we show it once instead of duplicating.)
        if let Some((link_text, url, consumed)) = parse_markdown_link(rest) {
            if link_text == url {
                spans.push(Span::styled(url.to_string(), code_style));
            } else {
                spans.push(Span::styled(link_text.to_string(), code_style));
                spans.push(Span::styled(
                    format!(" ({url})"),
                    normal_style.add_modifier(Modifier::DIM),
                ));
            }
            rest = &rest[consumed..];
            continue;
        }

        if let Some((struck, consumed)) = parse_markdown_strikethrough(rest) {
            spans.push(Span::styled(
                struck.to_string(),
                normal_style.add_modifier(Modifier::CROSSED_OUT),
            ));
            rest = &rest[consumed..];
            continue;
        }

        if let Some(after_open) = rest.strip_prefix("**")
            && let Some(close) = after_open.find("**")
        {
            let bold = &after_open[..close];
            if !bold.is_empty() {
                spans.push(Span::styled(bold.to_string(), bold_style));
            }
            rest = &after_open[close + 2..];
            continue;
        }

        if let Some(after_open) = rest.strip_prefix('`')
            && let Some(close) = after_open.find('`')
        {
            let code = &after_open[..close];
            if !code.is_empty() {
                spans.push(Span::styled(code.to_string(), code_style));
            }
            rest = &after_open[close + 1..];
            continue;
        }

        if let Some((emphasis, consumed)) = markdown_emphasis_segment(rest) {
            spans.push(Span::styled(
                emphasis.to_string(),
                bold_style.add_modifier(Modifier::ITALIC),
            ));
            rest = &rest[consumed..];
            continue;
        }

        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        // Stop a plain-text run before a link/strike opener so the next loop
        // iteration can parse it (otherwise the run would swallow `[` / `~~`).
        let next_link = rest.find('[');
        let next_strike = rest.find("~~");
        let next_emphasis = rest
            .char_indices()
            .skip(1)
            .find(|(_, ch)| matches!(ch, '*' | '_'))
            .map(|(idx, _)| idx);
        let next = [next_bold, next_code, next_link, next_strike, next_emphasis]
            .into_iter()
            .flatten()
            .min();
        let take = next.unwrap_or(rest.len());
        if take == 0 {
            let mut chars = rest.chars();
            if let Some(ch) = chars.next() {
                spans.push(Span::styled(ch.to_string(), normal_style));
                rest = chars.as_str();
            } else {
                break;
            }
        } else {
            spans.push(Span::styled(
                restore_streamed_sentence_spacing(&rest[..take]),
                normal_style,
            ));
            rest = &rest[take..];
        }
    }

    spans
}

fn restore_streamed_sentence_spacing(text: &str) -> String {
    let mut repaired = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        repaired.push(ch);
        let needs_sentence_space = matches!(ch, '.' | '!' | '?')
            && chars.peek().is_some_and(|next| next.is_ascii_uppercase())
            && repaired
                .chars()
                .rev()
                .nth(1)
                .is_some_and(|prev| prev.is_ascii_lowercase() || prev == ')');
        let needs_colon_space = ch == ':'
            && chars
                .peek()
                .is_some_and(|next| next.is_ascii_uppercase() || !next.is_ascii())
            && repaired
                .chars()
                .rev()
                .nth(1)
                .is_some_and(|prev| prev.is_ascii_alphanumeric() || prev == ')');
        if needs_sentence_space || needs_colon_space {
            repaired.push(' ');
        }
    }

    repaired
}

struct FileMutationActivity {
    operation: String,
    path: String,
    preview_ready: bool,
}

impl FileMutationActivity {
    fn from_item(item: &ActivityItem) -> Option<Self> {
        if item.kind != ActivityKind::Progress {
            return None;
        }
        if item.title != "file_mutation" && !item.status.starts_with("File mutation: ") {
            return None;
        }

        let source = item
            .detail
            .as_deref()
            .or_else(|| item.status.strip_prefix("File mutation: "))
            .filter(|source| !source.is_empty())?;
        let preview_ready = source.contains("diff preview ready");
        let source = source
            .replace(" | diff preview ready", "")
            .replace("diff preview ready", "");
        let (operation, path) = source.trim().split_once(' ')?;
        if path.trim().is_empty() {
            return None;
        }

        Some(Self {
            operation: operation.to_string(),
            path: path.trim().to_string(),
            preview_ready,
        })
    }
}

fn file_mutation_action_label(operation: &str) -> String {
    match operation {
        "add" | "added" | "create" | "created" => t!("app.tool.added"),
        "delete" | "deleted" | "remove" | "removed" => t!("app.tool.deleted"),
        "write" | "wrote" => t!("app.tool.wrote"),
        "modify" | "modified" | "update" | "updated" => t!("app.tool.changed"),
        _ => t!("app.tool.changed"),
    }
    .into_owned()
}

fn compact_file_path(path: &str) -> String {
    let components = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let keep = 4;
    if components.len() <= keep {
        return path.to_string();
    }
    format!(".../{}", components[components.len() - keep..].join("/"))
}

/// octos exposes several shell-family tools that all run a command string:
/// `shell`/`sh`/`exec`/`exec_command` (field `command`) and the
/// codex-compatible `bash` (field `cmd`, falling back to `command`). They all
/// render as a real command line, never the raw JSON arguments blob. Kept in
/// sync with the projection-side extraction in
/// [`crate::store::tool_invocation_detail`].
pub(crate) fn is_shell_family_tool(title: &str) -> bool {
    matches!(
        title.to_ascii_lowercase().as_str(),
        "shell" | "sh" | "exec" | "exec_command" | "bash"
    )
}

/// Longest raw-JSON fallback (display columns) `tool_invocation_text` will emit
/// when it has no better human rendering — a hard cap so a pathological args
/// blob can never be handed to the row builder unbounded. The per-row width
/// budget truncates further; this only bounds the worst case.
const RAW_ARG_FALLBACK_COLS: usize = 512;

/// A human-readable one-line invocation for a tool activity, preferring a real
/// command string over the raw serialized arguments (which used to leak into
/// the card as `{"cmd":…}`). Order: an explicit `detail` (run through the
/// args-echo humanizer — the server path fills it with the protocol #1606
/// `arguments_preview` JSON echo), then a shell-like tool's command string,
/// then a compact `key=value` of the first meaningful object field, then a
/// bounded raw-JSON fallback.
///
/// DISPLAY-ONLY: `ActivityItem.detail` itself is never rewritten — the
/// envelope thread marker stored there is load-bearing for the turn-less
/// reconcile ([`AppState::reconcile_envelope_thread_running_activity`]).
fn tool_invocation_text(item: &ActivityItem) -> Option<String> {
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        return Some(humanize_args_echo(detail, &item.title));
    }
    let arguments = item.arguments.as_ref()?;
    // The envelope lane parks the same serialized args echo in `arguments` as
    // a JSON String (its `detail` carries the thread marker instead): treat
    // the inner text exactly like a detail echo — re-serializing it would
    // render `"{\"cmd\":…`.
    if let Some(echo) = arguments.as_str() {
        let echo = echo.trim();
        if !echo.is_empty() {
            return Some(humanize_args_echo(echo, &item.title));
        }
    }
    // Shell-like tools carry their command under `command`/`cmd`; surface that
    // (untruncated — callers like `shell_action_label` match on the full text,
    // and the row builder applies the display-width budget) instead of the JSON
    // envelope.
    if is_shell_like_tool(&item.title) {
        if let Some(command) = shell_command_from_args(arguments) {
            return Some(command);
        }
    }
    // Other tools with an object payload: show a compact `key=value` of the
    // first meaningful string/number field rather than the whole JSON blob.
    if let Some(map) = arguments.as_object() {
        if let Some(rendered) = first_meaningful_arg(map) {
            return Some(single_line_invocation(&rendered));
        }
    }
    // Last resort: bounded raw JSON (never an unbounded dump).
    serde_json::to_string(arguments)
        .ok()
        .map(|json| truncate_to_display_width(&json, RAW_ARG_FALLBACK_COLS))
}

/// The `command`/`cmd` string of a shell-like tool's args object, flattened to
/// one line. `None` when the payload has no non-empty command string.
fn shell_command_from_args(arguments: &serde_json::Value) -> Option<String> {
    arguments
        .get("command")
        .or_else(|| arguments.get("cmd"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(single_line_invocation)
}

/// Humanize a serialized arguments echo for the one-line tool row. The server
/// caps the echo (~700 bytes, protocol #1606), so a JSON object echo often
/// arrives CUT mid-string — strict parsing gets the well-formed case, a
/// lenient scan covers the truncated one, and a cleanup pass guarantees the
/// floor: no raw `{"key":` prefix, no literal `\n`/`\t` escape leaking into
/// the row.
///
/// `detail` ALSO carries already-decoded REAL invocation text (the `!`-bang
/// echo, the live-lane command summaries, progress prose, thread markers), so
/// the transforms are gated on the two serialized-echo shapes and everything
/// else renders verbatim (one-lined only): a brace-group command `{ echo ok; }`
/// is NOT a JSON echo (that requires `{"`), and `printf '\n'` keeps its
/// intentional two-char escape (escape decoding requires the `key: value`
/// preview opener).
fn humanize_args_echo(echo: &str, title: &str) -> String {
    let trimmed = echo.trim();
    if looks_like_json_object_echo(trimmed) {
        // Complete echo: strict parse, then the same rendering the
        // object-arguments path uses (command string / first `key=value`).
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(trimmed)
        {
            let value = serde_json::Value::Object(map);
            if is_shell_like_tool(title) {
                if let Some(command) = shell_command_from_args(&value) {
                    return command;
                }
            }
            if let Some(map) = value.as_object() {
                if let Some(rendered) = first_meaningful_arg(map) {
                    return single_line_invocation(&rendered);
                }
            }
        } else if is_shell_like_tool(title) {
            // Truncated echo (strict parse fails): scan for the command key
            // and decode the string value up to the cut.
            if let Some(command) = lenient_echo_command(trimmed) {
                return command;
            }
        }
        // Floor for anything else `{`-shaped (truncated non-shell echo, or an
        // object with no scalar field): strip the JSON framing and decode the
        // common escapes so the row never shows `{"key":` or a literal `\n`.
        return single_line_invocation(&scrub_json_echo_fragment(trimmed));
    }
    // The producer's `key: value` preview format JSON-encodes string values,
    // so decode the common escapes there; rows are one-line, so an escaped
    // newline becomes a space.
    if has_key_value_echo_opener(trimmed) {
        return single_line_invocation(&decode_json_string_escapes(trimmed));
    }
    // Plain already-decoded text (bang commands, live-lane invocation
    // summaries, progress prose, thread markers): verbatim, one-lined.
    single_line_invocation(trimmed)
}

/// A serialized JSON object echo starts `{"` (optionally with whitespace
/// between — pretty printing), because the first thing inside a JSON object is
/// a quoted key. A brace-group shell command (`{ echo ok; }`) does not, so it
/// is never mistaken for an echo.
fn looks_like_json_object_echo(text: &str) -> bool {
    text.strip_prefix('{')
        .is_some_and(|rest| rest.trim_start().starts_with('"'))
}

/// The `key: value` preview opener the #1606 producer emits for object args
/// (`cmd: "grep …", timeout: 300`): a bare identifier-ish key, then `: `. Real
/// commands/prose almost never start this way (`printf '\n'` has no colon; an
/// `echo "note: x"` command's first token contains spaces/quotes and fails the
/// key charset).
fn has_key_value_echo_opener(text: &str) -> bool {
    let Some((key, _)) = text.split_once(": ") else {
        return false;
    };
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

/// Lenient `command`/`cmd` extraction from a truncated JSON object echo that
/// `serde_json` cannot parse (the ~700-byte cap cuts mid-string): find the
/// key, then decode its string value up to the closing unescaped quote or the
/// end of the input. Char-boundary safe (operates on `char`s, and the marker
/// find can only land on ASCII boundaries).
fn lenient_echo_command(echo: &str) -> Option<String> {
    for key in ["\"command\"", "\"cmd\""] {
        let Some(pos) = echo.find(key) else {
            continue;
        };
        let rest = echo[pos + key.len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let Some(body) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let command = single_line_invocation(&decode_json_string_body(body, true));
        if !command.is_empty() {
            return Some(command);
        }
    }
    None
}

/// Floor rendering for a truncated JSON echo with no better extraction: drop
/// the leading `{`/`"` framing and decode the common escapes. The result is
/// not pretty, but it never shows a raw `{"key":` prefix or a literal `\n`.
fn scrub_json_echo_fragment(echo: &str) -> String {
    let body = echo.strip_prefix('{').unwrap_or(echo).trim_start();
    let body = body.strip_prefix('"').unwrap_or(body);
    decode_json_string_escapes(body)
}

/// Decode the common JSON string escapes for one-line display: `\"`→`"`,
/// `\\`→`\`, `\n`/`\t`/`\r`→space. Unknown escapes pass through verbatim and a
/// dangling trailing backslash (left by the echo's byte cap) is dropped.
fn decode_json_string_escapes(text: &str) -> String {
    decode_json_string_body(text, false)
}

/// Shared escape decoder. With `stop_at_quote`, decoding ends at the first
/// unescaped `"` (the value's closing quote in a JSON echo — trailing sibling
/// keys are dropped); otherwise the whole input is decoded.
fn decode_json_string_body(text: &str, stop_at_quote: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if stop_at_quote => break,
            '\\' => match chars.next() {
                Some('n' | 't' | 'r') => out.push(' '),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                // Dangling backslash at the truncation cut — drop it.
                None => {}
            },
            other => out.push(other),
        }
    }
    out
}

/// Rows are one-line: flatten real newlines/tabs in an invocation to spaces
/// (the row is width-truncated by the builder; multi-line content belongs to
/// the `│` output-preview lines, which are NOT run through this).
fn single_line_invocation(text: &str) -> String {
    if text.chars().any(|ch| matches!(ch, '\n' | '\r' | '\t')) {
        text.chars()
            .map(|ch| match ch {
                '\n' | '\r' | '\t' => ' ',
                other => other,
            })
            .collect::<String>()
            .trim()
            .to_string()
    } else {
        text.trim().to_string()
    }
}

/// Case-insensitive check for the shell family whose invocation is a command
/// string (`shell`/`bash`/`sh`). Kept in one place so the command-extraction in
/// [`tool_invocation_text`] and the `$ ` prompt in the row builder agree.
fn is_shell_like_tool(title: &str) -> bool {
    matches!(title.to_ascii_lowercase().as_str(), "shell" | "bash" | "sh")
}

/// Render the first meaningful field of an args object as a compact
/// `key=value`, bounded so a huge value can't blow up the row. Returns `None`
/// when no scalar (string/number/bool) field is present.
fn first_meaningful_arg(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for (key, value) in map {
        let rendered = match value {
            serde_json::Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        let value = truncate_to_display_width(&rendered, RAW_ARG_FALLBACK_COLS);
        return Some(format!("{key}={value}"));
    }
    None
}
fn meaningful_output_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    let seconds = duration_ms as f64 / 1_000.0;
    if seconds < 10.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{seconds:.0}s")
    }
}

fn format_elapsed_secs(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn push_command_row(
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

fn push_inline_approval_card(
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

fn approval_action_labels(_approval: &ApprovalModalState) -> [String; 3] {
    [
        t!("app.approval.action_once").to_string(),
        t!("app.approval.action_session").to_string(),
        t!("app.approval.action_deny").to_string(),
    ]
}

/// UPCR-2026-023: render the pending AskUserQuestion picker inline, mirroring
/// [`push_inline_approval_card`]. Shows the mandatory `title`/`body` fallback,
/// the active structured question (1–4), each option as a radio/checkbox row,
/// and the always-present free-text "Other" row.
/// The `/btw` aside card: question echo, then `✽ Answering…` while the
/// out-of-band answer is in flight, then the answer as a dim `·` block (or a
/// failure line). Live-pane only — the aside is ephemeral by design.
fn push_btw_aside_card(
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

/// Render the `/btw` aside as a floating BORDERED pane pinned to the TOP of
/// the live viewport. It draws over the top rows of the live tail each frame
/// (never flushed to scrollback) and vanishes on the next prompt submit. The
/// border + title are load-bearing: a borderless overlay reads as embedded
/// transcript text whenever the tail is short — the box is what makes it a
/// visibly distinct window over the session instead of part of the flow.
/// Rows the `/btw` overlay pane wants (card lines sans leading blanks, plus
/// the two border rows); `0` when the active session has no aside. The aside
/// contributes NO lines to the turn flow, so [`live_tail_height_with_finalization`]
/// must reserve these rows explicitly — a settled session's tail otherwise
/// collapses to 1-2 rows, under [`render_btw_overlay`]'s 3-row minimum, and
/// the pane silently stops drawing while the aside is still answering
/// (codex P1). Kept in sync with `render_btw_overlay`'s layout math.
fn btw_overlay_height_hint(app: &AppState, area_width: u16) -> u16 {
    if area_width < 4 {
        return 0;
    }
    let Some(session) = app.active_session() else {
        return 0;
    };
    let Some(aside) = app.btw_asides.get(&session.id) else {
        return 0;
    };
    let mut lines = Vec::new();
    push_btw_aside_card(
        &mut lines,
        Palette::for_theme(app.theme),
        aside,
        area_width as usize - 2,
    );
    while line_is_blank(lines.first()) {
        lines.remove(0);
    }
    if lines.is_empty() {
        return 0;
    }
    (lines.len() as u16).saturating_add(2)
}

fn render_btw_overlay(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
    tail_area: Rect,
) {
    if tail_area.width < 4 || tail_area.height < 3 {
        return;
    }
    let Some(session) = app.active_session() else {
        return;
    };
    let Some(aside) = app.btw_asides.get(&session.id) else {
        return;
    };
    let mut lines = Vec::new();
    // Inner width: the block borders consume one column each side.
    push_btw_aside_card(&mut lines, palette, aside, tail_area.width as usize - 2);
    // The card opens with a spacer line for the inline flow; inside a bordered
    // pane the border already separates it — drop leading blanks.
    while line_is_blank(lines.first()) {
        lines.remove(0);
    }
    if lines.is_empty() {
        return;
    }
    // +2 for the top/bottom border rows.
    let height = (lines.len() as u16 + 2).min(tail_area.height);
    let overlay = Rect {
        x: tail_area.x,
        y: tail_area.y,
        width: tail_area.width,
        height,
    };
    let title = t!("app.btw.pane_title").into_owned();
    let close_hint = t!("app.btw.close_hint").into_owned();
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Paragraph::new(lines)
            .style(palette.text().bg(palette.surface))
            .block(titled_block(title, palette, false, Some(close_hint))),
        overlay,
    );
}

fn push_inline_user_question_card(
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
        push_prefixed_line(
            lines,
            "    ",
            palette.muted(),
            Line::from(Span::styled(picker.title.clone(), palette.text())),
        );
    }
    if !picker.body.is_empty() {
        push_prefixed_line(
            lines,
            "    ",
            palette.muted(),
            Line::from(Span::styled(picker.body.clone(), palette.muted())),
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
            // stays dismissible (Esc) and recoverable (Alt+a).
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

fn push_user_question_entry(
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
    push_prefixed_line(
        lines,
        "    ",
        palette.muted(),
        Line::from(Span::styled(entry.question.clone(), palette.text())),
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
fn push_user_question_option_row(
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
    // Budget the label to the remaining width after the bar + marker prefixes
    // (2 cols each). `fit_card_text` already reserves the 4-space indent, so
    // subtract only the extra 4 columns here — subtracting 6 clipped labels
    // two columns early (codex review).
    let label = fit_card_text(text, width.saturating_sub(4));
    lines.push(Line::from(vec![
        Span::styled("    ", palette.muted()),
        Span::styled(bar, bar_style),
        Span::styled(marker, marker_style),
        Span::styled(label, label_style),
    ]));
}

fn user_question_action_labels(picker: &UserQuestionPickerState) -> Vec<String> {
    // Garbled / 0-question event: nothing is answerable, so offer only a dismiss
    // hint — never a submit affordance that would form an invalid respond
    // (DO-NOT-SHIP #2). Alt+a re-opens it if dismissed (DO-NOT-SHIP #1).
    if picker.questions.is_empty() {
        return vec![t!("app.question.action_dismiss").to_string()];
    }
    let mut labels = vec![t!("app.question.action_toggle").to_string()];
    if picker.is_last_question() {
        labels.push(t!("app.question.action_submit").to_string());
    } else {
        labels.push(t!("app.question.action_next").to_string());
    }
    labels
}

fn fit_card_text(text: &str, width: usize) -> String {
    // Reserve the 4-space prefix added by the caller. The budget is DISPLAY
    // COLUMNS (unicode-width), not chars — CJK glyphs are double-width, so a
    // char-count budget let CJK options overflow the card (mirror of
    // `clip_line_spans`).
    let budget = width.saturating_sub(4).max(1);
    if text.width() <= budget {
        return text.to_string();
    }
    let cut = budget.saturating_sub(1); // leave a column for the ellipsis
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_w > cut {
            break;
        }
        out.push(ch);
        used += ch_w;
    }
    out.push('…');
    out
}

fn push_prefixed_line(
    lines: &mut Vec<Line<'static>>,
    prefix: &'static str,
    prefix_style: Style,
    mut line: Line<'static>,
) {
    let mut spans = vec![Span::styled(prefix, prefix_style)];
    spans.append(&mut line.spans);
    lines.push(Line::from(spans));
}

fn push_activity_section_with_finalization(
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

/// Pending master re-entries the server has queued for the active session
/// (from the `session/orchestration` mirror). Drives the "re-entering" chip
/// title so a settled-but-continuing turn doesn't read as completed.
fn active_session_pending_continuations(app: &AppState) -> u32 {
    app.active_session()
        .and_then(|session| app.orchestration.get(&session.id))
        .filter(|status| status.active)
        .map(|status| status.pending_continuations)
        .unwrap_or(0)
}

/// Whether the agent-task group identified by `group_turn` is the CURRENT/active
/// turn's group (vs an ARCHIVED past-turn group).
///
/// Blocking bug 1: `active_session_pending_continuations` is a per-SESSION
/// fact (the server's queued re-entry count), so feeding it to every group
/// retitled archived completed/failed groups as "Re-entering". Only the active
/// group may flip to "Re-entering"; this predicate scopes that.
///
/// A group is active when:
/// - its `turn_id` equals the active session's live turn (`active_turn`), OR
/// - it is the turn-less fold (`None`) AND no turn is live but the session is
///   orchestrating — the turn-less sub-agent fold of the live orchestration is
///   the current group (see `flow_activity_items` / `is_subagent_progress`).
fn is_active_group(app: &AppState, group_turn: Option<&octos_core::ui_protocol::TurnId>) -> bool {
    match (group_turn, app.active_turn()) {
        (Some(group_turn), Some((_, active_turn))) => group_turn == active_turn,
        (Some(_), None) => false,
        // Turn-less fold: only the live orchestration's sub-agent fold (no live
        // turn) is the current group. With a live turn present, the turn-less
        // fold is not the active group.
        (None, None) => app
            .active_session()
            .and_then(|session| app.orchestration.get(&session.id))
            .is_some_and(|status| status.active),
        (None, Some(_)) => false,
    }
}

fn flow_activity_items(app: &AppState) -> Vec<&ActivityItem> {
    let active_turn_id = app.active_turn().map(|(_, turn_id)| turn_id);
    app.activity
        .iter()
        .filter(|item| match active_turn_id {
            Some(turn_id) => item.turn_id.as_ref() == Some(turn_id),
            // When no turn is active, turn-less running sub-agent progress is
            // folded into the orchestrating turn's chip (as children) — don't
            // also render it here as a separate turn-less "Orchestrating" chip.
            None => item.turn_id.is_none() && !is_subagent_progress(app, item),
        })
        .collect()
}

/// A turn-less running sub-agent progress row (an `AgentUpdated` / spawn-complete
/// `Progress` item with no originating turn) that is ALSO represented by a
/// running sub-agent task. Such rows are surfaced under the orchestrating turn's
/// chip via `running_subagent_titles_for_chip`, so they must not also form their
/// own phantom turn-less "Orchestrating" chip (mini5 soak: the "two Orchestrating
/// chips" for one parallel-spawn turn).
///
/// codex P2: we only suppress when a matching running TASK exists. A turn-less
/// progress row with no matching task has nothing to fold into, so we keep it
/// visible in the flow rather than hiding it entirely (orphaned-from-view).
fn is_subagent_progress(app: &AppState, item: &ActivityItem) -> bool {
    if item.turn_id.is_some() || item.kind != ActivityKind::Progress || !is_running_activity(item) {
        return false;
    }
    app.active_session().is_some_and(|session| {
        session.tasks.iter().any(|task| {
            matches!(task_state_label(task.state), "pending" | "running")
                && task.title == item.title
        })
    })
}

fn push_turn_activity_log_section(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    log: &TurnActivityLog,
    app: &AppState,
    collapse_settled: bool,
    wrap_width: usize,
) {
    let summary = app.turn_summary_for(&log.turn_id);
    // A tool-less turn carries only a summary (no activity items); still render
    // its report. Nothing at all to show only when both are absent.
    if log.items.is_empty() && summary.is_none() {
        return;
    }
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    if !log.items.is_empty() {
        let shown_limit = if app.expanded_tool_outputs { 12 } else { 3 };
        // Full uncapped set (header counts + footer tally both derive from this
        // via `task_group_counts`, so they cannot diverge).
        let full = log.items.iter().collect::<Vec<_>>();
        let shown = full
            .iter()
            .rev()
            .take(shown_limit)
            .rev()
            .copied()
            .collect::<Vec<_>>();
        push_agent_task_group(
            lines,
            palette,
            Some(&log.turn_id),
            &full,
            &shown,
            &running_subagent_titles_for_chip(app, Some(&log.turn_id)),
            active_session_pending_continuations(app),
            is_active_group(app, Some(&log.turn_id)),
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
            push_inline_diff_preview(lines, palette, &app.diff_preview, app.expanded_tool_outputs);
        }
    }
    push_turn_summary_line(lines, palette, app, &log.turn_id);
}

fn push_turn_activity_log_section_unflushed(
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
fn push_turn_summary_line(
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

/// The committed per-turn status report line, e.g.
/// `✻ Ran for 5m 19s · 2 background task(s) still running`. The `✻` glyph and
/// duration mirror the live working indicator; the trailing clause is dropped
/// when nothing was left running.
fn turn_summary_text(summary: &crate::model::TurnActivitySummary) -> String {
    let ran_for = t!(
        "app.turn_summary.ran_for",
        duration = format_elapsed_secs(summary.elapsed_secs)
    );
    if summary.background_tasks > 0 {
        let still_running = t!(
            "app.turn_summary.tasks_still_running",
            count = summary.background_tasks
        );
        format!("✻ {ran_for} · {still_running}")
    } else {
        format!("✻ {ran_for}")
    }
}

fn push_finalized_activity_items_section(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    turn_id: Option<&octos_core::ui_protocol::TurnId>,
    items: &[&ActivityItem],
    wrap_width: usize,
) {
    if items.is_empty() {
        return;
    }
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    push_agent_task_group(
        lines,
        palette,
        turn_id,
        items,
        items,
        &[],
        0,
        false,
        app.expanded_tool_outputs,
        // Scrollback flush path: the archive never collapses.
        false,
        wrap_width,
    );
}

/// "Tentacle pulse" octopus spinner frames (braille blob, all single-width).
const SPINNER_FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

/// Current spinner frame, advancing ~every 120ms off a process-lifetime clock
/// (independent of any turn timer, so it keeps animating while background
/// sub-agents run after the parent turn has finished). The event loop redraws
/// every ~25ms, so this reads as smooth motion.
fn spinner_frame() -> &'static str {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    SPINNER_FRAMES[(elapsed / 120) as usize % SPINNER_FRAMES.len()]
}

/// Seconds since process start — the same process clock `spinner_frame` rides,
/// so a wave keyed off it advances on every ~25ms animation redraw without
/// threading a phase counter through `AppState`.
fn anim_time_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Extract an RGB triple from a ratatui `Color`. Truecolor themes store
/// `Color::Rgb`; named/`Reset` colors (the Terminal theme) fall back to neutral
/// grey so the wave degrades to a subtle ripple rather than panicking.
fn rgb_of(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (170, 170, 170),
    }
}

/// Linear RGB lerp across gradient `stops`; `t` clamped to 0..=1.
fn gradient_sample(stops: &[(u8, u8, u8)], t: f32) -> (u8, u8, u8) {
    match stops {
        [] => (255, 255, 255),
        [only] => *only,
        _ => {
            let f = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
            let lo = f.floor() as usize;
            let hi = (lo + 1).min(stops.len() - 1);
            let frac = f - lo as f32;
            let (a, b) = (stops[lo], stops[hi]);
            let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * frac).round() as u8;
            (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
        }
    }
}

/// One `Span` per grapheme, each colored from a sine-driven sample point that
/// slides with `phase`, so a bright crest travels along `text` like a ripple.
/// Advances by DISPLAY columns (CJK/emoji are double-width) so the wave stays
/// even across multi-width glyphs; `bg` preserves the row's surface background.
fn wave_gradient_spans(
    text: &str,
    phase: f32,
    stops: &[(u8, u8, u8)],
    bg: Color,
) -> Vec<Span<'static>> {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    const K: f32 = 0.45; // radians per display column — ripple tightness
    let mut spans = Vec::new();
    let mut col = 0.0f32;
    for g in text.graphemes(true) {
        let wave = 0.5 + 0.5 * (col * K - phase).sin();
        let (r, gg, b) = gradient_sample(stops, wave);
        spans.push(Span::styled(
            g.to_string(),
            Style::default().fg(Color::Rgb(r, gg, b)).bg(bg),
        ));
        col += g.width().max(1) as f32;
    }
    spans
}

/// Title for an agent-task group chip. Pure so it can be unit-tested
/// directly (Gap 2 fix #2). The order of precedence is deliberate:
///
/// 1. `in_progress` (live tool calls or running sub-agents) → "Orchestrating".
/// 2. `pending_continuations > 0` AND `is_active_group` → "re-entering". The
///    parent's tool calls can all be settled while the server has a master
///    re-entry queued; the CURRENT turn's chip must NOT read "Agent task
///    completed" in that gap (the "looks done" lie).
///
///    Blocking bug 1: `pending_continuations` is the active SESSION's queued
///    count, not a per-group fact. It must only retitle the CURRENT/active
///    turn's group — never an ARCHIVED past-turn group (whose work is over and
///    is not the thing being continued). `is_active_group` gates this. For the
///    active group the continuation is the live truth, so it even wins over a
///    `failed` parent (the failure is what is being retried/continued).
/// 3. `failed > 0` → finished with errors (the only re-entry-beating outcome
///    for ARCHIVED groups; pending never applies there).
/// 4. otherwise → completed.
fn agent_task_group_title(
    in_progress: bool,
    failed: usize,
    pending_continuations: u32,
    is_active_group: bool,
) -> String {
    if in_progress {
        t!("app.activity.orchestrating").to_string()
    } else if is_active_group && pending_continuations > 0 {
        t!("app.activity.re_entering").to_string()
    } else if failed > 0 {
        t!("app.activity.finished_errors").to_string()
    } else {
        t!("app.activity.completed").to_string()
    }
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
fn push_agent_task_group(
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

/// Claude-Code-style display name for a tool (`bash` → `Bash`, `read_file` →
/// `Read`, …). Unknown tools get their first letter capitalized.
fn tool_display_name(title: &str) -> String {
    match title {
        "shell" | "exec" | "exec_command" | "bash" => "Bash".into(),
        "read_file" => "Read".into(),
        "write_file" => "Write".into(),
        "edit_file" | "diff_edit" => "Edit".into(),
        "list_dir" => "List".into(),
        "grep" | "grep_tool" => "Grep".into(),
        "glob" | "glob_tool" => "Glob".into(),
        "web_search" | "deep_search" => "Search".into(),
        "web_fetch" => "Fetch".into(),
        "spawn" => "Spawn".into(),
        "browser" => "Browser".into(),
        "message" => "Message".into(),
        "send_file" => "Send".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        }
    }
}

/// The `⏺` card bullet, colored by status: green when the tool succeeded, red
/// when it failed, and the animated spinner while it is still running.
fn tool_card_bullet(item: &ActivityItem, palette: Palette) -> (String, Style) {
    if is_running_activity(item) {
        (spinner_frame().to_string(), palette.selected())
    } else if activity_is_failed(item) {
        // Failures keep a distinct glyph (not just red) so they stay legible
        // without color; success drops the checkmark for the calmer `⏺`.
        ("✗".to_string(), Style::default().fg(palette.danger))
    } else if activity_is_completed(item) {
        ("⏺".to_string(), Style::default().fg(palette.success))
    } else {
        // interrupted / skipped / pending — neutral, never a false green success.
        ("⏺".to_string(), palette.muted())
    }
}

/// Claude-Code-style tool-card header: `⏺ Bash(cmd)`. The invocation (shell
/// command, spawn task, file path, …) renders in parens with raw JSON and the
/// call-id stripped; multi-line commands indent to align under `(`. Every
/// emitted line is budgeted + clipped to `wrap_width` display columns so it
/// can never overflow and wrap to column 0 (the indent-not-honored bug).
fn push_tool_card_header(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    wrap_width: usize,
) {
    let (bullet, bullet_style) = tool_card_bullet(item, palette);
    let name = tool_display_name(&item.title);
    let duration = item
        .duration_ms
        .map(|ms| format!("  {}", format_duration_ms(ms)))
        .unwrap_or_default();

    let Some(invocation) = tool_invocation_text(item).filter(|text| !text.trim().is_empty()) else {
        // No arguments to show: `⏺ Bash`.
        let mut spans = vec![
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
    // `⏺ Bash($ cargo test)` — the command-row marker #276 established; the
    // prompt is part of the budgeted text so the width math stays exact.
    let invocation = if is_shell_family_tool(&item.title) {
        format!("$ {invocation}")
    } else {
        invocation
    };

    // Continuation lines align under the first char after `(`.
    let cont_indent = " ".repeat(bullet.chars().count() + 1 + name.chars().count() + 1);
    let cmd_lines: Vec<&str> = invocation.lines().collect();
    let max_lines = 10usize;
    let shown = cmd_lines.len().min(max_lines).max(1);
    let clipped = cmd_lines.len() > shown;
    // Budget the command text so lead-in + text + `)` + duration fit within
    // `wrap_width` (unicode-width, so CJK commands stay exact).
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

fn push_agent_task_child(
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

fn compact_activity_spans(
    item: &ActivityItem,
    palette: Palette,
    content_budget: usize,
) -> Vec<Span<'static>> {
    if let Some(mutation) = FileMutationActivity::from_item(item) {
        // Activity rows render uniformly muted, no bold: the runtime log must
        // never outweigh the reply prose or the user's own words.
        // "preview ready" was dropped: the TUI exposes no action to open the
        // preview here, so the label was a dead affordance.
        return vec![
            Span::styled(
                file_mutation_action_label(&mutation.operation),
                palette.muted(),
            ),
            Span::styled(" ", palette.muted()),
            Span::styled(compact_file_path(&mutation.path), palette.muted()),
            Span::styled(format!("  {}", mutation.operation), palette.muted()),
        ];
    }

    // Tool activities render as Claude-Code cards via `push_tool_card_header`;
    // this path only handles non-tool rows (progress, generic).

    // A context-compaction notice is an infrequent, notable event — render it
    // prominently (accent + ✦) so it stands out from the muted activity stream
    // instead of scrolling by unseen in a busy multi-agent session.
    let compacted_title = t!("status.activity_context_compacted");
    if item.kind == ActivityKind::Progress && item.title.as_str() == compacted_title.as_ref() {
        let mut spans = vec![
            Span::styled("✦ ", Style::default().fg(palette.accent)),
            Span::styled(
                item.title.clone(),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", item.status), palette.muted()),
        ];
        // Reserve the trailing metadata (duration) width up front so the
        // detail is truncated to fit BEFORE it, keeping the duration visible.
        let mut meta = Vec::new();
        push_compact_metadata_spans(&mut meta, palette, item);
        if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
            spans.push(Span::styled("  ", palette.muted()));
            let detail_budget = remaining_content_budget(content_budget, &spans, &meta);
            spans.push(Span::styled(
                truncate_to_display_width(detail, detail_budget),
                palette.muted(),
            ));
        }
        spans.extend(meta);
        return spans;
    }

    let mut spans = vec![
        Span::styled(item.title.clone(), palette.muted()),
        Span::styled(format!("  {}", item.status), palette.muted()),
    ];
    let mut meta = Vec::new();
    push_compact_metadata_spans(&mut meta, palette, item);
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        spans.push(Span::styled("  ", palette.muted()));
        let detail_budget = remaining_content_budget(content_budget, &spans, &meta);
        spans.push(Span::styled(
            truncate_to_display_width(detail, detail_budget),
            palette.muted(),
        ));
    }
    spans.extend(meta);
    spans
}

/// Display columns still available for a row's variable part, given the total
/// `content_budget`, the fixed leading spans already built, and the trailing
/// metadata spans reserved after it. Saturating so an over-tight budget yields
/// 0 (the variable part vanishes) rather than underflowing.
fn remaining_content_budget(
    content_budget: usize,
    leading: &[Span<'static>],
    trailing: &[Span<'static>],
) -> usize {
    let used: usize = leading
        .iter()
        .chain(trailing.iter())
        .map(|span| span.content.as_ref().width())
        .sum();
    content_budget.saturating_sub(used)
}

fn push_compact_metadata_spans(
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

fn push_compact_tool_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    expanded: bool,
    wrap_width: usize,
    in_scrollback: bool,
) {
    // The preview prefix `  ⎿ ` is 4 display columns; budget the content so a
    // preview line fits within `wrap_width` and never wraps to column 0.
    const PREVIEW_PREFIX_COLS: usize = 4;
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
            Span::styled("  ⎿ ", palette.border()),
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
                Span::styled("  ⎿ ", palette.border()),
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
                Span::styled("  ⎿ ", palette.border()),
                Span::styled(
                    t!("app.activity.expanded_hint").into_owned(),
                    palette.muted(),
                ),
            ],
            wrap_width,
        )));
    }
}

fn activity_status_icon(item: &ActivityItem, palette: Palette) -> (&'static str, Style) {
    if is_running_activity(item) {
        // Animate the marker for in-progress rows (octopus spinner) so a row
        // like "Background work started for run_pipeline" visibly reads as
        // still-running rather than a static dot. Uses the shared
        // process-clock spinner (not terminal SGR blink, which is unreliable /
        // distracting and inconsistently supported).
        (spinner_frame(), palette.selected())
    } else if activity_is_failed(item) {
        ("✗", Style::default().fg(palette.danger))
    } else if activity_is_completed(item) {
        ("✓", Style::default().fg(palette.success))
    } else {
        ("•", palette.muted())
    }
}

fn activity_is_completed(item: &ActivityItem) -> bool {
    matches!(item.success, Some(true))
        || matches!(
            item.status.as_str(),
            "complete" | "completed" | "done" | "success"
        )
}

fn activity_is_failed(item: &ActivityItem) -> bool {
    matches!(item.success, Some(false)) || matches!(item.status.as_str(), "failed" | "error")
}

/// Tally the agent-task-group counts over a slice of activity items.
///
/// Returns `(total, completed, active, failed)` using the SAME predicates the
/// chip header and footer already use ([`activity_is_completed`],
/// [`is_running_activity`], [`activity_is_failed`]).
///
/// The chip header MUST tally over the FULL turn activity set, not the
/// display-capped slice of children that's actually rendered — otherwise a
/// 66-action turn showing the last 3 rows reads "3 action(s) · 3 completed"
/// while its own "... +63 older action(s)" footer proves the real total is 66.
/// Both the header and the footer call this single helper so their numbers
/// cannot diverge.
fn task_group_counts(full_items: &[&ActivityItem]) -> (usize, usize, usize, usize) {
    let total = full_items.len();
    let completed = full_items
        .iter()
        .filter(|item| activity_is_completed(item))
        .count();
    let active = full_items
        .iter()
        .filter(|item| is_running_activity(item))
        .count();
    let failed = full_items
        .iter()
        .filter(|item| activity_is_failed(item))
        .count();
    (total, completed, active, failed)
}

fn push_inline_diff_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    diff: &DiffPreviewPaneState,
    expanded: bool,
) {
    // C6: when there is no usable line diff ("line diff unavailable for this
    // mutation"), hide the box entirely instead of rendering an empty preview
    // with a dead "[/] select hunk | c stage" UI. Loading/error stay visible.
    if !diff.has_renderable_diff() {
        return;
    }
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
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                preview
                    .title
                    .clone()
                    .unwrap_or_else(|| t!("app.diff.inline_patch").to_string()),
                palette.text().add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", palette.muted()),
            Span::styled(
                diff.status.as_deref().unwrap_or("unknown").to_string(),
                palette.muted(),
            ),
            Span::styled("  ", palette.muted()),
            Span::styled(
                diff.source.as_deref().unwrap_or("unknown").to_string(),
                palette.muted(),
            ),
        ]));

        if preview.files.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(t!("app.empty.no_file_changes").to_string(), palette.muted()),
            ]));
        }

        if !preview.files.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    t!("app.diff.select_stage_hint").into_owned(),
                    palette.selected(),
                ),
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

fn push_diff_content_line(
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

fn push_diff_file_lines(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    file_idx: usize,
    selected_file: usize,
    selected_hunk: usize,
    file: &crate::model::DiffPreviewFile,
    expanded: bool,
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
                for line in &hunk.lines {
                    push_diff_content_line(lines, palette, line);
                }
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
        for line in hunk.lines.iter().take(4) {
            push_diff_content_line(lines, palette, line);
        }
        if hunk.lines.len() > 4 {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    t!("app.diff.more_lines_hidden", count = hunk.lines.len() - 4).into_owned(),
                    palette.muted(),
                ),
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

fn diff_file_line_counts(file: &crate::model::DiffPreviewFile) -> (usize, usize) {
    file.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .fold((0, 0), |(added, removed), line| match line.kind.as_str() {
            "added" | "insert" | "inserted" => (added + 1, removed),
            "removed" | "delete" | "deleted" => (added, removed + 1),
            _ => (added, removed),
        })
}

fn diff_file_type_badge(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "rs" => "RUST",
        "toml" => "TOML",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "md" | "markdown" => "MD",
        "js" | "jsx" => "JS",
        "ts" | "tsx" => "TS",
        "css" | "scss" | "sass" => "CSS",
        "html" | "htm" => "HTML",
        "sh" | "bash" | "zsh" => "SH",
        "py" => "PY",
        _ => "FILE",
    }
}

fn diff_file_badge_style(badge: &str, palette: Palette) -> Style {
    let fg = match badge {
        "RUST" => palette.danger,
        "TOML" | "JSON" | "YAML" => palette.highlight,
        "MD" => palette.text,
        "JS" | "TS" => palette.accent,
        "CSS" | "HTML" => palette.accent,
        "SH" | "PY" => palette.success,
        _ => palette.muted,
    };
    Style::default()
        .fg(fg)
        .bg(palette.diff_context_bg)
        .add_modifier(Modifier::BOLD)
}

fn shell_command_from_line(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("$ ")
        .or_else(|| trimmed.strip_prefix("command: "))
        .filter(|command| !command.trim().is_empty())
}

fn active_background_tasks(app: &AppState) -> usize {
    app.active_session()
        .map(|session| {
            session
                .tasks
                .iter()
                .filter(|task| matches!(task_state_label(task.state), "pending" | "running"))
                .count()
        })
        .unwrap_or(0)
}

/// Titles of the running sub-agents attributed to an agent-task chip. Each
/// running task is attributed to the chip for its OWN originating turn
/// (`task.turn_id`, stamped by the server per C1 step 4), so two turns can no
/// longer both claim the same global sub-agent count — the "two Orchestrating
/// chips" bug (C5). Background sub-agents outlive the parent turn (it shows
/// "done" while they keep running), and that still works: their `turn_id` keeps
/// pointing at the turn that spawned them, so that — and only that — chip stays
/// "Orchestrating", and lists those agents as its children (so their live
/// progress no longer forms a *second*, turn-less "Orchestrating" chip).
///
/// Tasks the server couldn't stamp with a turn (legacy daemons, `session/open`
/// replay, synthetic emitters → `turn_id == None`) fall back to a SINGLE current
/// chip — the active (live) turn if one exists, else the latest activity-log
/// turn — so they still surface without being double-counted across chips.
fn running_subagent_titles_for_chip(
    app: &AppState,
    turn_id: Option<&octos_core::ui_protocol::TurnId>,
) -> Vec<String> {
    let Some(chip_turn) = turn_id else {
        return Vec::new();
    };
    let Some(session) = app.active_session() else {
        return Vec::new();
    };
    // The one chip that owns turn-less tasks: prefer the active (live) turn; if
    // the turn already finished, this session's latest activity-log turn. At most
    // one chip is ever "current", so unattributed tasks are counted exactly once.
    // Scope the log lookup to the active session (codex P2): `turn_activity_logs`
    // is cross-session, and the tasks we count belong to `session`, so a newer
    // log in a *different* session must not steal this session's fallback chip.
    let current_turn = app.active_turn().map(|(_, t)| t).or_else(|| {
        app.turn_activity_logs
            .iter()
            .rev()
            .find(|log| log.session_id == session.id)
            .map(|log| &log.turn_id)
    });
    let owns_unattributed = current_turn == Some(chip_turn);
    session
        .tasks
        .iter()
        .filter(|task| matches!(task_state_label(task.state), "pending" | "running"))
        .filter(|task| match task.turn_id.as_ref() {
            Some(task_turn) => task_turn == chip_turn,
            None => owns_unattributed,
        })
        .map(|task| task.title.clone())
        .collect()
}

fn render_plan(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let plan = extract_plan_lines(app);
    let lines = if plan.is_empty() {
        vec![
            Line::from(Span::styled(
                t!("app.empty.no_plan").to_string(),
                palette.muted(),
            )),
            Line::from(Span::styled(
                t!("app.empty.no_plan_hint").to_string(),
                palette.muted(),
            )),
        ]
    } else {
        plan.into_iter()
            .enumerate()
            .map(|(idx, step)| {
                let mut spans = vec![
                    Span::styled(format!("{}.", idx + 1), palette.muted()),
                    Span::styled(" ", palette.muted()),
                ];
                spans.extend(plan_step_text_spans(&step.text, palette));
                Line::from(spans)
            })
            .collect()
    };

    Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.plan").to_string(),
                palette,
                false,
                Some(t!("app.plan.live").into_owned()),
            )
            .border_style(palette.border()),
        )
        .wrap(Wrap { trim: false })
}

fn plan_step_text_spans(text: &str, palette: Palette) -> Vec<Span<'static>> {
    inline_markdown_spans(
        text,
        palette.text(),
        palette.title().add_modifier(Modifier::BOLD),
        palette.selected(),
    )
}

fn plain_inline_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        // Mirror the link/strikethrough rendering exactly so the measured width
        // equals what `inline_markdown_spans` draws — otherwise a link in a
        // table cell sizes the column by the raw `[text](url)` markup and can
        // shrink/ellipsize unrelated columns (issue #207).
        if let Some((link_text, url, consumed)) = parse_markdown_link(rest) {
            if link_text == url {
                output.push_str(url);
            } else {
                output.push_str(link_text);
                output.push_str(&format!(" ({url})"));
            }
            rest = &rest[consumed..];
            continue;
        }
        if let Some((struck, consumed)) = parse_markdown_strikethrough(rest) {
            output.push_str(struck);
            rest = &rest[consumed..];
            continue;
        }
        if let Some(after_open) = rest.strip_prefix("**")
            && let Some(close) = after_open.find("**")
        {
            output.push_str(&after_open[..close]);
            rest = &after_open[close + 2..];
            continue;
        }
        if let Some(after_open) = rest.strip_prefix('`')
            && let Some(close) = after_open.find('`')
        {
            output.push_str(&after_open[..close]);
            rest = &after_open[close + 1..];
            continue;
        }
        if let Some((emphasis, consumed)) = markdown_emphasis_segment(rest) {
            output.push_str(emphasis);
            rest = &rest[consumed..];
            continue;
        }
        if let Some(ch) = rest.chars().next() {
            output.push(ch);
            rest = &rest[ch.len_utf8()..];
        } else {
            break;
        }
    }
    output
}

fn extract_plan_lines(app: &AppState) -> Vec<RenderedPlanStep> {
    let mut plan = extract_plan_steps(app);
    normalize_rendered_plan_steps(&mut plan);
    apply_completed_plan_steps_from_history(app, &mut plan);
    plan
}

fn normalize_rendered_plan_steps(plan: &mut [RenderedPlanStep]) {
    for step in plan {
        while let Some((completed, rest)) = strip_leading_plan_checkbox(&step.text) {
            step.completed |= completed;
            step.text = rest.to_string();
        }
    }
}

fn apply_completed_plan_steps_from_history(app: &AppState, plan: &mut [RenderedPlanStep]) {
    if plan.iter().all(|step| step.completed) {
        return;
    }
    let Some(session) = app.active_session() else {
        return;
    };

    let completed_steps = session
        .messages
        .iter()
        .rev()
        .filter(|message| message.role.as_str() == "assistant")
        .flat_map(|message| completed_plan_texts(message.content.as_str()))
        .collect::<Vec<_>>();

    for step in plan.iter_mut().filter(|step| !step.completed) {
        if completed_steps
            .iter()
            .any(|completed| normalize_plan_text(completed) == normalize_plan_text(&step.text))
        {
            step.completed = true;
        }
    }
}

fn completed_plan_texts(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(completed_plan_text_from_line)
        .collect()
}

fn completed_plan_text_from_line(line: &str) -> Option<String> {
    let mut rest = line.trim();
    let mut completed = false;
    let mut saw_marker = false;

    for _ in 0..6 {
        rest = rest.trim_start();
        if let Some((checked, next)) = strip_leading_plan_checkbox(rest) {
            completed |= checked;
            saw_marker = true;
            rest = next;
            continue;
        }
        if let Some(next) = strip_leading_plan_bullet(rest) {
            saw_marker = true;
            rest = next;
            continue;
        }
        if let Some(next) = strip_leading_plan_number(rest) {
            saw_marker = true;
            rest = next;
            continue;
        }
        break;
    }

    let text = rest.trim_start_matches(['.', ')', ' ']).trim();
    (completed && saw_marker && !text.is_empty()).then(|| text.to_string())
}

fn strip_leading_plan_checkbox(line: &str) -> Option<(bool, &str)> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (marker, rest) = rest.split_once(']')?;
    let completed = match marker.trim() {
        "x" | "X" => true,
        "" => false,
        _ => return None,
    };
    Some((completed, rest.trim_start()))
}

fn strip_leading_plan_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
}

fn strip_leading_plan_number(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let split = trimmed.find(['.', ')'])?;
    let (number, rest) = trimmed.split_at(split);
    if number.is_empty() || number.len() > 3 || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let rest = rest[1..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

fn normalize_plan_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn render_workspace(app: &AppState, palette: Palette, area_height: u16) -> Paragraph<'static> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", t!("app.workspace.root")), palette.muted()),
            Span::styled(app.workspace.root.clone(), palette.text()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            t!("app.workspace.contract").into_owned(),
            palette.title(),
        )),
    ];

    for line in &app.workspace.contract {
        lines.push(Line::from(Span::styled(
            format!("  {line}"),
            palette.muted(),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        t!("app.workspace.tree").into_owned(),
        palette.title(),
    )));
    for (idx, entry) in app.workspace.entries.iter().enumerate() {
        let marker = if idx == app.workspace.selected {
            "›"
        } else {
            " "
        };
        let style = if idx == app.workspace.selected {
            palette.selected()
        } else {
            palette.text()
        };
        let indent = "  ".repeat(entry.depth);
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {indent}"), style),
            Span::styled(entry.label.clone(), style),
            Span::styled(format!("  {}", entry.detail), palette.muted()),
        ]));
    }

    let visible_height = usize::from(area_height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_top = app.workspace.scroll.min(max_scroll) as u16;

    Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.workspace").to_string(),
                palette,
                app.focus == FocusPane::Workspace,
                Some(t!("app.workspace.contract").into_owned()),
            )
            .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false })
}

fn render_git(app: &AppState, palette: Palette, area_height: u16) -> Paragraph<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{} ", t!("app.git.branch")), palette.muted()),
        Span::styled(app.git.branch.clone(), palette.text()),
    ])];

    if let Some(head) = &app.git.head {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<6} ", t!("app.git.head")), palette.muted()),
            Span::styled(head.clone(), palette.text()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        t!("app.git.status").into_owned(),
        palette.title(),
    )));
    let mut selected_idx = 0;
    if app.git.status.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", t!("app.git.clean")),
            palette.muted(),
        )));
    } else {
        for item in &app.git.status {
            let selected = app.git.selected == selected_idx;
            let marker = if selected { "›" } else { " " };
            let style = if selected {
                palette.selected()
            } else {
                palette.text()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} {} ", item.code), style),
                Span::styled(item.path.clone(), style),
            ]));
            lines.push(Line::from(Span::styled(
                format!("    {}", item.detail),
                palette.muted(),
            )));
            selected_idx += 1;
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        t!("app.git.history").into_owned(),
        palette.title(),
    )));
    if app.git.history.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", t!("app.git.none")),
            palette.muted(),
        )));
    } else {
        for item in &app.git.history {
            let selected = app.git.selected == selected_idx;
            let marker = if selected { "›" } else { " " };
            let style = if selected {
                palette.selected()
            } else {
                palette.text()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} {} ", item.commit), style),
                Span::styled(item.summary.clone(), style),
            ]));
            selected_idx += 1;
        }
    }

    let visible_height = usize::from(area_height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_top = app.git.scroll.min(max_scroll) as u16;

    Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.git").to_string(),
                palette,
                app.focus == FocusPane::Git,
                Some(t!("app.git.status_history").into_owned()),
            )
            .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false })
}

struct ComposerInputView {
    lines: Vec<String>,
    hidden_lines: usize,
    hidden_prefix: bool,
    cursor_row: u16,
    cursor_width: usize,
}

/// Max width of a per-loop chip label before truncation. Keeps the
/// indicator row compact when several loops are running concurrently.
const AUTONOMY_LOOP_LABEL_MAX: usize = 20;

/// Returns the active session's autonomy mirror, or `None` if either no
/// session is selected or the backend has not yet populated the mirror.
fn active_session_autonomy(app: &AppState) -> Option<&SessionAutonomyState> {
    let session = app.active_session()?;
    app.session_autonomy_for(&session.id)
}

/// Number of rows the sticky autonomy indicator needs: 0 when both goal
/// and loops are absent, 1 when only one is present, 2 when both are.
/// Max plan items rendered in the sticky panel before collapsing to a
/// `… +N more` summary line, so a long checklist can't dominate the screen.
const PLAN_PANEL_MAX_ITEMS: usize = 8;

/// Rows the plan checklist adds to the sticky panel: a header line plus one
/// row per shown item (capped), plus a `+N more` line when truncated.
fn plan_panel_rows(plan: &octos_core::ui_protocol::UiPlanRecord) -> u16 {
    if plan.items.is_empty() {
        return 0;
    }
    let shown = plan.items.len().min(PLAN_PANEL_MAX_ITEMS);
    let overflow = usize::from(plan.items.len() > PLAN_PANEL_MAX_ITEMS);
    (1 + shown + overflow) as u16
}

fn autonomy_indicator_height(app: &AppState) -> u16 {
    match active_session_autonomy(app) {
        Some(state) => {
            let mut rows = 0u16;
            if state.goal.is_some() {
                rows += 1;
            }
            if !state.loops.is_empty() {
                rows += 1;
            }
            if let Some(plan) = state.plan.as_ref() {
                rows += plan_panel_rows(plan);
            }
            rows
        }
        None => 0,
    }
}

/// Trim a loop's prompt down to a chip-sized label. Prefers the first
/// line for legibility; falls back to a UTF-8 safe char-boundary cut at
/// [`AUTONOMY_LOOP_LABEL_MAX`].
fn autonomy_loop_label(record: &octos_core::ui_protocol::UiLoopRecord) -> String {
    let prompt = record.prompt.trim();
    if prompt.is_empty() {
        return record
            .loop_id
            .chars()
            .take(AUTONOMY_LOOP_LABEL_MAX)
            .collect();
    }
    let first_line = prompt.lines().next().unwrap_or(prompt).trim();
    if first_line.chars().count() <= AUTONOMY_LOOP_LABEL_MAX {
        first_line.to_string()
    } else {
        let mut truncated: String = first_line
            .chars()
            .take(AUTONOMY_LOOP_LABEL_MAX.saturating_sub(1))
            .collect();
        truncated.push('…');
        truncated
    }
}

/// Format the cadence prefix for a loop chip (e.g. `5m`, `2h`,
/// `self-paced`, `maintenance`). Unknown modes pass through verbatim.
fn autonomy_loop_cadence(record: &octos_core::ui_protocol::UiLoopRecord) -> String {
    match record.mode.as_str() {
        "fixed_interval" => match record.interval_seconds {
            Some(secs) if secs >= 3600 && secs % 3600 == 0 => format!("{}h", secs / 3600),
            Some(secs) if secs >= 60 && secs % 60 == 0 => format!("{}m", secs / 60),
            Some(secs) => format!("{secs}s"),
            None => "interval".to_string(),
        },
        "self_paced" => "self-paced".to_string(),
        "maintenance" => "maintenance".to_string(),
        other => other.to_string(),
    }
}

/// True when a loop is in the runnable `"active"` state. Paused / deleted
/// loops still appear in the chip row but are dimmed.
fn autonomy_loop_is_active(record: &octos_core::ui_protocol::UiLoopRecord) -> bool {
    record.status == "active"
}

/// Build the line set for the sticky autonomy indicator. Returns 0, 1,
/// or 2 lines (goal first, then loops).
fn autonomy_indicator_lines(app: &AppState, palette: Palette) -> Vec<Line<'static>> {
    let Some(state) = active_session_autonomy(app) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(goal) = state.goal.as_ref() {
        let objective = if goal.objective.trim().is_empty() {
            goal.goal_id.clone()
        } else {
            goal.objective.clone()
        };
        let parenthetical = t!(
            "app.autonomy.goal_meta",
            status = goal.status,
            used = goal.tokens_used,
            budget = goal.token_budget
        )
        .into_owned();
        lines.push(Line::from(vec![
            Span::styled(
                "◆ ",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
                    .bg(palette.surface),
            ),
            Span::styled(
                t!("app.autonomy.goal_prefix").to_string(),
                palette.title().bg(palette.surface),
            ),
            Span::styled(objective, palette.text().bg(palette.surface)),
            Span::styled(parenthetical, palette.muted().bg(palette.surface)),
        ]));
    }
    if !state.loops.is_empty() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let running = state
            .loops
            .iter()
            .filter(|l| autonomy_loop_is_active(l))
            .count();
        // Chips render every non-deleted loop (active + paused). Count paused
        // too so the header reconciles with the chips — a `--solo` boot parks
        // active loops to `paused`, which otherwise reads as "0 running" beside
        // several visible chips.
        let paused = state.loops.iter().filter(|l| l.status == "paused").count();
        spans.push(Span::styled(
            "↻ ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .bg(palette.surface),
        ));
        let mut loops_label = t!("app.autonomy.loops_running", count = running).to_string();
        if paused > 0 {
            loops_label.push_str(&t!("app.autonomy.loops_paused_suffix", count = paused));
        }
        spans.push(Span::styled(
            loops_label,
            palette.title().bg(palette.surface),
        ));
        spans.push(Span::styled("   ", palette.text().bg(palette.surface)));
        for record in &state.loops {
            let label = autonomy_loop_label(record);
            let cadence = autonomy_loop_cadence(record);
            let chip = format!("[{cadence} {label}]");
            let chip_style = if autonomy_loop_is_active(record) {
                palette.text().bg(palette.surface)
            } else {
                palette.muted().bg(palette.surface)
            };
            spans.push(Span::styled(chip, chip_style));
            spans.push(Span::styled(" ", palette.text().bg(palette.surface)));
        }
        // Drop the trailing space for tidiness.
        if matches!(spans.last(), Some(s) if s.content == " ") {
            spans.pop();
        }
        lines.push(Line::from(spans));
    }
    if let Some(plan) = state.plan.as_ref() {
        lines.extend(plan_indicator_lines(plan, palette));
    }
    lines
}

/// Render the model-authored plan/todo checklist as a header line
/// (`✶ <activity> (done/total)`) plus a `⎿`-anchored tree of items with a
/// per-status glyph. Mirrors the sub-agent task-group tree visual.
fn plan_indicator_lines(
    plan: &octos_core::ui_protocol::UiPlanRecord,
    palette: Palette,
) -> Vec<Line<'static>> {
    use octos_core::ui_protocol::PlanItemStatus;
    if plan.items.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let total = plan.items.len();
    let done = plan
        .items
        .iter()
        .filter(|item| item.status == PlanItemStatus::Completed)
        .count();
    // Header: prefer the model's activity label, else the in-progress item,
    // else a generic fallback.
    let title = plan
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            plan.items
                .iter()
                .find(|item| item.status == PlanItemStatus::InProgress)
                .map(|item| item.title.clone())
        })
        .unwrap_or_else(|| "Plan".to_string());
    lines.push(Line::from(vec![
        Span::styled(
            "✶ ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .bg(palette.surface),
        ),
        Span::styled(title, palette.title().bg(palette.surface)),
        Span::styled(
            format!("  ({done}/{total})"),
            palette.muted().bg(palette.surface),
        ),
    ]));
    for (idx, item) in plan.items.iter().take(PLAN_PANEL_MAX_ITEMS).enumerate() {
        let (glyph, glyph_style) = match item.status {
            PlanItemStatus::Completed => (
                "✔",
                Style::default().fg(palette.success).bg(palette.surface),
            ),
            PlanItemStatus::InProgress => (
                "▸",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
                    .bg(palette.surface),
            ),
            PlanItemStatus::Pending => ("◼", palette.muted().bg(palette.surface)),
        };
        // `⎿` anchors the first child; the rest align under the glyph.
        let prefix = if idx == 0 { "  ⎿  " } else { "     " };
        let mut spans = vec![
            Span::styled(prefix, palette.muted().bg(palette.surface)),
            Span::styled(format!("{glyph} "), glyph_style),
        ];
        if let Some(priority) = item.priority.as_ref().filter(|p| !p.trim().is_empty()) {
            spans.push(Span::styled(
                format!("{priority} "),
                palette.muted().bg(palette.surface),
            ));
        }
        let item_style = if item.status == PlanItemStatus::Completed {
            palette.muted().bg(palette.surface)
        } else {
            palette.text().bg(palette.surface)
        };
        spans.push(Span::styled(item.title.clone(), item_style));
        lines.push(Line::from(spans));
    }
    if plan.items.len() > PLAN_PANEL_MAX_ITEMS {
        let more = plan.items.len() - PLAN_PANEL_MAX_ITEMS;
        lines.push(Line::from(Span::styled(
            format!("     … +{more} more"),
            palette.muted().bg(palette.surface),
        )));
    }
    lines
}

fn render_autonomy_indicator(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let lines = autonomy_indicator_lines(app, palette);
    Paragraph::new(Text::from(lines)).style(Style::default().fg(palette.text).bg(palette.surface))
}

/// Fallback context-window denominator for `ctx N%`, used only until a cost
/// update carries the real per-model window (`token_cost.context_window`, stored
/// in `AppState::session_context_window`). Surfaces the inspector-only
/// `token_estimate` as a glanceable budget bar in the harness status row.
const DEFAULT_CONTEXT_WINDOW_TOKENS: usize = 128_000;

/// Compact token count for the harness row: `34211` -> `34.2k`.
fn humanize_token_count(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

/// True when the harness has live state worth surfacing in the dedicated
/// status row: the active session is orchestrating (server `active`) OR a turn
/// is in progress locally. Idle → the row collapses to height 0 so it can
/// never collide with the composer's top-border chrome (the prior revert,
/// 249fe652, drew the indicator ON the composer border).
fn harness_status_active(app: &AppState) -> bool {
    let orchestrating = app
        .active_session()
        .and_then(|session| app.orchestration.get(&session.id))
        .is_some_and(|status| status.active);
    orchestrating || matches!(app.run_state, SessionRunState::InProgress)
}

/// Rows the harness status indicator needs: 1 when active, 0 when idle.
fn harness_status_height(app: &AppState) -> u16 {
    if harness_status_active(app) { 1 } else { 0 }
}

/// Context-window fill ratio (0.0..=1.0) for the harness row `LineGauge`, or
/// `None` when no `token_estimate` is known for the active session yet.
fn harness_context_ratio(app: &AppState) -> Option<f64> {
    let session = app.active_session()?;
    let token_estimate = app
        .context_lifecycle_for(&session.id)?
        .state
        .as_ref()?
        .token_estimate;
    // Prefer the real per-model context window carried on the wire
    // (`metadata.token_cost.context_window`); fall back to the fixed default
    // only until the first cost update arrives for this session.
    let window = app
        .session_context_window
        .get(&session.id)
        .copied()
        .filter(|w| *w > 0)
        .map(|w| w as usize)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
    if window == 0 {
        return None;
    }
    Some((token_estimate as f64 / window as f64).clamp(0.0, 1.0))
}

/// Integer context-window percent (0..=100) for the `ctx N%` label.
fn harness_context_percent(app: &AppState) -> Option<u16> {
    harness_context_ratio(app).map(|ratio| (ratio * 100.0).round() as u16)
}

/// Build the harness status line(s): spinner + phase + agent count +
/// re-entering + token in/out + cost + retry + ctx %. Empty when idle.
fn harness_status_lines(
    app: &AppState,
    palette: Palette,
    include_ctx_text: bool,
) -> Vec<Line<'static>> {
    if !harness_status_active(app) {
        return Vec::new();
    }
    let Some(session) = app.active_session() else {
        return Vec::new();
    };
    let session_id = session.id.clone();
    let status = app.orchestration.get(&session_id);

    let phase = match status.and_then(|s| s.phase.as_deref()) {
        Some("orchestrating") => t!("app.harness.orchestrating").to_string(),
        Some("re-entering") => t!("app.harness.re_entering").to_string(),
        Some("working") => t!("app.harness.working").to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => t!("app.harness.working").to_string(),
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    // Water-wave gradient on "spinner + phase" (e.g. "⣻ Working"): a bright crest
    // ripples across the label, advanced by the ~25ms animation redraw via the
    // shared process clock. Uses Color::Rgb like the rest of octos-tui's themes
    // (truecolor-assuming, so it works over SSH where COLORTERM isn't forwarded);
    // the non-RGB Terminal theme degrades to a neutral-grey ripple via rgb_of.
    let label = format!("{} {}", spinner_frame(), phase);
    let stops = [
        rgb_of(palette.muted),
        rgb_of(palette.accent),
        rgb_of(palette.highlight),
    ];
    spans.extend(wave_gradient_spans(
        &label,
        anim_time_secs() * 3.0,
        &stops,
        palette.surface,
    ));

    if let Some(status) = status {
        if status.running_agents > 0 {
            spans.push(Span::styled(
                format!(
                    " · {}",
                    t!("app.statusbar.agents", count = status.running_agents)
                ),
                palette.text().bg(palette.surface),
            ));
        }
        // The re-entry gap (sub-agents settled, a continuation queued) is the
        // whole reason for this row: it must NOT read as done.
        if status.pending_continuations > 0 {
            spans.push(Span::styled(
                format!(" · {}", t!("app.statusbar.re_entering")),
                palette.muted().bg(palette.surface),
            ));
        }
    }

    // Token in/out + cumulative session cost (from token_cost progress).
    if let Some((input, output, cost)) = app.session_usage.get(&session_id) {
        if input.is_some() || output.is_some() {
            spans.push(Span::styled(
                format!(
                    " · ↑{} ↓{}",
                    humanize_token_count(input.unwrap_or(0)),
                    humanize_token_count(output.unwrap_or(0)),
                ),
                palette.text().bg(palette.surface),
            ));
        }
        if let Some(cost) = cost.filter(|c| *c > 0.0) {
            spans.push(Span::styled(
                format!(" · ${cost:.4}"),
                palette.muted().bg(palette.surface),
            ));
        }
    }

    // Retry/backoff (metadata.retry — previously ignored on the wire).
    if let Some(retry) = app.session_retry.get(&session_id) {
        let attempt = match (retry.attempt, retry.max_attempts) {
            (Some(a), Some(max)) => format!(
                " · {}",
                t!("app.statusbar.retrying_attempt_max", attempt = a, max = max)
            ),
            (Some(a), None) => format!(" · {}", t!("app.statusbar.retrying_attempt", attempt = a)),
            _ => format!(" · {}", t!("app.statusbar.retrying")),
        };
        spans.push(Span::styled(
            attempt,
            palette
                .muted()
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Context window %. This textual label is the NARROW-terminal fallback:
    // when `render_harness_status_row` draws the LineGauge (wide terminal) it
    // passes `include_ctx_text = false` so the percent does not render twice —
    // once as this text on the left and once as the gauge's own label on the
    // right (the duplicate-`ctx ~N%` bug). Kept (and unit-tested) for narrow
    // terminals where the gauge column is dropped.
    if include_ctx_text {
        if let Some(percent) = harness_context_percent(app) {
            // `~` marks this as an estimate: the numerator is the harness
            // `token_estimate`. The denominator is the real per-model context
            // window once a cost update carries it (`token_cost.context_window`),
            // falling back to `DEFAULT_CONTEXT_WINDOW_TOKENS` until then.
            spans.push(Span::styled(
                format!(" · ctx ~{percent}%"),
                palette.muted().bg(palette.surface),
            ));
        }
    }

    vec![Line::from(spans)]
}

/// Render the dedicated harness status row. Splits the row so the textual
/// status sits on the left and a `LineGauge` context-window bar sits on the
/// right when a `token_estimate` is known. Drawn into its own layout row
/// (never the composer border).
fn render_harness_status_row(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
    area: Rect,
) {
    let ratio = harness_context_ratio(app);
    // Reserve a fixed-width column for the context gauge only when we have a
    // ratio to show AND the row is wide enough for both the text and the gauge.
    const GAUGE_WIDTH: u16 = 18;
    let show_gauge = ratio.is_some() && area.width > GAUGE_WIDTH + 12;
    // Suppress the textual `· ctx ~N%` label when the gauge will be drawn —
    // otherwise the percent renders twice on the same row (text on the left and
    // gauge on the right). The gauge is canonical on a wide terminal; the text
    // is the narrow-terminal fallback.
    let lines = harness_status_lines(app, palette, !show_gauge);
    if lines.is_empty() {
        return;
    }
    if let Some(ratio) = ratio.filter(|_| show_gauge) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(12), Constraint::Length(GAUGE_WIDTH)])
            .split(area);
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().fg(palette.text).bg(palette.surface)),
            split[0],
        );
        let percent = (ratio * 100.0).round() as u16;
        let gauge = LineGauge::default()
            .ratio(ratio)
            // Base style backs the label cells: `LineGauge` paints the whole
            // area with `self.style` before writing the (unstyled) label, so
            // without a surface bg here the `ctx ~N%` label renders on the raw
            // terminal background — a mismatched block to the right of the
            // harness row, just above the composer. Keep it on `surface`.
            .style(Style::default().fg(palette.muted).bg(palette.surface))
            .label(format!("ctx ~{percent}%"))
            .filled_style(Style::default().fg(palette.accent).bg(palette.surface))
            .unfilled_style(Style::default().fg(palette.frame).bg(palette.surface));
        frame.render_widget(gauge, split[1]);
    } else {
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().fg(palette.text).bg(palette.surface)),
            area,
        );
    }
}

/// The current model id for the active session, drawn on the composer's bottom
/// border. Prefers the runtime status's reported model, then its runtime policy
/// stamp, then the model catalog's selected entry — so the footer reflects the
/// current model whether it arrived via `session/status/read`, a model
/// selection, or just the `/model` catalog. `None` until any of those is known
/// (the footer then shows only the cwd).
fn composer_footer_model(app: &AppState) -> Option<String> {
    let session_id = &app.active_session()?.id;
    let from_status = app.runtime_status_for(session_id).and_then(|status| {
        status
            .model
            .as_ref()
            .map(|model| model.model.clone())
            .or_else(|| {
                status
                    .runtime_policy_stamp
                    .as_ref()
                    .and_then(|stamp| stamp.model.clone())
            })
    });
    from_status
        .or_else(|| {
            app.model_catalog_for(session_id).and_then(|catalog| {
                catalog
                    .models
                    .iter()
                    .find(|model| model.selected)
                    .map(|model| model.model.clone())
            })
        })
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

fn render_composer(app: &AppState, palette: Palette, area: Rect) -> Paragraph<'static> {
    let mut lines = Vec::new();
    let composer = app.composer_presentation();
    let input_view = match &composer {
        ComposerPresentation::Inline(text) => Some(composer_input_view(
            text,
            app.composer_cursor_index(),
            area.width,
            area.height.saturating_sub(COMPOSER_CHROME_ROWS),
        )),
        ComposerPresentation::Empty | ComposerPresentation::Collapsed(_) => None,
    };
    if !app.pending_messages.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            t!(
                "app.composer_hint.queued_messages",
                count = app.pending_messages.len()
            )
            .to_string(),
            palette.muted().bg(palette.surface),
        )]));
    } else if matches!(&composer, ComposerPresentation::Collapsed(_)) {
        lines.push(Line::from(vec![Span::styled(
            t!("app.composer_hint.large_paste").to_string(),
            palette.muted().bg(palette.surface),
        )]));
    } else if let Some(view) = &input_view
        && (view.hidden_lines > 0 || view.hidden_prefix)
    {
        let hint = if view.hidden_lines > 0 {
            t!(
                "app.composer_hint.multiline_tail_lines",
                count = view.hidden_lines
            )
            .to_string()
        } else {
            t!("app.composer_hint.multiline_tail_line").to_string()
        };
        lines.push(Line::from(vec![Span::styled(
            hint,
            palette.muted().bg(palette.surface),
        )]));
    } else {
        lines.push(Line::from(Span::styled(
            " ",
            palette.text().bg(palette.surface),
        )));
    }
    match &composer {
        ComposerPresentation::Empty if onboarding_first_launch_active(app) => {
            lines.push(Line::from(vec![
                Span::styled(" › ", palette.selected().bg(palette.surface)),
                Span::styled(
                    format!(" {}", t!("app.banner.onboarding_setup")),
                    palette.muted().bg(palette.surface),
                ),
            ]))
        }
        ComposerPresentation::Empty => lines.push(Line::from(vec![
            Span::styled(" › ", palette.selected().bg(palette.surface)),
            Span::styled(
                format!(" {}", t!("composer.placeholder")),
                palette.muted().bg(palette.surface),
            ),
        ])),
        ComposerPresentation::Inline(_) => {
            if let Some(view) = input_view.as_ref() {
                let text_width = composer_text_width(area.width);
                let mut first_row = true;
                for line in view.lines.iter() {
                    for chunk in wrap_composer_line(line, text_width) {
                        let prefix = if first_row { " › " } else { "   " };
                        let prefix_style = if first_row {
                            palette.selected().bg(palette.surface)
                        } else {
                            palette.muted().bg(palette.surface)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(prefix, prefix_style),
                            Span::styled(chunk, palette.text().bg(palette.surface)),
                        ]));
                        first_row = false;
                    }
                }
            }
        }
        ComposerPresentation::Collapsed(collapse) => lines.push(Line::from(vec![
            Span::styled(" › ", palette.selected().bg(palette.surface)),
            Span::styled("[paste] ", palette.selected().bg(palette.surface)),
            Span::styled(collapse.summary.clone(), palette.text().bg(palette.surface)),
        ])),
    }

    match composer {
        ComposerPresentation::Collapsed(collapse) => {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("   {}: ", t!("app.composer.preview_label")),
                    palette.muted().bg(palette.surface),
                ),
                Span::styled(collapse.preview, palette.text().bg(palette.surface)),
            ]));
        }
        ComposerPresentation::Empty | ComposerPresentation::Inline(_) => {
            lines.push(Line::from(Span::styled(
                " ",
                palette.text().bg(palette.surface),
            )));
        }
    }

    // When Vim mode is on, surface the current Normal/Insert mode in the title
    // so the user always knows which mode their keys act in.
    let title = if app.vim_mode {
        let mode = if app.composer_mode == crate::model::ComposerMode::Normal {
            t!("app.composer.vim_normal")
        } else {
            t!("app.composer.vim_insert")
        };
        format!("{} · {}", t!("app.pane.composer"), mode)
    } else {
        t!("app.pane.composer").to_string()
    };
    let mut block = titled_block(
        title,
        palette,
        app.focus == FocusPane::Composer,
        Some(t!("app.hint.composer_send").into_owned()),
    )
    .border_style(palette.border());

    // Surface the working directory (bottom-left) and current model
    // (bottom-right) right on the composer's bottom border. Both stay visible
    // at the input without consuming a content row — the bottom border already
    // exists. The cwd prefers the active session's server-confirmed workspace
    // root (populated by `session/status/read`), so after a session switch the
    // footer shows THAT session's workspace; the client-side `workspace.root`
    // is the fallback until a runtime status arrives.
    let cwd = app
        .active_session()
        .and_then(|session| app.runtime_status_for(&session.id))
        .and_then(|status| {
            status
                .workspace_root
                .as_deref()
                .or(status.cwd.as_deref())
                .filter(|root| !root.trim().is_empty())
        })
        .unwrap_or(app.workspace.root.as_str());
    let cwd_title = format!(" {} ", short_path(cwd));
    if let Some(model) = composer_footer_model(app) {
        let model_title = format!(" {} ", truncate_terminal_line(&model, 28));
        // Both bottom titles share one border row and ratatui paints
        // overlapping titles over each other. The model is the footer's
        // SOLE persistent display now that the status line no longer echoes
        // it (the de-dup), so when the border is too narrow for both, keep
        // the model and drop the cwd rather than hiding the model entirely
        // (which would leave the active model visible nowhere).
        let inner_width = area.width.saturating_sub(2) as usize;
        if cwd_title.width() + model_title.width() <= inner_width {
            block = block
                .title_bottom(Line::from(Span::styled(cwd_title, palette.muted())).left_aligned());
        }
        block = block.title_bottom(
            Line::from(Span::styled(
                model_title,
                Style::default().fg(palette.accent),
            ))
            .right_aligned(),
        );
    } else {
        block =
            block.title_bottom(Line::from(Span::styled(cwd_title, palette.muted())).left_aligned());
    }

    Paragraph::new(Text::from(lines))
        .style(Style::default().fg(palette.text).bg(palette.surface))
        .block(block)
}

fn set_composer_cursor(frame: &mut impl FrameLike, app: &AppState, area: Rect) {
    if app.focus != FocusPane::Composer {
        return;
    }
    if let Some(position) = composer_cursor_position(app, area) {
        frame.set_cursor_position(position);
    }
}

fn composer_cursor_position(app: &AppState, area: Rect) -> Option<Position> {
    if area.width <= 2 || area.height <= 2 {
        return None;
    }

    let (row_offset, text_width) = composer_cursor_row_and_width(
        &app.composer_presentation(),
        app.composer_cursor_index(),
        area,
    );
    let input_y = area.y + 2 + row_offset;
    if input_y >= area.y + area.height.saturating_sub(1) {
        return None;
    }

    let text_width = text_width as u16;
    let inner_right = area.x + area.width.saturating_sub(2);
    let input_x = area.x + 4 + text_width;
    Some(Position::new(input_x.min(inner_right), input_y))
}

fn composer_cursor_row_and_width(
    composer: &ComposerPresentation,
    cursor: usize,
    area: Rect,
) -> (u16, usize) {
    match composer {
        ComposerPresentation::Empty => (0, 0),
        ComposerPresentation::Inline(text) => {
            let view = composer_input_view(
                text,
                cursor,
                area.width,
                area.height.saturating_sub(COMPOSER_CHROME_ROWS),
            );
            (view.cursor_row, view.cursor_width)
        }
        ComposerPresentation::Collapsed(collapse) => {
            (0, "[paste] ".width() + collapse.summary.width())
        }
    }
}

fn composer_input_view(
    text: &str,
    cursor: usize,
    terminal_width: u16,
    max_rows: u16,
) -> ComposerInputView {
    let width = composer_text_width(terminal_width);
    let max_rows = usize::from(max_rows.max(1));
    let logical_lines = composer_logical_lines(text);
    let cursor = cursor.min(text.len());
    let cursor_line_index = logical_lines
        .iter()
        .position(|line| cursor <= line.end)
        .unwrap_or_else(|| logical_lines.len().saturating_sub(1));
    let line_window_end = if cursor == text.len() {
        logical_lines.len().saturating_sub(1)
    } else {
        cursor_line_index
    };
    let mut selected = Vec::new();
    let mut used_rows = 0usize;
    let mut hidden_prefix = false;
    let mut selected_cursor_line = 0usize;
    let mut cursor_width = 0usize;
    let mut cursor_row = 0usize;

    for index in (0..=line_window_end).rev() {
        let line = &logical_lines[index];
        let rows = visual_rows_for_text(line.text, width);
        if used_rows == 0 && rows > max_rows {
            let line_cursor = cursor.saturating_sub(line.start).min(line.text.len());
            let visible = tail_around_cursor(line.text, line_cursor, width, max_rows);
            cursor_row = cursor_row_for_text(&visible.before_cursor, width);
            cursor_width = cursor_width_for_text(&visible.before_cursor, width);
            selected_cursor_line = 0;
            selected.push(visible.text);
            hidden_prefix = true;
            break;
        }
        if used_rows + rows > max_rows {
            break;
        }
        if index == cursor_line_index {
            let before_cursor =
                &line.text[..cursor.saturating_sub(line.start).min(line.text.len())];
            cursor_row = cursor_row_for_text(before_cursor, width);
            cursor_width = cursor_width_for_text(before_cursor, width);
            selected_cursor_line = selected.len();
        }
        selected.push(line.text.to_string());
        used_rows += rows;
    }

    selected.reverse();
    selected_cursor_line = selected
        .len()
        .saturating_sub(1)
        .saturating_sub(selected_cursor_line);
    if selected.is_empty() {
        selected.push(String::new());
    }

    let hidden_lines = logical_lines.len().saturating_sub(selected.len());
    let rows_before_cursor = selected
        .iter()
        .take(selected_cursor_line)
        .map(|line| visual_rows_for_text(line, width))
        .sum::<usize>();

    ComposerInputView {
        lines: selected,
        hidden_lines,
        hidden_prefix,
        cursor_row: rows_before_cursor.saturating_add(cursor_row) as u16,
        cursor_width,
    }
}

struct ComposerLogicalLine<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn composer_logical_lines(text: &str) -> Vec<ComposerLogicalLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for line in text.split('\n') {
        let end = start + line.len();
        lines.push(ComposerLogicalLine {
            text: line,
            start,
            end,
        });
        start = end.saturating_add(1);
    }
    if lines.is_empty() {
        lines.push(ComposerLogicalLine {
            text: "",
            start: 0,
            end: 0,
        });
    }
    lines
}

struct VisibleCursorLine {
    text: String,
    before_cursor: String,
}

fn tail_around_cursor(
    text: &str,
    cursor: usize,
    width: usize,
    max_rows: usize,
) -> VisibleCursorLine {
    let prefix = &text[..cursor.min(text.len())];
    // Whole line fits the budget: show it unchanged. Measured via the same
    // grapheme wrapping render uses, so this can't disagree with what is drawn.
    if visual_rows_for_text(text, width) <= max_rows {
        return VisibleCursorLine {
            text: text.to_string(),
            before_cursor: prefix.to_string(),
        };
    }
    // Line is taller than the budget. If the cursor is still within the first
    // `max_rows` rows, show the HEAD window (the first `max_rows` wrapped rows)
    // — the cursor is already inside it — so render never emits more rows than
    // the composer reserved (which would clip the footer).
    let cursor_chunks = wrap_composer_line(prefix, width);
    if cursor_chunks.len() <= max_rows {
        let chunks = wrap_composer_line(text, width);
        let head: String = chunks[..max_rows.min(chunks.len())].concat();
        return VisibleCursorLine {
            text: head,
            before_cursor: prefix.to_string(),
        };
    }
    // Cursor is past the budget: show the tail of `prefix` ending at the cursor.
    // Keep the last `max_rows - 1` wrapped rows and reserve the first row for the
    // "..." marker, so the window never exceeds `max_rows` rows even when
    // double-width graphemes leave spare columns at a row boundary.
    let keep = max_rows.saturating_sub(1).max(1);
    let start = cursor_chunks.len().saturating_sub(keep);
    let tail: String = cursor_chunks[start..].concat();
    let text = format!("...{tail}");
    VisibleCursorLine {
        text: text.clone(),
        before_cursor: text,
    }
}

fn cursor_row_for_text(text: &str, width: usize) -> usize {
    // Row index of the cursor within its logical line, derived from the same
    // grapheme wrapping render uses (wrap_composer_line) so the cursor sits on
    // the row the text is actually drawn on.
    wrap_composer_line(text, width).len().saturating_sub(1)
}

fn cursor_width_for_text(text: &str, width: usize) -> usize {
    // Display column of the cursor within its row: the width of the last wrapped
    // chunk (0 for empty input).
    wrap_composer_line(text, width)
        .last()
        .map(|chunk| chunk.width())
        .unwrap_or(0)
}

fn render_status(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mode = if app.readonly {
        t!("app.status.read_only").to_string()
    } else {
        t!("app.status.interactive").to_string()
    };
    let turn = app
        .active_turn()
        .map(|(_, turn_id)| {
            t!(
                "app.status.turn_active",
                id = short_id(&turn_id.0.to_string())
            )
            .to_string()
        })
        .unwrap_or_else(|| t!("app.status.turn_idle").to_string());
    let profile = app
        .active_session()
        .and_then(|session| session.profile_id.as_deref())
        .unwrap_or("default");
    let policy = if app.readonly {
        t!("app.status.sends_disabled").to_string()
    } else {
        t!("app.status.approval_gated").to_string()
    };
    let context = app
        .active_session()
        .map(|session| {
            t!(
                "app.statusbar.msgs_tasks",
                msgs = session.messages.len(),
                tasks = session.tasks.len()
            )
            .into_owned()
        })
        .unwrap_or_else(|| t!("app.status.no_session").to_string());
    // Loop chip: an ACTIVE loop fires real model turns on an interval —
    // the operator must see that at a glance, or a forgotten loop burns
    // tokens invisibly (it only ever showed in the server log). Paused
    // loops (e.g. parked by the solo-boot safety) surface too so the
    // operator knows `/loop resume` is available.
    let loop_chip = app
        .active_session()
        .map(|session| app.session_loop_counts(&session.id))
        .filter(|(active, paused)| *active > 0 || *paused > 0)
        .map(|(active, paused)| {
            if active > 0 {
                t!("app.statusbar.loops_active", count = active).into_owned()
            } else {
                t!("app.statusbar.loops_paused", count = paused).into_owned()
            }
        });
    let context = match loop_chip {
        Some(chip) => format!("{context} | {chip}"),
        None => context,
    };
    let work = status_bar_work_text(app);
    let key_hint = hint_bar_text(hint_bar_model(app));

    Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", t!("app.status.state_label")),
            palette.title().bg(palette.surface_alt),
        ),
        Span::styled(
            run_state_marker(&app.run_state).to_string(),
            run_state_style(&app.run_state, palette).bg(palette.surface_alt),
        ),
        Span::styled(" ", palette.muted().bg(palette.surface_alt)),
        Span::styled(
            run_state_status_label(&app.run_state).to_string(),
            run_state_style(&app.run_state, palette).bg(palette.surface_alt),
        ),
        Span::styled(format!(" {work}"), palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(policy.to_string(), palette.text().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(profile.to_string(), palette.text().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(context, palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(app.status.clone(), palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(
            format!("{mode} {turn}"),
            palette.muted().bg(palette.surface_alt),
        ),
        // The cwd deliberately lives on the composer's bottom border, not here —
        // repeating it one line below the composer read as clutter.
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(key_hint, palette.selected().bg(palette.surface_alt)),
    ]))
    .style(Style::default().fg(palette.text).bg(palette.surface_alt))
}

fn hint_bar_text(model: HintBarModel) -> String {
    match model.mode {
        HintBarMode::StatusbarKeys => t!("app.hint.statusbar_keys").into_owned(),
        HintBarMode::Menu => t!("app.hint.menu").into_owned(),
        HintBarMode::Onboarding => t!("app.hint.onboarding").into_owned(),
        HintBarMode::Approval => t!("app.hint.approval").into_owned(),
        HintBarMode::UserQuestion => t!("app.hint.user_question").into_owned(),
        HintBarMode::PagerKeys => t!("app.hint.pager_keys").into_owned(),
        HintBarMode::PagerReviewing => t!("app.hint.pager_reviewing").into_owned(),
        HintBarMode::ActivityNavigator => t!("app.hint.activity_navigator").into_owned(),
    }
}

fn status_bar_work_text(app: &AppState) -> String {
    let mut parts = Vec::new();
    match &app.run_state {
        SessionRunState::Blocked { message } | SessionRunState::Error { message }
            if !message.trim().is_empty() =>
        {
            parts.push(truncate_terminal_line(message, 80));
        }
        _ => {}
    }
    if let Some(seconds) = app.run_state_elapsed_secs() {
        parts.push(format_elapsed_secs(seconds));
    }
    let background_tasks = active_background_tasks(app);
    if background_tasks > 0 {
        parts.push(t!("app.statusbar.background_tasks", count = background_tasks).into_owned());
        parts.push(t!("app.statusbar.ps_to_view").into_owned());
    }
    if app.active_turn().is_some() {
        parts.push(t!("app.statusbar.esc_interrupt").into_owned());
        parts.push(t!("app.statusbar.stop_to_close").into_owned());
    }
    if app.expanded_tool_outputs {
        parts.push(t!("app.statusbar.tool_output_expanded").into_owned());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("({})", parts.join(" | "))
    }
}

fn run_state_status_label(state: &SessionRunState) -> String {
    match state {
        SessionRunState::Idle => t!("app.status.idle").to_string(),
        SessionRunState::InProgress => t!("app.status.working").to_string(),
        SessionRunState::Blocked { .. } => t!("app.status.blocked").to_string(),
        SessionRunState::Success => t!("app.status.done").to_string(),
        SessionRunState::Error { .. } => t!("app.status.error").to_string(),
    }
}

fn run_state_style(state: &SessionRunState, palette: Palette) -> Style {
    match state {
        SessionRunState::Idle => palette.muted(),
        SessionRunState::InProgress => palette.selected().add_modifier(Modifier::BOLD),
        SessionRunState::Blocked { .. } => Style::default()
            .fg(palette.highlight)
            .add_modifier(Modifier::BOLD),
        SessionRunState::Success => Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD),
        SessionRunState::Error { .. } => Style::default()
            .fg(palette.danger)
            .add_modifier(Modifier::BOLD),
    }
}

fn run_state_marker(state: &SessionRunState) -> &'static str {
    match state {
        // Pin the swimming octopus to the always-visible status bar: on a big
        // turn the transcript's "Orchestrating" chip scrolls above the fold, so
        // this is the reliable "still working" signal that never scrolls away.
        // Time-based like the transcript spinner; the status bar redraws every
        // frame so it animates smoothly.
        SessionRunState::InProgress => spinner_frame(),
        SessionRunState::Blocked { .. } => "!",
        SessionRunState::Success => "✓",
        SessionRunState::Error { .. } => "x",
        SessionRunState::Idle => "·",
    }
}

fn short_id(id: &str) -> String {
    const MAX_ID_LEN: usize = 8;
    if id.len() <= MAX_ID_LEN {
        id.to_string()
    } else {
        id[..MAX_ID_LEN].to_string()
    }
}

/// Resolve the current user's home directory from `HOME`, falling back to
/// `USERPROFILE` (Windows normally sets only the latter), if set and non-empty.
fn home_dir_str() -> Option<String> {
    ["HOME", "USERPROFILE"].into_iter().find_map(|var| {
        std::env::var_os(var)
            .filter(|home| !home.is_empty())
            .and_then(|home| home.into_string().ok())
    })
}

/// Collapse a leading home-directory prefix to `~` the way a shell does
/// (`/Users/me/proj` → `~/proj`, `/Users/me` → `~`). A no-op when `home` is
/// absent/empty or is not a path-boundary prefix of `path` (so `/Users/mentor`
/// is never mangled by a `/Users/me` home). Both `/` and `\` count as the
/// boundary so native Windows paths collapse too. Pure over `home` so it is
/// testable without touching the process environment.
fn collapse_home_prefix(path: &str, home: Option<&str>) -> String {
    let Some(home) = home
        .map(|home| home.trim_end_matches(['/', '\\']))
        .filter(|home| !home.is_empty())
    else {
        return path.to_string();
    };
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn short_path(path: &str) -> String {
    const MAX_PATH_LEN: usize = 28;
    let path = collapse_home_prefix(path, home_dir_str().as_deref());
    if path.chars().count() <= MAX_PATH_LEN {
        return path;
    }
    let suffix = path
        .chars()
        .rev()
        .take(MAX_PATH_LEN.saturating_sub(3))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{suffix}")
}

fn approval_modal_lines(approval: &ApprovalModalState, palette: Palette) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(approval.title.clone(), palette.title())),
        Line::from(vec![
            Span::styled(format!("{} ", t!("app.field.tool")), palette.muted()),
            Span::styled(approval.tool_name.clone(), palette.text()),
        ]),
    ];

    if let Some(kind) = approval.approval_kind.as_ref() {
        let risk = approval
            .risk
            .as_ref()
            .map(|risk| format!("  {} {risk}", t!("app.field.risk")))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", t!("app.field.kind")), palette.muted()),
            Span::styled(kind.clone(), palette.text()),
            Span::styled(risk, palette.muted()),
        ]));
    }

    lines.push(Line::from(""));

    if let Some(details) = approval.typed_details.as_ref() {
        match details.kind.as_str() {
            approval_kinds::COMMAND => {
                if let Some(command) = details.command.as_ref() {
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.command").into_owned(),
                        command.command_line.as_deref(),
                    );
                    push_optional_field(&mut lines, palette, "cwd", command.cwd.as_deref());
                    if !command.argv.is_empty() {
                        push_field(&mut lines, palette, "argv", command.argv.join(" "));
                    }
                    if !command.env_keys.is_empty() {
                        push_field(&mut lines, palette, "env", command.env_keys.join(", "));
                    }
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.tool_call").into_owned(),
                        command.tool_call_id.as_deref(),
                    );
                }
                if let Some(sandbox) = details.sandbox.as_ref() {
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.sandbox").into_owned(),
                        sandbox.mode.as_deref(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.filesystem").into_owned(),
                        sandbox.filesystem_access.as_deref(),
                    );
                    if let Some(network_access) = sandbox.network_access {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.network").into_owned(),
                            network_access.to_string(),
                        );
                    }
                    if !sandbox.writable_roots.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.writable").into_owned(),
                            sandbox.writable_roots.join(", "),
                        );
                    }
                }
            }
            approval_kinds::DIFF => {
                if let Some(diff) = details.diff.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.preview").into_owned(),
                        diff.preview_id.0.to_string(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.operation").into_owned(),
                        diff.operation.as_deref(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.summary").into_owned(),
                        diff.summary.as_deref(),
                    );
                    let stats = [
                        diff.file_count
                            .map(|value| t!("app.field.files_count", count = value).into_owned()),
                        diff.additions.map(|value| format!("+{value}")),
                        diff.deletions.map(|value| format!("-{value}")),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    if !stats.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.stats").into_owned(),
                            stats,
                        );
                    }
                }
            }
            approval_kinds::FILESYSTEM => {
                if let Some(filesystem) = details.filesystem.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.operation").into_owned(),
                        filesystem.operation.clone(),
                    );
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.outside_workspace").into_owned(),
                        filesystem.outside_workspace.to_string(),
                    );
                    for path in &filesystem.paths {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.path").into_owned(),
                            path.clone(),
                        );
                    }
                    if !filesystem.writable_roots.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.writable").into_owned(),
                            filesystem.writable_roots.join(", "),
                        );
                    }
                }
            }
            approval_kinds::NETWORK => {
                if let Some(network) = details.network.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.operation").into_owned(),
                        network.operation.clone(),
                    );
                    if !network.hosts.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.hosts").into_owned(),
                            network.hosts.join(", "),
                        );
                    }
                    if !network.ports.is_empty() {
                        let ports = network
                            .ports
                            .iter()
                            .map(|port| port.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.ports").into_owned(),
                            ports,
                        );
                    }
                    for url in &network.urls {
                        push_field(&mut lines, palette, "url", url.clone());
                    }
                }
            }
            approval_kinds::SANDBOX_ESCALATION => {
                if let Some(escalation) = details.sandbox_escalation.as_ref() {
                    if let Some(from) = escalation.from.as_ref() {
                        push_optional_field(
                            &mut lines,
                            palette,
                            t!("app.field.from").into_owned(),
                            from.mode.as_deref(),
                        );
                    }
                    if let Some(to) = escalation.to.as_ref() {
                        push_optional_field(
                            &mut lines,
                            palette,
                            t!("app.field.to").into_owned(),
                            to.mode.as_deref(),
                        );
                    }
                    if !escalation.requested_permissions.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.permissions").into_owned(),
                            escalation.requested_permissions.join(", "),
                        );
                    }
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.justification").into_owned(),
                        escalation.justification.as_deref(),
                    );
                    if !escalation.suggested_prefix_rule.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            "prefix",
                            escalation.suggested_prefix_rule.join(" "),
                        );
                    }
                }
            }
            _ => {}
        }

        lines.push(Line::from(""));
    }

    lines.extend(
        approval
            .body
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), palette.text()))),
    );
    lines
}

fn push_optional_field(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    label: impl Into<String>,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        push_field(lines, palette, label, value.to_string());
    }
}

fn push_field(
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

fn render_task_output_modal(
    frame: &mut impl FrameLike,
    output: &TaskOutputDetailState,
    palette: Palette,
) {
    let area = centered_rect(82, 68, frame.area());
    let cursor = output
        .cursor
        .map(|cursor| format!(" @{}", cursor.offset))
        .unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::styled(output.title.clone(), palette.title()),
        Span::styled(cursor, palette.muted()),
    ])];
    lines.push(Line::from(""));

    if output.output.is_empty() {
        lines.push(Line::from(Span::styled(
            t!("app.empty.no_task_output").to_string(),
            palette.muted(),
        )));
    } else {
        lines.extend(
            output
                .output
                .lines()
                .map(|line| Line::from(Span::styled(line.to_string(), palette.text()))),
        );
    }

    let visible_height = usize::from(area.height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_from_bottom = output.scroll.min(max_scroll);
    let scroll_top =
        u16::try_from(max_scroll.saturating_sub(scroll_from_bottom)).unwrap_or(u16::MAX);

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.task_output").to_string(),
                palette,
                true,
                Some(t!("app.hint.task_output_modal").into_owned()),
            )
            .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn render_artifact_detail_modal(
    frame: &mut impl FrameLike,
    artifact: &ArtifactDetailState,
    palette: Palette,
) {
    let area = centered_rect(82, 68, frame.area());
    let mut lines = vec![
        Line::from(Span::styled(artifact.title.clone(), palette.title())),
        Line::from(Span::styled(artifact.subtitle.clone(), palette.muted())),
        Line::from(""),
    ];

    lines.extend(
        artifact
            .content
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), palette.text()))),
    );

    let visible_height = usize::from(area.height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_from_bottom = artifact.scroll.min(max_scroll);
    let scroll_top =
        u16::try_from(max_scroll.saturating_sub(scroll_from_bottom)).unwrap_or(u16::MAX);

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.artifact_modal").to_string(),
                palette,
                true,
                Some(t!("app.hint.scroll_modal").into_owned()),
            )
            .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn render_thread_graph_detail_modal(
    frame: &mut impl FrameLike,
    graph: &ThreadGraphDetailState,
    palette: Palette,
) {
    let area = centered_rect(82, 68, frame.area());
    let mut lines = vec![
        Line::from(Span::styled(graph.title.clone(), palette.title())),
        Line::from(Span::styled(graph.subtitle.clone(), palette.muted())),
        Line::from(""),
    ];

    lines.extend(
        graph
            .content
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), palette.text()))),
    );

    let visible_height = usize::from(area.height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_from_bottom = graph.scroll.min(max_scroll);
    let scroll_top =
        u16::try_from(max_scroll.saturating_sub(scroll_from_bottom)).unwrap_or(u16::MAX);

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.threads").to_string(),
                palette,
                true,
                Some(t!("app.hint.scroll_modal").into_owned()),
            )
            .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn render_turn_state_detail_modal(
    frame: &mut impl FrameLike,
    turn: &TurnStateDetailState,
    palette: Palette,
) {
    let area = centered_rect(82, 68, frame.area());
    let mut lines = vec![
        Line::from(Span::styled(turn.title.clone(), palette.title())),
        Line::from(Span::styled(turn.subtitle.clone(), palette.muted())),
        Line::from(""),
    ];

    lines.extend(
        turn.content
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), palette.text()))),
    );

    let visible_height = usize::from(area.height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_from_bottom = turn.scroll.min(max_scroll);
    let scroll_top =
        u16::try_from(max_scroll.saturating_sub(scroll_from_bottom)).unwrap_or(u16::MAX);

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                t!("app.pane.turn").to_string(),
                palette,
                true,
                Some(t!("app.hint.scroll_modal").into_owned()),
            )
            .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn diff_line_sign(kind: &str) -> &'static str {
    match kind {
        "added" => "+",
        "removed" => "-",
        "context" => " ",
        _ => "?",
    }
}

fn diff_line_style(kind: &str, palette: Palette) -> Style {
    match kind {
        "added" => Style::default().fg(palette.success).bg(palette.success_bg),
        "removed" => Style::default().fg(palette.danger).bg(palette.danger_bg),
        "context" => palette.text().bg(palette.diff_context_bg),
        _ => palette.text().bg(palette.surface_alt),
    }
}

fn diff_line_marker_style(kind: &str, palette: Palette) -> Style {
    diff_line_style(kind, palette).add_modifier(Modifier::BOLD)
}

fn diff_line_gutter_style(kind: &str, palette: Palette) -> Style {
    match kind {
        "added" => Style::default().fg(palette.success).bg(palette.success_bg),
        "removed" => Style::default().fg(palette.danger).bg(palette.danger_bg),
        "context" => palette.muted().bg(palette.diff_context_bg),
        _ => palette.muted().bg(palette.surface_alt),
    }
}

fn diff_file_status_style(status: &str, palette: Palette) -> Style {
    match status {
        "added" | "created" => Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD),
        "deleted" | "removed" => Style::default()
            .fg(palette.danger)
            .add_modifier(Modifier::BOLD),
        _ => palette.selected().add_modifier(Modifier::BOLD),
    }
}

fn diff_hunk_style(palette: Palette) -> Style {
    Style::default()
        .fg(palette.accent)
        .bg(palette.diff_context_bg)
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
fn inline_diff_style_for_test(kind: &str, palette: Palette) -> Style {
    diff_line_style(kind, palette)
}

#[cfg(test)]
fn inline_diff_marker_style_for_test(kind: &str, palette: Palette) -> Style {
    diff_line_marker_style(kind, palette)
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical_margin = (100 - percent_y) / 2;
    let horizontal_margin = (100 - percent_x) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(vertical_margin),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(vertical_margin),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(horizontal_margin),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(horizontal_margin),
        ])
        .split(vertical[1])[1]
}

fn titled_block<'a>(
    title: impl Into<String>,
    palette: Palette,
    focused: bool,
    suffix: Option<String>,
) -> Block<'a> {
    let mut spans = vec![Span::styled(title.into(), palette.title())];
    if let Some(suffix) = suffix {
        spans.push(Span::styled(format!("  {suffix}"), palette.muted()));
    }
    if focused {
        spans.push(Span::styled("  ●", palette.selected()));
    }

    Block::default()
        .borders(Borders::ALL)
        .title(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::ThemeName,
        model::{
            ApprovalModalState, DiffPreview, DiffPreviewFile, DiffPreviewGetResult,
            DiffPreviewHunk, DiffPreviewLine, ModelStatus, SessionModelCatalog,
            SessionRuntimeStatus, SessionView, TurnActivitySummary, TurnPromptAnchor,
        },
        store::Store,
        viewport::ScrollbackTracker,
    };
    use octos_core::{
        Message, SessionKey,
        ui_protocol::{
            ApprovalId, PreviewId, QuestionId, TaskRuntimeState, TurnId, UiProtocolCapabilities,
        },
    };
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
        buffer::Buffer,
        layout::Position,
    };

    fn rendered_buffer(app: &AppState, palette: Palette) -> Buffer {
        rendered_buffer_with_size(app, palette, 120, 42)
    }

    fn rendered_buffer_with_size(
        app: &AppState,
        palette: Palette,
        width: u16,
        height: u16,
    ) -> Buffer {
        rendered_buffer_and_cursor_with_size(app, palette, width, height).0
    }

    fn rendered_buffer_and_cursor(app: &AppState, palette: Palette) -> (Buffer, Position) {
        rendered_buffer_and_cursor_with_size(app, palette, 120, 42)
    }

    fn rendered_buffer_and_cursor_with_size(
        app: &AppState,
        palette: Palette,
        width: u16,
        height: u16,
    ) -> (Buffer, Position) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, app, palette))
            .expect("render succeeds");
        let cursor = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("cursor position");
        (terminal.backend().buffer().clone(), cursor)
    }

    fn rendered_text(app: &AppState) -> String {
        rendered_buffer(app, Palette::for_theme(ThemeName::Slate))
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn rendered_rows(buffer: &Buffer) -> Vec<String> {
        let width = usize::from(buffer.area.width);
        let height = usize::from(buffer.area.height);
        (0..height)
            .map(|y| {
                let row_start = y * width;
                buffer.content[row_start..row_start + width]
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn row_containing<'a>(rows: &'a [String], needle: &str) -> &'a str {
        rows.iter()
            .find(|row| row.contains(needle))
            .map(String::as_str)
            .unwrap_or_else(|| panic!("row containing {needle:?}"))
    }

    /// Test-only [`SessionRuntimeStatus`] carrying just the fields the composer
    /// footer reads (model + workspace root); everything else stays empty.
    fn runtime_status_with_model_cwd(
        session_id: SessionKey,
        model: &str,
        cwd: &str,
    ) -> SessionRuntimeStatus {
        SessionRuntimeStatus {
            session_id,
            runtime_mode: None,
            profile_id: None,
            cwd: Some(cwd.into()),
            workspace_root: Some(cwd.into()),
            active_turn_id: None,
            runtime_policy_stamp: None,
            model: Some(ModelStatus {
                model: model.into(),
                provider: "test".into(),
                title: None,
                family: None,
                route: None,
                selected: true,
                available: Some(true),
                queue_mode: None,
                qoe_policy: None,
            }),
            permission_profile: None,
            approval_policy: None,
            sandbox_mode: None,
            sandbox: None,
            filesystem_scope: None,
            network: None,
            tool_policy_id: None,
            mcp_servers: Vec::new(),
            memory_scope: None,
            health: None,
            mcp_summary: None,
            tool_summary: None,
            usage: None,
            cursor: None,
        }
    }

    #[test]
    fn render_composer_shows_current_model_and_cwd_on_bottom_border() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // A non-home absolute path renders verbatim regardless of the test
        // runner's HOME (home-dir collapsing is covered separately).
        app.workspace.root = "/srv/octos-workspace".into();
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id,
            "claude-fable-5",
            "/srv/octos-workspace",
        ));

        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let rows = rendered_rows(&buffer);

        // The current model is surfaced ONLY on the composer footer (the status
        // bar never shows it), so the row carrying it is the composer's bottom
        // border — and that same border row must also carry the cwd.
        let footer = row_containing(&rows, "claude-fable-5");
        assert!(
            footer.contains("/srv/octos-workspace"),
            "composer bottom border should show the cwd next to the model; got {footer:?}"
        );
    }

    fn app_with_reasoning_message(reasoning: &str) -> (AppState, SessionKey) {
        let session_id = SessionKey("local:rsn".into());
        let mut assistant = Message::assistant("the answer is 4");
        assistant.reasoning_content = Some(reasoning.to_string());
        let app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "t".into(),
                profile_id: None,
                messages: vec![Message::user("q"), assistant],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        (app, session_id)
    }

    fn history_text(app: &AppState) -> String {
        finalized_history_lines(app, Palette::for_theme(ThemeName::Codex), 200)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn reasoning_block_hidden_by_default_shown_when_toggled_on() {
        let (mut app, session_id) = app_with_reasoning_message("step one\nstep two");
        // OFF (default): the reasoning text must not appear in scrollback.
        assert!(
            !history_text(&app).contains("reasoning"),
            "reasoning block must be hidden by default"
        );
        // ON: the block renders.
        app.session_reasoning_display.insert(session_id);
        let text = history_text(&app);
        assert!(
            text.contains("· reasoning"),
            "toggle on renders the block: {text}"
        );
        assert!(text.contains("step one") && text.contains("step two"));
    }

    #[test]
    fn reasoning_block_caps_lines_until_expanded() {
        let long: String = (1..=12)
            .map(|n| format!("thought line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (mut app, session_id) = app_with_reasoning_message(&long);
        app.session_reasoning_display.insert(session_id);

        let capped = history_text(&app);
        assert!(
            capped.contains("thought line 6"),
            "cap shows the first 6 lines"
        );
        assert!(
            !capped.contains("thought line 7"),
            "beyond the cap is hidden until expanded"
        );
        assert!(capped.contains("more line(s) (Ctrl+O expand)"));

        app.expanded_tool_outputs = true;
        let expanded = history_text(&app);
        assert!(
            expanded.contains("thought line 12"),
            "Ctrl+O expand shows the full reasoning"
        );
    }

    #[test]
    fn toggling_reasoning_display_does_not_reflush_committed_scrollback() {
        // A terminal can't retroactively redraw scrolled-off history, so the
        // toggle must NOT flip the committed fingerprint — otherwise the
        // scrollback tracker's discontinuity branch would re-flush the whole
        // history and duplicate it below the stale copy. The toggle applies to
        // turns committed afterwards; past turns use the Tab inspector.
        let (mut app, session_id) = app_with_reasoning_message("some reasoning");
        let off = committed_messages_fingerprint(&app);
        app.session_reasoning_display.insert(session_id);
        let on = committed_messages_fingerprint(&app);
        assert_eq!(
            off.content_hash, on.content_hash,
            "the display toggle must not force a committed-history re-flush"
        );
    }

    #[test]
    fn in_progress_status_marker_is_the_octopus_spinner() {
        // The pinned "still working" signal: the in-progress status marker is
        // one of the octopus spinner frames (not a static bullet), so it stays
        // visible in the status bar even when the transcript chip scrolls off.
        let marker = run_state_marker(&SessionRunState::InProgress);
        assert!(
            SPINNER_FRAMES.contains(&marker),
            "in-progress marker must be an octopus spinner frame, got {marker:?}"
        );
        // Settled states keep their static, non-animated markers.
        assert_eq!(run_state_marker(&SessionRunState::Success), "✓");
        assert_eq!(run_state_marker(&SessionRunState::Idle), "·");
    }

    #[test]
    fn swimming_octopus_frames_have_boxed_eyes_four_arms_and_flip_direction() {
        // Each frame: a `[⇔]` head with one tilted-line arm glyph per side (彡/ミ).
        for frame in OCTOPUS_SWIM_FRAMES {
            assert!(frame.contains("[⇔]"), "[⇔] head: {frame}");
            let (left, right) = frame.split_once("[⇔]").expect("head splits arms");
            assert_eq!(left.chars().count(), 1, "one arm glyph left: {frame}");
            assert_eq!(right.chars().count(), 1, "one arm glyph right: {frame}");
        }
        // The two frames face opposite ways — now the travel *direction*:
        // 彡[⇔]ミ swims right, ミ[⇔]彡 swims left.
        assert_eq!(OCTOPUS_SWIM_FRAMES[0], "彡[⇔]ミ");
        assert_eq!(OCTOPUS_SWIM_FRAMES[1], "ミ[⇔]彡");
    }

    #[test]
    fn octopus_swim_starts_at_origin_with_the_first_stroke() {
        // elapsed=0 → sitting at the left margin, first paddle stroke.
        let (offset, frame) = octopus_swim(0, 80);
        assert_eq!(offset, 0, "starts flush-left");
        assert_eq!(frame, "彡[⇔]ミ");
        assert_eq!(frame, OCTOPUS_SWIM_FRAMES[0]);
    }

    #[test]
    fn octopus_swim_reaches_the_far_edge_in_one_sweep_on_any_width() {
        // The sweep is time-proportional: at exactly OCTOPUS_SWEEP_ONE_WAY_MS
        // the octopus touches the far edge — on ANY terminal width. (The old
        // fixed ms-per-column pace took ~21s one-way on a 146-column pane, so
        // real thinking phases ended with the octopus around mid-screen.)
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        for wrap_width in [20usize, 40, 80, 146, 200, 1000] {
            let max = wrap_width.saturating_sub(octopus_width + 1);
            let (at_peak, _) = octopus_swim(OCTOPUS_SWEEP_ONE_WAY_MS, wrap_width);
            assert_eq!(
                at_peak, max,
                "far edge reached after one sweep at wrap_width={wrap_width}"
            );
            // …and it comes all the way back by the end of the cycle.
            let (back, _) = octopus_swim(2 * OCTOPUS_SWEEP_ONE_WAY_MS, wrap_width);
            assert_eq!(back, 0, "returns to origin at wrap_width={wrap_width}");
        }
    }

    #[test]
    fn octopus_swim_traces_a_symmetric_triangle_while_paddling() {
        // Sampled through one full cycle: offset rises monotonically to MAX,
        // falls monotonically back, is mirror-symmetric around the peak, and
        // the paddle stroke alternates every OCTOPUS_STROKE_MS throughout.
        let wrap_width = 120usize;
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        let max = wrap_width.saturating_sub(octopus_width + 1);
        assert!(
            max > 28,
            "the sweep must exceed the old 28-column cap (got MAX={max})"
        );

        let cycle_ms = 2 * OCTOPUS_SWEEP_ONE_WAY_MS;
        let mut previous = None;
        for t in (0..=cycle_ms).step_by(50) {
            let (offset, frame) = octopus_swim(t, wrap_width);
            assert!(offset <= max, "offset {offset} exceeded MAX {max} at {t}ms");
            // Mirror symmetry: same distance from either end of the cycle.
            let (mirrored, _) = octopus_swim(cycle_ms - t, wrap_width);
            assert_eq!(offset, mirrored, "triangle asymmetric at {t}ms");
            // Monotone rise then fall.
            if let Some((prev_t, prev_offset)) = previous {
                if t <= OCTOPUS_SWEEP_ONE_WAY_MS {
                    assert!(
                        offset >= prev_offset,
                        "rising leg regressed between {prev_t}ms and {t}ms"
                    );
                } else if prev_t >= OCTOPUS_SWEEP_ONE_WAY_MS {
                    assert!(
                        offset <= prev_offset,
                        "falling leg climbed between {prev_t}ms and {t}ms"
                    );
                }
            }
            // Stroke follows the global clock, independent of position.
            assert_eq!(
                frame,
                OCTOPUS_SWIM_FRAMES[((t / OCTOPUS_STROKE_MS) % 2) as usize],
                "paddle stroke at {t}ms"
            );
            previous = Some((t, offset));
        }
    }

    #[test]
    fn octopus_swim_never_overflows_the_wrap_width() {
        // The octopus (plus a one-column right margin) always stays inside
        // the wrap boundary across full cycles, for a range of widths — and
        // reaches the far edge on every one of them (full-width travel).
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        for wrap_width in [octopus_width + 2, 20, 40, 80, 200, 1000] {
            let max = wrap_width.saturating_sub(octopus_width + 1);
            let mut peak = 0usize;
            for t in (0..2 * OCTOPUS_SWEEP_ONE_WAY_MS).step_by(25) {
                let (offset, _frame) = octopus_swim(t, wrap_width);
                assert!(
                    offset + octopus_width <= wrap_width,
                    "octopus overflowed wrap_width={wrap_width}: offset={offset}",
                );
                peak = peak.max(offset);
            }
            // step_by sampling always hits the exact SWEEP_MS peak instant
            // because 25 divides it; assert it rather than assuming.
            let (at_peak, _) = octopus_swim(OCTOPUS_SWEEP_ONE_WAY_MS, wrap_width);
            assert_eq!(
                peak.max(at_peak),
                max,
                "far edge at wrap_width={wrap_width}"
            );
        }
    }

    #[test]
    fn octopus_swim_tiny_terminal_paddles_in_place_without_panicking() {
        // A terminal too narrow to travel: MAX collapses to 0, so the octopus
        // holds the left margin — still paddling — instead of panicking or
        // wrapping.
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        for wrap_width in [0usize, 1, 2, octopus_width, octopus_width + 1] {
            // A large elapsed value also exercises the u128 math safely.
            let big = 9_999_999_999u128;
            let (offset, frame) = octopus_swim(big, wrap_width);
            assert_eq!(offset, 0, "no travel at wrap_width={wrap_width}");
            assert_eq!(
                frame,
                OCTOPUS_SWIM_FRAMES[((big / OCTOPUS_STROKE_MS) % 2) as usize],
                "keeps paddling in place"
            );
            // The next stroke interval still alternates while parked.
            let (_, next) = octopus_swim(big + OCTOPUS_STROKE_MS, wrap_width);
            assert_ne!(frame, next, "parked octopus must keep alternating strokes");
        }
    }

    #[test]
    fn render_composer_collapses_home_dir_in_cwd_footer() {
        // Build the cwd from the runner's actual HOME so the assertion is
        // deterministic across machines and exercises the real render path.
        let Some(home) = std::env::var_os("HOME")
            .and_then(|home| home.into_string().ok())
            .map(|home| home.trim_end_matches('/').to_string())
            .filter(|home| !home.is_empty())
        else {
            return; // no HOME → collapsing is a documented no-op, nothing to assert
        };
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let cwd = format!("{home}/proj/octos");
        app.workspace.root = cwd.clone();
        app.set_runtime_status(runtime_status_with_model_cwd(session_id, "kimi-k2", &cwd));

        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let rows = rendered_rows(&buffer);
        let footer = row_containing(&rows, "kimi-k2");

        assert!(
            footer.contains("~/proj/octos"),
            "composer cwd should collapse the home dir to ~; got {footer:?}"
        );
        assert!(
            !footer.contains(&home),
            "raw home dir must not leak once collapsed to ~; got {footer:?}"
        );
    }

    #[test]
    fn render_composer_shows_selected_catalog_model_without_runtime_status() {
        // When no session/status/read has landed yet (no runtime status), the
        // footer still shows the model the `/model` catalog marks selected.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/octos-workspace".into();
        app.set_model_catalog(SessionModelCatalog {
            session_id,
            models: vec![
                ModelStatus {
                    model: "deepseek-v4-pro".into(),
                    provider: "deepseek".into(),
                    title: None,
                    family: None,
                    route: None,
                    selected: true,
                    available: Some(true),
                    queue_mode: None,
                    qoe_policy: None,
                },
                ModelStatus {
                    model: "gpt-5".into(),
                    provider: "openai".into(),
                    title: None,
                    family: None,
                    route: None,
                    selected: false,
                    available: Some(true),
                    queue_mode: None,
                    qoe_policy: None,
                },
            ],
        });
        assert!(
            app.runtime_status_for(&SessionKey("local:test".into()))
                .is_none()
        );

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let footer = row_containing(&rows, "/srv/octos-workspace");
        assert!(
            footer.contains("deepseek-v4-pro"),
            "footer should fall back to the catalog's selected model; got {footer:?}"
        );
        assert!(
            !footer.contains("gpt-5"),
            "only the selected catalog model belongs on the footer; got {footer:?}"
        );
    }

    #[test]
    fn status_bar_does_not_duplicate_the_composer_footer_cwd() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/octos-workspace".into();

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));

        // The status bar (below the composer) must NOT repeat the cwd — it now
        // lives on the composer's bottom border, and repeating it one line below
        // read as clutter.
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            !status_row.contains("/srv/octos-workspace"),
            "status bar should not duplicate the cwd now on the composer border; got {status_row:?}"
        );
        // ...but the cwd is still shown once (on the composer border).
        assert!(
            rows.iter().any(|row| row.contains("/srv/octos-workspace")),
            "cwd should still appear once, on the composer border"
        );
    }

    #[test]
    fn unflushed_activity_section_still_emits_turn_summary() {
        // Regression: an orchestrated turn's activity log is still covered by the
        // live tail at the settling flush, so it routes through the UNFLUSHED
        // section renderer with its items already flushed (empty here). The
        // committed status report must still land in scrollback.
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do it"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.attach_turn_summary(&session_id, &turn_id, 75, 1);
        let log = crate::model::TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            request: Some("do it".into()),
            anchor_index: None,
            items: vec![],
        };

        let palette = Palette::for_theme(ThemeName::Codex);
        let mut lines = Vec::new();
        let coverage = LiveTurnFinalization::new(&session_id, &turn_id);
        push_turn_activity_log_section_unflushed(&mut lines, palette, &log, &app, &coverage, 80);

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(
            text.contains("✻ Ran for 1m 15s · 1 background task(s) still running"),
            "unflushed section must still emit the turn summary; got: {text:?}"
        );
    }

    #[test]
    fn turn_summary_text_formats_duration_and_running_tasks() {
        let with_tasks = TurnActivitySummary {
            session_id: SessionKey("local:test".into()),
            turn_id: TurnId::new(),
            elapsed_secs: 319,
            background_tasks: 2,
        };
        assert_eq!(
            turn_summary_text(&with_tasks),
            "✻ Ran for 5m 19s · 2 background task(s) still running"
        );

        let no_tasks = TurnActivitySummary {
            session_id: SessionKey("local:test".into()),
            turn_id: TurnId::new(),
            elapsed_secs: 8,
            background_tasks: 0,
        };
        assert_eq!(turn_summary_text(&no_tasks), "✻ Ran for 8s");
    }

    #[test]
    fn transcript_renders_turn_summary_line_after_completed_turn() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do the thing"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // A completed turn with one background task still running. No activity
        // log items — attach_turn_summary must synthesize a log so the report
        // still renders after the assistant reply.
        app.attach_turn_summary(&session_id, &turn_id, 75, 1);

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let text = rows.join("\n");
        assert!(
            text.contains("✻ Ran for 1m 15s · 1 background task(s) still running"),
            "transcript should carry the committed turn status report; got:\n{text}"
        );
    }

    #[test]
    fn settled_session_keeps_rendering_btw_aside() {
        // Regression (live soak): the live tail gates on should_show_turn_flow,
        // which went false once the session settled — the aside card vanished
        // the moment the main turn completed, often before the answer landed.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(&session_id, "still with me?".into());
        // The aside now renders as a floating overlay, no longer gated on the
        // turn flow — so it stays visible even after the session settles.
        let palette = Palette::for_theme(ThemeName::Codex);
        let text = rendered_rows(&rendered_buffer(&app, palette)).join("\n");
        assert!(
            text.contains("/btw still with me?"),
            "settled session must still render the aside overlay; got:\n{text}"
        );
        // The pane chrome is load-bearing: without the titled border the
        // overlay reads as embedded transcript text, not its own window.
        assert!(
            text.contains("Aside — /btw"),
            "aside must render as a titled pane, not bare lines; got:\n{text}"
        );
        assert!(
            text.contains("┌") && text.contains("└"),
            "aside pane must draw its border; got:\n{text}"
        );
    }

    /// codex P1 (merge reconcile): the aside no longer contributes lines to
    /// the turn flow, so a SETTLED session's live tail collapses to 1-2 rows —
    /// under `render_btw_overlay`'s 3-row minimum — and the overlay became
    /// invisible in the real inline viewport (state kept it, nothing drew it).
    /// The tail height hint must reserve the overlay's rows.
    #[test]
    fn btw_aside_overlay_survives_settled_inline_viewport() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(&session_id, "still with me?".into());
        // Real inline-viewport path: the viewport is sized by live_ui_height
        // (a settled tail otherwise reserves ~1 row) and the overlay draws
        // over the tail's top rows.
        let text = viewport_rows(&app, 100, 40).join("\n");
        assert!(
            text.contains("Aside — /btw"),
            "settled inline viewport must still draw the aside pane; got:\n{text}"
        );
        assert!(
            text.contains("/btw still with me?"),
            "aside question echo missing from inline viewport; got:\n{text}"
        );
    }

    #[test]
    fn btw_aside_card_renders_answering_then_answer() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do the thing"), Message::assistant("on it")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(&session_id, "what are you working on?".into());

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let text = rows.join("\n");
        assert!(
            text.contains("/btw what are you working on?"),
            "aside question echo missing:\n{text}"
        );
        assert!(
            text.contains("✽ Answering…"),
            "answering indicator missing:\n{text}"
        );

        assert!(
            app.resolve_btw_answer(&session_id, "Refactoring the parser.".into()),
            "answer resolves the answering aside"
        );
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let text = rows.join("\n");
        assert!(
            text.contains("Refactoring the parser."),
            "answer block missing:\n{text}"
        );
        assert!(
            !text.contains("✽ Answering…"),
            "answering indicator must clear once answered:\n{text}"
        );
    }

    #[test]
    fn collapse_home_prefix_replaces_home_with_tilde() {
        assert_eq!(
            collapse_home_prefix("/Users/me/proj/octos", Some("/Users/me")),
            "~/proj/octos"
        );
        // Exact home collapses to a bare ~.
        assert_eq!(collapse_home_prefix("/Users/me", Some("/Users/me")), "~");
        // A trailing slash on HOME is tolerated.
        assert_eq!(
            collapse_home_prefix("/Users/me/x", Some("/Users/me/")),
            "~/x"
        );
    }

    #[test]
    fn collapse_home_prefix_only_matches_on_path_boundary() {
        // `/Users/mentor` shares a textual prefix with `/Users/me` but is NOT a
        // subdirectory — it must be left untouched.
        assert_eq!(
            collapse_home_prefix("/Users/mentor/x", Some("/Users/me")),
            "/Users/mentor/x"
        );
        // Absent/empty HOME is a no-op.
        assert_eq!(collapse_home_prefix("/Users/me/x", None), "/Users/me/x");
        assert_eq!(collapse_home_prefix("/Users/me/x", Some("")), "/Users/me/x");
    }

    #[test]
    fn collapse_home_prefix_handles_windows_separators() {
        // Native Windows paths use `\` as the boundary (USERPROFILE homes).
        assert_eq!(
            collapse_home_prefix(r"C:\Users\me\proj", Some(r"C:\Users\me")),
            r"~\proj"
        );
        assert_eq!(
            collapse_home_prefix(r"C:\Users\me", Some(r"C:\Users\me")),
            "~"
        );
        // Trailing backslash on the home is tolerated; boundary still enforced.
        assert_eq!(
            collapse_home_prefix(r"C:\Users\me\x", Some(r"C:\Users\me\")),
            r"~\x"
        );
        assert_eq!(
            collapse_home_prefix(r"C:\Users\mentor\x", Some(r"C:\Users\me")),
            r"C:\Users\mentor\x"
        );
    }

    #[test]
    fn composer_footer_prefers_session_workspace_root_over_global() {
        // A canonicalized/global `workspace.root` must not shadow the ACTIVE
        // session's server-confirmed workspace (from session/status/read) —
        // switching between sessions with different workspaces shows each
        // session's own root.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/global-root".into();
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id,
            "kimi-k2",
            "/srv/session-root",
        ));

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let footer = row_containing(&rows, "kimi-k2");
        assert!(
            footer.contains("/srv/session-root"),
            "footer should show the session's server-confirmed workspace root; got {footer:?}"
        );
        assert!(
            !footer.contains("/srv/global-root"),
            "the global workspace root must not shadow the session's; got {footer:?}"
        );
    }

    #[test]
    fn composer_footer_keeps_model_and_drops_cwd_when_too_narrow_for_both_titles() {
        // Ratatui paints overlapping border titles over each other; when the
        // composer cannot fit cwd + model side by side, the cwd is dropped and
        // the MODEL is kept — never a collision. The model is the footer's sole
        // persistent home now that the status line no longer echoes it, so it
        // must never be the title that vanishes.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/quite/long/workspace/path/here".into();
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id,
            "moonshotai-kimi-k2-instruct",
            "/srv/quite/long/workspace/path/here",
        ));

        let palette = Palette::for_theme(ThemeName::Codex);
        let narrow = rendered_rows(&rendered_buffer_with_size(&app, palette, 40, 30));
        assert!(
            narrow.iter().any(|row| row.contains("kimi")),
            "model must be kept when both footer titles cannot fit; got {narrow:?}"
        );
        assert!(
            !narrow.iter().any(|row| row.contains("workspace/path")),
            "cwd must be dropped when both footer titles cannot fit; got {narrow:?}"
        );
        let wide = rendered_rows(&rendered_buffer_with_size(&app, palette, 120, 30));
        assert!(
            wide.iter().any(|row| row.contains("kimi")),
            "model still renders when the composer is wide enough"
        );
        assert!(
            wide.iter().any(|row| row.contains("workspace/path")),
            "cwd renders alongside the model once the composer is wide enough"
        );
    }

    fn row_index_containing(rows: &[String], needle: &str) -> usize {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row containing {needle:?}"))
    }

    fn style_for_text(buffer: &Buffer, needle: &str) -> Option<Style> {
        let width = usize::from(buffer.area.width);
        let height = usize::from(buffer.area.height);
        for y in 0..height {
            let row_start = y * width;
            let row = buffer.content[row_start..row_start + width]
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            if let Some(x) = row.find(needle) {
                let cell = &buffer.content[row_start + x];
                return Some(
                    Style::default()
                        .fg(cell.fg)
                        .bg(cell.bg)
                        .add_modifier(cell.modifier),
                );
            }
        }
        None
    }

    fn app_with_diff(result: DiffPreviewGetResult) -> AppState {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "diff".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.diff_preview.apply_result(result);
        app
    }

    #[test]
    fn gradient_sample_lerps_endpoints_and_midpoint() {
        let stops = [(0u8, 0u8, 0u8), (100u8, 200u8, 40u8)];
        assert_eq!(gradient_sample(&stops, 0.0), (0, 0, 0));
        assert_eq!(gradient_sample(&stops, 1.0), (100, 200, 40));
        assert_eq!(gradient_sample(&stops, 0.5), (50, 100, 20));
        // Out-of-range clamps; degenerate stop lists don't panic.
        assert_eq!(gradient_sample(&stops, 2.0), (100, 200, 40));
        assert_eq!(gradient_sample(&[(7, 7, 7)], 0.5), (7, 7, 7));
    }

    #[test]
    fn wave_gradient_spans_colors_each_grapheme_and_animates() {
        let stops = [(0u8, 0u8, 0u8), (255u8, 255u8, 255u8)];
        let a = wave_gradient_spans("abc", 0.0, &stops, Color::Reset);
        assert_eq!(a.len(), 3, "one span per grapheme");
        assert!(
            a.iter()
                .all(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _)))),
            "every glyph gets a truecolor fg"
        );
        // Advancing the phase moves the crest → the first glyph recolors.
        let b = wave_gradient_spans("abc", 1.5, &stops, Color::Reset);
        assert_ne!(a[0].style.fg, b[0].style.fg, "the wave advances with phase");
        // CJK double-width graphemes still produce one span each.
        assert_eq!(
            wave_gradient_spans("水波", 0.0, &stops, Color::Reset).len(),
            2
        );
    }

    #[test]
    fn render_default_view_is_coding_session_first() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "artifact task".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: "artifact log line\n".into(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "Mock backend ready".into(),
            Some("local mock snapshot".into()),
            false,
        );

        let text = rendered_text(&app);

        assert!(!text.contains("Octos TUI"));
        assert!(!text.contains("Protocol session"));
        assert!(!text.contains("ws://"));
        assert!(!text.contains("Transcript"));
        assert!(text.contains("Composer"));
        assert!(text.contains("Tab inspector"));
        assert!(!text.contains("Current Tasks"));
        assert!(!text.contains("tasks/status"));
        assert!(!text.contains("Sessions"));
        assert!(!text.contains("Artifacts"));
        assert!(!text.contains("Workspace"));
        assert!(!text.contains("Git"));
        assert!(!text.contains("INFO calling LLM"));
        assert!(!text.contains("parallel_tools"));
        assert!(!text.contains("tool_ids="));
    }

    #[test]
    fn render_artifact_detail_modal_shows_content() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.artifact_detail = crate::model::ArtifactDetailState {
            active: true,
            title: "notes.md".into(),
            subtitle: "agent ag-7 | markdown | ready".into(),
            content: "artifact body".into(),
            scroll: 0,
        };

        let text = rendered_text(&app);

        assert!(text.contains("Artifact"));
        assert!(text.contains("notes.md"));
        assert!(text.contains("agent ag-7"));
        assert!(text.contains("artifact body"));
    }

    #[test]
    fn render_thread_graph_detail_modal_shows_threads() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.thread_graph_detail = crate::model::ThreadGraphDetailState {
            active: true,
            title: "Thread Graph".into(),
            subtitle: "1 thread(s) @ session:7".into(),
            content: "thread-1 | active | root seq 1 | 2 message(s)".into(),
            scroll: 0,
        };

        let text = rendered_text(&app);

        assert!(text.contains("Threads"));
        assert!(text.contains("Thread Graph"));
        assert!(text.contains("thread-1"));
        assert!(text.contains("root seq 1"));
    }

    #[test]
    fn render_turn_state_detail_modal_shows_lifecycle() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_state_detail = crate::model::TurnStateDetailState {
            active: true,
            title: "Turn State".into(),
            subtitle: "turn 00000000-0000-0000-0000-000000000011".into(),
            content: "state: active\nthread: thread-1\ncommitted seqs: 1, 2".into(),
            scroll: 0,
        };

        let text = rendered_text(&app);

        assert!(text.contains("Turn"));
        assert!(text.contains("Turn State"));
        assert!(text.contains("state: active"));
        assert!(text.contains("thread-1"));
        assert!(text.contains("committed seqs"));
    }

    #[test]
    fn render_inspector_view_includes_m9_panes_without_hiding_chat() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "artifact task".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: "artifact log line\n".into(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "Mock backend ready".into(),
            Some("local mock snapshot".into()),
            false,
        );
        app.focus = FocusPane::Sessions;

        let text = rendered_text(&app);

        assert!(text.contains("Sessions"));
        assert!(text.contains("Tasks"));
        assert!(text.contains("Composer"));
        assert!(text.contains("Artifacts"));
        assert!(text.contains("Workspace"));
        assert!(text.contains("Git"));
        assert!(text.contains("artifact task output tail"));
        assert!(text.contains("m9.7/mock-snapshot"));
        assert!(text.contains("api octos-app-ui/v1alpha1"));
        assert!(!text.contains("INFO calling LLM"));
        assert!(!text.contains("parallel_tools"));
        assert!(!text.contains("tool_ids="));
    }

    #[test]
    fn render_chat_roles_use_gutter_anchor_and_distinct_styles() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::system("system secret should stay hidden"),
                    Message::user("please fix bubble colors"),
                    Message::assistant("done with bubble colors"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("please fix bubble colors"));
        assert!(text.contains("done with bubble colors"));
        assert!(!text.contains("system secret"));
        assert!(!text.contains("you    │"));
        assert!(!text.contains("octos  │"));
        assert!(!text.contains("system │"));

        let user_style = style_for_text(&buffer, "please fix bubble colors").expect("user style");
        let assistant_style =
            style_for_text(&buffer, "done with bubble colors").expect("assistant style");

        // Role-contrast contract: the user's words are the transcript's anchor
        // — accent gutter + bold body, NO bubble background (backgrounds are
        // unreliable in the pager / terminal theme / native scrollback).
        assert!(text.contains("▌ please fix bubble colors"));
        assert!(user_style.add_modifier.contains(Modifier::BOLD));
        assert_ne!(user_style.bg, Some(palette.diff_context_bg));
        // Assistant prose keeps its existing baseline rendering.
        assert_eq!(assistant_style.bg, Some(palette.surface));
        assert!(!text.contains("▌ done with bubble colors"));
    }

    #[test]
    fn render_default_view_keeps_turn_plan_in_chat_without_split_work_pane() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n- [x] Inspect renderer\n- [ ] Patch sticky plan\n- [ ] Run tests",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert!(text.contains("Plan"));
        assert!(text.contains("Inspect renderer"));
        assert!(text.contains("Patch sticky plan"));
        assert!(text.contains("Composer"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("No active plan"));
        assert!(
            row_index_containing(&rows, "Plan") < row_index_containing(&rows, "Composer"),
            "turn plan should stay in chat history above the composer"
        );
    }

    #[test]
    fn render_default_chat_hides_agent_round_plan() {
        let session_id = SessionKey("local:test".into());
        let completed_turn_id = TurnId::new();
        let active_turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("review the project code by code"),
                    Message::assistant("I inspected the first pass."),
                    Message::user("continue the review"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn_id.clone(),
                    text: "Continuing with deeper checks.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: completed_turn_id.clone(),
            request: Some("review the project code by code".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "list_dir", "complete")
                    .with_turn(completed_turn_id.clone())
                    .with_success(true),
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_turn(completed_turn_id)
                    .with_success(true),
            ],
        });
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_turn(active_turn_id)
                .with_success(true),
        );

        let text = rendered_text(&app);

        assert!(text.contains("Continuing with deeper checks."));
        assert!(text.contains("2 completed"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("Plan rounds"));
        assert!(!text.contains("Round 1: review the project code by code"));
        assert!(!text.contains("Current round: continue the review"));
    }

    #[test]
    fn render_plan_strips_source_checkboxes_and_marks_completed_live_items() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n1. [x] Inspect renderer\n2. [ ] Patch sticky plan",
                )],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Plan:\n1. [ ] Inspect renderer\n2. [ ] Patch sticky plan".into(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let plan = extract_plan_lines(&app);
        assert_eq!(
            plan,
            vec![
                RenderedPlanStep {
                    text: "Inspect renderer".into(),
                    completed: true,
                },
                RenderedPlanStep {
                    text: "Patch sticky plan".into(),
                    completed: false,
                },
            ]
        );
        let text = rendered_text(&app);

        assert!(text.contains("Inspect renderer"));
        assert!(text.contains("Patch sticky plan"));
        assert!(!text.contains("[ ] 1. [ ] Inspect renderer"));
    }

    #[test]
    fn render_plan_markdown_without_marker_leakage() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n- [x] **Hero** — build first viewport\n- [ ] `npm run build`",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("Hero"));
        assert!(text.contains("npm run build"));
        assert!(!text.contains("**Hero**"));
        assert!(!text.contains("`npm run build`"));
    }

    #[test]
    fn render_markdown_headings_and_emphasis_without_marker_leakage() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "# What I *can* access:\n\n#### 3.2 *Code Quality* & Maintainability\n\nThis is *available* and `local`.",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("What I can access:"));
        assert!(text.contains("3.2 Code Quality & Maintainability"));
        assert!(text.contains("This is available and local."));
        assert!(!text.contains("*can*"));
        assert!(!text.contains("#### 3.2"));
        assert!(!text.contains("*available*"));
        assert!(!text.contains("`local`"));
    }

    #[test]
    fn render_markdown_checkboxes_as_numbered_choices() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "- [x] Point me to a project inside the workspace\n- [x] Share more about what you want reviewed",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("1. Point me to a project inside the workspace"));
        assert!(text.contains("2. Share more about what you want reviewed"));
        assert!(!text.contains("[x]"));
        assert!(!text.contains("[ ]"));
    }

    #[test]
    fn render_diff_preview_stays_in_transcript_before_composer() {
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Styles patch".into()),
                files: vec![DiffPreviewFile {
                    path: "styles.css".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "added".into(),
                            content: "body {}".into(),
                            old_line: None,
                            new_line: Some(1),
                        }],
                    }],
                }],
            },
        });
        app.sessions[0].messages = vec![
            Message::user("build the site"),
            Message::assistant("Plan:\n- [x] **Hero**\n- [ ] Instruments"),
        ];
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_detail("styles.css")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let activity = row_index_containing(&rows, "Read");
        let diff = row_index_containing(&rows, "Diff Preview");
        let composer = row_index_containing(&rows, "Composer");

        assert!(
            activity < diff,
            "activity should precede diff in transcript"
        );
        assert!(
            diff < composer,
            "diff preview should stay in transcript above composer"
        );
        assert!(!rows.join("\n").contains("Work  sticky"));
        assert!(!rows.join("\n").contains("Activity"));
        assert!(!rows.join("\n").contains("**Hero**"));
    }

    #[test]
    fn render_turn_anchored_diff_preview_stays_with_original_turn() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let preview_id = PreviewId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build the site"),
                    Message::assistant("Built the site."),
                    Message::user("done?"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(
                    ActivityKind::Progress,
                    "file_mutation",
                    "File mutation: modify src/styles.css",
                )
                .with_detail("modify src/styles.css | diff preview ready")
                .with_success(true)
                .with_turn(turn_id.clone()),
            ],
        });
        app.diff_preview
            .open_loading_for_turn(preview_id.clone(), Some(turn_id));
        app.diff_preview.apply_result(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id,
                preview_id,
                title: Some("Styles patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/styles.css".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "added".into(),
                            content: "body {}".into(),
                            old_line: None,
                            new_line: Some(1),
                        }],
                    }],
                }],
            },
        });

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let diff = row_index_containing(&rows, "Diff Preview");
        let latest_prompt = row_index_containing(&rows, "▌ done?");
        let composer = row_index_containing(&rows, "Composer");

        assert!(
            diff < latest_prompt,
            "old diff preview should stay with its original turn, not jump to latest prompt"
        );
        assert!(latest_prompt < composer);
    }

    #[test]
    fn render_inline_approval_shows_diff_choices_without_work_plan() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n- one\n- two\n- three\n- four\n- five\n- six",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "diff_edit".into(),
            title: "Apply patch".into(),
            body: "approve?".into(),
            approval_kind: Some(approval_kinds::DIFF.into()),
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Apply patch"));
        assert!(text.contains("y = approve this command once"));
        assert!(text.contains("s = approve this command/scope for the session"));
        assert!(text.contains("n = deny it"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("more plan item(s) | Ctrl+O expand"));
    }

    fn app_with_user_question(questions: Vec<octos_core::ui_protocol::UserQuestion>) -> AppState {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("set up a project")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let event = octos_core::ui_protocol::UserQuestionRequestedEvent::new(
            SessionKey("local:test".into()),
            octos_core::ui_protocol::QuestionId::new(),
            TurnId::new(),
            "Pick a framework",
            "The agent needs your input.",
            questions,
        );
        app.user_question = Some(UserQuestionPickerState::from_event(event));
        app
    }

    fn user_question(
        header: &str,
        question: &str,
        labels: &[&str],
        multi_select: bool,
    ) -> octos_core::ui_protocol::UserQuestion {
        octos_core::ui_protocol::UserQuestion {
            header: header.into(),
            question: question.into(),
            options: labels
                .iter()
                .map(|label| octos_core::ui_protocol::UserQuestionOption {
                    label: (*label).into(),
                    description: String::new(),
                })
                .collect(),
            multi_select,
            allow_free_text: true,
        }
    }

    #[test]
    fn render_inline_single_select_user_question_shows_radios_and_other() {
        let app = app_with_user_question(vec![user_question(
            "Framework",
            "Which web framework?",
            &["axum", "actix"],
            false,
        )]);

        let text = rendered_text(&app);

        assert!(text.contains("Agent asked a question"));
        assert!(text.contains("Pick a framework"));
        assert!(text.contains("Which web framework?"));
        // Single-select uses a hollow radio marker, not a checkbox.
        assert!(text.contains("○ axum"));
        assert!(text.contains("○ actix"));
        assert!(!text.contains("▣ axum")); // not the multi-select marker
        // Prominence: the highlighted row (cursor defaults to the first
        // option) carries the ▌ accent bar; a non-active row does not.
        assert!(text.contains("▌ ○ axum"));
        assert!(!text.contains("▌ ○ actix"));
        // The always-present free-text "Other" row.
        assert!(text.contains("Other"));
        assert!(text.contains("Enter = submit answer(s)"));
    }

    #[test]
    fn fit_card_text_truncates_by_display_columns_not_chars() {
        // Fix #8: CJK glyphs are double-width; measuring chars() let a CJK
        // question option overflow the card. Budget is width - 4 (the caller's
        // 4-space prefix): width 12 -> 8 columns. "中文选项测试" is 6 chars
        // but 12 columns, so it must truncate (with the ellipsis) to <= 8.
        let fitted = fit_card_text("中文选项测试", 12);
        assert_eq!(fitted, "中文选…");
        assert!(fitted.width() <= 8, "fitted {fitted:?} overflows the card");

        // Within-budget text (by COLUMNS) is untouched: ASCII and a CJK string
        // sitting exactly on the budget.
        assert_eq!(fit_card_text("plain", 12), "plain");
        assert_eq!(fit_card_text("四字选项", 12), "四字选项");
    }

    #[test]
    fn render_inline_multi_select_user_question_shows_checkboxes() {
        let app = app_with_user_question(vec![user_question(
            "Targets",
            "Which targets?",
            &["stable", "nightly"],
            true,
        )]);

        let text = rendered_text(&app);

        // Multi-select uses a hollow square marker (distinct from the radio).
        assert!(text.contains("▢ stable"));
        assert!(text.contains("▢ nightly"));
        assert!(!text.contains("○ stable")); // not the single-select marker
        assert!(text.contains("Other"));
    }

    #[test]
    fn render_garbled_user_question_renders_info_fallback_without_submit_affordance() {
        // No structured questions: must still render the mandatory title/body
        // fallback as an INFORMATIONAL card, but must NOT offer a "Type your
        // answer" affordance (input would be discarded and a submit cannot form a
        // valid respond). Only a dismiss hint is shown (DO-NOT-SHIP #2).
        let app = app_with_user_question(Vec::new());

        let text = rendered_text(&app);

        assert!(text.contains("Pick a framework"));
        assert!(text.contains("The agent needs your input."));
        assert!(text.contains("No answerable options were provided."));
        assert!(text.contains("Esc = dismiss"));
        // No input affordance is offered for the garbled fallback.
        assert!(!text.contains("Type your answer"));
        assert!(!text.contains("Enter = submit"));
    }

    #[test]
    fn render_default_chat_lists_queued_user_questions_without_work_pane() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do a full code review pls")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "Plan:\n- Review renderer\n- Run tests".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.pending_messages = vec![
            "also list queued user questions".into(),
            "check the sticky pane height".into(),
        ];

        let text = rendered_text(&app);

        assert!(text.contains("do a full code review pls"));
        assert!(text.contains("queued 2 messages after active turn"));
        assert!(!text.contains("Work  sticky"));
        assert!(text.contains("› also list queued user questions"));
        assert!(text.contains("› check the sticky pane height"));
    }

    #[test]
    fn render_launch_banner_shows_box_logo_and_greeting_on_empty_session() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("dspfac".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        assert!(
            launch_banner_active(&app),
            "empty session must show the launch banner"
        );
        let text = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 100, 30)
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(
            text.contains("╭"),
            "banner must draw a top-left rounded corner"
        );
        assert!(
            text.contains("╯"),
            "banner must draw a bottom-right rounded corner"
        );
        assert!(text.contains("octos"), "banner box title");
        assert!(
            text.contains("██████╗"),
            "banner must show the OCTOS figlet"
        );
        assert!(
            text.contains("Welcome back — dspfac"),
            "banner greeting names the profile"
        );
    }

    #[test]
    fn launch_banner_hidden_once_session_has_messages() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("dspfac".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        assert!(
            !launch_banner_active(&app),
            "banner must disappear once the conversation starts"
        );
    }

    #[test]
    fn render_status_uses_static_idle_label_without_spinner() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("Idle"));
        for frame in ["◐", "◓", "◑", "◒"] {
            assert!(!text.contains(frame), "idle render must not animate");
        }
    }

    #[test]
    fn render_active_state_uses_bottom_status_without_split_progress_pane() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("build the site")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "Working on it.".into(),
                }),
            }],
            0,
            "thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 100, 28);
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");
        let spinner_count = ["◐", "◓", "◑", "◒"]
            .into_iter()
            .map(|frame| text.matches(frame).count())
            .sum::<usize>();

        assert!(text.contains("Working on it."));
        // The in-progress status marker is the animated octopus spinner now
        // (pinned so it survives a transcript that scrolls the chip off).
        assert!(
            SPINNER_FRAMES
                .iter()
                .any(|frame| text.contains(&format!("state {frame} Working"))),
            "status bar shows an octopus-spinner + Working:\n{text}"
        );
        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        assert_eq!(
            spinner_count, 0,
            "normal chat layout should not animate a split progress pane:\n{text}"
        );
    }

    #[test]
    fn render_work_status_shows_supported_task_affordances() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run background task")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "background build".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: None,
                }],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Working on it.".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("Working"));
        assert!(text.contains("1 background task(s)"));
        assert!(text.contains("/ps to view"));
        assert!(text.contains("/stop to close"));
    }

    #[test]
    fn render_composer_does_not_embed_blocked_status_details() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("complete m9 contract")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_run_state_blocked("approval required");

        let text = rendered_text(&app);

        assert!(text.contains("state ! Blocked"));
        assert!(text.contains("approval required"));
        assert!(!text.contains("Blocked:"));
        assert!(!text.contains("y/s/n approval"));
    }

    #[test]
    fn render_assistant_markdown_is_left_aligned_without_marker_leakage() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "First paragraph\n\n- **Either** install Node.js\n\n| Page | Content |\n|---|---|\n| Home | `Hero` section |",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let prose = row_containing(&rows, "First paragraph");
        let bullet = row_containing(&rows, "Either");
        let table = row_containing(&rows, "Page");
        let text = rows.join("\n");

        assert!(
            prose
                .find("•")
                .is_some_and(|idx| idx < prose.find("First paragraph").unwrap())
        );
        assert_eq!(bullet.find("- "), Some(0));
        // The table is now drawn as a real bordered grid, so its rows start with
        // the box border rather than the raw cell text — still no marker leakage.
        assert!(table.starts_with("│"));
        assert!(table.contains("Page"));
        assert!(!text.contains("|---|---|"));
        assert!(!text.contains("**Either**"));
        assert!(!text.contains("`Hero`"));
    }

    #[test]
    fn render_streaming_sentence_spacing_keeps_words_separated() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "We can implement.Now run tests.All pass. Build is ready:Next step. Rebuild:🎉 done.",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("implement. Now"));
        assert!(text.contains("tests. All"));
        assert!(text.contains("ready: Next"));
        assert!(text.contains("Rebuild: "));
        assert!(text.contains("🎉"));
        assert!(!text.contains("implement.Now"));
        assert!(!text.contains("tests.All"));
    }

    #[test]
    fn render_soft_newlines_in_prose_as_spaces() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "🎉 Build succeeded! All 5 pages built cleanly\nin 291ms:",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let row = row_containing(&rows, "Build succeeded");

        assert!(row.contains("Build succeeded! All 5 pages built cleanly in 291ms:"));
    }

    #[test]
    fn render_markdown_tables_inline_bold_and_inline_code() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "| File | Purpose |\n|---|---|\n| app.rs | **Renderer** and `layout` |",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("File"));
        assert!(text.contains("Purpose"));
        assert!(text.contains("Renderer"));
        assert!(text.contains("layout"));
        assert!(!text.contains("|---|---|"));
        assert!(text.contains("│"));
        let bold_style = style_for_text(&buffer, "Renderer").expect("bold cell style");
        let code_style = style_for_text(&buffer, "layout").expect("inline code style");
        assert!(bold_style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(code_style.fg, Some(palette.highlight));
    }

    #[test]
    fn render_markdown_table_keeps_visible_columns() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "| File | Problem | Fix |\n|---|---|---|\n| Hero.astro | Orphan --- with no content or closing marker | Removed the --- line entirely |\n| Header.astro | Same — bare --- then HTML | Removed the --- line |",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let header = row_containing(&rows, "Problem");
        let hero = row_containing(&rows, "Hero.astro");

        assert!(header.contains("File"));
        assert!(header.contains("Problem"));
        assert!(header.contains("Fix"));
        assert!(header.contains("│"));
        assert!(hero.contains("Hero.astro"));
        assert!(hero.contains("│"));
        assert!(!rows.join("\n").contains("|---|---|---|"));
    }

    #[test]
    fn render_markdown_table_draws_box_borders() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("| A | B |\n|---|---|\n| x | y |")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for ch in ["┌", "┬", "┐", "│", "├", "┼", "┤", "└", "┴", "┘"] {
            assert!(text.contains(ch), "bordered table missing `{ch}`");
        }
        // The old dashed header separator is gone (box-drawing replaces it).
        assert!(!text.contains("-+-"));
    }

    #[test]
    fn render_markdown_table_fits_and_truncates_on_narrow_width() {
        let wide = "| Column One | Column Two | Column Three |\n|---|---|---|\n| a very long first cell value | another long-ish value | a third long cell value |";
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(wide)],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 44, 30);
        let text = rendered_rows(&buffer).join("\n");
        // Still a bordered grid, but cells are ellipsized to fit the narrow pane.
        assert!(text.contains("┌"));
        assert!(text.contains("└"));
        assert!(text.contains("│"));
        assert!(text.contains("…"), "wide cells should be truncated to fit");
    }

    #[test]
    fn render_markdown_table_clips_many_columns_to_pane_width() {
        // codex P2: with enough columns, even minimum-width cells + borders
        // exceed a narrow pane. No produced line may be wider than the pane,
        // or ratatui wraps it and breaks the grid.
        let palette = Palette::for_theme(ThemeName::Codex);
        let header = (1..=8)
            .map(|i| format!("Col{i}"))
            .collect::<Vec<_>>()
            .join(" | ");
        let sep = ["---"; 8].join("|");
        let row = (1..=8)
            .map(|i| format!("value {i} text"))
            .collect::<Vec<_>>()
            .join(" | ");
        let content = format!("| {header} |\n|{sep}|\n| {row} |");
        let width = 30;
        let mut lines = Vec::new();
        push_formatted_body(
            &mut lines,
            palette,
            &content,
            "",
            Some(palette.surface),
            width,
        );
        for line in &lines {
            let line_width: usize = line
                .spans
                .iter()
                .map(|span| span.content.as_ref().width())
                .sum();
            assert!(
                line_width <= width,
                "table line width {line_width} exceeds pane width {width}"
            );
        }
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref().to_string())
            .collect();
        assert!(text.contains("│"), "still a bordered table");
    }

    #[test]
    fn table_cell_width_uses_display_width_for_wide_characters() {
        // Regression: emoji/CJK have display width 2 but a single char, so
        // chars().count() under-padded their table columns and misaligned the
        // separators. Width math must use display width.
        assert_eq!(table_cell_width("ab"), 2);
        assert_eq!(table_cell_width("🐳"), 2);
        assert_eq!(table_cell_width("中文"), 4);
        assert_eq!(table_cell_width("a🐳b"), 4);
    }

    #[test]
    fn markdown_blockquote_detects_quote_lines() {
        assert_eq!(markdown_blockquote("> quoted text"), Some("quoted text"));
        assert_eq!(markdown_blockquote(">quoted"), Some("quoted"));
        assert_eq!(markdown_blockquote("not a quote"), None);
        assert_eq!(markdown_blockquote(">"), None);
    }

    #[test]
    fn render_markdown_blockquote_strips_marker() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("> a quoted line")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("a quoted line"));
        // The literal markdown `>` marker must not leak into the rendered prose.
        assert!(!text.contains("> a quoted line"));
        assert!(text.contains("▌"));
    }

    #[test]
    fn render_markdown_code_fence_uses_clean_gutter() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("```python\nprint('hi')\n```")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("python"));
        assert!(text.contains("print('hi')"));
        // The verbose "end code … --------" footer is gone; a clean box gutter is used.
        assert!(!text.contains("end code"));
        assert!(text.contains("┌─"));
        assert!(text.contains("└─"));
    }

    #[test]
    fn render_diff_code_fence_highlights_added_removed_and_hunks() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "```diff\n--- before.json\n+++ after.json\n@@ -2,6 +2,6 @@\n-  \"scroll-mode\": \"pinned\",\n+  \"scroll-mode\": \"native\",\n```",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);

        let removed_style = style_for_text(&buffer, "pinned").expect("removed diff style");
        let added_style = style_for_text(&buffer, "native").expect("added diff style");
        let hunk_style = style_for_text(&buffer, "@@ -2,6 +2,6 @@").expect("hunk diff style");

        assert_eq!(removed_style.fg, Some(palette.danger));
        assert_eq!(removed_style.bg, Some(palette.danger_bg));
        assert_eq!(added_style.fg, Some(palette.success));
        assert_eq!(added_style.bg, Some(palette.success_bg));
        assert_eq!(hunk_style.fg, Some(palette.accent));
        assert_eq!(hunk_style.bg, Some(palette.diff_context_bg));
    }

    #[test]
    fn render_unlabeled_unified_diff_fence_is_reclassified_from_code() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "```\n--- before.json\n+++ after.json\n@@ -1 +1 @@\n-  \"scroll-mode\": \"pinned\"\n+  \"scroll-mode\": \"native\"\n```",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let rows = rendered_rows(&buffer);

        assert!(row_containing(&rows, "┌─ diff").contains("diff"));
        let added_style = style_for_text(&buffer, "native").expect("added diff style");
        assert_eq!(added_style.fg, Some(palette.success));
        assert_eq!(added_style.bg, Some(palette.success_bg));
    }

    #[test]
    fn render_pipe_commands_are_not_treated_as_markdown_tables() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Use `find . | xargs rm` only in a sandbox.",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        assert!(rendered_text(&app).contains("find . | xargs rm"));
    }

    #[test]
    fn render_first_launch_onboarding_is_not_mixed_with_empty_chat() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text = rendered_text(&store.state);

        assert!(text.contains("Welcome to Octos"));
        assert!(text.contains("Create your local Octos profile"));
        assert!(text.contains("Onboarding setup"));
        assert!(!text.contains("No session selected"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("Ask Octos to change code"));
    }

    /// M22 (#58): the first-run onboarding surface renders the ASCII OCTOS
    /// wordmark in the MAIN window (not a right-side preview pane). This pins
    /// the splash so a future refactor cannot quietly drop the distinctive
    /// identity.
    #[test]
    fn render_first_launch_onboarding_includes_ascii_octos_splash() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text = rendered_text(&store.state);

        // ASCII figlet wordmark (a characteristic block-letter row) plus the
        // human-readable tagline render in the MAIN window.
        assert!(
            text.contains("██████╗"),
            "expected OCTOS figlet art in the main window, got:\n{text}"
        );
        assert!(text.contains("Welcome to Octos — Your Coding Buddy"));
    }

    /// At the soak's narrow 80x24 first-launch size, the OCTOS logo shows in the
    /// main window AND the onboarding menu — through its Continue action — stays
    /// fully visible (codex P2: the logo must never clip the menu).
    #[test]
    fn render_first_launch_onboarding_80x24_shows_logo_without_clipping_menu() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));
        let text =
            rendered_buffer_with_size(&store.state, Palette::for_theme(ThemeName::Slate), 80, 24)
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
        assert!(
            text.contains("Welcome to Octos — Your Coding Buddy"),
            "logo/tagline must render at 80x24"
        );
        assert!(
            text.contains("Continue - Create profile"),
            "menu Continue must not be clipped at 80x24"
        );
    }

    /// UX2 A.1: the OCTOS banner header only consumes rows ABOVE what the menu
    /// needs, so the step list, its inputs, and the explanation pane are never
    /// clipped on short terminals. Full figlet box (11 rows) only with real
    /// surplus AND width; otherwise the compact tagline box (3 rows), then
    /// nothing.
    #[test]
    fn onboarding_header_height_takes_only_menu_surplus() {
        // Tall terminal, menu needs 14 rows → ample surplus → full figlet box.
        assert_eq!(onboarding_header_height(37, 120, 14), 11);
        // Short terminal (root[0] ~16-17 rows, menu needs 14): surplus 2-3 →
        // compact box only once there are 3 surplus rows; below that, nothing.
        assert_eq!(onboarding_header_height(17, 120, 14), 3);
        assert_eq!(onboarding_header_height(16, 120, 14), 0);
        // No surplus → no header at all (the menu takes everything).
        assert_eq!(onboarding_header_height(14, 120, 14), 0);
        // Narrow terminal → never the wide figlet; compact box at most.
        assert_eq!(onboarding_header_height(40, 40, 5), 3);
    }

    /// UX2 A: the three-region onboarding layout renders end-to-end on a wide
    /// terminal — TOP figlet banner header, MAIN step list, and the RIGHT
    /// teaching panel with the current step's explanatory prose (not a bare
    /// checklist). Asserts against the i18n source so it tracks wording/locale.
    #[test]
    fn render_first_launch_onboarding_shows_header_steps_and_explanation_pane() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text =
            rendered_buffer_with_size(&store.state, Palette::for_theme(ThemeName::Slate), 140, 44)
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();

        // TOP: figlet banner header (a characteristic block-letter row) + the
        // bordered box corner.
        assert!(text.contains("██████╗"), "figlet header at top:\n{text}");
        assert!(text.contains('╭'), "header is a bordered window:\n{text}");
        // RIGHT: the teaching panel title + the current step's explanatory
        // prose. Assert against the i18n source (NOT a hardcoded literal) so the
        // test tracks wording/locale changes.
        let panel_title = t!("onboarding.wizard.explain_title", locale = "en");
        assert!(
            text.contains(&*panel_title),
            "teaching panel title in the right pane:\n{text}"
        );
        // The Profile-step explanation is a multi-line source string; assert on
        // its first word so soft-wrapping in the pane can't flake it.
        let explain_first_word = crate::menu::wizard::WizardStep::Profile
            .explanation()
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            !explain_first_word.is_empty() && text.contains(&explain_first_word),
            "current-step explanation prose in the right pane (`{explain_first_word}`):\n{text}"
        );
    }

    #[test]
    fn render_first_launch_onboarding_child_menu_stays_on_onboarding_surface() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD_FAMILY,
        ));

        let text = rendered_text(&store.state);

        assert!(!text.contains("No session selected"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("Ask Octos to change code"));
    }

    /// M22-A: when the backend advertises no onboarding methods,
    /// opening the onboarding menu must render a disabled-reason
    /// status surface — never a blank pane that swallows the
    /// first-launch flow.
    #[test]
    fn render_onboarding_without_capabilities_shows_disabled_reason_not_blank() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store
            .state
            .set_capabilities(UiProtocolCapabilities::new(&[], &[]));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text = rendered_text(&store.state);

        // The status surface MUST surface a typed disabled reason.
        assert!(
            text.contains("Onboarding unavailable"),
            "expected disabled-reason title in rendered text:\n{text}"
        );
        // And it MUST NOT render the empty-chat scaffold under
        // first-launch (no sessions) — that would be the "blank pane"
        // regression the acceptance bullet bans.
        assert!(!text.contains("No session selected"));
        assert!(!text.contains("Ask Octos to change code"));
    }

    #[test]
    fn render_composer_shows_staged_messages() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("working")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "running tool".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.pending_messages = vec![
            "it did not do error recovery?".into(),
            "what is ip for mini5".into(),
        ];
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("Queued messages (2) after active turn"));
        assert!(text.contains("Ctrl+U clear"));
        assert!(text.contains("it did not do error recovery?"));
        assert!(text.contains("what is ip for mini5"));
        assert_eq!(composer_height(&app), 5);
        let pending_style =
            style_for_text(&buffer, "it did not do error recovery?").expect("pending style");
        assert_eq!(pending_style.bg, Some(palette.diff_context_bg));
    }

    #[test]
    fn render_composer_is_tall_and_places_cursor_in_input() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = "fix tests".into();
        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert_eq!(composer_height(&app), 5);
        assert!(text.contains("fix tests"));
        assert!(!text.contains("▌"));
        let rows = rendered_rows(&buffer);
        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "› fix tests")
        );
        assert_eq!(
            cursor,
            composer_cursor_position(&app, Rect::new(0, 36, 120, 5)).expect("cursor")
        );
    }

    /// Regression: the harness-row context `LineGauge` label (`ctx ~N%`) must
    /// inherit the theme `surface` background. `LineGauge` paints its whole
    /// area with the widget base style *before* writing the (unstyled) label,
    /// so without `.style(bg: surface)` the label cells fall back to the raw
    /// terminal background — a mismatched solid block on the right of the
    /// harness row, directly above the composer.
    #[test]
    fn harness_gauge_label_inherits_surface_background() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 1,
                pending_continuations: 0,
                phase: Some("orchestrating".into()),
            },
        );
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 15_360,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer_with_size(&app, palette, 120, 42);

        // The gauge label is rendered on the harness row (the `ctx ~N%` text).
        let label_style = style_for_text(&buffer, "ctx ~").expect("gauge label rendered");
        assert_eq!(
            label_style.bg,
            Some(palette.surface),
            "gauge label must use the surface bg, not the raw terminal background"
        );

        // The whole gauge column (label + filled/unfilled line) must be a single
        // contiguous surface-backed band — no stray bg=Reset cells.
        let rows = rendered_rows(&buffer);
        let gauge_row = row_index_containing(&rows, "ctx ~");
        let width = usize::from(buffer.area.width);
        let row_start = gauge_row * width;
        let first_label_col = rows[gauge_row].find("ctx ~").expect("label col");
        for x in first_label_col..width {
            let cell = &buffer.content[row_start + x];
            assert_eq!(
                cell.bg,
                palette.surface,
                "gauge cell at x={x} (sym {:?}) leaked a non-surface background",
                cell.symbol()
            );
        }
    }

    #[test]
    fn render_composer_places_cursor_after_chinese_display_width() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        for ch in "你好世界".chars() {
            app.insert_composer_char(ch);
        }

        let rect = Rect::new(0, 36, 120, 5);
        let cursor = composer_cursor_position(&app, rect).expect("cursor");

        assert_eq!(app.composer, "你好世界");
        assert_eq!(cursor.x, 12);
        assert_eq!(cursor.y, 38);
    }

    #[test]
    fn render_composer_places_cursor_after_mixed_cjk_and_ascii() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.insert_composer_text("abc你好");

        let cursor = composer_cursor_position(&app, Rect::new(0, 36, 120, 5)).expect("cursor");

        assert_eq!(cursor.x, 11);
        assert_eq!(cursor.y, 38);
    }

    #[test]
    fn render_composer_shows_short_multiline_prompt_rows() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = "first instruction\nsecond instruction\nthird instruction".into();

        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let rows = rendered_rows(&buffer);

        assert_eq!(composer_height(&app), 7);
        assert!(rows.iter().any(|row| row.contains("› first instruction")));
        assert!(rows.iter().any(|row| row.contains("second instruction")));
        assert!(rows.iter().any(|row| row.contains("third instruction")));
        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "third instruction")
        );
        assert_eq!(
            cursor,
            composer_cursor_position(&app, Rect::new(0, 34, 120, 7)).expect("cursor")
        );
    }

    #[test]
    fn render_composer_keeps_common_paste_visible_and_resizes() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = (1..=8)
            .map(|idx| format!("pasted visible line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");

        let (buffer, cursor) = rendered_buffer_and_cursor_with_size(
            &app,
            Palette::for_theme(ThemeName::Codex),
            80,
            42,
        );
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert_eq!(composer_height_for_size(&app, 80, 42), 12);
        assert!(text.contains("pasted visible line 1"));
        assert!(text.contains("pasted visible line 8"));
        assert!(!text.contains("Large paste collapsed"));
        assert_eq!(
            row_index_containing(&rows, "pasted visible line 8"),
            usize::from(cursor.y)
        );
    }

    #[test]
    fn render_composer_shows_tail_when_input_exceeds_visible_budget() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = (1..=14)
            .map(|idx| format!("budgeted line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 80, 42);
        let text = rendered_rows(&buffer).join("\n");

        assert_eq!(composer_height_for_size(&app, 80, 42), 16);
        assert!(text.contains("showing tail"));
        assert!(!text.contains("budgeted line 1 "));
        assert!(text.contains("budgeted line 14"));
    }

    #[test]
    fn render_composer_wraps_long_single_line_into_extra_rows() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = "x".repeat(180);

        assert_eq!(composer_height_for_size(&app, 80, 42), 7);
    }

    #[test]
    fn render_composer_draws_wrapped_tail_of_long_single_line() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // One logical line longer than the composer width: the tail must wrap
        // onto a 2nd visible row, not be clipped (and the reserved row left dark).
        app.composer = format!("HEAD{}TAIL", "x".repeat(160));

        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, _cursor) = rendered_buffer_and_cursor(&app, palette);
        let rows = rendered_rows(&buffer);

        let head_row = row_index_containing(&rows, "HEAD");
        let tail_row = row_index_containing(&rows, "TAIL");
        assert!(
            tail_row > head_row,
            "wrapped tail should render below the head (head={head_row}, tail={tail_row})"
        );
        // ...and it must be drawn in the visible text colour, not the surface bg.
        let tail_style = style_for_text(&buffer, "TAIL").expect("tail rendered");
        assert_eq!(
            tail_style.fg,
            Some(palette.text),
            "wrapped tail must use the composer text colour, not be invisible"
        );
    }

    #[test]
    fn tail_around_cursor_caps_window_to_row_budget() {
        let width = 10;
        let max_rows = 3;
        // A single logical line far taller than the budget.
        let text = "x".repeat(100);

        // Cursor at the very start: HEAD window, must not exceed the budget
        // (render_composer wraps the returned text, so an over-long return clips
        // the composer footer).
        let head = tail_around_cursor(&text, 0, width, max_rows);
        assert!(
            visual_rows_for_text(&head.text, width) <= max_rows,
            "head window must fit row budget, got {} rows",
            visual_rows_for_text(&head.text, width)
        );

        // Cursor at the end: TAIL window, also within budget, marked truncated.
        let tail = tail_around_cursor(&text, text.len(), width, max_rows);
        assert!(
            visual_rows_for_text(&tail.text, width) <= max_rows,
            "tail window must fit row budget, got {} rows",
            visual_rows_for_text(&tail.text, width)
        );
        assert!(tail.text.starts_with("..."), "tail window marks truncation");
    }

    #[test]
    fn render_empty_composer_shows_cursor_before_hint() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("›  Ask Octos to change code"));
        assert!(!text.contains("▌"));
        let rows = rendered_rows(&buffer);
        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "›  Ask Octos")
        );
        assert_eq!(
            cursor,
            composer_cursor_position(&app, Rect::new(0, 36, 120, 5)).expect("cursor")
        );
    }

    /// P2 (tri-repo #246 ⊃ #232 #3, codex fold 4): the live viewport must
    /// leave at least TWO rows above it on terminals tall enough — DECSTBM
    /// requires top < bottom, so both a full-screen viewport (`CSI 1;0r`)
    /// and a one-row region (`CSI 1;1r`) are unusable for history flushes.
    /// The degenerate 1–2-row terminals keep one live row and are handled by
    /// insert_history's streaming fallback.
    #[test]
    fn live_ui_height_leaves_a_valid_scroll_region_above_the_viewport() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        for height in 3..=10u16 {
            let live = live_ui_height(&app, 80, height);
            assert!(
                live <= height - 2,
                "live UI must leave ≥2 history rows above the viewport: height={height} live={live}"
            );
        }
        // Degenerate 1–2-row terminals: the streaming fallback owns these.
        assert_eq!(live_ui_height(&app, 80, 2), 1);
        assert_eq!(live_ui_height(&app, 80, 1), 1);
    }

    #[test]
    fn render_queued_composer_places_cursor_on_text_row() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("working")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "still running".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.composer = "dsada d".into();
        app.pending_messages = vec!["queued prompt".into()];

        let (buffer, cursor) =
            rendered_buffer_and_cursor(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);

        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "› dsada d")
        );
        assert_ne!(
            usize::from(cursor.y),
            row_index_containing(&rows, "Queued messages (1)") + 2
        );
    }

    #[test]
    fn render_composer_collapses_large_paste_and_keeps_chrome_visible_when_narrow() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = (1..=40)
            .map(|idx| format!("paste-line-{idx:02}-with-some-extra-context"))
            .collect::<Vec<_>>()
            .join("\n");

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 48, 18);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("Large paste collapsed"));
        assert!(text.contains("[paste] Pasted block"));
        assert!(text.contains("preview: paste-line-01"));
        assert!(!text.contains("paste-line-40"));
        assert!(text.contains("Composer"));
        assert!(text.contains("state"));
    }

    #[test]
    fn render_transcript_includes_activity_cards_and_dense_footer() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("gpt-5-codex".into()),
                messages: vec![Message::user("fix the UI")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "working".into(),
                }),
            }],
            0,
            "Tool started: shell".into(),
            Some("/repo/octos".into()),
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id)
                .with_tool_call("call-1")
                .with_detail("cargo test")
                .with_output_preview("running 6 tests\n6 passed")
                .with_success(true)
                .with_duration_ms(1250),
        );

        let text = rendered_text(&app);

        assert!(!text.contains("Activity"));
        assert!(text.contains("⏺ Bash($ cargo test"));
        assert!(text.contains("running 6 tests"));
        assert!(text.contains("1 more line(s) hidden (Ctrl+O expand)"));
        assert!(text.contains("1.2s"));
        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        // #267 no-call-id: the activity card must NOT display the `call <id>`
        // suffix (the tool_call_id field is retained, only the display is gone).
        assert!(!text.contains("call call-1"));
        assert!(text.contains("gpt-5-codex"));
        assert!(text.contains("state"));
        assert!(text.contains("running"));
        assert!(text.contains("approval"));
        assert!(text.contains("1 msgs/0 tasks"));
    }

    /// Regression (indent-not-honored): the agent-task child row used to be one
    /// long ratatui `Line` that overflowed the terminal width and wrapped back
    /// to column 0 (the transcript renders with `Wrap { trim: false }`, which
    /// has no hanging indent). Every rendered child line — the invocation row
    /// AND its output-preview lines — must now fit within `wrap_width` at ANY
    /// terminal width, measured with unicode-width so a long CJK command
    /// (double-width glyphs) still fits and never panics at a multibyte cut.
    #[test]
    fn agent_task_child_row_never_exceeds_wrap_width() {
        let long_ascii = format!("echo {}", "abcdefgh ".repeat(40));
        let long_cjk = format!("echo {}", "数据处理与网络请求".repeat(20));
        let items = [
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_arguments(serde_json::json!({ "cmd": long_ascii }))
                .with_tool_call("call_01_ABCDEFGHIJKLMNOP")
                .with_output_preview("=== 1. teams ===\nsome very long output line that keeps going and going and going and going and going")
                .with_success(true)
                .with_duration_ms(21),
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_arguments(serde_json::json!({ "cmd": long_cjk }))
                .with_tool_call("call_02_ZYXWVUTSRQPONMLK")
                .with_success(true)
                .with_duration_ms(21),
        ];
        for wrap_width in [20usize, 32, 48, 60, 80, 120] {
            for item in &items {
                let mut lines: Vec<Line<'static>> = Vec::new();
                push_agent_task_child(
                    &mut lines,
                    Palette::for_theme(ThemeName::Slate),
                    item,
                    true,
                    false,
                    wrap_width,
                    false,
                );
                assert!(
                    !lines.is_empty(),
                    "child row should render at least one line"
                );
                for line in &lines {
                    let w: usize = line
                        .spans
                        .iter()
                        .map(|span| span.content.as_ref().width())
                        .sum();
                    assert!(
                        w <= wrap_width,
                        "child line width {w} exceeds wrap_width {wrap_width}: {:?}",
                        lines_text(&lines)
                    );
                }
            }
        }
    }

    /// The bash row must surface the actual command (`$ echo …`), never the raw
    /// serialized arguments (`{"cmd":…}`), and must not append the `call <id>`
    /// noise (#267 established "no call-id" for CC-style activity cards; this
    /// agent-task-group child path predated that work and still leaked both).
    #[test]
    fn agent_task_bash_row_shows_command_not_raw_json_or_call_id() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_arguments(serde_json::json!({
                "cmd": "echo \"=== 1. teams ===\" && curl -sX POST http://localhost:4000/"
            }))
            .with_tool_call("call_01_UVIa9EBA331xAfxbPFPM4446")
            .with_success(false)
            .with_duration_ms(21);
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_agent_task_child(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            &item,
            true,
            false,
            120,
            false,
        );
        let text = lines_text(&lines);
        assert!(
            text.contains("Bash($ echo"),
            "bash row must show the command: {text:?}"
        );
        assert!(
            !text.contains("{\"cmd\""),
            "bash row must not show raw JSON args: {text:?}"
        );
        assert!(
            !text.contains("call call_"),
            "bash row must not show call-id noise: {text:?}"
        );
        assert!(
            !text.contains("call_01_UVIa9EBA331xAfxbPFPM4446"),
            "the call-id must not be displayed: {text:?}"
        );
    }

    fn agent_task_child_text(item: &ActivityItem, wrap_width: usize) -> String {
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_agent_task_child(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            item,
            true,
            false,
            wrap_width,
            false,
        );
        lines_text(&lines)
    }

    /// Live-capture regression (#273 follow-up): on the real server path the
    /// invocation comes from `detail` — the protocol #1606 `arguments_preview`
    /// echo, a JSON serialization of the tool args capped at ~700 bytes, so it
    /// often arrives CUT mid-string (no closing quote/brace, unparseable by
    /// strict serde). The shell row must still extract `$ <command>`; the raw
    /// `{"cmd":…` framing must never render.
    #[test]
    fn agent_task_bash_row_extracts_command_from_truncated_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_detail(
                r#"{"cmd":"grep -n '<img' /Users/yuechen/dev/2026-world-cup/client/src/pages/HomePage.tsx /Users/yuechen/dev/2026-world-cup/client/s"#,
            )
            .with_tool_call("call_01_ABCDEFGHIJKLMNOP")
            .with_success(true)
            .with_duration_ms(33);
        let text = agent_task_child_text(&item, 120);
        assert!(
            text.contains("$ grep -n '<img'"),
            "truncated echo must still yield the command: {text:?}"
        );
        assert!(
            !text.contains("{\"cmd\""),
            "raw JSON echo must never render: {text:?}"
        );
    }

    /// A complete (untruncated) args echo in `detail` parses strictly and the
    /// shell row shows the command alone — sibling keys like `timeout` are
    /// noise the raw echo used to drag in.
    #[test]
    fn agent_task_bash_row_extracts_command_from_complete_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_detail(r#"{"cmd":"echo hi","timeout":5}"#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            text.contains("$ echo hi"),
            "complete echo must yield the command: {text:?}"
        );
        assert!(
            !text.contains("{\"cmd\"") && !text.contains("timeout"),
            "echo framing and sibling keys must not render: {text:?}"
        );
    }

    /// The envelope live lane parks the same args echo in `arguments` as a
    /// JSON String (detail carries the load-bearing thread marker there, and
    /// after archival the echo can surface via `arguments`). A string-typed
    /// `arguments` must be treated exactly like a detail echo — never
    /// re-serialized into `"{\"cmd\":…`.
    #[test]
    fn agent_task_bash_row_extracts_command_from_string_arguments_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_arguments(serde_json::Value::String(
                r#"{"cmd":"echo hi","timeout":5}"#.into(),
            ))
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            text.contains("$ echo hi"),
            "string-arguments echo must yield the command: {text:?}"
        );
        assert!(
            !text.contains("cmd") && !text.contains("\\\""),
            "echo framing must not render (raw or re-escaped): {text:?}"
        );
    }

    /// Non-shell tools: a complete args echo in `detail` renders the compact
    /// `key=value` form (same as the object-arguments path), and JSON string
    /// escapes (`\n`) never leak into the one-line row as literal two-char
    /// sequences.
    #[test]
    fn agent_task_edit_row_compacts_complete_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "edit_file", "complete")
            .with_detail(r#"{"path":"/a/App.tsx","new_string":"<Route/>\n  <Route/>"}"#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        // serde_json maps iterate alphabetically (no preserve_order), so the
        // first meaningful field is `new_string`; its REAL newline (decoded by
        // the strict parse) must flatten to spaces in the one-line row.
        assert!(
            text.contains("new_string=<Route/>   <Route/>"),
            "complete echo must compact to key=value: {text:?}"
        );
        assert!(
            !text.contains("{\"path\""),
            "raw JSON echo must never render: {text:?}"
        );
        assert!(
            !text.contains("\\n"),
            "literal backslash-n must never render: {text:?}"
        );
    }

    /// Non-shell tools with a TRUNCATED echo (strict parse fails): the cleanup
    /// pass must strip the `{"` framing and decode the common escapes — the
    /// bar is NO raw `{"key":` prefix and NO literal `\n` in the row.
    #[test]
    fn agent_task_edit_row_scrubs_truncated_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "edit_file", "complete")
            .with_detail(r#"{"path":"/a/App.tsx","new_string":"<Route/>\n  <Ro"#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            !text.contains("{\"path\""),
            "raw JSON echo prefix must never render: {text:?}"
        );
        assert!(
            !text.contains("\\n"),
            "literal backslash-n must never render: {text:?}"
        );
        assert!(
            text.contains("/a/App.tsx"),
            "the echo's content should survive the scrub: {text:?}"
        );
    }

    /// The producer's `key: value` preview format (object args rendered as
    /// `path: "...", new_string: "..."`) JSON-encodes string values, so `\n`
    /// escapes leak as literal two-char sequences — the display pass must
    /// decode them (rows are one-line; an escaped newline becomes a space).
    #[test]
    fn agent_task_row_unescapes_key_value_echo_escapes() {
        let item = ActivityItem::new(ActivityKind::Tool, "edit_file", "complete")
            .with_detail(r#"path: "/a/App.tsx", new_string: "<Route/>\n  <Route/>""#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            !text.contains("\\n"),
            "literal backslash-n must never render: {text:?}"
        );
        assert!(
            text.contains("path: \"/a/App.tsx\""),
            "non-JSON detail otherwise renders as-is: {text:?}"
        );
    }

    /// Plain (non-JSON) details are untouched: a bang command echo and the
    /// load-bearing envelope thread marker render verbatim.
    #[test]
    fn agent_task_row_keeps_plain_detail_verbatim() {
        let bang = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_detail("! echo hi")
            .with_success(true);
        let text = agent_task_child_text(&bang, 120);
        assert!(
            text.contains("! echo hi"),
            "plain detail must render unchanged: {text:?}"
        );

        let marker = ActivityItem::new(ActivityKind::Tool, "shell", "running")
            .with_detail(AppState::envelope_tool_detail_for_thread("th-123"));
        let text = agent_task_child_text(&marker, 120);
        assert!(
            text.contains("thread th-123"),
            "thread marker must render unchanged: {text:?}"
        );
    }

    /// Fidelity guard (codex review): `detail` ALSO carries already-decoded
    /// REAL invocation text — the `!`-bang echo and the live-lane
    /// `tool_invocation_detail` command summaries. A brace-group command must
    /// keep its `{` (only `{"…` is a JSON echo), and an intentional two-char
    /// `\n` in a real command (`printf '\n'`) must render verbatim — the
    /// escape decode applies to serialized echo shapes, not plain commands.
    #[test]
    fn agent_task_row_keeps_real_commands_verbatim() {
        for title in ["shell", "!"] {
            let brace_group = ActivityItem::new(ActivityKind::Tool, title, "complete")
                .with_detail("{ echo ok; }")
                .with_success(true);
            let text = agent_task_child_text(&brace_group, 120);
            assert!(
                text.contains("{ echo ok; }"),
                "brace-group command must render verbatim for {title}: {text:?}"
            );
        }
        let printf = ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_detail(r#"printf '\n' | wc -l"#)
            .with_success(true);
        let text = agent_task_child_text(&printf, 120);
        assert!(
            text.contains(r#"printf '\n' | wc -l"#),
            "a real command's two-char escape must render verbatim: {text:?}"
        );
    }

    /// The lenient extractor never panics on multibyte content, respects a
    /// closing quote when one survived the cut, decodes escapes, and drops a
    /// dangling backslash left by the byte cap.
    #[test]
    fn lenient_echo_extraction_handles_multibyte_escapes_and_cuts() {
        let cases: &[(&str, &str)] = &[
            // CJK content cut with the producer's ellipsis, no closing quote.
            (
                "{\"cmd\":\"echo 日本語のコマンド…",
                "echo 日本語のコマンド…",
            ),
            // Closing quote survived the cut: trailing sibling junk dropped.
            (r#"{"cmd":"echo hi","timeo"#, "echo hi"),
            // Escaped quote/backslash decode; escaped newline becomes space.
            (r#"{"cmd":"echo \"hi\" \\ a\nb"#, "echo \"hi\" \\ a b"),
            // Dangling backslash at the cut is dropped.
            (r#"{"cmd":"echo hi\"#, "echo hi"),
            // `command` key works too.
            (r#"{"command":"ls -la","cwd":"/tmp"}"#, "ls -la"),
        ];
        for (echo, expected) in cases {
            let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete").with_detail(*echo);
            let text = tool_invocation_text(&item).expect("invocation");
            assert_eq!(
                &text, expected,
                "echo {echo:?} must extract {expected:?}, got {text:?}"
            );
        }
    }

    /// The recovery-suggestion row (a non-Tool `Warning` activity) also predated
    /// the no-call-id convention — it must not append `call <id>` either (the
    /// exact fragment that wrapped to column 0 in the reported bug).
    #[test]
    fn agent_task_recovery_row_drops_call_id() {
        let item = ActivityItem::new(
            ActivityKind::Warning,
            "Recovery suggestion",
            "permission blocked; ask for the exact permission/escalation",
        )
        .with_tool_call("call_01_UVIa9EBA331xAfxbPFPM4446");
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_agent_task_child(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            &item,
            false,
            false,
            120,
            false,
        );
        let text = lines_text(&lines);
        assert!(
            text.contains("Recovery suggestion"),
            "recovery row should render: {text:?}"
        );
        assert!(
            !text.contains("call call_"),
            "recovery row must not show call-id noise: {text:?}"
        );
        assert!(
            !text.contains("call_01_"),
            "recovery row call-id must not be displayed: {text:?}"
        );
    }

    /// An armed loop fires real model turns on an interval — the status bar
    /// must say so at a glance (a forgotten loop otherwise burns tokens
    /// invisibly). Paused loops surface too so `/loop resume` is
    /// discoverable.
    #[test]
    fn status_bar_shows_loop_chip_when_session_has_loops() {
        fn loop_record(status: &str) -> octos_core::ui_protocol::UiLoopRecord {
            serde_json::from_value(serde_json::json!({
                "loop_id": "loop-1",
                "session_id": "local:loops",
                "prompt": "keep poking",
                "mode": "interval",
                "interval_seconds": 60,
                "status": status,
                "expires_at_ms": 0,
                "created_at_ms": 0,
                "updated_at_ms": 0,
            }))
            .expect("loop record")
        }
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:loops".into()),
                title: "loops".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let session_id = SessionKey("local:loops".into());

        app.set_session_loops(&session_id, vec![loop_record("active")]);
        let text = rendered_text(&app);
        assert!(
            text.contains("1 active loop"),
            "active loop chip missing: {text}"
        );

        app.set_session_loops(&session_id, vec![loop_record("paused")]);
        let text = rendered_text(&app);
        assert!(
            text.contains("1 paused loop"),
            "paused loop chip missing: {text}"
        );

        app.set_session_loops(&session_id, vec![]);
        let text = rendered_text(&app);
        assert!(
            !text.contains("loop(s)"),
            "chip must vanish with no loops: {text}"
        );
    }

    /// A deleted loop is a tombstone — `/loop delete` removed it, so it
    /// must not linger as a dimmed zombie chip in the sticky autonomy
    /// indicator. Deleted records can still arrive via the `loop/list`
    /// rehydration path, so `set_session_loops` must strip them exactly
    /// like `upsert_session_loop` does. Without the filter the row reads
    /// "0 running" (the active/paused counts exclude tombstones) yet
    /// still renders chips — the `#1576` delete-can't-clear-it symptom.
    #[test]
    fn deleted_loops_do_not_surface_as_zombie_chips() {
        fn loop_record(status: &str) -> octos_core::ui_protocol::UiLoopRecord {
            serde_json::from_value(serde_json::json!({
                "loop_id": "loop-1",
                "session_id": "local:loops",
                "prompt": "keep poking",
                "mode": "interval",
                "interval_seconds": 60,
                "status": status,
                "expires_at_ms": 0,
                "created_at_ms": 0,
                "updated_at_ms": 0,
            }))
            .expect("loop record")
        }
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:loops".into()),
                title: "loops".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let session_id = SessionKey("local:loops".into());

        // Positive control: an active loop DOES surface as a chip, so the
        // negative assertions below are meaningful.
        let retained = app.set_session_loops(&session_id, vec![loop_record("active")]);
        assert_eq!(retained, 1);
        assert!(
            rendered_text(&app).contains("keep poking"),
            "active loop chip should render"
        );

        // Regression: a deleted (tombstoned) loop must be dropped, not
        // stored and dimmed. The returned count must reflect the retained
        // loops so the refresh acknowledgment can't claim more than the
        // indicator shows (codex P2).
        let retained = app.set_session_loops(&session_id, vec![loop_record("deleted")]);
        assert_eq!(retained, 0, "deleted-only batch retains nothing");
        assert_eq!(
            app.session_autonomy_for(&session_id)
                .map(|state| state.loops.len()),
            Some(0),
            "deleted loop must be filtered out of the mirror"
        );
        assert_eq!(app.session_loop_counts(&session_id), (0, 0));
        let text = rendered_text(&app);
        assert!(
            !text.contains("keep poking"),
            "deleted loop must not render a chip: {text}"
        );

        // Mixed batch: only the non-deleted survivor is kept and counted.
        let retained = app.set_session_loops(
            &session_id,
            vec![loop_record("active"), loop_record("deleted")],
        );
        assert_eq!(retained, 1, "mixed batch retains only the non-deleted loop");
        assert_eq!(
            app.session_autonomy_for(&session_id)
                .map(|state| state.loops.len()),
            Some(1),
            "mixed batch must keep only the non-deleted loop"
        );
        assert_eq!(app.session_loop_counts(&session_id), (1, 0));
    }

    #[test]
    fn render_activity_is_anchored_after_latest_user_prompt() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("what is the status"),
                    Message::user("are you working"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_detail("cargo test")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);
        let first_prompt = text.find("what is the status").expect("first prompt");
        let latest_prompt = text.find("are you working").expect("latest prompt");
        let command = text.find("Bash($ cargo test").expect("activity command");

        assert!(first_prompt < latest_prompt);
        assert!(latest_prompt < command);
        assert!(!text.contains("Activity"));
    }

    #[test]
    fn render_completed_turn_activity_log_is_interleaved_with_chat_history() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build the site"),
                    Message::assistant("The site is built and ready."),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo build")
                    .with_output_preview("Finished dev build")
                    .with_success(true),
            ],
        });

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);
        let prompt = text.find("build the site").expect("user prompt");
        let work_log = text.find("Agent task completed").expect("agent task");
        let command = text.find("Bash($ cargo build").expect("tool command");
        let answer = text
            .find("The site is built and ready.")
            .expect("assistant answer");

        assert!(prompt < answer);
        assert!(answer < work_log);
        assert!(work_log < command);
        assert!(!text.contains("Activity"));
    }

    #[test]
    fn render_large_completed_turn_activity_log_is_compact_by_default() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let items = (1..=12)
            .map(|idx| {
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_turn(turn_id.clone())
                    .with_tool_call(format!("read-{idx}"))
                    .with_detail(format!("src/file_{idx}.rs"))
                    .with_success(true)
            })
            .collect::<Vec<_>>();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("review everything"),
                    Message::assistant("Review complete."),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id,
            request: Some("review everything".into()),
            anchor_index: Some(0),
            items,
        });

        let text = rendered_text(&app);

        assert!(text.contains("Agent task completed"));
        assert!(text.contains("... +9 more"));
        assert!(text.contains("12 completed"));
        assert!(!text.contains("src/file_1.rs"));
    }

    #[test]
    fn chip_stays_orchestrating_while_sub_agents_run_after_parent_calls_complete() {
        // Parallel-spawn regression: `spawn` returns immediately, so the parent
        // turn's tool calls are all "completed" while the spawned sub-agents
        // (session.tasks, Running) are still working. The chip must NOT say
        // "Agent task completed" — it should stay "Orchestrating…" and surface
        // the running sub-agent count.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("launch agents to study X, Y, Z")],
                tasks: vec![
                    crate::model::TaskView {
                        id: octos_core::TaskId::new(),
                        title: "hermes-research".into(),
                        state: TaskRuntimeState::Running,
                        runtime_detail: None,
                        output_tail: String::new(),
                        turn_id: None,
                    },
                    crate::model::TaskView {
                        id: octos_core::TaskId::new(),
                        title: "openclaw-research".into(),
                        state: TaskRuntimeState::Running,
                        runtime_detail: None,
                        output_tail: String::new(),
                        turn_id: None,
                    },
                ],
                // Parent turn has FINISHED (no live_reply) but the background
                // sub-agents it spawned are still running — the chip must still
                // attribute them (via latest-turn), not flip to "completed".
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("launch agents".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
                ActivityItem::new(ActivityKind::Tool, "glob", "complete")
                    .with_turn(turn_id)
                    .with_success(true),
            ],
        });

        let text = rendered_text(&app);
        assert!(
            text.contains("Orchestrating"),
            "chip must stay Orchestrating while sub-agents run: {text:?}"
        );
        assert!(
            text.contains("2 sub-agent(s) running"),
            "chip should surface the running sub-agent count: {text:?}"
        );
        assert!(
            !text.contains("Agent task completed"),
            "chip must NOT report completed while sub-agents run: {text:?}"
        );
    }

    #[test]
    fn agent_task_group_title_with_pending_continuations_does_not_say_completed() {
        // Gap 2 fix #2: when the parent's tool calls are all settled (no active
        // items, no running sub-agents) but the server reports a pending
        // continuation, the title must NOT read "Agent task completed" — that
        // "looks done" lie hides the master re-entry. It must reflect
        // re-entering/continuing instead. The pending re-entry only applies to
        // the CURRENT/active group (`is_active_group = true`).
        let settled = agent_task_group_title(false, 0, 0, true);
        assert_eq!(settled, "Agent task completed", "baseline settled title");

        let reentering = agent_task_group_title(false, 0, 1, true);
        assert!(
            !reentering.to_lowercase().contains("completed")
                && !reentering.to_lowercase().contains("done"),
            "pending continuation must not read as completed/done: {reentering:?}"
        );
        assert!(
            reentering.to_lowercase().contains("re-enter")
                || reentering.to_lowercase().contains("continu"),
            "pending continuation must read as re-entering/continuing: {reentering:?}"
        );

        // In-progress still wins (orchestrating), and errors still surface even
        // with a pending continuation.
        assert!(agent_task_group_title(true, 0, 1, true).contains("Orchestrating"));
        assert!(
            agent_task_group_title(false, 2, 0, true)
                .to_lowercase()
                .contains("error")
        );
    }

    #[test]
    fn agent_task_group_title_pending_continuation_does_not_retitle_archived_group() {
        // Blocking bug 1: `pending_continuations` is the active session's queued
        // re-entry count. It is fed into EVERY group title call, including
        // ARCHIVED past-turn groups. A settled archived group (no live work)
        // must keep its "completed" title even while a continuation is pending —
        // only the CURRENT/active group may flip to "Re-entering". Guard via
        // `is_active_group = false`.
        let archived_completed = agent_task_group_title(false, 0, 1, false);
        assert_eq!(
            archived_completed, "Agent task completed",
            "archived completed group must NOT read as re-entering: {archived_completed:?}"
        );

        // An archived FAILED group must keep its failed title — `failed > 0`
        // must NOT be overridden by the active session's pending continuation.
        let archived_failed = agent_task_group_title(false, 2, 1, false);
        assert!(
            archived_failed.to_lowercase().contains("error"),
            "archived failed group must keep its failed title, not re-entering: {archived_failed:?}"
        );
        assert!(
            !archived_failed.to_lowercase().contains("re-enter"),
            "archived failed group must NOT read as re-entering: {archived_failed:?}"
        );
    }

    #[test]
    fn archived_completed_group_keeps_title_while_active_turn_continuation_pending() {
        // Blocking bug 1 (end-to-end render): a session has an ARCHIVED
        // completed turn (turn A) AND a live active turn (turn B). The server
        // reports a pending continuation for the session. The archived group
        // must STILL read "Agent task completed" — only the active turn's group
        // may flip to "Re-entering". (RED on f588b6f: the pending count was fed
        // to every group, retitling the archived completed turn.)
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let archived_turn = TurnId::new();
        let active_turn = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("first request"),
                    Message::assistant("First answer."),
                    Message::user("second request"),
                ],
                tasks: vec![],
                // Active turn B is live (the current/active group).
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Archived completed group for turn A, anchored to the first request.
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: archived_turn,
            request: Some("first request".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete").with_success(true),
            ],
        });
        // Server has a continuation queued for the active session.
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 1,
                phase: Some("re-entering".into()),
            },
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Agent task completed"),
            "archived completed group must keep its title: {text:?}"
        );
    }

    #[test]
    fn archived_failed_group_keeps_failed_title_while_continuation_pending() {
        // Blocking bug 1 (end-to-end render): an ARCHIVED FAILED group must keep
        // its failed title even while a continuation is pending for the active
        // session — pending must NOT override `failed > 0` for a non-active
        // group. (RED on f588b6f: pending won over failed, losing the failed
        // title on archived groups.)
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let archived_turn = TurnId::new();
        let active_turn = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("first request"),
                    Message::assistant("First answer."),
                    Message::user("second request"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: archived_turn,
            request: Some("first request".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "failed").with_success(false),
            ],
        });
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 1,
                phase: Some("re-entering".into()),
            },
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Agent task finished with errors"),
            "archived failed group must keep its failed title: {text:?}"
        );
    }

    #[test]
    fn active_turn_group_with_pending_continuation_reads_reentering() {
        // Blocking bug 1 (pins intended behavior): the ACTIVE/current turn's
        // group (the live `live_reply` turn, archived to its log) DOES read
        // "Re-entering (continuing)…" when a continuation is pending. The active
        // group is identified by `log.turn_id == active_turn().turn_id`.
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let active_turn = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("only request")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // The active turn's settled tool calls are archived to its log (the
        // re-entry gap: parent calls done, continuation queued).
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: active_turn.clone(),
            request: Some("only request".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(active_turn)
                    .with_success(true),
            ],
        });
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 1,
                phase: Some("re-entering".into()),
            },
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Re-entering (continuing)"),
            "active turn group with pending continuation reads re-entering: {text:?}"
        );
        assert!(
            !text.contains("Agent task completed"),
            "active turn group must NOT read completed during the re-entry gap: {text:?}"
        );
    }

    #[test]
    fn task_group_counts_tally_full_set_not_display_cap() {
        // Render-cap bug: the chip header counted the DISPLAY-CAPPED slice (3 or
        // 12 rows), so a 66-action turn read "3 action(s) · 3 completed" even
        // though its sibling footer correctly tallied the full 66. The header
        // and footer now both call `task_group_counts` over the FULL set, so the
        // counts reflect 66 actions — not the cap.
        let mut items: Vec<ActivityItem> = Vec::new();
        // 60 completed earlier actions.
        for _ in 0..60 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "shell", "complete").with_success(true),
            );
        }
        // 2 active (still running) earlier actions.
        items.push(ActivityItem::new(
            ActivityKind::Tool,
            "run_pipeline",
            "running",
        ));
        items.push(ActivityItem::new(
            ActivityKind::Tool,
            "run_pipeline",
            "running",
        ));
        // 1 failed earlier action.
        items.push(ActivityItem::new(ActivityKind::Tool, "shell", "failed").with_success(false));
        // Last 3 (the only ones the chip renders as children) are completed.
        for _ in 0..3 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete").with_success(true),
            );
        }
        assert_eq!(items.len(), 66, "fixture sanity: 66 total actions");

        let full: Vec<&ActivityItem> = items.iter().collect();
        let (total, completed, active, failed) = task_group_counts(&full);
        assert_eq!(total, 66, "total must be the FULL set, not the display cap");
        assert_eq!(completed, 63, "60 early + 3 late completed");
        assert_eq!(active, 2, "two running actions");
        assert_eq!(failed, 1, "one failed action");

        // The display-capped slice (last 3) must NOT be what the header counts:
        // if the header tallied the cap it would read 3/3/0/0 — the original bug.
        let capped: Vec<&ActivityItem> = full.iter().rev().take(3).rev().copied().collect();
        let (cap_total, cap_completed, _, _) = task_group_counts(&capped);
        assert_eq!(cap_total, 3);
        assert_eq!(cap_completed, 3);
        assert_ne!(
            (total, completed),
            (cap_total, cap_completed),
            "header tally must differ from the display-cap tally"
        );
    }

    #[test]
    fn chip_header_counts_full_turn_set_and_agrees_with_footer() {
        // End-to-end render guard: a 66-action turn's chip HEADER must read the
        // full set ("66 action(s) · ... 66 completed") and AGREE with its sibling
        // "... +63 more" footer — not the display-capped "3 action(s) · 3
        // completed". RED on the pre-fix code: the header counted only the last
        // 3 rendered children.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut items: Vec<ActivityItem> = Vec::new();
        // 63 earlier actions, all completed.
        for _ in 0..63 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
            );
        }
        // Last 3 (the rendered children) completed too → 66 total, 66 completed.
        for _ in 0..3 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
            );
        }
        assert_eq!(items.len(), 66);

        let app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("big turn")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let mut app = app;
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id,
            request: Some("big turn".into()),
            anchor_index: Some(0),
            items,
        });

        let text = rendered_text(&app);
        assert!(
            text.contains("66 action(s)"),
            "header must read the full 66-action set, not the display cap: {text:?}"
        );
        assert!(
            !text.contains("3 action(s)"),
            "header must NOT read the capped 3-action slice: {text:?}"
        );
        // 63 of the 66 are hidden (only 3 rendered as children); footer tallies
        // the full set, so header and footer must agree.
        assert!(
            text.contains("+63 more"),
            "footer must report the 63 hidden actions: {text:?}"
        );
        assert!(
            text.contains("66 completed"),
            "header completed count must reflect the full set: {text:?}"
        );
    }

    #[test]
    fn agent_task_group_title_failed_active_turn_with_pending_reads_reentering() {
        // Precedence decision for the ACTIVE group: a failed active turn that
        // the server is genuinely continuing (pending_continuations > 0) reads
        // "Re-entering (continuing)…" — the queued continuation is the live
        // truth (the failure is being retried/continued), so it wins over the
        // failed title FOR THE ACTIVE GROUP ONLY.
        let active_failed_pending = agent_task_group_title(false, 1, 1, true);
        assert!(
            active_failed_pending.to_lowercase().contains("re-enter")
                || active_failed_pending.to_lowercase().contains("continu"),
            "failed active turn that is continuing reads re-entering: {active_failed_pending:?}"
        );

        // A failed active turn with NO pending continuation still reads as
        // failed (no continuation queued → it really did finish with errors).
        let active_failed = agent_task_group_title(false, 1, 0, true);
        assert!(
            active_failed.to_lowercase().contains("error"),
            "failed active turn with no continuation reads as failed: {active_failed:?}"
        );
    }

    #[test]
    fn leaked_running_item_in_terminal_turn_log_does_not_pin_orchestrating() {
        // Orphan activity-chip self-heal: a `ToolStarted` whose matching
        // `ToolCompleted` never arrived (a leaked spawn_only chip / any future
        // uncovered path) leaves a "running"-status item bound to the turn. When
        // the turn reaches its terminal state, `capture_completed_turn_activity`
        // archives the turn's activity AND reconciles the stranded running item
        // to a terminal status. With no live work and no running sub-agents, the
        // captured chip must NOT stay pinned on "Orchestrating…" — its turn is
        // over. This is the path that reappears after a reconnect: hydrate
        // replays the unbalanced started-state and the turn re-completes through
        // the same capture, healing the residual chip.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("run the background job"),
                    Message::assistant("Kicked off the background job."),
                ],
                // No live_reply → this turn is terminal / not the active turn,
                // and no sub-agent tasks remain.
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Leaked started-state in the turn's live activity: status never reached
        // terminal because no `ToolCompleted` arrived.
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-leaked"),
        );
        // The turn went terminal: capturing it must self-heal the leaked item.
        assert!(app.capture_completed_turn_activity(&session_id, &turn_id));

        let text = rendered_text(&app);
        assert!(
            !text.contains("Orchestrating"),
            "a leaked running item in a terminal turn must not pin Orchestrating: {text:?}"
        );
        assert!(
            !text.contains("1 active"),
            "the leaked item must not be counted as active once its turn is terminal: {text:?}"
        );
    }

    #[test]
    fn leaked_running_item_in_active_turn_still_shows_orchestrating() {
        // Guard against over-suppression: a "running" item whose turn IS the
        // session's currently-active turn (live_reply present) is genuine
        // in-flight work and MUST still read as Orchestrating. The self-heal
        // only fires when the turn is captured as terminal; an active turn's
        // live activity is never captured/reconciled.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the live job")],
                tasks: vec![],
                // Active turn: live_reply present and pointing at turn_id.
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-live"),
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Orchestrating"),
            "the active turn's in-flight work must still read as Orchestrating: {text:?}"
        );
    }

    #[test]
    fn subagents_attributed_per_turn_not_double_counted() {
        // C5 regression: two turns each spawn sub-agents. Before C1's `turn_id`
        // landed on the task wire, `running_subagent_titles_for_chip` returned the
        // GLOBAL active count for every chip matching active-OR-latest, so both
        // turns' chips lit up "Orchestrating" with the same total ("two chips").
        // Now each chip counts ONLY its own turn's running tasks; turn-less tasks
        // (server couldn't stamp them) fall back to a SINGLE current chip.
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let running = |title: &str, turn: Option<TurnId>| crate::model::TaskView {
            id: octos_core::TaskId::new(),
            title: title.into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: turn,
        };
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("two turns of agents")],
                tasks: vec![
                    running("a1", Some(turn_a.clone())),
                    running("b1", Some(turn_b.clone())),
                    running("b2", Some(turn_b.clone())),
                    // Turn-less (legacy / replay / synthetic) → single current chip.
                    running("orphan", None),
                ],
                // turn_a is the live/active turn.
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_a.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_b.clone(),
            request: Some("earlier turn".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn_b.clone())
                    .with_success(true),
            ],
        });

        // turn_a (active) chip: its own 1 running task + the orphan (None → the
        // single current chip, which is the active turn).
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_a)).len(),
            2
        );
        // turn_b chip: its own 2 running tasks — NOT the global 4, NOT the orphan.
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_b)).len(),
            2
        );
        // The pre-C5 bug would have returned the global active count (4) for BOTH.
        assert_ne!(
            running_subagent_titles_for_chip(&app, Some(&turn_a)).len(),
            running_subagent_titles_for_chip(&app, Some(&turn_b)).len() + 2,
            "chips must not both report the global total"
        );
    }

    #[test]
    fn turnless_tasks_fall_back_to_active_session_not_a_newer_other_session_log() {
        // codex P2: the None-fallback chip is "this session's latest turn", not
        // the globally-latest log. A *different* session having the newest
        // activity log must not steal the active session's turn-less task.
        let turn_active = TurnId::new();
        let turn_other = TurnId::new();
        let active_id = SessionKey("local:active".into());
        let other_id = SessionKey("local:other".into());
        let orphan = crate::model::TaskView {
            id: octos_core::TaskId::new(),
            title: "orphan".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: None,
        };
        // Active session (index 0) has the turn-less task but NO live_reply and
        // NO log; the other session owns the globally-newest log.
        let mut app = AppState::new(
            vec![
                SessionView {
                    id: active_id.clone(),
                    title: "active".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("active session")],
                    tasks: vec![orphan],
                    live_reply: None,
                },
                SessionView {
                    id: other_id.clone(),
                    title: "other".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("other session")],
                    tasks: vec![],
                    live_reply: None,
                },
            ],
            0,
            "ready".into(),
            None,
            false,
        );
        // Active session's log first, then a NEWER log for the OTHER session.
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: active_id,
            turn_id: turn_active.clone(),
            request: Some("active turn".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete").with_success(true),
            ],
        });
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: other_id,
            turn_id: turn_other.clone(),
            request: Some("other turn".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete").with_success(true),
            ],
        });

        // The orphan attaches to the active session's own latest turn…
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_active)).len(),
            1
        );
        // …and NOT to the other (globally-newest) session's turn.
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_other)).len(),
            0
        );
    }

    #[test]
    fn subagent_progress_folds_into_orchestrating_chip_not_a_second_chip() {
        // mini5 soak: a parallel-spawn turn rendered TWO "Orchestrating" chips —
        // the parent turn's chip (spawn calls + "N sub-agent(s) running") AND a
        // phantom turn-less chip made of the sub-agents' own progress rows. The
        // progress rows must fold into the parent chip as children → exactly ONE
        // orchestrating chip, with the sub-agents listed under it.
        let turn = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let running = |title: &str| crate::model::TaskView {
            id: octos_core::TaskId::new(),
            title: title.into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: Some(turn.clone()),
        };
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run parallel agents")],
                tasks: vec![
                    running("openclaw-deep-analysis"),
                    running("hermes-deep-analysis"),
                ],
                // Parent turn finished; its sub-agents keep running.
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Parent turn's spawn tool-calls, logged + completed.
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn.clone(),
            request: Some("run parallel agents".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn.clone())
                    .with_success(true),
            ],
        });
        // The sub-agents' own live progress rows (turn-less) — the phantom-chip
        // source. These must NOT form their own chip.
        app.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "openclaw-deep-analysis",
            "running",
        ));
        app.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "hermes-deep-analysis",
            "running",
        ));

        let text = rendered_text(&app);
        assert_eq!(
            text.matches("Orchestrating").count(),
            1,
            "exactly one Orchestrating chip (the phantom must fold in): {text:?}"
        );
        assert!(
            text.contains("2 sub-agent(s) running"),
            "the orchestrating chip surfaces the count: {text:?}"
        );
        assert!(
            text.contains("openclaw-deep-analysis") && text.contains("hermes-deep-analysis"),
            "the running sub-agents are folded in as children: {text:?}"
        );
    }

    #[test]
    fn subagent_progress_suppressed_only_when_a_matching_task_exists() {
        // codex P2: a turn-less running progress row is folded (suppressed from
        // the flow) ONLY if a running sub-agent task with the same title exists —
        // otherwise it has nothing to fold into and must stay visible, not vanish.
        let turn = TurnId::new();
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("x")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "alpha".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: Some(turn),
                }],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let matched = ActivityItem::new(ActivityKind::Progress, "alpha", "running");
        let orphan = ActivityItem::new(ActivityKind::Progress, "ghost", "running");
        assert!(
            is_subagent_progress(&app, &matched),
            "a progress row with a matching running task folds in → suppressed"
        );
        assert!(
            !is_subagent_progress(&app, &orphan),
            "a progress row with NO matching task must stay visible, not vanish"
        );
    }

    #[test]
    fn active_and_delivering_sub_agents_count_as_running() {
        // Regression: the server reports non-terminal task states beyond
        // running/queued (TaskRuntimeState::Active -> "active",
        // "delivering_outputs"). They must classify as running, else the
        // agent-task group title flips to "Agent task completed" while a
        // sub-agent is still working.
        for status in ["active", "delivering_outputs", "running", "queued", "42%"] {
            assert!(
                is_running_activity(&ActivityItem::new(ActivityKind::Tool, "spawn", status)),
                "status {status:?} should count as running"
            );
        }
        for status in [
            "completed",
            "complete",
            "done",
            "success",
            "failed",
            "error",
            "cancelled",
        ] {
            assert!(
                !is_running_activity(&ActivityItem::new(ActivityKind::Tool, "spawn", status)),
                "terminal status {status:?} should NOT count as running"
            );
        }

        // ...and the group title reflects it.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do multi-agent work")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("do multi-agent work".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "active").with_turn(turn_id.clone()),
                ActivityItem::new(ActivityKind::Tool, "deep_research", "delivering_outputs")
                    .with_turn(turn_id),
            ],
        });

        let text = rendered_text(&app);
        assert!(
            text.contains("Orchestrating..."),
            "active/delivering sub-agents must keep the running title: {text:?}"
        );
        assert!(
            !text.contains("Agent task completed"),
            "must NOT show completed while sub-agents are active/delivering"
        );
    }

    #[test]
    fn render_code_fences_show_language_and_bound_long_lines() {
        let palette = Palette::for_theme(ThemeName::Codex);
        let long_code = format!(
            "let value = \"{}TAIL_UNIQUE_SHOULD_NOT_RENDER\";",
            "x".repeat(180)
        );
        let content = format!("```rust\n{long_code}\n```");
        let mut lines = Vec::new();

        push_formatted_body(
            &mut lines,
            palette,
            &content,
            "",
            Some(palette.surface),
            120,
        );

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("┌─ "));
        assert!(text.contains("rust"));
        assert!(text.contains("└─"));
        assert!(!text.contains("end code"));
        assert!(text.contains("let value ="));
        assert!(text.contains(" ..."));
        assert!(!text.contains("TAIL_UNIQUE_SHOULD_NOT_RENDER"));
    }

    #[test]
    fn render_activity_uses_action_keywords_for_wait_and_file_tools() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show activity verbs")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("wait-1")
                .with_detail("sleep 20; tmux capture-pane")
                .with_success(true)
                .with_duration_ms(20_000),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "write_file", "complete")
                .with_tool_call("write-1")
                .with_detail("src/lib.rs")
                .with_success(true)
                .with_duration_ms(18),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(text.contains("⏺ Bash($ sleep 20"));
        assert!(text.contains("20s"));
        assert!(text.contains("⏺ Write(src/lib.rs"));
        assert!(text.contains("18ms"));
        assert!(!text.contains("Command  ▸ shell"));
        assert!(!text.contains("Tool  ▸ write_file"));
    }

    #[test]
    fn render_activity_shows_bash_command_not_raw_json_args() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run a bash command")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // The codex-style `bash` tool carries its command in `arguments.cmd`
        // (no `detail`), unlike `shell`/`exec` which set `detail`. It must
        // still render as a real `$ command` line, not the raw JSON blob.
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_tool_call("bash-1")
                .with_arguments(serde_json::json!({
                    "cmd": "find . -name '*.ts' -newer server"
                }))
                .with_success(true)
                .with_duration_ms(8),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        // Claude-Code-style card: `⏺ Bash(cmd)`, clean command, no JSON.
        assert!(
            text.contains("⏺ Bash($ find . -name '*.ts' -newer server)"),
            "want Claude-Code-style bash card, got:\n{text}"
        );
        assert!(
            !text.contains("call bash-1"),
            "must not show the call id, got:\n{text}"
        );
        // No raw JSON arguments leaking through.
        assert!(
            !text.contains("{\"cmd\""),
            "must not show raw JSON args, got:\n{text}"
        );
    }

    #[test]
    fn render_spawn_and_multiline_tool_cards_claude_code_style() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("spawn + multiline")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // spawn's task (projected into `detail`) renders as `⏺ Spawn(task)`.
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                .with_tool_call("spawn-1")
                .with_detail("Restart the Vite dev server")
                .with_success(true)
                .with_duration_ms(2500),
        );
        // A multi-line command keeps both lines (indented under `(`).
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_tool_call("bash-2")
                .with_detail("cd /srv\nnpm run dev")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(
            text.contains("⏺ Spawn(Restart the Vite dev server)"),
            "spawn must show its task, got:\n{text}"
        );
        assert!(text.contains("⏺ Bash($ cd /srv"), "got:\n{text}");
        assert!(
            text.contains("npm run dev)"),
            "multi-line command must keep its second line, got:\n{text}"
        );
        assert!(
            !text.contains("spawn-1") && !text.contains("bash-2"),
            "must not show call ids, got:\n{text}"
        );
    }

    #[test]
    fn compaction_notice_renders_prominently_with_marker() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id,
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // The persistent "context compacted" notice must stand out from the
        // muted activity stream so it isn't lost in a busy session.
        app.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            t!("status.activity_context_compacted").into_owned(),
            "120k → 40k tokens",
        ));
        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);
        assert!(
            text.contains("✦ context compacted"),
            "compaction notice must render with a prominent marker, got:\n{text}"
        );
        assert!(text.contains("120k → 40k tokens"), "got:\n{text}");
    }

    #[test]
    fn compaction_completed_event_renders_prominent_notice_end_to_end() {
        use octos_core::app_ui::AppUiEvent;
        use octos_core::ui_protocol::{
            ContextCompactionCompletedEvent, UiContextCompactionRecord, UiContextState,
            UiNotification,
        };
        let session_id = SessionKey("local:test".into());
        // Compaction is reported DURING a turn — give the session a live reply
        // so the notice is turn-stamped (else it is suppressed mid-turn).
        let turn_id = TurnId::new();
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: session_id.clone(),
                    title: "test".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("do heavy work")],
                    tasks: vec![],
                    live_reply: Some(crate::model::LiveReply {
                        turn_id,
                        text: String::new(),
                    }),
                }],
                0,
                "ready".into(),
                None,
                false,
            ),
        };

        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextCompactionCompleted(ContextCompactionCompletedEvent {
                session_id: session_id.clone(),
                context_state: UiContextState {
                    session_id: session_id.clone(),
                    thread_id: None,
                    generation: 4,
                    transcript_hash: "abc123".into(),
                    item_count: 42,
                    token_estimate: 40_000,
                    recovery_state: "healthy".into(),
                    last_checkpoint_id: None,
                    last_compaction_id: Some("comp-001".into()),
                },
                compaction: UiContextCompactionRecord {
                    compaction_id: "comp-001".into(),
                    checkpoint_id: "chk-001".into(),
                    status: "applied".into(),
                    policy_id: "default".into(),
                    trigger: "token_budget".into(),
                    input_generation: 3,
                    output_generation: Some(4),
                    input_transcript_hash: "input-h".into(),
                    replacement_transcript_hash: Some("abc123".into()),
                    installed_transcript_hash: Some("abc123".into()),
                    input_item_count: 130,
                    retained_count: 42,
                    dropped_count: 88,
                    summary_item_id: Some("sum-1".into()),
                    token_estimate_before: 120_000,
                    token_estimate_after: Some(40_000),
                    error: None,
                },
            }),
        ));

        store.state.expanded_tool_outputs = true;
        let text = rendered_text(&store.state);
        // Full path: Completed event → persistent notice → prominent ✦ render.
        assert!(
            text.contains("✦ context compacted"),
            "a real compaction Completed event must render the prominent notice, got:\n{text}"
        );
    }

    #[test]
    fn render_file_mutation_progress_as_separate_activity_block() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show file mutation")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(
                ActivityKind::Progress,
                "file_mutation",
                "File mutation: modify /tmp/work/blue-origin/src/pages/index.astro",
            )
            .with_detail("modify /tmp/work/blue-origin/src/pages/index.astro | diff preview ready"),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(text.contains("Changed"));
        assert!(text.contains(".../blue-origin/src/pages/index.astro"));
        assert!(!text.contains("preview ready"));
        assert!(!text.contains("File mutation: modify /tmp/work"));
    }

    #[test]
    fn render_short_terminal_keeps_user_prompt_visible_above_activity() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("keep this prompt visible")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        for idx in 0..8 {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_detail(format!("Hydrating context {idx}"))
                    .with_output_preview("1 | pub fn demo() {}")
                    .with_success(true)
                    .with_duration_ms(420),
            );
        }

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("keep this prompt visible"));
        assert!(text.contains("Composer"));
    }

    #[test]
    fn render_transcript_scroll_bottom_counts_wrapped_rows_above_composer() {
        let long_body = (1..=18)
            .map(|idx| {
                format!(
                    "wrapped paragraph {idx:02} {}",
                    "中文内容 mixed ascii text ".repeat(5)
                )
            })
            .chain(std::iter::once(
                "final wrapped row should remain visible BOTTOM_VISIBLE_UNIQUE".to_string(),
            ))
            .collect::<Vec<_>>()
            .join("\n\n");
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("show long answer"),
                    Message::assistant(long_body),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 56, 20);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let rows = rendered_rows(&buffer);
        assert!(text.contains("BOTTOMVISIBLEUNIQUE"));
        assert!(text.contains("Composer"));
        assert!(!text.contains("Work  sticky"));
        let final_row = row_index_containing(&rows, "BOTTOMVISIBLEUNIQUE");
        let composer_row = row_index_containing(&rows, "Composer");
        assert!(
            final_row < composer_row,
            "final transcript row must stay above composer: final={final_row}, composer={composer_row}"
        );
    }

    #[test]
    fn render_long_active_turn_follows_tail_when_prompt_block_overflows() {
        let turn_id = TurnId::new();
        let live_reply = (1..=16)
            .map(|idx| format!("live answer row {idx:02} {}", "wrapped content ".repeat(4)))
            .chain(std::iter::once(
                "LIVETAILVISIBLEUNIQUE should stay visible above composer".to_string(),
            ))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("done?")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: live_reply,
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 80, 24);
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert!(text.contains("LIVETAILVISIBLEUNIQUE"));
        assert!(text.contains("Composer"));
        let tail_row = row_index_containing(&rows, "LIVETAILVISIBLEUNIQUE");
        let composer_row = row_index_containing(&rows, "Composer");
        assert!(
            tail_row < composer_row,
            "active turn tail must stay above composer: tail={tail_row}, composer={composer_row}"
        );
    }

    #[test]
    fn render_active_turn_answer_precedes_progress_and_hides_stale_activity() {
        let old_turn_id = TurnId::new();
        let current_turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build the site"),
                    Message::assistant("Started the site build."),
                    Message::user("done?"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: current_turn_id,
                    text: "Not yet - the build is still running.".into(),
                }),
            }],
            0,
            "thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(old_turn_id)
                .with_detail("cargo build from prior turn")
                .with_success(true),
        );

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 80, 24);
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert!(text.contains("done?"));
        assert!(text.contains("Not yet - the build is still running."));
        assert!(
            !text.contains("cargo build from prior turn"),
            "prior-turn activity must not render under the latest user prompt"
        );
        assert!(
            !rows
                .iter()
                .any(|row| matches!(row.trim(), "◐" | "◓" | "◑" | "◒")),
            "live assistant text must not render a second standalone spinner row"
        );
        let prompt_row = row_index_containing(&rows, "done?");
        let answer_row = row_index_containing(&rows, "Not yet - the build is still running.");
        let composer_row = row_index_containing(&rows, "Composer");
        assert!(
            prompt_row < answer_row && answer_row < composer_row,
            "latest prompt should be followed by live answer before composer: prompt={prompt_row}, answer={answer_row}, composer={composer_row}"
        );
    }

    #[test]
    fn render_live_answer_activity_without_sticky_round_plan() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("review the project")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "The project review found two issues.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_turn(turn_id)
                .with_tool_call("read-1")
                .with_detail("src/main.rs")
                .with_success(true),
        );

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 96, 28);
        let rows = rendered_rows(&buffer);
        let answer_row = row_index_containing(&rows, "The project review found two issues.");
        let activity_row = row_index_containing(&rows, "Agent task completed");

        assert!(
            answer_row < activity_row,
            "live answer should be followed by its activity log: answer={answer_row}, activity={activity_row}"
        );
        let text = rows.join("\n");
        assert!(!text.contains("Plan rounds"));
        assert!(!text.contains("Current round: review the project"));
        assert!(!text.contains("Work  sticky"));
    }

    #[test]
    fn render_tool_blocks_show_state_preview_failure_and_collapsed_detail() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show tool states")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("preview-1")
                .with_detail("cargo test")
                .with_output_preview("6 passed")
                .with_success(true)
                .with_duration_ms(1200),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "failed")
                .with_tool_call("fail-1")
                .with_detail("npm install")
                .with_success(false)
                .with_duration_ms(70_000),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_tool_call("collapsed-1")
                .with_detail("src/lib.rs")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(text.contains("✗"));
        assert!(text.contains("⏺"));
        assert!(text.contains("70s"));
        assert!(text.contains("6 passed"));
    }

    #[test]
    fn render_tool_output_expands_with_global_toggle_state() {
        let output = (1..=10)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show output")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("preview-1")
                .with_detail("cargo test")
                .with_output_preview(output)
                .with_success(true),
        );

        let collapsed = rendered_text(&app);
        // New contract: a settled group collapses to its one-line header — no
        // child rows, no per-tool preview hint, until Ctrl+O expands.
        assert!(!collapsed.contains("line10"));
        assert!(!collapsed.contains("cargo test"));
        assert!(collapsed.contains("(1"));

        app.expanded_tool_outputs = true;
        let expanded = rendered_text(&app);
        assert!(expanded.contains("line10"));
        assert!(expanded.contains("expanded (Ctrl+O collapse)"));
    }

    #[test]
    fn render_expanded_tool_output_remains_bounded() {
        let output = (1..=40)
            .map(|line| format!("output-line-{line:02}-unique"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show bounded output")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.expanded_tool_outputs = true;
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_detail("cargo test -- --nocapture")
                .with_output_preview(output)
                .with_success(true),
        );

        let text = rendered_text(&app);

        assert!(text.contains("output-line-24-unique"));
        assert!(!text.contains("output-line-40-unique"));
        assert!(text.contains("16 more line(s) hidden (Ctrl+O collapse)"));
    }

    #[test]
    fn render_active_turn_progress_uses_spinner_without_logs_or_timestamps() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("think")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: String::new(),
                }),
            }],
            0,
            "Queued turn/start".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("INFO "));
        assert!(!text.contains("2026-"));
        assert!(!text.contains("tool_ids="));
    }

    #[test]
    fn render_inline_approval_card_names_request_and_session_actions() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::system("ready"),
                    Message::user("complete m9 contract"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run command".into(),
            body: "cargo test".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("complete m9 contract"));
        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Run command"));
        assert!(text.contains("shell"));
        assert!(text.contains("y = approve this command once"));
        assert!(text.contains("s = approve this command/scope for the session"));
        assert!(text.contains("n = deny it"));
    }

    #[test]
    fn render_blocked_turn_keeps_latest_user_prompt_visible_near_approval() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("older prompt"),
                    Message::assistant("older answer"),
                    Message::user("complete m9 contract"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Planning a safe M9 scaffold over mock transport.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        for idx in 0..8 {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_detail(format!("Hydrating prototype context {idx}"))
                    .with_output_preview("1 | pub fn demo() {}")
                    .with_success(true)
                    .with_duration_ms(420),
            );
        }
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Mock approval boundary".into(),
            body: "approve?".into(),
            approval_kind: Some("command".into()),
            risk: Some("low".into()),
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("complete m9 contract"));
        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Mock approval boundary"));
    }

    #[test]
    fn render_compact_blocked_turn_keeps_latest_user_prompt_visible() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("older prompt"),
                    Message::assistant("older answer"),
                    Message::user("complete m9 contract"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Planning a safe M9 scaffold over mock transport.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        for idx in 0..8 {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_detail(format!("Hydrating prototype context {idx}"))
                    .with_output_preview("1 | pub fn demo() {}")
                    .with_success(true)
                    .with_duration_ms(420),
            );
        }
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Mock approval boundary".into(),
            body: "approve?".into(),
            approval_kind: Some("command".into()),
            risk: Some("low".into()),
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("complete m9 contract"));
        assert!(text.contains("Mock approval boundary"));
    }

    #[test]
    fn render_diff_preview_modal_includes_status_files_and_hunks() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Roman numeral patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/roman.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            DiffPreviewLine {
                                kind: "removed".into(),
                                content: "todo!()".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "Ok(42)".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                        ],
                    }],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(text.contains("Diff Preview"));
        assert!(text.contains("Roman numeral patch"));
        assert!(text.contains("ready"));
        assert!(text.contains("pending_store"));
        assert!(text.contains("modified"));
        assert!(text.contains("src/roman.rs"));
        assert!(text.contains("@@ -1 +1 @@"));
        assert!(text.contains("todo!()"));
        assert!(text.contains("Ok(42)"));
    }

    #[test]
    fn ctrl_o_expands_diff_preview_to_full_selected_hunk() {
        // The collapsed inline diff caps each hunk at 4 lines — the "Tab doesn't
        // expand the diff" complaint. Ctrl+O (expanded_tool_outputs) must reveal
        // the SELECTED hunk's complete body, and the hidden-lines hint must
        // point at that working key (was a misleading "(Tab inspector)").
        let make = || DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Big patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/big.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1,6 +1,6 @@".into(),
                        lines: (1u32..=6)
                            .map(|n| DiffPreviewLine {
                                kind: "added".into(),
                                content: format!("line {n} content"),
                                old_line: None,
                                new_line: Some(n),
                            })
                            .collect(),
                    }],
                }],
            },
        };

        // Collapsed (default): capped at 4 lines, hint points to Ctrl+O.
        let collapsed = rendered_text(&app_with_diff(make()));
        assert!(collapsed.contains("line 4 content"));
        assert!(
            !collapsed.contains("line 5 content"),
            "5th line hidden when collapsed: {collapsed:?}"
        );
        assert!(
            collapsed.contains("Ctrl+O expand"),
            "hidden-lines hint must point at the working key: {collapsed:?}"
        );

        // Expanded (Ctrl+O): full selected hunk, no truncation hint.
        let mut app = app_with_diff(make());
        app.expanded_tool_outputs = true;
        let expanded = rendered_text(&app);
        assert!(
            expanded.contains("line 5 content") && expanded.contains("line 6 content"),
            "all lines of the selected hunk shown when expanded: {expanded:?}"
        );
        assert!(
            !expanded.contains("more diff line(s) hidden"),
            "no truncation hint when expanded: {expanded:?}"
        );
    }

    #[test]
    fn diff_box_hidden_when_no_usable_hunks() {
        // C6 (mini5 soak): an auto-opened preview whose file carries no hunks
        // ("line diff unavailable for this mutation") must hide the whole box —
        // no "Diff Preview" header, no dead "[/] select hunk | c stage" UI.
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Empty mutation".into()),
                files: vec![DiffPreviewFile {
                    path: "src/empty.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(
            !text.contains("Diff Preview"),
            "diff box must be hidden when no usable hunks: {text:?}"
        );
        assert!(
            !text.contains("select hunk"),
            "dead hunk-select UI must not render: {text:?}"
        );
        assert!(!text.contains("line diff unavailable"));
    }

    #[test]
    fn render_inline_diff_uses_codex_style_add_delete_colors() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Color patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/color.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            DiffPreviewLine {
                                kind: "removed".into(),
                                content: "old_value()".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "new_value()".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                        ],
                    }],
                }],
            },
        });
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);

        let removed_style = style_for_text(&buffer, "old_value()").expect("removed line style");
        let added_style = style_for_text(&buffer, "new_value()").expect("added line style");
        let hunk_style = style_for_text(&buffer, "@@ -1 +1 @@").expect("hunk style");

        assert_eq!(removed_style.fg, Some(palette.danger));
        assert_eq!(removed_style.bg, Some(palette.danger_bg));
        assert_eq!(added_style.fg, Some(palette.success));
        assert_eq!(added_style.bg, Some(palette.success_bg));
        assert_eq!(hunk_style.fg, Some(palette.accent));
        assert_eq!(hunk_style.bg, Some(palette.diff_context_bg));
        assert!(
            inline_diff_marker_style_for_test("added", palette)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(
            inline_diff_style_for_test("removed", palette).bg,
            Some(palette.danger_bg)
        );
    }

    #[test]
    fn render_inline_diff_header_shows_file_badge_and_counts() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Header patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            DiffPreviewLine {
                                kind: "removed".into(),
                                content: "old_value()".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "new_value()".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "another_value()".into(),
                                old_line: None,
                                new_line: Some(2),
                            },
                        ],
                    }],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(text.contains("RUST"));
        assert!(text.contains("modified"));
        assert!(text.contains("+2"));
        assert!(text.contains("-1"));
        assert!(text.contains("src/lib.rs"));
    }

    #[test]
    fn render_inline_diff_shows_selected_hunk_not_always_first() {
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Selected hunk patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![
                        DiffPreviewHunk {
                            header: "@@ -1 +1 @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "added".into(),
                                content: "first_change()".into(),
                                old_line: None,
                                new_line: Some(1),
                            }],
                        },
                        DiffPreviewHunk {
                            header: "@@ -20 +20 @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "added".into(),
                                content: "second_change()".into(),
                                old_line: None,
                                new_line: Some(20),
                            }],
                        },
                    ],
                }],
            },
        });
        app.diff_preview.selected_hunk = 1;

        let text = rendered_text(&app);

        assert!(text.contains("@@ -20 +20 @@"));
        assert!(text.contains("second_change()"));
        assert!(!text.contains("first_change()"));
    }

    #[test]
    fn diff_preview_result_selects_first_changed_hunk() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Default hunk patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![
                        DiffPreviewHunk {
                            header: "@@ metadata @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "context".into(),
                                content: "unchanged metadata".into(),
                                old_line: Some(1),
                                new_line: Some(1),
                            }],
                        },
                        DiffPreviewHunk {
                            header: "@@ -20 +20 @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "added".into(),
                                content: "first_real_change()".into(),
                                old_line: None,
                                new_line: Some(20),
                            }],
                        },
                    ],
                }],
            },
        });

        assert_eq!(app.diff_preview.selected_file, 0);
        assert_eq!(app.diff_preview.selected_hunk, 1);
    }

    #[test]
    fn render_inline_diff_and_approval_share_chat_flow() {
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Visible patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "added".into(),
                            content: "new line".into(),
                            old_line: None,
                            new_line: Some(1),
                        }],
                    }],
                }],
            },
        });
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "diff_edit".into(),
            title: "Approval should be behind diff".into(),
            body: "approve?".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("Diff Preview"));
        assert!(text.contains("Visible patch"));
        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Approval should be behind diff"));
        assert!(text.contains("y = approve this command once"));
    }

    #[test]
    fn render_short_terminal_keeps_user_prompt_visible_above_inline_diff() {
        let diff_lines = (1..=10)
            .map(|line| DiffPreviewLine {
                kind: "added".into(),
                content: format!("generated line {line}"),
                old_line: None,
                new_line: Some(line),
            })
            .collect::<Vec<_>>();
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Large patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/generated.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +10 @@".into(),
                        lines: diff_lines,
                    }],
                }],
            },
        });
        app.sessions[0].messages = vec![Message::user("fix visible prompt")];

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("fix visible prompt"));
        assert!(text.contains("Diff Preview"));
        assert!(text.contains("6 more diff line(s) hidden"));
        assert!(text.contains("Composer"));
        assert!(text.contains("Idle"));
    }

    #[test]
    fn render_diff_preview_modal_keeps_unknown_future_labels_visible() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "requires_refresh".into(),
            source: "future_cache".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Future diff".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: Some("src/old.rs".into()),
                    status: "copied".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ metadata @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "metadata".into(),
                            content: "mode change".into(),
                            old_line: None,
                            new_line: None,
                        }],
                    }],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(text.contains("requires_refresh"));
        assert!(text.contains("future_cache"));
        assert!(text.contains("copied"));
        assert!(text.contains("src/old.rs -> src/lib.rs"));
        assert!(text.contains("mode change"));
    }

    // M15-E follow-up: sticky goal/loop indicator above the composer.
    // See the M9/M15 audit gap — `SessionAutonomyState` was populated
    // by notification mirrors but never surfaced unless the user typed
    // `/goal` or `/loop list`.

    fn autonomy_app_state() -> AppState {
        AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        )
    }

    fn sample_loop(
        loop_id: &str,
        prompt: &str,
        mode: &str,
        secs: Option<u64>,
    ) -> octos_core::ui_protocol::UiLoopRecord {
        octos_core::ui_protocol::UiLoopRecord {
            loop_id: loop_id.into(),
            session_id: SessionKey("local:test".into()),
            profile_id: None,
            prompt: prompt.into(),
            mode: mode.into(),
            interval_seconds: secs,
            status: "active".into(),
            next_run_at_ms: None,
            last_run_at_ms: None,
            expires_at_ms: 999,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn render_autonomy_indicator_idle_reserves_no_rows() {
        let app = autonomy_app_state();
        assert_eq!(autonomy_indicator_height(&app), 0);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
        assert!(
            lines.is_empty(),
            "idle state should produce no indicator rows"
        );

        let text = rendered_text(&app);
        assert!(
            !text.contains("Goal:"),
            "idle render must not surface a goal label",
        );
        assert!(
            !text.contains("Loops:"),
            "idle render must not surface a loop label",
        );
    }

    #[test]
    fn plan_indicator_renders_checklist_tree_with_glyphs() {
        use octos_core::ui_protocol::{PlanItemStatus, UiPlanItem, UiPlanRecord};
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.set_session_plan(
            &session_id,
            Some(UiPlanRecord {
                title: Some("Building memory panel".into()),
                updated_at_ms: 0,
                items: vec![
                    UiPlanItem {
                        id: "1".into(),
                        title: "PWA manifest".into(),
                        status: PlanItemStatus::Completed,
                        priority: None,
                    },
                    UiPlanItem {
                        id: "2".into(),
                        title: "memory panel".into(),
                        status: PlanItemStatus::InProgress,
                        priority: Some("P3".into()),
                    },
                    UiPlanItem {
                        id: "3".into(),
                        title: "cron toggle".into(),
                        status: PlanItemStatus::Pending,
                        priority: None,
                    },
                ],
            }),
            None,
        );

        // header + 3 item rows, no goal/loops.
        assert_eq!(autonomy_indicator_height(&app), 4);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
        assert_eq!(lines.len(), 4);

        let text = rendered_text(&app);
        assert!(
            text.contains("Building memory panel"),
            "header activity title"
        );
        assert!(text.contains("(1/3)"), "done/total counter");
        assert!(text.contains('⎿'), "tree anchor glyph");
        assert!(text.contains('✔'), "completed glyph");
        assert!(text.contains('◼'), "pending glyph");
        assert!(text.contains("PWA manifest"));
        assert!(text.contains("P3"), "priority chip on the in-progress item");
    }

    #[test]
    fn plan_cleared_only_when_its_authoring_turn_completes() {
        use octos_core::ui_protocol::{PlanItemStatus, UiPlanItem, UiPlanRecord};
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        let turn = TurnId::new();
        let other_turn = TurnId::new();
        let plan = Some(UiPlanRecord {
            title: Some("plan".into()),
            updated_at_ms: 0,
            items: vec![UiPlanItem {
                id: "1".into(),
                title: "do it".into(),
                status: PlanItemStatus::InProgress,
                priority: None,
            }],
        });
        app.set_session_plan(&session_id, plan, Some(turn.clone()));
        assert_eq!(autonomy_indicator_height(&app), 2);

        // A completion for a DIFFERENT turn must not clear the panel.
        app.clear_session_plan_for_turn(&session_id, &other_turn);
        assert_eq!(autonomy_indicator_height(&app), 2);

        // The authoring turn's completion clears it.
        app.clear_session_plan_for_turn(&session_id, &turn);
        assert_eq!(autonomy_indicator_height(&app), 0);
    }

    #[test]
    fn plan_indicator_truncates_long_checklist() {
        use octos_core::ui_protocol::{PlanItemStatus, UiPlanItem, UiPlanRecord};
        let mut app = autonomy_app_state();
        let items: Vec<_> = (0..12)
            .map(|i| UiPlanItem {
                id: i.to_string(),
                title: format!("item {i}"),
                status: PlanItemStatus::Pending,
                priority: None,
            })
            .collect();
        app.set_session_plan(
            &SessionKey("local:test".into()),
            Some(UiPlanRecord {
                title: Some("big plan".into()),
                updated_at_ms: 0,
                items,
            }),
            None,
        );
        // header + 8 shown + 1 overflow line.
        assert_eq!(autonomy_indicator_height(&app), 10);
        assert!(rendered_text(&app).contains("+4 more"));
    }

    #[test]
    fn render_autonomy_indicator_goal_only_renders_one_row() {
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: "finish the OAuth refactor".into(),
                status: "active".into(),
                token_budget: 50_000,
                tokens_used: 12_000,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );

        assert_eq!(autonomy_indicator_height(&app), 1);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
        assert_eq!(lines.len(), 1);

        let text = rendered_text(&app);
        assert!(
            text.contains("Goal:"),
            "goal row must surface 'Goal:' label"
        );
        assert!(text.contains("finish the OAuth refactor"));
        assert!(text.contains("active"));
        assert!(text.contains("12000/50000"));
        assert!(!text.contains("Loops:"), "loops row must be hidden");
    }

    #[test]
    fn render_autonomy_indicator_goal_and_loops_render_two_rows() {
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: "finish OAuth refactor".into(),
                status: "active".into(),
                token_budget: 50_000,
                tokens_used: 12_000,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        app.set_session_loops(
            &session_id,
            vec![
                sample_loop("l1", "deploy-check", "fixed_interval", Some(300)),
                sample_loop("l2", "PR-watch", "self_paced", None),
            ],
        );

        assert_eq!(autonomy_indicator_height(&app), 2);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
        assert_eq!(lines.len(), 2);

        let text = rendered_text(&app);
        assert!(text.contains("Goal:"));
        assert!(text.contains("finish OAuth refactor"));
        assert!(text.contains("Loops: 2 active"));
        assert!(text.contains("5m deploy-check"));
        assert!(text.contains("self-paced PR-watch"));
    }

    #[test]
    fn autonomy_indicator_counts_paused_loops_alongside_active() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // A `--solo` boot parks active loops to `paused`: the header must read
        // "0 active · 2 paused", not "0 running" beside two visible chips.
        let mut l1 = sample_loop("l1", "deploy-check", "fixed_interval", Some(300));
        l1.status = "paused".into();
        let mut l2 = sample_loop("l2", "PR-watch", "self_paced", None);
        l2.status = "paused".into();
        app.set_session_loops(&session_id, vec![l1, l2]);

        let text = rendered_text(&app);
        assert!(
            text.contains("Loops: 0 active · 2 paused"),
            "paused loops must reconcile with the chips, got:\n{text}"
        );
    }

    #[test]
    fn harness_context_ratio_uses_real_window_when_known() {
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 64_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        // No known window yet → fall back to the fixed default (64000/128000).
        assert_eq!(harness_context_ratio(&app), Some(0.5));

        // Once the real per-model window arrives on the wire (here 256k), the
        // SAME token estimate is honestly a quarter full — not a misleading 50%.
        app.session_context_window
            .insert(session_id.clone(), 256_000);
        assert_eq!(harness_context_ratio(&app), Some(0.25));

        // A tiny window clamps to a full gauge rather than overflowing.
        app.session_context_window.insert(session_id.clone(), 1_000);
        assert_eq!(harness_context_ratio(&app), Some(1.0));
    }

    #[test]
    fn harness_status_row_surfaces_orchestration_usage_and_context() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();

        // Idle: no orchestration, no active turn → row reserves no rows and is
        // absent from the render (so it cannot collide with the composer).
        assert_eq!(harness_status_height(&app), 0);
        assert!(harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true).is_empty());

        // Orchestrating: active, 2 running agents, 1 pending continuation.
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 2,
                pending_continuations: 1,
                phase: Some("orchestrating".into()),
            },
        );
        app.session_usage
            .insert(session_id.clone(), (Some(34_211), Some(374), Some(0.0123)));
        // Context usage (token_estimate) is inspector-only today — surface it
        // as ctx N% in the harness row.
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 64_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        assert_eq!(
            harness_status_height(&app),
            1,
            "active row reserves one row"
        );
        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(text.contains("Orchestrating"), "{text:?}");
        assert!(text.contains("2 agents"), "{text:?}");
        assert!(text.contains("re-entering"), "{text:?}");
        assert!(text.contains("↑34.2k"), "{text:?}");
        assert!(text.contains("↓374"), "{text:?}");
        assert!(text.contains("$0.0123"), "{text:?}");
        assert!(
            text.contains("ctx ~50%"),
            "ctx % from token_estimate (approximate marker): {text:?}"
        );
        // Context ratio drives the LineGauge (64000 / 128000 = 0.5).
        assert_eq!(harness_context_ratio(&app), Some(0.5));

        // Even with the row ACTIVE the composer's top-border chrome survives —
        // the indicator lives on its own dedicated layout row, not the border
        // (the collision that caused the 249fe652 revert cannot recur).
        let rendered = rendered_text(&app);
        assert!(
            rendered.contains("Orchestrating"),
            "active row renders: {rendered:?}"
        );
        assert!(
            rendered.contains("Composer"),
            "composer chrome intact: {rendered:?}"
        );
        assert!(
            rendered.contains("Tab inspector"),
            "composer hint not clobbered: {rendered:?}"
        );
        // Regression (duplicate ctx%): on a wide terminal (rendered_text uses
        // 120 cols, so the gauge column is drawn) the percent must render ONCE —
        // as the LineGauge on the right, NOT also as the textual `· ctx ~N%`
        // label on the left. Pre-fix this row showed both "· ctx ~50%" and
        // "ctx ~50% ───" on the same line.
        assert_eq!(
            rendered.matches("ctx ~").count(),
            1,
            "ctx% must render exactly once (gauge only) on a wide terminal: {rendered:?}"
        );
    }

    #[test]
    fn harness_status_row_ctx_label_marks_estimate() {
        // Nit: ctx% uses a fixed DEFAULT_CONTEXT_WINDOW_TOKENS denominator (no
        // per-model window on the wire), so the label must read as an ESTIMATE
        // (`ctx ~N%`) rather than an exact figure that would mislead when the
        // real model window differs.
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 0,
                phase: Some("working".into()),
            },
        );
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 32_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(
            text.contains("ctx ~25%"),
            "ctx label must carry the approximate marker: {text:?}"
        );
    }

    #[test]
    fn harness_status_row_surfaces_retry_state() {
        use octos_core::ui_protocol::{SessionOrchestrationEvent, UiRetryBackoff};
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 0,
                phase: Some("working".into()),
            },
        );
        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(3);
        app.session_retry.insert(session_id, retry);

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(
            text.to_lowercase().contains("retry") || text.to_lowercase().contains("retrying"),
            "retry state must render in the harness row: {text:?}"
        );
        assert!(
            text.contains('3'),
            "retry attempt number must render: {text:?}"
        );
    }

    #[test]
    fn harness_status_row_does_not_collide_with_composer_when_idle() {
        // Idle render: the dedicated harness row takes height 0, so the
        // composer's top-border chrome ("Composer  Enter send | Tab inspector")
        // is fully intact — the collision that caused the prior revert
        // (249fe652) cannot recur because the indicator is never on the border.
        let app = autonomy_app_state();
        assert_eq!(harness_status_height(&app), 0);
        let text = rendered_text(&app);
        assert!(text.contains("Composer"), "{text:?}");
        assert!(text.contains("Tab inspector"), "{text:?}");
        assert!(
            !text.contains("Orchestrating"),
            "idle harness row must be absent: {text:?}"
        );
    }

    #[test]
    fn autonomy_loop_label_truncates_long_prompt_with_ellipsis() {
        let long = octos_core::ui_protocol::UiLoopRecord {
            loop_id: "l1".into(),
            session_id: SessionKey("local:test".into()),
            profile_id: None,
            prompt: "this prompt is intentionally far too long to fit in a chip".into(),
            mode: "self_paced".into(),
            interval_seconds: None,
            status: "active".into(),
            next_run_at_ms: None,
            last_run_at_ms: None,
            expires_at_ms: 999,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let label = autonomy_loop_label(&long);
        assert!(
            label.chars().count() <= AUTONOMY_LOOP_LABEL_MAX,
            "label {label:?} should respect AUTONOMY_LOOP_LABEL_MAX",
        );
        assert!(label.ends_with('…'));
    }

    // ---- inline-viewport (scrollback) rendering ----

    fn chat_app(messages: Vec<Message>) -> AppState {
        AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages,
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        )
    }

    fn app_with_large_menu() -> AppState {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.menu_stack.open("geometry.test");
        let items = (0..20)
            .map(|idx| {
                crate::menu::MenuItem::new(
                    format!("geometry.item.{idx}"),
                    format!("Geometry item {idx}"),
                    crate::menu::MenuAction::Noop,
                )
            })
            .collect();
        app.active_menu = Some(crate::menu::MenuBuildResult::ready(
            crate::menu::MenuSpec::new(
                "geometry.test",
                "Geometry test",
                crate::menu::MenuMode::SingleSelect,
            )
            .with_items(items),
        ));
        app
    }

    #[test]
    fn chat_layout_areas_keep_composer_and_status_at_bottom() {
        let app = chat_app(vec![Message::user("hi"), Message::assistant("ready")]);
        let area = Rect::new(0, 0, 80, 24);

        let layout = chat_layout_areas(&app, area);

        assert_eq!(layout.status.y, area.y + area.height - 1);
        assert_eq!(layout.status.height, 1);
        assert_eq!(
            layout.composer.y + layout.composer.height,
            layout.status.y,
            "composer must sit immediately above the status row"
        );
        assert_eq!(layout.transcript.y, area.y);
        assert!(
            layout.transcript.y + layout.transcript.height <= layout.menu.y,
            "transcript and menu areas must not overlap"
        );
    }

    #[test]
    fn chat_layout_areas_clamp_menu_to_transcript_budget() {
        let app = app_with_large_menu();
        let area = Rect::new(0, 0, 80, 19);

        let layout = chat_layout_areas(&app, area);

        assert_eq!(
            layout.menu.height, 4,
            "large menus are clamped by the available surface budget"
        );
        assert!(
            layout.transcript.height >= min_transcript_height(area.height),
            "menu must not steal the transcript's minimum height"
        );
        assert_eq!(layout.status.y, area.y + area.height - 1);
        assert_eq!(layout.composer.y + layout.composer.height, layout.status.y);
    }

    #[test]
    fn render_chat_layout_matches_chat_layout_areas() {
        let mut app = chat_app(vec![
            Message::user("ask number 01"),
            Message::assistant("history message 01"),
        ]);
        app.transcript_pager_active = true;
        let area = Rect::new(0, 0, 80, 20);
        let layout = chat_layout_areas(&app, area);

        let buffer = rendered_buffer_with_size(
            &app,
            Palette::for_theme(ThemeName::default()),
            area.width,
            area.height,
        );
        let rows = rendered_rows(&buffer);
        let composer_row = row_index_containing(&rows, "Composer") as u16;
        assert!(
            composer_row >= layout.composer.y
                && composer_row < layout.composer.y + layout.composer.height,
            "composer title row {composer_row} must be inside {:?}",
            layout.composer
        );
        for y in layout.composer.y..layout.composer.y + layout.composer.height {
            assert!(
                !rows[usize::from(y)].contains("history message"),
                "transcript text must not render inside composer area at row {y}: {:?}",
                rows[usize::from(y)]
            );
        }
    }

    #[test]
    fn scrollbar_thumb_hidden_without_overflow() {
        let track = Rect::new(79, 0, 1, 10);
        let metrics = TranscriptScrollMetrics {
            visible_rows: 20,
            total_rows: 20,
            scroll_from_bottom: 0,
            max_scroll_from_bottom: 0,
        };

        assert_eq!(scrollbar_thumb(metrics, track), None);
    }

    #[test]
    fn scrollbar_thumb_places_bottom_at_track_end() {
        let track = Rect::new(79, 5, 1, 10);
        let metrics = TranscriptScrollMetrics {
            visible_rows: 20,
            total_rows: 100,
            scroll_from_bottom: 0,
            max_scroll_from_bottom: 80,
        };

        let thumb = scrollbar_thumb(metrics, track).expect("overflow thumb");

        assert_eq!(thumb.height, 2);
        assert_eq!(thumb.top + thumb.height, track.y + track.height);
    }

    #[test]
    fn scrollbar_thumb_moves_toward_top_when_scrolled_up() {
        let track = Rect::new(79, 5, 1, 10);
        let bottom = scrollbar_thumb(
            TranscriptScrollMetrics {
                visible_rows: 20,
                total_rows: 100,
                scroll_from_bottom: 0,
                max_scroll_from_bottom: 80,
            },
            track,
        )
        .expect("bottom thumb");
        let scrolled = scrollbar_thumb(
            TranscriptScrollMetrics {
                visible_rows: 20,
                total_rows: 100,
                scroll_from_bottom: 40,
                max_scroll_from_bottom: 80,
            },
            track,
        )
        .expect("scrolled thumb");

        assert!(
            scrolled.top < bottom.top,
            "scrolling up should move the thumb toward the top"
        );
    }

    #[test]
    fn hint_bar_model_defaults_to_statusbar_keys() {
        let app = chat_app(vec![Message::user("hi")]);

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::StatusbarKeys);
    }

    #[test]
    fn hint_bar_model_uses_pager_keys_at_bottom() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.transcript_pager_active = true;
        app.transcript_scroll = 0;

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::PagerKeys);
    }

    #[test]
    fn hint_bar_model_uses_reviewing_when_pager_scrolled() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.transcript_pager_active = true;
        app.transcript_scroll = 3;

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::PagerReviewing);
    }

    #[test]
    fn hint_bar_model_uses_menu_when_menu_is_active() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.menu_stack
            .open(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::Menu);
    }

    #[test]
    fn hint_bar_model_uses_onboarding_for_first_launch_menu() {
        let mut app = AppState::new(vec![], 0, "ready".into(), None, false);
        app.menu_stack.open(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::Onboarding);
    }

    #[test]
    fn hint_bar_model_uses_approval_when_visible() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run command?".into(),
            body: "cargo test".into(),
            approval_kind: Some(approval_kinds::COMMAND.into()),
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::Approval);
    }

    #[test]
    fn hint_bar_model_uses_user_question_when_visible() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.user_question = Some(UserQuestionPickerState {
            session_id: SessionKey("local:test".into()),
            question_id: QuestionId::new(),
            turn_id: TurnId::new(),
            title: "Choose path".into(),
            body: "Which option?".into(),
            questions: vec![],
            active: 0,
            visible: true,
        });

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::UserQuestion);
    }

    /// Render `render_viewport` into a buffer via the custom inline `Frame`, at
    /// the live-UI height the event loop would size it to. We render straight
    /// into a `Buffer` (no escape-emitting backend needed) so the test does not
    /// require a `Write` backend.
    fn viewport_rows(app: &AppState, width: u16, height: u16) -> Vec<String> {
        viewport_rows_with_finalization(app, width, height, None)
    }

    fn viewport_rows_with_finalization(
        app: &AppState,
        width: u16,
        height: u16,
        live_finalization: Option<&LiveTurnFinalization>,
    ) -> Vec<String> {
        let palette = Palette::for_theme(ThemeName::Slate);
        let live_height =
            super::live_ui_height_with_finalization(app, width, height, live_finalization);
        let area = Rect::new(0, 0, width, live_height);
        let mut buffer = Buffer::empty(area);
        let mut frame = crate::tui_terminal::Frame::for_test(area, &mut buffer);
        render_viewport_with_finalization(&mut frame, app, palette, height, live_finalization);
        rendered_rows(&buffer)
    }

    fn lines_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn viewport_renders_live_ui_not_committed_history() {
        // Committed messages live in scrollback (finalized_history_lines), NOT
        // in the inline viewport. The viewport shows the composer + status.
        let app = chat_app(vec![
            Message::user("an old committed question"),
            Message::assistant("an old committed answer"),
        ]);
        let rows = viewport_rows(&app, 100, 40);
        let text = rows.join("\n");
        assert!(
            text.contains("Composer"),
            "viewport should show the composer chrome, got:\n{text}"
        );
        assert!(
            !text.contains("an old committed answer"),
            "committed history must go to scrollback, not the viewport:\n{text}"
        );
    }

    #[test]
    fn finalized_history_lines_contain_committed_messages() {
        let app = chat_app(vec![
            Message::user("question one"),
            Message::assistant("answer one"),
        ]);
        let palette = Palette::for_theme(ThemeName::Slate);
        let lines = finalized_history_lines(&app, palette, 80);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("question one"), "missing user msg: {text:?}");
        assert!(
            text.contains("answer one"),
            "missing assistant msg: {text:?}"
        );
    }

    #[test]
    fn active_turn_completed_activity_flushes_to_scrollback_and_leaves_live_tail() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the checks")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "Still checking the last item".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-running")
                .with_detail("cargo clippy --all-targets"),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id)
                .with_tool_call("call-complete")
                .with_detail("cargo test")
                .with_success(true),
        );

        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let inserted = lines_text(&update.lines_to_insert);
        assert!(
            inserted.contains("Agent task completed") && inserted.contains("Bash($ cargo test"),
            "completed activity should be inserted into scrollback mid-turn: {inserted:?}"
        );
        assert!(
            !inserted.contains("cargo clippy --all-targets"),
            "running activity must stay in the live tail: {inserted:?}"
        );

        let rows =
            viewport_rows_with_finalization(&app, 100, 40, update.live_tail_finalization.as_ref());
        let live = rows.join("\n");
        assert!(
            !live.contains("cargo test"),
            "flushed activity must not remain in the repainting viewport:\n{live}"
        );
        assert!(
            live.contains("cargo clippy --all-targets") && live.contains("Orchestrating"),
            "running activity should remain as the small live tail:\n{live}"
        );

        // Fix #7: EVERY live-tail row must be visible, top rows included. The
        // borderless live tail used to reserve a phantom 2-row border
        // allowance, scrolling its top 2 rows out of the area and leaving 2
        // dead rows at the bottom whenever the tail was >= 2 rows.
        let tail_lines = live_tail_lines_with_finalization(
            &app,
            Palette::for_theme(ThemeName::Slate),
            98,
            update.live_tail_finalization.as_ref(),
        );
        assert!(
            tail_lines.len() >= 2,
            "precondition: the phantom allowance only bites on a >=2-row tail"
        );
        for line in &tail_lines {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            assert!(
                live.contains(text),
                "live-tail row {text:?} must be rendered (top rows included):\n{live}"
            );
        }
    }

    #[test]
    fn glued_completed_segment_flushes_via_boundary_so_live_tail_holds_only_current_segment() {
        // Agentic narration segments are glued in live_reply (no blank line
        // between "…step one.step two:"), so the blank-line flush watermark never
        // advances and the whole growing reply piles up in the height-limited live
        // tail, clipping to its bottom ("intermediate truncated"). A completed
        // segment boundary (recorded when its tool call started) must flush the
        // finished segment so the live tail holds only the in-progress one.
        let turn_id = TurnId::new();
        let session = SessionKey("local:test".into());
        let head = "segment one glued.";
        let mut app = AppState::new(
            vec![SessionView {
                id: session.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: format!("{head}segment two still live"),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        // Boundary recorded at the tool call between segment one and two. There is
        // no blank line, so only this boundary can advance the watermark.
        app.live_reply_segment_boundaries
            .insert((session, turn_id), vec![head.len()]);

        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let inserted = lines_text(&update.lines_to_insert);
        assert!(
            inserted.contains("segment one glued."),
            "completed boundary-terminated segment must flush to scrollback even \
             without a blank line: {inserted:?}"
        );
        let rows =
            viewport_rows_with_finalization(&app, 100, 40, update.live_tail_finalization.as_ref());
        let live = rows.join("\n");
        assert!(
            !live.contains("segment one glued."),
            "flushed segment must not remain in the live tail:\n{live}"
        );
        assert!(
            live.contains("segment two still live"),
            "the in-progress segment stays in the live tail:\n{live}"
        );
    }

    #[test]
    fn word_safe_boundary_rejects_mid_word_splits() {
        // Mid-word (both neighbors are word chars) -> rejected, so a message/persisted
        // event sampling the live buffer at "anim|ate" never splits/flushes mid-word.
        assert!(!boundary_is_word_safe("animate", 4));
        assert!(!boundary_is_word_safe("haloPhase", 7));
        // Adjacent to a delimiter -> accepted (real segment ends still pass).
        assert!(boundary_is_word_safe("loop: next", 5));
        assert!(boundary_is_word_safe("done. Now", 5));
        assert!(boundary_is_word_safe("a\nb", 2));
        assert!(boundary_is_word_safe("end", 3));
        // Non-char-boundary -> rejected (safe).
        assert!(!boundary_is_word_safe("五大", 1));
    }

    #[test]
    fn active_turn_completed_reply_lines_flush_to_scrollback_and_leave_only_suffix_live() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("summarize")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "finalized assistant line\n\nstreaming suffix still live".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let inserted = lines_text(&update.lines_to_insert);
        assert!(
            inserted.contains("finalized assistant line"),
            "completed reply line should be inserted into scrollback mid-turn: {inserted:?}"
        );
        assert!(
            !inserted.contains("streaming suffix still live"),
            "unterminated reply suffix must stay live: {inserted:?}"
        );

        let rows =
            viewport_rows_with_finalization(&app, 100, 40, update.live_tail_finalization.as_ref());
        let live = rows.join("\n");
        assert!(
            !live.contains("finalized assistant line"),
            "flushed reply line must not remain in the repainting viewport:\n{live}"
        );
        assert!(
            live.contains("streaming suffix still live"),
            "only the active reply suffix should remain live:\n{live}"
        );
    }

    #[test]
    fn live_delta_segment_boundary_starts_fresh_markdown_block() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let first_segment = "### Step 1\n\nBody one.";
        let second_segment = "### Step 2\n\nBody two.";
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("build a demo")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: first_segment.into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let previous = next_live_turn_finalization(&app, None).expect("first watermark");
        assert_eq!(previous.reply_flushed_text, "### Step 1\n\n");

        app.live_reply_segment_boundaries.insert(
            (session_id.clone(), turn_id.clone()),
            vec![first_segment.len()],
        );
        app.sessions[0].live_reply.as_mut().unwrap().text =
            format!("{first_segment}{second_segment}");
        let next = next_live_turn_finalization(&app, Some(&previous)).expect("next watermark");

        let rendered = line_texts(&finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            &previous,
            &next,
        ));
        let body = rendered
            .iter()
            .position(|line| line == "Body one.")
            .expect("first segment body should render before the boundary");
        let heading = rendered
            .iter()
            .position(|line| line == "Step 2")
            .expect("second segment heading should render as markdown");

        assert_eq!(
            rendered.get(body + 1).map(String::as_str),
            Some(""),
            "segment boundary should force a blank paragraph break: {rendered:#?}"
        );
        assert_eq!(
            heading,
            body + 2,
            "Step 2 should be a discrete heading immediately after the boundary break: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("###")),
            "markdown heading markers must not leak in live scrollback: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("Body one.###")),
            "segment boundary must prevent body/header gluing: {rendered:#?}"
        );
    }

    #[test]
    fn committed_assistant_segment_boundary_starts_fresh_markdown_block() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let first_segment = "**Step 1:** a.";
        let second_segment = "**Step 2:** b.";
        let content = format!("{first_segment}{second_segment}");
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build a demo"),
                    Message::assistant(content.as_str()),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_prompt_anchors.push(TurnPromptAnchor {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            content: "build a demo".into(),
            anchor_index: 0,
            prior_matching_user_count: 0,
        });
        app.live_reply_segment_boundaries
            .insert((session_id, turn_id), vec![first_segment.len()]);

        let rendered = line_texts(&finalized_history_lines_range_dedup_live(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            1,
            &[],
        ));
        let first = rendered
            .iter()
            .position(|line| line == "• Step 1: a.")
            .expect("first segment should render as assistant prose");
        let second = rendered
            .iter()
            .position(|line| line == "Step 2: b." || line == "• Step 2: b.")
            .expect("second segment should render as a discrete markdown block");

        assert_eq!(
            rendered.get(first + 1).map(String::as_str),
            Some(""),
            "segment boundary should force a blank paragraph break: {rendered:#?}"
        );
        assert_eq!(
            second,
            first + 2,
            "Step 2 should not be glued onto Step 1: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("a.Step 2")),
            "committed assistant segment boundary must prevent gluing: {rendered:#?}"
        );
    }

    #[test]
    fn streamed_code_fence_separator_survives_chunk_boundary() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let flushed_fence = "```rust\nfn main() {}\n```\n";
        let full = format!("{flushed_fence}\nAfter the block.\n\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show code")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: full,
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let previous = LiveTurnFinalization::new(&session_id, &turn_id);
        let mut fence = LiveTurnFinalization::new(&session_id, &turn_id);
        fence.reply_flushed_text = flushed_fence.to_string();
        let next = next_live_turn_finalization(&app, Some(&fence)).expect("watermark");

        let mut streamed = finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            80,
            &previous,
            &fence,
        );
        streamed.extend(finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            80,
            &fence,
            &next,
        ));

        let rendered = streamed
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let close = rendered
            .iter()
            .position(|line| line.contains("└─"))
            .expect("code fence close");
        let after = rendered
            .iter()
            .position(|line| line.contains("After the block."))
            .expect("paragraph after fence");
        assert_eq!(
            &rendered[close + 1..after],
            [""],
            "streaming should keep exactly one blank between code and prose: {rendered:#?}"
        );
    }

    #[test]
    fn committed_turn_does_not_duplicate_live_flushed_reply_or_activity() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let first_activity = ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_turn(turn_id.clone())
            .with_detail("cargo test")
            .with_success(true);
        let second_activity_running = ActivityItem::new(ActivityKind::Tool, "shell", "running")
            .with_turn(turn_id.clone())
            .with_detail("cargo clippy --all-targets");
        let second_activity_done = ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_turn(turn_id.clone())
            .with_detail("cargo clippy --all-targets")
            .with_success(true);
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("finish the turn")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "already flushed line\n\nfinal answer tail".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(first_activity.clone());
        app.push_activity(second_activity_running);

        let mut tracker = ScrollbackTracker::new();
        let first = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let first_text = lines_text(&first.lines_to_insert);
        assert!(first_text.contains("already flushed line"));
        assert!(first_text.contains("Bash($ cargo test"));

        app.sessions[0].live_reply = None;
        app.sessions[0].messages.push(Message::assistant(
            "already flushed line\n\nfinal answer tail",
        ));
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id,
            request: Some("finish the turn".into()),
            anchor_index: Some(0),
            items: vec![first_activity, second_activity_done],
        });
        app.activity.clear();
        app.set_run_state_success();

        let second = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let second_text = lines_text(&second.lines_to_insert);
        assert!(
            !second_text.contains("already flushed line"),
            "committed assistant must not duplicate the live-flushed prefix: {second_text:?}"
        );
        assert!(
            second_text.contains("final answer tail"),
            "committed assistant should flush the unflushed suffix: {second_text:?}"
        );
        assert!(
            !second_text.contains("Bash($ cargo test"),
            "committed activity log must not duplicate the live-flushed action: {second_text:?}"
        );
        assert!(
            second_text.contains("cargo clippy --all-targets"),
            "committed activity log should flush the previously-running action: {second_text:?}"
        );

        app.sessions[0].messages.push(Message::user("new turn"));
        app.sessions[0].messages.push(Message::assistant(
            "already flushed line\nunrelated later answer",
        ));
        let third = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let third_text = lines_text(&third.lines_to_insert);
        assert!(
            third_text.contains("already flushed line")
                && third_text.contains("unrelated later answer"),
            "stale live-prefix coverage must not suppress a later assistant message: {third_text:?}"
        );
    }

    #[test]
    fn committed_agentic_turn_keeps_later_assistant_messages_discrete() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let first = "### Step 1\n\nI'll create demo.html with an HTML5 skeleton.";
        let second = "### Step 2\n\nNow I'll add a style block.";
        let third = "### Step 3\n\nFinally, I'll add an <h1>.";
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build a demo page"),
                    Message::assistant(first),
                    Message::assistant(second),
                    Message::assistant(third),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_prompt_anchors.push(TurnPromptAnchor {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            content: "build a demo page".into(),
            anchor_index: 0,
            prior_matching_user_count: 0,
        });

        let coverage = LiveTurnFinalization {
            session_id: session_id.0,
            turn_id: turn_id.0.to_string(),
            reply_flushed_text: "### Step ".into(),
            ..Default::default()
        };
        let rendered = line_texts(&finalized_history_lines_range_dedup_live(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            1,
            &[coverage],
        ));

        assert_eq!(
            rendered,
            vec![
                "1",
                "",
                "I'll create demo.html with an HTML5 skeleton.",
                "",
                "Step 2",
                "",
                "• Now I'll add a style block.",
                "",
                "Step 3",
                "",
                "• Finally, I'll add an <h1>.",
            ],
            "later assistant messages must render as fresh markdown blocks, not live-reply continuations"
        );
    }

    #[test]
    fn pending_prompt_present_in_session_history_renders_once() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("active prompt"),
                    Message::assistant("partial answer"),
                    Message::user("queued next"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "still working".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.pending_messages.push("queued next".into());
        app.set_run_state_in_progress();

        let rendered = line_texts(&live_tail_lines_with_finalization(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            None,
        ));

        assert_eq!(
            rendered,
            vec![
                "• still working",
                "",
                "queued 1 messages after active turn",
                "› queued next",
            ],
            "a prompt that is still pending must not also render as recent user context"
        );
    }

    #[test]
    fn finalized_history_lines_range_skips_already_flushed() {
        let app = chat_app(vec![
            Message::user("q1"),
            Message::assistant("a1"),
            Message::user("q2"),
            Message::assistant("a2"),
        ]);
        let palette = Palette::for_theme(ThemeName::Slate);
        let tail = finalized_history_lines_range(&app, palette, 80, 2);
        let text: String = tail
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("q2") && text.contains("a2"));
        assert!(
            !text.contains("a1"),
            "range(2) must not re-emit already-flushed messages: {text:?}"
        );
    }

    #[test]
    fn finalized_history_lines_include_anchored_activity_logs() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = chat_app(vec![
            Message::user("build the site"),
            Message::assistant("The site is built."),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo build")
                    .with_output_preview("Finished dev build")
                    .with_success(true),
            ],
        });

        let palette = Palette::for_theme(ThemeName::Codex);
        let lines = finalized_history_lines_range(&app, palette, 80, 1);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(
            text.contains("The site is built."),
            "missing answer: {text:?}"
        );
        assert!(
            text.contains("Agent task completed"),
            "missing activity log: {text:?}"
        );
        assert!(
            text.contains("Bash($ cargo build"),
            "missing tool detail: {text:?}"
        );
    }

    #[test]
    fn scrollback_renders_full_tool_output_without_ctrl_o_hint() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = chat_app(vec![
            Message::user("run tests"),
            Message::assistant("Done."),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("run tests".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo test")
                    .with_output_preview("line1\nline2\nline3\nline4\nline5")
                    .with_success(true),
            ],
        });

        let palette = Palette::for_theme(ThemeName::Codex);
        let lines = finalized_history_lines_range(&app, palette, 80, 1);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");

        // Frozen scrollback shows the FULL output (the toggle can't repaint it)…
        for n in 1..=5 {
            assert!(
                text.contains(&format!("line{n}")),
                "missing line{n}:\n{text}"
            );
        }
        // …and never the un-actionable Ctrl+O hint.
        assert!(
            !text.contains("Ctrl+O"),
            "scrollback must not show a dead Ctrl+O hint:\n{text}"
        );
        assert!(
            !text.contains("hidden"),
            "scrollback must not hide output:\n{text}"
        );
    }

    #[test]
    fn finalized_history_lines_place_each_activity_log_after_own_reply() {
        let session_id = SessionKey("local:test".into());
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut app = chat_app(vec![
            Message::user("first prompt"),
            Message::assistant("First answer."),
            Message::user("second prompt"),
            Message::assistant("Second answer."),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_a.clone(),
            request: Some("first prompt".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_a)
                    .with_detail("cargo test --first")
                    .with_success(true),
            ],
        });
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_b.clone(),
            request: Some("second prompt".into()),
            anchor_index: Some(2),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_b)
                    .with_detail("cargo test --second")
                    .with_success(true),
            ],
        });

        let lines = finalized_history_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let first_reply = rendered
            .iter()
            .position(|line| line.contains("First answer."))
            .expect("first reply");
        let second_prompt = rendered
            .iter()
            .position(|line| line.contains("second prompt"))
            .expect("second prompt");
        let second_reply = rendered
            .iter()
            .position(|line| line.contains("Second answer."))
            .expect("second reply");
        let cards = rendered
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| line.contains("Agent task completed").then_some(idx))
            .collect::<Vec<_>>();
        assert_eq!(cards.len(), 2, "expected two activity cards: {rendered:#?}");
        let first_card = cards[0];
        let second_card = cards[1];

        assert_eq!(
            first_card,
            first_reply + 2,
            "first card should follow first reply with one blank: {rendered:#?}"
        );
        assert!(line_is_blank(lines.get(first_reply + 1)));
        assert_eq!(
            second_card,
            second_reply + 2,
            "second card should follow second reply with one blank: {rendered:#?}"
        );
        assert!(line_is_blank(lines.get(second_reply + 1)));
        assert!(
            first_card < second_prompt
                && second_prompt < second_reply
                && second_reply < second_card,
            "activity cards must stay in turn order: {rendered:#?}"
        );
    }

    #[test]
    fn finalized_history_lines_carry_no_theme_background() {
        // Bug 3a: scrollback content must render on the terminal's native
        // background. The Codex theme's `surface` / `surface_alt` and the user
        // message's `diff_context_bg` would otherwise paint solid "brown blocks"
        // into the terminal's real scrollback. Every finalized line/span must
        // have `bg == None` so `insert_history` emits the default background.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = chat_app(vec![
            Message::user("a user message"),
            Message::assistant("an assistant reply\nwith two lines"),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("a user message".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo test")
                    .with_output_preview("tests passed")
                    .with_success(true),
            ],
        });
        // Use a theme with a non-default (brownish) surface, the regression case.
        let palette = Palette::for_theme(ThemeName::Codex);
        let lines = finalized_history_lines(&app, palette, 80);
        assert!(!lines.is_empty(), "expected finalized history lines");
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("Agent task completed"),
            "activity log must be part of finalized history: {text:?}"
        );
        for line in &lines {
            assert_eq!(
                line.style.bg, None,
                "finalized line carries a theme bg (brown block): {line:?}"
            );
            for span in &line.spans {
                assert_eq!(
                    span.style.bg, None,
                    "finalized span carries a theme bg (brown block): {span:?}"
                );
            }
        }
    }

    #[test]
    fn live_ui_height_is_bounded_below_screen_height() {
        // Even with a huge live tail, the inline viewport must leave scrollback
        // visible above it (so the user can always select/scroll prior output).
        let mut app = chat_app(vec![Message::user("hi")]);
        app.pending_messages = (0..50).map(|i| format!("queued {i}")).collect();
        let height = 30;
        let h = super::live_ui_height(&app, 100, height);
        assert!(
            h <= height.saturating_sub(super::LIVE_VIEWPORT_MIN_SCROLLBACK),
            "live UI height {h} must leave >= {} rows of scrollback on a {height}-row screen",
            super::LIVE_VIEWPORT_MIN_SCROLLBACK
        );
        assert!(h >= 1);
    }

    #[test]
    fn wants_fullscreen_overlay_tracks_inspector_and_modals() {
        let mut app = chat_app(vec![Message::user("hi")]);
        assert!(
            !super::wants_fullscreen_overlay(&app),
            "plain chat should use the inline viewport, not alt-screen"
        );
        app.focus = FocusPane::Workspace;
        assert!(
            super::wants_fullscreen_overlay(&app),
            "inspector panes should use the full-screen overlay"
        );
        app.focus = FocusPane::Composer;
        app.task_output.active = true;
        assert!(
            super::wants_fullscreen_overlay(&app),
            "an active detail modal should use the full-screen overlay"
        );
    }

    #[test]
    fn committed_fingerprint_changes_on_append_and_session_switch() {
        let app1 = chat_app(vec![Message::user("hi")]);
        let fp1 = committed_messages_fingerprint(&app1);
        let app2 = chat_app(vec![Message::user("hi"), Message::assistant("yo")]);
        let fp2 = committed_messages_fingerprint(&app2);
        assert_ne!(fp1, fp2, "appending a message must change the fingerprint");
        assert_eq!(fp1.session_id, fp2.session_id);
        assert_eq!(fp2.message_count, 2);
    }

    // ===== scrollback scar mitigation (specs/task-scrollback-scar.spec) =====

    fn active_turn_app(reply: &str) -> AppState {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:scar".into()),
                title: "scar".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: reply.into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app
    }

    #[test]
    fn live_reasoning_before_answer_renders_swimming_octopus_without_text() {
        // Codex-style live render: with non-empty live_reasoning and NO answer
        // streamed yet, push_turn_flow surfaces a single dimmed swimming-octopus
        // indicator (no "thinking" label) and NEVER the verbose reasoning prose.
        const VERBOSE: &str =
            "Let me carefully reason step by step about the user's request in great detail";
        // Empty live_reply.text => the answer has not started streaming yet.
        let app = active_turn_app("");
        let (session_id, turn_id) = app
            .active_turn()
            .map(|(sid, tid)| (sid.clone(), tid.clone()))
            .expect("active turn present (live_reply is Some)");
        let mut app = app;
        app.live_reasoning
            .insert((session_id, turn_id), VERBOSE.to_string());

        let palette = Palette::for_theme(ThemeName::Slate);
        let session = app.active_session().expect("active session").clone();
        let mut lines = Vec::new();
        push_turn_flow(&mut lines, palette, &app, &session, 80, None);
        let rendered = lines_text(&lines);

        assert!(
            OCTOPUS_SWIM_FRAMES
                .iter()
                .any(|frame| rendered.contains(frame)),
            "the indicator should show the swimming octopus; got: {rendered:?}"
        );
        // The octopus alone signals the thinking phase — no "thinking" label.
        assert!(
            !rendered.to_lowercase().contains("thinking"),
            "the indicator must carry no `thinking` text; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(VERBOSE),
            "verbose live reasoning text must NOT be rendered; got: {rendered:?}"
        );
    }

    #[test]
    fn live_reasoning_after_answer_started_renders_neither() {
        // Once the answer has begun streaming (live_reply.text non-empty), the
        // codex-style live render drops the thinking indicator too (and never
        // shows the verbose reasoning).
        const VERBOSE: &str = "internal chain of thought that should stay hidden";
        let app = active_turn_app("the answer has begun");
        let (session_id, turn_id) = app
            .active_turn()
            .map(|(sid, tid)| (sid.clone(), tid.clone()))
            .expect("active turn present");
        let mut app = app;
        app.live_reasoning
            .insert((session_id, turn_id), VERBOSE.to_string());

        let palette = Palette::for_theme(ThemeName::Slate);
        let session = app.active_session().expect("active session").clone();
        let mut lines = Vec::new();
        push_turn_flow(&mut lines, palette, &app, &session, 80, None);
        let rendered = lines_text(&lines);

        assert!(
            !OCTOPUS_SWIM_FRAMES
                .iter()
                .any(|frame| rendered.contains(frame)),
            "the swimming-octopus indicator must drop once the answer streams; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(VERBOSE),
            "verbose live reasoning text must NOT be rendered; got: {rendered:?}"
        );
        assert!(
            rendered.contains("the answer has begun"),
            "the streamed answer should still render; got: {rendered:?}"
        );
    }

    #[test]
    fn committed_assistant_reasoning_content_is_not_rendered_in_scrollback() {
        // Codex-style committed render: a finalized assistant message carrying
        // reasoning_content must NOT spill the verbose reasoning into scrollback.
        const VERBOSE: &str = "Here is my long winded committed reasoning that should never show";
        let mut assistant = Message::assistant("The final answer.");
        assistant.reasoning_content = Some(VERBOSE.to_string());

        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:scar".into()),
                title: "scar".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), assistant],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "Idle".into(),
            None,
            false,
        );

        let palette = Palette::for_theme(ThemeName::Slate);
        let lines = finalized_history_lines_range(&app, palette, 80, 0);
        let rendered = lines_text(&lines);

        assert!(
            rendered.contains("The final answer."),
            "the committed answer should still render; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(VERBOSE),
            "verbose committed reasoning must NOT appear in scrollback; got: {rendered:?}"
        );
    }

    #[test]
    fn live_tail_trims_trailing_blank_rows() {
        // Direct unit: trailing blanks popped, interior blanks kept.
        let mut lines = vec![
            Line::from("a"),
            Line::from(""),
            Line::from("b"),
            Line::from("   "),
            Line::from(""),
        ];
        trim_trailing_blank_lines(&mut lines);
        assert_eq!(lines.len(), 3);
        assert!(
            !line_is_blank(lines.last()),
            "tail must end on real content"
        );

        // End-to-end: the live-tail builder never returns a trailing blank.
        let app = active_turn_app("a streamed answer line");
        let tail =
            live_tail_lines_with_finalization(&app, Palette::for_theme(ThemeName::Slate), 80, None);
        assert!(!tail.is_empty());
        assert!(
            !line_is_blank(tail.last()),
            "live tail must not end on a spacer row (scar source)"
        );
    }

    #[test]
    fn collapse_blank_runs_reduces_multi_blank_gaps_to_one() {
        // The reported bug: concatenated block builders stack into 5-6 blank
        // gaps. A run of any length collapses to a single blank; single blanks,
        // content, and order are untouched. Mixed plain + styled (whitespace
        // span) blanks count the same.
        let mut lines = vec![
            Line::from("user"),
            Line::from(""),
            Line::from("   "), // styled-ish blank (whitespace)
            Line::from(""),
            Line::from(""),
            Line::from(""), // 5-blank run (the "6-blank user→reply" shape)
            Line::from("• reply"),
            Line::from(""), // a lone interior blank — must survive
            Line::from("more"),
        ];
        collapse_blank_runs(&mut lines);

        let rendered: Vec<String> = lines
            .iter()
            .map(|l| {
                if line_is_blank(Some(l)) {
                    "<blank>".to_string()
                } else {
                    l.spans.iter().map(|s| s.content.as_ref()).collect()
                }
            })
            .collect();
        assert_eq!(
            rendered,
            vec!["user", "<blank>", "• reply", "<blank>", "more"],
            "every blank run collapses to exactly one; content + order preserved"
        );
    }

    #[test]
    fn collapse_blank_runs_seeded_closes_cross_flush_seam() {
        // Reply text streams to scrollback across many small flushes. Flush 1
        // ends on its trailing blank separator; flush 2 opens on a blank. Per
        // flush each is fine, but at the seam they stack to a 2-line gap — the
        // exact mini5-observed bug. Seeding flush 2 with "prev ended blank"
        // drops its leading blank.
        let mut flush1 = vec![Line::from("paragraph one"), Line::from("")];
        let ends_blank = collapse_blank_runs_seeded(&mut flush1, false);
        assert!(ends_blank, "flush 1 ends on a blank separator");

        let mut flush2 = vec![Line::from(""), Line::from("paragraph two")];
        let ends_blank2 = collapse_blank_runs_seeded(&mut flush2, ends_blank);
        let rendered: Vec<String> = flush2
            .iter()
            .map(|l| {
                if line_is_blank(Some(l)) {
                    "<blank>".to_string()
                } else {
                    l.spans.iter().map(|s| s.content.as_ref()).collect()
                }
            })
            .collect();
        assert_eq!(
            rendered,
            vec!["paragraph two"],
            "seam blank dropped: scrollback shows one blank between the chunks, not two"
        );
        assert!(!ends_blank2, "flush 2 ends on content");

        // An all-blank batch after a blank collapses to nothing and leaves the
        // seam state blank (the separator already in scrollback stands).
        let mut flush3 = vec![Line::from(""), Line::from("  ")];
        assert!(collapse_blank_runs_seeded(&mut flush3, true));
        assert!(flush3.is_empty(), "redundant blanks after a blank all drop");
    }

    #[test]
    fn orphan_guard_drops_only_multi_line_leading_blank_runs() {
        let mut orphaned = vec![
            Line::from(""),
            Line::from(" "),
            Line::from(""),
            Line::from("▌ next prompt"),
        ];
        let ends_blank = collapse_blank_runs_seeded_orphan_guard(&mut orphaned, false, true);
        let rendered = line_texts(&orphaned);

        assert_eq!(
            rendered,
            vec!["▌ next prompt"],
            "a live-tail shrink must not carry an orphaned guardian blank run into scrollback"
        );
        assert!(!ends_blank);

        let mut legitimate_separator = vec![Line::from(""), Line::from("▌ next prompt")];
        collapse_blank_runs_seeded_orphan_guard(&mut legitimate_separator, false, true);
        assert_eq!(
            line_texts(&legitimate_separator),
            vec!["", "▌ next prompt"],
            "a single separator between distinct turns must survive"
        );
    }

    #[test]
    fn collapse_blank_runs_is_idempotent_and_preserves_edges() {
        // Already-collapsed input is unchanged (idempotent), and a single
        // leading/trailing blank is kept (collapse only removes *excess*).
        let mut lines = vec![
            Line::from(""),
            Line::from("a"),
            Line::from(""),
            Line::from("b"),
            Line::from(""),
        ];
        let before = lines.len();
        collapse_blank_runs(&mut lines);
        assert_eq!(lines.len(), before, "no runs to collapse → unchanged");
        collapse_blank_runs(&mut lines);
        assert_eq!(lines.len(), before, "idempotent on a second pass");
    }

    #[test]
    fn tail_height_cap_scales_with_terminal() {
        // Blank-separated paragraphs (each its own block) so the content is a
        // tall stack of rows that overruns any cap — not one wrapped paragraph.
        let huge = (1..=80)
            .map(|i| format!("para {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let app = active_turn_app(&huge);
        let tall = live_tail_height_with_finalization(&app, 80, 50, None);
        let short = live_tail_height_with_finalization(&app, 80, 24, None);
        assert!(
            tall <= 25,
            "tall cap must not exceed half the terminal: {tall}"
        );
        assert_ne!(tall, 18, "cap must no longer be the fixed 18");
        assert!(
            tall > short,
            "the cap scales with terminal height: {tall} vs {short}"
        );
    }

    #[test]
    fn live_ui_height_matches_rendered_tail() {
        // The height path must reflect exactly the builder the render path uses:
        // live_tail_height == capped visual rows of live_tail_lines (same source
        // render reads), so there is no off-by blank gap between them.
        let app = active_turn_app("one short answer line");
        let (w, h) = (80u16, 40u16);
        let wrap = usize::from(w.saturating_sub(2)).max(1);
        let lines = live_tail_lines_with_finalization(
            &app,
            Palette::for_theme(ThemeName::Slate),
            wrap,
            None,
        );
        let raw_rows = transcript_visual_rows(&lines, wrap) as u16;
        let cap = (h / 2).max(3);
        let expected = raw_rows.min(cap);
        assert_eq!(
            live_tail_height_with_finalization(&app, w, h, None),
            expected,
            "height path must equal capped rows of the shared live-tail builder"
        );
    }

    #[test]
    fn settled_turn_leaves_bounded_blank_rows() {
        // Active turn → settle: once idle with no live reply, the live tail is
        // empty (no trailing blanks carried over), so the viewport collapses to
        // chrome and no fresh blank rows are emitted.
        let mut app = active_turn_app("answer body");
        let _ =
            live_tail_lines_with_finalization(&app, Palette::for_theme(ThemeName::Slate), 80, None);
        // Settle the turn.
        app.set_run_state_idle();
        app.sessions[0].live_reply = None;
        app.sessions[0]
            .messages
            .push(Message::assistant("answer body"));

        let tail =
            live_tail_lines_with_finalization(&app, Palette::for_theme(ThemeName::Slate), 80, None);
        assert!(
            tail.iter().all(|line| line_is_blank(Some(line))) || tail.is_empty(),
            "a settled turn must not strand content-bearing tail rows: {}",
            lines_text(&tail)
        );
        assert!(
            !tail.last().is_some_and(|line| line_is_blank(Some(line))),
            "and never a trailing blank row"
        );
    }

    #[test]
    fn committed_history_stays_in_scrollback() {
        // Non-pager inline render must not repaint committed history into the
        // viewport (it lives in native scrollback) — the invariant the scar
        // mitigation must not regress.
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:scar".into()),
                title: "scar".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("earlier question"),
                    Message::assistant("COMMITTED_HISTORY_MARKER reply"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        assert!(!app.transcript_pager_active);
        let rows = viewport_rows(&app, 80, 24);
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("COMMITTED_HISTORY_MARKER")),
            "committed history must stay in scrollback, not the inline viewport: {rows:#?}"
        );
    }

    // ===== markdown link/strikethrough edge cases (issue #207) =====

    #[test]
    fn markdown_link_and_strike_parsers_validate_content() {
        // Well-formed link: returns (text, url, bytes consumed incl. delimiters).
        assert_eq!(parse_markdown_link("[a](b)rest"), Some(("a", "b", 6)));
        // Empty text or url → not a link (fall through to plain text).
        assert_eq!(parse_markdown_link("[](b)"), None);
        assert_eq!(parse_markdown_link("[a]()"), None);
        assert_eq!(parse_markdown_link("plain"), None);
        // Strikethrough requires non-whitespace content.
        assert_eq!(parse_markdown_strikethrough("~~x~~y"), Some(("x", 5)));
        assert_eq!(parse_markdown_strikethrough("~~~~"), None);
        assert_eq!(parse_markdown_strikethrough("~~  ~~"), None);
    }

    #[test]
    fn degenerate_strikethrough_keeps_literal_tildes() {
        let style = Style::default();
        // `~~~~` and `~~ ~~` have no real content: the markers must NOT be eaten
        // — the literal tildes survive and nothing is struck through.
        for input in ["~~~~", "~~ ~~"] {
            let spans = inline_markdown_spans(input, style, style, style);
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text, input, "degenerate `{input}` must render literally");
            assert!(
                spans
                    .iter()
                    .all(|s| !s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
                "degenerate `{input}` must produce no struck span"
            );
        }
        // A real strikethrough still renders struck.
        let spans = inline_markdown_spans("~~gone~~", style, style, style);
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT)
                    && s.content.as_ref() == "gone"),
            "a non-empty strikethrough must still be struck"
        );
    }

    #[test]
    fn table_cell_width_matches_rendered_link() {
        // The width path (`plain_inline_markdown`) must measure the RENDERED
        // link form, not the raw `[text](url)` markup, or a link in a table
        // cell mis-sizes its column (issue #207).
        assert_eq!(
            plain_inline_markdown("[Octos](https://octos.dev)"),
            "Octos (https://octos.dev)"
        );
        // When the text already IS the url it collapses to a single url — the
        // measured width must collapse the same way (was measuring `[url](url)`).
        assert_eq!(
            plain_inline_markdown("[https://octos.dev](https://octos.dev)"),
            "https://octos.dev"
        );

        // Measured text equals the concatenated rendered span text — same parser
        // drives both, so they cannot drift.
        let style = Style::default();
        for input in [
            "see [Octos](https://octos.dev) here",
            "[https://octos.dev](https://octos.dev)",
            "a ~~struck~~ b",
        ] {
            let rendered: String = inline_markdown_spans(input, style, style, style)
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(
                plain_inline_markdown(input),
                rendered,
                "width measurement must equal rendered text for `{input}`"
            );
        }
    }

    // ===== composer multi-line (specs/task-composer-multiline.spec) =====

    #[test]
    fn composer_height_grows_with_newlines() {
        // The composer box must reserve more rows as newlines are added, so a
        // multi-line draft is fully visible instead of being clipped.
        let mut app = AppState::new(vec![], 0, "ready".into(), None, false);
        app.composer = "one".into();
        let single = composer_height_for_size(&app, 80, 40);
        app.composer = "one\ntwo\nthree".into();
        let multi = composer_height_for_size(&app, 80, 40);
        assert!(
            multi > single,
            "composer height must grow with newlines: {multi} vs {single}"
        );
    }

    #[test]
    fn multiline_composer_not_capped_in_inline_viewport() {
        // Regression: the inline render derived the composer row cap from the
        // small viewport-region height (flooring at 3 rows), so a 6-line draft
        // dropped its earliest lines. The cap must come from the FULL terminal
        // height — the same basis `live_ui_height` reserved against — so every
        // line stays visible.
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:composer".into()),
                title: "composer".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.focus = crate::model::FocusPane::Composer;
        app.composer = "L1\nL2\nL3\nL4\nL5\nL6".into();
        let rows = viewport_rows(&app, 80, 40);
        let joined = rows.join("\n");
        for marker in ["L1", "L6"] {
            assert!(
                joined.contains(marker),
                "composer line {marker} must stay visible (not capped); rows: {rows:#?}"
            );
        }
    }
}

#[cfg(test)]
mod running_row_regression {
    use super::*;
    use crate::model::*;
    use crate::store::Store;
    use octos_core::app_ui::AppUiEvent;
    use octos_core::ui_protocol::*;

    #[test]
    fn running_bash_tool_started_renders_cc_card_not_legacy_verb_row() {
        let session_id = SessionKey("local:t".into());
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: session_id.clone(),
                    title: "t".into(),
                    profile_id: Some("dev".into()),
                    messages: vec![Message::user("go")],
                    tasks: vec![],
                    live_reply: None,
                }],
                0,
                "ready".into(),
                None,
                false,
            ),
        };
        let turn_id = TurnId::new();
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
            ToolStartedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id,
                tool_call_id: "c1".into(),
                tool_name: "bash".into(),
                arguments: Some(serde_json::json!({
                    "cmd": "sleep 20 && echo never-finishes",
                    "timeout_ms": 30000
                })),
            },
        )));
        let palette = Palette::for_theme(crate::cli::ThemeName::Codex);
        let backend = ratatui::backend::TestBackend::new(140, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("t");
        terminal
            .draw(|frame| render(frame, &store.state, palette))
            .expect("render");
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            text.push('\n');
        }
        assert!(!text.contains("Using bash"), "old verb leaked:\n{text}");
        assert!(!text.contains("{\"cmd\""), "raw JSON leaked:\n{text}");
    }
}
