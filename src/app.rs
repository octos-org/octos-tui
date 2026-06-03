use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, LineGauge, List, ListItem, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use octos_core::ui_protocol::approval_kinds;

use crate::{
    menu::render as menu_render,
    model::{
        ActivityItem, ActivityKind, AppState, ApprovalModalState, ArtifactDetailState,
        ComposerPresentation, DiffPreviewPaneState, FocusPane, PlanStep as RenderedPlanStep,
        SessionAutonomyState, SessionRunState, SessionView, TaskOutputDetailState,
        ThreadGraphDetailState, TurnActivityLog, TurnStateDetailState, extract_plan_steps,
        task_state_label,
    },
    theme::Palette,
};

pub fn render(frame: &mut Frame<'_>, app: &AppState, palette: Palette) {
    if inspector_visible(app) {
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

fn render_chat_layout(frame: &mut Frame<'_>, app: &AppState, palette: Palette) {
    if onboarding_first_launch_active(app) {
        render_onboarding_first_launch_layout(frame, app, palette);
        return;
    }

    let composer_height = composer_height_for_size(app, frame.area().width, frame.area().height);
    let active_menu = active_menu_surface(app);
    let desired_menu_height = menu_height_hint(
        active_menu.as_ref(),
        frame.area().width,
        frame.area().height,
    );
    let autonomy_height = autonomy_indicator_height(app);
    let harness_height = harness_status_height(app);
    let surface_budget = frame.area().height.saturating_sub(
        min_transcript_height(frame.area().height)
            + composer_height
            + autonomy_height
            + harness_height
            + 1,
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
        .split(frame.area());

    frame.render_widget(render_transcript(app, palette, root[0]), root[0]);
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

fn render_onboarding_first_launch_layout(frame: &mut Frame<'_>, app: &AppState, palette: Palette) {
    let composer_height = composer_height_for_size(app, frame.area().width, frame.area().height);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    if let Some(menu) = active_menu_surface(app).as_ref() {
        menu_render::render_menu_surface(frame, root[0], menu, palette);
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
                    | crate::menu::registry::MENU_ONBOARD_FAMILY
                    | crate::menu::registry::MENU_ONBOARD_MODEL
                    | crate::menu::registry::MENU_ONBOARD_ROUTE
            )
        })
}

fn min_transcript_height(terminal_height: u16) -> u16 {
    if terminal_height < 30 { 8 } else { 12 }
}

fn render_inspector_layout(frame: &mut Frame<'_>, app: &AppState, palette: Palette) {
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
            "Sessions",
            palette,
            app.focus == FocusPane::Sessions,
            Some("Tab"),
        )
        .border_style(palette.border()),
    )
}

fn render_tasks(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if let Some(session) = app.active_session() {
        if session.tasks.is_empty() {
            lines.push(Line::from(Span::styled("No tasks yet", palette.muted())));
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
                "Tasks",
                palette,
                app.focus == FocusPane::Tasks,
                Some("j/k or Up/Down"),
            )
            .border_style(palette.border()),
        )
        .wrap(Wrap { trim: false })
}

fn render_artifacts(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mut lines = Vec::new();

    if app.artifacts.items.is_empty() {
        lines.push(Line::from(Span::styled(
            "No artifacts in snapshot",
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
                    format!("    from {}", item.source),
                    palette.muted(),
                )));
            }
        }
    }

    Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                "Artifacts",
                palette,
                app.focus == FocusPane::Artifacts,
                Some("j/k"),
            )
            .border_style(palette.border()),
        )
        .wrap(Wrap { trim: false })
}

fn render_transcript(app: &AppState, palette: Palette, area: Rect) -> Paragraph<'static> {
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
            if let Some(reasoning) = message.reasoning_content.as_deref() {
                push_message_block(&mut lines, palette, "reasoning", reasoning, wrap_width);
            }
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
                push_turn_activity_log_section(&mut lines, palette, log, app);
            }

            if turn_flow_visible && Some(idx) == latest_user_index {
                approval_context_start = Some(message_start);
                push_turn_flow(&mut lines, palette, app, session, wrap_width);
                turn_flow_rendered = true;
            }
        }

        if !turn_flow_rendered
            && approval_visible
            && let Some(prompt) = latest_user_message(session)
        {
            approval_context_start = Some(lines.len());
            push_recent_user_context(&mut lines, palette, prompt, wrap_width);
            push_turn_flow(&mut lines, palette, app, session, wrap_width);
        } else if !turn_flow_rendered {
            push_turn_flow(&mut lines, palette, app, session, wrap_width);
        }

        if !app.pending_messages.is_empty() {
            push_pending_messages_block(&mut lines, palette, &app.pending_messages, wrap_width);
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No session selected",
            palette.muted(),
        )));
    }

    let visible_height = transcript_visible_height(area);
    let total_rows = transcript_visual_rows(&lines, wrap_width);
    let max_scroll = total_rows.saturating_sub(visible_height);
    let scroll_from_bottom = app.transcript_scroll.min(max_scroll);
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
    let scroll_top = scroll_top as u16;

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .style(Style::default().fg(palette.text).bg(palette.surface_alt))
                .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false })
}

