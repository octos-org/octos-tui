use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use octos_core::ui_protocol::approval_kinds;

use crate::{
    menu::render as menu_render,
    model::{
        ActivityItem, ActivityKind, AppState, ApprovalModalState, DiffPreviewPaneState, FocusPane,
        PlanStep as RenderedPlanStep, SessionRunState, SessionView, TaskOutputDetailState,
        extract_plan_steps, task_state_label,
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
    let composer_height = composer_height(app);
    let active_menu = active_menu_surface(app);
    let banner_height = router_failover_banner_height(app);
    let desired_menu_height = menu_height_hint(
        active_menu.as_ref(),
        frame.area().width,
        frame.area().height,
    );
    let desired_work_height = sticky_work_height(app);
    let mut surface_budget = frame.area().height.saturating_sub(
        min_transcript_height(frame.area().height) + composer_height + banner_height + 1,
    );
    let menu_height = desired_menu_height.min(surface_budget);
    surface_budget = surface_budget.saturating_sub(menu_height);
    let work_height = desired_work_height.min(surface_budget);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(work_height),
            Constraint::Length(menu_height),
            Constraint::Length(banner_height),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(render_transcript(app, palette, root[0].height), root[0]);
    frame.render_widget(render_work_pane(app, palette, root[1].height), root[1]);
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, root[2], menu, palette);
    }
    if banner_height > 0 {
        frame.render_widget(render_router_failover_banner(app, palette), root[3]);
    }
    frame.render_widget(render_composer(app, palette), root[4]);
    set_composer_cursor(frame, app, root[4]);
    frame.render_widget(render_status(app, palette), root[5]);
}

/// Wave4-B2: returns 1 when a transient failover banner is active (and not
/// yet expired), 0 otherwise. The chat / inspector layouts use this to
/// budget an extra row above the composer without disturbing the existing
/// flow when no banner is showing.
fn router_failover_banner_height(app: &AppState) -> u16 {
    match &app.router_failover_banner {
        Some(banner) if !banner.is_expired(std::time::Instant::now()) => 1,
        _ => 0,
    }
}

/// Wave4-B2: render the one-line failover banner using the existing
/// status-bar surface conventions (muted bg, highlight foreground).
fn render_router_failover_banner(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let text = app
        .router_failover_banner
        .as_ref()
        .map(|banner| banner.render())
        .unwrap_or_default();
    Paragraph::new(Line::from(vec![Span::styled(
        format!(" {text}"),
        Style::default()
            .fg(palette.highlight)
            .bg(palette.surface_alt),
    )]))
    .style(
        Style::default()
            .fg(palette.highlight)
            .bg(palette.surface_alt),
    )
}

fn min_transcript_height(terminal_height: u16) -> u16 {
    if terminal_height < 30 { 8 } else { 12 }
}

fn render_inspector_layout(frame: &mut Frame<'_>, app: &AppState, palette: Palette) {
    let composer_height = composer_height(app);
    let active_menu = active_menu_surface(app);
    let banner_height = router_failover_banner_height(app);
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
            Constraint::Length(banner_height),
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
    frame.render_widget(render_transcript(app, palette, upper[1].height), upper[1]);
    frame.render_widget(render_plan(app, palette), right[0]);
    frame.render_widget(render_workspace(app, palette, right[1].height), right[1]);
    frame.render_widget(render_git(app, palette, right[2].height), right[2]);
    if let Some(menu) = active_menu.as_ref() {
        menu_render::render_menu_surface(frame, root[1], menu, palette);
    }
    if banner_height > 0 {
        frame.render_widget(render_router_failover_banner(app, palette), root[2]);
    }
    frame.render_widget(render_composer(app, palette), root[3]);
    set_composer_cursor(frame, app, root[3]);
    frame.render_widget(render_status(app, palette), root[4]);
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

fn composer_height(app: &AppState) -> u16 {
    let _ = app;
    5
}

fn sticky_work_height(app: &AppState) -> u16 {
    let plan_len = extract_plan_lines(app).len();
    let approval_rows = if app
        .approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
    {
        5 + u16::from(latest_user_prompt(app).is_some())
    } else {
        0
    };
    if approval_rows > 0 {
        return (4 + approval_rows + u16::from(plan_len > 0)).min(9);
    }

    if plan_len == 0 {
        if should_show_work_request(app) { 4 } else { 3 }
    } else if app.run_state.is_active() {
        if should_show_work_request(app) { 6 } else { 5 }
    } else {
        4
    }
}

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

fn render_transcript(app: &AppState, palette: Palette, area_height: u16) -> Paragraph<'static> {
    let mut lines = Vec::new();
    let mut approval_context_start = None;

    if let Some(session) = app.active_session() {
        for message in &session.messages {
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
        }

        if !app.activity.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "Activity",
                palette.title().add_modifier(Modifier::BOLD),
            )));
            for item in app.activity.iter().rev().take(8).rev() {
                push_activity_row(&mut lines, palette, item, app.expanded_tool_outputs);
            }
        }

        let approval_visible = app
            .approval
            .as_ref()
            .is_some_and(|approval| approval.visible);
        if approval_visible && let Some(prompt) = latest_user_message(session) {
            approval_context_start = Some(lines.len());
            push_recent_user_context(&mut lines, palette, prompt);
        } else if should_pin_recent_user_context(app, session)
            && let Some(prompt) = latest_user_message(session)
        {
            approval_context_start = Some(lines.len());
            push_recent_user_context(&mut lines, palette, prompt);
        }

        if let Some(approval) = app.approval.as_ref().filter(|approval| approval.visible) {
            push_inline_approval_card(&mut lines, palette, approval);
        }

        push_inline_progress_card(&mut lines, palette, app);

        let plan = extract_plan_lines(app);
        if !plan.is_empty() {
            push_inline_plan_card(&mut lines, palette, plan);
        }

        if app.diff_preview.active {
            push_inline_diff_preview(&mut lines, palette, &app.diff_preview);
        }

        if let Some(live_reply) = &session.live_reply {
            push_live_reply_block(&mut lines, palette, &live_reply.text);
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

    let visible_height = usize::from(area_height.saturating_sub(2)).max(1);
    let max_scroll = lines.len().saturating_sub(visible_height);
    let scroll_from_bottom = app.transcript_scroll.min(max_scroll);
    let mut scroll_top = max_scroll.saturating_sub(scroll_from_bottom);
    if scroll_from_bottom == 0
        && let Some(context_start) = approval_context_start
    {
        scroll_top = scroll_top.min(context_start);
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

fn latest_user_message(session: &SessionView) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role.as_str() == "user")
        .map(|message| message.content.as_str())
        .filter(|content| !content.trim().is_empty())
}

