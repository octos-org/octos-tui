use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use octos_core::ui_protocol::approval_kinds;

use crate::{
    menu::render as menu_render,
    model::{
        ActivityItem, ActivityKind, AppState, ApprovalModalState, ComposerPresentation,
        DiffPreviewPaneState, FocusPane, PlanStep as RenderedPlanStep, SessionRunState,
        SessionView, TaskOutputDetailState, TurnActivityLog, extract_plan_steps, task_state_label,
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
    let surface_budget = frame
        .area()
        .height
        .saturating_sub(min_transcript_height(frame.area().height) + composer_height + 1);
    let menu_height = desired_menu_height.min(surface_budget);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(menu_height),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(render_transcript(app, palette, root[0]), root[0]);
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, root[1], menu, palette);
    }
    frame.render_widget(render_composer(app, palette, root[2]), root[2]);
    set_composer_cursor(frame, app, root[2]);
    frame.render_widget(render_status(app, palette), root[3]);
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
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(12),
            Constraint::Length(menu_height),
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
    frame.render_widget(render_composer(app, palette, root[2]), root[2]);
    set_composer_cursor(frame, app, root[2]);
    frame.render_widget(render_status(app, palette), root[3]);
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
    text.width().max(1).div_ceil(width.max(1))
}

const CODE_BLOCK_LINE_LIMIT: usize = 120;
const COLLAPSED_TOOL_PREVIEW_LINES: usize = 1;
const EXPANDED_TOOL_PREVIEW_LINES: usize = 24;

fn is_running_activity(item: &ActivityItem) -> bool {
    matches!(item.status.as_str(), "running" | "queued") || item.status.ends_with('%')
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
            push_message_block(&mut lines, palette, message.role.as_str(), &message.content);
            if let Some(reasoning) = message.reasoning_content.as_deref() {
                push_message_block(&mut lines, palette, "reasoning", reasoning);
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
                push_turn_flow(&mut lines, palette, app, session);
                turn_flow_rendered = true;
            }
        }

        if !turn_flow_rendered
            && approval_visible
            && let Some(prompt) = latest_user_message(session)
        {
            approval_context_start = Some(lines.len());
            push_recent_user_context(&mut lines, palette, prompt);
            push_turn_flow(&mut lines, palette, app, session);
        } else if !turn_flow_rendered {
            push_turn_flow(&mut lines, palette, app, session);
        }

        if !app.pending_messages.is_empty() {
            push_pending_messages_block(&mut lines, palette, &app.pending_messages);
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No session selected",
            palette.muted(),
        )));
    }

    let wrap_width = transcript_wrap_width(area);
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
) {
    if let Some(approval) = app.approval.as_ref().filter(|approval| approval.visible) {
        push_inline_approval_card(lines, palette, approval);
    }

    if let Some(live_reply) = &session.live_reply {
        push_live_reply_block(lines, palette, &live_reply.text);
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

fn push_recent_user_context(lines: &mut Vec<Line<'static>>, palette: Palette, content: &str) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }
    let bg = chat_message_bg(palette, "user");
    push_formatted_body(lines, palette, content, "› ", Some(bg));
}

fn push_message_block(lines: &mut Vec<Line<'static>>, palette: Palette, role: &str, content: &str) {
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

    push_formatted_body_marked(lines, palette, content, indent, prose_marker, Some(bg));
}

fn push_live_reply_block(lines: &mut Vec<Line<'static>>, palette: Palette, content: &str) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, "assistant");
    push_formatted_body_marked(lines, palette, content, "", Some("• "), Some(bg));
}

fn push_pending_messages_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    pending: &[String],
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
        push_formatted_body(lines, palette, pending, "› ", Some(bg));
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
) {
    push_formatted_body_marked(lines, palette, content, indent, None, bg);
}

fn push_formatted_body_marked(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    content: &str,
    indent: &'static str,
    prose_marker: Option<&'static str>,
    bg: Option<Color>,
) {
    let mut in_code = false;
    let mut code_language: Option<String> = None;
    let mut last_blank = false;
    let mut prose = Vec::new();
    let mut table = Vec::new();
    let mut checkbox_index = 1usize;
    let normalized = content.trim_matches(|ch: char| ch.is_whitespace() && ch != '\n');

    for raw_line in normalized.lines() {
        let line = if in_code { raw_line } else { raw_line.trim() };
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
            flush_markdown_table(lines, palette, &mut table, indent, bg);
            let label = if in_code {
                let language = code_language.take();
                in_code = false;
                language
                    .map(|language| format!("end code {language}"))
                    .unwrap_or_else(|| "end code".to_string())
            } else {
                let language = rest
                    .trim()
                    .split_whitespace()
                    .next()
                    .filter(|language| !language.is_empty())
                    .map(str::to_string);
                code_language = language.clone();
                in_code = true;
                language
                    .map(|language| format!("code {language}"))
                    .unwrap_or_else(|| "code".to_string())
            };
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled(label, style_bg(palette.selected(), bg)),
                    Span::styled(
                        " ------------------------------------------------",
                        style_bg(palette.border(), bg),
                    ),
                ],
                bg,
            ));
            last_blank = false;
            continue;
        }

        if in_code {
            flush_markdown_table(lines, palette, &mut table, indent, bg);
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
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
            flush_markdown_table(lines, palette, &mut table, indent, bg);
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
            flush_markdown_table(lines, palette, &mut table, indent, bg);
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
            flush_markdown_table(lines, palette, &mut table, indent, bg);
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
            flush_markdown_table(lines, palette, &mut table, indent, bg);
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
            flush_markdown_table(lines, palette, &mut table, indent, bg);
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
            flush_markdown_table(lines, palette, &mut table, indent, bg);
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

        flush_markdown_table(lines, palette, &mut table, indent, bg);
        prose.push(line.to_string());
    }

    flush_prose_paragraph(lines, palette, &mut prose, indent, prose_marker, bg);
    flush_markdown_table(lines, palette, &mut table, indent, bg);
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