fn transcript_visible_height(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(2)).max(1)
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
                })
                .or_else(|| {
                    session
                        .messages
                        .iter()
                        .rposition(|message| message.role.as_str() == "user")
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

fn should_pin_recent_user_context(app: &AppState, session: &SessionView) -> bool {
    session.live_reply.is_some()
        || live_turn_diff_preview_visible(app)
        || app.active_turn().is_some()
        || app.run_state.is_active()
        || has_flow_activity(app)
}

fn should_show_turn_flow(app: &AppState, session: &SessionView) -> bool {
    app.approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
        || should_pin_recent_user_context(app, session)
}

fn push_turn_flow(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    app: &AppState,
    session: &SessionView,
    width: usize,
) {
    if let Some(approval) = app.approval.as_ref().filter(|approval| approval.visible) {
        push_inline_approval_card(lines, palette, approval);
    }

    if let Some(live_reply) = &session.live_reply {
        push_live_reply_block(lines, palette, &live_reply.text, width);
    }

    push_activity_section(lines, palette, app);

    if live_turn_diff_preview_visible(app) {
        push_inline_diff_preview(lines, palette, &app.diff_preview);
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
    width: usize,
) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    let bg = chat_message_bg(palette, "user");
    push_formatted_body(lines, palette, content, "› ", Some(bg), width);
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

    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, role);
    let indent = match role {
        "user" => "› ",
        "tool" => "$ ",
        "reasoning" => "· ",
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

fn push_live_reply_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    width: usize,
) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, "assistant");
    push_formatted_body_marked(lines, palette, content, "", Some("• "), Some(bg), width);
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
                "queued ",
                palette.title().add_modifier(Modifier::BOLD).bg(bg),
            ),
            Span::styled(
                format!(
                    "{} message{} after active turn",
                    pending.len(),
                    if pending.len() == 1 { "" } else { "s" }
                ),
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
                format!("› +{} more queued", pending.len() - 3),
                palette.muted().bg(bg),
            )],
            Some(bg),
        ));
    }
}

fn chat_message_bg(palette: Palette, role: &str) -> Color {
    match role {
        "user" => palette.diff_context_bg,
        "assistant" => palette.surface,
        "reasoning" => palette.surface,
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
    let mut in_code = false;
    let mut last_blank = false;
    let mut prose = Vec::new();
    let mut table = Vec::new();
    let mut checkbox_index = 1usize;
    let normalized = content.trim_matches(|ch: char| ch.is_whitespace() && ch != '\n');

    for raw_line in normalized.lines() {
        let line = if in_code { raw_line } else { raw_line.trim() };
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            if in_code {
                in_code = false;
                lines.push(chat_line(
                    vec![
                        Span::styled(indent, style_bg(palette.border(), bg)),
                        Span::styled("└─", style_bg(palette.border(), bg)),
                    ],
                    bg,
                ));
            } else {
                in_code = true;
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
                        Span::styled(language, style_bg(palette.selected(), bg)),
                    ],
                    bg,
                ));
            }
            last_blank = false;
            continue;
        }

        if in_code {
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled("│ ", style_bg(palette.border(), bg)),
                    Span::styled(
                        truncate_terminal_line(line, CODE_BLOCK_LINE_LIMIT),
                        style_bg(palette.muted(), bg),
                    ),
                ],
                bg,
            ));
            last_blank = false;
            continue;
        }

        if line.is_empty() {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg, width);
            checkbox_index = 1;
            if !last_blank && !lines.is_empty() {
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

fn line_is_blank(line: Option<&Line<'static>>) -> bool {
    line.map(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        .unwrap_or(false)
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

fn inline_markdown_spans(
    text: &str,
    normal_style: Style,
    bold_style: Style,
    code_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;

    while !rest.is_empty() {
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
        let next_emphasis = rest
            .char_indices()
            .skip(1)
            .find(|(_, ch)| matches!(ch, '*' | '_'))
            .map(|(idx, _)| idx);
        let next = [next_bold, next_code, next_emphasis]
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

fn file_mutation_action_label(operation: &str) -> &'static str {
    match operation {
        "add" | "added" | "create" | "created" => "Added",
        "delete" | "deleted" | "remove" | "removed" => "Deleted",
        "write" | "wrote" => "Wrote",
        "modify" | "modified" | "update" | "updated" => "Changed",
        _ => "Changed",
    }
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

fn tool_invocation_text(item: &ActivityItem) -> Option<String> {
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        return Some(detail.to_string());
    }
    item.arguments
        .as_ref()
        .and_then(|arguments| serde_json::to_string(arguments).ok())
}

fn meaningful_output_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn tool_action_label(item: &ActivityItem, running: bool) -> &'static str {
    if item.title == "shell" {
        return shell_action_label(
            tool_invocation_text(item).as_deref().unwrap_or_default(),
            running,
        );
    }

    match (item.title.as_str(), running) {
        ("read_file", true) => "Reading",
        ("read_file", false) => "Read",
        ("write_file", true) => "Writing",
        ("write_file", false) => "Wrote",
        ("edit_file" | "diff_edit", true) => "Editing",
        ("edit_file" | "diff_edit", false) => "Edited",
        ("list_dir", true) => "Listing",
        ("list_dir", false) => "Listed",
        ("grep" | "glob" | "web_search" | "deep_search", true) => "Searching",
        ("grep" | "glob" | "web_search" | "deep_search", false) => "Searched",
        ("web_fetch", true) => "Fetching",
        ("web_fetch", false) => "Fetched",
        ("browser", true) => "Browsing",
        ("browser", false) => "Browsed",
        ("spawn", true) => "Spawning",
        ("spawn", false) => "Spawned",
        ("send_file", true) => "Sending",
        ("send_file", false) => "Sent",
        ("manage_skills" | "admin_manage_skills", true) => "Managing",
        ("manage_skills" | "admin_manage_skills", false) => "Managed",
        (_, true) => "Using",
        (_, false) => "Used",
    }
}

fn shell_action_label(command: &str, running: bool) -> &'static str {
    let command = command.trim_start();
    let lower = command.to_ascii_lowercase();
    let label = if lower.starts_with("sleep ") || lower.contains("; sleep ") {
        ("Waiting", "Waited")
    } else if lower.contains("cargo test")
        || lower.contains("npm test")
        || lower.contains("npm run test")
        || lower.contains("pytest")
        || lower.contains("go test")
    {
        ("Testing", "Tested")
    } else if lower.contains("cargo build")
        || lower.contains("npm run build")
        || lower.contains("pnpm build")
        || lower.contains("go build")
    {
        ("Building", "Built")
    } else if lower.contains("npm install")
        || lower.contains("pnpm install")
        || lower.contains("cargo install")
    {
        ("Installing", "Installed")
    } else {
        ("Running", "Ran")
    };

    if running { label.0 } else { label.1 }
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
        Span::styled("▸ command  ", palette.selected().bg(palette.surface)),
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
            "Approval Requested",
            palette.title().add_modifier(Modifier::BOLD),
        ),
        Span::styled("  inline", palette.muted()),
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
            Span::styled("d = view diff", palette.selected()),
        ]));
    }
}