fn latest_user_prompt(app: &AppState) -> Option<&str> {
    app.active_session().and_then(latest_user_message)
}

fn should_pin_recent_user_context(app: &AppState, session: &SessionView) -> bool {
    session.live_reply.is_some()
        || app.diff_preview.active
        || app.active_turn().is_some()
        || app.run_state.is_active()
        || !app.activity.is_empty()
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

    if content.is_empty() {
        lines.push(chat_line(
            vec![Span::styled("<empty>", palette.muted().bg(bg))],
            Some(bg),
        ));
        return;
    }

    push_formatted_body(lines, palette, content, indent, Some(bg));
}

fn push_live_reply_block(lines: &mut Vec<Line<'static>>, palette: Palette, content: &str) {
    if !lines.is_empty() && !line_is_blank(lines.last()) {
        lines.push(Line::from(""));
    }

    let bg = chat_message_bg(palette, "assistant");
    lines.push(chat_line(
        vec![Span::styled(active_spinner(), palette.selected().bg(bg))],
        Some(bg),
    ));
    push_formatted_body(lines, palette, content, "", Some(bg));
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
    let mut in_code = false;
    let mut last_blank = false;
    let normalized = content.trim_matches(|ch: char| ch.is_whitespace() && ch != '\n');

    for line in normalized.lines() {
        let line = if in_code { line } else { line.trim() };
        if line.is_empty() {
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

        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled("code", style_bg(palette.selected(), bg)),
                    Span::styled(
                        " ------------------------------------------------",
                        style_bg(palette.border(), bg),
                    ),
                ],
                bg,
            ));
            continue;
        }

        if let Some(command) = shell_command_from_line(line) {
            push_command_row(lines, palette, indent, command);
            continue;
        }

        if in_code {
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled(line.to_string(), style_bg(palette.muted(), bg)),
                ],
                bg,
            ));
            continue;
        }

        if markdown_table_separator(line) {
            continue;
        }

        if let Some(cells) = markdown_table_cells(line) {
            let mut spans = vec![Span::styled(indent, style_bg(palette.border(), bg))];
            for (idx, cell) in cells.into_iter().enumerate() {
                if idx > 0 {
                    spans.push(Span::styled("  ", style_bg(palette.muted(), bg)));
                }
                spans.extend(inline_markdown_spans(
                    cell,
                    style_bg(palette.text(), bg),
                    style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
                    style_bg(palette.selected(), bg),
                ));
            }
            lines.push(chat_line(spans, bg));
            continue;
        }

        if let Some(heading) = markdown_heading(line) {
            lines.push(chat_line(
                vec![
                    Span::styled(indent, style_bg(palette.border(), bg)),
                    Span::styled(
                        heading.to_string(),
                        style_bg(palette.title(), bg).add_modifier(Modifier::BOLD),
                    ),
                ],
                bg,
            ));
            continue;
        }

        if let Some((checked, text)) = markdown_checkbox(line) {
            let marker = if checked { "[x]" } else { "[ ]" };
            let mut spans = vec![
                Span::styled(indent, style_bg(palette.border(), bg)),
                Span::styled(marker, style_bg(palette.selected(), bg)),
                Span::styled(" ", style_bg(palette.muted(), bg)),
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

        if let Some(text) = markdown_bullet(line) {
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

        let mut spans = vec![Span::styled(indent, style_bg(palette.border(), bg))];
        spans.extend(inline_markdown_spans(
            line,
            style_bg(palette.text(), bg),
            style_bg(palette.title().add_modifier(Modifier::BOLD), bg),
            style_bg(palette.selected(), bg),
        ));
        lines.push(chat_line(spans, bg));
    }
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

fn line_is_blank(line: Option<&Line<'static>>) -> bool {
    line.map(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        .unwrap_or(false)
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let heading = trimmed
        .strip_prefix("### ")
        .or_else(|| trimmed.strip_prefix("## "))
        .or_else(|| trimmed.strip_prefix("# "))?;
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

        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        let next = [next_bold, next_code].into_iter().flatten().min();
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
        if matches!(ch, '.' | '!' | '?')
            && chars.peek().is_some_and(|next| next.is_ascii_uppercase())
            && repaired
                .chars()
                .rev()
                .nth(1)
                .is_some_and(|prev| prev.is_ascii_lowercase() || prev == ')')
        {
            repaired.push(' ');
        }
    }

    repaired
}

fn push_activity_row(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    expanded: bool,
) {
    if item.kind == ActivityKind::Tool {
        push_command_output_block(lines, palette, item, expanded);
        return;
    }
    if let Some(mutation) = FileMutationActivity::from_item(item) {
        push_file_mutation_block(lines, palette, item, mutation);
        return;
    }

    let kind_style = match item.kind {
        ActivityKind::Tool => palette.selected(),
        ActivityKind::Progress => palette.title(),
        ActivityKind::Approval => palette.selected(),
        ActivityKind::Warning | ActivityKind::Error => Style::default().fg(palette.danger),
    };
    let running =
        matches!(item.status.as_str(), "running" | "queued") || item.status.ends_with('%');
    let marker = if running { active_spinner() } else { "▸" };
    let detail = item
        .detail
        .as_deref()
        .filter(|detail| !detail.is_empty())
        .map(|detail| format!("  {detail}"))
        .unwrap_or_default();

    lines.push(Line::from(vec![
        Span::styled(format!("  {marker} "), palette.selected()),
        Span::styled(
            format!("[{}] ", item.kind.label()),
            kind_style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(item.title.clone(), palette.text()),
        Span::styled(format!("  {}", item.status), palette.muted()),
        Span::styled(detail, palette.muted()),
    ]));

    let mut metadata = Vec::new();
    if let Some(tool_call_id) = item.tool_call_id.as_deref() {
        metadata.push(format!("call {tool_call_id}"));
    }
    if let Some(turn_id) = item.turn_id.as_ref() {
        metadata.push(format!("turn {}", turn_id.0));
    }
    if let Some(detail) = item.detail.as_deref() {
        metadata.push(detail.to_string());
    }
    if !metadata.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("      ", palette.border()),
            Span::styled(metadata.join(" | "), palette.muted()),
        ]));
    }
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

