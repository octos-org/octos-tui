use octos_core::app_ui::{AppUiEvent, AppUiSnapshot};
use octos_core::ui_protocol::{
    ApprovalAutoResolvedEvent, ApprovalCancelledEvent, ApprovalDecidedEvent, ApprovalId,
    ApprovalRespondParams, DiffPreviewGetParams, InputItem, MessageDeltaEvent,
    MessagePersistedEvent, ReplayLossyEvent, SessionOpenParams, TaskOutputDeltaEvent,
    TaskOutputReadParams, TaskRuntimeState, TaskUpdatedEvent, TurnCompletedEvent, TurnErrorEvent,
    TurnId, TurnInterruptParams, TurnStartParams, UiNotification, UiProgressEvent,
};
use octos_core::{Message, TaskId};
use serde_json::Value;

use crate::{
    client_event::{
        CapabilitiesClientEvent, ClientEvent, McpConfigListClientEvent,
        McpConfigMutationClientEvent, McpStatusClientEvent, ModelListClientEvent,
        ModelSelectClientEvent, PermissionProfileClientEvent, ProfileLlmCatalogClientEvent,
        ProfileLlmListClientEvent, ProfileLlmMutationClientEvent, ProfileSkillsListClientEvent,
        ProfileSkillsMutationClientEvent, ProfileSkillsRegistrySearchClientEvent,
        SessionStatusClientEvent, ToolConfigListClientEvent, ToolConfigMutationClientEvent,
        ToolStatusClientEvent,
    },
    menu::{
        CommandEntry, CommandRegistry, CommandResolution, LocalAction, MenuAction, MenuAppSnapshot,
        MenuBuildResult, MenuContext, MenuId, TerminalSize, providers::core_menu_registry,
    },
    model::{
        ActivityItem, ActivityKind, AppState, AppUiCommand, ApprovalModalAction,
        ApprovalModalState, AuthSendCodeParams, AuthVerifyParams, DiffHunkContext,
        DiffPreviewGetResult, FocusPane, LiveReply, LlmRouteConfig, LlmSelectionConfig,
        McpConfigDeleteParams, McpConfigListParams, McpConfigSetEnabledParams, McpConfigTestParams,
        McpConfigUpsertParams, OnboardingAction, OnboardingProviderPending,
        OnboardingProviderSaveTarget, ProfileLlmCatalogParams, ProfileLlmListParams,
        ProfileLlmListResult, ProfileLocalCreateParams, ProfileSkillsInstallParams,
        ProfileSkillsListParams, ProfileSkillsRegistrySearchParams, ProfileSkillsRemoveParams,
        SecretString, SessionMcpCatalog, SessionModelCatalog, SessionRuntimeStatus,
        SessionToolCatalog, SessionView, TaskView, ToolConfigDeleteParams, ToolConfigListParams,
        ToolConfigSetEnabledParams, ToolConfigTestParams, ToolConfigUpsertParams,
        complete_plan_steps_in_text, task_state_label,
    },
};

const TASK_OUTPUT_TAIL_BYTES: usize = 600;
const TASK_OUTPUT_READ_LIMIT_BYTES: u64 = 4096;

#[derive(Default)]
struct TurnActivitySummary {
    action_count: usize,
    files_changed: Vec<String>,
    validation: Vec<String>,
    failures: Vec<String>,
}