fn approval_action_labels(_approval: &ApprovalModalState) -> [&'static str; 3] {
    [
        "y = approve this command once",
        "s = approve this command/scope for the session",
        "n = deny it",
    ]
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

fn push_activity_section(lines: &mut Vec<Line<'static>>, palette: Palette, app: &AppState) {
    let flow_activity = flow_activity_items(app);
    if flow_activity.is_empty() {
        return;
    }
    if !lines.is_empty() {
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
        );
    }
    if flow_activity.len() > recent.len() {
        lines.push(Line::from(vec![
            Span::styled("     ", palette.muted()),
            Span::styled(
                format!(
                    "... +{} older action(s)",
                    flow_activity.len() - recent.len()
                ),
                palette.muted(),
            ),
        ]));
    }
}

fn has_flow_activity(app: &AppState) -> bool {
    !flow_activity_items(app).is_empty()
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
) {
    if log.items.is_empty() {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    let shown_limit = if app.expanded_tool_outputs { 12 } else { 3 };
    // Full uncapped set (header counts + footer tally both derive from this via
    // `task_group_counts`, so they cannot diverge).
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
    );
    if full.len() > shown.len() {
        let hidden = full.len() - shown.len();
        let (_, completed, active, _) = task_group_counts(&full);
        lines.push(Line::from(vec![
            Span::styled("     ", palette.muted()),
            Span::styled(
                format!("... +{hidden} more, {completed} completed, {active} active"),
                palette.muted(),
            ),
        ]));
    }
    if app.diff_preview.active && app.diff_preview.turn_id.as_ref() == Some(&log.turn_id) {
        push_inline_diff_preview(lines, palette, &app.diff_preview);
    }
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
) -> &'static str {
    if in_progress {
        "Orchestrating..."
    } else if is_active_group && pending_continuations > 0 {
        "Re-entering (continuing)..."
    } else if failed > 0 {
        "Agent task finished with errors"
    } else {
        "Agent task completed"
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
    let mut metadata = vec![format!("{total} action(s)")];
    if active > 0 {
        metadata.push(format!("{active} active"));
    }
    if completed > 0 {
        metadata.push(format!("{completed} completed"));
    }
    if failed > 0 {
        metadata.push(format!("{failed} failed"));
    }
    if active_subagents > 0 {
        metadata.push(format!("{active_subagents} sub-agent(s) running"));
    }
    if let Some(turn_id) = turn_id {
        metadata.push(format!("turn {}", short_id(&turn_id.0.to_string())));
    }

    // While orchestrating, show the animated octopus "tentacle pulse" spinner;
    // a settled chip keeps the static bullet. Both are 1 col wide so the title
    // stays aligned whether running or done.
    let icon = if in_progress { spinner_frame() } else { "•" };
    let spans = vec![
        Span::styled(format!("{icon} "), palette.selected()),
        Span::styled(title, palette.title().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" ({})", metadata.join(" · ")), palette.muted()),
    ];
    lines.push(Line::from(spans));

    for (idx, item) in items.iter().enumerate() {
        push_agent_task_child(lines, palette, item, idx == 0, expanded);
    }

    // List this turn's running sub-agents (from session.tasks, attributed by
    // turn) as children, so their live progress shows under THIS chip instead
    // of forming a separate turn-less "Orchestrating" chip (mini5 soak: folds
    // the phantom second chip into the orchestrating turn's chip).
    for (idx, title) in subagent_titles.iter().enumerate() {
        let first = items.is_empty() && idx == 0;
        let prefix = if first { "  ⎿  " } else { "     " };
        lines.push(Line::from(vec![
            Span::styled(prefix, palette.border()),
            Span::styled("◻ ", palette.selected()),
            Span::styled(title.clone(), palette.text()),
            Span::styled("  running", palette.muted()),
        ]));
    }
}

