//! `render` — extracted from `app.rs` (#365 step 2). Items keep their
//! original names; `app.rs` glob-re-exports them so every call site is
//! unchanged. `use super::*` reaches the app module's remaining items.
use super::*;

pub fn render(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    if app.activity_navigator.active {
        render_activity_navigator_overlay(frame, app, palette);
        return;
    } else if agent_view_active(app) {
        // Peeking a sub-agent: the whole screen becomes that agent's output
        // (full-screen, like the transcript pager) so the native scrollback that
        // holds the real chat is never touched. Tab/Shift+Tab/Esc restore it.
        render_agent_overlay(frame, app, palette);
        return;
    } else if app.focus == FocusPane::Tasks {
        // #337: `/ps` gets a dedicated two-pane dock, not the full inspector.
        render_tasks_dock_layout(frame, app, palette);
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
    let autonomy_height = autonomy_indicator_height(app, area.width);
    let harness_height = harness_status_height(app);
    // Parked-decision watchdog banner: one reserved row just above the composer,
    // present only once the escalation threshold has passed.
    let decision_height = decision_banner_height(app);
    // Height-gated on `terminal_height` — the SAME basis `live_ui_height` used to
    // reserve the row — so the reservation and this layout always agree.
    let agent_strip_height = agent_strip_height(app, terminal_height);
    // #407: Peer Dock — mirrors the agent strip. 0 when no peers exist or
    // the terminal is too short; otherwise reserves rows for the per-peer
    // view (or 1 for the collapsed pill).
    let peer_strip_height = peer_strip_height(app, terminal_height);
    let active_menu = active_menu_surface(app);
    // Budget the menu against the room left AFTER every OTHER row in the root
    // layout: the `Min(1)` live-tail floor, composer, status, the
    // autonomy/harness indicators, and the selector strip. Omitting any of them
    // (originally composer+status only) let a tall slash menu overcommit the
    // layout, so Ratatui compressed a fixed row — the tail floor included, since
    // `Min(1)` yields before a `Length` when space is short.
    let menu_available = area.height.saturating_sub(
        1 // Min(1) live-tail floor
            + composer_height
            + 1 // status
            + autonomy_height
            + harness_height
            + decision_height
            + agent_strip_height
            + peer_strip_height,
    );
    let menu_height = menu_height_for_viewport(active_menu.as_ref(), area.width, menu_available);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(menu_height),
            Constraint::Length(autonomy_height),
            Constraint::Length(harness_height),
            Constraint::Length(decision_height),
            Constraint::Length(composer_height),
            Constraint::Length(agent_strip_height),
            Constraint::Length(peer_strip_height),
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
        frame.render_widget(
            render_autonomy_indicator(app, palette, root[2].width),
            root[2],
        );
    }
    if harness_height > 0 {
        render_harness_status_row(frame, app, palette, root[3]);
    }
    if decision_height > 0 {
        frame.render_widget(render_decision_banner(app, palette), root[4]);
    }
    frame.render_widget(render_composer(app, palette, root[5]), root[5]);
    set_composer_cursor(frame, app, root[5]);
    if agent_strip_height > 0 {
        frame.render_widget(
            render_agent_strip(app, palette, root[6].height.saturating_sub(1)),
            root[6],
        );
    }
    if peer_strip_height > 0 {
        // #407: render the Peer Dock — collapsed pill or per-peer rows.
        frame.render_widget(
            Paragraph::new(peer_strip_lines(
                app,
                palette,
                peer_strip_height.saturating_sub(1),
                root[7].width,
            ))
            .style(Style::default().bg(palette.surface)),
            root[7],
        );
    }
    frame.render_widget(render_status(app, palette), root[8]);
}