fn looks_like_validation_activity(activity: &ActivityItem) -> bool {
    let text = format!(
        "{} {}",
        activity.title,
        activity.detail.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    text.contains("test")
        || text.contains("build")
        || text.contains("check")
        || text.contains("lint")
        || text.contains("cargo ")
        || text.contains("pytest")
        || text.contains("npm run")
        || text.contains("pnpm ")
}

fn looks_like_file_change_activity(activity: &ActivityItem) -> bool {
    let text = format!(
        "{} {} {}",
        activity.title,
        activity.status,
        activity.detail.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    text.contains("file mutation")
        || text.contains("diff preview")
        || text.contains(" modified")
        || text.contains(" created")
        || text.contains(" deleted")
}

fn compact_first_line(value: &str, max_chars: usize) -> String {
    let line = value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut out = String::new();
    for ch in line.chars().take(max_chars) {
        out.push(ch);
    }
    if line.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn push_unique_summary(values: &mut Vec<String>, value: String) {
    if value.is_empty() || values.iter().any(|existing| existing == &value) {
        return;
    }
    values.push(value);
}

fn format_limited_list(values: &[String], empty: &str) -> String {
    if values.is_empty() {
        return empty.to_string();
    }
    let mut rendered = values
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > 3 {
        rendered.push_str(&format!(", +{} more", values.len() - 3));
    }
    rendered
}

fn looks_like_partial_live_answer(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.lines().count() > 1 || trimmed.chars().count() < 32 {
        return false;
    }
    !trimmed
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | ':' | ')' | ']' | '`'))
}

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

        if self.active_session().is_none() {
            self.state.status =
                "No coding session open. Run /onboard open-session before sending a prompt.".into();
            self.state.focus = FocusPane::Composer;
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
                let names = std::iter::once(command.name).chain(command.aliases.iter().copied());
                let rank = if query.is_empty() {
                    Some(0)
                } else if names
                    .clone()
                    .any(|name| name.eq_ignore_ascii_case(query.as_str()))
                {
                    Some(0)
                } else if names
                    .clone()
                    .any(|name| name.to_ascii_lowercase().starts_with(&query))
                {
                    Some(1)
                } else if names
                    .clone()
                    .any(|name| name.to_ascii_lowercase().contains(&query))
                    || command.description.to_ascii_lowercase().contains(&query)
                {
                    Some(2)
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
                invocation,
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
                self.dispatch_command_entry(&command.entry, Some(invocation.args))
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

    fn dispatch_command_entry(
        &mut self,
        entry: &CommandEntry,
        inline_args: Option<&str>,
    ) -> Option<AppUiCommand> {
        match entry {
            CommandEntry::OpenMenu(id) => {
                self.open_menu(id.clone());
                None
            }
            CommandEntry::LocalAction(action) => {
                self.dispatch_local_action(action.clone(), inline_args)
            }
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

    fn dispatch_local_action(
        &mut self,
        action: LocalAction,
        inline_args: Option<&str>,
    ) -> Option<AppUiCommand> {
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
            LocalAction::Exit => {
                self.state.exit_requested = true;
                None
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
            LocalAction::EditComposer(draft) => {
                self.state.set_composer_text(draft);
                self.state.focus = FocusPane::Composer;
                self.state.status = "Edit the field, then press Enter".into();
                None
            }
            LocalAction::Onboarding(action) => self.dispatch_onboarding_action(action, inline_args),
            LocalAction::Skills => self.dispatch_skills_inline(inline_args.unwrap_or_default()),
            LocalAction::McpConfig => self.dispatch_mcp_inline(inline_args.unwrap_or_default()),
            LocalAction::ToolConfig => self.dispatch_tools_inline(inline_args.unwrap_or_default()),
            LocalAction::Custom(name) => {
                self.state.status = format!("Local menu action `{name}` is not wired yet");
                None
            }
        }
    }

    fn dispatch_mcp_inline(&mut self, inline_args: &str) -> Option<AppUiCommand> {
        let args = inline_args.trim();
        if args.is_empty() {
            self.open_menu(MenuId::from(crate::menu::registry::MENU_MCP));
            return None;
        }

        let (verb, rest) = split_first_word(args);
        match verb {
            "list" | "refresh" | "config" => self.mcp_config_list_command(),
            "status" => self.mcp_status_list_command(),
            "enable" => self.mcp_config_set_enabled_command(rest, true),
            "disable" => self.mcp_config_set_enabled_command(rest, false),
            "test" => self.mcp_config_test_command(rest),
            "delete" | "remove" | "rm" => self.mcp_config_delete_command(rest),
            "upsert" | "add" | "set" => self.mcp_config_upsert_command(rest),
            "help" => {
                self.state.status = mcp_usage();
                self.open_menu(MenuId::from(crate::menu::registry::MENU_MCP));
                None
            }
            _ => {
                self.state.status = mcp_usage();
                None
            }
        }
    }

    fn dispatch_tools_inline(&mut self, inline_args: &str) -> Option<AppUiCommand> {
        let args = inline_args.trim();
        if args.is_empty() {
            self.open_menu(MenuId::from(crate::menu::registry::MENU_TOOL_SETTINGS));
            return None;
        }

        let (verb, rest) = split_first_word(args);
        match verb {
            "list" | "refresh" | "config" => self.tool_config_list_command(),
            "status" => self.tool_status_list_command(),
            "enable" => self.tool_config_set_enabled_command(rest, true),
            "disable" => self.tool_config_set_enabled_command(rest, false),
            "test" => self.tool_config_test_command(rest),
            "delete" | "remove" | "rm" => self.tool_config_delete_command(rest),
            "upsert" | "add" | "set" => self.tool_config_upsert_command(rest),
            "help" => {
                self.state.status = tools_usage();
                self.open_menu(MenuId::from(crate::menu::registry::MENU_TOOL_SETTINGS));
                None
            }
            _ => {
                self.state.status = tools_usage();
                None
            }
        }
    }

    fn dispatch_skills_inline(&mut self, inline_args: &str) -> Option<AppUiCommand> {
        let args = inline_args.trim();
        if args.is_empty() {
            self.open_menu(MenuId::from(crate::menu::registry::MENU_SKILLS));
            return None;
        }

        let (verb, rest) = split_first_word(args);
        match verb {
            "list" | "installed" | "refresh" => self.profile_skills_list_command(),
            "search" | "registry" => {
                let query = rest.trim();
                if query.is_empty() {
                    self.state.status = skills_usage();
                    return None;
                }
                self.profile_skills_registry_search_command(query.to_owned())
            }
            "install" | "add" => match parse_skill_install_args(rest) {
                Ok((repo, branch, force)) => {
                    self.profile_skills_install_command(repo, branch, force)
                }
                Err(message) => {
                    self.state.status = message;
                    None
                }
            },
            "remove" | "rm" | "uninstall" => {
                let (name, trailing) = split_first_word(rest);
                if name.is_empty() || !trailing.trim().is_empty() {
                    self.state.status = "Usage: /skills remove <name>".into();
                    return None;
                }
                self.profile_skills_remove_command(name.to_owned())
            }
            "help" => {
                self.state.status = skills_usage();
                self.open_menu(MenuId::from(crate::menu::registry::MENU_SKILLS));
                None
            }
            _ => {
                self.state.status = skills_usage();
                None
            }
        }
    }

    fn profile_skills_list_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST) {
            return None;
        }
        self.state.status = "Refreshing profile skills".into();
        Some(AppUiCommand::ProfileSkillsList(ProfileSkillsListParams {
            profile_id: self.current_profile_for_onboarding(),
        }))
    }

    fn profile_skills_registry_search_command(&mut self, query: String) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH) {
            return None;
        }
        self.state.status = format!("Searching skill registry for `{query}`");
        Some(AppUiCommand::ProfileSkillsRegistrySearch(
            ProfileSkillsRegistrySearchParams {
                profile_id: self.current_profile_for_onboarding(),
                q: Some(query),
            },
        ))
    }

    fn profile_skills_install_command(
        &mut self,
        repo: String,
        branch: Option<String>,
        force: bool,
    ) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL) {
            return None;
        }
        if self.state.readonly {
            self.state.status = "Read-only mode: profile/skills/install disabled".into();
            return None;
        }
        self.state.status = format!("Installing profile skill from `{repo}`");
        Some(AppUiCommand::ProfileSkillsInstall(
            ProfileSkillsInstallParams {
                profile_id: self.current_profile_for_onboarding(),
                repo,
                branch,
                force,
            },
        ))
    }

    fn profile_skills_remove_command(&mut self, name: String) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_SKILLS_REMOVE) {
            return None;
        }
        if self.state.readonly {
            self.state.status = "Read-only mode: profile/skills/remove disabled".into();
            return None;
        }
        self.state.status = format!("Removing profile skill `{name}`");
        Some(AppUiCommand::ProfileSkillsRemove(
            ProfileSkillsRemoveParams {
                profile_id: self.current_profile_for_onboarding(),
                name,
            },
        ))
    }

    fn mcp_config_list_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_MCP_CONFIG_LIST) {
            return None;
        }
        self.state.status = "Refreshing MCP config".into();
        Some(AppUiCommand::ListMcpConfig(McpConfigListParams {
            session_id: self.active_session().map(|session| session.id.clone()),
            profile_id: self.current_profile_for_onboarding(),
            include_disabled: true,
        }))
    }

    fn mcp_status_list_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_MCP_STATUS_LIST) {
            return None;
        }
        let Some(session_id) = self.active_session().map(|session| session.id.clone()) else {
            self.state.status = "MCP status requires an open session".into();
            return None;
        };
        self.state.status = "Refreshing MCP status".into();
        Some(AppUiCommand::ListMcpStatus(
            crate::model::McpStatusListParams {
                session_id,
                include_disabled: true,
            },
        ))
    }

    fn mcp_config_set_enabled_command(
        &mut self,
        rest: &str,
        enabled: bool,
    ) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_MCP_CONFIG_SET_ENABLED) {
            return None;
        }
        let Some(server) = parse_single_name(rest, "Usage: /mcp enable <server>") else {
            self.state.status = "Usage: /mcp enable <server>".into();
            return None;
        };
        self.state.status = format!(
            "{} MCP config `{server}`",
            if enabled { "Enabling" } else { "Disabling" }
        );
        Some(AppUiCommand::SetMcpConfigEnabled(
            McpConfigSetEnabledParams {
                profile_id: self.current_profile_for_onboarding(),
                server,
                enabled,
            },
        ))
    }

    fn mcp_config_test_command(&mut self, rest: &str) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_MCP_CONFIG_TEST) {
            return None;
        }
        let Some(server) = parse_single_name(rest, "Usage: /mcp test <server>") else {
            self.state.status = "Usage: /mcp test <server>".into();
            return None;
        };
        self.state.status = format!("Testing MCP config `{server}`");
        Some(AppUiCommand::TestMcpConfig(McpConfigTestParams {
            session_id: self.active_session().map(|session| session.id.clone()),
            profile_id: self.current_profile_for_onboarding(),
            server,
        }))
    }

    fn mcp_config_delete_command(&mut self, rest: &str) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_MCP_CONFIG_DELETE) {
            return None;
        }
        let Some(server) = parse_single_name(rest, "Usage: /mcp delete <server>") else {
            self.state.status = "Usage: /mcp delete <server>".into();
            return None;
        };
        self.state.status = format!("Deleting MCP config `{server}`");
        Some(AppUiCommand::DeleteMcpConfig(McpConfigDeleteParams {
            profile_id: self.current_profile_for_onboarding(),
            server,
        }))
    }

    fn mcp_config_upsert_command(&mut self, rest: &str) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_MCP_CONFIG_UPSERT) {
            return None;
        }
        let Ok((server, config)) = parse_name_and_json(rest, mcp_usage()) else {
            self.state.status = mcp_usage();
            return None;
        };
        self.state.status = format!("Upserting MCP config `{server}`");
        Some(AppUiCommand::UpsertMcpConfig(McpConfigUpsertParams {
            profile_id: self.current_profile_for_onboarding(),
            server,
            config,
            enabled: None,
        }))
    }

    fn tool_config_list_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_TOOL_CONFIG_LIST) {
            return None;
        }
        self.state.status = "Refreshing tool config".into();
        Some(AppUiCommand::ListToolConfig(ToolConfigListParams {
            session_id: self.active_session().map(|session| session.id.clone()),
            profile_id: self.current_profile_for_onboarding(),
            include_disabled: true,
        }))
    }

    fn tool_status_list_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_TOOL_STATUS_LIST) {
            return None;
        }
        let Some(session_id) = self.active_session().map(|session| session.id.clone()) else {
            self.state.status = "Tool status requires an open session".into();
            return None;
        };
        self.state.status = "Refreshing tool status".into();
        Some(AppUiCommand::ListToolStatus(
            crate::model::ToolStatusListParams {
                session_id,
                include_denied: true,
            },
        ))
    }

    fn tool_config_set_enabled_command(
        &mut self,
        rest: &str,
        enabled: bool,
    ) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_TOOL_CONFIG_SET_ENABLED) {
            return None;
        }
        let Some(tool) = parse_single_name(rest, "Usage: /tools enable <tool>") else {
            self.state.status = "Usage: /tools enable <tool>".into();
            return None;
        };
        self.state.status = format!(
            "{} tool config `{tool}`",
            if enabled { "Enabling" } else { "Disabling" }
        );
        Some(AppUiCommand::SetToolConfigEnabled(
            ToolConfigSetEnabledParams {
                profile_id: self.current_profile_for_onboarding(),
                tool,
                enabled,
            },
        ))
    }

    fn tool_config_test_command(&mut self, rest: &str) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_TOOL_CONFIG_TEST) {
            return None;
        }
        let Some(tool) = parse_single_name(rest, "Usage: /tools test <tool>") else {
            self.state.status = "Usage: /tools test <tool>".into();
            return None;
        };
        self.state.status = format!("Testing tool config `{tool}`");
        Some(AppUiCommand::TestToolConfig(ToolConfigTestParams {
            session_id: self.active_session().map(|session| session.id.clone()),
            profile_id: self.current_profile_for_onboarding(),
            tool,
        }))
    }

    fn tool_config_delete_command(&mut self, rest: &str) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_TOOL_CONFIG_DELETE) {
            return None;
        }
        let Some(tool) = parse_single_name(rest, "Usage: /tools delete <tool>") else {
            self.state.status = "Usage: /tools delete <tool>".into();
            return None;
        };
        self.state.status = format!("Deleting tool config `{tool}`");
        Some(AppUiCommand::DeleteToolConfig(ToolConfigDeleteParams {
            profile_id: self.current_profile_for_onboarding(),
            tool,
        }))
    }

    fn tool_config_upsert_command(&mut self, rest: &str) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_TOOL_CONFIG_UPSERT) {
            return None;
        }
        let Ok((tool, config)) = parse_name_and_json(rest, tools_usage()) else {
            self.state.status = tools_usage();
            return None;
        };
        self.state.status = format!("Upserting tool config `{tool}`");
        Some(AppUiCommand::UpsertToolConfig(ToolConfigUpsertParams {
            profile_id: self.current_profile_for_onboarding(),
            tool,
            config,
            enabled: None,
        }))
    }

    fn dispatch_onboarding_action(
        &mut self,
        action: OnboardingAction,
        inline_args: Option<&str>,
    ) -> Option<AppUiCommand> {
        if matches!(action, OnboardingAction::Open)
            && inline_args.is_some_and(|args| !args.trim().is_empty())
        {
            return self.dispatch_onboarding_inline(inline_args.unwrap_or_default());
        }

        match action {
            OnboardingAction::Open => {
                self.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
                None
            }
            OnboardingAction::OpenLogin => {
                if inline_args.is_some_and(|args| !args.trim().is_empty()) {
                    self.dispatch_login_inline(inline_args.unwrap_or_default())
                } else {
                    self.open_menu(MenuId::from(crate::menu::registry::MENU_LOGIN));
                    None
                }
            }
            OnboardingAction::OpenProvider => {
                if inline_args.is_some_and(|args| !args.trim().is_empty()) {
                    self.dispatch_provider_inline(inline_args.unwrap_or_default())
                } else {
                    self.open_menu(MenuId::from(crate::menu::registry::MENU_PROVIDER));
                    None
                }
            }
            OnboardingAction::SetName(name) => {
                self.state.onboarding.name = name.trim().to_owned();
                self.state.onboarding.local_profile_created = false;
                self.state.onboarding.last_message = Some("Name updated".into());
                self.state.status = "Onboarding name updated".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetUsername(username) => {
                self.state.onboarding.username = username.trim().to_owned();
                self.state.onboarding.local_profile_created = false;
                self.state.onboarding.profile_id = None;
                self.state.onboarding.last_message = Some("Username updated".into());
                self.state.status = "Onboarding username updated".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetEmail(email) => {
                self.state.onboarding.email = email.trim().to_owned();
                self.state.onboarding.local_profile_created = false;
                self.state.onboarding.auth_code_sent = false;
                self.state.onboarding.auth_verified = false;
                self.state.onboarding.last_message = Some("Email updated".into());
                self.state.status = "Onboarding email updated".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetOtpCode(code) => {
                self.state.onboarding.otp_code = code.trim().to_owned();
                self.state.onboarding.last_message = Some("OTP code updated".into());
                self.state.status = "Onboarding OTP code updated".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetProfileId(profile_id) => {
                self.state.onboarding.profile_id = non_empty_string(profile_id);
                self.state.onboarding.last_message = Some("Profile updated".into());
                self.state.status = "Onboarding profile updated".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetProviderSelection(selection) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.apply_selection(selection);
                self.state.status = "Provider route selected; enter API key".into();
                if self.active_menu_id_is(crate::menu::registry::MENU_ONBOARD_ROUTE) {
                    self.close_all_menus();
                    self.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
                    self.focus_provider_api_key_row();
                } else {
                    self.refresh_active_menu_if_open();
                    self.focus_provider_api_key_row();
                }
                None
            }
            OnboardingAction::SetFamilyId(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                let from_family_menu =
                    self.active_menu_id_is(crate::menu::registry::MENU_ONBOARD_FAMILY);
                self.state.onboarding.provider.family_id = value.trim().to_owned();
                self.state.onboarding.provider.model_id.clear();
                self.state.onboarding.provider.route = LlmRouteConfig {
                    api_type: Some("openai".into()),
                    ..LlmRouteConfig::default()
                };
                self.mark_onboarding_provider_dirty("Provider family updated");
                if from_family_menu {
                    self.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL));
                }
                None
            }
            OnboardingAction::SetModelId(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                let from_model_menu =
                    self.active_menu_id_is(crate::menu::registry::MENU_ONBOARD_MODEL);
                self.state.onboarding.provider.model_id = value.trim().to_owned();
                self.state.onboarding.provider.route = LlmRouteConfig {
                    api_type: Some("openai".into()),
                    ..LlmRouteConfig::default()
                };
                self.mark_onboarding_provider_dirty("Provider model updated");
                if from_model_menu {
                    self.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE));
                }
                None
            }
            OnboardingAction::SetRouteId(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.route_id = value.trim().to_owned();
                self.mark_onboarding_provider_dirty("Provider route updated")
            }
            OnboardingAction::SetRouteLabel(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.label = non_empty_string(value);
                self.mark_onboarding_provider_dirty("Provider route label updated")
            }
            OnboardingAction::SetBaseUrl(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.base_url = non_empty_string(value);
                self.mark_onboarding_provider_dirty("Provider base URL updated")
            }
            OnboardingAction::SetApiKeyEnv(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.api_key_env = non_empty_string(value);
                self.mark_onboarding_provider_dirty("Provider API key env updated")
            }
            OnboardingAction::SetApiType(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.api_type = non_empty_string(value);
                self.mark_onboarding_provider_dirty("Provider API type updated")
            }
            OnboardingAction::SetApiKey(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.api_key = Some(value);
                self.state.onboarding.provider_tested = false;
                self.state.onboarding.provider_pending = None;
                self.state.onboarding.provider_save_target = None;
                self.state.onboarding.last_message = Some("API key updated".into());
                self.state.status = "Onboarding API key updated".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::ClearApiKey => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.api_key = None;
                self.state.onboarding.provider_tested = false;
                self.state.onboarding.provider_pending = None;
                self.state.onboarding.provider_save_target = None;
                self.state.onboarding.last_message = Some("API key cleared".into());
                self.state.status = "Onboarding API key cleared".into();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SendCode => self.onboarding_send_code_command(),
            OnboardingAction::VerifyCode => self.onboarding_verify_code_command(),
            OnboardingAction::CreateLocalProfile => {
                self.onboarding_create_local_profile_command(false)
            }
            OnboardingAction::RefreshCatalog => self.onboarding_refresh_catalog_command(),
            OnboardingAction::RefreshProviders => self.onboarding_refresh_providers_command(),
            OnboardingAction::FetchModels => self.onboarding_fetch_models_command(),
            OnboardingAction::SaveProvider => self.onboarding_save_provider_command(),
            OnboardingAction::SaveProviderFallback => {
                self.onboarding_save_provider_fallback_command()
            }
            OnboardingAction::TestProvider => self.onboarding_test_provider_command(),
            OnboardingAction::Finish => self.onboarding_finish_command(),
            OnboardingAction::Reset => {
                self.state.onboarding = Default::default();
                self.state.status = "Onboarding wizard reset".into();
                self.refresh_active_menu_if_open();
                None
            }
        }
    }

    fn dispatch_onboarding_inline(&mut self, args: &str) -> Option<AppUiCommand> {
        let (verb, rest) = split_first_word(args);
        let rest = rest.trim();
        match verb {
            "" | "open" | "status" => self.dispatch_onboarding_action(OnboardingAction::Open, None),
            "name" | "display-name" => {
                self.dispatch_onboarding_action(OnboardingAction::SetName(rest.to_owned()), None)
            }
            "username" | "user" => self
                .dispatch_onboarding_action(OnboardingAction::SetUsername(rest.to_owned()), None),
            "email" => {
                self.dispatch_onboarding_action(OnboardingAction::SetEmail(rest.to_owned()), None)
            }
            "code" | "otp" => {
                self.dispatch_onboarding_action(OnboardingAction::SetOtpCode(rest.to_owned()), None)
            }
            "profile" | "profile-id" => self
                .dispatch_onboarding_action(OnboardingAction::SetProfileId(rest.to_owned()), None),
            "family" | "family-id" => self
                .dispatch_onboarding_action(OnboardingAction::SetFamilyId(rest.to_owned()), None),
            "model" | "model-id" => {
                self.dispatch_onboarding_action(OnboardingAction::SetModelId(rest.to_owned()), None)
            }
            "route" | "route-id" => {
                self.dispatch_onboarding_action(OnboardingAction::SetRouteId(rest.to_owned()), None)
            }
            "label" | "route-label" => self
                .dispatch_onboarding_action(OnboardingAction::SetRouteLabel(rest.to_owned()), None),
            "base-url" | "url" => {
                self.dispatch_onboarding_action(OnboardingAction::SetBaseUrl(rest.to_owned()), None)
            }
            "api-key-env" | "env" => self
                .dispatch_onboarding_action(OnboardingAction::SetApiKeyEnv(rest.to_owned()), None),
            "api-type" => {
                self.dispatch_onboarding_action(OnboardingAction::SetApiType(rest.to_owned()), None)
            }
            "key" | "api-key" => self.dispatch_onboarding_action(
                OnboardingAction::SetApiKey(SecretString::new(rest)),
                None,
            ),
            "clear-key" => self.dispatch_onboarding_action(OnboardingAction::ClearApiKey, None),
            "select" => self.onboarding_select_inline(rest),
            "send-code" | "send" => {
                self.dispatch_onboarding_action(OnboardingAction::SendCode, None)
            }
            "verify" => self.dispatch_onboarding_action(OnboardingAction::VerifyCode, None),
            "create-profile" | "create-local" | "local-profile" => {
                self.dispatch_onboarding_action(OnboardingAction::CreateLocalProfile, None)
            }
            "catalog" | "refresh-catalog" => {
                self.dispatch_onboarding_action(OnboardingAction::RefreshCatalog, None)
            }
            "providers" | "list" => {
                self.dispatch_onboarding_action(OnboardingAction::RefreshProviders, None)
            }
            "fetch-models" => self.dispatch_onboarding_action(OnboardingAction::FetchModels, None),
            "save" => self.dispatch_onboarding_action(OnboardingAction::SaveProvider, None),
            "test" => self.dispatch_onboarding_action(OnboardingAction::TestProvider, None),
            "finish" | "open-session" => {
                self.dispatch_onboarding_action(OnboardingAction::Finish, None)
            }
            "reset" => self.dispatch_onboarding_action(OnboardingAction::Reset, None),
            _ => {
                self.state.status = onboarding_usage();
                self.push_local_activity(
                    ActivityKind::Warning,
                    "onboarding",
                    "Unknown onboarding command",
                    Some(onboarding_usage()),
                );
                self.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
                None
            }
        }
    }

    fn dispatch_login_inline(&mut self, args: &str) -> Option<AppUiCommand> {
        let (verb, rest) = split_first_word(args);
        let rest = rest.trim();
        match verb {
            "" | "open" => {
                self.open_menu(MenuId::from(crate::menu::registry::MENU_LOGIN));
                None
            }
            "status" => Some(AppUiCommand::AuthStatus(Default::default())),
            "email" => {
                self.dispatch_onboarding_action(OnboardingAction::SetEmail(rest.to_owned()), None)
            }
            "code" | "otp" => {
                self.dispatch_onboarding_action(OnboardingAction::SetOtpCode(rest.to_owned()), None)
            }
            "send" | "send-code" => {
                if !rest.is_empty() {
                    self.dispatch_onboarding_action(
                        OnboardingAction::SetEmail(rest.to_owned()),
                        None,
                    );
                }
                self.dispatch_onboarding_action(OnboardingAction::SendCode, None)
            }
            "verify" => {
                if !rest.is_empty() {
                    self.dispatch_onboarding_action(
                        OnboardingAction::SetOtpCode(rest.to_owned()),
                        None,
                    );
                }
                self.dispatch_onboarding_action(OnboardingAction::VerifyCode, None)
            }
            "me" | "account" => Some(AppUiCommand::AuthMe(crate::model::AuthMeParams {
                token: self.state.onboarding.auth_token.clone(),
            })),
            "logout" => Some(AppUiCommand::AuthLogout(crate::model::AuthLogoutParams {
                token: self.state.onboarding.auth_token.clone(),
            })),
            _ => {
                self.state.status = login_usage();
                self.push_local_activity(
                    ActivityKind::Warning,
                    "login",
                    "Unknown login command",
                    Some(login_usage()),
                );
                self.open_menu(MenuId::from(crate::menu::registry::MENU_LOGIN));
                None
            }
        }
    }

    fn dispatch_provider_inline(&mut self, args: &str) -> Option<AppUiCommand> {
        let (verb, rest) = split_first_word(args);
        let rest = rest.trim();
        match verb {
            "" | "open" => {
                self.open_menu(MenuId::from(crate::menu::registry::MENU_PROVIDER));
                None
            }
            "catalog" | "refresh-catalog" => {
                self.dispatch_onboarding_action(OnboardingAction::RefreshCatalog, None)
            }
            "providers" | "list" => {
                self.dispatch_onboarding_action(OnboardingAction::RefreshProviders, None)
            }
            "select" => self.onboarding_select_inline(rest),
            "family" | "family-id" => self
                .dispatch_onboarding_action(OnboardingAction::SetFamilyId(rest.to_owned()), None),
            "model" | "model-id" => {
                self.dispatch_onboarding_action(OnboardingAction::SetModelId(rest.to_owned()), None)
            }
            "route" | "route-id" => {
                self.dispatch_onboarding_action(OnboardingAction::SetRouteId(rest.to_owned()), None)
            }
            "label" | "route-label" => self
                .dispatch_onboarding_action(OnboardingAction::SetRouteLabel(rest.to_owned()), None),
            "base-url" | "url" => {
                self.dispatch_onboarding_action(OnboardingAction::SetBaseUrl(rest.to_owned()), None)
            }
            "api-key-env" | "env" => self
                .dispatch_onboarding_action(OnboardingAction::SetApiKeyEnv(rest.to_owned()), None),
            "api-type" => {
                self.dispatch_onboarding_action(OnboardingAction::SetApiType(rest.to_owned()), None)
            }
            "key" | "api-key" => self.dispatch_onboarding_action(
                OnboardingAction::SetApiKey(SecretString::new(rest)),
                None,
            ),
            "clear-key" => self.dispatch_onboarding_action(OnboardingAction::ClearApiKey, None),
            "fetch-models" => self.dispatch_onboarding_action(OnboardingAction::FetchModels, None),
            "test" => self.dispatch_onboarding_action(OnboardingAction::TestProvider, None),
            "save" => self.dispatch_onboarding_action(OnboardingAction::SaveProvider, None),
            "fallback" | "save-fallback" | "add-fallback" => {
                self.dispatch_onboarding_action(OnboardingAction::SaveProviderFallback, None)
            }
            _ => {
                self.state.status = provider_usage();
                self.push_local_activity(
                    ActivityKind::Warning,
                    "provider",
                    "Unknown provider command",
                    Some(provider_usage()),
                );
                self.open_menu(MenuId::from(crate::menu::registry::MENU_PROVIDER));
                None
            }
        }
    }

    fn onboarding_select_inline(&mut self, args: &str) -> Option<AppUiCommand> {
        let parts = args.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 3 {
            self.state.status =
                "Usage: /onboard select <family_id> <model_id> <route_id> [base_url] [api_key_env]"
                    .into();
            return None;
        }
        let selection = LlmSelectionConfig {
            family_id: parts[0].to_owned(),
            model_id: parts[1].to_owned(),
            route: LlmRouteConfig {
                route_id: parts[2].to_owned(),
                label: None,
                base_url: parts.get(3).map(|value| (*value).to_owned()),
                api_key_env: parts.get(4).map(|value| (*value).to_owned()),
                api_type: Some("openai".into()),
            },
            ..LlmSelectionConfig::default()
        };
        self.dispatch_onboarding_action(OnboardingAction::SetProviderSelection(selection), None)
    }

    fn mark_onboarding_provider_dirty(&mut self, message: &'static str) -> Option<AppUiCommand> {
        self.state.onboarding.provider_tested = false;
        self.state.onboarding.provider_pending = None;
        self.state.onboarding.provider_save_target = None;
        self.state.onboarding.last_message = Some(message.into());
        self.state.status = message.into();
        self.refresh_active_menu_if_open();
        None
    }

    fn block_onboarding_provider_edit_if_pending(&mut self) -> bool {
        let Some(pending) = self.state.onboarding.provider_pending else {
            return false;
        };
        self.state.status = onboarding_pending_status(pending);
        self.refresh_active_menu_if_open();
        true
    }

    fn onboarding_send_code_command(&mut self) -> Option<AppUiCommand> {
        if self.local_profile_create_supported() {
            self.state.status =
                "Local solo onboarding uses profile/local/create; OTP send is hidden".into();
            return None;
        }
        if !self.require_appui_method(crate::model::APPUI_METHOD_AUTH_SEND_CODE) {
            return None;
        }
        if !self.state.onboarding.has_email() {
            self.state.status = "Onboarding email is empty; use /onboard email <address>".into();
            return None;
        }
        self.state.onboarding.last_message = Some("Sending OTP code".into());
        Some(AppUiCommand::AuthSendCode(AuthSendCodeParams {
            email: self.state.onboarding.email.clone(),
        }))
    }

    fn onboarding_verify_code_command(&mut self) -> Option<AppUiCommand> {
        if self.local_profile_create_supported() {
            self.state.status =
                "Local solo onboarding uses profile/local/create; OTP verify is hidden".into();
            return None;
        }
        if !self.require_appui_method(crate::model::APPUI_METHOD_AUTH_VERIFY) {
            return None;
        }
        if !self.state.onboarding.has_email() || !self.state.onboarding.has_otp_code() {
            self.state.status =
                "Onboarding email or OTP is empty; use /onboard email and /onboard code".into();
            return None;
        }
        self.state.onboarding.last_message = Some("Verifying OTP code".into());
        Some(AppUiCommand::AuthVerify(AuthVerifyParams {
            email: self.state.onboarding.email.clone(),
            code: self.state.onboarding.otp_code.clone(),
        }))
    }

    fn onboarding_create_local_profile_command(
        &mut self,
        open_session_after_create: bool,
    ) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE) {
            return None;
        }
        if !self.state.onboarding.local_profile_ready() {
            self.state.status =
                "Local profile is incomplete; use /onboard name, /onboard username, and /onboard email"
                    .into();
            return None;
        }
        self.state.onboarding.open_session_after_profile_create = open_session_after_create;
        self.state.onboarding.last_message = Some("Creating local solo profile".into());
        Some(AppUiCommand::ProfileLocalCreate(ProfileLocalCreateParams {
            name: self.state.onboarding.name.clone(),
            username: self.state.onboarding.username.clone(),
            email: self.state.onboarding.email.clone(),
        }))
    }

    fn onboarding_refresh_catalog_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG) {
            return None;
        }
        Some(AppUiCommand::ProfileLlmCatalog(
            ProfileLlmCatalogParams::default(),
        ))
    }

    fn onboarding_refresh_providers_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_MODEL_LIST) {
            return None;
        }
        Some(AppUiCommand::ProfileLlmList(ProfileLlmListParams {
            profile_id: self.current_profile_for_onboarding(),
        }))
    }

    fn onboarding_fetch_models_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_LLM_FETCH_MODELS) {
            return None;
        }
        if let Some(pending) = self.state.onboarding.provider_pending {
            self.state.status = onboarding_pending_status(pending);
            self.refresh_active_menu_if_open();
            return None;
        }
        let Some(params) = self
            .state
            .onboarding
            .build_fetch_models_params(self.current_profile_for_onboarding().as_deref())
        else {
            self.state.status = "Onboarding provider route is incomplete".into();
            return None;
        };
        Some(AppUiCommand::ProfileLlmFetchModels(params))
    }

    fn onboarding_save_provider_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT) {
            return None;
        }
        if let Some(pending) = self.state.onboarding.provider_pending {
            self.state.status = onboarding_pending_status(pending);
            self.refresh_active_menu_if_open();
            return None;
        }
        let current_profile = self.current_profile_for_onboarding();
        let Some(params) = self
            .state
            .onboarding
            .build_upsert_params(current_profile.as_deref())
        else {
            self.state.status = "Onboarding provider selection is incomplete".into();
            return None;
        };
        if !self.state.onboarding.has_api_key() {
            self.state.status = "Onboarding API key is empty; use /onboard key <secret>".into();
            return None;
        }
        self.state.onboarding.last_message = Some("Saving provider".into());
        self.state.onboarding.provider_pending = Some(OnboardingProviderPending::Save);
        self.state.onboarding.provider_save_target = Some(OnboardingProviderSaveTarget::Primary);
        self.state.status = "Saving provider configuration".into();
        self.refresh_active_menu_if_open();
        Some(AppUiCommand::ProfileLlmUpsert(params))
    }

    fn onboarding_save_provider_fallback_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT) {
            return None;
        }
        if let Some(pending) = self.state.onboarding.provider_pending {
            self.state.status = onboarding_pending_status(pending);
            self.refresh_active_menu_if_open();
            return None;
        }
        let current_profile = self.current_profile_for_onboarding();
        let Some(params) = self
            .state
            .onboarding
            .build_fallback_upsert_params(current_profile.as_deref())
        else {
            self.state.status = "Onboarding fallback provider selection is incomplete".into();
            return None;
        };
        if !self.state.onboarding.has_api_key() {
            self.state.status = "Onboarding API key is empty; use /provider key <secret>".into();
            return None;
        }
        self.state.onboarding.last_message = Some("Saving fallback provider".into());
        self.state.onboarding.provider_pending = Some(OnboardingProviderPending::Save);
        self.state.onboarding.provider_save_target = Some(OnboardingProviderSaveTarget::Fallback);
        self.state.status = "Saving fallback provider configuration".into();
        self.refresh_active_menu_if_open();
        Some(AppUiCommand::ProfileLlmUpsert(params))
    }

    fn onboarding_test_provider_command(&mut self) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_LLM_TEST) {
            return None;
        }
        if let Some(pending) = self.state.onboarding.provider_pending {
            self.state.status = onboarding_pending_status(pending);
            self.refresh_active_menu_if_open();
            return None;
        }
        let current_profile = self.current_profile_for_onboarding();
        let Some(params) = self
            .state
            .onboarding
            .build_test_params(current_profile.as_deref())
        else {
            self.state.status = "Onboarding provider selection is incomplete".into();
            return None;
        };
        if !self.state.onboarding.has_api_key() {
            self.state.status = "Onboarding API key is empty; use /onboard key <secret>".into();
            return None;
        }
        self.state.onboarding.last_message = Some("Testing provider".into());
        self.state.onboarding.provider_pending = Some(OnboardingProviderPending::Test);
        self.state.status = "Testing provider connection".into();
        self.refresh_active_menu_if_open();
        Some(AppUiCommand::ProfileLlmTest(params))
    }

    fn onboarding_finish_command(&mut self) -> Option<AppUiCommand> {
        if let Some(pending) = self.state.onboarding.provider_pending {
            self.state.status = onboarding_pending_status(pending);
            self.refresh_active_menu_if_open();
            return None;
        }
        if self.local_profile_create_supported()
            && self.state.onboarding.profile_id.is_none()
            && !self.state.onboarding.local_profile_created
            && self.state.onboarding.local_profile_ready()
        {
            return self.onboarding_create_local_profile_command(true);
        }

        let Some(profile_id) = self.current_profile_for_onboarding() else {
            if self.local_profile_create_supported() {
                return self.onboarding_create_local_profile_command(true);
            }
            self.state.status =
                "Cannot open session: profile unresolved. Use /onboard profile <profile_id>."
                    .into();
            return None;
        };
        if let Some(reason) = self.open_session_provider_block_reason(&profile_id) {
            self.state.status = reason;
            self.refresh_active_menu_if_open();
            return None;
        }
        let session_id =
            octos_core::SessionKey::with_profile_topic(&profile_id, "local", "tui", "coding");
        self.state.status = format!("Opening coding session for profile {profile_id}");
        Some(AppUiCommand::OpenSession(SessionOpenParams {
            session_id,
            profile_id: Some(profile_id),
            cwd: onboarding_workspace_cwd(&self.state.workspace.root),
            after: None,
        }))
    }

    fn current_profile_for_onboarding(&self) -> Option<String> {
        let runtime_profile = self.active_session().and_then(|session| {
            self.state
                .runtime_status_for(&session.id)
                .and_then(|status| status.profile_id.as_deref())
                .or(session.profile_id.as_deref())
        });
        self.state
            .onboarding
            .effective_profile_id(runtime_profile)
            .or_else(|| {
                self.state
                    .profile_llm_state
                    .as_ref()
                    .and_then(|state| non_empty_string(state.profile_id.as_deref()?.to_owned()))
            })
            .or_else(|| {
                self.state
                    .profile_skills
                    .as_ref()
                    .and_then(|state| non_empty_string(state.profile_id.as_deref()?.to_owned()))
            })
            .or_else(|| {
                self.state
                    .profile_skill_registry
                    .as_ref()
                    .and_then(|state| non_empty_string(state.profile_id.as_deref()?.to_owned()))
            })
    }

    fn open_session_provider_block_reason(&self, profile_id: &str) -> Option<String> {
        if let Some(pending) = self.state.onboarding.provider_pending {
            return Some(onboarding_pending_status(pending));
        }
        if self.profile_has_saved_primary_provider(profile_id) {
            return None;
        }
        Some("Cannot open session: save a primary LLM provider first.".into())
    }

    fn profile_has_saved_primary_provider(&self, profile_id: &str) -> bool {
        self.state.onboarding.provider_saved
            || self
                .state
                .profile_llm_state
                .as_ref()
                .filter(|state| {
                    state
                        .profile_id
                        .as_deref()
                        .is_none_or(|state_profile| state_profile == profile_id)
                })
                .and_then(|state| state.primary_provider())
                .is_some_and(|provider| provider.has_api_key)
    }

    fn local_profile_create_supported(&self) -> bool {
        self.state
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| {
                capabilities.supports_method(crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE)
            })
    }

    fn profile_llm_catalog_supported(&self) -> bool {
        self.state
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| {
                capabilities.supports_method(crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG)
            })
    }

    fn active_menu_id_is(&self, id: &str) -> bool {
        self.state
            .menu_stack
            .active()
            .is_some_and(|frame| frame.id.as_str() == id)
    }

    fn require_appui_method(&mut self, method: &'static str) -> bool {
        if self
            .state
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.supports_method(method))
        {
            return true;
        }
        self.state.status = format!("AppUI method `{method}` is not advertised");
        false
    }

    fn require_mutating_appui_method(&mut self, method: &'static str) -> bool {
        if self.state.readonly {
            self.state.status = format!("Read-only mode: {method} disabled");
            return false;
        }
        self.require_appui_method(method)
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

    fn advance_active_menu_selection(&mut self) -> bool {
        let len = active_menu_item_len(self.state.active_menu.as_ref());
        let Some(frame) = self.state.menu_stack.active_mut() else {
            return false;
        };
        if len == 0 {
            return false;
        }
        let next_index = (frame.selected_index + 1).min(len.saturating_sub(1));
        if next_index == frame.selected_index {
            return false;
        }
        frame.selected_index = next_index;
        self.refresh_active_menu();
        true
    }

    fn select_active_menu_item_by_id(&mut self, item_id: &str) -> bool {
        let Some(index) = self.state.active_menu.as_ref().and_then(|menu| match menu {
            MenuBuildResult::Ready(spec) => spec.items.iter().position(|item| item.id == item_id),
            MenuBuildResult::Loading(_)
            | MenuBuildResult::Unavailable(_)
            | MenuBuildResult::Error(_) => None,
        }) else {
            return false;
        };
        let Some(frame) = self.state.menu_stack.active_mut() else {
            return false;
        };
        frame.selected_index = index;
        self.refresh_active_menu();
        true
    }

    fn focus_provider_api_key_row(&mut self) -> bool {
        self.select_active_menu_item_by_id("onboard.provider.key")
            || self.select_active_menu_item_by_id("provider.key")
    }

    fn focus_provider_start_row(&mut self) -> bool {
        self.select_active_menu_item_by_id("onboard.provider.family")
            || self.select_active_menu_item_by_id("provider.current")
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
            MenuAction::Local(action) => self.dispatch_local_action(action, None),
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
        let result = filter_menu_result_for_search(
            core_menu_registry().build(&frame.id, &ctx),
            &frame.search_query,
        );
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

    fn refresh_active_menu_and_advance(&mut self) {
        if self.state.menu_stack.is_active() {
            self.refresh_active_menu();
            self.advance_active_menu_selection();
        }
    }

    fn menu_app_snapshot(&self) -> MenuAppSnapshot<'_> {
        let selected_session = self.state.active_session();
        let selected_task = self.state.active_task();
        let runtime_status =
            selected_session.and_then(|session| self.state.runtime_status_for(&session.id));
        let model_catalog =
            selected_session.and_then(|session| self.state.model_catalog_for(&session.id));
        let mcp_catalog =
            selected_session.and_then(|session| self.state.mcp_catalog_for(&session.id));
        let tool_catalog =
            selected_session.and_then(|session| self.state.tool_catalog_for(&session.id));
        let current_model = runtime_status.and_then(|status| {
            status
                .model
                .as_ref()
                .map(|model| model.model.as_str())
                .or_else(|| {
                    status
                        .runtime_policy_stamp
                        .as_ref()
                        .and_then(|stamp| stamp.model.as_deref())
                })
        });
        let current_profile = runtime_status
            .and_then(|status| {
                status.profile_id.as_deref().or_else(|| {
                    status
                        .runtime_policy_stamp
                        .as_ref()
                        .and_then(|stamp| stamp.profile_id.as_deref())
                })
            })
            .or_else(|| selected_session.and_then(|session| session.profile_id.as_deref()))
            .or(self.state.onboarding.profile_id.as_deref())
            .or_else(|| {
                self.state
                    .profile_llm_state
                    .as_ref()
                    .and_then(|state| state.profile_id.as_deref())
            })
            .or_else(|| {
                self.state
                    .profile_skills
                    .as_ref()
                    .and_then(|state| state.profile_id.as_deref())
            });
        let cwd = runtime_status
            .and_then(|status| status.workspace_root.as_deref().or(status.cwd.as_deref()))
            .or(Some(self.state.workspace.root.as_str()));
        MenuAppSnapshot {
            status: Some(self.state.status.as_str()),
            target: self.state.target.as_deref(),
            cwd,
            current_model,
            current_profile,
            permission_profile: selected_session
                .and_then(|session| self.state.permission_profile_for(&session.id)),
            runtime_status,
            model_catalog,
            profile_llm_catalog: self.state.profile_llm_catalog.as_ref(),
            profile_llm_state: self.state.profile_llm_state.as_ref(),
            profile_skills: self.state.profile_skills.as_ref(),
            profile_skill_registry: self.state.profile_skill_registry.as_ref(),
            mcp_catalog,
            tool_catalog,
            mcp_config_catalog: self.state.mcp_config_catalog.as_ref(),
            tool_config_catalog: self.state.tool_config_catalog.as_ref(),
            onboarding: Some(&self.state.onboarding),
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
            media: Vec::new(),
            topic: None,
            rewrite_for: None,
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
                self.state.composer_cursor = None;
                self.state.insert_composer_text("\n\n");
            }
            self.state.insert_composer_text(&prompt);
            self.state.status = format!("Added selected diff hunk context to composer: {path}");
        }

        self.state.focus = FocusPane::Composer;
        self.state.scroll_transcript_to_latest();
    }

    pub fn apply_client_event(&mut self, event: ClientEvent) -> Option<AppUiCommand> {
        match event {
            ClientEvent::App(event) => self.apply_event(*event),
            ClientEvent::Capabilities(event) => {
                self.apply_capabilities_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::DiffPreview(result) => {
                self.apply_diff_preview_result(result);
                None
            }
            ClientEvent::ModelList(event) => {
                self.apply_model_list_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ModelSelect(event) => {
                self.apply_model_select_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::McpStatus(event) => {
                self.apply_mcp_status_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::McpConfigList(event) => {
                self.apply_mcp_config_list_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::McpConfigMutation(event) => {
                self.apply_mcp_config_mutation_event(event);
                self.refresh_active_menu_if_open();
                self.mcp_config_list_command()
            }
            ClientEvent::PermissionProfile(event) => {
                self.apply_permission_profile_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::AuthStatus(event) => {
                self.state.onboarding.apply_auth_status(&event.result);
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "auth",
                    event.message.clone(),
                ));
                self.state.onboarding.last_message = Some(event.message.clone());
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::AuthSendCode(event) => {
                self.state.onboarding.auth_code_sent = event.result.ok;
                self.state.onboarding.last_message = Some(event.message.clone());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "auth",
                    event.message.clone(),
                ));
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::AuthVerify(event) => {
                self.state.onboarding.apply_auth_verify(&event.result);
                let follow_up = event.result.ok.then(|| {
                    AppUiCommand::AuthMe(crate::model::AuthMeParams {
                        token: self.state.onboarding.auth_token.clone(),
                    })
                });
                self.state.onboarding.last_message = Some(event.message.clone());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "auth",
                    event.message.clone(),
                ));
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                follow_up
            }
            ClientEvent::AuthMe(event) => {
                self.state.onboarding.apply_auth_me(&event.result);
                self.state.onboarding.last_message = Some(event.message.clone());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "auth",
                    event.message.clone(),
                ));
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::AuthLogout(event) => {
                if event.result.ok {
                    self.state.onboarding.auth_verified = false;
                    self.state.onboarding.auth_token = None;
                }
                self.state.onboarding.last_message = Some(event.message.clone());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "auth",
                    event.message.clone(),
                ));
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ProfileLocalCreate(event) => {
                self.state
                    .onboarding
                    .apply_profile_local_create(&event.result);
                let open_session = self.state.onboarding.open_session_after_profile_create;
                self.state.onboarding.open_session_after_profile_create = false;
                self.state.onboarding.last_message = Some(event.message.clone());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "local profile",
                    event.message.clone(),
                ));
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                let follow_up = open_session
                    .then(|| self.onboarding_finish_command())
                    .flatten();
                follow_up.or_else(|| {
                    self.profile_llm_catalog_supported().then(|| {
                        AppUiCommand::ProfileLlmCatalog(ProfileLlmCatalogParams::default())
                    })
                })
            }
            ClientEvent::ProfileLlmCatalog(event) => {
                self.apply_profile_llm_catalog_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ProfileLlmList(event) => {
                self.apply_profile_llm_list_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ProfileLlmMutation(event) => {
                self.apply_profile_llm_mutation_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ProfileSkillsList(event) => {
                self.apply_profile_skills_list_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ProfileSkillsRegistrySearch(event) => {
                self.apply_profile_skills_registry_search_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ProfileSkillsMutation(event) => {
                self.apply_profile_skills_mutation_event(event);
                self.refresh_active_menu_if_open();
                self.profile_skills_list_command()
            }
            ClientEvent::SessionStatus(event) => {
                self.apply_session_status_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ToolStatus(event) => {
                self.apply_tool_status_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ToolConfigList(event) => {
                self.apply_tool_config_list_event(event);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ToolConfigMutation(event) => {
                self.apply_tool_config_mutation_event(event);
                self.refresh_active_menu_if_open();
                self.tool_config_list_command()
            }
        }
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
                let onboarding = self.state.onboarding.clone();
                let permission_profiles = self.state.permission_profiles.clone();
                let session_runtime_statuses = self.state.session_runtime_statuses.clone();
                let profile_llm_catalog = self.state.profile_llm_catalog.clone();
                let profile_llm_state = self.state.profile_llm_state.clone();
                let profile_skills = self.state.profile_skills.clone();
                let profile_skill_registry = self.state.profile_skill_registry.clone();
                let session_model_catalogs = self.state.session_model_catalogs.clone();
                let session_mcp_catalogs = self.state.session_mcp_catalogs.clone();
                let session_tool_catalogs = self.state.session_tool_catalogs.clone();
                let mcp_config_catalog = self.state.mcp_config_catalog.clone();
                let tool_config_catalog = self.state.tool_config_catalog.clone();

                let mut state = AppState::from_snapshot(snapshot);
                if state.capabilities.is_none() {
                    state.capabilities = previous_capabilities;
                }
                state.set_composer_text(composer);
                state.composer_drafts = composer_drafts;
                state.pending_messages = pending_messages;
                state.optimistic_user_messages = optimistic_user_messages;
                state.approval_auto_open = approval_auto_open;
                state.expanded_tool_outputs = expanded_tool_outputs;
                state.menu_stack = menu_stack;
                state.onboarding = onboarding;
                state.permission_profiles = permission_profiles;
                state.session_runtime_statuses = session_runtime_statuses;
                state.profile_llm_catalog = profile_llm_catalog;
                state.profile_llm_state = profile_llm_state;
                state.profile_skills = profile_skills;
                state.profile_skill_registry = profile_skill_registry;
                state.session_model_catalogs = session_model_catalogs;
                state.session_mcp_catalogs = session_mcp_catalogs;
                state.session_tool_catalogs = session_tool_catalogs;
                state.mcp_config_catalog = mcp_config_catalog;
                state.tool_config_catalog = tool_config_catalog;
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

    fn apply_capabilities_event(&mut self, event: CapabilitiesClientEvent) {
        self.state.set_capabilities(event.result.capabilities);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "capabilities",
            event.message.clone(),
        ));
        self.state.status = event.message;
        self.maybe_open_onboarding_on_first_launch();
    }

    fn maybe_open_onboarding_on_first_launch(&mut self) {
        if !self.state.sessions.is_empty() || self.state.menu_stack.is_active() {
            return;
        }

        // M22-A: only auto-open onboarding when the backend advertises a
        // *profile-creation* surface (local solo no-OTP, or legacy email
        // OTP). Provider/model-only catalogs do NOT trigger onboarding
        // on first launch because there is nothing to onboard into.
        let Some(capabilities) = self.state.capabilities.as_ref() else {
            return;
        };
        let supports_local_solo = crate::menu::registry::APPUI_FIRST_LAUNCH_LOCAL_SOLO_METHODS
            .iter()
            .any(|method| capabilities.supports_method(method));
        let supports_legacy_auth = crate::menu::registry::APPUI_FIRST_LAUNCH_LEGACY_AUTH_METHODS
            .iter()
            .all(|method| capabilities.supports_method(method));
        if supports_local_solo || supports_legacy_auth {
            self.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
        }
    }

    fn apply_model_list_event(&mut self, event: ModelListClientEvent) {
        let result = event.result;
        self.state.set_model_catalog(SessionModelCatalog {
            session_id: result.session_id,
            models: result.models,
        });
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "models",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_model_select_event(&mut self, event: ModelSelectClientEvent) {
        let result = event.result;
        if let Some(status) = self
            .state
            .session_runtime_statuses
            .iter_mut()
            .find(|status| status.session_id == result.session_id)
        {
            status.model = Some(result.selected.clone());
            if let Some(stamp) = result.runtime_policy_stamp.clone() {
                status.runtime_policy_stamp = Some(stamp);
            }
        }
        if let Some(catalog) = self
            .state
            .session_model_catalogs
            .iter_mut()
            .find(|catalog| catalog.session_id == result.session_id)
        {
            for model in &mut catalog.models {
                model.selected = model.model == result.selected.model
                    && model.provider == result.selected.provider;
            }
            if !catalog.models.iter().any(|model| {
                model.model == result.selected.model && model.provider == result.selected.provider
            }) {
                catalog.models.push(result.selected);
            }
        }
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "model",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_mcp_status_event(&mut self, event: McpStatusClientEvent) {
        let result = event.result;
        self.state.set_mcp_catalog(SessionMcpCatalog {
            session_id: result.session_id,
            servers: result.servers,
        });
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "mcp",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_mcp_config_list_event(&mut self, event: McpConfigListClientEvent) {
        self.state.mcp_config_catalog = Some(event.result);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "mcp config",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_mcp_config_mutation_event(&mut self, event: McpConfigMutationClientEvent) {
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "mcp config",
            event.message.clone(),
        ));
        self.state.status = event.message;
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

    fn apply_profile_llm_catalog_event(&mut self, event: ProfileLlmCatalogClientEvent) {
        self.state.profile_llm_catalog = Some(event.result);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "provider catalog",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_profile_llm_list_event(&mut self, event: ProfileLlmListClientEvent) {
        if self.state.onboarding.profile_id.is_none() {
            if let Some(profile_id) = event
                .result
                .profile_id
                .as_deref()
                .and_then(|profile_id| non_empty_string(profile_id.to_owned()))
            {
                self.state.onboarding.profile_id = Some(profile_id);
            }
        }
        self.state.profile_llm_state = Some(event.result.clone());
        if let Some(session_id) = self.active_session().map(|session| session.id.clone()) {
            self.state.set_model_catalog(SessionModelCatalog {
                session_id,
                models: event.result.models(),
            });
        }
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "providers",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_profile_llm_mutation_event(&mut self, event: ProfileLlmMutationClientEvent) {
        let pending = self.state.onboarding.provider_pending.take();
        let save_target = self.state.onboarding.provider_save_target.take();
        let staged_provider_label = self.state.onboarding.provider_label();
        let mut reset_staged_provider = false;
        if event.result.applied {
            if profile_llm_list_has_provider_state(&event.result.to_list_result()) {
                self.state.profile_llm_state = Some(event.result.to_list_result());
            }
            match pending {
                Some(OnboardingProviderPending::Test) => {
                    self.state.onboarding.provider_tested = true;
                }
                Some(OnboardingProviderPending::Save) => {
                    match save_target.unwrap_or(OnboardingProviderSaveTarget::Primary) {
                        OnboardingProviderSaveTarget::Primary => {
                            self.state.onboarding.provider_saved = true;
                            self.state.onboarding.provider_tested = true;
                            self.state.onboarding.saved_primary_provider_label =
                                Some(staged_provider_label.clone());
                        }
                        OnboardingProviderSaveTarget::Fallback => {
                            self.state.onboarding.provider_tested = false;
                            reset_staged_provider = true;
                        }
                    }
                    self.state.onboarding.last_saved_provider_label =
                        Some(staged_provider_label.clone());
                    self.state.onboarding.last_saved_provider_target =
                        Some(save_target.unwrap_or(OnboardingProviderSaveTarget::Primary));
                }
                None => {
                    self.state.onboarding.provider_saved = true;
                    self.state.onboarding.provider_tested = true;
                    self.state.onboarding.saved_primary_provider_label =
                        Some(staged_provider_label.clone());
                    self.state.onboarding.last_saved_provider_label =
                        Some(staged_provider_label.clone());
                    self.state.onboarding.last_saved_provider_target =
                        Some(OnboardingProviderSaveTarget::Primary);
                }
            }
            if reset_staged_provider {
                self.state.onboarding.reset_staged_provider();
            }
            self.state.onboarding.last_message = Some(event.message.clone());
        } else if pending.is_some() {
            self.state.onboarding.last_message = Some(event.message.clone());
        }
        if let Some(session_id) = self.active_session().map(|session| session.id.clone()) {
            let models = event.result.models();
            self.state
                .set_model_catalog(SessionModelCatalog { session_id, models });
        }
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "providers",
            event.message.clone(),
        ));
        self.state.status = event.message;
        if reset_staged_provider {
            self.refresh_active_menu_if_open();
            self.focus_provider_start_row();
        } else if event.result.applied && pending.is_some() {
            self.refresh_active_menu_and_advance();
        } else {
            self.refresh_active_menu_if_open();
        }
    }

    fn apply_profile_skills_list_event(&mut self, event: ProfileSkillsListClientEvent) {
        if self.state.onboarding.profile_id.is_none() {
            if let Some(profile_id) = event
                .result
                .profile_id
                .as_deref()
                .and_then(|profile_id| non_empty_string(profile_id.to_owned()))
            {
                self.state.onboarding.profile_id = Some(profile_id);
            }
        }
        self.state.profile_skills = Some(event.result);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "skills",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_profile_skills_registry_search_event(
        &mut self,
        event: ProfileSkillsRegistrySearchClientEvent,
    ) {
        if self.state.onboarding.profile_id.is_none() {
            if let Some(profile_id) = event
                .result
                .profile_id
                .as_deref()
                .and_then(|profile_id| non_empty_string(profile_id.to_owned()))
            {
                self.state.onboarding.profile_id = Some(profile_id);
            }
        }
        self.state.profile_skill_registry = Some(event.result);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "skill registry",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_profile_skills_mutation_event(&mut self, event: ProfileSkillsMutationClientEvent) {
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "skills",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_session_status_event(&mut self, event: SessionStatusClientEvent) {
        if let Some(capabilities) = event.result.capabilities.clone() {
            self.state.set_capabilities(capabilities);
        }
        if let Some(profile_id) = event.result.profile_id.as_deref() {
            if let Some(session) = self
                .state
                .sessions
                .iter_mut()
                .find(|session| session.id == event.result.session_id)
            {
                session.profile_id = Some(profile_id.to_owned());
            }
        }
        let message = event.message;
        self.state
            .set_runtime_status(SessionRuntimeStatus::from(event.result));
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "runtime status",
            message.clone(),
        ));
        self.state.status = message;
    }

    fn apply_tool_status_event(&mut self, event: ToolStatusClientEvent) {
        let result = event.result;
        self.state.set_tool_catalog(SessionToolCatalog {
            session_id: result.session_id,
            policy_id: result.policy_id,
            coding_tool_contract: result.coding_tool_contract,
            tools: result.tools,
        });
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "tools",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_tool_config_list_event(&mut self, event: ToolConfigListClientEvent) {
        self.state.tool_config_catalog = Some(event.result);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "tool config",
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_tool_config_mutation_event(&mut self, event: ToolConfigMutationClientEvent) {
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "tool config",
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
            self.state
                .diff_preview
                .open_loading_for_turn(preview_id.clone(), event.turn_id.clone());
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
                let follow_tail = self.state.transcript_scroll == 0;
                let mut reset_scroll = false;
                if let Some(session) = self.find_session_mut(&session_id) {
                    if let Some(live_reply) = session.live_reply.as_mut() {
                        if live_reply.turn_id == turn_id {
                            live_reply.text.push_str(&text);
                            reset_scroll = true;
                        }
                    }
                }
                if reset_scroll && follow_tail {
                    self.state.scroll_transcript_to_latest();
                } else if reset_scroll {
                    self.state.preserve_transcript_position_after_append(
                        text.lines().count().saturating_sub(1),
                    );
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
                let diff_preview_turn_id = approval.turn_id.clone();
                self.state.approval = Some(approval);
                self.state.focus = FocusPane::Composer;
                self.state.set_run_state_blocked(title.clone());
                self.state.status = format!("Approval requested: {title}");
                if let Some(preview_id) = diff_preview_id {
                    let request_already_in_flight = self.state.diff_preview.loading
                        && self.state.diff_preview.requested_preview_id.as_ref()
                            == Some(&preview_id);
                    self.state
                        .diff_preview
                        .open_loading_for_turn(preview_id.clone(), Some(diff_preview_turn_id));
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
            UiNotification::FileAttached(event) => self.apply_file_attached(event),
            UiNotification::SessionEventBridged(event) => self.apply_session_event_bridged(event),
            UiNotification::RouterStatus(event) => {
                self.state.status = format!(
                    "Router {} using {} ({})",
                    event.mode, event.provider_name, event.session_id.0
                );
                None
            }
            UiNotification::RouterFailover(event) => {
                self.state.push_activity(
                    ActivityItem::new(ActivityKind::Warning, "Router failover", event.reason)
                        .with_detail(format!(
                            "{} -> {} in {}ms",
                            event.from_provider, event.to_provider, event.elapsed_ms
                        )),
                );
                self.state.status = format!("Router failover to {}", event.to_provider);
                None
            }
            UiNotification::QueueState(event) => {
                self.state.status = if event.pending_count == 0 {
                    "Queue empty".into()
                } else {
                    format!("Queue pending: {}", event.pending_count)
                };
                None
            }
            UiNotification::AgentUpdated(event) => {
                let title = event
                    .agent
                    .title
                    .clone()
                    .unwrap_or_else(|| event.agent.nickname.clone());
                let detail = event
                    .agent
                    .summary
                    .clone()
                    .or_else(|| event.agent.last_task.clone())
                    .unwrap_or_else(|| event.agent.role.clone());
                self.state.push_activity(
                    ActivityItem::new(ActivityKind::Progress, title.clone(), event.agent.status)
                        .with_detail(detail),
                );
                self.state.status = format!("Agent status refreshed: {title}");
                None
            }
            UiNotification::AgentOutputDelta(event) => {
                let bytes = event.text.len();
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Progress,
                        "agent output",
                        format!("Agent output refreshed: {} ({bytes} bytes)", event.agent_id),
                    )
                    .with_detail(compact_preview(&event.text)),
                );
                None
            }
            UiNotification::AgentArtifactUpdated(event) => {
                let count = event.artifacts.len();
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Tool,
                    "agent artifacts",
                    format!("{} artifact(s) refreshed for {}", count, event.agent_id),
                ));
                None
            }
            UiNotification::SessionGoalUpdated(event) => {
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    "session goal",
                    event.goal.status.clone(),
                ));
                self.state.status = format!("Goal updated: {}", event.goal.objective);
                None
            }
            UiNotification::SessionGoalCleared(event) => {
                self.state.status = if event.cleared {
                    "Goal cleared".into()
                } else {
                    "Goal clear requested".into()
                };
                None
            }
            UiNotification::LoopUpdated(event) => {
                let status = event
                    .status
                    .clone()
                    .unwrap_or_else(|| event.loop_state.status.clone());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    event.loop_state.loop_id,
                    status,
                ));
                None
            }
            UiNotification::LoopFired(event) => {
                let status = event.status.unwrap_or_else(|| {
                    event
                        .fire
                        .as_ref()
                        .map(|fire| if fire.queued { "queued" } else { "fired" })
                        .unwrap_or("fired")
                        .into()
                });
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    event.loop_id,
                    status,
                ));
                None
            }
            UiNotification::LoopCompleted(event) => {
                let status = event.status.unwrap_or_else(|| "completed".into());
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    event.loop_id,
                    status,
                ));
                None
            }
            UiNotification::ContextCompactionCompleted(event) => {
                let session_id = event.session_id.clone();
                let state = crate::model::ContextLifecycleState {
                    session_id: event.context_state.session_id.clone(),
                    thread_id: event.context_state.thread_id.clone(),
                    generation: event.context_state.generation,
                    transcript_hash: event.context_state.transcript_hash.clone(),
                    item_count: event.context_state.item_count,
                    token_estimate: event.context_state.token_estimate,
                    recovery_state: event.context_state.recovery_state.clone(),
                    last_checkpoint_id: event.context_state.last_checkpoint_id.clone(),
                    last_compaction_id: event.context_state.last_compaction_id.clone(),
                };
                let compaction = crate::model::ContextCompactionSummary {
                    compaction_id: event.compaction.compaction_id.clone(),
                    status: event.compaction.status.clone(),
                    trigger: event.compaction.trigger.clone(),
                    input_generation: event.compaction.input_generation,
                    output_generation: event.compaction.output_generation,
                    retained_count: event.compaction.retained_count,
                    dropped_count: event.compaction.dropped_count,
                    token_estimate_before: event.compaction.token_estimate_before,
                    token_estimate_after: event.compaction.token_estimate_after,
                    error: event.compaction.error.clone(),
                };
                let compaction_id = event.compaction.compaction_id.clone();
                let compaction_status = event.compaction.status.clone();
                self.state
                    .context_lifecycle_mut(&session_id)
                    .apply_compaction(state, compaction);
                self.state.status =
                    format!("Context compaction {compaction_id}: {compaction_status}");
                None
            }
            UiNotification::ContextNormalizationReported(event) => {
                let session_id = event.session_id.clone();
                let state = crate::model::ContextLifecycleState {
                    session_id: event.context_state.session_id.clone(),
                    thread_id: event.context_state.thread_id.clone(),
                    generation: event.context_state.generation,
                    transcript_hash: event.context_state.transcript_hash.clone(),
                    item_count: event.context_state.item_count,
                    token_estimate: event.context_state.token_estimate,
                    recovery_state: event.context_state.recovery_state.clone(),
                    last_checkpoint_id: event.context_state.last_checkpoint_id.clone(),
                    last_compaction_id: event.context_state.last_compaction_id.clone(),
                };
                let normalization = crate::model::ContextNormalizationSummary {
                    generation: event.normalization.generation,
                    model_capability_id: event.normalization.model_capability_id.clone(),
                    prompt_message_count: event.normalization.prompt_message_count,
                    token_estimate: event.normalization.token_estimate,
                    repaired_count: event.normalization.repaired_count,
                    dropped_count: event.normalization.dropped_count,
                    synthetic_count: event.normalization.synthetic_count,
                    truncated_count: event.normalization.truncated_count,
                };
                let prompt_count = event.normalization.prompt_message_count;
                self.state
                    .context_lifecycle_mut(&session_id)
                    .apply_normalization(state, normalization);
                self.state.status = format!("Context normalized: {prompt_count} prompt messages");
                None
            }
        }
    }

    fn apply_turn_spawn_complete(
        &mut self,
        event: octos_core::ui_protocol::TurnSpawnCompleteEvent,
    ) -> Option<AppUiCommand> {
        if let Some(session) = self.find_session_mut(&event.session_id) {
            session
                .messages
                .push(Message::assistant(event.content.clone()));
        }
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Progress, event.task_id.clone(), "completed")
                .with_detail(event.source),
        );
        self.state.status = format!("Background completion persisted: {}", event.message_id);
        None
    }

    fn apply_file_attached(
        &mut self,
        event: octos_core::ui_protocol::FileAttachedEvent,
    ) -> Option<AppUiCommand> {
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, event.path.clone(), "attached")
                .with_turn(event.turn_id)
                .with_detail(event.mime.unwrap_or_else(|| "artifact".into())),
        );
        self.state.status = format!("File attached: {}", event.path);
        None
    }

    fn apply_session_event_bridged(
        &mut self,
        event: octos_core::ui_protocol::SessionEventBridgedEvent,
    ) -> Option<AppUiCommand> {
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Progress, event.kind.clone(), "bridged")
                .with_detail("legacy session event"),
        );
        self.state.status = format!("Session event: {}", event.kind);
        None
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
        let follow_tail = self.state.transcript_scroll == 0;
        let complete_live_plan = self.turn_had_completion_activity(&event.turn_id);
        let fallback_summary = self.turn_completion_fallback_message(&event.turn_id);
        let partial_fallback_summary =
            self.turn_partial_completion_fallback_message(&event.turn_id);
        let (status, reset_scroll, completed_current_turn) = {
            let Some(session) = self.find_session_mut(&event.session_id) else {
                return None;
            };
            let title = session.title.clone();
            match session.live_reply.take() {
                Some(live_reply) if live_reply.turn_id == event.turn_id => {
                    let text = if live_reply.text.trim().is_empty() {
                        fallback_summary
                    } else if complete_live_plan && looks_like_partial_live_answer(&live_reply.text)
                    {
                        format!(
                            "{}\n\n{}",
                            live_reply.text.trim_end(),
                            partial_fallback_summary
                        )
                    } else if complete_live_plan {
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
                    {
                        session.messages.push(Message::assistant(fallback_summary));
                        format!("Turn completed in {title} at seq {seq}")
                    },
                    true,
                    true,
                ),
            }
        };
        if reset_scroll {
            if follow_tail {
                self.state.scroll_transcript_to_latest();
            } else {
                self.state.preserve_transcript_position_after_append(3);
            }
        }
        self.state.status = status;
        if completed_current_turn {
            self.state
                .capture_completed_turn_activity(&event.session_id, &event.turn_id);
            self.state.set_run_state_success();
        }
        self.submit_next_pending_if_idle()
    }

    fn fail_live_reply(&mut self, event: TurnErrorEvent) -> Option<AppUiCommand> {
        let follow_tail = self.state.transcript_scroll == 0;
        let fallback_summary =
            self.turn_error_fallback_message(&event.turn_id, &event.code, &event.message);
        let Some(session) = self.find_session_mut(&event.session_id) else {
            return None;
        };
        let title = session.title.clone();
        let (status, failed_current_turn) = match session.live_reply.take() {
            Some(live_reply) if live_reply.turn_id == event.turn_id => {
                let partial = compact_first_line(&live_reply.text, 120);
                let text = if partial.is_empty() {
                    fallback_summary
                } else {
                    format!("{fallback_summary}\n- Partial response: {partial}")
                };
                session.messages.push(Message::assistant(text));
                (
                    format!("Turn error {}: {}", event.code, event.message),
                    true,
                )
            }
            Some(live_reply) => {
                session.live_reply = Some(live_reply);
                (
                    format!("Ignored stale turn error in {title}: {}", event.code),
                    false,
                )
            }
            None => {
                session.messages.push(Message::assistant(fallback_summary));
                (
                    format!("Turn error {}: {}", event.code, event.message),
                    true,
                )
            }
        };
        if failed_current_turn {
            if follow_tail {
                self.state.scroll_transcript_to_latest();
            } else {
                self.state.preserve_transcript_position_after_append(3);
            }
            self.state
                .capture_completed_turn_activity(&event.session_id, &event.turn_id);
        }
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

    fn turn_completion_fallback_message(&self, turn_id: &TurnId) -> String {
        let summary = self.summarize_turn_activity(turn_id);
        format!(
            "Session Summary\n- Result: Turn completed, but the TUI did not receive a final assistant answer.\n- Activity: {} action(s) recorded.\n- Files changed: {}.\n- Validation: {}.\n- Risks / follow-up: Review the activity above and continue the turn if the requested answer is incomplete.",
            summary.action_count,
            format_limited_list(&summary.files_changed, "none observed"),
            format_limited_list(&summary.validation, "not reported"),
        )
    }

    fn turn_partial_completion_fallback_message(&self, turn_id: &TurnId) -> String {
        let summary = self.summarize_turn_activity(turn_id);
        format!(
            "Session Summary\n- Result: Turn completed, but the TUI only received a partial live answer.\n- Activity: {} action(s) recorded.\n- Files changed: {}.\n- Validation: {}.\n- Risks / follow-up: The server may have persisted a fuller answer; continue if the visible answer is incomplete.",
            summary.action_count,
            format_limited_list(&summary.files_changed, "none observed"),
            format_limited_list(&summary.validation, "not reported"),
        )
    }

    fn turn_error_fallback_message(&self, turn_id: &TurnId, code: &str, message: &str) -> String {
        let summary = self.summarize_turn_activity(turn_id);
        let failed = format_limited_list(&summary.failures, "none recorded");
        format!(
            "Session Summary\n- Result: Turn failed before producing a final answer.\n- Error: {code}: {message}\n- Activity: {} action(s) recorded.\n- Failures: {failed}.\n- Risks / follow-up: Fix the error above or continue the turn with a more specific instruction.",
            summary.action_count,
        )
    }

    fn summarize_turn_activity(&self, turn_id: &TurnId) -> TurnActivitySummary {
        let mut summary = TurnActivitySummary::default();
        for activity in self
            .state
            .activity
            .iter()
            .filter(|activity| activity.turn_id.as_ref() == Some(turn_id))
        {
            match activity.kind {
                ActivityKind::Tool => {
                    summary.action_count += 1;
                    let detail = activity
                        .detail
                        .as_deref()
                        .filter(|detail| !detail.trim().is_empty())
                        .unwrap_or(activity.title.as_str());
                    if activity.success == Some(false) || activity.status == "failed" {
                        push_unique_summary(&mut summary.failures, compact_first_line(detail, 96));
                    } else if looks_like_validation_activity(activity) {
                        push_unique_summary(
                            &mut summary.validation,
                            compact_first_line(detail, 96),
                        );
                    }
                }
                ActivityKind::Progress => {
                    if looks_like_file_change_activity(activity) {
                        let detail = activity
                            .detail
                            .as_deref()
                            .or_else(|| Some(activity.status.as_str()))
                            .unwrap_or_default();
                        push_unique_summary(
                            &mut summary.files_changed,
                            compact_first_line(detail, 96),
                        );
                    }
                }
                ActivityKind::Approval | ActivityKind::Warning | ActivityKind::Error => {}
            }
        }
        summary
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

fn onboarding_pending_status(pending: OnboardingProviderPending) -> String {
    match pending {
        OnboardingProviderPending::Test => "Provider test already in progress".into(),
        OnboardingProviderPending::Save => "Provider save already in progress".into(),
    }
}

fn split_first_word(input: &str) -> (&str, &str) {
    let input = input.trim();
    let Some(split_at) = input.find(char::is_whitespace) else {
        return (input, "");
    };
    let (head, rest) = input.split_at(split_at);
    (head, rest.trim_start())
}

fn non_empty_string(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn onboarding_workspace_cwd(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(command) = value.strip_prefix("stdio:") {
        return stdio_command_cwd(command);
    }
    if value.is_empty()
        || value.starts_with("ws://")
        || value.starts_with("wss://")
        || value == "stdio"
        || value == "unknown"
        || value == "not supplied"
    {
        None
    } else {
        Some(value.to_owned())
    }
}

fn stdio_command_cwd(command: &str) -> Option<String> {
    let parsed_parts = shlex::split(command);
    let fallback_parts;
    let parts = if let Some(parts) = parsed_parts.as_ref() {
        parts
    } else {
        fallback_parts = command
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        &fallback_parts
    };
    let mut parts = parts.iter().map(String::as_str);
    while let Some(part) = parts.next() {
        if part == "--cwd" {
            return parts.next().and_then(|cwd| non_empty_string(cwd.into()));
        }
        if let Some(cwd) = part.strip_prefix("--cwd=") {
            return non_empty_string(cwd.into());
        }
    }
    None
}

fn onboarding_usage() -> String {
    "Usage: /onboard [name|username|email|create-profile|profile|select|family|model|route|base-url|api-key-env|key|send-code|verify|catalog|save|test|finish|reset]".into()
}

fn login_usage() -> String {
    "Usage: /login [email <address>|send-code [email]|code <otp>|verify [otp]|status|me|logout]"
        .into()
}

fn provider_usage() -> String {
    "Usage: /provider [catalog|list|select <family_id> <model_id> <route_id> [base_url] [api_key_env]|family|model|route|base-url|api-key-env|api-type|key|test|save|add-fallback]".into()
}

fn skills_usage() -> String {
    "Usage: /skills [list|search <query>|install <repo> [--branch <branch>] [--force]|remove <name>]"
        .into()
}

fn mcp_usage() -> String {
    "Usage: /mcp [list|status|enable <server>|disable <server>|test <server>|upsert <server> {json}|delete <server>]"
        .into()
}

fn tools_usage() -> String {
    "Usage: /tools [list|status|enable <tool>|disable <tool>|test <tool>|upsert <tool> {json}|delete <tool>]"
        .into()
}

fn parse_single_name(input: &str, _usage: &str) -> Option<String> {
    let (name, trailing) = split_first_word(input);
    if name.is_empty() || !trailing.trim().is_empty() {
        return None;
    }
    non_empty_string(name.to_owned())
}

fn parse_name_and_json(input: &str, usage: String) -> Result<(String, Value), String> {
    let (name, rest) = split_first_word(input);
    let Some(name) = non_empty_string(name.to_owned()) else {
        return Err(usage);
    };
    let rest = rest.trim();
    let config = if rest.is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(rest).map_err(|err| format!("{usage}; invalid JSON: {err}"))?
    };
    Ok((name, config))
}

fn parse_skill_install_args(input: &str) -> Result<(String, Option<String>, bool), String> {
    let mut repo = None;
    let mut branch = None;
    let mut force = false;
    let mut parts = input.split_whitespace();

    while let Some(part) = parts.next() {
        if part == "--force" || part == "-f" {
            force = true;
        } else if part == "--branch" || part == "-b" {
            let Some(value) = parts
                .next()
                .and_then(|value| non_empty_string(value.to_owned()))
            else {
                return Err("Usage: /skills install <repo> [--branch <branch>] [--force]".into());
            };
            branch = Some(value);
        } else if let Some(value) = part.strip_prefix("--branch=") {
            let Some(value) = non_empty_string(value.to_owned()) else {
                return Err("Usage: /skills install <repo> [--branch <branch>] [--force]".into());
            };
            branch = Some(value);
        } else if part.starts_with('-') {
            return Err(format!("Unknown /skills install flag: {part}"));
        } else if repo.is_none() {
            repo = Some(part.to_owned());
        } else {
            return Err("Usage: /skills install <repo> [--branch <branch>] [--force]".into());
        }
    }

    let Some(repo) = repo.and_then(non_empty_string) else {
        return Err("Usage: /skills install <repo> [--branch <branch>] [--force]".into());
    };
    Ok((repo, branch, force))
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

fn filter_menu_result_for_search(mut result: MenuBuildResult, query: &str) -> MenuBuildResult {
    let tokens = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return result;
    }

    if let MenuBuildResult::Ready(spec) = &mut result
        && spec.searchable
    {
        spec.items
            .retain(|item| menu_item_matches_search_tokens(item, &tokens));
    }
    result
}

fn menu_item_matches_search_tokens(item: &crate::menu::MenuItem, tokens: &[String]) -> bool {
    let haystack = format!(
        "{} {} {}",
        item.id,
        item.label,
        item.description.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    tokens.iter().all(|token| haystack.contains(token))
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

fn profile_llm_list_has_provider_state(result: &ProfileLlmListResult) -> bool {
    result.primary_provider().is_some() || !result.fallback_providers().is_empty()
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
    use crate::model::{
        McpConfigMutationResult, McpStatusSummary, ModelStatus, OnboardingProviderPending,
        OnboardingProviderSaveTarget, ProfileLlmMutationResult, ProfileLocalCreateResult,
        RuntimeHealthStatus, RuntimePolicyMcpServer, RuntimePolicyStamp, SessionCursorStatus,
        SessionStatusReadResult, SessionUsageStatus, ToolConfigMutationResult,
    };
    use octos_core::SessionKey;
    use octos_core::ui_protocol::{
        ApprovalAutoResolvedEvent, ApprovalCancelledEvent, ApprovalDecidedEvent, ApprovalDecision,
        ApprovalDiffDetails, ApprovalId, ApprovalRequestedEvent, ApprovalTypedDetails,
        OutputCursor, PreviewId, ReplayLossyEvent, TaskRuntimeState, ToolCompletedEvent,
        ToolStartedEvent, TurnId, UiCursor, UiFileMutationNotice, UiProgressMetadata,
        UiProtocolCapabilities, UiTokenCostUpdate, approval_kinds, approval_scopes, methods,
        progress_kinds,
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

    fn protocol_store_without_sessions() -> Store {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "AppUI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.capabilities = None;
        store
    }

    fn applied_profile_llm_result() -> ProfileLlmMutationResult {
        ProfileLlmMutationResult {
            profile_id: Some("coding".into()),
            primary: None,
            fallbacks: Vec::new(),
            applied: true,
            llm: None,
            runtime_policy_stamp: None,
            message: None,
            error: None,
        }
    }

    fn failed_profile_llm_result(message: &str, error: &str) -> ProfileLlmMutationResult {
        ProfileLlmMutationResult {
            profile_id: Some("coding".into()),
            primary: None,
            fallbacks: Vec::new(),
            applied: false,
            llm: None,
            runtime_policy_stamp: None,
            message: Some(message.into()),
            error: Some(error.into()),
        }
    }

    #[test]
    fn onboarding_slash_flow_builds_profile_llm_upsert_and_masks_secret() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_AUTH_SEND_CODE,
            crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);

        store.state.composer = "/onboard email user@example.com".into();
        assert!(store.compose_command().is_none());
        assert_eq!(store.state.onboarding.email, "user@example.com");

        store.state.composer = "/onboard select moonshot kimi-k2.5 autodl https://www.autodl.art/api/v1 AUTODL_API_KEY".into();
        assert!(store.compose_command().is_none());
        assert_eq!(store.state.onboarding.provider.family_id, "moonshot");
        assert_eq!(store.state.onboarding.provider.model_id, "kimi-k2.5");
        assert_eq!(store.state.onboarding.provider.route.route_id, "autodl");

        store.state.composer = "/onboard key sk-test-secret".into();
        assert!(store.compose_command().is_none());
        assert_eq!(store.state.onboarding.api_key_label(), "********");

        store.state.composer = "/onboard save".into();
        let command = store
            .compose_command()
            .expect("save emits profile/llm/upsert");
        let AppUiCommand::ProfileLlmUpsert(params) = command else {
            panic!("expected profile/llm/upsert");
        };
        assert_eq!(params.profile_id.as_deref(), Some("coding"));
        assert_eq!(params.selection.family_id, "moonshot");
        assert_eq!(params.selection.model_id, "kimi-k2.5");
        assert_eq!(params.selection.route.route_id, "autodl");
        assert_eq!(
            params
                .api_key
                .as_ref()
                .expect("api key is included for transport")
                .expose_for_transport(),
            "sk-test-secret"
        );
        assert!(!format!("{params:?}").contains("sk-test-secret"));
        assert!(!format!("{:?}", store.state.onboarding).contains("sk-test-secret"));
    }

    #[test]
    fn provider_slash_flow_builds_fallback_llm_upsert() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT]);

        store.state.composer =
            "/provider select minimax MiniMax-M2.5-highspeed wisemodel https://open.ospreyai.cn/v1 WISEMODEL_API_KEY".into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/provider key sk-fallback-secret".into();
        assert!(store.compose_command().is_none());

        store.state.composer = "/provider add-fallback".into();
        let command = store
            .compose_command()
            .expect("fallback save emits profile/llm/upsert");
        let AppUiCommand::ProfileLlmUpsert(params) = command else {
            panic!("expected profile/llm/upsert");
        };
        assert!(!params.set_primary);
        assert_eq!(params.selection.family_id, "minimax");
        assert_eq!(params.selection.model_id, "MiniMax-M2.5-highspeed");
        assert_eq!(params.selection.route.route_id, "wisemodel");
        assert_eq!(
            params
                .api_key
                .as_ref()
                .expect("api key is included for transport")
                .expose_for_transport(),
            "sk-fallback-secret"
        );
    }

    #[test]
    fn onboarding_provider_test_shows_pending_until_result() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_TEST]);
        store.state.composer =
            "/onboard select moonshot kimi-k2.5 autodl https://www.autodl.art/api/v1 AUTODL_API_KEY"
                .into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/onboard key sk-test-secret".into();
        assert!(store.compose_command().is_none());

        store.state.composer = "/onboard test".into();
        let command = store
            .compose_command()
            .expect("test emits profile/llm/test");
        assert!(matches!(command, AppUiCommand::ProfileLlmTest(_)));
        assert_eq!(
            store.state.onboarding.provider_pending,
            Some(OnboardingProviderPending::Test)
        );
        assert_eq!(store.state.status, "Testing provider connection");

        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: applied_profile_llm_result(),
                message: "Provider connection verified".into(),
            },
        ));

        assert_eq!(store.state.onboarding.provider_pending, None);
        assert!(store.state.onboarding.provider_tested);
        assert!(!store.state.onboarding.provider_saved);
    }

    #[test]
    fn onboarding_provider_test_failure_clears_pending_without_marking_tested() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_TEST]);
        store.state.composer =
            "/onboard select moonshot kimi-k2.5 autodl https://www.autodl.art/api/v1 AUTODL_API_KEY"
                .into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/onboard key sk-test-secret".into();
        assert!(store.compose_command().is_none());

        store.state.composer = "/onboard test".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ProfileLlmTest(_))
        ));
        assert_eq!(
            store.state.onboarding.provider_pending,
            Some(OnboardingProviderPending::Test)
        );

        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: failed_profile_llm_result("Provider connection failed", "invalid API key"),
                message: "Provider connection failed: invalid API key".into(),
            },
        ));

        assert_eq!(store.state.onboarding.provider_pending, None);
        assert!(!store.state.onboarding.provider_tested);
        assert!(!store.state.onboarding.provider_saved);
        assert_eq!(
            store.state.status,
            "Provider connection failed: invalid API key"
        );
    }

    #[test]
    fn onboarding_provider_save_shows_pending_until_result() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT]);
        store.state.composer =
            "/onboard select moonshot kimi-k2.5 autodl https://www.autodl.art/api/v1 AUTODL_API_KEY"
                .into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/onboard key sk-test-secret".into();
        assert!(store.compose_command().is_none());

        store.state.composer = "/onboard save".into();
        let command = store
            .compose_command()
            .expect("save emits profile/llm/upsert");
        assert!(matches!(command, AppUiCommand::ProfileLlmUpsert(_)));
        assert_eq!(
            store.state.onboarding.provider_pending,
            Some(OnboardingProviderPending::Save)
        );
        assert_eq!(store.state.status, "Saving provider configuration");

        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: applied_profile_llm_result(),
                message: "Provider profile updated".into(),
            },
        ));

        assert_eq!(store.state.onboarding.provider_pending, None);
        assert!(store.state.onboarding.provider_tested);
        assert!(store.state.onboarding.provider_saved);
    }

    #[test]
    fn fallback_save_resets_staged_provider_but_keeps_primary_saved_status() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT]);
        store.state.composer =
            "/provider select moonshot kimi-k2.5 autodl https://www.autodl.art/api/v1 AUTODL_API_KEY"
                .into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/provider key sk-primary-secret".into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/provider save".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ProfileLlmUpsert(_))
        ));
        assert_eq!(
            store.state.onboarding.provider_save_target,
            Some(OnboardingProviderSaveTarget::Primary)
        );
        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: applied_profile_llm_result(),
                message: "Primary provider saved".into(),
            },
        ));
        assert!(store.state.onboarding.provider_saved);
        assert_eq!(
            store
                .state
                .onboarding
                .saved_primary_provider_label
                .as_deref(),
            Some("moonshot / kimi-k2.5 via autodl")
        );

        store.state.composer =
            "/provider select minimax MiniMax-M2.5-highspeed wisemodel https://open.ospreyai.cn/v1 WISEMODEL_API_KEY"
                .into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/provider key sk-fallback-secret".into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/provider add-fallback".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ProfileLlmUpsert(_))
        ));
        assert_eq!(
            store.state.onboarding.provider_save_target,
            Some(OnboardingProviderSaveTarget::Fallback)
        );

        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: applied_profile_llm_result(),
                message: "Fallback provider saved".into(),
            },
        ));

        assert!(store.state.onboarding.provider_saved);
        assert_eq!(store.state.onboarding.provider_pending, None);
        assert_eq!(store.state.onboarding.provider_save_target, None);
        assert!(!store.state.onboarding.provider_tested);
        assert!(!store.state.onboarding.selection_ready());
        assert!(store.state.onboarding.api_key.is_none());
        assert_eq!(
            store.state.onboarding.last_saved_provider_target,
            Some(OnboardingProviderSaveTarget::Fallback)
        );
        assert_eq!(
            store.state.onboarding.last_saved_provider_label.as_deref(),
            Some("minimax / MiniMax-M2.5-highspeed via wisemodel")
        );
        assert_eq!(
            store
                .state
                .onboarding
                .saved_primary_provider_label
                .as_deref(),
            Some("moonshot / kimi-k2.5 via autodl")
        );
    }

    #[test]
    fn onboarding_finish_opens_profile_scoped_session() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.workspace.root = "/tmp/workspace".into();
        store.state.onboarding.provider_saved = true;

        store.state.composer = "/onboard profile alice".into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/onboard finish".into();
        let command = store.compose_command().expect("finish emits session/open");
        let AppUiCommand::OpenSession(params) = command else {
            panic!("expected session/open");
        };

        assert_eq!(params.profile_id.as_deref(), Some("alice"));
        assert_eq!(params.cwd.as_deref(), Some("/tmp/workspace"));
        assert!(params.session_id.0.starts_with("alice:local:tui#coding"));
    }

    #[test]
    fn onboarding_session_open_extracts_cwd_from_stdio_target_label() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.workspace.root =
            "stdio:/opt/octos serve --stdio --data-dir /tmp/octos/data --cwd /tmp/octos/workspace"
                .into();
        store.state.onboarding.provider_saved = true;

        store.state.composer = "/onboard profile alice".into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/onboard finish".into();
        let command = store.compose_command().expect("finish emits session/open");
        let AppUiCommand::OpenSession(params) = command else {
            panic!("expected session/open");
        };

        assert_eq!(params.cwd.as_deref(), Some("/tmp/octos/workspace"));
    }

    #[test]
    fn onboarding_session_open_unquotes_cwd_from_stdio_target_label() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.workspace.root =
            "stdio:\"/opt/octos/bin/octos\" serve --stdio --data-dir \"/tmp/octos/data dir\" --cwd \"/tmp/octos/workspace dir\""
                .into();
        store.state.onboarding.provider_saved = true;

        store.state.composer = "/onboard profile alice".into();
        assert!(store.compose_command().is_none());
        store.state.composer = "/onboard finish".into();
        let command = store.compose_command().expect("finish emits session/open");
        let AppUiCommand::OpenSession(params) = command else {
            panic!("expected session/open");
        };

        assert_eq!(params.cwd.as_deref(), Some("/tmp/octos/workspace dir"));
    }

    #[test]
    fn normal_prompt_without_open_session_is_preserved() {
        let mut store = protocol_store_without_sessions();
        store.state.composer = "please edit src/main.rs".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert_eq!(store.state.composer, "please edit src/main.rs");
        assert_eq!(
            store.state.status,
            "No coding session open. Run /onboard open-session before sending a prompt."
        );
    }

    #[test]
    fn onboarding_open_session_requires_saved_primary_provider() {
        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
        ]));
        store.state.onboarding.profile_id = Some("alice".into());
        store.state.composer = "/onboard open-session".into();

        let command = store.compose_command();

        assert!(command.is_none());
        assert_eq!(
            store.state.status,
            "Cannot open session: save a primary LLM provider first."
        );
    }

    #[test]
    fn onboarding_open_session_uses_profile_llm_primary_provider() {
        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
        ]));
        store.state.profile_llm_state = Some(crate::model::ProfileLlmListResult {
            profile_id: Some("alice".into()),
            primary: Some(crate::model::LlmConfiguredProvider {
                provider: "deepseek".into(),
                model: "deepseek-reasoner".into(),
                family_id: Some("deepseek".into()),
                model_id: Some("deepseek-reasoner".into()),
                route: None,
                route_id: Some("deepseek".into()),
                base_url: None,
                api_key_env: None,
                has_api_key: true,
                selected: true,
                available: Some(true),
                model_hints: None,
                cost_per_m: None,
                strong: Some(true),
            }),
            fallbacks: Vec::new(),
            llm: None,
            runtime_policy_stamp: None,
        });
        store.state.composer = "/onboard open-session".into();

        let command = store.compose_command().expect("session/open command");
        let AppUiCommand::OpenSession(params) = command else {
            panic!("expected session/open");
        };

        assert_eq!(params.profile_id.as_deref(), Some("alice"));
        assert!(params.session_id.0.starts_with("alice:local:tui#coding"));
    }

    #[test]
    fn solo_onboarding_finish_creates_local_profile_without_otp() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::model::APPUI_METHOD_AUTH_SEND_CODE,
            crate::model::APPUI_METHOD_AUTH_VERIFY,
        ]);
        store.state.workspace.root = "/tmp/solo-project".into();

        for command in [
            "/onboard name Ada Lovelace",
            "/onboard username ada",
            "/onboard email ada@example.com",
        ] {
            store.state.composer = command.into();
            assert!(store.compose_command().is_none());
        }

        store.state.composer = "/onboard finish".into();
        let command = store
            .compose_command()
            .expect("finish emits profile/local/create");
        let AppUiCommand::ProfileLocalCreate(params) = command else {
            panic!("expected profile/local/create");
        };
        assert_eq!(params.name, "Ada Lovelace");
        assert_eq!(params.username, "ada");
        assert_eq!(params.email, "ada@example.com");

        store.state.composer = "/onboard send-code".into();
        assert!(store.compose_command().is_none());
        assert!(store.state.status.contains("profile/local/create"));
    }

    #[test]
    fn solo_profile_id_from_server_opens_session() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.workspace.root = "/tmp/solo-project".into();
        store.state.onboarding.open_session_after_profile_create = true;
        store.state.onboarding.provider_saved = true;

        let follow_up = store.apply_client_event(ClientEvent::ProfileLocalCreate(
            crate::client_event::ProfileLocalCreateClientEvent {
                result: ProfileLocalCreateResult {
                    profile_id: "ada-server".into(),
                    user_id: "ada-user".into(),
                    name: "Ada Lovelace".into(),
                    username: "ada".into(),
                    email: "ada@example.com".into(),
                    created: true,
                    runtime_mode: "solo".into(),
                },
                message: "Local solo profile created: ada-server".into(),
            },
        ));
        let Some(AppUiCommand::OpenSession(params)) = follow_up else {
            panic!("profile/local/create should be followed by session/open");
        };

        assert_eq!(
            store.state.onboarding.profile_id.as_deref(),
            Some("ada-server")
        );
        assert_eq!(params.profile_id.as_deref(), Some("ada-server"));
        assert_eq!(params.cwd.as_deref(), Some("/tmp/solo-project"));
        assert!(
            params
                .session_id
                .0
                .starts_with("ada-server:local:tui#coding")
        );
    }

    #[test]
    fn local_profile_create_auto_loads_provider_catalog_for_next_onboarding_step() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
        ]);

        let follow_up = store.apply_client_event(ClientEvent::ProfileLocalCreate(
            crate::client_event::ProfileLocalCreateClientEvent {
                result: ProfileLocalCreateResult {
                    profile_id: "ada-server".into(),
                    user_id: "ada-user".into(),
                    name: "Ada Lovelace".into(),
                    username: "ada".into(),
                    email: "ada@example.com".into(),
                    created: true,
                    runtime_mode: "solo".into(),
                },
                message: "Local solo profile created: ada-server".into(),
            },
        ));

        assert!(matches!(
            follow_up,
            Some(AppUiCommand::ProfileLlmCatalog(_))
        ));
        assert_eq!(
            store.state.onboarding.profile_id.as_deref(),
            Some("ada-server")
        );
    }

    #[test]
    fn first_launch_opens_onboarding_menu_when_server_advertises_solo_profile_create() {
        let mut store = protocol_store_without_sessions();

        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 1 methods".into(),
        }));

        assert!(store.state.sessions.is_empty());
        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected onboarding menu to open");
        };
        assert_eq!(spec.id, MenuId::from(crate::menu::registry::MENU_ONBOARD));
        assert!(
            spec.items
                .iter()
                .any(|item| item.id == "onboard.local.create")
        );
    }

    /// M22-A: provider-only capabilities (e.g. `profile/llm/catalog`)
    /// must NOT auto-open onboarding on first launch — without any
    /// profile creation method, there is no onboarding to drive.
    #[test]
    fn first_launch_does_not_open_onboarding_when_only_provider_methods_advertised() {
        let mut store = protocol_store_without_sessions();

        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[
                        crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
                        crate::model::APPUI_METHOD_MODEL_LIST,
                        crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
                    ],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 3 methods".into(),
        }));

        assert!(store.state.sessions.is_empty());
        assert!(
            store.state.active_menu.is_none(),
            "onboarding must not auto-open without a profile-creation method"
        );
    }

    /// M22-A: legacy email-OTP onboarding triggers first-launch flow
    /// only when `auth/send_code`, `auth/verify`, AND `auth/me` are
    /// advertised. `auth/me` is required because the wizard auto-issues
    /// it after a successful `auth/verify` to resolve the profile id.
    #[test]
    fn first_launch_opens_onboarding_when_legacy_auth_advertised() {
        let mut store = protocol_store_without_sessions();

        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[
                        crate::model::APPUI_METHOD_AUTH_SEND_CODE,
                        crate::model::APPUI_METHOD_AUTH_VERIFY,
                        crate::model::APPUI_METHOD_AUTH_ME,
                    ],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 3 methods".into(),
        }));

        let Some(menu) = store.state.active_menu.as_ref() else {
            panic!("legacy auth must open onboarding on first launch");
        };
        let active_id = match menu {
            MenuBuildResult::Ready(spec) => spec.id.as_str().to_owned(),
            MenuBuildResult::Loading(status)
            | MenuBuildResult::Unavailable(status)
            | MenuBuildResult::Error(status) => status.id.as_str().to_owned(),
        };
        assert_eq!(active_id, crate::menu::registry::MENU_ONBOARD);
    }

    /// M22-A: a backend that advertises only `auth/send_code` (missing
    /// `auth/verify` or `auth/me`) is mid-implementation and must not
    /// auto-open; the registry constant requires all three legs of the
    /// OTP flow.
    #[test]
    fn first_launch_does_not_open_onboarding_when_legacy_auth_is_partial() {
        let mut store = protocol_store_without_sessions();

        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[crate::model::APPUI_METHOD_AUTH_SEND_CODE],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 1 method".into(),
        }));

        assert!(
            store.state.active_menu.is_none(),
            "partial legacy auth must not auto-open onboarding"
        );
    }

    /// M22-A: `auth/send_code` + `auth/verify` without `auth/me` is
    /// still partial — the wizard would strand the user after OTP
    /// verification with no profile id resolved, which is exactly the
    /// non-completable state this gate must prevent.
    #[test]
    fn first_launch_does_not_open_onboarding_when_legacy_auth_lacks_auth_me() {
        let mut store = protocol_store_without_sessions();

        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[
                        crate::model::APPUI_METHOD_AUTH_SEND_CODE,
                        crate::model::APPUI_METHOD_AUTH_VERIFY,
                    ],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 2 methods".into(),
        }));

        assert!(
            store.state.active_menu.is_none(),
            "legacy auth without auth/me cannot complete; must not auto-open"
        );
    }

    /// M22-A: with zero capabilities the TUI cannot decide whether
    /// onboarding is supported, so it must leave the first-launch
    /// surface alone instead of opening a broken onboarding menu.
    #[test]
    fn first_launch_does_not_open_onboarding_without_capabilities() {
        let store = protocol_store_without_sessions();

        assert!(store.state.capabilities.is_none());
        assert!(
            store.state.active_menu.is_none(),
            "no capabilities means no auto-open of onboarding"
        );
    }

    /// M22-A: `/setup` is an alias of `/onboard`, so typing either
    /// must open the same onboarding surface — there is exactly one
    /// `OnboardingWizardState`-backed menu.
    #[test]
    fn setup_alias_opens_same_onboarding_surface_as_onboard() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        // Clear any auto-opened menu so the slash command itself drives
        // the surface deterministically.
        store.close_all_menus();

        store.state.composer = "/setup".into();
        let command = store.compose_command();
        assert!(command.is_none());

        let active_id = store
            .state
            .menu_stack
            .active()
            .map(|frame| frame.id.as_str().to_owned())
            .expect("/setup must open the onboarding menu");
        assert_eq!(active_id, crate::menu::registry::MENU_ONBOARD);
    }

    #[test]
    fn searchable_menu_filters_items_and_dispatches_filtered_action() {
        let mut store = store_with_empty_session();
        store.open_menu(MenuId::from(crate::menu::registry::MENU_HELP));
        store
            .state
            .menu_stack
            .active_mut()
            .expect("active menu")
            .search_query = "keymap".into();
        store.refresh_active_menu();

        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected searchable menu");
        };
        let labels = spec
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["/keymap"]);

        assert!(store.accept_active_menu_item().is_none());
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_KEYMAP)
        );
    }

    #[test]
    fn mcp_and_tool_config_mutations_refresh_server_truth() {
        let mut store = store_with_empty_session();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_MCP_CONFIG_LIST,
            crate::model::APPUI_METHOD_TOOL_CONFIG_LIST,
        ]));

        let mcp_follow_up = store.apply_client_event(ClientEvent::McpConfigMutation(
            McpConfigMutationClientEvent {
                result: McpConfigMutationResult {
                    profile_id: Some("coding".into()),
                    ok: true,
                    applied: true,
                    server: Some("github".into()),
                    ..McpConfigMutationResult::default()
                },
                message: "MCP config applied: github".into(),
            },
        ));
        let Some(AppUiCommand::ListMcpConfig(params)) = mcp_follow_up else {
            panic!("expected MCP config refresh after mutation");
        };
        assert_eq!(params.profile_id.as_deref(), Some("coding"));

        let tool_follow_up = store.apply_client_event(ClientEvent::ToolConfigMutation(
            ToolConfigMutationClientEvent {
                result: ToolConfigMutationResult {
                    profile_id: Some("coding".into()),
                    ok: true,
                    applied: true,
                    tool: Some("web_fetch".into()),
                    ..ToolConfigMutationResult::default()
                },
                message: "Tool config applied: web_fetch".into(),
            },
        ));
        assert!(matches!(
            tool_follow_up,
            Some(AppUiCommand::ListToolConfig(_))
        ));
    }

    #[test]
    fn onboarding_field_rows_prefill_composer_for_inline_editing() {
        let mut store = protocol_store_without_sessions();
        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 1 methods".into(),
        }));
        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected onboarding menu");
        };
        let name_index = spec
            .items
            .iter()
            .position(|item| item.id == "onboard.local.name")
            .expect("name row");
        store
            .state
            .menu_stack
            .active_mut()
            .expect("active menu")
            .selected_index = name_index;

        assert!(store.accept_active_menu_item().is_none());

        assert_eq!(store.state.composer, "/onboard name ");
        assert_eq!(store.state.focus, FocusPane::Composer);
        assert_eq!(store.state.status, "Edit the field, then press Enter");
    }

    #[test]
    fn onboarding_local_fields_advance_menu_selection_after_edit() {
        let mut store = protocol_store_without_sessions();
        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 1 methods".into(),
        }));
        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected onboarding menu");
        };
        let name_index = spec
            .items
            .iter()
            .position(|item| item.id == "onboard.local.name")
            .expect("name row");
        store
            .state
            .menu_stack
            .active_mut()
            .expect("active menu")
            .selected_index = name_index;

        store.state.composer = "/onboard name Ada Lovelace".into();
        assert!(store.compose_command().is_none());

        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected onboarding menu after edit");
        };
        let selected = store
            .state
            .menu_stack
            .active()
            .expect("active menu")
            .selected_index;
        assert_eq!(spec.items[selected].id, "onboard.local.username");
    }

    #[test]
    fn onboarding_provider_selection_focuses_api_key_then_api_key_advances() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_TEST]);
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));

        store.state.composer = "/onboard select deepseek deepseek-reasoner official".into();
        assert!(store.compose_command().is_none());

        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected provider setup menu");
        };
        let selected = store
            .state
            .menu_stack
            .active()
            .expect("active menu")
            .selected_index;
        assert_eq!(spec.items[selected].id, "onboard.provider.key");

        store.state.composer = "/onboard key sk-test-secret".into();
        assert!(store.compose_command().is_none());

        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected provider setup menu after key");
        };
        let selected = store
            .state
            .menu_stack
            .active()
            .expect("active menu")
            .selected_index;
        assert_eq!(spec.items[selected].id, "onboard.provider.test");
    }

    #[test]
    fn capabilities_do_not_steal_focus_from_existing_session() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        assert!(!store.state.menu_stack.is_active());

        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
                    &[],
                ),
            },
            message: "AppUI capabilities refreshed: 1 methods".into(),
        }));

        assert!(!store.state.menu_stack.is_active());
        assert!(store.state.active_menu.is_none());
    }

    fn session_status_result(session_id: &SessionKey) -> SessionStatusReadResult {
        SessionStatusReadResult {
            session_id: session_id.clone(),
            runtime_mode: Some("solo".into()),
            profile_id: Some("coding".into()),
            cwd: Some("/workspace/octos".into()),
            workspace_root: Some("/workspace/octos".into()),
            active_turn_id: Some(TurnId::new()),
            runtime_policy_stamp: Some(RuntimePolicyStamp {
                runtime_mode: Some("solo".into()),
                profile_id: Some("coding".into()),
                model: Some("deepseek-v4-pro".into()),
                provider: Some("deepseek".into()),
                approval_policy: Some("never".into()),
                sandbox_mode: Some("workspace-write".into()),
                sandbox: Some("workspace-write".into()),
                permission_profile: Some("workspace-write-no-network".into()),
                filesystem_scope: Some("workspace".into()),
                network: Some("blocked".into()),
                tool_policy_id: Some("coding-v3".into()),
                mcp_servers: vec![
                    RuntimePolicyMcpServer::name("github"),
                    RuntimePolicyMcpServer::name("filesystem"),
                ],
                memory_scope: Some("profile-session".into()),
                qoe_policy: Some("balanced".into()),
                queue_mode: Some("collect".into()),
                tool_contract_id: Some("codex-compatible-coding-v1".into()),
                tool_contract_version: Some("1".into()),
                model_toolset: Some("coding".into()),
                dynamic_tool_discovery: Some("enabled".into()),
            }),
            model: Some(ModelStatus {
                model: "deepseek-v4-pro".into(),
                provider: "deepseek".into(),
                title: None,
                family: None,
                route: None,
                selected: true,
                available: Some(true),
                queue_mode: Some("collect".into()),
                qoe_policy: Some("balanced".into()),
            }),
            permission_profile: Some("workspace-write-no-network".into()),
            approval_policy: Some("never".into()),
            sandbox_mode: Some("workspace-write".into()),
            sandbox: Some("workspace-write".into()),
            filesystem_scope: Some("workspace".into()),
            network: Some("blocked".into()),
            tool_policy_id: Some("coding-v3".into()),
            mcp_servers: vec!["github".into(), "filesystem".into()],
            memory_scope: Some("profile-session".into()),
            health: Some(RuntimeHealthStatus {
                status: "healthy".into(),
                message: Some("ws ok".into()),
            }),
            mcp_summary: Some(McpStatusSummary {
                connected: 2,
                connecting: 0,
                failed: 0,
                disabled: 1,
            }),
            tool_summary: None,
            usage: Some(SessionUsageStatus {
                input_tokens: Some(1200),
                output_tokens: Some(340),
                cached_input_tokens: None,
                cached_output_tokens: None,
                estimated_cost_micros_usd: Some(2500),
            }),
            cursor: Some(SessionCursorStatus {
                cursor: Some(UiCursor {
                    stream: "session".into(),
                    seq: 42,
                }),
                replay_supported: true,
                healthy: true,
                detail: Some("caught up".into()),
            }),
            capabilities: Some(UiProtocolCapabilities::new(
                &[crate::model::APPUI_METHOD_SESSION_STATUS_READ],
                &[],
            )),
        }
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
        assert!(no_capability_names.contains(&"/model".into()));
        assert!(!no_capability_names.contains(&"/mcp".into()));

        let partial = protocol_store_with_methods(&[methods::APPROVAL_SCOPES_LIST]);
        let partial_names = partial
            .slash_command_matches("")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(partial_names.contains(&"/permissions".into()));
        assert!(partial_names.contains(&"/model".into()));
        assert!(!partial_names.contains(&"/mcp".into()));

        let full = protocol_store_with_methods(&[
            methods::APPROVAL_SCOPES_LIST,
            crate::menu::registry::APPUI_METHOD_MODEL_LIST,
            crate::menu::registry::APPUI_METHOD_MODEL_SELECT,
            crate::menu::registry::APPUI_METHOD_MCP_STATUS_LIST,
        ]);
        let full_names = full
            .slash_command_matches("")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(full_names.contains(&"/model".into()));
        assert!(full_names.contains(&"/permissions".into()));
        assert!(full_names.contains(&"/mcp".into()));
    }

    #[test]
    fn skills_slash_commands_build_profile_skill_appui_commands() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_REMOVE,
        ]);

        store.state.composer =
            "/skills install octos-org/skills/deep-search --branch dev --force".into();
        let command = store.compose_command().expect("install command");
        let AppUiCommand::ProfileSkillsInstall(params) = command else {
            panic!("expected profile skills install command");
        };
        assert_eq!(params.profile_id.as_deref(), Some("coding"));
        assert_eq!(params.repo, "octos-org/skills/deep-search");
        assert_eq!(params.branch.as_deref(), Some("dev"));
        assert!(params.force);

        store.state.composer = "/skills search research".into();
        let command = store.compose_command().expect("search command");
        let AppUiCommand::ProfileSkillsRegistrySearch(params) = command else {
            panic!("expected profile skills registry search command");
        };
        assert_eq!(params.profile_id.as_deref(), Some("coding"));
        assert_eq!(params.q.as_deref(), Some("research"));

        store.state.composer = "/skills remove deep-search".into();
        let command = store.compose_command().expect("remove command");
        let AppUiCommand::ProfileSkillsRemove(params) = command else {
            panic!("expected profile skills remove command");
        };
        assert_eq!(params.name, "deep-search");
    }

    #[test]
    fn profile_llm_list_seeds_pre_session_profile_for_skill_commands() {
        let mut store = Store {
            state: AppState::new(Vec::new(), 0, "ready".into(), Some("stdio".into()), false),
        };
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST,
        ]));

        store.apply_client_event(ClientEvent::ProfileLlmList(ProfileLlmListClientEvent {
            result: crate::model::ProfileLlmListResult {
                profile_id: Some("dspfac".into()),
                primary: None,
                fallbacks: Vec::new(),
                llm: None,
                runtime_policy_stamp: None,
            },
            message: "Loaded profile LLM settings".into(),
        }));

        let command = store
            .dispatch_skills_inline("list")
            .expect("skills list command");
        let AppUiCommand::ProfileSkillsList(params) = command else {
            panic!("expected profile skills list command");
        };
        assert_eq!(params.profile_id.as_deref(), Some("dspfac"));
    }

    #[test]
    fn skills_mutation_refreshes_installed_skill_list() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL,
        ]);

        let follow_up = store.apply_client_event(ClientEvent::ProfileSkillsMutation(
            ProfileSkillsMutationClientEvent {
                result: crate::model::ProfileSkillsMutationResult {
                    profile_id: Some("coding".into()),
                    ok: true,
                    installed: vec!["deep-search".into()],
                    ..crate::model::ProfileSkillsMutationResult::default()
                },
                message: "Installed skill(s): deep-search".into(),
            },
        ));

        assert!(matches!(
            follow_up,
            Some(AppUiCommand::ProfileSkillsList(_))
        ));
        assert_eq!(store.state.status, "Refreshing profile skills");
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
        assert_eq!(store.state.status, "Menu: model");
    }

    #[test]
    fn snapshot_preserves_active_menu_stack_and_rebuilds_from_capabilities() {
        let mut store = protocol_store_with_methods(&[]);
        store.open_menu(MenuId::from(crate::menu::registry::MENU_HELP));
        assert!(help_menu_labels(&store).contains(&"/status".to_string()));
        assert!(!help_menu_labels(&store).contains(&"/permissions".to_string()));
        assert!(!help_menu_labels(&store).contains(&"/model".to_string()));

        let session = store.state.sessions[0].clone();
        store.apply_event(AppUiEvent::Snapshot(AppUiSnapshot {
            sessions: vec![session],
            selected_session: 0,
            status: "snapshot replay".into(),
            target: Some("ws://example.test/ui-protocol".into()),
            readonly: false,
        }));
        store.apply_capabilities_event(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[
                        crate::menu::registry::APPUI_METHOD_MODEL_LIST,
                        crate::menu::registry::APPUI_METHOD_MODEL_SELECT,
                        methods::APPROVAL_SCOPES_LIST,
                    ],
                    &[],
                ),
            },
            message: "capabilities replay".into(),
        });
        store.refresh_active_menu();

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
    fn session_status_client_event_updates_cached_runtime_status() {
        let mut store = protocol_store_with_methods(&[methods::APPROVAL_SCOPES_LIST]);
        let session_id = store.state.sessions[0].id.clone();

        store.apply_client_event(ClientEvent::SessionStatus(SessionStatusClientEvent {
            result: session_status_result(&session_id),
            message: "Runtime status refreshed".into(),
        }));

        let runtime_status = store
            .state
            .runtime_status_for(&session_id)
            .expect("cached runtime status");
        assert_eq!(
            runtime_status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.tool_policy_id.as_deref()),
            Some("coding-v3")
        );
        assert_eq!(
            store.state.sessions[0].profile_id.as_deref(),
            Some("coding")
        );
        assert!(
            store
                .state
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| {
                    capabilities.supports_method(crate::model::APPUI_METHOD_SESSION_STATUS_READ)
                })
        );
        assert_eq!(store.state.status, "Runtime status refreshed");
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
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        assert_eq!(store.state.sessions[0].messages.len(), 1);
        assert!(store.state.sessions[0].live_reply.is_none());
        assert_eq!(store.state.run_state.label(), "done");
    }

    #[test]
    fn turn_completed_captures_activity_log_for_transcript_and_clears_live_buffer() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "done");
        let session_id = store.state.sessions[0].id.clone();
        store.state.record_submitted_user_prompt(
            session_id.clone(),
            turn_id.clone(),
            "build the site".into(),
        );
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id.clone())
                .with_detail("cargo build")
                .with_success(true),
        );

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id: turn_id.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        assert!(
            store
                .state
                .activity
                .iter()
                .all(|item| item.turn_id.as_ref() != Some(&turn_id))
        );
        let log = store
            .state
            .turn_activity_logs
            .iter()
            .find(|log| log.turn_id == turn_id)
            .expect("turn activity log");
        assert_eq!(log.request.as_deref(), Some("build the site"));
        assert_eq!(log.items.len(), 1);
    }

    #[test]
    fn turn_completed_without_model_answer_inserts_fallback_summary() {
        let turn_id = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id.clone())
                .with_detail("cargo test")
                .with_success(true),
        );

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let message = store.state.sessions[0]
            .messages
            .last()
            .expect("fallback assistant message");
        assert_eq!(message.role.as_str(), "assistant");
        assert!(message.content.contains("Session Summary"));
        assert!(
            message
                .content
                .contains("TUI did not receive a final assistant answer")
        );
        assert!(message.content.contains("cargo test"));
        assert_eq!(store.state.run_state.label(), "done");
    }

    #[test]
    fn turn_completed_with_empty_live_reply_inserts_fallback_summary() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let message = store.state.sessions[0]
            .messages
            .last()
            .expect("fallback assistant message");
        assert!(message.content.contains("Session Summary"));
        assert!(
            message
                .content
                .contains("TUI did not receive a final assistant answer")
        );
    }

    #[test]
    fn turn_completed_with_partial_live_reply_appends_fallback_summary() {
        let turn_id = TurnId::new();
        let mut store =
            store_with_live_reply(turn_id.clone(), "The JWST site is complete and ready in");
        let session_id = store.state.sessions[0].id.clone();
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "list_dir", "complete")
                .with_turn(turn_id.clone())
                .with_detail("jwst-site")
                .with_success(true),
        );

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                turn_id,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let message = store.state.sessions[0]
            .messages
            .last()
            .expect("assistant message");
        assert!(message.content.starts_with("The JWST site is complete"));
        assert!(
            message
                .content
                .contains("TUI only received a partial live answer")
        );
        assert!(message.content.contains("1 action(s) recorded"));
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
                tokens_in: None,
                tokens_out: None,
                session_result: None,
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
                    tokens_in: None,
                    tokens_out: None,
                    session_result: None,
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
                    tokens_in: None,
                    tokens_out: None,
                    session_result: None,
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
                tokens_in: None,
                tokens_out: None,
                session_result: None,
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
        let message = store.state.sessions[0]
            .messages
            .last()
            .expect("fallback assistant message");
        assert!(message.content.contains("Session Summary"));
        assert!(
            message
                .content
                .contains("interrupted: turn interrupted by client")
        );
        assert!(message.content.contains("Partial response: streaming"));
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

    /// M16-G2 wiring guard: `context/compaction_completed` events must
    /// land on the per-session lifecycle ledger so the bounded status
    /// surface can render the active generation and last compaction
    /// summary without poking back into the raw event stream.
    #[test]
    fn context_compaction_event_writes_lifecycle_ledger_entry() {
        use octos_core::ui_protocol::{
            ContextCompactionCompletedEvent, UiContextCompactionRecord, UiContextState,
        };

        let session_id = SessionKey("local:test".into());
        let session = SessionView {
            id: session_id.clone(),
            title: "test".into(),
            profile_id: None,
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        let mut store = Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        };

        // Before the event the ledger is empty (TUI hides the surface).
        assert!(store.state.context_lifecycle_for(&session_id).is_none());

        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextCompactionCompleted(ContextCompactionCompletedEvent {
                session_id: session_id.clone(),
                context_state: UiContextState {
                    session_id: session_id.clone(),
                    thread_id: Some("thread-1".into()),
                    generation: 4,
                    transcript_hash: "abc123".into(),
                    item_count: 42,
                    token_estimate: 9100,
                    recovery_state: "healthy".into(),
                    last_checkpoint_id: Some("chk-001".into()),
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
                    token_estimate_before: 31200,
                    token_estimate_after: Some(9100),
                    error: None,
                },
            }),
        ));

        let ledger = store
            .state
            .context_lifecycle_for(&session_id)
            .expect("ledger created");
        assert_eq!(ledger.state.as_ref().unwrap().generation, 4);
        assert_eq!(ledger.last_compaction.as_ref().unwrap().retained_count, 42);
        assert_eq!(ledger.last_compaction.as_ref().unwrap().dropped_count, 88);
        // The bounded status string the user sees must NOT include raw
        // transcript hashes or summary item ids.
        assert!(
            !store.state.status.contains("abc123"),
            "{}",
            store.state.status
        );
        assert!(
            !store.state.status.contains("sum-1"),
            "{}",
            store.state.status
        );
    }

    /// M16-G2 wiring guard: `context/normalization_reported` events
    /// must update the same per-session ledger without trashing prior
    /// compaction state (so the status surface can render both at once).
    #[test]
    fn context_normalization_event_preserves_prior_compaction_entry() {
        use octos_core::ui_protocol::{
            ContextCompactionCompletedEvent, ContextNormalizationReportedEvent,
            UiContextCompactionRecord, UiContextNormalizationReport, UiContextState,
        };

        let session_id = SessionKey("local:test".into());
        let session = SessionView {
            id: session_id.clone(),
            title: "test".into(),
            profile_id: None,
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        let mut store = Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        };

        let base_state = UiContextState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 5,
            transcript_hash: "h".into(),
            item_count: 10,
            token_estimate: 4000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        };
        // Compact first.
        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextCompactionCompleted(ContextCompactionCompletedEvent {
                session_id: session_id.clone(),
                context_state: base_state.clone(),
                compaction: UiContextCompactionRecord {
                    compaction_id: "c-1".into(),
                    checkpoint_id: "chk-1".into(),
                    status: "applied".into(),
                    policy_id: "p".into(),
                    trigger: "token_budget".into(),
                    input_generation: 4,
                    output_generation: Some(5),
                    input_transcript_hash: "in".into(),
                    replacement_transcript_hash: None,
                    installed_transcript_hash: None,
                    input_item_count: 30,
                    retained_count: 10,
                    dropped_count: 20,
                    summary_item_id: None,
                    token_estimate_before: 12000,
                    token_estimate_after: Some(4000),
                    error: None,
                },
            }),
        ));
        // Then normalize.
        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextNormalizationReported(ContextNormalizationReportedEvent {
                session_id: session_id.clone(),
                context_state: base_state.clone(),
                normalization: UiContextNormalizationReport {
                    generation: 5,
                    input_transcript_hash: "h".into(),
                    output_prompt_hash: "out".into(),
                    model_capability_id: "anthropic/sonnet-4.7".into(),
                    prompt_message_count: 10,
                    token_estimate: 4000,
                    repaired_count: 1,
                    dropped_count: 0,
                    synthetic_count: 0,
                    truncated_count: 0,
                },
            }),
        ));

        let ledger = store
            .state
            .context_lifecycle_for(&session_id)
            .expect("ledger created");
        // Both records survive — the renderer can show both at once.
        assert!(ledger.last_compaction.is_some(), "compaction retained");
        assert!(ledger.last_normalization.is_some(), "normalization stored");
        assert_eq!(ledger.last_compaction.as_ref().unwrap().retained_count, 10);
        assert_eq!(
            ledger.last_normalization.as_ref().unwrap().repaired_count,
            1
        );
    }
}