fn push_agent_task_child(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    first: bool,
    expanded: bool,
) {
    let (icon, icon_style) = activity_status_icon(item, palette);
    let prefix = if first { "  ⎿  " } else { "     " };
    let mut spans = vec![
        Span::styled(prefix, palette.border()),
        Span::styled(icon, icon_style),
        Span::styled(" ", palette.muted()),
    ];
    spans.extend(compact_activity_spans(item, palette));
    lines.push(Line::from(spans));

    if item.kind == ActivityKind::Tool {
        push_compact_tool_preview(lines, palette, item, expanded);
    }
}

fn compact_activity_spans(item: &ActivityItem, palette: Palette) -> Vec<Span<'static>> {
    if let Some(mutation) = FileMutationActivity::from_item(item) {
        let mut spans = vec![
            Span::styled(
                file_mutation_action_label(&mutation.operation),
                palette.text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", palette.muted()),
            Span::styled(compact_file_path(&mutation.path), palette.text()),
            Span::styled(format!("  {}", mutation.operation), palette.muted()),
        ];
        if mutation.preview_ready {
            spans.push(Span::styled("  preview ready", palette.selected()));
        }
        return spans;
    }

    if item.kind == ActivityKind::Tool {
        let running = is_running_activity(item);
        let mut spans = vec![
            Span::styled(
                tool_action_label(item, running),
                palette.text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", palette.muted()),
            Span::styled(item.title.clone(), palette.text()),
        ];
        if let Some(invocation) = tool_invocation_text(item) {
            let prompt = if item.title == "shell" { "$ " } else { "" };
            spans.push(Span::styled(": ", palette.muted()));
            spans.push(Span::styled(
                format!("{prompt}{}", truncate_terminal_line(&invocation, 96)),
                palette.text(),
            ));
        }
        push_compact_metadata_spans(&mut spans, palette, item);
        return spans;
    }

    let mut spans = vec![
        Span::styled(
            item.title.clone(),
            palette.text().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", item.status), palette.muted()),
    ];
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        spans.push(Span::styled("  ", palette.muted()));
        spans.push(Span::styled(
            truncate_terminal_line(detail, 96),
            palette.muted(),
        ));
    }
    push_compact_metadata_spans(&mut spans, palette, item);
    spans
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
    if let Some(tool_call_id) = item.tool_call_id.as_deref() {
        spans.push(Span::styled(
            format!("  call {tool_call_id}"),
            palette.muted(),
        ));
    }
}

fn push_compact_tool_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    expanded: bool,
) {
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
    let line_limit = if expanded {
        EXPANDED_TOOL_PREVIEW_LINES
    } else {
        COLLAPSED_TOOL_PREVIEW_LINES
    };
    let shown = total.min(line_limit);
    for line in preview_lines.iter().take(shown) {
        lines.push(Line::from(vec![
            Span::styled("     │ ", palette.border()),
            Span::styled(truncate_terminal_line(line, 110), palette.text()),
        ]));
    }
    if total > shown {
        let action = if expanded {
            "Ctrl+O collapse"
        } else {
            "Ctrl+O expand"
        };
        lines.push(Line::from(vec![
            Span::styled("     │ ", palette.border()),
            Span::styled(
                format!("... {} more line(s) hidden ({action})", total - shown),
                palette.muted(),
            ),
        ]));
    } else if expanded && total > COLLAPSED_TOOL_PREVIEW_LINES {
        lines.push(Line::from(vec![
            Span::styled("     │ ", palette.border()),
            Span::styled("expanded (Ctrl+O collapse)", palette.muted()),
        ]));
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
        Span::styled("Diff Preview", palette.title().add_modifier(Modifier::BOLD)),
    ]));

    if let Some(preview) = &diff.preview {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                preview
                    .title
                    .clone()
                    .unwrap_or_else(|| "Inline patch".into()),
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
                Span::styled("No file changes", palette.muted()),
            ]));
        }

        if !preview.files.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    "[/] select hunk | c stage selected diff context",
                    palette.selected(),
                ),
            ]));
        }

        for (file_idx, file) in preview.files.iter().take(1).enumerate() {
            push_diff_file_lines(
                lines,
                palette,
                file_idx,
                diff.selected_file,
                diff.selected_hunk,
                file,
            );
        }
        if preview.files.len() > 1 {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    format!(
                        "+{} more file(s) hidden (Tab inspector)",
                        preview.files.len() - 1
                    ),
                    palette.muted(),
                ),
            ]));
        }
    } else if diff.loading {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled("Loading diff preview...", palette.selected()),
        ]));
    } else if let Some(error) = &diff.error {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(error.clone(), Style::default().fg(palette.danger)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled("No diff preview loaded", palette.muted()),
        ]));
    }
}