/// The live (uncommitted / in-flight) transcript tail rendered inside the
/// viewport: recent-user context, turn-flow, the streaming reply, activity, and
/// pending messages. Committed messages are NOT here — they are in scrollback.
pub(super) fn render_live_tail_with_finalization(
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

/// #324 Phase C: the top session strip — one chip per open session.
/// Focused: `● title`; background: `○ title` + `✻` while its turn is live +
/// `(n)` unread terminals since last focus. Chips that don't fit collapse
/// into a trailing `+N`.
pub(super) fn render_session_strip(
    app: &AppState,
    palette: Palette,
    width: u16,
) -> Paragraph<'static> {
    let budget = width as usize;
    // #407 (review F4 + F5): decide the trailing overflow marker UP FRONT so
    // the chip loop can reserve its exact width. The structured peer pill is
    // shown UNCONDITIONALLY (not gated on `peer_dock_collapsed`) whenever the
    // hidden tail contains at least one peer — it is strictly more informative
    // than `+N` and needs no opt-in. When the tail mixes peers and non-peers,
    // keep the non-peer remainder as `+K` (review F6: don't erase them).
    let peer_tail_present = app
        .peer_session_meta
        .keys()
        .any(|sid| app.sessions.iter().any(|s| &s.id == sid));
    // The pill text the tail would render if it ends up peer-driven. Pre-compute
    // so both the width reservation and the final render agree on the string.
    let pill_text = if peer_tail_present {
        let (total_p, live_p, blocked_p, _unread_p) = peer_dock_counts(app);
        let mut text = t!(
            "app.hint.peer_dock_pill",
            count = total_p.to_string(),
            live = live_p.to_string(),
        )
        .into_owned();
        if blocked_p > 0 {
            text.push_str(&t!(
                "app.hint.peer_dock_pill_blocked",
                count = blocked_p.to_string()
            ));
        }
        Some(text)
    } else {
        None
    };
    // Reserve width for the trailing marker: the structured pill's measured
    // width (most important — ⚠ must not clip), else the legacy "+N" constant
    // (review F4: previously a fixed 4, which clipped the pill's most valuable
    // segment in exactly the big-fleet case the pill exists for).
    let trailing_reserve = pill_text
        .as_ref()
        .map(|text| UnicodeWidthStr::width(text.as_str()))
        .unwrap_or(4);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut hidden = 0usize;
    let total = app.sessions.len();
    for (idx, session) in app.sessions.iter().enumerate() {
        let focused = idx == app.selected_session;
        let live = app.session_turn_live(&session.id);
        let unread = app.unread_turns.get(&session.id).copied().unwrap_or(0);
        let mut title: String = session.title.chars().take(14).collect();
        if session.title.chars().count() > 14 {
            title.push('…');
        }
        let blocked = app.session_blocked_reason(&session.id).is_some();
        let mut chip = format!("{} {}", if focused { "●" } else { "○" }, title);
        // tui#398: a blocked session needs the user — ⚠ outranks the live
        // marker (a blocked turn is paused, not streaming).
        if blocked {
            chip.push_str(" ⚠");
        } else if live {
            chip.push_str(" ✻");
        }
        if unread > 0 && !focused {
            chip.push_str(&format!(" ({unread})"));
        }
        let chip_width = UnicodeWidthStr::width(chip.as_str()) + 3; // "   " gap
        // Reserve room for the trailing marker computed above.
        if used + chip_width + trailing_reserve > budget && idx + 1 < total {
            hidden = total - idx;
            break;
        }
        if used + chip_width > budget {
            hidden = total - idx;
            break;
        }
        let style = if focused {
            palette.text().add_modifier(Modifier::BOLD)
        } else if blocked || live || unread > 0 {
            palette.text()
        } else {
            palette.muted()
        };
        spans.push(Span::styled(chip, style));
        spans.push(Span::styled("   ".to_string(), palette.muted()));
        used += chip_width;
    }
    if hidden > 0 {
        // Count how many of the hidden tail are peers vs non-peers (review F6):
        // keep the non-peer remainder as `+K` so a mixed overflow doesn't
        // erase main/topic sessions from the strip.
        let hidden_sessions = app.sessions.iter().skip(total - hidden);
        let hidden_peer_count = hidden_sessions
            .clone()
            .filter(|s| app.peer_session_meta.contains_key(&s.id))
            .count()
            .min(hidden);
        let hidden_non_peer = hidden - hidden_peer_count;
        if hidden_non_peer > 0 {
            spans.push(Span::styled(format!("+{hidden_non_peer}"), palette.muted()));
        }
        if let Some(text) = pill_text {
            // Render the pre-computed pill text (same string the width
            // reservation measured — review F9: single source of truth).
            spans.push(Span::styled(text, palette.text()));
        }
    }
    Paragraph::new(Line::from(spans)).style(Style::default().bg(palette.surface_alt))
}

