use octos_core::app_ui::{AppUiCommand, AppUiEvent, AppUiSnapshot};
use octos_core::ui_protocol::{
    ApprovalAutoResolvedEvent, ApprovalCancelledEvent, ApprovalDecidedEvent, ApprovalId,
    ApprovalRespondParams, DiffPreviewGetParams, InputItem, MessageDeltaEvent,
    MessagePersistedEvent, ReplayLossyEvent, TaskOutputDeltaEvent, TaskOutputReadParams,
    TaskRuntimeState, TaskUpdatedEvent, TurnCompletedEvent, TurnErrorEvent, TurnInterruptParams,
    TurnSpawnCompleteEvent, TurnStartParams, UiNotification, UiProgressEvent,
};
use octos_core::{Message, TaskId};
use serde_json::Value;

use crate::{
    client_event::{CapabilityClientEvent, ClientEvent, PermissionProfileClientEvent},
    menu::{
        CommandEntry, CommandRegistry, CommandResolution, LocalAction, MenuAction, MenuAppSnapshot,
        MenuBuildResult, MenuContext, MenuId, TerminalSize, providers::core_menu_registry,
    },
    model::{
        ActivityItem, ActivityKind, AppState, ApprovalModalAction, ApprovalModalState,
        DiffHunkContext, DiffPreviewGetResult, FocusPane, LiveReply, SessionView, TaskView,
        complete_plan_steps_in_text, task_state_label,
    },
};

const TASK_OUTPUT_TAIL_BYTES: usize = 600;
const TASK_OUTPUT_READ_LIMIT_BYTES: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct SlashCommandMatch {
    pub name: String,
    pub description: String,
    pub available: bool,
}

pub struct Store {
    pub state: AppState,
}

impl Store {
    pub fn from_snapshot(snapshot: AppUiSnapshot) -> Self {
        Self {
            state: AppState::from_snapshot(snapshot),
        }
    }

    pub fn active_session(&self) -> Option<&SessionView> {
        self.state.active_session()
    }

    pub fn compose_command(&mut self) -> Option<AppUiCommand> {
        let prompt = self.state.composer.trim().to_string();
        if prompt.starts_with('/') {
            return self.dispatch_slash_command(&prompt);
        }

        if self.state.readonly {
            self.state.status = "Read-only mode: turn/start disabled".into();
            self.state.clear_current_composer_draft();
            return None;
        }

        if prompt.is_empty() {
            return None;
        }

        self.state.clear_current_composer_draft();
        if self.state.active_turn().is_some() {
            self.state.pending_messages.push(prompt);
            self.state.status =
                "Message staged; it will submit after the active turn. Press Esc to interrupt/send."
                    .into();
            self.state.scroll_transcript_to_latest();
            return None;
        }

        self.start_prompt_turn(prompt, "Queued turn/start")
    }