fn push_diff_file_lines(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    file_idx: usize,
    selected_file: usize,
    selected_hunk: usize,
    file: &crate::model::DiffPreviewFile,
) {
    let path = match &file.old_path {
        Some(old_path) if old_path != &file.path => format!("{old_path} -> {}", file.path),
        _ => file.path.clone(),
    };
    lines.push(Line::from(vec![
        Span::styled("    ", palette.muted()),
        Span::styled(
            file.status.clone(),
            diff_file_status_style(&file.status, palette),
        ),
        Span::styled("  ", palette.muted()),
        Span::styled(path, palette.text().add_modifier(Modifier::BOLD)),
    ]));

    if file.hunks.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled("line diff unavailable for this mutation", palette.muted()),
        ]));
    }

    for (hunk_idx, hunk) in file.hunks.iter().take(1).enumerate() {
        let selected = file_idx == selected_file && hunk_idx == selected_hunk;
        let marker = if selected { "  › " } else { "    " };
        lines.push(Line::from(vec![
            Span::styled(marker, palette.selected()),
            Span::styled(hunk.header.clone(), diff_hunk_style(palette)),
        ]));
        for line in hunk.lines.iter().take(4) {
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
        if hunk.lines.len() > 4 {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    format!(
                        "{} more diff line(s) hidden (Tab inspector)",
                        hunk.lines.len() - 4
                    ),
                    palette.muted(),
                ),
            ]));
        }
    }
    if file.hunks.len() > 1 {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                format!(
                    "+{} more hunk(s) hidden (Tab inspector)",
                    file.hunks.len() - 1
                ),
                palette.muted(),
            ),
        ]));
    }
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
            Line::from(Span::styled("No active plan", palette.muted())),
            Line::from(Span::styled(
                "Plan text is inferred from assistant/live replies.",
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
        .block(titled_block("Plan", palette, false, Some("live")).border_style(palette.border()))
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
            Span::styled("root ", palette.muted()),
            Span::styled(app.workspace.root.clone(), palette.text()),
        ]),
        Line::from(""),
        Line::from(Span::styled("contract", palette.title())),
    ];

    for line in &app.workspace.contract {
        lines.push(Line::from(Span::styled(
            format!("  {line}"),
            palette.muted(),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("tree", palette.title())));
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
                "Workspace",
                palette,
                app.focus == FocusPane::Workspace,
                Some("contract"),
            )
            .border_style(palette.border()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false })
}