fn flush_markdown_table(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    table: &mut Vec<Vec<String>>,
    indent: &'static str,
    bg: Option<Color>,
) {
    if table.is_empty() {
        return;
    }

    let col_count = table.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; col_count];
    for row in table.iter() {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(table_cell_width(cell));
        }
    }

    for (row_idx, row) in table.iter().enumerate() {
        let header = row_idx == 0 && table.len() > 1;
        let normal_style = if header {
            style_bg(palette.title().add_modifier(Modifier::BOLD), bg)
        } else {
            style_bg(palette.text(), bg)
        };
        let mut spans = vec![Span::styled(indent, style_bg(palette.border(), bg))];
        for (idx, width) in widths.iter().enumerate() {
            if idx > 0 {
                spans.push(Span::styled(" | ", style_bg(palette.muted(), bg)));
            }
            let cell = row.get(idx).map(String::as_str).unwrap_or("");
            spans.extend(inline_markdown_spans(
                cell,
                normal_style,
                style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                style_bg(palette.selected(), bg),
            ));
            let padding = width.saturating_sub(table_cell_width(cell));
            if padding > 0 {
                spans.push(Span::styled(" ".repeat(padding), normal_style));
            }
        }
        lines.push(chat_line(spans, bg));

        if header {
            let mut separator = String::new();
            for (idx, width) in widths.iter().enumerate() {
                if idx > 0 {
                    separator.push_str("-+-");
                }
                separator.push_str(&"-".repeat((*width).max(3)));
            }
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled(separator, style_bg(palette.muted(), bg)),
                ],
                bg,
            ));
        }
    }
    table.clear();
}