fn push_file_mutation_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    mutation: FileMutationActivity,
) {
    let action = file_mutation_action_label(&mutation.operation);
    let compact_path = compact_file_path(&mutation.path);
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled(action, palette.title().add_modifier(Modifier::BOLD)),
        Span::styled("  ", palette.muted()),
        Span::styled("▸", palette.selected()),
        Span::styled(" ", palette.muted()),
        Span::styled(compact_path, palette.text().add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", mutation.operation), palette.muted()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("    › ", palette.selected().bg(palette.surface)),
        Span::styled(mutation.path, palette.text().bg(palette.surface)),
    ]));
    if mutation.preview_ready {
        lines.push(Line::from(vec![
            Span::styled("    diff ", palette.muted()),
            Span::styled("preview ready", palette.selected()),
        ]));
    }

    let mut metadata = Vec::new();
    if let Some(turn_id) = item.turn_id.as_ref() {
        metadata.push(format!("turn {}", turn_id.0));
    }
    if !metadata.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.border()),
            Span::styled(metadata.join(" | "), palette.muted()),
        ]));
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

fn push_command_output_block(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    item: &ActivityItem,
    expanded: bool,
) {
    let running = is_running_activity(item);
    let marker = if running { active_spinner() } else { "▸" };
    let state = tool_block_state_label(item, running);
    let state_style = tool_block_state_style(item, running, palette);
    let duration = item
        .duration_ms
        .map(format_duration_ms)
        .map(|duration| format!("  {duration}"))
        .unwrap_or_default();
    let action_label = tool_action_label(item, running);
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled(action_label, palette.title().add_modifier(Modifier::BOLD)),
        Span::styled("  ", palette.muted()),
        Span::styled(marker, palette.selected()),
        Span::styled(" ", palette.muted()),
        Span::styled(
            item.title.clone(),
            palette.text().add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", palette.muted()),
        Span::styled(state, state_style),
        Span::styled(duration, palette.muted()),
    ]));

    if let Some(invocation) = tool_invocation_text(item) {
        let prompt = if item.title == "shell" { "$ " } else { "› " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("    {prompt}"),
                palette.selected().bg(palette.surface),
            ),
            Span::styled(invocation, palette.text().bg(palette.surface)),
        ]));
    }

    let exit_label = match (item.status.as_str(), item.success) {
        (_, Some(true)) => "exit 0",
        (_, Some(false)) => "failed",
        ("complete" | "completed", _) => "exit 0",
        ("failed" | "error", _) => "failed",
        (status, _) => status,
    };
    lines.push(Line::from(vec![
        Span::styled("    output ", palette.muted()),
        Span::styled(
            exit_label.to_string(),
            command_status_style(item.status.as_str(), palette),
        ),
        Span::styled("  inline preview", palette.muted()),
    ]));

    if let Some(output_preview) = item
        .output_preview
        .as_deref()
        .filter(|output| !output.trim().is_empty())
    {
        push_tool_output_preview(lines, palette, output_preview, expanded);
    } else if !running {
        let collapsed_detail = collapsed_tool_detail(item);
        lines.push(Line::from(vec![
            Span::styled("    │ ", palette.border()),
            Span::styled(collapsed_detail, palette.muted()),
        ]));
    }

    let mut metadata = Vec::new();
    if let Some(tool_call_id) = item.tool_call_id.as_deref() {
        metadata.push(format!("call {tool_call_id}"));
    }
    if let Some(turn_id) = item.turn_id.as_ref() {
        metadata.push(format!("turn {}", turn_id.0));
    }
    if let Some(duration_ms) = item.duration_ms {
        metadata.push(format!("elapsed {}", format_duration_ms(duration_ms)));
    }
    if !metadata.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.border()),
            Span::styled(metadata.join(" | "), palette.muted()),
        ]));
    }
}