    #[allow(dead_code)]
    pub(crate) fn slash_command_matches(&self, query: &str) -> Vec<SlashCommandMatch> {
        let registry = CommandRegistry::with_core_commands();
        let query = query.trim().trim_start_matches('/').to_ascii_lowercase();
        let ctx = self.state.availability_context();
        let mut matches = registry
            .visible_commands(&ctx)
            .into_iter()
            .filter_map(|visible| {
                let command = visible.command;
                let rank = if query.is_empty() {
                    Some(0)
                } else if command.name.eq_ignore_ascii_case(query.as_str())
                    || command
                        .aliases
                        .iter()
                        .copied()
                        .any(|alias| alias.eq_ignore_ascii_case(query.as_str()))
                {
                    Some(0)
                } else if command.name.to_ascii_lowercase().starts_with(&query) {
                    Some(1)
                } else if command
                    .aliases
                    .iter()
                    .copied()
                    .any(|alias| alias.to_ascii_lowercase().starts_with(&query))
                {
                    Some(2)
                } else if command.name.to_ascii_lowercase().contains(&query) {
                    Some(3)
                } else if command
                    .aliases
                    .iter()
                    .copied()
                    .any(|alias| alias.to_ascii_lowercase().contains(&query))
                    || command.description.to_ascii_lowercase().contains(&query)
                {
                    Some(4)
                } else {
                    None
                }?;
                Some((
                    rank,
                    SlashCommandMatch {
                        name: command.slash_name(),
                        description: command.description.into(),
                        available: visible.availability.is_available(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|(rank, command)| (*rank, command.name.clone()));
        matches.into_iter().map(|(_, command)| command).collect()
    }

    fn dispatch_slash_command(&mut self, draft: &str) -> Option<AppUiCommand> {
        let registry = CommandRegistry::with_core_commands();
        let resolution = registry.resolve(draft);
        self.state.clear_current_composer_draft();

        match resolution {
            CommandResolution::Found {
                command,
                invocation: _,
            } => {
                let availability = registry.evaluate(command, &self.state.availability_context());
                if !availability.is_available() {
                    let command_name = command.slash_name();
                    self.show_unavailable_slash_command(
                        &command_name,
                        availability
                            .reason
                            .as_deref()
                            .unwrap_or("command is unavailable"),
                    );
                    return None;
                }
                self.dispatch_command_entry(&command.entry)
            }
            CommandResolution::EmptyCommand => {
                self.open_menu(MenuId::from(crate::menu::registry::MENU_HELP));
                None
            }
            CommandResolution::Unknown { invocation } => {
                self.show_unknown_slash_command(&format!("/{}", invocation.name), draft);
                None
            }
            CommandResolution::NotCommand => None,
        }
    }

    fn dispatch_command_entry(&mut self, entry: &CommandEntry) -> Option<AppUiCommand> {
        match entry {
            CommandEntry::OpenMenu(id) => {
                self.open_menu(id.clone());
                None
            }
            CommandEntry::LocalAction(action) => self.dispatch_local_action(action.clone()),
            CommandEntry::AppUiAction(action) => {
                self.state.status = format!(
                    "AppUI command `{}` is advertised but not wired yet",
                    action.method()
                );
                None
            }
            CommandEntry::PromptTemplate(template) => {
                self.start_prompt_turn((*template).to_string(), "Queued prompt template")
            }
        }
    }

    fn dispatch_local_action(&mut self, action: LocalAction) -> Option<AppUiCommand> {
        match action {
            LocalAction::ShowProcessStatus => {
                self.show_local_process_status();
                None
            }
            LocalAction::StopActiveTurn => {
                let had_active_turn = self.state.active_turn().is_some();
                let command = self.interrupt_command();
                if !had_active_turn {
                    self.push_local_activity(
                        ActivityKind::Warning,
                        "local /stop",
                        "No active turn to interrupt",
                        Some("Nothing was sent to the backend."),
                    );
                }
                command
            }
            LocalAction::ShowHelp => {
                self.open_menu(MenuId::from(crate::menu::registry::MENU_HELP));
                None
            }
            LocalAction::SetTheme(theme) => {
                self.state.status = format!("Theme selected: {theme}");
                None
            }
            LocalAction::SaveStatusLine(items) => {
                self.state.status = format!("Status line layout selected: {}", items.join(", "));
                None
            }
            LocalAction::SaveTerminalTitle(items) => {
                self.state.status = format!("Terminal title layout selected: {}", items.join(", "));
                None
            }
            LocalAction::SaveKeymap => {
                self.state.status = "Keymap save is not wired yet".into();
                None
            }
            LocalAction::RefreshMenu(id) => {
                self.open_menu(id);
                None
            }
            LocalAction::Custom(name) => {
                self.state.status = format!("Local menu action `{name}` is not wired yet");
                None
            }
        }
    }

    pub fn open_menu(&mut self, id: MenuId) {
        self.state.menu_stack.open(id);
        self.refresh_active_menu();
        if let Some(frame) = self.state.menu_stack.active() {
            self.state.status = format!("Menu: {}", frame.id);
        }
    }

    pub fn close_menu(&mut self) -> bool {
        if self.state.menu_stack.close().is_some() {
            self.refresh_active_menu();
            if let Some(frame) = self.state.menu_stack.active() {
                self.state.status = format!("Menu: {}", frame.id);
            }
            return true;
        }
        false
    }

    pub fn close_all_menus(&mut self) -> bool {
        if self.state.menu_stack.is_empty() {
            return false;
        }
        self.state.menu_stack.close_all();
        self.state.active_menu = None;
        true
    }

    pub fn select_next_menu_item(&mut self) -> bool {
        let Some(frame) = self.state.menu_stack.active_mut() else {
            return false;
        };
        let len = active_menu_item_len(self.state.active_menu.as_ref());
        if len == 0 {
            return true;
        }
        frame.selected_index = (frame.selected_index + 1) % len;
        self.refresh_active_menu();
        true
    }

    pub fn select_prev_menu_item(&mut self) -> bool {
        let Some(frame) = self.state.menu_stack.active_mut() else {
            return false;
        };
        let len = active_menu_item_len(self.state.active_menu.as_ref());
        if len == 0 {
            return true;
        }
        frame.selected_index = if frame.selected_index == 0 {
            len - 1
        } else {
            frame.selected_index - 1
        };
        self.refresh_active_menu();
        true
    }

    pub fn accept_active_menu_item(&mut self) -> Option<AppUiCommand> {
        let selected_index = self
            .state
            .menu_stack
            .active()
            .map(|frame| frame.selected_index)
            .unwrap_or(0);
        let Some(action) = self
            .state
            .active_menu
            .as_ref()
            .and_then(|menu| active_menu_selected_action(menu, selected_index))
        else {
            return None;
        };
        self.dispatch_menu_action(action)
    }

    pub fn active_menu_has_selectable_item(&self) -> bool {
        let selected_index = self
            .state
            .menu_stack
            .active()
            .map(|frame| frame.selected_index)
            .unwrap_or(0);
        self.state
            .active_menu
            .as_ref()
            .and_then(|menu| active_menu_selected_action(menu, selected_index))
            .is_some()
    }

    fn dispatch_menu_action(&mut self, action: MenuAction) -> Option<AppUiCommand> {
        match action {
            MenuAction::OpenMenu(id) => {
                self.open_menu(id);
                None
            }
            MenuAction::ReplaceMenu(id) => {
                self.state.menu_stack.replace(id);
                self.refresh_active_menu();
                if let Some(frame) = self.state.menu_stack.active() {
                    self.state.status = format!("Menu: {}", frame.id);
                }
                None
            }
            MenuAction::Close => {
                self.close_menu();
                None
            }
            MenuAction::CloseAll => {
                self.close_all_menus();
                None
            }
            MenuAction::Local(action) => self.dispatch_local_action(action),
            MenuAction::SendAppUi(command) => Some(command),
            MenuAction::SubmitPrompt(prompt) => {
                self.start_prompt_turn(prompt, "Queued menu prompt")
            }
            MenuAction::Noop => None,
        }
    }

    pub fn refresh_active_menu(&mut self) {
        let Some(frame) = self.state.menu_stack.active().cloned() else {
            self.state.active_menu = None;
            return;
        };
        let path = self.state.menu_stack.path();
        let app = self.menu_app_snapshot();
        let availability = self.state.availability_context();
        let ctx = MenuContext {
            availability,
            app,
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &path,
        };
        let mut result = core_menu_registry().build(&frame.id, &ctx);
        apply_active_menu_search_query(&mut result, &frame.id, &frame.search_query);
        let len = active_menu_item_len(Some(&result));
        if len > 0
            && let Some(frame) = self.state.menu_stack.active_mut()
        {
            frame.selected_index = frame.selected_index.min(len.saturating_sub(1));
        }
        self.state.active_menu = Some(result);
    }

    fn refresh_active_menu_if_open(&mut self) {
        if self.state.menu_stack.is_active() {
            self.refresh_active_menu();
        }
    }

    fn menu_app_snapshot(&self) -> MenuAppSnapshot<'_> {
        let selected_session = self.state.active_session();
        let selected_task = self.state.active_task();
        MenuAppSnapshot {
            status: Some(self.state.status.as_str()),
            target: self.state.target.as_deref(),
            cwd: Some(self.state.workspace.root.as_str()),
            current_model: None,
            current_profile: selected_session.and_then(|session| session.profile_id.as_deref()),
            permission_profile: selected_session
                .and_then(|session| self.state.permission_profile_for(&session.id)),
            selected_session_id: selected_session.map(|session| &session.id),
            selected_session_title: selected_session.map(|session| session.title.as_str()),
            selected_task_title: selected_task.map(|task| task.title.as_str()),
            background_task_count: self
                .state
                .sessions
                .iter()
                .flat_map(|session| session.tasks.iter())
                .filter(|task| {
                    matches!(
                        task.state,
                        TaskRuntimeState::Pending | TaskRuntimeState::Running
                    )
                })
                .count(),
        }
    }

    fn show_local_process_status(&mut self) {
        let counts = count_tasks(self);
        let turn = self
            .state
            .active_turn()
            .map(|(_, turn_id)| format!("active turn {}", short_id(&turn_id.0.to_string())))
            .unwrap_or_else(|| "idle".into());
        let selected_task = self
            .state
            .active_task()
            .map(|task| {
                format!(
                    "selected task: {} [{}]",
                    task.title,
                    task_state_label(task.state)
                )
            })
            .unwrap_or_else(|| "selected task: none".into());
        let staged = self.state.pending_messages.len();
        let status = format!(
            "Local /ps: {turn}; tasks {} total ({} running, {} pending, {} done, {} failed); {staged} staged",
            counts.total, counts.running, counts.pending, counts.done, counts.failed
        );
        let detail = format!(
            "run state: {} | {selected_task} | {} activity item(s)",
            self.state.run_state.label(),
            self.state.activity.len()
        );

        self.state.focus = FocusPane::Tasks;
        self.state.status = status.clone();
        self.state.scroll_transcript_to_latest();
        self.push_local_activity(ActivityKind::Progress, "local /ps", status, Some(detail));
    }

    fn show_unknown_slash_command(&mut self, command: &str, draft: &str) {
        let ctx = self.state.availability_context();
        let status = format!(
            "Unknown slash command: {command}. Try {}.",
            slash_command_try_hint(&ctx)
        );
        self.state.status = status.clone();
        self.push_local_activity(
            ActivityKind::Warning,
            "local slash command",
            status,
            Some(format!("Ignored input: {draft}")),
        );
    }

    fn show_unavailable_slash_command(&mut self, command: &str, reason: &str) {
        let status = format!("{command} is unavailable: {reason}");
        self.state.status = status.clone();
        self.push_local_activity(
            ActivityKind::Warning,
            "local slash command",
            status,
            Some("The command was resolved by the registry but did not pass availability gates."),
        );
    }

    fn push_local_activity(
        &mut self,
        kind: ActivityKind,
        title: impl Into<String>,
        status: impl Into<String>,
        detail: Option<impl Into<String>>,
    ) {
        let mut item = ActivityItem::new(kind, title, status);
        if let Some(detail) = detail {
            item = item.with_detail(detail);
        }
        self.state.push_activity(item);
    }

    fn start_prompt_turn(
        &mut self,
        prompt: String,
        status: impl Into<String>,
    ) -> Option<AppUiCommand> {
        let session_id = self.active_session()?.id.clone();
        let turn_id = octos_core::ui_protocol::TurnId::new();
        self.state.record_submitted_user_prompt(
            session_id.clone(),
            turn_id.clone(),
            prompt.clone(),
        );
        self.state.scroll_transcript_to_latest();
        self.state.status = status.into();
        self.state.set_run_state_in_progress();
        Some(AppUiCommand::SubmitPrompt(TurnStartParams {
            session_id,
            turn_id,
            input: vec![InputItem::Text { text: prompt }],
        }))
    }

    pub fn interrupt_staged_command(&mut self) -> Option<AppUiCommand> {
        if !self.state.has_pending_messages() {
            self.state.status = "No staged message to send".into();
            return None;
        }

        let command = self.interrupt_command();
        if command.is_some() {
            self.state.status =
                "Interrupt requested; staged message will submit when the turn stops".into();
        }
        command
    }

    pub fn interrupt_command(&mut self) -> Option<AppUiCommand> {
        let Some((session_id, turn_id)) = self
            .state
            .active_turn()
            .map(|(session_id, turn_id)| (session_id.clone(), turn_id.clone()))
        else {
            self.state.status = "No active turn to interrupt".into();
            return None;
        };

        self.state.status = "Interrupt requested for active turn".into();
        Some(AppUiCommand::InterruptTurn(TurnInterruptParams {
            session_id,
            turn_id,
        }))
    }

    pub fn respond_approval_command(
        &mut self,
        action: ApprovalModalAction,
    ) -> Option<AppUiCommand> {
        let Some(approval) = self.state.approval.take() else {
            self.state.status = "No active approval request".into();
            return None;
        };

        self.state.status = format!("Approval {}: {}", action.status_label(), approval.title);
        if self.state.active_turn().is_some() {
            self.state.set_run_state_in_progress();
        } else if self.state.run_state.is_active() {
            self.state.set_run_state_success();
        }
        self.state.approval_auto_open = true;

        let mut params = ApprovalRespondParams::new(
            approval.session_id,
            approval.approval_id,
            action.decision(),
        );
        params.approval_scope = Some(action.approval_scope().into());

        Some(AppUiCommand::RespondApproval(params))
    }

    pub fn clear_composer_or_staged_messages(&mut self) {
        if !self.state.pending_messages.is_empty() {
            let cleared = self.state.pending_messages.len();
            self.state.pending_messages.clear();
            self.state.status = format!("Cleared {cleared} staged message(s)");
            return;
        }

        if !self.state.composer.is_empty() {
            self.state.clear_current_composer_draft();
            self.state.status = "Cleared composer draft".into();
            return;
        }

        self.state.status = "No composer draft or staged message to clear".into();
    }

    pub fn show_pending_approval(&mut self) -> bool {
        let title = {
            let Some(approval) = self.state.approval.as_mut() else {
                self.state.status = "No pending approval to show".into();
                return false;
            };

            approval.visible = true;
            approval.title.clone()
        };

        self.state.approval_auto_open = true;
        self.state.focus = FocusPane::Composer;
        self.state.status = format!("Approval shown: {title}");
        true
    }

    pub fn read_task_output_command(&mut self) -> Option<AppUiCommand> {
        let Some(task) = self.state.active_task_context() else {
            self.state.status = "No selected task output to read".into();
            return None;
        };

        let cursor = self
            .state
            .task_output_cursor(&task.session_id, &task.task_id);
        self.state.task_output.open(
            task.session_id.clone(),
            task.task_id.clone(),
            task.title.clone(),
            task.output_tail.clone(),
            cursor,
        );
        self.state.status = format!("Requested task output: {}", task.title);

        Some(AppUiCommand::ReadTaskOutput(TaskOutputReadParams {
            session_id: task.session_id,
            task_id: task.task_id,
            cursor,
            limit_bytes: Some(TASK_OUTPUT_READ_LIMIT_BYTES),
        }))
    }

    pub fn read_diff_preview_command(&mut self) -> Option<AppUiCommand> {
        let Some(session_id) = self.active_session().map(|session| session.id.clone()) else {
            self.state.status = "No active session for diff preview".into();
            return None;
        };
        let preview_id = self
            .state
            .approval
            .as_ref()
            .and_then(ApprovalModalState::diff_preview_id)
            .or_else(|| self.state.active_diff_preview_id());
        let Some(preview_id) = preview_id else {
            self.state.status = "No diff preview id is available for the selected task".into();
            return None;
        };

        self.state.diff_preview.open_loading(preview_id.clone());
        self.state.status = "Requested diff preview".into();
        Some(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
            session_id,
            preview_id,
        }))
    }

    pub fn close_modal(&mut self) -> bool {
        if let Some(approval) = self.state.approval.as_mut()
            && approval.visible
        {
            approval.visible = false;
            self.state.approval_auto_open = false;
            self.state.status =
                "Approval pane hidden; auto-open disabled until approval is shown again".into();
            return true;
        }

        if self.state.task_output.active {
            self.state.task_output.close();
            self.state.status = "Closed task output".into();
            return true;
        }

        if self.state.diff_preview.active {
            self.state.diff_preview.close();
            self.state.status = "Closed inline diff preview".into();
            return true;
        }

        false
    }

    pub fn show_diff_preview_placeholder(&mut self) {
        self.state.status =
            "Diff preview unavailable: protocol does not expose preview ids/content to the TUI yet"
                .into();
    }

    pub fn select_next_diff_hunk(&mut self) {
        self.state.diff_preview.select_next_hunk();
        if let Some(context) = self.state.diff_preview.selected_hunk_context() {
            self.state.status = format!(
                "Selected diff hunk: {} {}",
                context.path, context.hunk_header
            );
        } else {
            self.state.status = "No diff hunk is available to select".into();
        }
    }

    pub fn select_prev_diff_hunk(&mut self) {
        self.state.diff_preview.select_prev_hunk();
        if let Some(context) = self.state.diff_preview.selected_hunk_context() {
            self.state.status = format!(
                "Selected diff hunk: {} {}",
                context.path, context.hunk_header
            );
        } else {
            self.state.status = "No diff hunk is available to select".into();
        }
    }

    pub fn stage_selected_diff_context(&mut self) {
        let Some(context) = self.state.diff_preview.selected_hunk_context() else {
            self.state.status = "No selected diff hunk context to stage".into();
            return;
        };
        let path = context.path.clone();
        let prompt = diff_hunk_context_prompt(&context);

        if self.state.active_turn().is_some() {
            self.state.pending_messages.push(prompt);
            self.state.status = format!("Staged selected diff hunk context for next turn: {path}");
        } else {
            if !self.state.composer.trim().is_empty() {
                self.state.composer.push_str("\n\n");
            }
            self.state.composer.push_str(&prompt);
            self.state.status = format!("Added selected diff hunk context to composer: {path}");
        }

        self.state.focus = FocusPane::Composer;
        self.state.scroll_transcript_to_latest();
    }

    pub fn apply_client_event(&mut self, event: ClientEvent) -> Option<AppUiCommand> {
        match event {
            ClientEvent::App(event) => self.apply_event(*event),
            ClientEvent::Capabilities(event) => {
                self.apply_capability_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::DiffPreview(result) => {
                self.apply_diff_preview_result(result);
                None
            }
            ClientEvent::PermissionProfile(event) => {
                self.apply_permission_profile_event(event);
                self.refresh_active_menu_if_open();
                None
            }
        }
    }

    fn apply_capability_event(&mut self, event: CapabilityClientEvent) {
        self.state.set_capabilities(event.server_capabilities);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "capabilities",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    pub fn apply_event(&mut self, event: AppUiEvent) -> Option<AppUiCommand> {
        let command = match event {
            AppUiEvent::Snapshot(snapshot) => {
                let composer = self.state.composer.clone();
                let composer_drafts = self.state.composer_drafts.clone();
                let pending_messages = self.state.pending_messages.clone();
                let optimistic_user_messages = self.state.optimistic_user_messages.clone();
                let approval_auto_open = self.state.approval_auto_open;
                let expanded_tool_outputs = self.state.expanded_tool_outputs;
                let menu_stack = self.state.menu_stack.clone();
                let previous_capabilities = self.state.capabilities.clone();
                let permission_profiles = self.state.permission_profiles.clone();

                let mut state = AppState::from_snapshot(snapshot);
                if state.capabilities.is_none() {
                    state.capabilities = previous_capabilities;
                }
                state.composer = composer;
                state.composer_drafts = composer_drafts;
                state.pending_messages = pending_messages;
                state.optimistic_user_messages = optimistic_user_messages;
                state.approval_auto_open = approval_auto_open;
                state.expanded_tool_outputs = expanded_tool_outputs;
                state.menu_stack = menu_stack;
                state.permission_profiles = permission_profiles;
                state.restore_optimistic_user_messages();
                self.state = state;
                None
            }
            AppUiEvent::Protocol(notification) => self.apply_notification(notification),
            AppUiEvent::Progress(progress) => self.apply_progress(progress),
            AppUiEvent::Status(status) => {
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "status",
                    status.message.clone(),
                ));
                self.state.status = status.message;
                None
            }
            AppUiEvent::Error(error) => {
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Error,
                        error.code.clone(),
                        error.message.clone(),
                    )
                    .with_detail("app-ui error"),
                );
                self.state.status = format!("Error [{}]: {}", error.code, error.message);
                self.state.set_run_state_error(error.message);
                None
            }
        };
        self.refresh_active_menu_if_open();
        command
    }