fn table_cell_width(cell: &str) -> usize {
    restore_streamed_sentence_spacing(&plain_inline_markdown(cell))
        .chars()
        .count()
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
    let mut group: Vec<&ActivityItem> = Vec::new();
    let mut last_turn: Option<&octos_core::ui_protocol::TurnId> = None;
    for item in recent.iter().copied() {
        let turn_id = item.turn_id.as_ref();
        if last_turn != turn_id {
            if !group.is_empty() {
                push_agent_task_group(lines, palette, last_turn, &group, app.expanded_tool_outputs);
                group.clear();
            }
            last_turn = turn_id;
        }
        group.push(item);
    }
    if !group.is_empty() {
        push_agent_task_group(lines, palette, last_turn, &group, app.expanded_tool_outputs);
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

fn flow_activity_items(app: &AppState) -> Vec<&ActivityItem> {
    let active_turn_id = app.active_turn().map(|(_, turn_id)| turn_id);
    app.activity
        .iter()
        .filter(|item| match active_turn_id {
            Some(turn_id) => item.turn_id.as_ref() == Some(turn_id),
            None => item.turn_id.is_none(),
        })
        .collect()
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
    let shown = log
        .items
        .iter()
        .rev()
        .take(shown_limit)
        .rev()
        .collect::<Vec<_>>();
    push_agent_task_group(
        lines,
        palette,
        Some(&log.turn_id),
        &shown,
        app.expanded_tool_outputs,
    );
    if log.items.len() > shown.len() {
        let hidden = log.items.len() - shown.len();
        let completed = log
            .items
            .iter()
            .filter(|item| activity_is_completed(item))
            .count();
        let active = log
            .items
            .iter()
            .filter(|item| is_running_activity(item))
            .count();
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

fn push_agent_task_group(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    turn_id: Option<&octos_core::ui_protocol::TurnId>,
    items: &[&ActivityItem],
    expanded: bool,
) {
    if items.is_empty() {
        return;
    }
    let active = items
        .iter()
        .filter(|item| is_running_activity(item))
        .count();
    let completed = items
        .iter()
        .filter(|item| activity_is_completed(item))
        .count();
    let failed = items.iter().filter(|item| activity_is_failed(item)).count();
    let title = if active > 0 {
        "Orchestrating..."
    } else if failed > 0 {
        "Agent task finished with errors"
    } else {
        "Agent task completed"
    };
    let mut metadata = vec![format!("{} action(s)", items.len())];
    if active > 0 {
        metadata.push(format!("{active} active"));
    }
    if completed > 0 {
        metadata.push(format!("{completed} completed"));
    }
    if failed > 0 {
        metadata.push(format!("{failed} failed"));
    }
    if let Some(turn_id) = turn_id {
        metadata.push(format!("turn {}", short_id(&turn_id.0.to_string())));
    }

    let spans = vec![
        Span::styled("• ", palette.selected()),
        Span::styled(title, palette.title().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" ({})", metadata.join(" · ")), palette.muted()),
    ];
    lines.push(Line::from(spans));

    for (idx, item) in items.iter().enumerate() {
        push_agent_task_child(lines, palette, item, idx == 0, expanded);
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
        ("◻", palette.selected())
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

fn push_inline_diff_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    diff: &DiffPreviewPaneState,
) {
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
                for (index, line) in view.lines.iter().enumerate() {
                    let prefix = if index == 0 { " › " } else { "   " };
                    let prefix_style = if index == 0 {
                        palette.selected().bg(palette.surface)
                    } else {
                        palette.muted().bg(palette.surface)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, prefix_style),
                        Span::styled(line.clone(), palette.text().bg(palette.surface)),
                    ]));
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

    Paragraph::new(Text::from(lines))
        .style(Style::default().fg(palette.text).bg(palette.surface))
        .block(
            titled_block(
                "Composer",
                palette,
                app.focus == FocusPane::Composer,
                Some("Enter send | Tab inspector"),
            )
            .border_style(palette.border()),
        )
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
    let max_width = width.saturating_mul(max_rows).max(1);
    let prefix = &text[..cursor.min(text.len())];
    let prefix_width = prefix.width();
    if text.width() <= max_width || prefix_width < max_width {
        return VisibleCursorLine {
            text: text.to_string(),
            before_cursor: prefix.to_string(),
        };
    }

    let suffix_width = max_width.saturating_sub(3).max(1);
    let before_cursor = suffix_by_display_width(prefix, suffix_width);
    let text = format!("...{before_cursor}");
    VisibleCursorLine {
        text: text.clone(),
        before_cursor: text,
    }
}

fn cursor_row_for_text(text: &str, width: usize) -> usize {
    let display_width = text.width();
    if display_width == 0 {
        0
    } else {
        (display_width - 1) / width.max(1)
    }
}

fn cursor_width_for_text(text: &str, width: usize) -> usize {
    let display_width = text.width();
    if display_width == 0 {
        0
    } else {
        ((display_width - 1) % width.max(1)) + 1
    }
}

fn suffix_by_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut width = 0usize;
    let mut chars = Vec::new();
    for ch in text.chars().rev() {
        let ch_width = ch.width().unwrap_or(0);
        if width > 0 && width.saturating_add(ch_width) > max_width {
            break;
        }
        width = width.saturating_add(ch_width);
        chars.push(ch);
        if width >= max_width {
            break;
        }
    }
    chars.into_iter().rev().collect()
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
                    hunks: Vec::new(),
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
                    hunks: Vec::new(),
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
        assert_eq!(table.find("Page"), Some(0));
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
        assert!(text.contains(" | "));
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
        assert!(header.contains(" | "));
        assert!(hero.contains("Hero.astro"));
        assert!(hero.contains(" | "));
        assert!(!rows.join("\n").contains("|---|---|---|"));
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
    fn render_code_fences_show_language_and_bound_long_lines() {
        let palette = Palette::for_theme(ThemeName::Codex);
        let long_code = format!(
            "let value = \"{}TAIL_UNIQUE_SHOULD_NOT_RENDER\";",
            "x".repeat(180)
        );
        let content = format!("```rust\n{long_code}\n```");
        let mut lines = Vec::new();

        push_formatted_body(&mut lines, palette, &content, "", Some(palette.surface));

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("code rust"));
        assert!(text.contains("end code rust"));
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
}