fn collapsed_tool_detail(item: &ActivityItem) -> String {
    let hint = " (Ctrl+O expand | Tab inspector)";
    if matches!(item.success, Some(false)) || matches!(item.status.as_str(), "failed" | "error") {
        if let Some(detail) = item
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
        {
            return format!("failed: {}{hint}", first_meaningful_line(detail));
        }
    }
    format!("details collapsed{hint}")
}

fn tool_block_state_label(item: &ActivityItem, running: bool) -> &'static str {
    if running {
        return "running";
    }
    if matches!(item.success, Some(false)) || matches!(item.status.as_str(), "failed" | "error") {
        return "failed";
    }
    if item
        .output_preview
        .as_deref()
        .is_some_and(|output| !output.trim().is_empty())
    {
        return "preview";
    }
    "collapsed"
}

fn tool_block_state_style(item: &ActivityItem, running: bool, palette: Palette) -> Style {
    match tool_block_state_label(item, running) {
        "running" => palette.selected(),
        "failed" => Style::default().fg(palette.danger),
        "preview" => Style::default().fg(palette.success),
        _ => palette.muted(),
    }
}

fn tool_invocation_text(item: &ActivityItem) -> Option<String> {
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        return Some(detail.to_string());
    }
    item.arguments
        .as_ref()
        .and_then(|arguments| serde_json::to_string(arguments).ok())
}

fn push_tool_output_preview(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    output_preview: &str,
    expanded: bool,
) {
    const MAX_PREVIEW_LINES: usize = 8;
    let meaningful = meaningful_output_lines(output_preview);
    let preview_lines = if meaningful.is_empty() {
        output_preview.lines().collect::<Vec<_>>()
    } else {
        meaningful
    };
    let total = preview_lines.len();
    let shown = if expanded { total } else { total.min(1) };
    for line in preview_lines.iter().take(shown) {
        lines.push(Line::from(vec![
            Span::styled("    │ ", palette.border()),
            Span::styled((*line).to_string(), palette.text()),
        ]));
    }
    if total > shown {
        lines.push(Line::from(vec![
            Span::styled("    │ ", palette.border()),
            Span::styled(
                format!("... {} more line(s) hidden (Ctrl+O expand)", total - shown),
                palette.muted(),
            ),
        ]));
    } else if expanded && total > MAX_PREVIEW_LINES {
        lines.push(Line::from(vec![
            Span::styled("    │ ", palette.border()),
            Span::styled("expanded (Ctrl+O collapse)", palette.muted()),
        ]));
    }
}

fn meaningful_output_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn first_meaningful_line(output: &str) -> &str {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| output.trim())
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

fn command_status_style(status: &str, palette: Palette) -> Style {
    match status {
        "complete" | "completed" => Style::default().fg(palette.success),
        "failed" | "error" => Style::default().fg(palette.danger),
        "running" | "queued" => palette.selected(),
        _ if status.ends_with('%') => palette.selected(),
        _ => palette.muted(),
    }
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

fn push_inline_progress_card(lines: &mut Vec<Line<'static>>, palette: Palette, app: &AppState) {
    let running = app
        .activity
        .iter()
        .rev()
        .filter(|item| is_running_activity(item))
        .take(3)
        .collect::<Vec<_>>();
    if running.is_empty() && app.active_turn().is_none() {
        return;
    }
    let status_text = if running.is_empty() {
        "Thinking".to_string()
    } else {
        app.status.clone()
    };

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled("Progress", palette.title().add_modifier(Modifier::BOLD)),
        Span::styled("  ", palette.muted()),
        Span::styled(active_spinner(), palette.selected()),
        Span::styled(" ", palette.muted()),
        Span::styled(status_text, palette.text()),
    ]));

    for item in running.into_iter().rev() {
        let detail = item
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
            .unwrap_or(item.status.as_str());
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                item.title.clone(),
                palette.text().add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", palette.muted()),
            Span::styled(detail.to_string(), palette.muted()),
        ]));
    }
}

fn push_inline_plan_card(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    plan: Vec<RenderedPlanStep>,
) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ", palette.muted()),
        Span::styled("Plan", palette.title().add_modifier(Modifier::BOLD)),
        Span::styled("  live", palette.muted()),
    ]));
    for (idx, step) in plan.into_iter().take(6).enumerate() {
        let status = if step.completed { "[x]" } else { "[ ]" };
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(status, palette.selected()),
            Span::styled(format!(" {}. ", idx + 1), palette.muted()),
            Span::styled(step.text, palette.text()),
        ]));
    }
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