pub(super) fn render_chat_layout(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    if onboarding_first_launch_active(app) {
        render_onboarding_first_launch_layout(frame, app, palette);
        return;
    }

    let active_menu = active_menu_surface(app);
    let areas = chat_layout_areas_for_menu(app, frame.area(), active_menu.as_ref());

    if areas.session_strip.height > 0 {
        frame.render_widget(
            render_session_strip(app, palette, areas.session_strip.width),
            areas.session_strip,
        );
    }

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
        frame.render_widget(
            render_autonomy_indicator(app, palette, areas.autonomy.width),
            areas.autonomy,
        );
    }
    if areas.harness.height > 0 {
        render_harness_status_row(frame, app, palette, areas.harness);
    }
    if areas.decision.height > 0 {
        frame.render_widget(render_decision_banner(app, palette), areas.decision);
    }
    frame.render_widget(
        render_composer(app, palette, areas.composer),
        areas.composer,
    );
    set_composer_cursor(frame, app, areas.composer);
    if areas.agent_strip.height > 0 {
        frame.render_widget(
            render_agent_strip(app, palette, areas.agent_strip.height.saturating_sub(1)),
            areas.agent_strip,
        );
    }
    frame.render_widget(render_status(app, palette), areas.status);
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

/// UX2 A.1: render the OCTOS wordmark as a bordered window/header spanning the
/// top of the onboarding screen. `height >= 11` draws the full figlet; a
/// shorter box draws just the tagline. The box content is centered using
/// `unicode-width` column math so the CJK tagline and the box-drawing art stay
/// aligned. Mirrors `render_launch_banner`'s centering primitive.
pub(super) fn render_onboarding_header(area: Rect, palette: Palette) -> Paragraph<'static> {
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

pub(super) fn render_onboarding_first_launch_layout(
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

pub(super) fn render_inspector_layout(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
) {
    let composer_height = composer_height_for_size(app, frame.area().width, frame.area().height);
    let active_menu = active_menu_surface(app);
    let menu_height = menu_height_hint(
        active_menu.as_ref(),
        frame.area().width,
        frame.area().height,
    );
    let autonomy_height = autonomy_indicator_height(app, frame.area().width);
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
        frame.render_widget(
            render_autonomy_indicator(app, palette, root[2].width),
            root[2],
        );
    }
    if harness_height > 0 {
        render_harness_status_row(frame, app, palette, root[3]);
    }
    frame.render_widget(render_composer(app, palette, root[4]), root[4]);
    set_composer_cursor(frame, app, root[4]);
    frame.render_widget(render_status(app, palette), root[5]);
}