fn render_git(app: &AppState, palette: Palette, area_height: u16) -> Paragraph<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled("branch ", palette.muted()),
        Span::styled(app.git.branch.clone(), palette.text()),
    ])];

    if let Some(head) = &app.git.head {
        lines.push(Line::from(vec![
            Span::styled("head   ", palette.muted()),
            Span::styled(head.clone(), palette.text()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("status", palette.title())));
    let mut selected_idx = 0;
    if app.git.status.is_empty() {
        lines.push(Line::from(Span::styled("  clean", palette.muted())));
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
    lines.push(Line::from(Span::styled("history", palette.title())));
    if app.git.history.is_empty() {
        lines.push(Line::from(Span::styled("  none", palette.muted())));
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
                "Git",
                palette,
                app.focus == FocusPane::Git,
                Some("status/history"),
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
        let parenthetical = format!(
            " ({} · {}/{} tokens)",
            goal.status, goal.tokens_used, goal.token_budget
        );
        lines.push(Line::from(vec![
            Span::styled(
                "◆ ",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
                    .bg(palette.surface),
            ),
            Span::styled("Goal: ", palette.title().bg(palette.surface)),
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
        spans.push(Span::styled(
            "↻ ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .bg(palette.surface),
        ));
        spans.push(Span::styled(
            format!("Loops: {running} running"),
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
    lines
}

fn render_autonomy_indicator(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let lines = autonomy_indicator_lines(app, palette);
    Paragraph::new(Text::from(lines)).style(Style::default().fg(palette.text).bg(palette.surface))
}

/// Default context-window denominator used to render `ctx N%`. The wire does
/// not (yet) carry a per-model context-window max, so we estimate the percent
/// against a common modern default. Surfaces the inspector-only
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
    if DEFAULT_CONTEXT_WINDOW_TOKENS == 0 {
        return None;
    }
    Some((token_estimate as f64 / DEFAULT_CONTEXT_WINDOW_TOKENS as f64).clamp(0.0, 1.0))
}

/// Integer context-window percent (0..=100) for the `ctx N%` label.
fn harness_context_percent(app: &AppState) -> Option<u16> {
    harness_context_ratio(app).map(|ratio| (ratio * 100.0).round() as u16)
}

/// Build the harness status line(s): spinner + phase + agent count +
/// re-entering + token in/out + cost + retry + ctx %. Empty when idle.
fn harness_status_lines(app: &AppState, palette: Palette) -> Vec<Line<'static>> {
    if !harness_status_active(app) {
        return Vec::new();
    }
    let Some(session) = app.active_session() else {
        return Vec::new();
    };
    let session_id = session.id.clone();
    let status = app.orchestration.get(&session_id);

    let phase = match status.and_then(|s| s.phase.as_deref()) {
        Some("orchestrating") => "Orchestrating",
        Some("re-entering") => "Re-entering",
        Some("working") => "Working",
        Some(other) if !other.is_empty() => other,
        _ => "Working",
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        format!("{} ", spinner_frame()),
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD)
            .bg(palette.surface),
    ));
    spans.push(Span::styled(
        phase.to_string(),
        palette.title().bg(palette.surface),
    ));

    if let Some(status) = status {
        if status.running_agents > 0 {
            spans.push(Span::styled(
                format!(
                    " · {} agent{}",
                    status.running_agents,
                    if status.running_agents == 1 { "" } else { "s" }
                ),
                palette.text().bg(palette.surface),
            ));
        }
        // The re-entry gap (sub-agents settled, a continuation queued) is the
        // whole reason for this row: it must NOT read as done.
        if status.pending_continuations > 0 {
            spans.push(Span::styled(
                " · re-entering".to_string(),
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
            (Some(a), Some(max)) => format!(" · retrying (attempt {a}/{max})"),
            (Some(a), None) => format!(" · retrying (attempt {a})"),
            _ => " · retrying".to_string(),
        };
        spans.push(Span::styled(
            attempt,
            palette
                .muted()
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Context window %, mirrored as the textual label alongside the gauge so
    // it survives a narrow terminal (and is unit-testable).
    if let Some(percent) = harness_context_percent(app) {
        // `~` marks this as an estimate: the wire carries no per-model context
        // window, so the percent is against a fixed default denominator
        // (`DEFAULT_CONTEXT_WINDOW_TOKENS`) and is approximate when the real
        // model window differs.
        spans.push(Span::styled(
            format!(" · ctx ~{percent}%"),
            palette.muted().bg(palette.surface),
        ));
    }

    vec![Line::from(spans)]
}

/// Render the dedicated harness status row. Splits the row so the textual
/// status sits on the left and a `LineGauge` context-window bar sits on the
/// right when a `token_estimate` is known. Drawn into its own layout row
/// (never the composer border).
fn render_harness_status_row(frame: &mut Frame<'_>, app: &AppState, palette: Palette, area: Rect) {
    let lines = harness_status_lines(app, palette);
    if lines.is_empty() {
        return;
    }
    let ratio = harness_context_ratio(app);
    // Reserve a fixed-width column for the context gauge only when we have a
    // ratio to show; otherwise the text spans the full width.
    const GAUGE_WIDTH: u16 = 18;
    if let Some(ratio) = ratio.filter(|_| area.width > GAUGE_WIDTH + 12) {
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
            format!(
                "Queued messages ({}) after active turn | Esc interrupt/send | Ctrl+U clear",
                app.pending_messages.len()
            ),
            palette.muted().bg(palette.surface),
        )]));
    } else if matches!(&composer, ComposerPresentation::Collapsed(_)) {
        lines.push(Line::from(vec![Span::styled(
            "Large paste collapsed | Enter sends full text | Ctrl+U clear",
            palette.muted().bg(palette.surface),
        )]));
    } else if let Some(view) = &input_view
        && (view.hidden_lines > 0 || view.hidden_prefix)
    {
        let hidden = if view.hidden_lines > 0 {
            format!("showing tail, {} earlier line(s) hidden", view.hidden_lines)
        } else {
            "showing tail of long line".to_string()
        };
        lines.push(Line::from(vec![Span::styled(
            format!("Multiline input | {hidden} | Enter sends full text | Ctrl+U clear"),
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
                Span::styled(" Onboarding setup...", palette.muted().bg(palette.surface)),
            ]))
        }
        ComposerPresentation::Empty => lines.push(Line::from(vec![
            Span::styled(" › ", palette.selected().bg(palette.surface)),
            Span::styled(
                " Ask Octos to change code...",
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
                Span::styled("   preview: ", palette.muted().bg(palette.surface)),
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

    let block = titled_block(
        "Composer",
        palette,
        app.focus == FocusPane::Composer,
        Some("Enter send | Tab inspector"),
    )
    .border_style(palette.border());
    Paragraph::new(Text::from(lines))
        .style(Style::default().fg(palette.text).bg(palette.surface))
        .block(block)
}

fn set_composer_cursor(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
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
        "read-only"
    } else {
        "interactive"
    };
    let turn = app
        .active_turn()
        .map(|(_, turn_id)| format!("active {}", short_id(&turn_id.0.to_string())))
        .unwrap_or_else(|| "idle".into());
    let profile = app
        .active_session()
        .and_then(|session| session.profile_id.as_deref())
        .unwrap_or("default");
    let cwd = app.workspace.root.as_str();
    let policy = if app.readonly {
        "sends disabled"
    } else {
        "approval gated"
    };
    let context = app
        .active_session()
        .map(|session| {
            format!(
                "{} msgs/{} tasks",
                session.messages.len(),
                session.tasks.len()
            )
        })
        .unwrap_or_else(|| "no session".into());
    let work = status_bar_work_text(app);

    Paragraph::new(Line::from(vec![
        Span::styled(" state ", palette.title().bg(palette.surface_alt)),
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
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(short_path(cwd), palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(
            "Tab inspector | Ctrl+O expand",
            palette.selected().bg(palette.surface_alt),
        ),
    ]))
    .style(Style::default().fg(palette.text).bg(palette.surface_alt))
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
        parts.push(format!("{background_tasks} background task(s)"));
        parts.push("/ps to view".into());
    }
    if app.active_turn().is_some() {
        parts.push("Esc interrupt".into());
        parts.push("/stop to close".into());
    }
    if app.expanded_tool_outputs {
        parts.push("tool output expanded".into());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("({})", parts.join(" | "))
    }
}

fn run_state_status_label(state: &SessionRunState) -> &'static str {
    match state {
        SessionRunState::Idle => "Idle",
        SessionRunState::InProgress => "Working",
        SessionRunState::Blocked { .. } => "Blocked",
        SessionRunState::Success => "Done",
        SessionRunState::Error { .. } => "Error",
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
        SessionRunState::InProgress => "•",
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

fn short_path(path: &str) -> String {
    const MAX_PATH_LEN: usize = 28;
    if path.chars().count() <= MAX_PATH_LEN {
        return path.to_string();
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
            Span::styled("tool ", palette.muted()),
            Span::styled(approval.tool_name.clone(), palette.text()),
        ]),
    ];

    if let Some(kind) = approval.approval_kind.as_ref() {
        let risk = approval
            .risk
            .as_ref()
            .map(|risk| format!("  risk {risk}"))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled("kind ", palette.muted()),
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
                        "command",
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
                        "tool call",
                        command.tool_call_id.as_deref(),
                    );
                }
                if let Some(sandbox) = details.sandbox.as_ref() {
                    push_optional_field(&mut lines, palette, "sandbox", sandbox.mode.as_deref());
                    push_optional_field(
                        &mut lines,
                        palette,
                        "filesystem",
                        sandbox.filesystem_access.as_deref(),
                    );
                    if let Some(network_access) = sandbox.network_access {
                        push_field(&mut lines, palette, "network", network_access.to_string());
                    }
                    if !sandbox.writable_roots.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            "writable",
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
                        "preview",
                        diff.preview_id.0.to_string(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        "operation",
                        diff.operation.as_deref(),
                    );
                    push_optional_field(&mut lines, palette, "summary", diff.summary.as_deref());
                    let stats = [
                        diff.file_count.map(|value| format!("{value} files")),
                        diff.additions.map(|value| format!("+{value}")),
                        diff.deletions.map(|value| format!("-{value}")),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    if !stats.is_empty() {
                        push_field(&mut lines, palette, "stats", stats);
                    }
                }
            }
            approval_kinds::FILESYSTEM => {
                if let Some(filesystem) = details.filesystem.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        "operation",
                        filesystem.operation.clone(),
                    );
                    push_field(
                        &mut lines,
                        palette,
                        "outside workspace",
                        filesystem.outside_workspace.to_string(),
                    );
                    for path in &filesystem.paths {
                        push_field(&mut lines, palette, "path", path.clone());
                    }
                    if !filesystem.writable_roots.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            "writable",
                            filesystem.writable_roots.join(", "),
                        );
                    }
                }
            }
            approval_kinds::NETWORK => {
                if let Some(network) = details.network.as_ref() {
                    push_field(&mut lines, palette, "operation", network.operation.clone());
                    if !network.hosts.is_empty() {
                        push_field(&mut lines, palette, "hosts", network.hosts.join(", "));
                    }
                    if !network.ports.is_empty() {
                        let ports = network
                            .ports
                            .iter()
                            .map(|port| port.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        push_field(&mut lines, palette, "ports", ports);
                    }
                    for url in &network.urls {
                        push_field(&mut lines, palette, "url", url.clone());
                    }
                }
            }
            approval_kinds::SANDBOX_ESCALATION => {
                if let Some(escalation) = details.sandbox_escalation.as_ref() {
                    if let Some(from) = escalation.from.as_ref() {
                        push_optional_field(&mut lines, palette, "from", from.mode.as_deref());
                    }
                    if let Some(to) = escalation.to.as_ref() {
                        push_optional_field(&mut lines, palette, "to", to.mode.as_deref());
                    }
                    if !escalation.requested_permissions.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            "permissions",
                            escalation.requested_permissions.join(", "),
                        );
                    }
                    push_optional_field(
                        &mut lines,
                        palette,
                        "justification",
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
    label: &'static str,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        push_field(lines, palette, label, value.to_string());
    }
}

fn push_field(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    label: &'static str,
    value: String,
) {
    lines.push(Line::from(vec![
        Span::styled(format!("{label} "), palette.muted()),
        Span::styled(value, palette.text()),
    ]));
}

fn render_task_output_modal(
    frame: &mut Frame<'_>,
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
            "No output loaded for this task yet",
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
    let scroll_top = max_scroll.saturating_sub(scroll_from_bottom) as u16;

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block(
                "Task Output",
                palette,
                true,
                Some("o read more | PgUp/PgDn | Esc close"),
            )
            .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn render_artifact_detail_modal(
    frame: &mut Frame<'_>,
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
    let scroll_top = max_scroll.saturating_sub(scroll_from_bottom) as u16;

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block("Artifact", palette, true, Some("PgUp/PgDn | Esc close"))
                .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn render_thread_graph_detail_modal(
    frame: &mut Frame<'_>,
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
    let scroll_top = max_scroll.saturating_sub(scroll_from_bottom) as u16;

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block("Threads", palette, true, Some("PgUp/PgDn | Esc close"))
                .border_style(palette.selected()),
        )
        .scroll((scroll_top, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(pane, area);
}

fn render_turn_state_detail_modal(
    frame: &mut Frame<'_>,
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
    let scroll_top = max_scroll.saturating_sub(scroll_from_bottom) as u16;

    let pane = Paragraph::new(Text::from(lines))
        .block(
            titled_block("Turn", palette, true, Some("PgUp/PgDn | Esc close"))
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
    title: &'a str,
    palette: Palette,
    focused: bool,
    suffix: Option<&'a str>,
) -> Block<'a> {
    let mut spans = vec![Span::styled(title.to_string(), palette.title())];
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
            DiffPreviewHunk, DiffPreviewLine, SessionView,
        },
        store::Store,
    };
    use octos_core::{
        Message, SessionKey,
        ui_protocol::{ApprovalId, PreviewId, TaskRuntimeState, TurnId, UiProtocolCapabilities},
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
    fn render_chat_bubbles_hide_role_titles_and_use_distinct_backgrounds() {
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

        assert_eq!(user_style.bg, Some(palette.diff_context_bg));
        assert_eq!(assistant_style.bg, Some(palette.surface));
        assert_ne!(user_style.bg, assistant_style.bg);
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
        let latest_prompt = row_index_containing(&rows, "› done?");
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
        assert!(text.contains("state • Working"));
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
        let sep = vec!["---"; 8].join("|");
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
                "AppUI connected".into(),
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

    /// M22 (#58): the first-run onboarding surface renders an
    /// ASCII OCTOS wordmark in the preview pane. This pins the
    /// splash so a future refactor cannot quietly drop the
    /// distinctive identity.
    #[test]
    fn render_first_launch_onboarding_includes_ascii_octos_splash() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "AppUI connected".into(),
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

        // ASCII wordmark — at least one characteristic letterform
        // line plus the human-readable label live in the preview.
        assert!(
            text.contains("OCTOS"),
            "expected OCTOS label/wordmark in splash, got:\n{text}"
        );
        assert!(text.contains("Welcome to Octos"));
    }

    #[test]
    fn render_first_launch_onboarding_child_menu_stays_on_onboarding_surface() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "AppUI connected".into(),
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
                "AppUI connected".into(),
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
        assert!(text.contains("Tested"));
        assert!(text.contains("$ cargo test"));
        assert!(text.contains("running 6 tests"));
        assert!(text.contains("1 more line(s) hidden (Ctrl+O expand)"));
        assert!(text.contains("1.2s"));
        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        assert!(text.contains("call call-1"));
        assert!(text.contains("gpt-5-codex"));
        assert!(text.contains("state"));
        assert!(text.contains("running"));
        assert!(text.contains("approval"));
        assert!(text.contains("1 msgs/0 tasks"));
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

        let text = rendered_text(&app);
        let first_prompt = text.find("what is the status").expect("first prompt");
        let latest_prompt = text.find("are you working").expect("latest prompt");
        let command = text.find("$ cargo test").expect("activity command");

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

        let text = rendered_text(&app);
        let prompt = text.find("build the site").expect("user prompt");
        let work_log = text.find("Agent task completed").expect("agent task");
        let command = text.find("$ cargo build").expect("tool command");
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

        let text = rendered_text(&app);

        assert!(text.contains("Waited"));
        assert!(text.contains("20s"));
        assert!(text.contains("Wrote"));
        assert!(text.contains("18ms"));
        assert!(!text.contains("Command  ▸ shell"));
        assert!(!text.contains("Tool  ▸ write_file"));
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

        let text = rendered_text(&app);

        assert!(text.contains("Changed"));
        assert!(text.contains(".../blue-origin/src/pages/index.astro"));
        assert!(text.contains("preview ready"));
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

        let text = rendered_text(&app);

        assert!(text.contains("failed"));
        assert!(text.contains("✗"));
        assert!(text.contains("✓"));
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
        assert!(collapsed.contains("9 more line(s) hidden (Ctrl+O expand)"));
        assert!(!collapsed.contains("line10"));

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
        assert!(text.contains("Loops: 2 running"));
        assert!(text.contains("5m deploy-check"));
        assert!(text.contains("self-paced PR-watch"));
    }

    #[test]
    fn harness_status_row_surfaces_orchestration_usage_and_context() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();

        // Idle: no orchestration, no active turn → row reserves no rows and is
        // absent from the render (so it cannot collide with the composer).
        assert_eq!(harness_status_height(&app), 0);
        assert!(harness_status_lines(&app, Palette::for_theme(ThemeName::Codex)).is_empty());

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
        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex))
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

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex))
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

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex))
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
}