fn render_work_pane(app: &AppState, palette: Palette, area_height: u16) -> Paragraph<'static> {
    let mut lines = vec![work_status_line(app, palette)];
    let plan = extract_plan_lines(app);
    let inner_height = usize::from(area_height.saturating_sub(2)).max(1);

    if let Some(approval) = app.approval.as_ref().filter(|approval| approval.visible) {
        if let Some(prompt) = latest_user_prompt(app) {
            lines.push(Line::from(vec![
                Span::styled("Request ", palette.title()),
                Span::styled(compact_inline(prompt, 96), palette.text()),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Approval ", palette.title()),
            Span::styled(approval.title.clone(), palette.text()),
        ]));
        for action in approval_action_labels(approval) {
            lines.push(Line::from(vec![
                Span::styled("  ", palette.muted()),
                Span::styled(action, palette.selected()),
            ]));
        }
    } else if should_show_work_request(app)
        && let Some(prompt) = latest_user_prompt(app)
    {
        lines.push(Line::from(vec![
            Span::styled("Request ", palette.title()),
            Span::styled(compact_inline(prompt, 96), palette.text()),
        ]));
    }

    if plan.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Plan ", palette.title()),
            Span::styled("No active plan", palette.muted()),
        ]));
    } else if inner_height <= lines.len() + 2 {
        let total = plan.len();
        let (idx, step) = plan
            .iter()
            .enumerate()
            .find(|(_, step)| !step.completed)
            .unwrap_or((0, &plan[0]));
        let status = if step.completed { "[x]" } else { "[ ]" };
        let hidden = total.saturating_sub(1);
        let suffix = if hidden > 0 {
            format!(" | +{hidden} more plan item(s) | Ctrl+O expand")
        } else {
            String::new()
        };
        if lines.len() < inner_height {
            lines.push(Line::from(vec![
                Span::styled("Plan ", palette.title()),
                Span::styled(status, palette.selected()),
                Span::styled(format!(" {}. ", idx + 1), palette.muted()),
                Span::styled(compact_inline(&step.text, 72), palette.text()),
                Span::styled(suffix, palette.muted()),
            ]));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("Plan ", palette.title()),
            Span::styled("live", palette.muted()),
        ]));
        let reserved_for_hint = usize::from(plan.len() > 1);
        let available_plan_rows = inner_height
            .saturating_sub(lines.len())
            .saturating_sub(reserved_for_hint)
            .max(1);
        let shown = plan.len().min(4).min(available_plan_rows);
        let total = plan.len();
        for (idx, step) in plan.into_iter().take(shown).enumerate() {
            let status = if step.completed { "[x]" } else { "[ ]" };
            lines.push(Line::from(vec![
                Span::styled("  ", palette.muted()),
                Span::styled(status, palette.selected()),
                Span::styled(format!(" {}. ", idx + 1), palette.muted()),
                Span::styled(step.text, palette.text()),
            ]));
        }
        let hidden = total.saturating_sub(shown);
        if hidden > 0 {
            lines.push(Line::from(vec![
                Span::styled("  ", palette.muted()),
                Span::styled(
                    format!("+{hidden} more plan item(s) | Ctrl+O expand | Tab inspector"),
                    palette.muted(),
                ),
            ]));
        }
    }

    Paragraph::new(Text::from(lines))
        .block(titled_block("Work", palette, false, Some("sticky")).border_style(palette.border()))
        .wrap(Wrap { trim: false })
}

fn work_status_line(app: &AppState, palette: Palette) -> Line<'static> {
    let background_tasks = active_background_tasks(app);
    let elapsed = app
        .run_state_elapsed_secs()
        .map(|secs| format!(" {} ", format_elapsed_secs(secs)))
        .unwrap_or_else(|| " ".into());
    let interrupt = if app.active_turn().is_some() {
        " | Esc interrupt | /stop to close"
    } else {
        ""
    };
    let task_hint = if background_tasks == 0 {
        String::new()
    } else {
        format!(" | {background_tasks} background task(s) | /ps to view")
    };
    let detail = current_goal_text(app);

    Line::from(vec![
        Span::styled("Task ", palette.title()),
        Span::styled(
            run_state_status_label(&app.run_state).to_string(),
            run_state_style(&app.run_state, palette),
        ),
        Span::styled(elapsed, palette.muted()),
        Span::styled(detail, palette.text()),
        Span::styled(task_hint, palette.muted()),
        Span::styled(interrupt.to_string(), palette.muted()),
    ])
}

fn current_goal_text(app: &AppState) -> String {
    if let Some(detail) = app.run_state.detail() {
        return detail.to_string();
    }
    if let Some(item) = app
        .activity
        .iter()
        .rev()
        .find(|item| is_running_activity(item))
    {
        return item
            .detail
            .as_ref()
            .filter(|detail| !detail.is_empty())
            .cloned()
            .unwrap_or_else(|| item.title.clone());
    }
    app.active_task()
        .map(|task| task.title.clone())
        .unwrap_or_else(|| app.status.clone())
}

fn should_show_work_request(app: &AppState) -> bool {
    latest_user_prompt(app).is_some()
        && (app.run_state.is_active()
            || app.active_turn().is_some()
            || app.diff_preview.active
            || !app.activity.is_empty())
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
                let status = if step.completed { "[x]" } else { "[ ]" };
                Line::from(vec![
                    Span::styled(format!("{status} "), palette.selected()),
                    Span::styled(format!("{}.", idx + 1), palette.muted()),
                    Span::styled(format!(" {}", step.text), palette.text()),
                ])
            })
            .collect()
    };

    Paragraph::new(Text::from(lines))
        .block(titled_block("Plan", palette, false, Some("live")).border_style(palette.border()))
        .wrap(Wrap { trim: false })
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