    pub fn apply_diff_preview_result(&mut self, result: DiffPreviewGetResult) {
        let title = result
            .preview
            .title
            .clone()
            .unwrap_or_else(|| format!("{} file diff", result.preview.files.len()));
        let status = result.status.clone();
        let file_count = result.preview.files.len();
        self.state.diff_preview.apply_result(result);
        self.state.status = format!("Diff preview {status}: {title} ({file_count} files)");
    }

    fn apply_permission_profile_event(&mut self, event: PermissionProfileClientEvent) {
        self.state
            .set_permission_profile(event.session_id, event.current);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "permissions",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_progress(&mut self, event: UiProgressEvent) -> Option<AppUiCommand> {
        let status = progress_status(&event);
        let record_activity = should_record_progress_activity(&event);
        let diff_preview_request = event.metadata.file_mutation.as_ref().and_then(|notice| {
            notice
                .preview_id
                .clone()
                .map(|preview_id| (notice.operation.clone(), notice.path.clone(), preview_id))
        });
        let mutation_detail = event.metadata.file_mutation.as_ref().map(|notice| {
            let preview = if notice.preview_id.is_some() {
                " | diff preview ready"
            } else {
                ""
            };
            format!("{} {}{preview}", notice.operation, notice.path)
        });
        if record_activity {
            let mut item = ActivityItem::new(
                ActivityKind::Progress,
                event
                    .metadata
                    .label
                    .clone()
                    .unwrap_or_else(|| event.metadata.kind.clone()),
                status.clone(),
            );
            if let Some(turn_id) = event.turn_id.clone() {
                item = item.with_turn(turn_id);
            }
            if let Some(detail) = event.metadata.detail.clone().or(mutation_detail) {
                item = item.with_detail(detail);
            }
            self.state.push_activity(item);
        }
        if event.turn_id.is_some() {
            self.state.set_run_state_in_progress();
        }

        if let Some((operation, path, preview_id)) = diff_preview_request {
            let request_already_in_flight = self.state.diff_preview.loading
                && self.state.diff_preview.requested_preview_id.as_ref() == Some(&preview_id);
            self.state.diff_preview.open_loading(preview_id.clone());
            self.state.status = format!("Opening diff preview: {operation} {path}");
            if !request_already_in_flight {
                return Some(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                    session_id: event.session_id,
                    preview_id,
                }));
            }
            return None;
        }