/// #337: the dedicated `/ps` dock — a focused two-pane layout (a full-height
/// Tasks/sub-agents dock on the left + the transcript on the right) instead of
/// the busy six-pane `render_inspector_layout`. `/ps` is the only way to reach
/// `FocusPane::Tasks` (Tab no longer cycles panes), so a clean task dashboard is
/// what the user asked for there; the other panes (Sessions/Artifacts/Workspace/
/// Git, reachable via `!cmd`) still use the full inspector grid.
pub(super) fn render_tasks_dock_layout(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
) {
    let composer_height = composer_height_for_size(app, frame.area().width, frame.area().height);
    let active_menu = active_menu_surface(app);
    let menu_height = menu_height_hint(
        active_menu.as_ref(),
        frame.area().width,
        frame.area().height,
    );
    let autonomy_height = autonomy_indicator_height(app, frame.area().width);
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

    // Two columns: a wide, full-height Tasks dock + the transcript.
    let upper = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(root[0]);

    frame.render_widget(render_tasks(app, palette), upper[0]);
    frame.render_widget(render_transcript(app, palette, upper[1]), upper[1]);
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, root[1], menu, palette);
    }
    if autonomy_height > 0 {
        frame.render_widget(
            render_autonomy_indicator(app, palette, root[2].width),
            root[2],
        );
    }
    if harness_height > 0 {
        render_harness_status_row(frame, app, palette, root[3]);
    }
    frame.render_widget(render_composer(app, palette, root[4]), root[4]);
    set_composer_cursor(frame, app, root[4]);
    frame.render_widget(render_status(app, palette), root[5]);
}

/// Full-screen overlay shown when the main pane is peeking a sub-agent
/// (`chat_view == Agent(id)`). Renders that agent's streamed output over the
/// whole terminal — the native scrollback holding the real chat is left
/// untouched — with the selector strip and a key hint pinned at the bottom.
pub(super) fn render_agent_overlay(frame: &mut impl FrameLike, app: &AppState, palette: Palette) {
    let crate::model::ChatViewTarget::Agent(agent_id) = &app.chat_view else {
        return;
    };
    let area = frame.area();
    frame.render_widget(Clear, area);

    // The peek is full-screen, so `area.height` is the whole terminal — the same
    // basis the inline strip uses, keeping the affordance consistent.
    let strip_height = agent_strip_height(app, area.height);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(strip_height),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        render_agent_overlay_body(app, palette, root[0], agent_id),
        root[0],
    );
    if strip_height > 0 {
        frame.render_widget(
            render_agent_strip(app, palette, root[1].height.saturating_sub(1)),
            root[1],
        );
    }
    frame.render_widget(render_agent_overlay_hint(palette), root[2]);
}

/// The scrollable body of the agent peek: an identity/status header followed by
/// the agent's streamed output log, anchored to the bottom via the shared
/// `transcript_scroll` (rows-from-bottom) so freshly streamed output stays in
/// view.
pub(super) fn render_agent_overlay_body(
    app: &AppState,
    palette: Palette,
    area: Rect,
    agent_id: &str,
) -> Paragraph<'static> {
    let wrap_width = transcript_wrap_width(area);
    let lines = agent_overlay_lines(app, palette, agent_id);

    // Inner height excludes the 2 border rows drawn by the block below.
    let visible_height = transcript_visible_height(area).saturating_sub(2).max(1);
    let total_rows = transcript_visual_rows(&lines, wrap_width);
    let max_scroll = total_rows.saturating_sub(visible_height);
    // Feed the true maximum back so `scroll_agent_view_up`/Home clamp to it
    // instead of overshooting with a sentinel (see `agent_view_scroll_max`).
    app.record_agent_view_scroll_max(max_scroll);
    let scroll_from_bottom = app.agent_view_scroll.min(max_scroll);
    let scroll_top =
        u16::try_from(max_scroll.saturating_sub(scroll_from_bottom)).unwrap_or(u16::MAX);

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(palette.text).bg(palette.surface_alt))
                .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false })
}

/// Bottom hint row for the agent peek: the keys that move between agents / the
/// main chat and scroll the output.
pub(super) fn render_agent_overlay_hint(palette: Palette) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(
        t!("app.hint.agent_peek_keys").into_owned(),
        palette.muted().bg(palette.surface),
    )))
    .style(Style::default().bg(palette.surface))
}