fn render_composer(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if !app.pending_messages.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            format!(
                "Queued messages ({}) after active turn | Esc interrupt/send | Ctrl+U clear",
                app.pending_messages.len()
            ),
            palette.muted().bg(palette.surface),
        )]));
    } else {
        lines.push(Line::from(Span::styled(
            " ",
            palette.text().bg(palette.surface),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled(" › ", palette.selected().bg(palette.surface)),
        Span::styled(app.composer.clone(), palette.text().bg(palette.surface)),
        if app.composer.is_empty() {
            Span::styled(
                " Ask Octos to change code...",
                palette.muted().bg(palette.surface),
            )
        } else {
            Span::styled("", palette.text().bg(palette.surface))
        },
    ]));
    lines.push(Line::from(Span::styled(
        " ",
        palette.text().bg(palette.surface),
    )));

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
        .wrap(Wrap { trim: false })
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

    let input_y = area.y
        + if app.pending_messages.is_empty() {
            3
        } else {
            2
        };
    if input_y >= area.y + area.height.saturating_sub(1) {
        return None;
    }

    let text_width = app.composer.chars().count() as u16;
    let inner_right = area.x + area.width.saturating_sub(2);
    let input_x = area.x + 4 + text_width;
    Some(Position::new(input_x.min(inner_right), input_y))
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

    let mut spans: Vec<Span<'static>> = vec![
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
    ];
    spans.extend(router_status_bar_spans(app, palette));
    spans.extend(vec![
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
    ]);

    Paragraph::new(Line::from(spans))
        .style(Style::default().fg(palette.text).bg(palette.surface_alt))
}

/// Wave4-B2: build the styled spans that render the router pill inside
/// [`render_status`]. Returns an empty Vec when no router state is
/// observed (cold start), so existing layouts stay byte-identical for
/// non-router profiles.
fn router_status_bar_spans(app: &AppState, palette: Palette) -> Vec<Span<'static>> {
    let Some(router) = app.router.as_ref() else {
        return Vec::new();
    };
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
    spans.push(Span::styled(" | ", palette.muted().bg(palette.surface_alt)));
    spans.push(Span::styled(
        router.provider_name.clone(),
        palette.text().bg(palette.surface_alt),
    ));
    spans.push(Span::styled(" ", palette.muted().bg(palette.surface_alt)));
    let badge_style = if router.mode.is_active() {
        palette.title().bg(palette.surface_alt)
    } else {
        palette.muted().bg(palette.surface_alt)
    };
    spans.push(Span::styled(router.mode.badge().to_string(), badge_style));
    if let Some(pending) = app.pending_router_mode.as_deref() {
        let pending_mode = crate::model::RouterMode::from_wire(pending);
        if pending_mode != router.mode {
            spans.push(Span::styled(" → ", palette.muted().bg(palette.surface_alt)));
            spans.push(Span::styled(
                pending_mode.badge().to_string(),
                palette.selected().bg(palette.surface_alt),
            ));
        }
    }
    if app.queue_depth > 0 {
        spans.push(Span::styled(" ", palette.muted().bg(palette.surface_alt)));
        spans.push(Span::styled(
            format!("↳{}", app.queue_depth),
            palette.title().bg(palette.surface_alt),
        ));
    }
    let breaker = router.breaker_summary();
    if breaker.dot().is_some() {
        spans.push(Span::styled(" ", palette.muted().bg(palette.surface_alt)));
        let dot_style = match breaker {
            crate::model::CircuitBreakerSummary::AnyOpen => {
                Style::default().fg(palette.danger).bg(palette.surface_alt)
            }
            crate::model::CircuitBreakerSummary::AnyHalfOpen => Style::default()
                .fg(palette.highlight)
                .bg(palette.surface_alt),
            crate::model::CircuitBreakerSummary::AllClosed => {
                palette.muted().bg(palette.surface_alt)
            }
        };
        spans.push(Span::styled("•".to_string(), dot_style));
    }
    spans
}