        if !is_noisy_progress_status(&status) {
            self.state.status = status;
        }
        None
    }

    fn apply_notification(&mut self, notification: UiNotification) -> Option<AppUiCommand> {
        match notification {
            UiNotification::SessionOpened(event) => {
                let session_id = event.session_id.clone();
                self.state.set_capabilities(event.capabilities.clone());
                if let Some(panes) = event.panes {
                    self.state.apply_pane_snapshot(panes);
                }
                if let Some(workspace_root) = event.workspace_root {
                    self.state.workspace.root = workspace_root;
                }
                if let Some(index) = self
                    .state
                    .sessions
                    .iter()
                    .position(|session| session.id == session_id)
                {
                    self.state.selected_session = index;
                    self.state.sessions[index].profile_id = event.active_profile_id.clone();
                } else {
                    self.state.sessions.push(SessionView {
                        id: session_id.clone(),
                        title: session_id.0.clone(),
                        profile_id: event.active_profile_id.clone(),
                        messages: Vec::new(),
                        tasks: Vec::new(),
                        live_reply: None,
                    });
                    self.state.selected_session = self.state.sessions.len().saturating_sub(1);
                }
                if self.state.active_turn().is_none() {
                    self.state.set_run_state_idle();
                }
                self.state.status =
                    format!("Opened {} on {}", session_id.0, self.state.protocol_version);
                None
            }
            UiNotification::TurnStarted(event) => {
                if let Some(session) = self.find_session_mut(&event.session_id) {
                    session.live_reply = Some(LiveReply {
                        turn_id: event.turn_id,
                        text: String::new(),
                    });
                    self.state.status = format!("Turn started in {}", session.title);
                    self.state.set_run_state_in_progress();
                }
                None
            }
            UiNotification::MessageDelta(MessageDeltaEvent {
                session_id,
                turn_id,
                text,
            }) => {
                let mut reset_scroll = false;
                if let Some(session) = self.find_session_mut(&session_id) {
                    if let Some(live_reply) = session.live_reply.as_mut() {
                        if live_reply.turn_id == turn_id {
                            live_reply.text.push_str(&text);
                            reset_scroll = true;
                        }
                    }
                }
                if reset_scroll {
                    self.state.scroll_transcript_to_latest();
                }
                None
            }
            UiNotification::ToolStarted(event) => {
                let mut item =
                    ActivityItem::new(ActivityKind::Tool, event.tool_name.clone(), "running")
                        .with_turn(event.turn_id)
                        .with_tool_call(event.tool_call_id.clone());
                if let Some(arguments) = event.arguments {
                    if let Some(detail) = tool_invocation_detail(&event.tool_name, &arguments) {
                        item = item.with_detail(detail);
                    }
                    item = item.with_arguments(arguments);
                }
                self.state.push_activity(item);
                self.state.set_run_state_in_progress();
                self.state.status =
                    format!("Tool started: {} ({})", event.tool_name, event.tool_call_id);
                None
            }
            UiNotification::ToolProgress(event) => {
                let status = event
                    .progress_pct
                    .map(|pct| format!("{pct:.0}%"))
                    .unwrap_or_else(|| "running".into());
                self.state.update_tool_activity(
                    &event.tool_call_id,
                    status,
                    event.message.clone(),
                    None,
                    None,
                    None,
                );
                self.state.set_run_state_in_progress();
                self.state.status = event
                    .message
                    .unwrap_or_else(|| format!("Tool progress {}", event.tool_call_id));
                None
            }
            UiNotification::ToolCompleted(event) => {
                let status = match event.success {
                    Some(false) => "failed",
                    _ => "complete",
                };
                let output_preview = event.output_preview.clone();
                self.state.update_tool_activity(
                    &event.tool_call_id,
                    status,
                    None,
                    event.output_preview,
                    event.success,
                    event.duration_ms,
                );
                if event.success == Some(false) {
                    if let Some(recovery) =
                        tool_failure_recovery_hint(&event.tool_name, output_preview.as_deref())
                    {
                        self.state.push_activity(
                            ActivityItem::new(
                                ActivityKind::Warning,
                                "Recovery suggestion",
                                recovery.clone(),
                            )
                            .with_turn(event.turn_id)
                            .with_tool_call(event.tool_call_id),
                        );
                        self.state.status = recovery;
                    } else {
                        self.state.status = format!("Tool failed: {}", event.tool_name);
                    }
                } else {
                    self.state.status = format!("Tool completed: {}", event.tool_name);
                }
                None
            }
            UiNotification::ApprovalRequested(event) => {
                let title = event.title.clone();
                let session_id = event.session_id.clone();
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Approval,
                        event.tool_name.clone(),
                        title.clone(),
                    )
                    .with_turn(event.turn_id.clone())
                    .with_detail(
                        event
                            .approval_kind
                            .clone()
                            .unwrap_or_else(|| "generic".into()),
                    ),
                );
                let mut approval = ApprovalModalState::from_event(event);
                approval.visible = self.state.approval_auto_open;
                let diff_preview_id = approval.diff_preview_id();
                self.state.approval = Some(approval);
                self.state.focus = FocusPane::Composer;
                self.state.set_run_state_blocked(title.clone());
                self.state.status = format!("Approval requested: {title}");
                if let Some(preview_id) = diff_preview_id {
                    let request_already_in_flight = self.state.diff_preview.loading
                        && self.state.diff_preview.requested_preview_id.as_ref()
                            == Some(&preview_id);
                    self.state.diff_preview.open_loading(preview_id.clone());
                    self.state.status = format!("Opening inline diff preview: {title}");
                    if !request_already_in_flight {
                        return Some(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                            session_id,
                            preview_id,
                        }));
                    }
                }
                None
            }
            UiNotification::TaskUpdated(event) => {
                self.apply_task_update(event);
                None
            }
            UiNotification::TaskOutputDelta(event) => {
                self.apply_task_output(event);
                None
            }
            UiNotification::Warning(event) => {
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Warning,
                        event.code.clone(),
                        event.message.clone(),
                    )
                    .with_detail("protocol warning"),
                );
                self.state.status = format!("Warning [{}]: {}", event.code, event.message);
                None
            }
            UiNotification::TurnCompleted(event) => self.commit_live_reply(event),
            UiNotification::TurnError(event) => self.fail_live_reply(event),
            UiNotification::ApprovalAutoResolved(event) => self.apply_approval_auto_resolved(event),
            UiNotification::ApprovalDecided(event) => self.apply_approval_decided(event),
            UiNotification::ApprovalCancelled(event) => self.apply_approval_cancelled(event),
            UiNotification::ProgressUpdated(event) => self.apply_progress(event),
            UiNotification::ReplayLossy(event) => self.apply_replay_lossy(event),
            UiNotification::MessagePersisted(event) => self.apply_message_persisted(event),
            UiNotification::TurnSpawnComplete(event) => self.apply_turn_spawn_complete(event),
        }
    }

    fn apply_message_persisted(&mut self, event: MessagePersistedEvent) -> Option<AppUiCommand> {
        let attachment_count = event.media.len();
        let attachment_hint = match attachment_count {
            0 => String::new(),
            1 => " with 1 attachment".into(),
            count => format!(" with {count} attachments"),
        };
        self.state.status = format!(
            "Persisted {} message seq {}{}",
            event.role, event.seq, attachment_hint
        );
        None
    }

    fn apply_turn_spawn_complete(&mut self, event: TurnSpawnCompleteEvent) -> Option<AppUiCommand> {
        if let Some(session) = self.find_session_mut(&event.session_id) {
            let mut message = Message::assistant(event.content);
            message.media = event.media;
            message.thread_id = event
                .thread_id
                .or(event.response_to_client_message_id.clone());
            session.messages.push(message);
            session.live_reply = None;
            self.state.scroll_transcript_to_latest();
        }
        self.state.set_run_state_success();
        self.state.status = format!(
            "Background task {} persisted assistant message seq {}",
            event.task_id, event.seq
        );
        None
    }

    fn apply_approval_auto_resolved(
        &mut self,
        event: ApprovalAutoResolvedEvent,
    ) -> Option<AppUiCommand> {
        let decision = event.decision.as_wire_str().to_owned();
        let scope = event.scope.clone();
        let scope_match = event.scope_match.clone();
        let tool_name = event.tool_name.clone();
        let cleared = self.clear_matching_approval(&event.approval_id);
        self.state.push_activity(
            ActivityItem::new(
                ActivityKind::Approval,
                tool_name,
                format!("auto-resolved {decision}"),
            )
            .with_turn(event.turn_id)
            .with_detail(format!("scope={scope} match={scope_match}")),
        );
        if cleared {
            self.state.set_run_state_in_progress();
        }
        self.state.status = format!("Approval auto-resolved ({decision}) by scope policy");
        None
    }

    fn apply_approval_decided(&mut self, event: ApprovalDecidedEvent) -> Option<AppUiCommand> {
        let decision = event.decision.as_wire_str().to_owned();
        let detail = if event.auto_resolved {
            format!("auto-resolved by {}", event.decided_by)
        } else {
            format!("decided by {}", event.decided_by)
        };
        let cleared = self.clear_matching_approval(&event.approval_id);
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Approval, "decision", decision.clone())
                .with_turn(event.turn_id)
                .with_detail(detail.clone()),
        );
        if cleared {
            self.state.set_run_state_in_progress();
        }
        self.state.status = format!("Approval decided: {decision} ({detail})");
        None
    }

    fn apply_approval_cancelled(&mut self, event: ApprovalCancelledEvent) -> Option<AppUiCommand> {
        let reason = event.reason.clone();
        let cleared = self.clear_matching_approval(&event.approval_id);
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Approval, "cancelled", reason.clone())
                .with_turn(event.turn_id),
        );
        if cleared {
            self.state.set_run_state_in_progress();
        }
        self.state.status = format!("Approval cancelled: {reason}");
        None
    }

    fn apply_replay_lossy(&mut self, event: ReplayLossyEvent) -> Option<AppUiCommand> {
        let cursor_hint = event
            .last_durable_cursor
            .as_ref()
            .map(|cursor| format!(" (last durable seq {})", cursor.seq))
            .unwrap_or_default();
        let message = format!(
            "Replay lossy: {} dropped{cursor_hint}; reconnect to rehydrate",
            event.dropped_count,
        );
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Warning, "replay_lossy", message.clone())
                .with_detail("durable cursor diverged"),
        );
        self.state.status = message;
        None
    }

    fn apply_task_update(&mut self, event: TaskUpdatedEvent) {
        let Some(session) = self.find_session_mut(&event.session_id) else {
            return;
        };

        if let Some(task) = session
            .tasks
            .iter_mut()
            .find(|task| task.id == event.task_id)
        {
            task.state = event.state;
            task.runtime_detail = event.runtime_detail;
        } else {
            session.tasks.push(TaskView {
                id: event.task_id,
                title: event.title,
                state: event.state,
                runtime_detail: event.runtime_detail,
                output_tail: String::new(),
            });
        }
    }

    fn apply_task_output(&mut self, event: TaskOutputDeltaEvent) {
        let TaskOutputDeltaEvent {
            session_id,
            task_id,
            cursor,
            text,
        } = event;

        let Some(task) = self.find_task_mut(&session_id, &task_id) else {
            return;
        };

        task.output_tail.push_str(&text);
        if task.output_tail.len() > TASK_OUTPUT_TAIL_BYTES {
            let mut split_at = task.output_tail.len() - TASK_OUTPUT_TAIL_BYTES;
            while !task.output_tail.is_char_boundary(split_at) {
                split_at += 1;
            }
            task.output_tail = task.output_tail.split_off(split_at);
        }

        self.state
            .set_task_output_cursor(session_id.clone(), task_id.clone(), cursor);
        if self.state.task_output.is_for(&session_id, &task_id) {
            self.state.task_output.append_output(&text, cursor);
        }
        self.state.status = format!("Task output @{}", cursor.offset);
    }

    fn commit_live_reply(&mut self, event: TurnCompletedEvent) -> Option<AppUiCommand> {
        let seq = event.cursor.map(|cursor| cursor.seq).unwrap_or(0);
        let complete_live_plan = self.turn_had_completion_activity(&event.turn_id);
        let (status, reset_scroll, completed_current_turn) = {
            let Some(session) = self.find_session_mut(&event.session_id) else {
                return None;
            };
            let title = session.title.clone();
            match session.live_reply.take() {
                Some(live_reply) if live_reply.turn_id == event.turn_id => {
                    let text = if complete_live_plan {
                        complete_plan_steps_in_text(&live_reply.text)
                    } else {
                        live_reply.text
                    };
                    session.messages.push(Message::assistant(text));
                    (
                        format!("Turn completed in {title} at seq {seq}"),
                        true,
                        true,
                    )
                }
                Some(live_reply) => {
                    session.live_reply = Some(live_reply);
                    (
                        format!("Ignored completed stale turn in {title}"),
                        false,
                        false,
                    )
                }
                None => (
                    format!("Turn completed in {title} at seq {seq}"),
                    false,
                    true,
                ),
            }
        };
        if reset_scroll {
            self.state.scroll_transcript_to_latest();
        }
        self.state.status = status;
        if completed_current_turn {
            self.state.set_run_state_success();
        }
        self.submit_next_pending_if_idle()
    }

    fn fail_live_reply(&mut self, event: TurnErrorEvent) -> Option<AppUiCommand> {
        let Some(session) = self.find_session_mut(&event.session_id) else {
            return None;
        };
        let title = session.title.clone();
        let (status, failed_current_turn) = match session.live_reply.take() {
            Some(live_reply) if live_reply.turn_id == event.turn_id => (
                format!("Turn error {}: {}", event.code, event.message),
                true,
            ),
            Some(live_reply) => {
                session.live_reply = Some(live_reply);
                (
                    format!("Ignored stale turn error in {title}: {}", event.code),
                    false,
                )
            }
            None => (
                format!("Turn error {}: {}", event.code, event.message),
                true,
            ),
        };
        self.state.status = status;
        if failed_current_turn {
            self.state
                .set_run_state_error(format!("{}: {}", event.code, event.message));
        }
        self.submit_next_pending_if_idle()
    }

    fn submit_next_pending_if_idle(&mut self) -> Option<AppUiCommand> {
        if self.state.active_turn().is_some() || self.state.pending_messages.is_empty() {
            return None;
        }

        let prompt = self.state.pending_messages[0].clone();
        let command = self.start_prompt_turn(prompt, "Submitted staged message");
        if command.is_some() {
            self.state.pending_messages.remove(0);
        }
        command
    }

    fn find_session_mut(
        &mut self,
        session_id: &octos_core::SessionKey,
    ) -> Option<&mut SessionView> {
        self.state
            .sessions
            .iter_mut()
            .find(|session| &session.id == session_id)
    }

    fn turn_had_completion_activity(&self, turn_id: &octos_core::ui_protocol::TurnId) -> bool {
        self.state.activity.iter().any(|activity| {
            activity.turn_id.as_ref() == Some(turn_id)
                && match activity.kind {
                    ActivityKind::Tool => {
                        activity.status == "complete" && activity.success != Some(false)
                    }
                    ActivityKind::Progress => {
                        activity.title == octos_core::ui_protocol::progress_kinds::FILE_MUTATION
                            || activity
                                .detail
                                .as_deref()
                                .is_some_and(|detail| detail.contains("diff preview ready"))
                    }
                    ActivityKind::Approval | ActivityKind::Warning | ActivityKind::Error => false,
                }
        })
    }

    fn clear_matching_approval(&mut self, approval_id: &ApprovalId) -> bool {
        let matches = self
            .state
            .approval
            .as_ref()
            .is_some_and(|approval| &approval.approval_id == approval_id);
        if matches {
            self.state.approval = None;
        }
        matches
    }

    fn find_task_mut(
        &mut self,
        session_id: &octos_core::SessionKey,
        task_id: &TaskId,
    ) -> Option<&mut TaskView> {
        self.find_session_mut(session_id)?
            .tasks
            .iter_mut()
            .find(|task| &task.id == task_id)
    }
}

fn slash_command_try_hint(ctx: &crate::menu::AvailabilityContext<'_>) -> String {
    let registry = CommandRegistry::with_core_commands();
    let names = registry
        .visible_commands(ctx)
        .iter()
        .take(3)
        .map(|visible| visible.command.slash_name())
        .collect::<Vec<_>>();
    match names.len() {
        0 => "a registered command".into(),
        1 => names[0].clone(),
        2 => format!("{} or {}", names[0], names[1]),
        _ => {
            let last = names.last().expect("non-empty command names");
            format!("{}, or {last}", names[..names.len() - 1].join(", "))
        }
    }
}

fn active_menu_item_len(menu: Option<&MenuBuildResult>) -> usize {
    match menu {
        Some(MenuBuildResult::Ready(spec)) => spec.items.len(),
        Some(MenuBuildResult::Loading(_))
        | Some(MenuBuildResult::Unavailable(_))
        | Some(MenuBuildResult::Error(_))
        | None => 0,
    }
}

fn apply_active_menu_search_query(
    result: &mut MenuBuildResult,
    menu_id: &MenuId,
    search_query: &str,
) {
    if search_query.trim().is_empty() || menu_id.as_str() != crate::menu::registry::MENU_HELP {
        return;
    }
    let MenuBuildResult::Ready(spec) = result else {
        return;
    };
    if !spec.searchable {
        return;
    }
    let query = search_query.trim().to_ascii_lowercase();
    spec.items.retain(|item| {
        item.label
            .trim_start_matches('/')
            .to_ascii_lowercase()
            .starts_with(&query)
    });
}

fn active_menu_selected_action(
    menu: &MenuBuildResult,
    selected_index: usize,
) -> Option<MenuAction> {
    match menu {
        MenuBuildResult::Ready(spec) => spec
            .items
            .get(selected_index)
            .filter(|item| item.is_enabled())
            .map(|item| item.action.clone()),
        MenuBuildResult::Loading(_)
        | MenuBuildResult::Unavailable(_)
        | MenuBuildResult::Error(_) => None,
    }
}

#[derive(Default)]
struct TaskCounts {
    total: usize,
    pending: usize,
    running: usize,
    done: usize,
    failed: usize,
}