pub(super) fn render_activity_navigator_overlay(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
) {
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

pub(super) fn render_activity_navigator_toolbar(
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

pub(super) fn render_activity_navigator_list(
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

pub(super) fn render_activity_navigator_detail(
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

pub(super) fn render_sessions(app: &AppState, palette: Palette) -> List<'static> {
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

pub(super) fn render_tasks(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mut lines = Vec::new();

    // #333 (Phase 1): the pane `/ps` opens must surface the LIVE sub-agent
    // roster (`session_autonomy[].agents`, kept current by `agent/updated`),
    // not only the older `session.tasks` cache. A background spawn populates the
    // roster; rendering it here is what makes `/ps` a live background-task view
    // instead of a stale side-panel. The roster re-renders every frame from the
    // roster state, so it updates without a manual refresh.
    let agents = app.active_session_agents();
    if !agents.is_empty() {
        lines.push(Line::from(Span::styled(
            t!("app.pane.tasks_subagents").to_string(),
            palette.title(),
        )));
        for agent in agents {
            let glyph = agent_status_glyph(&agent.status);
            let name = {
                let n = agent.nickname.trim();
                if n.is_empty() {
                    agent.agent_id.clone()
                } else {
                    n.to_string()
                }
            };
            let mut spans = vec![
                Span::styled(format!("  {glyph} "), palette.text()),
                Span::styled(name, palette.text()),
                Span::styled(format!("  {}", agent.status), palette.muted()),
            ];
            if let Some(elapsed) = agent_elapsed_label(agent) {
                spans.push(Span::styled(format!("  {elapsed}"), palette.muted()));
            }
            lines.push(Line::from(spans));
            let detail = agent
                .last_task
                .as_deref()
                .or(agent.summary.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(detail) = detail {
                lines.push(Line::from(Span::styled(
                    format!("      {}", truncate_to_display_width(detail, 72)),
                    palette.muted(),
                )));
            }
        }
        lines.push(Line::from(Span::raw("")));
    }

    // #338: a background spawn appears in BOTH the roster (shown above) and the
    // legacy `session.tasks` cache. Skip the tasks already represented as
    // sub-agents (matched by task id) so each spawn shows once; non-agent tasks
    // (e.g. `spawn_only` pipeline tools that never become roster agents) still
    // render below.
    let roster_task_ids: std::collections::HashSet<&str> = agents
        .iter()
        .filter_map(|agent| agent.task_id.as_deref())
        .collect();
    if let Some(session) = app.active_session() {
        let non_roster_tasks: Vec<(usize, &crate::model::TaskView)> = session
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| !roster_task_ids.contains(task.id.0.to_string().as_str()))
            .collect();
        if non_roster_tasks.is_empty() {
            if agents.is_empty() {
                lines.push(Line::from(Span::styled(
                    t!("app.empty.no_tasks").to_string(),
                    palette.muted(),
                )));
            }
        } else {
            if !agents.is_empty() {
                lines.push(Line::from(Span::styled(
                    t!("app.pane.tasks_other").to_string(),
                    palette.title(),
                )));
            }
            for (idx, task) in non_roster_tasks {
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

pub(super) fn render_artifacts(app: &AppState, palette: Palette) -> Paragraph<'static> {
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

/// Claude-Code-style launch banner: a rounded box with the OCTOS logo, a
/// greeting, and the workspace path. No right-hand panel (per product call).
/// Rendered at the TOP of the transcript area for an empty session.
pub(super) fn render_launch_banner(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
    area: Rect,
) {
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

pub(super) fn render_transcript(
    app: &AppState,
    palette: Palette,
    area: Rect,
) -> Paragraph<'static> {
    transcript_render_model(app, palette, area).paragraph
}

pub(super) fn render_pager_scrollbar(
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

pub(super) fn render_btw_overlay(
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
    // Inner width: the block borders consume one column each side. Wrapping to
    // this width means no line is ever hard-clipped mid-word at the border
    // (the pre-fix overlay had no `.wrap()`, so long prose was cut).
    let inner_width = tail_area.width as usize - 2;
    let wrapped = btw_overlay_wrapped_lines(palette, aside, inner_width);
    if wrapped.is_empty() {
        return;
    }
    let content_rows = wrapped.len();
    // Rows available for content inside the pane borders. The tail area is
    // already capped at half the viewport by the caller, so a long answer can
    // exceed this — in which case we scroll rather than silently drop rows.
    let max_content = tail_area.height.saturating_sub(2) as usize;
    if max_content == 0 {
        return;
    }
    let scrollable = content_rows > max_content;
    let visible_rows = content_rows.min(max_content);
    // Clamp the stored offset to the true max each frame (mirrors the
    // transcript-scroll pattern: setters saturate, render clamps for display).
    let max_offset = content_rows.saturating_sub(visible_rows) as u16;
    let offset = aside.scroll.min(max_offset);
    let height = visible_rows as u16 + 2;
    let overlay = Rect {
        x: tail_area.x,
        y: tail_area.y,
        width: tail_area.width,
        height,
    };
    let title = t!("app.btw.pane_title").into_owned();
    let close_hint = t!("app.btw.close_hint").into_owned();
    let mut block = titled_block(title, palette, false, Some(close_hint));
    if scrollable {
        // Bottom-border position indicator so the user knows content is hidden
        // and how to reach it.
        let shown_end = offset as usize + visible_rows;
        let indicator = format!(
            " {}\u{2013}{}/{} \u{00b7} PgUp/PgDn ",
            offset as usize + 1,
            shown_end,
            content_rows
        );
        block = block
            .title_bottom(Line::from(Span::styled(indicator, palette.muted())).right_aligned());
    }
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Paragraph::new(wrapped)
            .scroll((offset, 0))
            .style(palette.text().bg(palette.surface))
            .block(block),
        overlay,
    );
}

pub(super) fn render_plan(app: &AppState, palette: Palette) -> Paragraph<'static> {
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

pub(super) fn render_workspace(
    app: &AppState,
    palette: Palette,
    area_height: u16,
) -> Paragraph<'static> {
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

pub(super) fn render_git(app: &AppState, palette: Palette, area_height: u16) -> Paragraph<'static> {
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

pub(super) fn render_autonomy_indicator(
    app: &AppState,
    palette: Palette,
    width: u16,
) -> Paragraph<'static> {
    let lines = autonomy_indicator_lines(app, palette, width);
    Paragraph::new(Text::from(lines)).style(Style::default().fg(palette.text).bg(palette.surface))
}

/// Render the sub-agent selector strip shown under the composer: a title row
/// with the `main` chip, then one line per visible sub-agent (vertical for
/// glanceable status/task detail), the selected target highlighted. Selection
/// is moved in the event loop; selecting an agent redirects the main pane to
/// its live output. `agent_rows` is the row budget the layout reserved beyond
/// the title row (`area.height - 1`).
pub(super) fn render_agent_strip(
    app: &AppState,
    palette: Palette,
    agent_rows: u16,
) -> Paragraph<'static> {
    Paragraph::new(agent_strip_lines(app, palette, agent_rows))
        .style(Style::default().bg(palette.surface))
}

/// Render the dedicated harness status row. Splits the row so the textual
/// status sits on the left and a `LineGauge` context-window bar sits on the
/// right when a `token_estimate` is known. Drawn into its own layout row
/// (never the composer border).
pub(super) fn render_harness_status_row(
    frame: &mut impl FrameLike,
    app: &AppState,
    palette: Palette,
    area: Rect,
) {
    let ratio = harness_context_ratio(app);
    let ctx_label = harness_context_label(app);
    // Size the gauge column to its label plus a short bar. The label now
    // carries the used/max token counts (`ctx 128K/1M ~13%`), so a fixed
    // 18-cell column would truncate it — derive the width from the label, and
    // only draw the gauge when the row is wide enough for both text and gauge.
    let gauge_width = ctx_label
        .as_deref()
        .map(|label| label.chars().count() as u16 + 6)
        .unwrap_or(18);
    let show_gauge = ratio.is_some() && ctx_label.is_some() && area.width > gauge_width + 12;
    // Suppress the textual `· ctx …` label when the gauge will be drawn —
    // otherwise the context readout renders twice on the same row (text on the
    // left and gauge on the right). The gauge is canonical on a wide terminal;
    // the text is the narrow-terminal fallback.
    let lines = harness_status_lines(app, palette, !show_gauge);
    if lines.is_empty() {
        return;
    }
    if let (Some(ratio), Some(label)) = (
        ratio.filter(|_| show_gauge),
        ctx_label.filter(|_| show_gauge),
    ) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(12), Constraint::Length(gauge_width)])
            .split(area);
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().fg(palette.text).bg(palette.surface)),
            split[0],
        );
        let gauge = LineGauge::default()
            .ratio(ratio)
            // Base style backs the label cells: `LineGauge` paints the whole
            // area with `self.style` before writing the (unstyled) label, so
            // without a surface bg here the `ctx …` label renders on the raw
            // terminal background — a mismatched block to the right of the
            // harness row, just above the composer. Keep it on `surface`.
            .style(Style::default().fg(palette.muted).bg(palette.surface))
            .label(label)
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

pub(super) fn render_composer(app: &AppState, palette: Palette, area: Rect) -> Paragraph<'static> {
    // #441: when the focused session is a peer, the composer is locked —
    // the peer is managed by the master agent, not the human. Show a
    // watermark and block local input.
    if let Some(session) = app.active_session() {
        if app.is_peer_session(&session.id) {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                " ",
                palette.text().bg(palette.surface),
            )));
            lines.push(Line::from(vec![
                Span::styled(" 🔒 ", palette.selected().bg(palette.surface)),
                Span::styled(
                    " This peer is managed by the master agent — type in the master session. ",
                    palette.muted().bg(palette.surface),
                ),
            ]));
            let title = t!("app.pane.composer").to_string();
            let block = titled_block(
                title,
                palette,
                false,
                Some("peer session — composer locked".into()),
            )
            .border_style(palette.border());
            return Paragraph::new(Text::from(lines))
                .style(Style::default().fg(palette.text).bg(palette.surface))
                .block(block);
        }
    }

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
        ComposerPresentation::Inline(draft) => {
            if let Some(view) = input_view.as_ref() {
                let text_width = composer_text_width(area.width);
                let base_style = palette.text().bg(palette.surface);
                // Live markdown highlighting is STYLE-ONLY: the wrap chunks
                // below are the exact strings the unstyled composer rendered,
                // and `split_highlighted_spans` re-emits them verbatim (the
                // highlight spans contribute styles, never text), so content,
                // wrapping, and the cursor math stay byte-identical. The fence
                // state is seeded from the draft lines scrolled off above the
                // visible window so an open ``` block keeps its styling.
                let mut in_fence = false;
                for hidden in draft.split('\n').take(view.first_line_index) {
                    if markdown_highlight::is_fence_line(hidden) {
                        in_fence = !in_fence;
                    }
                }
                let mut first_row = true;
                for line in view.lines.iter() {
                    let highlighted =
                        markdown_highlight::markdown_highlight_line(line, &mut in_fence, palette);
                    let chunks = wrap_composer_line(line, text_width);
                    for row_spans in markdown_highlight::split_highlighted_spans(
                        &highlighted,
                        &chunks,
                        base_style,
                    ) {
                        let prefix = if first_row { " › " } else { "   " };
                        let prefix_style = if first_row {
                            palette.selected().bg(palette.surface)
                        } else {
                            palette.muted().bg(palette.surface)
                        };
                        let mut spans = vec![Span::styled(prefix, prefix_style)];
                        spans.extend(row_spans);
                        lines.push(Line::from(spans));
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

pub(super) fn render_status(app: &AppState, palette: Palette) -> Paragraph<'static> {
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

    // A turn parked on an operator decision is not "Working": the model is
    // stopped until the human answers. Show a distinct Waiting state (with a
    // steady `?` instead of the spinner) whenever an approval or an
    // AskUserQuestion is pending FOR THE ACTIVE SESSION — visible or
    // collapsed, the turn is parked either way. The arrival path parks
    // run_state at Blocked (and some mid-turn paths keep InProgress), so
    // both count (codex P1: the InProgress-only gate never fired on the
    // real flow, and an unscoped modal check marked the active session
    // waiting for another session's decision).
    let active_session_id = app.active_session().map(|session| session.id.clone());
    let pending_decision_for_active = active_session_id.as_ref().is_some_and(|session_id| {
        app.approval
            .as_ref()
            .is_some_and(|approval| &approval.session_id == session_id)
            || app
                .user_question
                .as_ref()
                .is_some_and(|question| &question.session_id == session_id)
    });
    let waiting_on_operator = pending_decision_for_active
        && matches!(
            app.run_state,
            SessionRunState::InProgress | SessionRunState::Blocked { .. }
        );
    let (state_marker, state_label, state_style) = if waiting_on_operator {
        (
            "?".to_string(),
            t!("app.status.waiting").to_string(),
            palette.selected().add_modifier(Modifier::BOLD),
        )
    } else if matches!(app.run_state, SessionRunState::InProgress) && active_turn_is_thinking(app) {
        // Reasoning phase (octopus swimming): keep the animated spinner marker
        // and the in-progress style, but label it "Thinking" — the turn is
        // running, but it is not yet acting (no answer/tool output). Flips to
        // "Working" the moment the answer or a tool call begins. Gated on
        // InProgress so a late terminal (e.g. an Error for a switch-finalized
        // turn while a successor is still live-and-blank) is never masked by
        // the Thinking label (codex P2).
        (
            run_state_marker(&app.run_state).to_string(),
            t!("app.status.thinking").to_string(),
            run_state_style(&app.run_state, palette),
        )
    } else {
        (
            run_state_marker(&app.run_state).to_string(),
            run_state_status_label(&app.run_state),
            run_state_style(&app.run_state, palette),
        )
    };
    Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", t!("app.status.state_label")),
            palette.title().bg(palette.surface_alt),
        ),
        Span::styled(state_marker, state_style.bg(palette.surface_alt)),
        Span::styled(" ", palette.muted().bg(palette.surface_alt)),
        Span::styled(state_label, state_style.bg(palette.surface_alt)),
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

pub(super) fn render_decision_banner(app: &AppState, palette: Palette) -> Paragraph<'static> {
    // The 60s parked-decision escalation is a danger-styled alert and takes
    // precedence over the (calmer) submit affordance.
    if let Some(elapsed) = parked_decision_escalation_secs(app) {
        let text = t!(
            "app.statusbar.parked_decision_banner",
            elapsed = format_elapsed_secs(elapsed)
        )
        .into_owned();
        return Paragraph::new(Line::from(Span::styled(
            format!(" {text} "),
            Style::default()
                .fg(palette.text)
                .bg(palette.danger_bg)
                .add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(palette.danger_bg));
    }

    // A pending question: pin its submit/toggle affordance here so it is ALWAYS
    // visible, regardless of live-tail scroll or status-bar truncation. Styled
    // like a highlighted option row (▌ accent) so it reads as a control, not a
    // dim footer hint.
    if let Some(picker) = pending_question_for_banner(app) {
        let hint = user_question_action_labels(picker).join("   ");
        return Paragraph::new(Line::from(vec![
            Span::styled(" ▌ ", palette.title().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{hint} "),
                palette.selected().add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    // `decision_banner_height` is 0 in every other case, so this is never shown.
    Paragraph::new(Line::from(""))
}

pub(super) fn render_task_output_modal(
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

pub(super) fn render_artifact_detail_modal(
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

pub(super) fn render_thread_graph_detail_modal(
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

pub(super) fn render_turn_state_detail_modal(
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