fn status_bar_work_text(app: &AppState) -> String {
    let mut parts = Vec::new();
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

/// Wave4-B2: compact status-bar segment for the adaptive router pill —
/// `<provider> [Mode] [↳N] [•]`. Empty (zero-length) when no
/// `router/status` has been observed yet.
///
/// Order is fixed left-to-right: provider name first, then the mode badge,
/// then queue-depth (only when `pending_count > 0`), then a circuit-breaker
/// dot (only when any lane is non-`closed`). Keeping the order stable lets
/// the status bar diff cleanly across renders.
///
/// Used by tests to assert content independently of the styled
/// `router_status_bar_spans` renderer.
#[cfg(test)]
fn status_bar_router_text(app: &AppState) -> String {
    let Some(router) = app.router.as_ref() else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    parts.push(router.provider_name.clone());
    let badge = router.mode.badge();
    if let Some(pending) = app.pending_router_mode.as_deref() {
        let pending_mode = crate::model::RouterMode::from_wire(pending);
        if pending_mode != router.mode {
            parts.push(format!("{badge} → {}", pending_mode.badge()));
        } else {
            parts.push(badge.to_string());
        }
    } else {
        parts.push(badge.to_string());
    }
    if app.queue_depth > 0 {
        parts.push(format!("↳{}", app.queue_depth));
    }
    if router.breaker_summary().dot().is_some() {
        parts.push("•".to_string());
    }
    parts.join(" ")
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

fn compact_inline(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = normalized.chars().count();
    if count <= max_chars {
        return normalized;
    }
    let prefix = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    format!("{prefix}...")
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
        SessionRunState::InProgress => active_spinner(),
        SessionRunState::Blocked { .. } => "!",
        SessionRunState::Success => "✓",
        SessionRunState::Error { .. } => "x",
        SessionRunState::Idle => "·",
    }
}

fn active_spinner() -> &'static str {
    const FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_millis() / 180) as usize)
        .unwrap_or(0);
    FRAMES[tick % FRAMES.len()]
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
    };
    use octos_core::{
        Message, SessionKey,
        ui_protocol::{ApprovalId, PreviewId, TaskRuntimeState, TurnId},
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
    fn render_default_view_keeps_work_plan_sticky_above_composer() {
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

        let text = rendered_text(&app);

        assert!(text.contains("Work"));
        assert!(text.contains("Plan"));
        assert!(text.contains("[x] 1. Inspect renderer"));
        assert!(text.contains("[ ] 2. Patch sticky plan"));
        assert!(text.contains("Composer"));
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

        assert!(text.contains("[x] 1. Inspect renderer"));
        assert!(text.contains("[ ] 2. Patch sticky plan"));
        assert!(!text.contains("[ ] 1. [ ] Inspect renderer"));
    }

    #[test]
    fn render_work_pane_shows_overflow_and_diff_approval_choices() {
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

        assert!(text.contains("Approval Apply patch"));
        assert!(text.contains("y = approve this command once"));
        assert!(text.contains("s = approve this command/scope for the session"));
        assert!(text.contains("n = deny it"));
        assert!(text.contains("more plan item(s) | Ctrl+O expand"));
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

        assert!(text.contains("Task Blocked"));
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

        assert_eq!(prose.find("First paragraph"), Some(0));
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
                    "We can implement.Now run tests.All pass.",
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
        assert!(!text.contains("implement.Now"));
        assert!(!text.contains("tests.All"));
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
        let bold_style = style_for_text(&buffer, "Renderer").expect("bold cell style");
        let code_style = style_for_text(&buffer, "layout").expect("inline code style");
        assert!(bold_style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(code_style.fg, Some(palette.highlight));
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

        assert!(text.contains("Activity"));
        assert!(text.contains("Tested"));
        assert!(text.contains("$ cargo test"));
        assert!(text.contains("running 6 tests"));
        assert!(text.contains("1 more line(s) hidden (Ctrl+O expand)"));
        assert!(text.contains("1.2s"));
        assert!(text.contains("Progress"));
        assert!(text.contains("call call-1"));
        assert!(text.contains("gpt-5-codex"));
        assert!(text.contains("state"));
        assert!(text.contains("running"));
        assert!(text.contains("approval"));
        assert!(text.contains("1 msgs/0 tasks"));
    }

    #[test]
    fn render_activity_uses_action_keywords_for_wait_and_file_tools() {
        let turn_id = TurnId::new();
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
                .with_turn(turn_id.clone())
                .with_tool_call("wait-1")
                .with_detail("sleep 20; tmux capture-pane")
                .with_success(true)
                .with_duration_ms(20_000),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "write_file", "complete")
                .with_turn(turn_id)
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
        let turn_id = TurnId::new();
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
            .with_turn(turn_id)
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
    fn render_tool_blocks_show_state_preview_failure_and_collapsed_detail() {
        let turn_id = TurnId::new();
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
                .with_turn(turn_id.clone())
                .with_tool_call("preview-1")
                .with_detail("cargo test")
                .with_output_preview("6 passed")
                .with_success(true)
                .with_duration_ms(1200),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "failed")
                .with_turn(turn_id.clone())
                .with_tool_call("fail-1")
                .with_detail("npm install")
                .with_success(false)
                .with_duration_ms(70_000),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_turn(turn_id)
                .with_tool_call("collapsed-1")
                .with_detail("src/lib.rs")
                .with_success(true),
        );

        let text = rendered_text(&app);

        assert!(text.contains("preview"));
        assert!(text.contains("failed"));
        assert!(text.contains("details collapsed"));
        assert!(text.contains("Ctrl+O expand | Tab inspector"));
        assert!(text.contains("elapsed 70s"));
        assert!(text.contains("6 passed"));
    }

    #[test]
    fn render_tool_output_expands_with_global_toggle_state() {
        let turn_id = TurnId::new();
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
                .with_turn(turn_id)
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

        assert!(text.contains("Progress"));
        assert!(text.contains("Thinking"));
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

    // ----- Wave4-B2: status bar router + queue indicators -----

    fn app_with_router(
        provider: &str,
        mode: crate::model::RouterMode,
        breakers: &[(&str, &str)],
    ) -> AppState {
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
        let mut circuit_breakers = std::collections::BTreeMap::new();
        for (lane, state) in breakers {
            circuit_breakers.insert((*lane).to_string(), (*state).to_string());
        }
        app.router = Some(crate::model::RouterState {
            provider_name: provider.into(),
            mode,
            qos_ranking: true,
            lane_scores: std::collections::BTreeMap::new(),
            circuit_breakers,
        });
        app
    }

    #[test]
    fn status_bar_renders_provider_name_when_router_state_present() {
        let app = app_with_router("deepseek/v4-pro", crate::model::RouterMode::Off, &[]);
        let text = rendered_text(&app);
        assert!(
            text.contains("deepseek/v4-pro"),
            "status bar should surface router provider; got: {text}",
        );
    }

    #[test]
    fn status_bar_renders_mode_badge_for_each_mode() {
        for (mode, badge) in [
            (crate::model::RouterMode::Off, "[Off]"),
            (crate::model::RouterMode::Lane, "[Lane]"),
            (crate::model::RouterMode::Hedge, "[Hedge]"),
        ] {
            let app = app_with_router("deepseek/v4-pro", mode, &[]);
            let text = rendered_text(&app);
            assert!(
                text.contains(badge),
                "status bar should render badge {badge} for mode {mode:?}; got: {text}",
            );
        }
    }

    #[test]
    fn status_bar_hides_router_segments_without_router_state() {
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
        for forbidden in ["[Off]", "[Lane]", "[Hedge]"] {
            assert!(
                !text.contains(forbidden),
                "status bar should hide mode badge before any router/status; got: {text}",
            );
        }
    }

    #[test]
    fn status_bar_shows_queue_depth_indicator_when_pending() {
        let mut app = app_with_router("deepseek/v4-pro", crate::model::RouterMode::Lane, &[]);
        app.queue_depth = 2;
        let text = rendered_text(&app);
        assert!(
            text.contains("↳2"),
            "queue depth indicator should appear as ↳N; got: {text}",
        );
    }

    #[test]
    fn status_bar_hides_queue_depth_indicator_when_empty() {
        let app = app_with_router("deepseek/v4-pro", crate::model::RouterMode::Lane, &[]);
        let text = rendered_text(&app);
        assert!(
            !text.contains("↳"),
            "queue depth indicator should hide when zero; got: {text}",
        );
    }

    #[test]
    fn status_bar_shows_circuit_breaker_dot_when_open() {
        let app = app_with_router(
            "deepseek/v4-pro",
            crate::model::RouterMode::Lane,
            &[("deepseek/v4-pro", "open"), ("anthropic/sonnet", "closed")],
        );
        let text = rendered_text(&app);
        assert!(
            text.contains("•"),
            "circuit-breaker dot should render when any lane is open; got: {text}",
        );
    }

    #[test]
    fn status_bar_hides_circuit_breaker_dot_when_all_closed() {
        let app = app_with_router(
            "deepseek/v4-pro",
            crate::model::RouterMode::Lane,
            &[
                ("deepseek/v4-pro", "closed"),
                ("anthropic/sonnet", "closed"),
            ],
        );
        let text = rendered_text(&app);
        assert!(
            !text.contains("•"),
            "circuit-breaker dot should hide when all closed; got: {text}",
        );
    }

    #[test]
    fn status_bar_router_segments_combines_provider_mode_queue_breaker() {
        let mut app = app_with_router(
            "deepseek/v4-pro",
            crate::model::RouterMode::Hedge,
            &[("deepseek/v4-pro", "open")],
        );
        app.queue_depth = 3;

        let line = status_bar_router_text(&app);

        assert!(line.contains("deepseek/v4-pro"));
        assert!(line.contains("[Hedge]"));
        assert!(line.contains("↳3"));
        assert!(line.contains("•"));
    }

    #[test]
    fn status_bar_router_text_renders_pending_mode_with_arrow_until_confirmed() {
        let mut app = app_with_router("deepseek/v4-pro", crate::model::RouterMode::Off, &[]);
        app.pending_router_mode = Some("hedge".into());

        let line = status_bar_router_text(&app);

        // Optimistic UX: render `[Off] → [Hedge]` to surface that the
        // client has dispatched a `router/set_mode` but the server hasn't
        // confirmed yet.
        assert!(
            line.contains("[Off]") && line.contains("[Hedge]") && line.contains("→"),
            "pending-mode line should show both badges joined by →; got: {line}",
        );
    }

    // ----- Wave4-B2: failover banner overlay -----

    #[test]
    fn failover_banner_is_rendered_above_status_when_active() {
        let mut app = app_with_router("deepseek/v4-pro", crate::model::RouterMode::Lane, &[]);
        app.router_failover_banner = Some(crate::model::RouterFailoverBanner {
            from_provider: "deepseek/v4-pro".into(),
            to_provider: "anthropic/sonnet".into(),
            reason: "circuit_break".into(),
            elapsed_ms: 8240,
            created_at: std::time::Instant::now(),
        });

        let text = rendered_text(&app);

        assert!(
            text.contains("↺")
                && text.contains("deepseek/v4-pro")
                && text.contains("anthropic/sonnet")
                && text.contains("8240ms"),
            "failover banner content should be visible somewhere on screen; got: {text}",
        );
    }

    #[test]
    fn failover_banner_absent_when_no_active_banner() {
        let app = app_with_router("deepseek/v4-pro", crate::model::RouterMode::Lane, &[]);
        let text = rendered_text(&app);
        assert!(
            !text.contains("↺"),
            "no failover banner glyph should render without a banner; got: {text}",
        );
    }
}