fn count_tasks(store: &Store) -> TaskCounts {
    let mut counts = TaskCounts::default();
    for task in store
        .state
        .sessions
        .iter()
        .flat_map(|session| session.tasks.iter())
    {
        counts.total += 1;
        match task_state_label(task.state) {
            "pending" => counts.pending += 1,
            "running" => counts.running += 1,
            "done" => counts.done += 1,
            "failed" | "cancelled" => counts.failed += 1,
            _ => {}
        }
    }
    counts
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn tool_invocation_detail(tool_name: &str, arguments: &Value) -> Option<String> {
    fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }

    let detail = match tool_name {
        "shell" => str_field(arguments, "command")?.to_string(),
        "read_file" => {
            let path = str_field(arguments, "path")?;
            let start = arguments.get("start_line").and_then(Value::as_u64);
            let end = arguments.get("end_line").and_then(Value::as_u64);
            match (start, end) {
                (Some(start), Some(end)) => format!("{path}:{start}-{end}"),
                (Some(start), None) => format!("{path}:{start}-"),
                _ => path.to_string(),
            }
        }
        "write_file" => {
            let path = str_field(arguments, "path").unwrap_or(".");
            let content = str_field(arguments, "content")
                .map(compact_preview)
                .unwrap_or_default();
            if content.is_empty() {
                path.to_string()
            } else {
                format!("{path} <= {content}")
            }
        }
        "edit_file" => {
            let path = str_field(arguments, "path").unwrap_or(".");
            let old = str_field(arguments, "old_string")
                .map(compact_preview)
                .unwrap_or_default();
            let new = str_field(arguments, "new_string")
                .map(compact_preview)
                .unwrap_or_default();
            format!("{path}: {old} -> {new}")
        }
        "diff_edit" => {
            let path = str_field(arguments, "path").unwrap_or(".");
            let diff = str_field(arguments, "diff")
                .map(compact_preview)
                .unwrap_or_default();
            if diff.is_empty() {
                path.to_string()
            } else {
                format!("{path} {diff}")
            }
        }
        "list_dir" => str_field(arguments, "path").unwrap_or(".").to_string(),
        "grep" | "grep_tool" => {
            let pattern = str_field(arguments, "pattern")
                .or_else(|| str_field(arguments, "query"))
                .unwrap_or("");
            let path = str_field(arguments, "path").unwrap_or(".");
            format!("{pattern} in {path}")
        }
        "glob" | "glob_tool" => str_field(arguments, "pattern")
            .or_else(|| str_field(arguments, "glob"))
            .unwrap_or("*")
            .to_string(),
        _ => serde_json::to_string(arguments).ok()?,
    };

    Some(detail)
}

fn tool_failure_recovery_hint(tool_name: &str, output_preview: Option<&str>) -> Option<String> {
    let output = output_preview?.to_ascii_lowercase();
    if output.contains("enotfound") && output.contains("registry.npmjs.org") {
        return Some(
            "npm registry DNS failed; retry with an alternate registry, fix DNS/network, or use a local scaffold"
                .into(),
        );
    }

    if output.contains("command timed out") {
        return Some(
            "command timed out; narrow the command, add a timeout, or ask for missing context"
                .into(),
        );
    }

    if output.contains("permission denied")
        || output.contains("operation not permitted")
        || output.contains("eacces")
    {
        return Some(
            "permission blocked; ask for the exact permission/escalation or choose a writable path"
                .into(),
        );
    }

    if output.contains("could not resolve host")
        || output.contains("network is unreachable")
        || output.contains("network request")
        || output.contains("timeout")
    {
        return Some(
            "network access failed; ask for network/proxy/registry permission or use an offline fallback"
                .into(),
        );
    }

    if matches!(tool_name, "web_search" | "web_fetch" | "deep_search")
        && (output.contains("restricted") || output.contains("not configured"))
    {
        return Some(
            "search/fetch is restricted; ask for provider configuration or proceed with an explicit offline caveat"
                .into(),
        );
    }

    None
}

fn compact_preview(value: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut preview = value
        .lines()
        .take(4)
        .collect::<Vec<_>>()
        .join("\\n")
        .trim()
        .to_string();
    if preview.chars().count() > MAX_CHARS {
        preview = preview.chars().take(MAX_CHARS).collect::<String>();
        preview.push_str("...");
    }
    preview
}

fn diff_hunk_context_prompt(context: &DiffHunkContext) -> String {
    let path = match &context.old_path {
        Some(old_path) if old_path != &context.path => format!("{old_path} -> {}", context.path),
        _ => context.path.clone(),
    };
    let mut text = format!(
        "Use this selected diff hunk as context for the next coding turn.\nfile: {path}\nstatus: {}\nhunk: {}\n```diff\n",
        context.file_status, context.hunk_header
    );
    for line in &context.lines {
        text.push_str(diff_context_line_prefix(&line.kind));
        text.push_str(&line.content);
        text.push('\n');
    }
    text.push_str("```");
    text
}

fn diff_context_line_prefix(kind: &str) -> &'static str {
    match kind {
        "added" | "add" | "addition" => "+",
        "removed" | "delete" | "deleted" | "deletion" => "-",
        _ => " ",
    }
}

fn progress_status(event: &UiProgressEvent) -> String {
    let metadata = &event.metadata;
    if let Some(message) = metadata
        .message
        .as_deref()
        .filter(|message| !message.is_empty())
    {
        return message.to_owned();
    }

    if let Some(retry) = &metadata.retry {
        let attempt = retry
            .attempt
            .map(|attempt| attempt.to_string())
            .unwrap_or_else(|| "?".into());
        let max_attempts = retry
            .max_attempts
            .map(|max| format!("/{max}"))
            .unwrap_or_default();
        let reason = retry
            .reason
            .as_deref()
            .filter(|reason| !reason.is_empty())
            .unwrap_or("transient failure");
        return match retry.backoff_ms {
            Some(backoff_ms) => {
                format!("Retrying attempt {attempt}{max_attempts} after {backoff_ms}ms: {reason}")
            }
            None => format!("Retrying attempt {attempt}{max_attempts}: {reason}"),
        };
    }

    if let Some(file_mutation) = &metadata.file_mutation {
        return format!(
            "File mutation: {} {}",
            file_mutation.operation, file_mutation.path
        );
    }

    if let Some(token_cost) = &metadata.token_cost {
        if let Some(total_tokens) = token_cost.total_tokens {
            return format!("Token/cost update: {total_tokens} tokens");
        }
        return "Token/cost update".into();
    }

    if let Some(label) = metadata.label.as_deref().filter(|label| !label.is_empty()) {
        if let Some(detail) = metadata
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
        {
            return format!("{label}: {detail}");
        }
        return label.to_owned();
    }

    format!("Progress: {}", metadata.kind)
}

fn is_noisy_progress_status(status: &str) -> bool {
    let normalized = status.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    status.contains("[progress]")
        || normalized.contains("token/cost_update")
        || normalized.contains("token_cost_update")
        || normalized == "token_cost"
}

fn should_record_progress_activity(event: &UiProgressEvent) -> bool {
    let metadata = &event.metadata;
    if metadata.file_mutation.is_some() || metadata.retry.is_some() {
        return true;
    }

    !is_low_value_progress_metadata(metadata)
}

fn is_low_value_progress_metadata(metadata: &octos_core::ui_protocol::UiProgressMetadata) -> bool {
    metadata.token_cost.is_some()
        || is_low_value_progress_name(&metadata.kind)
        || metadata
            .label
            .as_deref()
            .is_some_and(is_low_value_progress_name)
}

fn is_low_value_progress_name(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    matches!(
        normalized.as_str(),
        "thinking"
            | "response"
            | "stream_start"
            | "stream_end"
            | "token_cost"
            | "token_cost_update"
            | "cost_update"
            | "token_usage"
            | "token_usage_update"
            | "tokens"
            | "tool_started"
            | "tool_progress"
            | "tool_completed"
            | "turn_completed"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use octos_core::SessionKey;
    use octos_core::ui_protocol::{
        ApprovalAutoResolvedEvent, ApprovalCancelledEvent, ApprovalDecidedEvent, ApprovalDecision,
        ApprovalDiffDetails, ApprovalId, ApprovalRequestedEvent, ApprovalTypedDetails,
        OutputCursor, PreviewId, ReplayLossyEvent, TaskRuntimeState, ToolCompletedEvent,
        ToolStartedEvent, TurnId, UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1, UiCursor,
        UiFileMutationNotice, UiProgressMetadata, UiProtocolCapabilities, UiTokenCostUpdate,
        approval_kinds, approval_scopes, methods, progress_kinds,
    };

    fn store_with_live_reply(turn_id: TurnId, text: impl Into<String>) -> Store {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: Some(LiveReply {
                turn_id,
                text: text.into(),
            }),
        };
        Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        }
    }

    fn store_with_task(task_id: TaskId) -> Store {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![TaskView {
                id: task_id,
                title: "task".into(),
                state: TaskRuntimeState::Running,
                runtime_detail: None,
                output_tail: String::new(),
            }],
            live_reply: None,
        };
        Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        }
    }

    fn store_with_empty_session() -> Store {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        }
    }

    fn protocol_store_with_methods(methods: &[&str]) -> Store {
        let mut store = store_with_empty_session();
        store.state.target = Some("ws://example.test/ui-protocol".into());
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods(
            methods.iter().copied(),
        ));
        store
    }

    fn help_menu_labels(store: &Store) -> Vec<String> {
        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected active ready menu");
        };
        spec.items.iter().map(|item| item.label.clone()).collect()
    }

    #[test]
    fn slash_command_registry_matches_exact_alias_and_prefix() {
        let store = store_with_empty_session();

        let exact = store.slash_command_matches("/ps");
        assert_eq!(exact[0].name, "/ps");
        assert!(exact[0].available);
        assert!(exact.iter().any(|command| command.name == "/ps"));

        let alias = store.slash_command_matches("/?");
        assert_eq!(alias[0].name, "/help");

        let prefix = store.slash_command_matches("he");
        assert_eq!(prefix[0].name, "/help");
        assert!(prefix.iter().any(|command| command.name == "/help"));
    }

    #[test]
    fn compose_command_dispatches_help_slash_without_prompt_submission() {
        let mut store = store_with_empty_session();
        store.state.composer = "/help".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert!(store.state.composer.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert!(store.state.pending_messages.is_empty());
        assert!(store.state.menu_stack.is_active());
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_HELP)
        );
    }

    #[test]
    fn unknown_slash_during_active_turn_is_local_error_not_staged_prompt() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id, "working");
        store.state.composer = "/wat".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert!(store.state.composer.is_empty());
        assert!(store.state.pending_messages.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(
            store.state.status,
            "Unknown slash command: /wat. Try /ps, /stop, or /help."
        );
        let activity = store.state.activity.last().expect("local warning activity");
        assert_eq!(activity.kind, ActivityKind::Warning);
        assert_eq!(activity.title, "local slash command");
    }

    #[test]
    fn slash_matches_follow_full_partial_and_missing_capabilities() {
        let mut no_capability = store_with_empty_session();
        no_capability.state.target = Some("ws://example.test/ui-protocol".into());
        let no_capability_names = no_capability
            .slash_command_matches("")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(no_capability_names.contains(&"/status".into()));
        assert!(!no_capability_names.contains(&"/permissions".into()));
        assert!(!no_capability_names.contains(&"/model".into()));
        assert!(!no_capability_names.contains(&"/mcp".into()));

        let partial = protocol_store_with_methods(&[methods::APPROVAL_SCOPES_LIST]);
        let partial_names = partial
            .slash_command_matches("")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(!partial_names.contains(&"/permissions".into()));
        assert!(!partial_names.contains(&"/model".into()));
        assert!(!partial_names.contains(&"/mcp".into()));

        let mut full = protocol_store_with_methods(&[
            methods::APPROVAL_SCOPES_LIST,
            crate::menu::registry::APPUI_METHOD_MODEL_LIST,
            crate::menu::registry::APPUI_METHOD_MCP_STATUS_LIST,
        ]);
        full.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [
                methods::APPROVAL_SCOPES_LIST,
                crate::menu::registry::APPUI_METHOD_MODEL_LIST,
                crate::menu::registry::APPUI_METHOD_MCP_STATUS_LIST,
            ],
            [UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1],
        ));
        let full_names = full
            .slash_command_matches("")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(full_names.contains(&"/model".into()));
        assert!(full_names.contains(&"/permissions".into()));
        assert!(full_names.contains(&"/mcp".into()));

        let approval_feature = {
            let mut store = protocol_store_with_methods(&[methods::APPROVAL_SCOPES_LIST]);
            store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
                [methods::APPROVAL_SCOPES_LIST],
                [UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1],
            ));
            store
        };
        let approval_names = approval_feature
            .slash_command_matches("")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(approval_names.contains(&"/permissions".into()));
    }

    #[test]
    fn client_capability_event_refreshes_active_command_menu() {
        let mut store = store_with_empty_session();
        store.state.target = Some("ws://example.test/ui-protocol".into());
        store.open_menu(MenuId::from(crate::menu::registry::MENU_HELP));
        assert!(!help_menu_labels(&store).contains(&"/model".to_string()));

        store.apply_client_event(ClientEvent::Capabilities(CapabilityClientEvent {
            accepted_capabilities: Vec::new(),
            server_capabilities: UiProtocolCapabilities::new(&[methods::SESSION_OPEN], &[]),
            message: "AppUI capabilities negotiated: 0 accepted".into(),
        }));

        assert!(store.state.capabilities.is_some());
        assert!(!help_menu_labels(&store).contains(&"/permissions".to_string()));
        assert!(!help_menu_labels(&store).contains(&"/model".to_string()));
        assert_eq!(
            store.state.status,
            "AppUI capabilities negotiated: 0 accepted"
        );

        store.apply_client_event(ClientEvent::Capabilities(CapabilityClientEvent {
            accepted_capabilities: Vec::new(),
            server_capabilities: UiProtocolCapabilities::new(
                &[
                    methods::SESSION_OPEN,
                    crate::menu::registry::APPUI_METHOD_MODEL_LIST,
                ],
                &[],
            ),
            message: "AppUI capabilities negotiated: model enabled".into(),
        }));

        assert!(help_menu_labels(&store).contains(&"/model".to_string()));
        assert!(!help_menu_labels(&store).contains(&"/permissions".to_string()));
        assert_eq!(
            store.state.status,
            "AppUI capabilities negotiated: model enabled"
        );

        store.apply_client_event(ClientEvent::Capabilities(CapabilityClientEvent {
            accepted_capabilities: vec![UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1.into()],
            server_capabilities: UiProtocolCapabilities::for_negotiated_features([
                UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1,
            ]),
            message: "AppUI capabilities negotiated: approval enabled".into(),
        }));

        assert!(help_menu_labels(&store).contains(&"/permissions".to_string()));
    }

    #[test]
    fn readonly_slash_stop_is_intercepted_without_prompt_submission() {
        let mut store = store_with_empty_session();
        store.state.readonly = true;
        store.state.composer = "/stop".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert!(store.state.composer.is_empty());
        assert!(store.state.pending_messages.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(
            store.state.status,
            "/stop is unavailable: blocked in read-only mode"
        );
    }

    #[test]
    fn session_bound_slash_is_intercepted_without_open_session() {
        let mut store = Store {
            state: AppState::new(
                Vec::new(),
                0,
                "ready".into(),
                Some("ws://example.test/ui-protocol".into()),
                false,
            ),
        };
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            methods::APPROVAL_SCOPES_LIST,
        ]));
        store.state.composer = "/permissions".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert!(store.state.composer.is_empty());
        assert!(store.state.pending_messages.is_empty());
        assert_eq!(
            store.state.status,
            "/permissions is unavailable: requires an open session"
        );
    }

    #[test]
    fn unavailable_slash_during_active_turn_is_not_staged_as_prompt() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id, "working");
        store.state.target = Some("ws://example.test/ui-protocol".into());
        store.state.composer = "/model".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert!(store.state.composer.is_empty());
        assert!(store.state.pending_messages.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(
            store.state.status,
            "/model is unavailable: AppUI capabilities are not available"
        );
    }

    #[test]
    fn snapshot_preserves_active_menu_stack_and_session_capabilities() {
        let mut store = protocol_store_with_methods(&[]);
        store.open_menu(MenuId::from(crate::menu::registry::MENU_HELP));
        assert!(help_menu_labels(&store).contains(&"/status".to_string()));
        assert!(!help_menu_labels(&store).contains(&"/permissions".to_string()));
        assert!(!help_menu_labels(&store).contains(&"/model".to_string()));

        let session = store.state.sessions[0].clone();
        let opened: octos_core::ui_protocol::SessionOpened =
            serde_json::from_value(serde_json::json!({
                "session_id": session.id,
                "capabilities": octos_core::ui_protocol::UiProtocolCapabilities::new(
                    &[
                        crate::menu::registry::APPUI_METHOD_MODEL_LIST,
                        methods::APPROVAL_SCOPES_LIST,
                    ],
                    &[],
                )
                .with_supported_features([UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1]),
            }))
            .expect("session/opened fixture");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));
        assert!(help_menu_labels(&store).contains(&"/model".to_string()));

        let session = store.state.sessions[0].clone();
        store.apply_event(AppUiEvent::Snapshot(AppUiSnapshot {
            sessions: vec![session],
            selected_session: 0,
            status: "snapshot replay".into(),
            target: Some("ws://example.test/ui-protocol".into()),
            readonly: false,
        }));

        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_HELP)
        );
        assert!(help_menu_labels(&store).contains(&"/model".to_string()));
        assert!(help_menu_labels(&store).contains(&"/permissions".to_string()));
    }

    #[test]
    fn turn_completed_commits_live_reply_into_messages() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "hello");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id,
                cursor: None,
            },
        )));

        assert_eq!(store.state.sessions[0].messages.len(), 1);
        assert!(store.state.sessions[0].live_reply.is_none());
        assert_eq!(store.state.run_state.label(), "done");
    }

    #[test]
    fn turn_completed_ignores_mismatched_live_reply() {
        let live_turn_id = TurnId::new();
        let mut store = store_with_live_reply(live_turn_id.clone(), "do not commit");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id: TurnId::new(),
                cursor: None,
            },
        )));

        assert!(store.state.sessions[0].messages.is_empty());
        let live_reply = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("live reply remains active");
        assert_eq!(live_reply.turn_id, live_turn_id);
        assert_eq!(live_reply.text, "do not commit");
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn message_delta_ignores_mismatched_turn() {
        let live_turn_id = TurnId::new();
        let mut store = store_with_live_reply(live_turn_id, "hello");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id,
                turn_id: TurnId::new(),
                text: " stale".into(),
            },
        )));

        let live_reply = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("live reply remains active");
        assert_eq!(live_reply.text, "hello");
    }

    #[test]
    fn interrupt_command_targets_active_turn() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "streaming");
        let session_id = store.state.sessions[0].id.clone();

        let command = store.interrupt_command().expect("active turn interrupts");

        let AppUiCommand::InterruptTurn(params) = command else {
            panic!("expected interrupt command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.turn_id, turn_id);
        assert_eq!(store.state.status, "Interrupt requested for active turn");
    }

    #[test]
    fn interrupt_command_reports_when_no_turn_is_active() {
        let mut store = store_with_empty_session();

        let command = store.interrupt_command();

        assert!(command.is_none());
        assert_eq!(store.state.status, "No active turn to interrupt");
    }

    #[test]
    fn compose_command_stages_message_during_active_turn() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id, "working");
        store.state.composer = "what is ip for mini5".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert_eq!(store.state.pending_messages, vec!["what is ip for mini5"]);
        assert!(store.state.sessions[0].messages.is_empty());
        assert!(
            store
                .state
                .status
                .contains("Message staged; it will submit after the active turn")
        );
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn compose_command_commits_user_prompt_before_protocol_output() {
        let mut store = store_with_empty_session();
        store.state.composer = "complete m9 contract".into();

        let command = store
            .compose_command()
            .expect("submitted prompt emits command");

        let AppUiCommand::SubmitPrompt(params) = command else {
            panic!("expected prompt submission");
        };
        let messages = &store.state.sessions[0].messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role.as_str(), "user");
        assert_eq!(messages[0].content, "complete m9 contract");

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
            ApprovalRequestedEvent::generic(
                params.session_id,
                ApprovalId::new(),
                params.turn_id,
                "shell",
                "Run command",
                "cargo test -p octos-core ui_protocol",
            ),
        )));

        let messages = &store.state.sessions[0].messages;
        assert_eq!(messages[0].role.as_str(), "user");
        assert_eq!(messages[0].content, "complete m9 contract");
        assert!(store.state.approval.is_some());
        assert_eq!(
            store.state.activity.last().map(|item| item.kind),
            Some(ActivityKind::Approval)
        );
    }

    #[test]
    fn snapshot_replay_keeps_submitted_prompt_before_approval_output() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        store.state.composer = "complete m9 contract".into();

        let command = store
            .compose_command()
            .expect("submitted prompt emits command");
        let AppUiCommand::SubmitPrompt(params) = command else {
            panic!("expected prompt submission");
        };

        store.apply_event(AppUiEvent::Snapshot(AppUiSnapshot {
            sessions: vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            selected_session: 0,
            status: "replayed snapshot".into(),
            target: None,
            readonly: false,
        }));

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
            ApprovalRequestedEvent::generic(
                session_id,
                ApprovalId::new(),
                params.turn_id,
                "shell",
                "Run command",
                "cargo test -p octos-core ui_protocol",
            ),
        )));

        let messages = &store.state.sessions[0].messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role.as_str(), "user");
        assert_eq!(messages[0].content, "complete m9 contract");
        assert!(store.state.approval.is_some());
    }

    #[test]
    fn completed_turn_submits_next_staged_message() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "done");
        let session_id = store.state.sessions[0].id.clone();
        store.state.pending_messages.push("continue now".into());

        let command = store
            .apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
                TurnCompletedEvent {
                    session_id: session_id.clone(),
                    turn_id,
                    cursor: None,
                },
            )))
            .expect("staged prompt submits after turn completion");

        let AppUiCommand::SubmitPrompt(params) = command else {
            panic!("expected staged prompt submission");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(
            params.input,
            vec![InputItem::Text {
                text: "continue now".into()
            }]
        );
        assert!(store.state.pending_messages.is_empty());
        assert_eq!(store.state.sessions[0].messages.len(), 2);
        assert_eq!(store.state.sessions[0].messages[1].content, "continue now");
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn queued_prompt_survives_snapshot_without_entering_old_chat_history() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "working");
        let session_id = store.state.sessions[0].id.clone();
        store.state.sessions[0].messages = vec![
            Message::user("old prompt"),
            Message::assistant("old answer"),
        ];
        store.state.composer = "queued next".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert_eq!(store.state.pending_messages, vec!["queued next"]);
        assert!(
            store.state.sessions[0]
                .messages
                .iter()
                .all(|message| message.content != "queued next")
        );

        store.apply_event(AppUiEvent::Snapshot(AppUiSnapshot {
            sessions: vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("old prompt"),
                    Message::assistant("old answer"),
                ],
                tasks: vec![],
                live_reply: Some(LiveReply {
                    turn_id: turn_id.clone(),
                    text: "working".into(),
                }),
            }],
            selected_session: 0,
            status: "replayed snapshot".into(),
            target: None,
            readonly: false,
        }));

        assert_eq!(store.state.pending_messages, vec!["queued next"]);
        assert!(
            store.state.sessions[0]
                .messages
                .iter()
                .all(|message| message.content != "queued next")
        );

        let command = store
            .apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
                TurnCompletedEvent {
                    session_id: session_id.clone(),
                    turn_id,
                    cursor: None,
                },
            )))
            .expect("queued prompt submits after active turn completes");

        let AppUiCommand::SubmitPrompt(params) = command else {
            panic!("expected queued prompt submission");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(
            params.input,
            vec![InputItem::Text {
                text: "queued next".into()
            }]
        );

        let messages = &store.state.sessions[0].messages;
        assert_eq!(messages[0].content, "old prompt");
        assert_eq!(messages[1].content, "old answer");
        assert_eq!(messages[2].content, "working");
        assert_eq!(messages[3].role.as_str(), "user");
        assert_eq!(messages[3].content, "queued next");
    }

    fn open_generic_approval(store: &mut Store) -> (SessionKey, ApprovalId) {
        let session_id = store.state.sessions[0].id.clone();
        let approval_id = ApprovalId::new();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
            ApprovalRequestedEvent::generic(
                session_id.clone(),
                approval_id.clone(),
                TurnId::new(),
                "shell",
                "Run command",
                "cargo test",
            ),
        )));

        (session_id, approval_id)
    }

    #[test]
    fn approval_request_opens_modal_and_approve_request_emits_scoped_command() {
        let mut store = store_with_live_reply(TurnId::new(), "working");
        let (session_id, approval_id) = open_generic_approval(&mut store);

        let approval = store.state.approval.as_ref().expect("approval is visible");
        assert!(approval.visible);
        assert_eq!(approval.title, "Run command");
        assert_eq!(approval.body, "cargo test");
        assert_eq!(store.state.run_state.label(), "blocked");

        let command = store
            .respond_approval_command(ApprovalModalAction::ApproveRequest)
            .expect("approval response command");

        let AppUiCommand::RespondApproval(params) = command else {
            panic!("expected approval response command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Approve);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::REQUEST)
        );
        assert!(store.state.approval.is_none());
        assert_eq!(
            store.state.status,
            "Approval approved for this request: Run command"
        );
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn approval_response_does_not_restart_completed_turn() {
        let mut store = store_with_empty_session();
        let (_, approval_id) = open_generic_approval(&mut store);
        store.state.set_run_state_success();

        let command = store
            .respond_approval_command(ApprovalModalAction::ApproveSession)
            .expect("approval response command");

        let AppUiCommand::RespondApproval(params) = command else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::SESSION)
        );
        assert_eq!(store.state.run_state.label(), "done");
    }

    #[test]
    fn approval_response_distinguishes_session_approval_and_request_denial() {
        let mut store = store_with_empty_session();
        let (_, approval_id) = open_generic_approval(&mut store);

        let command = store
            .respond_approval_command(ApprovalModalAction::ApproveSession)
            .expect("session approval response command");

        let AppUiCommand::RespondApproval(params) = command else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Approve);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::SESSION)
        );
        assert_eq!(
            store.state.status,
            "Approval approved for this session: Run command"
        );

        let (_, approval_id) = open_generic_approval(&mut store);
        let command = store
            .respond_approval_command(ApprovalModalAction::DenyRequest)
            .expect("denial response command");

        let AppUiCommand::RespondApproval(params) = command else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Deny);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::REQUEST)
        );
        assert_eq!(store.state.status, "Approval denied: Run command");
    }

    #[test]
    fn approval_lifecycle_notifications_clear_matching_modal() {
        let mut store = store_with_empty_session();
        let (session_id, approval_id) = open_generic_approval(&mut store);
        assert!(store.state.approval.is_some());
        assert_eq!(store.state.run_state.label(), "blocked");

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalDecided(
            ApprovalDecidedEvent::manual(
                session_id,
                approval_id,
                TurnId::new(),
                ApprovalDecision::Approve,
                "server",
            ),
        )));

        assert!(store.state.approval.is_none());
        assert_eq!(store.state.run_state.label(), "running");
        assert_eq!(
            store.state.status,
            "Approval decided: approve (decided by server)"
        );
        assert!(store.state.activity.iter().any(|activity| {
            activity.kind == ActivityKind::Approval && activity.title == "decision"
        }));
    }

    #[test]
    fn approval_cancelled_notification_clears_matching_modal() {
        let mut store = store_with_empty_session();
        let (session_id, approval_id) = open_generic_approval(&mut store);

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalCancelled(
            ApprovalCancelledEvent::turn_interrupted(session_id, approval_id, TurnId::new()),
        )));

        assert!(store.state.approval.is_none());
        assert_eq!(store.state.run_state.label(), "running");
        assert_eq!(store.state.status, "Approval cancelled: turn_interrupted");
    }

    #[test]
    fn approval_auto_resolved_notification_records_policy_decision() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalAutoResolved(
            ApprovalAutoResolvedEvent {
                session_id,
                approval_id: ApprovalId::new(),
                turn_id: TurnId::new(),
                tool_name: "shell".into(),
                scope: approval_scopes::SESSION.into(),
                scope_match: "cargo test".into(),
                decision: ApprovalDecision::Approve,
            },
        )));

        assert_eq!(
            store.state.status,
            "Approval auto-resolved (approve) by scope policy"
        );
        assert!(store.state.activity.iter().any(|activity| {
            activity.kind == ActivityKind::Approval
                && activity.title == "shell"
                && activity.status == "auto-resolved approve"
        }));
    }

    #[test]
    fn replay_lossy_notification_surfaces_rehydrate_status() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ReplayLossy(
            ReplayLossyEvent {
                session_id,
                dropped_count: 3,
                last_durable_cursor: Some(UiCursor {
                    stream: "session_events".into(),
                    seq: 42,
                }),
            },
        )));

        assert_eq!(
            store.state.status,
            "Replay lossy: 3 dropped (last durable seq 42); reconnect to rehydrate"
        );
        assert!(store.state.activity.iter().any(|activity| {
            activity.kind == ActivityKind::Warning && activity.title == "replay_lossy"
        }));
    }

    #[test]
    fn diff_approval_request_drives_preview_command() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        let preview_id = PreviewId::new();
        let mut event = ApprovalRequestedEvent::generic(
            session_id.clone(),
            ApprovalId::new(),
            TurnId::new(),
            "diff_edit",
            "Approve diff",
            "Review pending diff",
        );
        event.approval_kind = Some(approval_kinds::DIFF.into());
        event.typed_details = Some(ApprovalTypedDetails {
            kind: approval_kinds::DIFF.into(),
            command: None,
            sandbox: None,
            diff: Some(ApprovalDiffDetails {
                preview_id: preview_id.clone(),
                operation: Some("apply".into()),
                file_count: Some(1),
                additions: Some(4),
                deletions: Some(1),
                summary: Some("Update tests".into()),
            }),
            filesystem: None,
            network: None,
            sandbox_escalation: None,
        });

        let command = store
            .apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
                event,
            )))
            .expect("diff approval preview command");
        let AppUiCommand::GetDiffPreview(params) = command else {
            panic!("expected diff preview command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.preview_id, preview_id);
        assert_eq!(
            store.state.status,
            "Opening inline diff preview: Approve diff"
        );
        assert_eq!(store.state.run_state.label(), "blocked");
        assert!(store.state.diff_preview.active);
        assert!(store.state.diff_preview.loading);
        assert!(
            store
                .state
                .approval
                .as_ref()
                .is_some_and(|approval| approval.visible)
        );
    }

    #[test]
    fn completed_turn_marks_live_plan_done_after_successful_tool_activity() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(
            turn_id.clone(),
            "Plan:\n1. [ ] Fix store/model progress handling\n2. [ ] Run tests",
        );
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
            ToolStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                tool_name: "shell".into(),
                arguments: Some(serde_json::json!({"command": "cargo test"})),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                tool_name: "shell".into(),
                success: Some(true),
                output_preview: Some("tests passed".into()),
                duration_ms: Some(100),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id,
                cursor: None,
            },
        )));

        let content = &store.state.sessions[0].messages[0].content;
        assert!(content.contains("- [x] Fix store/model progress handling"));
        assert!(content.contains("- [x] Run tests"));
    }

    #[test]
    fn tool_notifications_update_activity_card_state() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        let turn_id = TurnId::new();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
            ToolStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                tool_name: "shell".into(),
                arguments: Some(serde_json::json!({"command": "cargo test"})),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolProgress(
            octos_core::ui_protocol::ToolProgressEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                message: Some("cargo test".into()),
                progress_pct: Some(50.0),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id,
                turn_id,
                tool_call_id: "call-1".into(),
                tool_name: "shell".into(),
                success: Some(true),
                output_preview: Some("6 tests passed".into()),
                duration_ms: Some(1250),
            },
        )));

        assert_eq!(store.state.activity.len(), 1);
        let activity = &store.state.activity[0];
        assert_eq!(activity.kind, ActivityKind::Tool);
        assert_eq!(activity.title, "shell");
        assert_eq!(activity.status, "complete");
        assert_eq!(activity.detail.as_deref(), Some("cargo test"));
        assert_eq!(activity.output_preview.as_deref(), Some("6 tests passed"));
        assert_eq!(activity.success, Some(true));
        assert_eq!(activity.duration_ms, Some(1250));
        assert_eq!(activity.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn failed_tool_surfaces_recovery_suggestion() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        let turn_id = TurnId::new();
        let tool_call_id = "call-1".to_string();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
            ToolStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_call_id: tool_call_id.clone(),
                tool_name: "shell".into(),
                arguments: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id,
                turn_id,
                tool_call_id,
                tool_name: "shell".into(),
                success: Some(false),
                output_preview: Some(
                    "npm error code ENOTFOUND\ngetaddrinfo ENOTFOUND registry.npmjs.org".into(),
                ),
                duration_ms: Some(70_000),
            },
        )));

        assert!(
            store
                .state
                .status
                .contains("npm registry DNS failed; retry with an alternate registry")
        );
        assert!(store.state.activity.iter().any(|activity| {
            activity.kind == ActivityKind::Warning
                && activity.title == "Recovery suggestion"
                && activity.status.contains("npm registry DNS failed")
        }));
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn command_timeout_recovery_is_not_misclassified_as_network() {
        assert_eq!(
            tool_failure_recovery_hint("shell", Some("Command timed out after 15 seconds"))
                .as_deref(),
            Some(
                "command timed out; narrow the command, add a timeout, or ask for missing context"
            )
        );
    }

    #[test]
    fn close_modal_hides_pending_approval_without_responding() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
            ApprovalRequestedEvent::generic(
                session_id,
                ApprovalId::new(),
                TurnId::new(),
                "shell",
                "Run command",
                "cargo test",
            ),
        )));

        assert!(store.close_modal());
        let approval = store
            .state
            .approval
            .as_ref()
            .expect("approval remains pending");
        assert!(!approval.visible);
        assert!(!store.state.approval_auto_open);
        assert_eq!(
            store.state.status,
            "Approval pane hidden; auto-open disabled until approval is shown again"
        );
    }

    #[test]
    fn approval_auto_open_setting_applies_to_next_request() {
        let mut store = store_with_empty_session();
        store.state.approval_auto_open = false;
        store.state.focus = FocusPane::Git;
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
            ApprovalRequestedEvent::generic(
                session_id,
                ApprovalId::new(),
                TurnId::new(),
                "shell",
                "Run command",
                "cargo test",
            ),
        )));

        let approval = store.state.approval.as_ref().expect("approval pending");
        assert!(!approval.visible);
        assert_eq!(store.state.focus, FocusPane::Composer);
        assert_eq!(store.state.run_state.label(), "blocked");
    }

    #[test]
    fn task_output_read_command_targets_selected_task_cursor() {
        let task_id = TaskId::new();
        let mut store = store_with_task(task_id.clone());
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TaskOutputDelta(
            TaskOutputDeltaEvent {
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                text: "line one\n".into(),
                cursor: OutputCursor { offset: 9 },
            },
        )));

        let command = store
            .read_task_output_command()
            .expect("selected task output command");

        let AppUiCommand::ReadTaskOutput(params) = command else {
            panic!("expected task output read command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.task_id, task_id);
        assert_eq!(params.cursor, Some(OutputCursor { offset: 9 }));
        assert_eq!(params.limit_bytes, Some(TASK_OUTPUT_READ_LIMIT_BYTES));
        assert!(store.state.task_output.active);
        assert_eq!(store.state.task_output.title, "task");
        assert_eq!(store.state.task_output.output, "line one\n");
    }

    #[test]
    fn no_active_task_or_approval_reports_status_without_command() {
        let mut store = store_with_empty_session();

        let approval_command = store.respond_approval_command(ApprovalModalAction::DenyRequest);

        assert!(approval_command.is_none());
        assert_eq!(store.state.status, "No active approval request");

        let task_command = store.read_task_output_command();

        assert!(task_command.is_none());
        assert_eq!(store.state.status, "No selected task output to read");
    }

    #[test]
    fn diff_preview_command_uses_selected_task_preview_id_when_present() {
        let task_id = TaskId::new();
        let preview_id = PreviewId::new();
        let mut store = store_with_task(task_id);
        let session_id = store.state.sessions[0].id.clone();
        store.state.sessions[0].tasks[0].runtime_detail =
            Some(format!("pending diff preview_id: {}", preview_id.0));

        let command = store
            .read_diff_preview_command()
            .expect("diff preview command");

        let AppUiCommand::GetDiffPreview(params) = command else {
            panic!("expected diff preview command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.preview_id, preview_id);
        assert!(store.state.diff_preview.active);
        assert!(store.state.diff_preview.loading);
        assert_eq!(store.state.status, "Requested diff preview");
    }

    #[test]
    fn diff_preview_without_protocol_id_reports_status_without_command() {
        let mut store = store_with_task(TaskId::new());

        let command = store.read_diff_preview_command();

        assert!(command.is_none());
        assert_eq!(
            store.state.status,
            "No diff preview id is available for the selected task"
        );
        assert!(!store.state.diff_preview.active);
    }

    #[test]
    fn diff_preview_result_updates_visible_pane_and_status() {
        let mut store = store_with_empty_session();
        let preview_id = PreviewId::new();

        store.apply_client_event(ClientEvent::DiffPreview(DiffPreviewGetResult {
            status: "requires_refresh".into(),
            source: "future_cache".into(),
            preview: crate::model::DiffPreview {
                session_id: store.state.sessions[0].id.clone(),
                preview_id,
                title: Some("Patch".into()),
                files: vec![crate::model::DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "copied".into(),
                    hunks: vec![crate::model::DiffPreviewHunk {
                        header: "@@ metadata @@".into(),
                        lines: vec![crate::model::DiffPreviewLine {
                            kind: "metadata".into(),
                            content: "mode change".into(),
                            old_line: None,
                            new_line: None,
                        }],
                    }],
                }],
            },
        }));

        assert!(store.state.diff_preview.active);
        assert!(!store.state.diff_preview.loading);
        assert_eq!(
            store.state.diff_preview.status.as_deref(),
            Some("requires_refresh")
        );
        assert_eq!(
            store.state.diff_preview.source.as_deref(),
            Some("future_cache")
        );
        assert_eq!(
            store.state.status,
            "Diff preview requires_refresh: Patch (1 files)"
        );
    }

    #[test]
    fn selected_diff_hunk_context_can_be_staged_for_next_turn() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id, "working");
        let preview_id = PreviewId::new();
        store.apply_client_event(ClientEvent::DiffPreview(DiffPreviewGetResult {
            status: "ready".into(),
            source: "cache".into(),
            preview: crate::model::DiffPreview {
                session_id: store.state.sessions[0].id.clone(),
                preview_id,
                title: Some("Patch".into()),
                files: vec![crate::model::DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![crate::model::DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            crate::model::DiffPreviewLine {
                                kind: "removed".into(),
                                content: "old".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            crate::model::DiffPreviewLine {
                                kind: "added".into(),
                                content: "new".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                        ],
                    }],
                }],
            },
        }));

        store.stage_selected_diff_context();

        assert_eq!(store.state.pending_messages.len(), 1);
        assert!(store.state.pending_messages[0].contains("file: src/lib.rs"));
        assert!(store.state.pending_messages[0].contains("-old"));
        assert!(store.state.pending_messages[0].contains("+new"));
        assert_eq!(
            store.state.status,
            "Staged selected diff hunk context for next turn: src/lib.rs"
        );
    }

    #[test]
    fn interrupted_turn_error_clears_live_reply_and_reports_cancel_status() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "streaming");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id,
                turn_id,
                code: "interrupted".into(),
                message: "turn interrupted by client".into(),
            },
        )));

        assert!(store.state.sessions[0].live_reply.is_none());
        assert_eq!(
            store.state.status,
            "Turn error interrupted: turn interrupted by client"
        );
        assert_eq!(store.state.run_state.label(), "error");
        assert_eq!(
            store.state.run_state.detail(),
            Some("interrupted: turn interrupted by client")
        );
    }

    #[test]
    fn progress_event_updates_status_without_becoming_protocol_unknown() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id,
            None,
            UiProgressMetadata::file_mutation(UiFileMutationNotice::new("src/main.rs", "modify")),
        )));

        assert_eq!(store.state.status, "File mutation: modify src/main.rs");
    }

    #[test]
    fn file_mutation_progress_with_preview_id_requests_and_opens_diff_preview() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        let preview_id = PreviewId::new();
        let mut notice = UiFileMutationNotice::new("src/lib.rs", "modify");
        notice.preview_id = Some(preview_id.clone());

        let command = store
            .apply_event(AppUiEvent::Progress(UiProgressEvent::new(
                session_id.clone(),
                Some(TurnId::new()),
                UiProgressMetadata::file_mutation(notice),
            )))
            .expect("diff preview request command");

        let AppUiCommand::GetDiffPreview(params) = command else {
            panic!("expected diff preview command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.preview_id, preview_id);
        assert!(store.state.diff_preview.active);
        assert!(store.state.diff_preview.loading);
        assert_eq!(
            store.state.diff_preview.requested_preview_id,
            Some(preview_id)
        );
        assert_eq!(
            store.state.status,
            "Opening diff preview: modify src/lib.rs"
        );
        assert_eq!(
            store
                .state
                .activity
                .last()
                .and_then(|activity| activity.detail.as_deref()),
            Some("modify src/lib.rs | diff preview ready")
        );
    }

    #[test]
    fn progress_event_prefers_user_facing_message() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id,
            None,
            UiProgressMetadata::new(progress_kinds::THINKING).with_message("Thinking"),
        )));

        assert_eq!(store.state.status, "Thinking");
    }

    #[test]
    fn low_value_progress_updates_status_without_activity() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        store.state.status = "Working".into();
        let mut token_cost = UiTokenCostUpdate::new();
        token_cost.total_tokens = Some(123);

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id.clone(),
            Some(TurnId::new()),
            UiProgressMetadata::token_cost(token_cost),
        )));

        assert_eq!(store.state.status, "Working");
        assert!(store.state.activity.is_empty());
        assert_eq!(store.state.run_state.label(), "running");

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id.clone(),
            None,
            UiProgressMetadata::new(progress_kinds::STREAM_END).with_message("stream closed"),
        )));

        assert_eq!(store.state.status, "stream closed");
        assert!(store.state.activity.is_empty());

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id,
            Some(TurnId::new()),
            UiProgressMetadata::new("tool_completed").with_message("[progress] tool_completed"),
        )));

        assert_eq!(store.state.status, "stream closed");
        assert!(store.state.activity.is_empty());
    }

    #[test]
    fn important_progress_still_records_activity() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id,
            None,
            UiProgressMetadata::file_mutation(UiFileMutationNotice::new("src/main.rs", "modify")),
        )));

        assert_eq!(store.state.activity.len(), 1);
        let activity = &store.state.activity[0];
        assert_eq!(activity.kind, ActivityKind::Progress);
        assert_eq!(activity.title, progress_kinds::FILE_MUTATION);
        assert_eq!(activity.status, "File mutation: modify src/main.rs");
    }

    #[test]
    fn turn_error_ignores_mismatched_live_reply() {
        let live_turn_id = TurnId::new();
        let mut store = store_with_live_reply(live_turn_id.clone(), "still streaming");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id,
                turn_id: TurnId::new(),
                code: "stale_error".into(),
                message: "old turn failed".into(),
            },
        )));

        let live_reply = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("live reply remains active");
        assert_eq!(live_reply.turn_id, live_turn_id);
        assert_eq!(live_reply.text, "still streaming");
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn readonly_store_does_not_emit_submit_prompt() {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        let mut store = Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, true),
        };
        store.state.composer = "blocked prompt".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert!(store.state.sessions[0].messages.is_empty());
        assert!(store.state.composer.is_empty());
        assert_eq!(store.state.status, "Read-only mode: turn/start disabled");
    }

    #[test]
    fn task_output_tail_truncates_on_utf8_boundary() {
        let task_id = TaskId::new();
        let mut store = store_with_task(task_id.clone());
        let session_id = store.state.sessions[0].id.clone();
        let retained_tail = "é".repeat(299);
        let text = format!("界{retained_tail}");

        store.apply_event(AppUiEvent::Protocol(UiNotification::TaskOutputDelta(
            TaskOutputDeltaEvent {
                session_id,
                task_id,
                text,
                cursor: OutputCursor { offset: 601 },
            },
        )));

        assert_eq!(store.state.sessions[0].tasks[0].output_tail, retained_tail);
        assert!(store.state.sessions[0].tasks[0].output_tail.len() <= TASK_OUTPUT_TAIL_BYTES);
    }
}
