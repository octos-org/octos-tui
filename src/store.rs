use std::collections::BTreeSet;

use octos_core::app_ui::{AppUiEvent, AppUiSnapshot};
use octos_core::ui_protocol::{
    ApprovalAutoResolvedEvent, ApprovalCancelledEvent, ApprovalDecidedEvent, ApprovalId,
    ApprovalRespondParams, DiffPreviewGetParams, EnvelopeNotification, EnvelopeToolEndStatus,
    HydratedMessage, InputItem, MessageDeltaEvent, MessagePersistedEvent, Payload,
    ReplayLossyEvent, SessionHydrateParams, SessionHydrateResult, SessionOpenParams,
    TaskArtifactReadParams, TaskOutputDeltaEvent, TaskOutputReadParams, TaskRuntimeState,
    TaskUpdatedEvent, ThreadGraphGetParams, TurnCompletedEvent, TurnErrorEvent, TurnId,
    TurnInterruptParams, TurnLifecycleState, TurnSpawnCompleteEvent, TurnStartParams,
    TurnStateGetParams, UiContextState, UiNotification, UiProgressEvent,
    UserQuestionRequestedEvent,
};
use octos_core::{Message, MessageRole, SessionKey, TaskId, ThreadId};
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
        ReviewStartParams, ReviewStartResult, SecretString, SessionMcpCatalog, SessionModelCatalog,
        SessionRuntimeStatus, SessionToolCatalog, SessionView, TaskView, ToolConfigDeleteParams,
        ToolConfigListParams, ToolConfigSetEnabledParams, ToolConfigTestParams,
        ToolConfigUpsertParams, UserQuestionPickerState, complete_plan_steps_in_text,
        task_state_label, terminal_task_state_from_agent_status,
    },
};

const TASK_OUTPUT_TAIL_BYTES: usize = 600;
const TASK_OUTPUT_READ_LIMIT_BYTES: u64 = 4096;
const TASK_ARTIFACT_READ_LIMIT_BYTES: u64 = 4096;

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

/// Compact token-count for the compaction activity notice: `31200` -> `31.2k`,
/// small counts stay verbatim.
fn humanize_token_count(tokens: usize) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
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
        rendered.push_str(&t!("status.list_more", count = values.len() - 3));
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

/// Finalize the accumulated `live_reply` text into the assistant message body
/// for a turn that just completed. Empty streams fall back to the summary card;
/// non-empty streams may be appended with a partial-answer note or have their
/// plan checkboxes completed. Shared by `commit_live_reply` (matched-turn arm)
/// and the lazy-bind path so a continuation turn renders the same way whether
/// or not its `TurnStarted` was delivered.
fn finalize_live_reply_text(
    text: String,
    complete_live_plan: bool,
    fallback_summary: &str,
    partial_fallback_summary: &str,
) -> String {
    if text.trim().is_empty() {
        fallback_summary.to_string()
    } else if complete_live_plan && looks_like_partial_live_answer(&text) {
        format!("{}\n\n{}", text.trim_end(), partial_fallback_summary)
    } else if complete_live_plan {
        complete_plan_steps_in_text(&text)
    } else {
        text
    }
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

    /// Seed the first-launch onboarding workspace candidate from the launch
    /// `--cwd`, when explicitly provided.
    ///
    /// The onboarding workspace probe resolves its target via
    /// [`OnboardingWizardState::workspace_target`], which prefers
    /// `workspace_candidate` over `workspace.root`. For a stdio launch,
    /// `workspace.root` is seeded from the transport target LABEL (e.g.
    /// `stdio:octos serve --stdio --solo --data-dir ...`), not from `--cwd` —
    /// so without this the probe can only validate a `--cwd` that the user
    /// also embedded inside the `--stdio-command` string. The top-level
    /// `--cwd` is otherwise wired only into `session/open`; the onboarding
    /// probe never sees it, so `/onboard finish` stays blocked on
    /// "workspace not validated" and the profile runtime never bootstraps.
    ///
    /// Seeding the candidate from the explicit `--cwd` (the same path
    /// `session/open` uses) lets the probe validate it. `get_or_insert` keeps
    /// a later explicit `/onboard workspace <path>` authoritative.
    ///
    /// UX2 (#1377 follow-up): when no `--cwd` is supplied the candidate now
    /// falls back to the process working directory (the `octos-tui --cwd`
    /// default the help text already documents), so the documented launch
    /// `octos serve --stdio --solo` — which carries NO `--cwd` and whose
    /// transport label resolves to `"stdio"`/empty — still validates a genuine
    /// directory out of the box instead of dead-ending on
    /// "no usable workspace cwd". The fallback is skipped for remote/WS
    /// transports, where the workspace root lives on the server and the local
    /// cwd would be wrong.
    pub fn seed_onboarding_workspace_cwd(&mut self, cwd: Option<String>) {
        let resolved =
            resolve_launch_workspace_cwd(cwd, !self.is_remote_transport_target(), || {
                std::env::current_dir()
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            });
        if let Some(resolved) = resolved {
            self.state
                .onboarding
                .workspace_candidate
                .get_or_insert(resolved);
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
            self.state.status = t!("status.readonly_turn_disabled").into_owned();
            self.state.clear_current_composer_draft();
            return None;
        }

        if prompt.is_empty() {
            return None;
        }

        if self.active_session().is_none() {
            self.state.status = t!("status.no_session_send_prompt").into_owned();
            self.state.focus = FocusPane::Composer;
            return None;
        }

        self.state.clear_current_composer_draft();
        if self.state.active_turn().is_some() {
            self.state.pending_messages.push(prompt);
            self.state.status = t!("status.message_staged").into_owned();
            self.state.scroll_transcript_to_latest();
            return None;
        }

        self.start_prompt_turn(prompt, t!("status.queued_turn_start").into_owned())
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
                    || t!(command.description)
                        .to_ascii_lowercase()
                        .contains(&query)
                {
                    Some(2)
                } else {
                    None
                }?;
                Some((
                    rank,
                    SlashCommandMatch {
                        name: command.slash_name(),
                        description: t!(command.description).into_owned(),
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
                    let fallback_reason = t!("status.command_unavailable");
                    self.show_unavailable_slash_command(
                        &command_name,
                        availability.reason.as_deref().unwrap_or(&fallback_reason),
                    );
                    return None;
                }
                // M15-E autonomy commands need richer-than-verb parsing
                // (intervals, multi-word objectives). The registry's
                // job here is purely capability gating + menu visibility;
                // the parser owns syntax.
                if matches!(
                    &command.entry,
                    crate::menu::types::CommandEntry::LocalAction(
                        crate::menu::types::LocalAction::Custom("autonomy"),
                    )
                ) {
                    return self.dispatch_autonomy_slash(draft);
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

    /// M15-E: parse `/agents`, `/goal`, `/loop` through
    /// [`crate::autonomy::parse_autonomy_slash`] and dispatch one
    /// Octos UI command per parsed intent. Capability checks are enforced
    /// at the dispatch site (and via the registry's
    /// `coding.autonomy.v1` gate), so old servers see the slash
    /// command rendered as `Unsupported` rather than getting probed.
    pub(crate) fn dispatch_autonomy_slash(&mut self, draft: &str) -> Option<AppUiCommand> {
        match crate::autonomy::parse_autonomy_slash(draft) {
            Ok(Some(crate::autonomy::AutonomyCommand::Agents(cmd))) => {
                self.dispatch_agents_command(cmd)
            }
            Ok(Some(crate::autonomy::AutonomyCommand::Task(cmd))) => {
                self.dispatch_task_command(cmd)
            }
            Ok(Some(crate::autonomy::AutonomyCommand::Thread(cmd))) => {
                self.dispatch_thread_command(cmd)
            }
            Ok(Some(crate::autonomy::AutonomyCommand::Turn(cmd))) => {
                self.dispatch_turn_command(cmd)
            }
            Ok(Some(crate::autonomy::AutonomyCommand::Goal(cmd))) => {
                self.dispatch_goal_command(cmd)
            }
            Ok(Some(crate::autonomy::AutonomyCommand::Loop(cmd))) => {
                self.dispatch_loop_command(cmd)
            }
            Ok(None) => None,
            Err(err) => {
                self.state.status = err.to_string();
                None
            }
        }
    }

    fn dispatch_task_command(&mut self, cmd: crate::autonomy::TaskCommand) -> Option<AppUiCommand> {
        use crate::autonomy::TaskCommand;
        let session_id = self.active_autonomy_session_id()?;
        let profile_id = self
            .state
            .active_session()
            .and_then(|session| session.profile_id.clone());
        match cmd {
            TaskCommand::ArtifactRead { task_id, selector } => {
                if !self.require_appui_feature(crate::model::APPUI_FEATURE_TASK_ARTIFACTS_V1) {
                    return None;
                }
                if !self.require_appui_method(crate::model::APPUI_METHOD_TASK_ARTIFACT_READ) {
                    return None;
                }
                let Ok(task_id) = task_id.parse::<TaskId>() else {
                    self.state.status = t!("status.invalid_task_id", id = task_id).into_owned();
                    return None;
                };
                let (artifact_id, path, label) = match selector {
                    crate::autonomy::TaskArtifactSelector::Id(id) => {
                        let label = id.clone();
                        (Some(id), None, label)
                    }
                    crate::autonomy::TaskArtifactSelector::Path(path) => {
                        let label = path.clone();
                        (None, Some(path), label)
                    }
                };
                self.state.status =
                    t!("status.reading_task_artifact", label = label, id = task_id).into_owned();
                Some(AppUiCommand::ReadTaskArtifact(TaskArtifactReadParams {
                    session_id,
                    task_id,
                    artifact_id,
                    path,
                    cursor: None,
                    limit_bytes: Some(TASK_ARTIFACT_READ_LIMIT_BYTES),
                    profile_id,
                    agent_id: None,
                }))
            }
        }
    }

    fn dispatch_thread_command(
        &mut self,
        cmd: crate::autonomy::ThreadCommand,
    ) -> Option<AppUiCommand> {
        use crate::autonomy::ThreadCommand;
        let session_id = self.active_autonomy_session_id()?;
        match cmd {
            ThreadCommand::Graph => {
                if !self.require_appui_feature(crate::model::APPUI_FEATURE_THREAD_GRAPH_V1) {
                    return None;
                }
                if !self.require_appui_method(crate::model::APPUI_METHOD_THREAD_GRAPH_GET) {
                    return None;
                }
                self.state.status = t!("status.reading_thread_graph").into_owned();
                Some(AppUiCommand::GetThreadGraph(ThreadGraphGetParams {
                    session_id,
                    at: None,
                }))
            }
        }
    }

    fn dispatch_turn_command(&mut self, cmd: crate::autonomy::TurnCommand) -> Option<AppUiCommand> {
        use crate::autonomy::TurnCommand;
        let session_id = self.active_autonomy_session_id()?;
        match cmd {
            TurnCommand::State(turn_id_raw) => {
                if !self.require_appui_feature(crate::model::APPUI_FEATURE_TURN_STATE_GET_V1) {
                    return None;
                }
                if !self.require_appui_method(crate::model::APPUI_METHOD_TURN_STATE_GET) {
                    return None;
                }
                let turn_id = match turn_id_raw {
                    Some(raw) => match serde_json::from_value::<TurnId>(Value::String(raw.clone()))
                    {
                        Ok(turn_id) => turn_id,
                        Err(_) => {
                            self.state.status = t!("status.invalid_turn_id", id = raw).into_owned();
                            return None;
                        }
                    },
                    None => match self.state.active_turn().map(|(_, turn_id)| turn_id.clone()) {
                        Some(turn_id) => turn_id,
                        None => {
                            self.state.status = t!("status.no_active_turn_inspect").into_owned();
                            return None;
                        }
                    },
                };
                self.state.status = t!(
                    "status.reading_turn_state",
                    id = short_id(&turn_id.0.to_string())
                )
                .into_owned();
                Some(AppUiCommand::GetTurnState(TurnStateGetParams {
                    session_id,
                    turn_id,
                }))
            }
        }
    }

    fn dispatch_agents_command(
        &mut self,
        cmd: crate::autonomy::AgentsCommand,
    ) -> Option<AppUiCommand> {
        use crate::autonomy::AgentsCommand;
        let session_id = self.active_autonomy_session_id()?;
        match cmd {
            AgentsCommand::List => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_LIST) {
                    return None;
                }
                self.state.status = t!("status.refreshing_agent_list").into_owned();
                Some(AppUiCommand::ListAgents(crate::model::AgentListParams {
                    session_id,
                    parent_agent_id: None,
                }))
            }
            AgentsCommand::Status(maybe_id) => match maybe_id {
                Some(agent_id) => {
                    if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_STATUS_READ) {
                        return None;
                    }
                    self.state.status =
                        t!("status.reading_agent_status", id = agent_id).into_owned();
                    Some(AppUiCommand::ReadAgentStatus(
                        crate::model::AgentStatusReadParams {
                            session_id,
                            agent_id,
                        },
                    ))
                }
                None => {
                    if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_LIST) {
                        return None;
                    }
                    self.state.status = t!("status.refreshing_agent_list").into_owned();
                    Some(AppUiCommand::ListAgents(crate::model::AgentListParams {
                        session_id,
                        parent_agent_id: None,
                    }))
                }
            },
            AgentsCommand::Output(agent_id) => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_OUTPUT_READ) {
                    return None;
                }
                self.state.status = t!("status.reading_agent_output", id = agent_id).into_owned();
                Some(AppUiCommand::ReadAgentOutput(
                    crate::model::AgentOutputReadParams {
                        session_id,
                        agent_id,
                        cursor: None,
                    },
                ))
            }
            AgentsCommand::Artifacts(agent_id) => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_ARTIFACT_LIST) {
                    return None;
                }
                self.state.status =
                    t!("status.listing_agent_artifacts", id = agent_id).into_owned();
                Some(AppUiCommand::ListAgentArtifacts(
                    crate::model::AgentArtifactListParams {
                        session_id,
                        agent_id,
                    },
                ))
            }
            AgentsCommand::ArtifactRead { agent_id, selector } => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_AGENT_ARTIFACT_READ) {
                    return None;
                }
                let (artifact_id, path, label) = match selector {
                    crate::autonomy::AgentArtifactSelector::Id(id) => {
                        let label = id.clone();
                        (Some(id), None, label)
                    }
                    crate::autonomy::AgentArtifactSelector::Path(path) => {
                        let label = path.clone();
                        (None, Some(path), label)
                    }
                };
                self.state.status = t!(
                    "status.reading_agent_artifact",
                    label = label,
                    id = agent_id
                )
                .into_owned();
                Some(AppUiCommand::ReadAgentArtifact(
                    crate::model::AgentArtifactReadParams {
                        session_id,
                        agent_id,
                        artifact_id,
                        path,
                    },
                ))
            }
            AgentsCommand::Interrupt(agent_id) => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_AGENT_INTERRUPT) {
                    return None;
                }
                self.state.status =
                    t!("status.interrupt_requested_for", id = agent_id).into_owned();
                Some(AppUiCommand::InterruptAgent(
                    crate::model::AgentInterruptParams {
                        session_id,
                        agent_id,
                    },
                ))
            }
            AgentsCommand::Close(agent_id) => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_AGENT_CLOSE) {
                    return None;
                }
                self.state.status = t!("status.close_requested_for", id = agent_id).into_owned();
                Some(AppUiCommand::CloseAgent(crate::model::AgentCloseParams {
                    session_id,
                    agent_id,
                }))
            }
        }
    }

    fn dispatch_goal_command(&mut self, cmd: crate::autonomy::GoalCommand) -> Option<AppUiCommand> {
        use crate::autonomy::GoalCommand;
        let session_id = self.active_autonomy_session_id()?;
        let profile_id = self.active_session_profile_id();
        match cmd {
            GoalCommand::Show => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_SESSION_GOAL_GET) {
                    return None;
                }
                self.state.status = t!("status.refreshing_goal").into_owned();
                Some(AppUiCommand::GetSessionGoal(
                    crate::model::SessionGoalGetParams {
                        session_id,
                        profile_id,
                    },
                ))
            }
            GoalCommand::Set(objective) => {
                let objective = objective.trim().to_string();
                if objective.is_empty() {
                    self.state.status = t!("status.goal_objective_empty").into_owned();
                    return None;
                }
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_SESSION_GOAL_SET)
                {
                    return None;
                }
                self.state.status = t!("status.setting_goal", objective = objective).into_owned();
                Some(AppUiCommand::SetSessionGoal(
                    crate::model::SessionGoalSetParams {
                        session_id,
                        profile_id,
                        objective,
                        status: Some("active".into()),
                        token_budget: None,
                        transition_actor: Some("user".into()),
                        action: crate::model::SessionGoalSetAction::Set,
                    },
                ))
            }
            GoalCommand::Pause => self.start_goal_transition(
                session_id,
                profile_id,
                "paused",
                crate::model::SessionGoalSetAction::Pause,
                "pause",
            ),
            GoalCommand::Resume => self.start_goal_transition(
                session_id,
                profile_id,
                "active",
                crate::model::SessionGoalSetAction::Resume,
                "resume",
            ),
            GoalCommand::Clear => {
                if !self
                    .require_mutating_appui_method(crate::model::APPUI_METHOD_SESSION_GOAL_CLEAR)
                {
                    return None;
                }
                self.state.status = t!("status.clearing_goal").into_owned();
                Some(AppUiCommand::ClearSessionGoal(
                    crate::model::SessionGoalClearParams {
                        session_id,
                        profile_id,
                    },
                ))
            }
        }
    }

    fn dispatch_loop_command(&mut self, cmd: crate::autonomy::LoopCommand) -> Option<AppUiCommand> {
        use crate::autonomy::{LoopCadence, LoopCommand};
        let session_id = self.active_autonomy_session_id()?;
        let profile_id = self.active_session_profile_id();
        match cmd {
            LoopCommand::List => {
                if !self.require_appui_method(crate::model::APPUI_METHOD_LOOP_LIST) {
                    return None;
                }
                self.state.status = t!("status.listing_loops").into_owned();
                Some(AppUiCommand::ListLoops(crate::model::LoopListParams {
                    session_id,
                    profile_id,
                }))
            }
            LoopCommand::Create { prompt, cadence } => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_LOOP_CREATE) {
                    return None;
                }
                let (mode, interval_seconds) = match cadence {
                    LoopCadence::SelfPaced => (crate::model::LoopMode::SelfPaced, None),
                    LoopCadence::Every(duration) => {
                        let secs = duration.as_secs();
                        if secs == 0 {
                            self.state.status = t!("status.loop_interval_min").into_owned();
                            return None;
                        }
                        (crate::model::LoopMode::FixedInterval, Some(secs))
                    }
                    LoopCadence::Maintenance => (crate::model::LoopMode::Maintenance, None),
                };
                self.state.status = t!("status.creating_loop").into_owned();
                Some(AppUiCommand::CreateLoop(crate::model::LoopCreateParams {
                    session_id,
                    profile_id,
                    prompt,
                    mode,
                    interval_seconds,
                }))
            }
            LoopCommand::Delete(loop_id) => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_LOOP_DELETE) {
                    return None;
                }
                self.state.status = t!("status.deleting_loop", id = loop_id).into_owned();
                Some(AppUiCommand::DeleteLoop(crate::model::LoopIdParams {
                    session_id,
                    loop_id,
                }))
            }
            LoopCommand::Pause(loop_id) => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_LOOP_PAUSE) {
                    return None;
                }
                self.state.status = t!("status.pausing_loop", id = loop_id).into_owned();
                Some(AppUiCommand::PauseLoop(crate::model::LoopIdParams {
                    session_id,
                    loop_id,
                }))
            }
            LoopCommand::Resume(loop_id) => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_LOOP_RESUME) {
                    return None;
                }
                self.state.status = t!("status.resuming_loop", id = loop_id).into_owned();
                Some(AppUiCommand::ResumeLoop(crate::model::LoopIdParams {
                    session_id,
                    loop_id,
                }))
            }
            LoopCommand::FireNow(loop_id) => {
                if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_LOOP_FIRE_NOW) {
                    return None;
                }
                self.state.status = t!("status.firing_loop", id = loop_id).into_owned();
                Some(AppUiCommand::FireLoopNow(crate::model::LoopIdParams {
                    session_id,
                    loop_id,
                }))
            }
        }
    }

    /// Returns the session id every autonomy command targets — the
    /// currently selected session. None when no session is open; the
    /// caller updates `status` and bails.
    fn active_autonomy_session_id(&mut self) -> Option<SessionKey> {
        match self.active_session() {
            Some(session) => Some(session.id.clone()),
            None => {
                self.state.status = t!("status.no_session_runtime").into_owned();
                self.state.focus = FocusPane::Composer;
                None
            }
        }
    }

    fn active_session_profile_id(&self) -> Option<String> {
        self.active_session()
            .and_then(|session| session.profile_id.clone())
    }

    /// Returns the cached goal record IFF the goal is in a state the
    /// TUI is allowed to transition. Per UPCR-2026-021 the model owns
    /// the `complete` transition — the TUI must not reactivate a
    /// completed goal via pause/resume. Returns `Err(status)` when a
    /// goal is cached but not in a TUI-transitionable state, so the
    /// caller can surface a precise message.
    fn cached_goal_for_transition(
        &self,
        session_id: &SessionKey,
    ) -> Result<octos_core::ui_protocol::UiGoalRecord, Option<String>> {
        let goal = self
            .state
            .session_autonomy_for(session_id)
            .and_then(|entry| entry.goal.as_ref())
            .ok_or(None)?
            .clone();
        match goal.status.as_str() {
            "active" | "paused" | "budget_limited" => Ok(goal),
            other => Err(Some(other.to_string())),
        }
    }

    /// Shared `/goal pause` / `/goal resume` entry. To avoid sending a
    /// stale cached objective on transition (the cached mirror can drift
    /// between explicit refreshes), this stages the desired status in
    /// `pending_goal_transition` and emits a `session/goal/get`. When
    /// the `GoalGet` response arrives, [`Self::apply_autonomy_result`]
    /// emits the follow-up `session/goal/set` with the freshly-fetched
    /// objective + staged status. The model-owned `complete` guard is
    /// still enforced up front using the cached mirror so the TUI fails
    /// fast on a clearly invalid transition without a round-trip.
    fn start_goal_transition(
        &mut self,
        session_id: SessionKey,
        profile_id: Option<String>,
        status: &'static str,
        action: crate::model::SessionGoalSetAction,
        verb: &str,
    ) -> Option<AppUiCommand> {
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_SESSION_GOAL_SET) {
            return None;
        }
        match self.cached_goal_for_transition(&session_id) {
            Ok(_) => {}
            Err(None) => {
                self.state.status =
                    t!("status.cannot_verb_no_goal_cached", verb = verb).into_owned();
                return None;
            }
            Err(Some(state)) => {
                self.state.status =
                    t!("status.cannot_verb_goal_state", verb = verb, state = state).into_owned();
                return None;
            }
        }
        // `session/goal/get` is the precondition for the follow-up set.
        // If the server advertised set but not get, the TUI cannot do
        // the refresh dance — fall back to "no transition" and let the
        // user re-issue `/goal <objective>` explicitly.
        if !self.require_appui_method(crate::model::APPUI_METHOD_SESSION_GOAL_GET) {
            self.state.status = t!("status.cannot_verb_no_goal_get", verb = verb).into_owned();
            return None;
        }
        self.state.pending_goal_transition = Some(crate::model::PendingGoalTransition {
            session_id: session_id.clone(),
            profile_id: profile_id.clone(),
            status,
            action,
        });
        self.state.status = t!("status.refreshing_goal_before", verb = verb).into_owned();
        Some(AppUiCommand::GetSessionGoal(
            crate::model::SessionGoalGetParams {
                session_id,
                profile_id,
            },
        ))
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
            CommandEntry::AppUiAction(crate::menu::types::AppUiActionKind::ReviewStart) => {
                self.review_start_command(inline_args.unwrap_or_default())
            }
            CommandEntry::AppUiAction(action) => {
                self.state.status =
                    t!("status.appui_not_wired", method = action.method()).into_owned();
                None
            }
            CommandEntry::PromptTemplate(template) => self.start_prompt_turn(
                (*template).to_string(),
                t!("status.queued_prompt_template").into_owned(),
            ),
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
                        t!("status.local_stop").into_owned(),
                        t!("status.no_active_turn").into_owned(),
                        Some(t!("status.nothing_sent_to_backend").into_owned()),
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
                match crate::cli::ThemeName::from_id(&theme) {
                    Some(name) => {
                        self.state.theme = name;
                        // Repaint the open menu so the `*` current marker moves
                        // to the just-selected theme; the palette itself updates
                        // on the next frame (event loop reads `state.theme`).
                        self.refresh_active_menu_if_open();
                        self.state.status = t!("status.theme_set", theme = theme).into_owned();
                    }
                    None => {
                        self.state.status = t!("status.theme_unknown", theme = theme).into_owned();
                    }
                }
                None
            }
            LocalAction::SaveStatusLine(items) => {
                self.state.status = t!(
                    "status.statusline_layout_selected",
                    items = items.join(", ")
                )
                .into_owned();
                None
            }
            LocalAction::SaveTerminalTitle(items) => {
                self.state.status = t!(
                    "status.terminal_title_layout_selected",
                    items = items.join(", ")
                )
                .into_owned();
                None
            }
            LocalAction::SaveKeymap => {
                self.state.status = t!("status.keymap_save_not_wired").into_owned();
                None
            }
            LocalAction::RefreshMenu(id) => {
                self.open_menu(id);
                None
            }
            LocalAction::EditComposer(draft) => {
                self.state.set_composer_text(draft);
                self.state.focus = FocusPane::Composer;
                self.state.status = t!("status.edit_field_prompt").into_owned();
                None
            }
            LocalAction::Onboarding(action) => self.dispatch_onboarding_action(action, inline_args),
            LocalAction::Skills => self.dispatch_skills_inline(inline_args.unwrap_or_default()),
            LocalAction::McpConfig => self.dispatch_mcp_inline(inline_args.unwrap_or_default()),
            LocalAction::ToolConfig => self.dispatch_tools_inline(inline_args.unwrap_or_default()),
            LocalAction::SetLanguage => self.dispatch_set_language(inline_args.unwrap_or_default()),
            LocalAction::SetLanguageCode(lang) => self.dispatch_set_language_code(lang),
            LocalAction::SetThinking => self.dispatch_set_thinking(inline_args.unwrap_or_default()),
            LocalAction::SetThinkingLevel(level) => self.dispatch_set_thinking_level(level),
            LocalAction::CopyLastReply => {
                self.copy_last_reply();
                None
            }
            LocalAction::Custom(name) => {
                self.state.status = t!("status.local_action_not_wired", name = name).into_owned();
                None
            }
        }
    }

    /// `/lang <code>` — switch the UI display language at runtime. Empty arg
    /// shows the current locale + usage; an unknown code is reported without
    /// changing the locale. On success `rust_i18n::set_locale` flips the
    /// process-global locale and the next render repaints the UI; the
    /// confirmation is itself rendered in the newly-selected language.
    /// `/lang` with no arg opens the language selection menu; with an arg
    /// (`en`/`zh`/a `LANG`-style value) it sets the locale inline as a shortcut.
    fn dispatch_set_language(&mut self, inline_args: &str) -> Option<AppUiCommand> {
        let arg = inline_args.trim();
        if arg.is_empty() {
            self.open_menu(MenuId::from(crate::menu::registry::MENU_LANG));
            return None;
        }
        match crate::cli::Lang::from_env_value(arg) {
            Some(lang) => self.dispatch_set_language_code(lang),
            None => {
                self.state.status = t!("lang.unknown", value = arg.to_string()).to_string();
                None
            }
        }
    }

    /// Apply a specific UI language. Shared by the inline `/lang <code>` shortcut
    /// and the `/lang` selection menu.
    fn dispatch_set_language_code(&mut self, lang: crate::cli::Lang) -> Option<AppUiCommand> {
        rust_i18n::set_locale(lang.code());
        // Rebuild any open menu so it repaints in the new language now; the
        // cached `active_menu` spec was built under the old locale. (The status
        // line + composer placeholder are rebuilt every frame, so they switch
        // without this.)
        self.refresh_active_menu_if_open();
        self.state.status = t!("lang.switched").to_string();
        None
    }

    /// `/thinking <low|medium|high|max|default>` — set the per-session reasoning
    /// effort attached to every turn/start, or `default` to clear the override
    /// (server gateway/profile default applies). No-op for models without a
    /// reasoning style; the server decides whether the effort is honored.
    /// `/thinking` with no arg opens the selection menu; with an arg
    /// (`low|medium|high|max|default`) it sets the level inline as a shortcut.
    fn dispatch_set_thinking(&mut self, inline_args: &str) -> Option<AppUiCommand> {
        use octos_core::ui_protocol::ReasoningEffortLevel as L;
        let arg = inline_args.trim().to_ascii_lowercase();
        if arg.is_empty() {
            self.open_menu(MenuId::from(crate::menu::registry::MENU_THINKING));
            return None;
        }
        let level = match arg.as_str() {
            "low" => Some(L::Low),
            "medium" | "med" => Some(L::Medium),
            "high" => Some(L::High),
            "max" => Some(L::Max),
            "default" | "reset" => None,
            other => {
                self.state.status = t!("thinking.unknown", value = other.to_string()).to_string();
                return None;
            }
        };
        self.dispatch_set_thinking_level(level)
    }

    /// Set the active session's reasoning effort to `level` (`None` clears the
    /// override). Shared by the inline `/thinking <level>` shortcut and the
    /// `/thinking` selection menu.
    fn dispatch_set_thinking_level(
        &mut self,
        level: Option<octos_core::ui_protocol::ReasoningEffortLevel>,
    ) -> Option<AppUiCommand> {
        use octos_core::ui_protocol::ReasoningEffortLevel as L;
        let Some(session_id) = self.active_session().map(|s| s.id.clone()) else {
            self.state.status = t!("thinking.no_session").to_string();
            return None;
        };
        match level {
            Some(l) => {
                self.state.session_reasoning_effort.insert(session_id, l);
                let name = match l {
                    L::Low => "low",
                    L::Medium => "medium",
                    L::High => "high",
                    L::Max => "max",
                };
                self.state.status = t!("thinking.set", level = name).to_string();
            }
            None => {
                self.state.session_reasoning_effort.remove(&session_id);
                self.state.status = t!("thinking.cleared").to_string();
            }
        }
        None
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
                    self.state.status = t!("status.usage_skills_remove").into_owned();
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
        self.state.status = t!("status.refreshing_profile_skills").into_owned();
        Some(AppUiCommand::ProfileSkillsList(ProfileSkillsListParams {
            profile_id: self.current_profile_for_onboarding(),
        }))
    }

    fn profile_skills_registry_search_command(&mut self, query: String) -> Option<AppUiCommand> {
        if !self.require_appui_method(crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH) {
            return None;
        }
        self.state.status = t!("status.searching_skill_registry", query = query).into_owned();
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
            self.state.status = t!("status.readonly_skills_install").into_owned();
            return None;
        }
        self.state.status = t!("status.installing_profile_skill", repo = repo).into_owned();
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
            self.state.status = t!("status.readonly_skills_remove").into_owned();
            return None;
        }
        self.state.status = t!("status.removing_profile_skill", name = name).into_owned();
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
        self.state.status = t!("status.refreshing_mcp_config").into_owned();
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
            self.state.status = t!("status.mcp_status_requires_session").into_owned();
            return None;
        };
        self.state.status = t!("status.refreshing_mcp_status").into_owned();
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
        let usage = t!("status.usage_mcp_enable").into_owned();
        let Some(server) = parse_single_name(rest, &usage) else {
            self.state.status = usage;
            return None;
        };
        self.state.status = if enabled {
            t!("status.enabling_mcp_config", server = server).into_owned()
        } else {
            t!("status.disabling_mcp_config", server = server).into_owned()
        };
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
        let usage = t!("status.usage_mcp_test").into_owned();
        let Some(server) = parse_single_name(rest, &usage) else {
            self.state.status = usage;
            return None;
        };
        self.state.status = t!("status.testing_mcp_config", server = server).into_owned();
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
        let usage = t!("status.usage_mcp_delete").into_owned();
        let Some(server) = parse_single_name(rest, &usage) else {
            self.state.status = usage;
            return None;
        };
        self.state.status = t!("status.deleting_mcp_config", server = server).into_owned();
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
        self.state.status = t!("status.upserting_mcp_config", server = server).into_owned();
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
        self.state.status = t!("status.refreshing_tool_config").into_owned();
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
            self.state.status = t!("status.tool_status_requires_session").into_owned();
            return None;
        };
        self.state.status = t!("status.refreshing_tool_status").into_owned();
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
        let usage = t!("status.usage_tools_enable").into_owned();
        let Some(tool) = parse_single_name(rest, &usage) else {
            self.state.status = usage;
            return None;
        };
        self.state.status = if enabled {
            t!("status.enabling_tool_config", tool = tool).into_owned()
        } else {
            t!("status.disabling_tool_config", tool = tool).into_owned()
        };
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
        let usage = t!("status.usage_tools_test").into_owned();
        let Some(tool) = parse_single_name(rest, &usage) else {
            self.state.status = usage;
            return None;
        };
        self.state.status = t!("status.testing_tool_config", tool = tool).into_owned();
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
        let usage = t!("status.usage_tools_delete").into_owned();
        let Some(tool) = parse_single_name(rest, &usage) else {
            self.state.status = usage;
            return None;
        };
        self.state.status = t!("status.deleting_tool_config", tool = tool).into_owned();
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
        self.state.status = t!("status.upserting_tool_config", tool = tool).into_owned();
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
                self.state.onboarding.clear_local_profile_recovery();
                self.state.onboarding.last_message = Some(t!("status.name_updated").into_owned());
                self.state.status = t!("status.onboarding_name_updated").into_owned();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetUsername(username) => {
                self.state.onboarding.username = username.trim().to_owned();
                self.state.onboarding.local_profile_created = false;
                self.state.onboarding.profile_id = None;
                self.state.onboarding.clear_local_profile_recovery();
                self.state.onboarding.last_message =
                    Some(t!("status.username_updated").into_owned());
                self.state.status = t!("status.onboarding_username_updated").into_owned();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetEmail(email) => {
                self.state.onboarding.email = email.trim().to_owned();
                self.state.onboarding.local_profile_created = false;
                self.state.onboarding.auth_code_sent = false;
                self.state.onboarding.auth_verified = false;
                self.state.onboarding.clear_local_profile_recovery();
                self.state.onboarding.last_message = Some(t!("status.email_updated").into_owned());
                self.state.status = t!("status.onboarding_email_updated").into_owned();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetOtpCode(code) => {
                self.state.onboarding.otp_code = code.trim().to_owned();
                self.state.onboarding.last_message =
                    Some(t!("status.otp_code_updated").into_owned());
                self.state.status = t!("status.onboarding_otp_code_updated").into_owned();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetProfileId(profile_id) => {
                self.state.onboarding.profile_id = non_empty_string(profile_id);
                self.state.onboarding.last_message =
                    Some(t!("status.profile_updated").into_owned());
                self.state.status = t!("status.onboarding_profile_updated").into_owned();
                self.refresh_active_menu_and_advance();
                None
            }
            OnboardingAction::SetProviderSelection(selection) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.apply_selection(selection);
                self.state.status = t!("status.provider_route_selected").into_owned();
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
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_family_updated").into_owned(),
                );
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
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_model_updated").into_owned(),
                );
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
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_route_updated").into_owned(),
                )
            }
            OnboardingAction::SetRouteLabel(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.label = non_empty_string(value);
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_route_label_updated").into_owned(),
                )
            }
            OnboardingAction::SetBaseUrl(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.base_url = non_empty_string(value);
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_base_url_updated").into_owned(),
                )
            }
            OnboardingAction::SetApiKeyEnv(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.api_key_env = non_empty_string(value);
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_api_key_env_updated").into_owned(),
                )
            }
            OnboardingAction::SetApiType(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.provider.route.api_type = non_empty_string(value);
                self.mark_onboarding_provider_dirty(
                    t!("status.provider_api_type_updated").into_owned(),
                )
            }
            OnboardingAction::SetApiKey(value) => {
                if self.block_onboarding_provider_edit_if_pending() {
                    return None;
                }
                self.state.onboarding.api_key = Some(value);
                self.state.onboarding.provider_tested = false;
                self.state.onboarding.provider_pending = None;
                self.state.onboarding.provider_save_target = None;
                // M22-E: a new key invalidates the prior test
                // failure — the user is implicitly retrying.
                self.state.onboarding.provider_test_failure_reason = None;
                self.state.onboarding.last_message =
                    Some(t!("status.api_key_updated").into_owned());
                self.state.status = t!("status.onboarding_api_key_updated").into_owned();
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
                self.state.onboarding.provider_test_failure_reason = None;
                self.state.onboarding.last_message =
                    Some(t!("status.api_key_cleared").into_owned());
                self.state.status = t!("status.onboarding_api_key_cleared").into_owned();
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
            OnboardingAction::SetWorkspace(path) => {
                let path = path.trim().to_owned();
                if path.is_empty() {
                    self.state.onboarding.workspace_candidate = None;
                    self.state.status = t!("status.workspace_candidate_cleared").into_owned();
                } else {
                    self.state.onboarding.workspace_candidate = Some(path);
                    self.state.status = t!("status.workspace_candidate_staged").into_owned();
                }
                self.state.onboarding.workspace_validation =
                    crate::model::OnboardingWorkspaceValidation::Unvalidated;
                self.refresh_active_menu_if_open();
                None
            }
            OnboardingAction::ValidateWorkspace => {
                self.onboarding_validate_workspace();
                None
            }
            OnboardingAction::ResetWorkspace => {
                self.state.onboarding.workspace_candidate = None;
                self.state.onboarding.workspace_validation =
                    crate::model::OnboardingWorkspaceValidation::Unvalidated;
                self.state.status = t!("status.workspace_selection_reset").into_owned();
                self.refresh_active_menu_if_open();
                None
            }
            OnboardingAction::StagePermissionProfile(update) => {
                self.state.onboarding.staged_permission_profile = update.clone();
                self.state.onboarding.permission_profile_mismatch = None;
                self.state.status = match update {
                    Some(update) => {
                        let unchanged_mode = t!("status.unchanged_mode").into_owned();
                        let unchanged_approval = t!("status.unchanged_approval").into_owned();
                        let unchanged_network = t!("status.unchanged_network").into_owned();
                        let mode = update
                            .mode
                            .map(|m| m.label().to_string())
                            .unwrap_or(unchanged_mode);
                        let approval = update.approval_policy.clone().unwrap_or(unchanged_approval);
                        let network = update
                            .network
                            .map(|n| n.label().to_string())
                            .unwrap_or(unchanged_network);
                        t!(
                            "status.permission_profile_staged",
                            mode = mode,
                            approval = approval,
                            network = network
                        )
                        .into_owned()
                    }
                    None => t!("status.permission_profile_staging_cleared").into_owned(),
                };
                self.refresh_active_menu_if_open();
                None
            }
            OnboardingAction::Doctor => {
                self.run_onboarding_doctor();
                None
            }
            OnboardingAction::Finish => self.onboarding_finish_command(),
            OnboardingAction::Reset => {
                self.state.onboarding = Default::default();
                self.state.status = t!("status.onboarding_wizard_reset").into_owned();
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
            "workspace" | "cwd" | "dir" => self
                .dispatch_onboarding_action(OnboardingAction::SetWorkspace(rest.to_owned()), None),
            "workspace-validate" | "validate-workspace" => {
                self.dispatch_onboarding_action(OnboardingAction::ValidateWorkspace, None)
            }
            "workspace-reset" | "reset-workspace" => {
                self.dispatch_onboarding_action(OnboardingAction::ResetWorkspace, None)
            }
            "permissions" | "permission" => {
                let update = match parse_onboarding_permission_mode(rest) {
                    Ok(update) => update,
                    Err(reason) => {
                        self.state.status = reason;
                        self.refresh_active_menu_if_open();
                        return None;
                    }
                };
                self.dispatch_onboarding_action(
                    OnboardingAction::StagePermissionProfile(update),
                    None,
                )
            }
            "doctor" | "check" => self.dispatch_onboarding_action(OnboardingAction::Doctor, None),
            "finish" | "open-session" => {
                self.dispatch_onboarding_action(OnboardingAction::Finish, None)
            }
            "reset" => self.dispatch_onboarding_action(OnboardingAction::Reset, None),
            _ => {
                self.state.status = onboarding_usage();
                self.push_local_activity(
                    ActivityKind::Warning,
                    "onboarding",
                    t!("status.unknown_onboarding_command").into_owned(),
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
                    t!("status.unknown_login_command").into_owned(),
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
                    t!("status.unknown_provider_command").into_owned(),
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
            self.state.status = t!("status.usage_onboard_select").into_owned();
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

    fn mark_onboarding_provider_dirty(
        &mut self,
        message: impl Into<String>,
    ) -> Option<AppUiCommand> {
        self.state.onboarding.provider_tested = false;
        self.state.onboarding.provider_pending = None;
        self.state.onboarding.provider_save_target = None;
        // M22-E: any staged-input edit invalidates the last test
        // failure — the reason was tied to the old selection/key.
        self.state.onboarding.provider_test_failure_reason = None;
        let message = message.into();
        self.state.onboarding.last_message = Some(message.clone());
        self.state.status = message;
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
            self.state.status = t!("status.local_onboarding_otp_send_hidden").into_owned();
            return None;
        }
        if !self.require_appui_method(crate::model::APPUI_METHOD_AUTH_SEND_CODE) {
            return None;
        }
        if !self.state.onboarding.has_email() {
            self.state.status = t!("status.onboarding_email_empty").into_owned();
            return None;
        }
        self.state.onboarding.last_message = Some(t!("status.sending_otp_code").into_owned());
        Some(AppUiCommand::AuthSendCode(AuthSendCodeParams {
            email: self.state.onboarding.email.clone(),
        }))
    }

    fn onboarding_verify_code_command(&mut self) -> Option<AppUiCommand> {
        if self.local_profile_create_supported() {
            self.state.status = t!("status.local_onboarding_otp_verify_hidden").into_owned();
            return None;
        }
        if !self.require_appui_method(crate::model::APPUI_METHOD_AUTH_VERIFY) {
            return None;
        }
        if !self.state.onboarding.has_email() || !self.state.onboarding.has_otp_code() {
            self.state.status = t!("status.onboarding_email_or_otp_empty").into_owned();
            return None;
        }
        self.state.onboarding.last_message = Some(t!("status.verifying_otp_code").into_owned());
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
        // M22-B: block overlapping local-profile creates. If a
        // create is already in flight, do NOT overwrite the
        // pending-username snapshot or fire another RPC — the late
        // response from the first request would otherwise be
        // attributed to the new snapshot, blaming the wrong
        // username, and the backend would receive a duplicate.
        if self.state.onboarding.local_profile_create_pending {
            self.state.status = t!("status.local_profile_create_in_progress").into_owned();
            return None;
        }
        // M22-B: client-side pre-flight validation catches obvious
        // bad fields before a backend round-trip; surfaces typed
        // recovery instead of a generic "incomplete" status.
        if let Err(recovery) = self.state.onboarding.validate_local_profile() {
            self.state.status = recovery.message.clone();
            self.state.onboarding.last_message = Some(recovery.message.clone());
            let focus_field = recovery.focus_field;
            self.state.onboarding.local_profile_recovery = Some(recovery);
            self.refresh_active_menu_if_open();
            // M22-B: drop the keyboard user onto the offending row so
            // they can edit it immediately — applies to both pre-flight
            // validation and server-side typed errors.
            self.focus_local_profile_field(focus_field);
            return None;
        }
        self.state.onboarding.open_session_after_profile_create = open_session_after_create;
        self.state.onboarding.local_profile_create_pending = true;
        self.state.onboarding.local_profile_create_pending_username =
            Some(self.state.onboarding.username.clone());
        self.state.onboarding.local_profile_recovery = None;
        self.state.onboarding.last_message = Some(t!("status.creating_local_profile").into_owned());
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
            self.state.status = t!("status.onboarding_provider_route_incomplete").into_owned();
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
            self.state.status = t!("status.onboarding_provider_selection_incomplete").into_owned();
            return None;
        };
        if !self.state.onboarding.has_api_key() {
            self.state.status = t!("status.onboarding_api_key_empty_onboard").into_owned();
            return None;
        }
        self.state.onboarding.last_message = Some(t!("status.saving_provider").into_owned());
        self.state.onboarding.provider_pending = Some(OnboardingProviderPending::Save);
        self.state.onboarding.provider_save_target = Some(OnboardingProviderSaveTarget::Primary);
        self.state.status = t!("status.saving_provider_config").into_owned();
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
            self.state.status = t!("status.onboarding_fallback_selection_incomplete").into_owned();
            return None;
        };
        if !self.state.onboarding.has_api_key() {
            self.state.status = t!("status.onboarding_api_key_empty_provider").into_owned();
            return None;
        }
        self.state.onboarding.last_message =
            Some(t!("status.saving_fallback_provider").into_owned());
        self.state.onboarding.provider_pending = Some(OnboardingProviderPending::Save);
        self.state.onboarding.provider_save_target = Some(OnboardingProviderSaveTarget::Fallback);
        self.state.status = t!("status.saving_fallback_provider_config").into_owned();
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
            self.state.status = t!("status.onboarding_provider_selection_incomplete").into_owned();
            return None;
        };
        if !self.state.onboarding.has_api_key() {
            self.state.status = t!("status.onboarding_api_key_empty_onboard").into_owned();
            return None;
        }
        self.state.onboarding.last_message = Some(t!("status.testing_provider").into_owned());
        self.state.onboarding.provider_pending = Some(OnboardingProviderPending::Test);
        self.state.status = t!("status.testing_provider_connection").into_owned();
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
            self.state.status = t!("status.cannot_open_profile_unresolved").into_owned();
            return None;
        };
        if let Some(reason) = self.open_session_provider_block_reason(&profile_id) {
            self.state.status = reason;
            self.refresh_active_menu_if_open();
            return None;
        }
        // M22-C: refuse `session/open` unless the workspace is
        // validated. If the user has not yet pressed
        // `/onboard workspace-validate`, kick off the probe here so
        // pressing finish is enough — otherwise the user would have
        // to type two commands for the happy path.
        if self.state.onboarding.workspace_validation.is_unvalidated() {
            self.onboarding_validate_workspace();
        }
        if !self.state.onboarding.workspace_ready_for_finish() {
            let reason = match &self.state.onboarding.workspace_validation {
                crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
                    t!("status.cannot_open_workspace_invalid", reason = reason).into_owned()
                }
                crate::model::OnboardingWorkspaceValidation::Validating => {
                    t!("status.workspace_validation_in_progress").into_owned()
                }
                _ => t!("status.cannot_open_workspace_not_validated").into_owned(),
            };
            self.state.status = reason;
            self.refresh_active_menu_if_open();
            return None;
        }
        // M22-C: promote the validated CANONICAL path (not the raw
        // candidate) so `session/open` sends exactly what the probe
        // verified. A user typing `/onboard workspace .` would
        // otherwise have the raw "." reach the server even though
        // the probe canonicalised it — breaking the validation
        // boundary.
        if let crate::model::OnboardingWorkspaceValidation::Valid { canonical, .. } =
            &self.state.onboarding.workspace_validation
        {
            self.state.workspace.root = canonical.clone();
        }
        let session_id =
            octos_core::SessionKey::with_profile_topic(&profile_id, "local", "tui", "coding");
        self.state.status = t!("status.opening_coding_session", profile = profile_id).into_owned();
        Some(AppUiCommand::OpenSession(SessionOpenParams {
            session_id,
            topic: None,
            profile_id: Some(profile_id),
            cwd: onboarding_workspace_cwd(&self.state.workspace.root),
            after: None,
        }))
    }

    /// M22-C: true when the Octos UI target is a non-local network
    /// transport (e.g. `wss://remote.example/...`). Local stdio
    /// and `ws://localhost` are treated as same-host, so the
    /// filesystem probe is meaningful.
    fn is_remote_transport_target(&self) -> bool {
        let target = match self.state.target.as_deref() {
            Some(value) => value,
            None => return false,
        };
        if target.starts_with("stdio:") || target == "stdio" {
            return false;
        }
        if let Some(rest) = target
            .strip_prefix("ws://")
            .or_else(|| target.strip_prefix("wss://"))
        {
            // host:port/... — extract the host part.
            let host = rest.split([':', '/']).next().unwrap_or("");
            return !matches!(host, "" | "localhost" | "127.0.0.1" | "::1" | "[::1]");
        }
        false
    }

    /// M22-C: workspace probe used when the TUI is transport-local
    /// (stdio or local `ws://localhost`). Remote-only transports
    /// put the workspace on the SERVER host, so the client cannot
    /// stat it; in that case the probe falls back to a shape-only
    /// `Valid` status and trusts the server to validate on
    /// `session/open`. When the backend gains a `workspace/probe`
    /// RPC (out of scope per slice-0), the caller can swap this
    /// for an outbound command and keep the same
    /// `OnboardingWorkspaceValidation` consumer.
    fn onboarding_validate_workspace(&mut self) {
        let active = self.state.workspace.root.clone();
        let raw_target = self.state.onboarding.workspace_target(&active).to_owned();
        // M22-C: a stdio launch label like
        // `stdio:octos serve --stdio --cwd /tmp/project` carries
        // the cwd inside the command string. Run the existing
        // extractor first so the user does not have to retype.
        let target = onboarding_workspace_cwd(&raw_target).unwrap_or_else(|| raw_target.clone());
        if target.is_empty()
            || target == "unknown"
            || target == "not supplied"
            || target == "stdio"
            || target.starts_with("ws://")
            || target.starts_with("wss://")
        {
            self.state.onboarding.workspace_validation =
                crate::model::OnboardingWorkspaceValidation::Invalid {
                    reason: t!("status.no_usable_workspace_cwd", target = raw_target).into_owned(),
                };
            self.state.status = t!("status.workspace_cwd_invalid").into_owned();
            self.refresh_active_menu_if_open();
            return;
        }
        // M22-C: remote-only transports (non-local WebSocket) put
        // the workspace on the SERVER host. Skip the client
        // filesystem probe and trust that the server will validate
        // on `session/open` — but still record a typed `Valid`
        // status with the staged path so finish is unblocked and
        // the user can see what cwd will be sent.
        if self.is_remote_transport_target() {
            self.state.onboarding.workspace_validation =
                crate::model::OnboardingWorkspaceValidation::Valid {
                    canonical: target.clone(),
                    writable: true,
                    has_workspace_toml: false,
                };
            self.state.status = t!("status.workspace_staged_remote", target = target).into_owned();
            self.refresh_active_menu_if_open();
            return;
        }
        let path = std::path::PathBuf::from(&target);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                self.state.onboarding.workspace_validation =
                    crate::model::OnboardingWorkspaceValidation::Invalid {
                        reason: t!("status.path_not_accessible", target = target, err = err)
                            .into_owned(),
                    };
                self.state.status =
                    t!("status.workspace_not_accessible", target = target).into_owned();
                self.refresh_active_menu_if_open();
                return;
            }
        };
        if !metadata.is_dir() {
            self.state.onboarding.workspace_validation =
                crate::model::OnboardingWorkspaceValidation::Invalid {
                    reason: t!("status.path_not_directory", target = target).into_owned(),
                };
            self.state.status = t!("status.workspace_not_directory", target = target).into_owned();
            self.refresh_active_menu_if_open();
            return;
        }
        let canonical = std::fs::canonicalize(&path)
            .map(|canonical| canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| target.clone());
        // Reject obvious root-escape attempts: a workspace MUST NOT
        // be `/`, the user's home root, or contain `..` after
        // canonicalisation. The backend will re-validate but the
        // TUI should reject the worst cases up front.
        if canonical == "/" || canonical.is_empty() {
            self.state.onboarding.workspace_validation =
                crate::model::OnboardingWorkspaceValidation::Invalid {
                    reason: t!("status.workspace_cannot_be_root").into_owned(),
                };
            self.state.status = t!("status.workspace_cannot_be_root_status").into_owned();
            self.refresh_active_menu_if_open();
            return;
        }
        let writable = !metadata.permissions().readonly();
        let has_workspace_toml = path.join(".octos-workspace.toml").is_file();
        self.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: canonical.clone(),
                writable,
                has_workspace_toml,
            };
        let writable_label = if writable {
            t!("status.workspace_writable").into_owned()
        } else {
            t!("status.workspace_read_only").into_owned()
        };
        let toml_label = if has_workspace_toml {
            t!("status.workspace_has_toml").into_owned()
        } else {
            String::new()
        };
        self.state.status = t!(
            "status.workspace_ok",
            canonical = canonical,
            writable = writable_label,
            toml = toml_label
        )
        .into_owned();
        self.refresh_active_menu_if_open();
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
        Some(t!("status.cannot_open_save_primary_first").into_owned())
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

    /// True when any onboarding wizard menu (welcome / provider setup / its
    /// family/model/route children) is the active surface. Used to decide
    /// whether opening a session should tear the wizard down (issue #4).
    fn active_menu_is_onboarding(&self) -> bool {
        self.state.menu_stack.active().is_some_and(|frame| {
            matches!(
                frame.id.as_str(),
                crate::menu::registry::MENU_ONBOARD
                    | crate::menu::registry::MENU_ONBOARD_FAMILY
                    | crate::menu::registry::MENU_ONBOARD_MODEL
                    | crate::menu::registry::MENU_ONBOARD_ROUTE
                    // UX2 B.2: Activate now lives on the workspace step screen,
                    // so pressing it there must also tear the wizard down (drop
                    // the user into the coding surface), not leave the workspace
                    // menu stacked over the chat.
                    | crate::menu::registry::MENU_ONBOARD_WORKSPACE
            )
        })
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
        self.state.status = t!("status.appui_method_not_advertised", method = method).into_owned();
        false
    }

    fn require_appui_feature(&mut self, feature: &'static str) -> bool {
        if self
            .state
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.supports_feature(feature))
        {
            return true;
        }
        self.state.status =
            t!("status.appui_feature_not_advertised", feature = feature).into_owned();
        false
    }

    fn require_mutating_appui_method(&mut self, method: &'static str) -> bool {
        if self.state.readonly {
            self.state.status = t!("status.readonly_method_disabled", method = method).into_owned();
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
                self.state.status = t!("status.menu_label", id = frame.id.to_string()).into_owned();
            }
            return true;
        }
        false
    }

    /// True while the first-launch onboarding wizard is still in progress: the
    /// wizard auto-opens only when there is no session yet, and finishing it
    /// opens a profile-scoped session. So "no session" == "onboarding not yet
    /// complete" for the purpose of the Esc trap below.
    fn onboarding_in_progress(&self) -> bool {
        self.state.sessions.is_empty()
    }

    /// Handle Esc on the active menu. Mirrors `close_menu` for every menu EXCEPT
    /// the *root* onboarding step (`MENU_ONBOARD`) while onboarding is still in
    /// progress: that wizard is only ever auto-opened on first launch (issue #5),
    /// so closing it would strand the user with no way back. Esc on a child
    /// onboarding step (family/model/route/workspace) still pops back to the
    /// parent wizard step — that is just `close_menu`. Returns true when a menu
    /// was actually closed.
    pub fn handle_menu_escape(&mut self) -> bool {
        if self.onboarding_in_progress()
            && self.active_menu_id_is(crate::menu::registry::MENU_ONBOARD)
        {
            // No-op: keep the root onboarding wizard open.
            return false;
        }
        self.close_menu()
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

    /// M22-F: produce the onboarding doctor report. Each check is
    /// a typed projection of existing wizard / app state, so the
    /// doctor surface itself is read-only — recovery is delegated
    /// back to the existing `/onboard <step>` actions via the
    /// per-check `recovery` strings.
    pub fn onboarding_doctor_report(&self) -> crate::model::OnboardingDoctorReport {
        use crate::model::{
            OnboardingDoctorCheck, OnboardingDoctorOutcome, OnboardingDoctorReport,
        };
        let onboarding = &self.state.onboarding;
        let capabilities = self.state.capabilities.as_ref();
        let supports = |method: &str| -> bool {
            capabilities.is_some_and(|caps| caps.supports_method(method))
        };
        let local_create_supported = supports(crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE);

        // M22-F: use the same resolved-profile source as
        // `onboarding_finish_command` so the doctor recognises a
        // profile that the server has already published (active
        // session, runtime status, profile_llm_state, …) even
        // when the local `onboarding.profile_id` is still blank.
        let resolved_profile = self.current_profile_for_onboarding();
        let profile_check = if onboarding.local_profile_created || resolved_profile.is_some() {
            let label = resolved_profile
                .clone()
                .or_else(|| onboarding.profile_id.clone())
                .unwrap_or_else(|| onboarding.username.clone());
            OnboardingDoctorOutcome::Pass {
                detail: t!("status.doctor_profile_id", label = label).into_owned(),
            }
        } else if local_create_supported {
            OnboardingDoctorOutcome::Fail {
                reason: t!("status.doctor_no_local_profile").into_owned(),
                recovery: t!("status.doctor_no_local_profile_recovery").into_owned(),
            }
        } else {
            OnboardingDoctorOutcome::Skipped {
                detail: t!("status.doctor_local_create_unadvertised").into_owned(),
            }
        };

        // M22-F: accept a server-published primary provider — the
        // `/onboard finish` open-session path already trusts
        // `profile_llm_state.primary_provider().has_api_key`, so
        // the doctor must too. Falls back to the local wizard
        // checks (selection + key staged, etc.) when the server
        // has not yet published a primary.
        let published_primary = self
            .state
            .profile_llm_state
            .as_ref()
            .and_then(|llm| llm.primary_provider())
            .filter(|provider| provider.has_api_key);
        let provider_check = if let Some(provider) = published_primary {
            OnboardingDoctorOutcome::Pass {
                detail: t!(
                    "status.doctor_server_primary",
                    family = provider
                        .family_id
                        .clone()
                        .unwrap_or_else(|| provider.provider.clone()),
                    model = provider
                        .model_id
                        .clone()
                        .unwrap_or_else(|| provider.model.clone())
                )
                .into_owned(),
            }
        } else if onboarding.provider_saved {
            OnboardingDoctorOutcome::Pass {
                detail: t!(
                    "status.doctor_saved_primary",
                    label = onboarding
                        .saved_primary_provider_label
                        .clone()
                        .unwrap_or_else(|| onboarding.provider_label())
                )
                .into_owned(),
            }
        } else if onboarding.selection_ready() && onboarding.has_api_key() {
            OnboardingDoctorOutcome::Warn {
                reason: t!("status.doctor_provider_unsaved").into_owned(),
                recovery: t!("status.doctor_provider_unsaved_recovery").into_owned(),
            }
        } else if onboarding.selection_ready() {
            OnboardingDoctorOutcome::Warn {
                reason: t!("status.doctor_provider_no_key").into_owned(),
                recovery: t!("status.doctor_provider_no_key_recovery").into_owned(),
            }
        } else {
            OnboardingDoctorOutcome::Fail {
                reason: t!("status.doctor_no_provider").into_owned(),
                recovery: t!("status.doctor_no_provider_recovery").into_owned(),
            }
        };

        // Workspace check.
        let workspace_check = if onboarding_workspace_cwd(&self.state.workspace.root).is_some() {
            OnboardingDoctorOutcome::Pass {
                detail: t!(
                    "status.doctor_workspace_resolvable",
                    root = self.state.workspace.root
                )
                .into_owned(),
            }
        } else {
            OnboardingDoctorOutcome::Fail {
                reason: t!(
                    "status.doctor_workspace_unusable",
                    root = self.state.workspace.root
                )
                .into_owned(),
                recovery: t!("status.doctor_workspace_unusable_recovery").into_owned(),
            }
        };

        // Capability check.
        let capability_check = if let Some(caps) = capabilities {
            // Probe known onboarding-relevant methods to summarize.
            let known = [
                crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
                crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
                crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
                crate::model::APPUI_METHOD_PROFILE_LLM_TEST,
                crate::model::APPUI_METHOD_MODEL_LIST,
                crate::model::APPUI_METHOD_SESSION_STATUS_READ,
                crate::menu::registry::APPUI_METHOD_PERMISSION_PROFILE_SET,
            ];
            let advertised = known
                .iter()
                .filter(|method| caps.supports_method(method))
                .count();
            OnboardingDoctorOutcome::Pass {
                detail: t!(
                    "status.doctor_methods_advertised",
                    advertised = advertised,
                    total = known.len()
                )
                .into_owned(),
            }
        } else {
            OnboardingDoctorOutcome::Fail {
                reason: t!("status.doctor_caps_not_received").into_owned(),
                recovery: t!("status.doctor_caps_not_received_recovery").into_owned(),
            }
        };

        // Transport check.
        let transport_check = match self.state.target.as_deref() {
            Some(target) if !target.is_empty() => OnboardingDoctorOutcome::Pass {
                detail: t!("status.doctor_appui_target", target = target).into_owned(),
            },
            _ => OnboardingDoctorOutcome::Fail {
                reason: t!("status.doctor_no_transport").into_owned(),
                recovery: t!("status.doctor_no_transport_recovery").into_owned(),
            },
        };

        OnboardingDoctorReport {
            checks: vec![
                OnboardingDoctorCheck {
                    id: "transport",
                    title: "Octos UI transport",
                    outcome: transport_check,
                },
                OnboardingDoctorCheck {
                    id: "capabilities",
                    title: "Server capabilities",
                    outcome: capability_check,
                },
                OnboardingDoctorCheck {
                    id: "profile",
                    title: "Local profile",
                    outcome: profile_check,
                },
                OnboardingDoctorCheck {
                    id: "workspace",
                    title: "Workspace cwd",
                    outcome: workspace_check,
                },
                OnboardingDoctorCheck {
                    id: "provider",
                    title: "LLM provider",
                    outcome: provider_check,
                },
            ],
        }
    }

    fn run_onboarding_doctor(&mut self) {
        let report = self.onboarding_doctor_report();
        let summary_line = report
            .checks
            .iter()
            .map(|check| format!("{}: {}", check.id, check.outcome.label()))
            .collect::<Vec<_>>()
            .join(" · ");
        self.state.status =
            t!("status.onboarding_doctor_summary", summary = summary_line).into_owned();
        for check in &report.checks {
            let detail = match &check.outcome {
                crate::model::OnboardingDoctorOutcome::Pass { detail }
                | crate::model::OnboardingDoctorOutcome::Skipped { detail } => detail.clone(),
                crate::model::OnboardingDoctorOutcome::Warn { reason, recovery } => {
                    format!("{reason} → {recovery}")
                }
                crate::model::OnboardingDoctorOutcome::Fail { reason, recovery } => {
                    format!("{reason} → {recovery}")
                }
            };
            let kind = match check.outcome {
                crate::model::OnboardingDoctorOutcome::Pass { .. }
                | crate::model::OnboardingDoctorOutcome::Skipped { .. } => ActivityKind::Progress,
                crate::model::OnboardingDoctorOutcome::Warn { .. } => ActivityKind::Warning,
                crate::model::OnboardingDoctorOutcome::Fail { .. } => ActivityKind::Error,
            };
            self.state.push_activity(
                ActivityItem::new(kind, check.id, check.outcome.label()).with_detail(detail),
            );
        }
        self.refresh_active_menu_if_open();
    }

    fn focus_provider_api_key_row(&mut self) -> bool {
        self.select_active_menu_item_by_id("onboard.provider.key")
            || self.select_active_menu_item_by_id("provider.key")
    }

    /// M22-B: focus the local-profile field row identified by the
    /// recovery state so the user is dropped on the offending field
    /// after a typed `profile/local/create` error or pre-flight
    /// validation rejection.
    fn focus_local_profile_field(
        &mut self,
        field: crate::model::OnboardingLocalProfileField,
    ) -> bool {
        let row_id = match field {
            crate::model::OnboardingLocalProfileField::Name => "onboard.local.name",
            crate::model::OnboardingLocalProfileField::Username => "onboard.local.username",
            crate::model::OnboardingLocalProfileField::Email => "onboard.local.email",
        };
        self.select_active_menu_item_by_id(row_id)
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
                    self.state.status =
                        t!("status.menu_label", id = frame.id.to_string()).into_owned();
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
                self.start_prompt_turn(prompt, t!("status.queued_menu_prompt").into_owned())
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
            theme_name: Some(self.state.theme.as_str()),
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
            reasoning_effort: selected_session.and_then(|session| {
                self.state
                    .session_reasoning_effort
                    .get(&session.id)
                    .copied()
            }),
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
            .map(|(_, turn_id)| {
                t!(
                    "status.ps_active_turn",
                    id = short_id(&turn_id.0.to_string())
                )
                .into_owned()
            })
            .unwrap_or_else(|| t!("status.ps_idle").into_owned());
        let selected_task = self
            .state
            .active_task()
            .map(|task| {
                t!(
                    "status.ps_selected_task",
                    title = task.title,
                    state = task_state_label(task.state)
                )
                .into_owned()
            })
            .unwrap_or_else(|| t!("status.ps_selected_task_none").into_owned());
        let staged = self.state.pending_messages.len();
        let status = t!(
            "status.ps_summary",
            turn = turn,
            total = counts.total,
            running = counts.running,
            pending = counts.pending,
            done = counts.done,
            failed = counts.failed,
            staged = staged
        )
        .into_owned();
        let detail = t!(
            "status.ps_detail",
            state = self.state.run_state.label(),
            selected_task = selected_task,
            activity = self.state.activity.len()
        )
        .into_owned();

        self.state.focus = FocusPane::Tasks;
        self.state.status = status.clone();
        self.state.scroll_transcript_to_latest();
        self.push_local_activity(
            ActivityKind::Progress,
            t!("status.local_ps").into_owned(),
            status,
            Some(detail),
        );
    }

    fn show_unknown_slash_command(&mut self, command: &str, draft: &str) {
        let ctx = self.state.availability_context();
        let status = t!(
            "status.unknown_slash_command",
            command = command,
            hint = slash_command_try_hint(&ctx)
        )
        .into_owned();
        self.state.status = status.clone();
        self.push_local_activity(
            ActivityKind::Warning,
            t!("status.local_slash_command").into_owned(),
            status,
            Some(t!("status.ignored_input", draft = draft).into_owned()),
        );
    }

    fn show_unavailable_slash_command(&mut self, command: &str, reason: &str) {
        let status = t!(
            "status.command_unavailable_reason",
            command = command,
            reason = reason
        )
        .into_owned();
        self.state.status = status.clone();
        self.push_local_activity(
            ActivityKind::Warning,
            t!("status.local_slash_command").into_owned(),
            status,
            Some(t!("status.command_gate_failed").into_owned()),
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
        let reasoning_effort = self
            .state
            .session_reasoning_effort
            .get(&session_id)
            .copied();
        Some(AppUiCommand::SubmitPrompt(TurnStartParams {
            session_id,
            turn_id,
            input: vec![InputItem::Text { text: prompt }],
            media: Vec::new(),
            topic: None,
            rewrite_for: None,
            reasoning_effort,
        }))
    }

    fn review_start_command(&mut self, inline_args: &str) -> Option<AppUiCommand> {
        if self.state.active_turn().is_some() {
            self.state.status = t!("status.cannot_start_review_active_turn").into_owned();
            return None;
        }
        if !self.require_appui_feature(crate::model::APPUI_FEATURE_REVIEW_START_V1) {
            return None;
        }
        if !self.require_mutating_appui_method(crate::model::APPUI_METHOD_REVIEW_START) {
            return None;
        }
        let session = self.active_session()?;
        let session_id = session.id.clone();
        let profile_id = session.profile_id.clone();
        let prompt = inline_args.trim();
        let prompt = (!prompt.is_empty()).then(|| prompt.to_owned());
        let turn_id = TurnId::new();
        self.state.status = t!("status.starting_code_review").into_owned();
        self.state.set_run_state_in_progress();
        self.state.push_activity(
            ActivityItem::new(
                ActivityKind::Progress,
                t!("status.activity_code_review").into_owned(),
                t!("status.review_requested").into_owned(),
            )
            .with_turn(turn_id.clone()),
        );
        Some(AppUiCommand::StartReview(ReviewStartParams {
            session_id,
            profile_id,
            turn_id: Some(turn_id),
            target: None,
            prompt,
            instructions: None,
            delivery: Some("inline".into()),
        }))
    }

    pub fn interrupt_staged_command(&mut self) -> Option<AppUiCommand> {
        if !self.state.has_pending_messages() {
            self.state.status = t!("status.no_staged_message").into_owned();
            return None;
        }

        let command = self.interrupt_command();
        if command.is_some() {
            self.state.status = t!("status.interrupt_staged_submit").into_owned();
        }
        command
    }

    pub fn interrupt_command(&mut self) -> Option<AppUiCommand> {
        let Some((session_id, turn_id)) = self
            .state
            .active_turn()
            .map(|(session_id, turn_id)| (session_id.clone(), turn_id.clone()))
        else {
            self.state.status = t!("status.no_active_turn_interrupt").into_owned();
            return None;
        };

        self.state.status = t!("status.interrupt_requested_active_turn").into_owned();
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
            self.state.status = t!("status.no_active_approval").into_owned();
            return None;
        };

        self.state.status = t!(
            "status.approval_action",
            action = action.status_label(),
            title = approval.title
        )
        .into_owned();
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
            self.state.status = t!("status.cleared_staged_messages", count = cleared).into_owned();
            return;
        }

        if !self.state.composer.is_empty() {
            self.state.clear_current_composer_draft();
            self.state.status = t!("status.cleared_composer_draft").into_owned();
            return;
        }

        self.state.status = t!("status.nothing_to_clear").into_owned();
    }

    /// `/copy` (and the `Ctrl+Y` keybinding): stage the last assistant reply
    /// for a clipboard write. The store can't touch the terminal, so it parks
    /// the text on `state.pending_clipboard`; the event loop drains it and
    /// emits the OSC 52 escape sequence on the next tick. OSC 52 is used (vs a
    /// local clipboard crate) because the TUI commonly runs over SSH against
    /// the fleet minis — the copy must reach the *operator's* clipboard, not
    /// the remote host's.
    pub fn copy_last_reply(&mut self) {
        match crate::clipboard::copyable_assistant_text(&self.state) {
            Some(text) => {
                let chars = text.chars().count();
                self.state.pending_clipboard = Some(text);
                self.state.status = t!("status.copied_last_reply", chars = chars).into_owned();
            }
            None => {
                self.state.status = t!("status.nothing_to_copy").into_owned();
            }
        }
    }

    pub fn show_pending_approval(&mut self) -> bool {
        let title = {
            let Some(approval) = self.state.approval.as_mut() else {
                self.state.status = t!("status.no_pending_approval").into_owned();
                return false;
            };

            approval.visible = true;
            approval.title.clone()
        };

        self.state.approval_auto_open = true;
        self.state.focus = FocusPane::Composer;
        self.state.status = t!("status.approval_shown", title = title).into_owned();
        true
    }

    // ── UPCR-2026-023 AskUserQuestion picker driving ──────────────────
    // Mirrors the approval picker driving above: option toggle/navigation,
    // free-text capture, stepping through 1–4 questions, and submit.

    /// Move the highlighted row down within the active question.
    pub fn user_question_cursor_down(&mut self) {
        if let Some(entry) = self
            .state
            .user_question
            .as_mut()
            .and_then(UserQuestionPickerState::active_question_mut)
        {
            entry.move_cursor_down();
        }
    }

    /// Move the highlighted row up within the active question.
    pub fn user_question_cursor_up(&mut self) {
        if let Some(entry) = self
            .state
            .user_question
            .as_mut()
            .and_then(UserQuestionPickerState::active_question_mut)
        {
            entry.move_cursor_up();
        }
    }

    /// Toggle the highlighted option (or enter free-text editing on "Other").
    pub fn user_question_toggle(&mut self) {
        if let Some(entry) = self
            .state
            .user_question
            .as_mut()
            .and_then(UserQuestionPickerState::active_question_mut)
        {
            entry.toggle_cursor();
        }
    }

    /// Append a character into the active question's free-text "Other" box.
    pub fn user_question_push_free_text(&mut self, ch: char) {
        if let Some(entry) = self
            .state
            .user_question
            .as_mut()
            .and_then(UserQuestionPickerState::active_question_mut)
        {
            entry.editing_free_text = true;
            entry.cursor = entry.free_text_row();
            entry.free_text.push(ch);
        }
    }

    /// Delete the last character from the active question's free-text box.
    pub fn user_question_pop_free_text(&mut self) {
        if let Some(entry) = self
            .state
            .user_question
            .as_mut()
            .and_then(UserQuestionPickerState::active_question_mut)
        {
            entry.free_text.pop();
        }
    }

    /// True while the active question's "Other" box is capturing keystrokes.
    pub fn user_question_editing_free_text(&self) -> bool {
        self.state
            .user_question
            .as_ref()
            .and_then(UserQuestionPickerState::active_question)
            .is_some_and(|entry| entry.editing_free_text)
    }

    /// Step to the next question (Enter on a non-final question), or report that
    /// the picker is ready to submit when on the final question. Returns `true`
    /// when there are no more questions to step through (caller may submit).
    pub fn user_question_advance(&mut self) -> bool {
        let Some(picker) = self.state.user_question.as_mut() else {
            return false;
        };
        if let Some(entry) = picker.active_question_mut() {
            entry.editing_free_text = false;
        }
        if picker.is_last_question() {
            true
        } else {
            picker.focus_next_question();
            let active = picker.active + 1;
            let total = picker.questions.len();
            self.state.status =
                t!("status.question_progress", active = active, total = total).into_owned();
            false
        }
    }

    /// Step back to the previous question.
    pub fn user_question_back(&mut self) {
        if let Some(picker) = self.state.user_question.as_mut() {
            if let Some(entry) = picker.active_question_mut() {
                entry.editing_free_text = false;
            }
            picker.focus_prev_question();
        }
    }

    /// Build and consume the `user_question/respond` command for the pending
    /// picker, mirroring [`Self::respond_approval_command`]. Clears the picker
    /// and unblocks the run state.
    pub fn respond_user_question_command(&mut self) -> Option<AppUiCommand> {
        // A garbled/protocol-violation event with NO structured questions is not
        // submittable: any respond we form would either be rejected by the
        // backend validator (answers.len() must equal questions.len()) or carry
        // no answers at all. Refuse to submit WITHOUT consuming the picker so it
        // stays Esc-dismissible and recoverable (DO-NOT-SHIP #1/#2). The user
        // dismisses it via Esc; the turn is unblocked by the server-side
        // terminal, not a TUI respond.
        if self
            .state
            .user_question
            .as_ref()
            .is_some_and(|picker| picker.questions.is_empty())
        {
            self.state.status = t!("status.question_no_options").into_owned();
            return None;
        }

        let Some(picker) = self.state.user_question.take() else {
            self.state.status = t!("status.no_active_question").into_owned();
            return None;
        };

        let params = picker.to_respond_params();
        self.state.status = t!("status.answered_question", title = picker.title).into_owned();
        if self.state.active_turn().is_some() {
            self.state.set_run_state_in_progress();
        } else if self.state.run_state.is_active() {
            self.state.set_run_state_success();
        }
        self.state.user_question_auto_open = true;

        Some(AppUiCommand::RespondUserQuestion(params))
    }

    pub fn show_pending_user_question(&mut self) -> bool {
        let title = {
            let Some(picker) = self.state.user_question.as_mut() else {
                self.state.status = t!("status.no_pending_question").into_owned();
                return false;
            };
            picker.visible = true;
            picker.title.clone()
        };

        self.state.user_question_auto_open = true;
        self.state.focus = FocusPane::Composer;
        self.state.status = t!("status.question_shown", title = title).into_owned();
        true
    }

    pub fn read_task_output_command(&mut self) -> Option<AppUiCommand> {
        let Some(task) = self.state.active_task_context() else {
            self.state.status = t!("status.no_task_output_to_read").into_owned();
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
        self.state.status = t!("status.requested_task_output", title = task.title).into_owned();

        Some(AppUiCommand::ReadTaskOutput(TaskOutputReadParams {
            session_id: task.session_id,
            task_id: task.task_id,
            cursor,
            limit_bytes: Some(TASK_OUTPUT_READ_LIMIT_BYTES),
        }))
    }

    /// Cancel the currently selected background task. Gives the task dock a
    /// user affordance (`x`) that sends `task/cancel`. The task-control command
    /// surface is gated on `harness.task_control.v1`, which the TUI now
    /// negotiates (see the `X-Octos-Ui-Features` header in `transport.rs`).
    /// Only Pending/Running tasks are cancellable; the resulting terminal state
    /// arrives via the `task/updated` notification.
    pub fn cancel_task_command(&mut self) -> Option<AppUiCommand> {
        let Some(task) = self.state.active_task() else {
            self.state.status = t!("status.no_task_to_cancel").into_owned();
            return None;
        };
        let cancellable = matches!(
            task.state,
            TaskRuntimeState::Pending | TaskRuntimeState::Running
        );
        let task_id = task.id.clone();
        let title = task.title.clone();
        let state_label = task_state_label(task.state);
        if !cancellable {
            self.state.status = t!(
                "status.task_already_terminal",
                title = title,
                state = state_label
            )
            .into_owned();
            return None;
        }
        // octos#1380: only send task/cancel when the server actually advertises
        // task control. Capabilities arrive via the authoritative
        // config/capabilities/list response (negotiated through the
        // X-Octos-Ui-Features header); until that lands, or against a server
        // that doesn't advertise harness.task_control.v1, the method is
        // unsupported — so disable the affordance with a clear status rather
        // than send a doomed RPC. We deliberately do NOT trust the in-band
        // session/opened slice, which serde-defaults to the full first-server
        // surface when the field is absent and would re-enable cancel against a
        // non-negotiating server (codex P1).
        let task_control_supported = self
            .state
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| {
                capabilities.supports_method(octos_core::ui_protocol::methods::TASK_CANCEL)
            });
        if !task_control_supported {
            self.state.status = t!("status.task_control_unavailable").into_owned();
            return None;
        }
        let session_id = self
            .state
            .active_session()
            .map(|session| session.id.clone());
        self.state.status = t!("status.requested_cancel", title = title).into_owned();
        Some(AppUiCommand::CancelTask(
            octos_core::ui_protocol::TaskCancelParams {
                task_id,
                session_id,
                profile_id: None,
            },
        ))
    }

    pub fn read_diff_preview_command(&mut self) -> Option<AppUiCommand> {
        let Some(session_id) = self.active_session().map(|session| session.id.clone()) else {
            self.state.status = t!("status.no_session_for_diff").into_owned();
            return None;
        };
        let preview_id = self
            .state
            .approval
            .as_ref()
            .and_then(ApprovalModalState::diff_preview_id)
            .or_else(|| self.state.active_diff_preview_id());
        let Some(preview_id) = preview_id else {
            self.state.status = t!("status.no_diff_preview_id").into_owned();
            return None;
        };

        self.state.diff_preview.open_loading(preview_id.clone());
        self.state.status = t!("status.requested_diff_preview").into_owned();
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
            self.state.status = t!("status.approval_pane_hidden").into_owned();
            return true;
        }

        if let Some(picker) = self.state.user_question.as_mut()
            && picker.visible
        {
            picker.visible = false;
            self.state.user_question_auto_open = false;
            self.state.status = t!("status.question_pane_hidden").into_owned();
            return true;
        }

        if self.state.task_output.active {
            self.state.task_output.close();
            self.state.status = t!("status.closed_task_output").into_owned();
            return true;
        }

        if self.state.artifact_detail.active {
            self.state.artifact_detail.close();
            self.state.status = t!("status.closed_artifact_detail").into_owned();
            return true;
        }

        if self.state.thread_graph_detail.active {
            self.state.thread_graph_detail.close();
            self.state.status = t!("status.closed_thread_graph").into_owned();
            return true;
        }

        if self.state.turn_state_detail.active {
            self.state.turn_state_detail.close();
            self.state.status = t!("status.closed_turn_state").into_owned();
            return true;
        }

        if self.state.diff_preview.active {
            self.state.diff_preview.close();
            self.state.status = t!("status.closed_inline_diff").into_owned();
            return true;
        }

        false
    }

    pub fn show_diff_preview_placeholder(&mut self) {
        self.state.status = t!("status.diff_preview_unavailable").into_owned();
    }

    pub fn select_next_diff_hunk(&mut self) {
        self.state.diff_preview.select_next_hunk();
        if let Some(context) = self.state.diff_preview.selected_hunk_context() {
            self.state.status = t!(
                "status.selected_diff_hunk",
                path = context.path,
                header = context.hunk_header
            )
            .into_owned();
        } else {
            self.state.status = t!("status.no_diff_hunk").into_owned();
        }
    }

    pub fn select_prev_diff_hunk(&mut self) {
        self.state.diff_preview.select_prev_hunk();
        if let Some(context) = self.state.diff_preview.selected_hunk_context() {
            self.state.status = t!(
                "status.selected_diff_hunk",
                path = context.path,
                header = context.hunk_header
            )
            .into_owned();
        } else {
            self.state.status = t!("status.no_diff_hunk").into_owned();
        }
    }

    pub fn stage_selected_diff_context(&mut self) {
        let Some(context) = self.state.diff_preview.selected_hunk_context() else {
            self.state.status = t!("status.no_diff_hunk_context").into_owned();
            return;
        };
        let path = context.path.clone();
        let prompt = diff_hunk_context_prompt(&context);

        if self.state.active_turn().is_some() {
            self.state.pending_messages.push(prompt);
            self.state.status = t!("status.staged_diff_context", path = path).into_owned();
        } else {
            if !self.state.composer.trim().is_empty() {
                self.state.composer_cursor = None;
                self.state.insert_composer_text("\n\n");
            }
            self.state.insert_composer_text(&prompt);
            self.state.status = t!("status.added_diff_context", path = path).into_owned();
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
                // M22-D: if we just applied a staged onboarding
                // permission profile and `session/status/read` is
                // advertised, refresh status so the runtime policy
                // stamp arrives and the mismatch validator runs.
                // `PermissionProfileSetResult` itself does not carry
                // the stamp, so without this refresh the user would
                // never see a clamp warning.
                let session_id = event.session_id.clone();
                self.apply_permission_profile_event(event);
                self.refresh_active_menu_if_open();
                if self.state.onboarding.staged_permission_profile.is_some()
                    && self.state.capabilities.as_ref().is_some_and(|caps| {
                        caps.supports_method(crate::model::APPUI_METHOD_SESSION_STATUS_READ)
                    })
                {
                    return Some(AppUiCommand::ReadSessionStatus(
                        crate::model::SessionStatusReadParams { session_id },
                    ));
                }
                None
            }
            ClientEvent::SessionHydrate(result) => {
                self.apply_session_hydrate_result(result);
                self.refresh_active_menu_if_open();
                None
            }
            ClientEvent::ReviewStart(result) => {
                self.apply_review_start_result(result);
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
                    t!("status.activity_local_profile").into_owned(),
                    event.message.clone(),
                ));
                self.state.status = event.message;
                self.refresh_active_menu_if_open();
                // Creating the profile flips the wizard from the local-profile
                // (Step 1) screen to the provider (LLM config) step, which is
                // rebuilt in place on the SAME menu frame. The frame still holds
                // the `selected_index` of the local-profile "Continue" row, and
                // that index lines up with "API key" in the provider menu — so
                // without repositioning, the fresh provider step would open with
                // the cursor on API key instead of the first config row. Drop
                // the cursor onto the Model family row (the first thing the user
                // fills in here). A no-op when the menu didn't transition (e.g.
                // the auto-finish open-session path below), since the row id is
                // absent there.
                self.focus_provider_start_row();
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
            ClientEvent::Autonomy(event) => self.apply_autonomy_result(event),
        }
    }

    /// M15-E: fold an autonomy RPC result into the per-session mirror
    /// and emit a status line. This is the dual of the matching
    /// notification handler in [`Self::apply_notification`]; the
    /// mirror stays consistent whether updates arrive as a response
    /// or as a server-pushed notification.
    ///
    /// Returns an optional follow-up command. Today only the `GoalGet`
    /// branch produces one — when a pause/resume is pending (see
    /// [`Self::start_goal_transition`]), the freshly-fetched goal
    /// objective is forwarded as a `session/goal/set` so the backend
    /// receives server truth, not a possibly-stale cached mirror.
    fn apply_autonomy_result(
        &mut self,
        event: crate::client_event::AutonomyClientEvent,
    ) -> Option<AppUiCommand> {
        use crate::client_event::AutonomyResult;
        match event.result {
            AutonomyResult::AgentList(result) => {
                let count = result.agents.len();
                // Stuck-chip reconnect safety net: on session reopen the
                // `agent/list` snapshot carries the authoritative terminal
                // status for each background agent. Reconcile any stale
                // running `session.tasks` entry against it so a chip that was
                // pinned on "Orchestrating…" before the reconnect flips even
                // if the live terminal `agent/updated` was missed while
                // disconnected.
                for agent in &result.agents {
                    self.reconcile_task_from_agent_record(&result.session_id, agent);
                }
                self.state
                    .set_session_agents(&result.session_id, result.agents);
                self.state.status = t!("status.agent_list_refreshed", count = count).into_owned();
            }
            AutonomyResult::AgentStatus(result) => {
                let agent_id = result.agent.agent_id.clone();
                self.reconcile_task_from_agent_record(&result.session_id, &result.agent);
                self.state
                    .upsert_session_agent(&result.session_id, result.agent);
                self.state.status = t!("status.agent_status_updated", id = agent_id).into_owned();
            }
            AutonomyResult::AgentOutput(result) => {
                let bytes = result.text.len();
                self.state.set_agent_output(
                    &result.session_id,
                    &result.agent_id,
                    result.text.clone(),
                    result.cursor,
                );
                self.state.status = t!(
                    "status.agent_output_bytes",
                    id = result.agent_id,
                    bytes = bytes
                )
                .into_owned();
            }
            AutonomyResult::AgentArtifacts(result) => {
                let count = result.artifacts.len();
                let agent_id = result.agent_id.clone();
                self.state
                    .set_agent_artifacts(&result.session_id, &agent_id, result.artifacts);
                self.state.status =
                    t!("status.agent_artifacts_count", id = agent_id, count = count).into_owned();
            }
            AutonomyResult::AgentArtifactRead(result) => {
                let title = result.artifact.title.clone();
                self.state.artifact_detail.open_agent_artifact(
                    &result.agent_id,
                    &result.artifact,
                    result.content,
                );
                self.state.status = t!(
                    "status.agent_artifact_loaded",
                    id = result.agent_id,
                    title = title
                )
                .into_owned();
            }
            AutonomyResult::TaskArtifactRead(result) => {
                let title = result.artifact.title.clone();
                self.state.artifact_detail.open_task_artifact(
                    &result.task_id,
                    &result.artifact,
                    result.content,
                );
                self.state.status = t!(
                    "status.task_artifact_loaded",
                    id = result.task_id,
                    title = title
                )
                .into_owned();
            }
            AutonomyResult::ThreadGraph(result) => {
                let count = result.threads.len();
                self.state.thread_graph_detail.open(&result);
                self.state.status = t!("status.thread_graph_loaded", count = count).into_owned();
            }
            AutonomyResult::TurnState(result) => {
                let state = result.state.as_str();
                self.state.turn_state_detail.open(&result);
                self.state.status = t!(
                    "status.turn_state",
                    id = short_id(&result.turn_id.0.to_string()),
                    state = state
                )
                .into_owned();
            }
            AutonomyResult::AgentInterrupt(result) => {
                if let Some(agent) = result.agent.clone() {
                    self.state.upsert_session_agent(&result.session_id, agent);
                }
                let outcome = if result.ok {
                    t!("status.accepted")
                } else {
                    t!("status.rejected")
                };
                self.state.status = t!(
                    "status.agent_interrupt",
                    id = result.agent_id,
                    outcome = outcome
                )
                .into_owned();
            }
            AutonomyResult::AgentClose(result) => {
                if let Some(agent) = result.agent.clone() {
                    self.state.upsert_session_agent(&result.session_id, agent);
                }
                let outcome = if result.ok {
                    t!("status.accepted")
                } else {
                    t!("status.rejected")
                };
                self.state.status = t!(
                    "status.agent_close",
                    id = result.agent_id,
                    outcome = outcome
                )
                .into_owned();
            }
            AutonomyResult::GoalGet(result) => {
                let session_id = result.session_id.clone();
                let summary = match result.goal.as_ref() {
                    Some(goal) => t!(
                        "status.goal_summary",
                        status = goal.status,
                        objective = goal.objective
                    )
                    .into_owned(),
                    None => t!("status.no_active_goal").into_owned(),
                };
                let fresh_goal = result.goal.clone();
                self.state.set_session_goal(&session_id, result.goal, None);
                self.state.status = summary;
                // Pause/resume staged a transition before the refresh.
                // Consume it now with the freshly-fetched objective so
                // the backend never receives a stale cached mirror.
                return self.consume_pending_goal_transition(&session_id, fresh_goal.as_ref());
            }
            AutonomyResult::GoalSet(result) => {
                let session_id = result.session_id.clone();
                let summary = match result.goal.as_ref() {
                    Some(goal) => t!(
                        "status.goal_summary",
                        status = goal.status,
                        objective = goal.objective
                    )
                    .into_owned(),
                    None if result.ok => t!("status.goal_accepted_no_record").into_owned(),
                    None => t!("status.goal_set_rejected").into_owned(),
                };
                self.state
                    .set_session_goal(&session_id, result.goal, result.transition_actor);
                self.state.status = summary;
            }
            AutonomyResult::GoalClear(result) => {
                if result.cleared {
                    self.state
                        .set_session_goal(&result.session_id, None, result.transition_actor);
                    self.state.status = t!("status.goal_cleared").into_owned();
                } else {
                    self.state.status = t!("status.goal_clear_rejected").into_owned();
                }
                // Goal cleared / clear-rejected: a previously-staged
                // pause/resume against this session no longer makes
                // sense. Drop it so a later `/goal pause` cannot fire
                // an orphan transition.
                self.discard_pending_goal_transition_for(&result.session_id);
            }
            AutonomyResult::LoopCreate(result) => {
                let loop_id = result.loop_state.loop_id.clone();
                let mode = result.loop_state.mode.clone();
                self.state
                    .upsert_session_loop(&result.session_id, result.loop_state);
                self.state.status =
                    t!("status.loop_created", id = loop_id, mode = mode).into_owned();
            }
            AutonomyResult::LoopList(result) => {
                let count = result.loops.len();
                self.state
                    .set_session_loops(&result.session_id, result.loops);
                self.state.status = t!("status.loop_list_refreshed", count = count).into_owned();
            }
            AutonomyResult::LoopMutation { method, result } => {
                let loop_id = result.loop_id.clone();
                let session_id = result.session_id.clone();
                // Only mutate the mirror when the backend accepted the
                // request. A rejected `loop/delete` (policy denial,
                // backend pause-and-deny, etc.) must NOT remove a still
                // active loop from local state — that would hide it
                // until the next full hydration.
                if result.ok {
                    if method == crate::model::APPUI_METHOD_LOOP_DELETE {
                        self.state.remove_session_loop(&session_id, &loop_id);
                    } else if let Some(loop_state) = result.loop_state {
                        self.state.upsert_session_loop(&session_id, loop_state);
                    }
                } else if let Some(loop_state) = result.loop_state {
                    // Even on rejection, the backend may echo the
                    // current loop record (status="paused" etc.) — keep
                    // the mirror consistent without dropping the entry.
                    self.state.upsert_session_loop(&session_id, loop_state);
                }
                let verb = match method.as_str() {
                    "loop/delete" => "delete",
                    "loop/pause" => "pause",
                    "loop/resume" => "resume",
                    "loop/fire_now" => "fire_now",
                    _ => "mutation",
                };
                let outcome = if result.ok {
                    t!("status.accepted")
                } else {
                    t!("status.rejected")
                };
                self.state.status = t!(
                    "status.loop_mutation",
                    id = loop_id,
                    verb = verb,
                    outcome = outcome
                )
                .into_owned();
            }
        }
        None
    }

    /// Consume a staged pause/resume transition with the freshly-fetched
    /// goal record. Returns the follow-up `session/goal/set` command, or
    /// `None` if there is no matching pending transition, if the goal
    /// vanished server-side, or if the fresh goal status is no longer
    /// transitionable (e.g. the model marked it complete between dispatch
    /// and refresh).
    fn consume_pending_goal_transition(
        &mut self,
        session_id: &SessionKey,
        fresh_goal: Option<&octos_core::ui_protocol::UiGoalRecord>,
    ) -> Option<AppUiCommand> {
        let pending = self.state.pending_goal_transition.as_ref()?;
        if &pending.session_id != session_id {
            return None;
        }
        let pending = self.state.pending_goal_transition.take()?;
        let goal = match fresh_goal {
            Some(goal) => goal,
            None => {
                self.state.status = t!("status.cannot_transition_no_goal").into_owned();
                return None;
            }
        };
        if !matches!(goal.status.as_str(), "active" | "paused" | "budget_limited") {
            self.state.status =
                t!("status.cannot_transition_goal_state", state = goal.status).into_owned();
            return None;
        }
        let verb = match pending.action {
            crate::model::SessionGoalSetAction::Pause => t!("status.goal_verb_pausing"),
            crate::model::SessionGoalSetAction::Resume => t!("status.goal_verb_resuming"),
            crate::model::SessionGoalSetAction::Set => t!("status.goal_verb_updating"),
        };
        self.state.status = t!("status.verb_goal", verb = verb).into_owned();
        Some(AppUiCommand::SetSessionGoal(
            crate::model::SessionGoalSetParams {
                session_id: pending.session_id,
                profile_id: pending.profile_id,
                objective: goal.objective.clone(),
                status: Some(pending.status.into()),
                token_budget: None,
                transition_actor: Some("user".into()),
                action: pending.action,
            },
        ))
    }

    /// Drop any staged pause/resume that targeted the given session.
    /// Used when the goal is cleared (so a stale transition cannot fire
    /// against a non-existent goal).
    fn discard_pending_goal_transition_for(&mut self, session_id: &SessionKey) {
        if self
            .state
            .pending_goal_transition
            .as_ref()
            .is_some_and(|pending| &pending.session_id == session_id)
        {
            self.state.pending_goal_transition = None;
        }
    }

    /// M15-E reconnect-hydration: re-request the autonomy mirror from
    /// the backend after a session opens (or reopens). The TUI must
    /// never construct agent/goal/loop state from local config — this
    /// is the canonical "ask the server" hook. Each follow-up is
    /// gated on the matching method advertisement so old servers see
    /// nothing.
    ///
    /// The current public surface only emits ONE command per call
    /// (matching the rest of `apply_event`/`apply_client_event`); the
    /// caller can chain `hydrate_autonomy_state_next()` to walk all
    /// three. The lowest-priority follow-up (loops) is dispatched last.
    pub fn hydrate_autonomy_state_commands(&self, session_id: &SessionKey) -> Vec<AppUiCommand> {
        let mut commands = Vec::new();
        let capabilities = match self.state.capabilities.as_ref() {
            Some(caps) => caps,
            None => return commands,
        };
        if !capabilities.supports_feature(crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1) {
            return commands;
        }
        let profile_id = self
            .state
            .active_session()
            .and_then(|session| session.profile_id.clone());
        if capabilities.supports_method(crate::model::APPUI_METHOD_AGENT_LIST) {
            commands.push(AppUiCommand::ListAgents(crate::model::AgentListParams {
                session_id: session_id.clone(),
                parent_agent_id: None,
            }));
        }
        if capabilities.supports_method(crate::model::APPUI_METHOD_SESSION_GOAL_GET) {
            commands.push(AppUiCommand::GetSessionGoal(
                crate::model::SessionGoalGetParams {
                    session_id: session_id.clone(),
                    profile_id: profile_id.clone(),
                },
            ));
        }
        if capabilities.supports_method(crate::model::APPUI_METHOD_LOOP_LIST) {
            commands.push(AppUiCommand::ListLoops(crate::model::LoopListParams {
                session_id: session_id.clone(),
                profile_id,
            }));
        }
        commands
    }

    pub fn hydrate_session_state_command(&self, session_id: &SessionKey) -> Option<AppUiCommand> {
        let capabilities = self.state.capabilities.as_ref()?;
        if !capabilities.supports_feature(crate::model::APPUI_FEATURE_SESSION_HYDRATE_V1)
            || !capabilities.supports_method(crate::model::APPUI_METHOD_SESSION_HYDRATE)
        {
            return None;
        }
        Some(AppUiCommand::HydrateSession(SessionHydrateParams {
            session_id: session_id.clone(),
            after: None,
            include: vec![
                octos_core::ui_protocol::hydrate_sections::MESSAGES.into(),
                octos_core::ui_protocol::hydrate_sections::THREADS.into(),
                octos_core::ui_protocol::hydrate_sections::TURNS.into(),
                octos_core::ui_protocol::hydrate_sections::PENDING_APPROVALS.into(),
            ],
        }))
    }

    pub fn apply_event(&mut self, event: AppUiEvent) -> Option<AppUiCommand> {
        let command = match event {
            AppUiEvent::Snapshot(snapshot) => {
                let composer = self.state.composer.clone();
                let composer_drafts = self.state.composer_drafts.clone();
                let pending_messages = self.state.pending_messages.clone();
                let optimistic_user_messages = self.state.optimistic_user_messages.clone();
                let approval_auto_open = self.state.approval_auto_open;
                let user_question_auto_open = self.state.user_question_auto_open;
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
                // Local-only: the server doesn't know the per-session /thinking
                // level, so preserve it across snapshot replays (reconnect/refresh).
                let session_reasoning_effort = self.state.session_reasoning_effort.clone();
                // Local-only: the active /theme palette is a client setting the
                // server never echoes, so preserve it across snapshot replays
                // (otherwise a launch --theme or a runtime /theme reverts to Codex).
                let theme = self.state.theme;

                let mut state = AppState::from_snapshot(snapshot);
                if state.capabilities.is_none() {
                    state.capabilities = previous_capabilities;
                }
                state.set_composer_text(composer);
                state.composer_drafts = composer_drafts;
                state.pending_messages = pending_messages;
                state.optimistic_user_messages = optimistic_user_messages;
                state.approval_auto_open = approval_auto_open;
                state.user_question_auto_open = user_question_auto_open;
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
                state.session_reasoning_effort = session_reasoning_effort;
                state.theme = theme;
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
                // M22-B: route `profile/local/create` failures back
                // into the onboarding step so the user lands on a
                // typed recovery instead of a generic status line.
                //
                // Order matters here:
                //
                // 1. Transport-level codes (`transport_read`,
                //    `transport_send`, `malformed_frame`) take
                //    PRECEDENCE: even if the message text mentions
                //    `profile/local/create`, the failure is a wire-
                //    level event, not a profile rejection. Clear the
                //    pending flag so the user can retry without
                //    pretending the username was at fault.
                // 2. Otherwise attribution requires a POSITIVE
                //    signal — a known local-create error code or an
                //    explicit method-prefixed error message
                //    (`profile/local/create request tui-N failed: …`,
                //    see `error_response_to_app_event`). The bare
                //    `local_profile_create_pending` boolean is NOT
                //    enough on its own because an unrelated RPC
                //    failing during the pending window would
                //    otherwise be misclassified.
                // Codes the client raises that are NOT profile-
                // level rejections. The substring check below MUST
                // NOT route these through profile recovery even
                // when the message names `profile/local/create` —
                // the wire/policy/cancellation failure is not a
                // field problem.
                let is_client_synth_error = matches!(
                    error.code.as_str(),
                    "transport_read"
                        | "transport_send"
                        | "malformed_frame"
                        | "malformed_json"
                        | "frame_too_large"
                        | "readonly"
                        | "too_many_pending_requests"
                        | "request_cancelled"
                );
                let attribute_to_local_create = !is_client_synth_error
                    && (is_local_create_error_code(&error.code)
                        || error.message.contains("profile/local/create"));

                // Of those, only the codes that DEFINITIVELY end the
                // in-flight local-create request should clear the
                // pending snapshot. Generic `too_many_pending_requests`,
                // `frame_too_large`, and `malformed_json` can fire
                // on OTHER commands while the local-create response
                // is still on its way; clearing the snapshot in
                // that case would let a second create dispatch
                // (the overlapping-create finding) and could
                // misattribute the eventual response to a stale
                // pending tracker.
                //
                // The conservative set is:
                //   - `transport_read`/`transport_send`: wire-level
                //     break → no response will arrive for ANY in-
                //     flight request including the local-create.
                //   - Other client-synth codes (`request_cancelled`,
                //     `readonly`, `frame_too_large`,
                //     `too_many_pending_requests`) when the message
                //     names `profile/local/create`: the rejection
                //     is attributed to the local-create RPC itself
                //     (cancellation, pre-send policy/encoding/queue
                //     gate) so the request is GONE.
                //
                // `malformed_frame` and `malformed_json` are
                // recoverable parser errors — the transport stays
                // connected and `pending_requests` is not drained,
                // so the original `profile/local/create` response
                // can still arrive. Clearing the pending flag for
                // those would allow a duplicate create and
                // misattribute the eventual response.
                let names_local_create = error.message.contains("profile/local/create");
                let cancels_in_flight_create =
                    matches!(error.code.as_str(), "transport_read" | "transport_send")
                        || (matches!(
                            error.code.as_str(),
                            "request_cancelled"
                                | "readonly"
                                | "frame_too_large"
                                | "too_many_pending_requests"
                        ) && names_local_create);
                let is_transport_error = matches!(
                    error.code.as_str(),
                    "transport_read" | "transport_send" | "malformed_frame"
                );
                if cancels_in_flight_create && self.state.onboarding.local_profile_create_pending {
                    self.state.onboarding.local_profile_create_pending = false;
                    self.state.onboarding.local_profile_create_pending_username = None;
                    self.state.status = if is_transport_error {
                        t!(
                            "status.local_create_cancelled_transport",
                            code = error.code,
                            message = error.message
                        )
                        .into_owned()
                    } else {
                        t!(
                            "status.local_create_cancelled",
                            code = error.code,
                            message = error.message
                        )
                        .into_owned()
                    };
                } else if error.code == "frame_too_large" {
                    // Recoverable pre-send rejection: the frame (e.g. a large
                    // inline turn input or paste) exceeded the 1 MB UI-protocol
                    // cap. The connection + session are fine — surface an
                    // actionable message rather than a raw "Error [...]" and do
                    // NOT wedge the session in Error (mini5: a 1.1 MB inline send
                    // left the session stuck in Error, unrecoverable). The
                    // local-create attribution above runs first so the wizard's
                    // pending-clear is preserved.
                    self.state.status =
                        t!("status.message_too_large", message = error.message).into_owned();
                } else if is_client_synth_error {
                    // Surfaced for the user but does NOT touch the
                    // local-create pending state.
                    self.state.status = t!(
                        "status.error_code_message",
                        code = error.code,
                        message = error.message
                    )
                    .into_owned();
                } else if attribute_to_local_create {
                    self.state
                        .onboarding
                        .apply_local_profile_error(&error.code, &error.message);
                    let recovery_message_and_focus = self
                        .state
                        .onboarding
                        .local_profile_recovery
                        .as_ref()
                        .map(|recovery| (recovery.message.clone(), recovery.focus_field));
                    if let Some((message, focus_field)) = recovery_message_and_focus {
                        self.state.status =
                            t!("status.local_profile_setup_blocked", message = message)
                                .into_owned();
                        self.refresh_active_menu_if_open();
                        self.focus_local_profile_field(focus_field);
                    } else {
                        self.state.status = t!(
                            "status.error_code_message",
                            code = error.code,
                            message = error.message
                        )
                        .into_owned();
                    }
                } else {
                    self.state.status = t!(
                        "status.error_code_message",
                        code = error.code,
                        message = error.message
                    )
                    .into_owned();
                }
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Error,
                        error.code.clone(),
                        error.message.clone(),
                    )
                    .with_detail("app-ui error"),
                );
                if error.code == "frame_too_large" {
                    // Recoverable — keep the session usable (idle) instead of
                    // wedging it in Error on an oversized inline send.
                    self.state.set_run_state_idle();
                } else {
                    self.state.set_run_state_error(error.message);
                }
                None
            }
        };
        self.refresh_active_menu_if_open();
        command
    }

    pub fn apply_diff_preview_result(&mut self, result: DiffPreviewGetResult) {
        let title = result.preview.title.clone().unwrap_or_else(|| {
            t!("status.file_diff_count", count = result.preview.files.len()).into_owned()
        });
        let status = result.status.clone();
        let file_count = result.preview.files.len();
        self.state.diff_preview.apply_result(result);
        self.state.status = t!(
            "status.diff_preview_result",
            status = status,
            title = title,
            file_count = file_count
        )
        .into_owned();
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
            t!("status.activity_mcp_config").into_owned(),
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_mcp_config_mutation_event(&mut self, event: McpConfigMutationClientEvent) {
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            t!("status.activity_mcp_config").into_owned(),
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

    fn apply_session_hydrate_result(&mut self, result: SessionHydrateResult) {
        let session_id = result.session_id.clone();
        let projected_messages = hydrated_projection_messages(&result);
        let message_count = projected_messages.as_ref().map_or(0, Vec::len);
        let thread_count = result.threads.as_ref().map_or(0, Vec::len);
        let turn_count = result.turns.as_ref().map_or(0, Vec::len);
        let pending_approvals_present = result.pending_approvals.is_some();
        let approval_count = result.pending_approvals.as_ref().map_or(0, Vec::len);

        if let Some(messages) = projected_messages {
            if let Some(session) = self.find_session_mut(&session_id) {
                session.messages = messages;
                // codex P1: do NOT clear `live_reply` here. The hydrate result
                // is COMMITTED history only; `live_reply` holds the turn that is
                // still streaming. On a mid-turn reconnect, clearing it silently
                // dropped the rest of that turn's deltas (the turn froze). Keep
                // it — subsequent deltas keep appending and `TurnCompleted` still
                // commits it normally.
            } else {
                self.state.sessions.push(SessionView {
                    id: session_id.clone(),
                    title: session_id.0.clone(),
                    profile_id: self.active_session_profile_id(),
                    messages,
                    tasks: Vec::new(),
                    live_reply: None,
                });
                self.state.selected_session = self.state.sessions.len().saturating_sub(1);
            }
            self.state
                .optimistic_user_messages
                .retain(|optimistic| optimistic.session_id != session_id);
            self.state.scroll_transcript_to_latest();
        }

        if let Some(context_state) = result.context_state.as_ref() {
            self.state.context_lifecycle_mut(&session_id).state =
                Some(context_lifecycle_state_from_ui(context_state));
        }

        if let Some(threads) = result.threads.as_ref()
            && self.state.thread_graph_detail.active
        {
            self.state
                .thread_graph_detail
                .open(&octos_core::ui_protocol::ThreadGraphGetResult {
                    session_id: session_id.clone(),
                    cursor: result.cursor.clone(),
                    threads: threads.clone(),
                    orphans: Vec::new(),
                });
        }

        if let Some(turns) = result.turns.as_ref() {
            // GAP 1: orphan activity-chip self-heal on the rehydrate path. A
            // client rehydrating a session whose turn is already TERMINAL
            // (Completed/Errored/Interrupted) but that still carries a stranded
            // running-status activity item (a `ToolStarted` whose `ToolCompleted`
            // never arrived) would otherwise pin "Orchestrating…" forever after
            // reconnect — the hydrate path never ran the terminal reconcile that
            // the live `TurnCompleted`/`TurnError` chokepoint does. Reconcile each
            // genuinely-terminal hydrated turn here, sharing the exact same sweep.
            //
            // NEVER reconcile the session's currently-active/live turn: its
            // running work is legitimately in-flight. The local `live_reply` turn
            // is the source of truth for "still streaming", so we skip it even if
            // the snapshot were to mislabel it terminal.
            let live_turn_id = self
                .state
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .and_then(|session| session.live_reply.as_ref())
                .map(|live_reply| live_reply.turn_id.clone());
            for turn in turns {
                let is_terminal = matches!(
                    turn.state,
                    TurnLifecycleState::Completed
                        | TurnLifecycleState::Errored
                        | TurnLifecycleState::Interrupted
                );
                if is_terminal && live_turn_id.as_ref() != Some(&turn.turn_id) {
                    self.state
                        .reconcile_terminal_turn_running_activity(&turn.turn_id);
                }
            }

            if self.state.turn_state_detail.active
                && let Some(active_turn) = turns.last()
            {
                self.state
                    .turn_state_detail
                    .open(&octos_core::ui_protocol::TurnStateGetResult {
                        session_id: session_id.clone(),
                        turn_id: active_turn.turn_id.clone(),
                        state: active_turn.state,
                        context: result.context.clone(),
                        context_state: result.context_state.clone(),
                        started_at: active_turn.started_at,
                        completed_at: active_turn.completed_at,
                        thread_id: active_turn.thread_id.clone(),
                        committed_seqs: Vec::new(),
                    });
            }
        }

        if let Some(approvals) = result.pending_approvals {
            self.apply_hydrated_pending_approvals(&session_id, approvals);
        }

        if let Some(questions) = result.pending_questions {
            self.apply_hydrated_pending_questions(&session_id, questions);
        }

        let mut sections = Vec::new();
        if result.messages.is_some() {
            sections.push(t!("status.message_count", count = message_count).into_owned());
        }
        if result.threads.is_some() {
            sections.push(t!("status.thread_count", count = thread_count).into_owned());
        }
        if result.turns.is_some() {
            sections.push(t!("status.turn_count", count = turn_count).into_owned());
        }
        if approval_count > 0 || pending_approvals_present {
            sections.push(t!("status.pending_approval_count", count = approval_count).into_owned());
        }
        let summary = if sections.is_empty() {
            t!("status.session_state").into_owned()
        } else {
            sections.join(", ")
        };
        self.state.status = t!("status.session_hydrated", summary = summary).into_owned();
    }

    fn apply_review_start_result(&mut self, result: ReviewStartResult) {
        let workflow = result.workflow.as_deref().unwrap_or("code_review");
        let backend = result.backend.as_deref().unwrap_or("backend");
        let agent_count = result.agent_count.unwrap_or_default();
        let status = if result.accepted {
            t!(
                "status.review_started",
                count = agent_count,
                backend = backend
            )
            .into_owned()
        } else {
            t!("status.review_not_accepted").into_owned()
        };
        self.state.push_activity(
            ActivityItem::new(
                ActivityKind::Progress,
                t!("status.activity_code_review").into_owned(),
                status.clone(),
            )
            .with_turn(result.turn_id.clone())
            .with_detail(format!(
                "workflow={workflow}, session={}",
                result.session_id
            )),
        );
        if result.accepted {
            self.state.set_run_state_in_progress();
        }
        self.state.status = status;
    }

    fn apply_hydrated_pending_approvals(
        &mut self,
        session_id: &SessionKey,
        approvals: Vec<octos_core::ui_protocol::ApprovalRequestedEvent>,
    ) {
        let count = approvals.len();
        let Some(event) = approvals.into_iter().last() else {
            if self
                .state
                .approval
                .as_ref()
                .is_some_and(|approval| &approval.session_id == session_id)
            {
                self.state.approval = None;
                self.state.set_run_state_idle();
            }
            return;
        };
        let title = event.title.clone();
        self.state.push_activity(
            ActivityItem::new(
                ActivityKind::Approval,
                t!("status.activity_pending_approvals").into_owned(),
                t!("status.pending_approval_count", count = count).into_owned(),
            )
            .with_turn(event.turn_id.clone())
            .with_detail(title.clone()),
        );
        let mut approval = ApprovalModalState::from_event(event);
        approval.visible = self.state.approval_auto_open;
        self.state.approval = Some(approval);
        self.state.focus = FocusPane::Composer;
        self.state.set_run_state_blocked(title);
    }

    /// UPCR-2026-023 reconnect path: re-render a still-pending AskUserQuestion
    /// from `session/hydrate`, mirroring [`Self::apply_hydrated_pending_approvals`].
    fn apply_hydrated_pending_questions(
        &mut self,
        session_id: &SessionKey,
        questions: Vec<UserQuestionRequestedEvent>,
    ) {
        let Some(event) = questions.into_iter().last() else {
            if self
                .state
                .user_question
                .as_ref()
                .is_some_and(|picker| &picker.session_id == session_id)
            {
                self.state.user_question = None;
                self.state.set_run_state_idle();
            }
            return;
        };
        let title = event.title.clone();
        self.state.push_activity(
            ActivityItem::new(
                ActivityKind::Approval,
                t!("status.activity_pending_question").into_owned(),
                title.clone(),
            )
            .with_turn(event.turn_id.clone()),
        );
        let mut picker = UserQuestionPickerState::from_event(event);
        picker.visible = self.state.user_question_auto_open;
        self.state.user_question = Some(picker);
        self.state.focus = FocusPane::Composer;
        self.state.set_run_state_blocked(title);
    }

    fn apply_profile_llm_catalog_event(&mut self, event: ProfileLlmCatalogClientEvent) {
        self.state.profile_llm_catalog = Some(event.result);
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            t!("status.activity_provider_catalog").into_owned(),
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
                    // M22-E: a successful test clears any prior
                    // failure reason so the menu does not surface
                    // a stale "test failed" recovery line.
                    self.state.onboarding.provider_test_failure_reason = None;
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
                    self.state.onboarding.provider_test_failure_reason = None;
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
                    self.state.onboarding.provider_test_failure_reason = None;
                }
            }
            if reset_staged_provider {
                self.state.onboarding.reset_staged_provider();
            }
            self.state.onboarding.last_message = Some(event.message.clone());
        } else if pending.is_some() {
            // M22-E: a failed `profile/llm/test` (or save) must
            // NOT mark the provider as ready. Record the typed
            // failure reason from the server so the menu shows a
            // recovery line — `provider_tested` stays false and
            // `provider_status()` reports `TestFailed`.
            if matches!(pending, Some(OnboardingProviderPending::Test)) {
                self.state.onboarding.provider_tested = false;
                let staged_secret = self.state.onboarding.api_key.clone();
                self.state.onboarding.provider_test_failure_reason =
                    Some(provider_failure_reason(&event, staged_secret.as_ref()));
            }
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
            t!("status.activity_skill_registry").into_owned(),
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
        // M22-D: snapshot the stamp BEFORE consuming the result so
        // we can compare it against the staged permission profile.
        let stamp = event.result.runtime_policy_stamp.clone();
        let message = event.message;
        self.state
            .set_runtime_status(SessionRuntimeStatus::from(event.result));
        if let (Some(staged), Some(stamp)) = (
            self.state.onboarding.staged_permission_profile.clone(),
            stamp,
        ) {
            let mismatch = permission_profile_stamp_mismatch(&staged, &stamp);
            self.state.onboarding.permission_profile_mismatch = mismatch.clone();
            if let Some(reason) = mismatch {
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Warning,
                        t!("status.activity_permission_profile_mismatch").into_owned(),
                        reason.clone(),
                    )
                    .with_detail(t!("status.server_clamped_onboarding_choice").into_owned()),
                );
            }
        }
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            t!("status.activity_runtime_status").into_owned(),
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
            t!("status.activity_tool_config").into_owned(),
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_tool_config_mutation_event(&mut self, event: ToolConfigMutationClientEvent) {
        self.state.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            t!("status.activity_tool_config").into_owned(),
            event.message.clone(),
        ));
        self.state.status = event.message;
    }

    fn apply_progress(&mut self, event: UiProgressEvent) -> Option<AppUiCommand> {
        // Retain the latest cumulative usage (tokens + session cost) per session
        // for the whole-job indicator. Merge so a partial update (only cost, or
        // only tokens) doesn't wipe the other field.
        if let Some(token_cost) = event.metadata.token_cost.as_ref() {
            let entry = self
                .state
                .session_usage
                .entry(event.session_id.clone())
                .or_insert((None, None, None));
            if token_cost.input_tokens.is_some() {
                entry.0 = token_cost.input_tokens;
            }
            if token_cost.output_tokens.is_some() {
                entry.1 = token_cost.output_tokens;
            }
            if token_cost.session_cost.is_some() {
                entry.2 = token_cost.session_cost;
            }
        }
        // Gap 2 fix #3: surface the `UiRetryBackoff` carried on
        // `metadata.retry` (previously ignored) so the harness status row can
        // render "retrying (attempt N)". A non-retry progress event clears the
        // stale entry so a settled turn doesn't linger as "retrying".
        if let Some(retry) = event.metadata.retry.as_ref() {
            self.state
                .session_retry
                .insert(event.session_id.clone(), retry.clone());
        } else {
            self.state.session_retry.remove(&event.session_id);
        }
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
            self.state.status = t!(
                "status.opening_diff_preview",
                operation = operation,
                path = path
            )
            .into_owned();
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
                // Restore the server-persisted per-session reasoning effort so
                // /thinking + its menu reflect it after a full restart (the server
                // is the source of truth; `None` means no override is stored).
                match event.reasoning_effort {
                    Some(level) => {
                        self.state
                            .session_reasoning_effort
                            .insert(session_id.clone(), level);
                    }
                    None => {
                        self.state.session_reasoning_effort.remove(&session_id);
                    }
                }
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
                // Issue #4: finishing the setup wizard must drop the user into a
                // clean, ready coding surface — not leave the onboarding menu
                // stacked over the chat. When a session opens while an
                // onboarding menu is active, tear the wizard down so the
                // composer is focused and the transcript is the active surface.
                if self.active_menu_is_onboarding() {
                    self.close_all_menus();
                    self.state.focus = FocusPane::Composer;
                }
                self.state.status =
                    format!("Opened {} on {}", session_id.0, self.state.protocol_version);
                if let Some(command) = self.hydrate_session_state_command(&session_id) {
                    self.state.enqueue_autonomy_hydration(command);
                }
                // M15-E: queue autonomy hydration follow-ups so the
                // local mirror reflects backend truth on session open
                // and after reconnect. Gated on
                // `coding.autonomy.v1` — old servers receive no probe.
                let hydration = self.hydrate_autonomy_state_commands(&session_id);
                for command in hydration {
                    self.state.enqueue_autonomy_hydration(command);
                }
                // M22-D: if the user staged a permission profile in
                // onboarding, apply it now that we have a session id.
                // Server authority is preserved — the follow-up RPC
                // is only emitted when `permission/profile/set` is
                // advertised, and the runtime policy stamp returned
                // afterward is the source of truth.
                if let Some(update) = self.state.onboarding.staged_permission_profile.clone() {
                    if self.state.capabilities.as_ref().is_some_and(|caps| {
                        caps.supports_method(
                            crate::menu::registry::APPUI_METHOD_PERMISSION_PROFILE_SET,
                        )
                    }) {
                        return Some(AppUiCommand::SetPermissionProfile(
                            octos_core::ui_protocol::PermissionProfileSetParams {
                                session_id,
                                update,
                                runtime_mode: None,
                            },
                        ));
                    }
                }
                None
            }
            UiNotification::TurnStarted(event) => {
                // A new turn for the active session starts a fresh live_reply
                // UNCONDITIONALLY — server-INITIATED master-continuation turns
                // (reason=child_completed / scatter_join_complete) carry no
                // client-side handle, yet their assistant stream must render.
                // If a prior turn's live_reply is still bound (its TurnCompleted
                // was missed or this turn started before it committed), commit
                // the prior answer first so it is neither lost nor merged.
                // (No-op when the bound buffer is for the SAME turn — that case
                // is the lazy-bind/replay race handled just below.)
                self.commit_pending_live_reply_for_turn_switch(&event.session_id, &event.turn_id);
                if let Some(session) = self.find_session_mut(&event.session_id) {
                    // Out-of-order lifecycle (nit 1): a MessageDelta may have
                    // already lazy-bound THIS turn before its TurnStarted was
                    // delivered/replayed. Replacing the buffer unconditionally
                    // would wipe the accumulated text, so preserve a buffer that
                    // is already bound to the same turn; only bind a fresh empty
                    // one when there is no buffer (the prior turn was committed/
                    // dropped above for the different-turn case).
                    let same_turn_already_bound = session
                        .live_reply
                        .as_ref()
                        .is_some_and(|live_reply| live_reply.turn_id == event.turn_id);
                    if !same_turn_already_bound {
                        session.live_reply = Some(LiveReply {
                            turn_id: event.turn_id,
                            text: String::new(),
                        });
                    }
                    self.state.status = format!("Turn started in {}", session.title);
                    self.state.set_run_state_in_progress();
                }
                None
            }
            UiNotification::MessageDelta(MessageDeltaEvent {
                session_id,
                turn_id,
                text,
                ..
            }) => {
                let follow_tail = self.state.transcript_scroll == 0;
                // Lazy-bind: a delta whose turn_id has no current live_reply
                // binding (None, or a binding for an OLDER turn) must still be
                // accumulated — never dropped. This covers continuation turns
                // whose `TurnStarted` was not delivered to this connection, so
                // the first frame the client sees for the turn is a delta. When
                // switching to a new turn_id, commit/close the prior turn's
                // live_reply first so its answer is preserved.
                let needs_bind = self
                    .find_session(&session_id)
                    .and_then(|session| session.live_reply.as_ref())
                    .map(|live_reply| live_reply.turn_id != turn_id)
                    .unwrap_or(true);
                if needs_bind {
                    self.commit_pending_live_reply_for_turn_switch(&session_id, &turn_id);
                }
                let mut reset_scroll = false;
                if let Some(session) = self.find_session_mut(&session_id) {
                    let live_reply = session.live_reply.get_or_insert_with(|| LiveReply {
                        turn_id: turn_id.clone(),
                        text: String::new(),
                    });
                    if live_reply.turn_id == turn_id {
                        live_reply.text.push_str(&text);
                        reset_scroll = true;
                    }
                }
                if needs_bind && reset_scroll {
                    // A delta-first continuation turn (its TurnStarted was not
                    // delivered) is genuinely streaming — reflect the active run.
                    self.state.set_run_state_in_progress();
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
                self.state.status = t!(
                    "status.tool_started",
                    name = event.tool_name,
                    id = event.tool_call_id
                )
                .into_owned();
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
                                t!("status.activity_recovery_suggestion").into_owned(),
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
                self.state.status = t!("status.approval_requested", title = title).into_owned();
                if let Some(preview_id) = diff_preview_id {
                    let request_already_in_flight = self.state.diff_preview.loading
                        && self.state.diff_preview.requested_preview_id.as_ref()
                            == Some(&preview_id);
                    self.state
                        .diff_preview
                        .open_loading_for_turn(preview_id.clone(), Some(diff_preview_turn_id));
                    self.state.status =
                        t!("status.opening_inline_diff_preview", title = title).into_owned();
                    if !request_already_in_flight {
                        return Some(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                            session_id,
                            preview_id,
                        }));
                    }
                }
                None
            }
            UiNotification::UserQuestionRequested(event) => {
                self.apply_user_question_requested(event)
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
            UiNotification::Envelope(event) => self.apply_envelope(event),
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
                let status_label = event.agent.status.clone();
                self.state
                    .upsert_session_agent(&event.session_id, event.agent.clone());
                // Stuck-chip safety net: a spawn_only background task that
                // outlives its spawning turn goes terminal only AFTER the
                // per-turn task-progress channel was torn down, so the
                // terminal `task/updated` never arrives and `session.tasks`
                // stays "running" — pinning the chip on "Orchestrating…". The
                // DURABLE terminal `agent/updated` (delivered via the ledger)
                // carries `task_id` + a terminal status, so reconcile the
                // matching task's state from the agent record. Only terminal
                // statuses flip the task — a "running" record must never
                // resurrect a task the client already saw go terminal.
                self.reconcile_task_from_agent_record(&event.session_id, &event.agent);
                self.state.push_activity(
                    ActivityItem::new(ActivityKind::Progress, title, status_label)
                        .with_detail(detail),
                );
                // Don't churn the status bar with "Agent status refreshed: …" on
                // every agent-status event — during a multi-agent turn that floods
                // the bottom line. The activity item above already surfaces it; the
                // status bar is reserved for low-frequency, meaningful state.
                None
            }
            UiNotification::AgentOutputDelta(event) => {
                let bytes = event.text.len();
                self.state.append_agent_output(
                    &event.session_id,
                    &event.agent_id,
                    event.cursor,
                    &event.text,
                );
                self.state.push_activity(
                    ActivityItem::new(
                        ActivityKind::Progress,
                        t!("status.activity_agent_output").into_owned(),
                        format!("Agent output refreshed: {} ({bytes} bytes)", event.agent_id),
                    )
                    .with_detail(compact_preview(&event.text)),
                );
                None
            }
            UiNotification::AgentArtifactUpdated(event) => {
                let count = event.artifacts.len();
                self.state.set_agent_artifacts(
                    &event.session_id,
                    &event.agent_id,
                    event.artifacts.clone(),
                );
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Tool,
                    t!("status.activity_agent_artifacts").into_owned(),
                    format!("{count} artifact(s) refreshed for {}", event.agent_id),
                ));
                None
            }
            UiNotification::SessionGoalUpdated(event) => {
                let objective = event.goal.objective.clone();
                let status_label = event.goal.status.clone();
                self.state.set_session_goal(
                    &event.session_id,
                    Some(event.goal.clone()),
                    Some(event.transition_actor.clone()),
                );
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    t!("status.activity_session_goal").into_owned(),
                    status_label,
                ));
                self.state.status = format!("Goal updated: {objective}");
                None
            }
            UiNotification::SessionGoalCleared(event) => {
                let actor = event.transition_actor.clone();
                if event.cleared {
                    self.state
                        .set_session_goal(&event.session_id, None, Some(actor));
                }
                self.state.status = if event.cleared {
                    t!("status.goal_cleared").into_owned()
                } else {
                    t!("status.goal_clear_requested").into_owned()
                };
                None
            }
            UiNotification::LoopUpdated(event) => {
                let status = event
                    .status
                    .clone()
                    .unwrap_or_else(|| event.loop_state.status.clone());
                let loop_id = event.loop_state.loop_id.clone();
                if event.deleted == Some(true) || event.loop_state.status == "deleted" {
                    self.state.remove_session_loop(&event.session_id, &loop_id);
                } else {
                    self.state
                        .upsert_session_loop(&event.session_id, event.loop_state.clone());
                }
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    loop_id,
                    status,
                ));
                None
            }
            UiNotification::LoopFired(event) => {
                let status = event.status.clone().unwrap_or_else(|| {
                    event
                        .fire
                        .as_ref()
                        .map(|fire| if fire.queued { "queued" } else { "fired" })
                        .unwrap_or("fired")
                        .into()
                });
                if let Some(loop_state) = event.loop_state.clone() {
                    self.state
                        .upsert_session_loop(&event.session_id, loop_state);
                }
                self.state.push_activity(ActivityItem::new(
                    ActivityKind::Progress,
                    event.loop_id,
                    status,
                ));
                None
            }
            UiNotification::LoopCompleted(event) => {
                let status = event.status.clone().unwrap_or_else(|| "completed".into());
                if let Some(loop_state) = event.loop_state.clone() {
                    self.state
                        .upsert_session_loop(&event.session_id, loop_state);
                }
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
                // Codex-style surface: also leave a PERSISTENT activity row so
                // the user actually sees that context was compacted. The
                // `status` line above is a shared single-line string that the
                // per-turn `context/normalization_reported` (and the next user
                // action) immediately overwrites, so on its own it is
                // effectively invisible. A successful compaction is an
                // infrequent, notable event worth a durable notice.
                if event.compaction.error.is_none() {
                    let before = event.compaction.token_estimate_before;
                    let after = event.compaction.token_estimate_after.unwrap_or(before);
                    // codex P2: stamp the notice with the session's in-flight
                    // turn. Compaction is reported DURING the turn that
                    // triggered it (after `TurnStarted` set `live_reply`), and
                    // the renderer suppresses turnless activities while a turn
                    // is active + `capture_completed_turn_activity` only
                    // archives turn-scoped items. A turnless notice would be
                    // hidden exactly when it fires and never persisted; attach
                    // it to the live turn so it shows and is archived with the
                    // turn. (Falls back to turnless only if no turn is live —
                    // e.g. the connection-independent drain.)
                    let turn_id = self.find_session_mut(&session_id).and_then(|session| {
                        session.live_reply.as_ref().map(|live| live.turn_id.clone())
                    });
                    let mut notice = ActivityItem::new(
                        ActivityKind::Progress,
                        t!("status.activity_context_compacted").into_owned(),
                        format!(
                            "{} → {} tokens",
                            humanize_token_count(before),
                            humanize_token_count(after)
                        ),
                    );
                    notice.detail = Some(format!(
                        "kept {} message(s), dropped {} (trigger: {})",
                        event.compaction.retained_count,
                        event.compaction.dropped_count,
                        event.compaction.trigger,
                    ));
                    notice.success = Some(true);
                    if let Some(turn_id) = turn_id {
                        notice = notice.with_turn(turn_id);
                    }
                    self.state.push_activity(notice);
                }
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
                self.state
                    .context_lifecycle_mut(&session_id)
                    .apply_normalization(state, normalization);
                // Gap 2 fix #4: do NOT write the shared status line here.
                // Normalization is a background lifecycle signal that fires
                // every turn; clobbering `status` overwrote meaningful state
                // (e.g. "compacting…"). The report still lands in the
                // per-session lifecycle ledger above for the inspector.
                None
            }
            UiNotification::SessionOrchestration(event) => {
                // Whole-job status for the composer top-border indicator. Keep
                // it only while active; drop on the terminal active:false so the
                // indicator hides when the job (turn + sub-agents +
                // continuations) is fully done.
                if event.active {
                    self.state
                        .orchestration
                        .insert(event.session_id.clone(), event);
                } else {
                    // Blocking bug 2 (belt-and-suspenders): the whole job is
                    // done — drop the indicator AND any stale retry so it cannot
                    // linger across the terminal orchestration boundary.
                    self.state.orchestration.remove(&event.session_id);
                    self.state.session_retry.remove(&event.session_id);
                }
                None
            }
        }
    }

    fn apply_envelope(&mut self, event: EnvelopeNotification) -> Option<AppUiCommand> {
        let EnvelopeNotification {
            session_id,
            envelope,
            ..
        } = event;
        let thread_id = envelope.thread_id.clone();

        match envelope.payload {
            Payload::UserMessage { text, files } => {
                if let Some(session) = self.find_session_mut(&session_id) {
                    let already_present = session.messages.iter().any(|message| {
                        message.role == MessageRole::User
                            && message.thread_id.as_deref() == Some(thread_id.as_str())
                    });
                    if !already_present {
                        let mut message =
                            Message::user(text).with_thread_id(ThreadId::new(thread_id.clone()));
                        message.media = files.into_iter().map(|file| file.path).collect();
                        session.messages.push(message);
                    }
                }
                // Envelope projection is internal bookkeeping — don't leak the
                // thread_id into the status bar on every projected message (it
                // churned "… projected for <thread_id>" at the bottom of the
                // composer). The transcript already reflects the message.
                None
            }
            Payload::AssistantDelta { text } => {
                self.upsert_envelope_assistant_message(&session_id, &thread_id, text, false);
                // Streaming deltas arrive many times per second; writing the
                // status bar each time flooded the bottom line with "Assistant
                // delta projected for <thread_id>". The streamed text is already
                // visible in the transcript — leave the status line stable.
                None
            }
            Payload::AssistantPersisted { text, .. } => {
                self.upsert_envelope_assistant_message(&session_id, &thread_id, text, true);
                // Same as AssistantDelta: internal projection, not status-bar news.
                None
            }
            Payload::ToolStart { tool_call_id, name } => {
                self.state.push_activity(
                    ActivityItem::new(ActivityKind::Tool, name.clone(), "running")
                        .with_tool_call(tool_call_id.clone())
                        .with_session(session_id.clone())
                        .with_detail(AppState::envelope_tool_detail_for_thread(&thread_id)),
                );
                self.state.set_run_state_in_progress();
                self.state.status =
                    t!("status.tool_started", name = name, id = tool_call_id).into_owned();
                None
            }
            Payload::ToolProgress {
                tool_call_id,
                message,
            } => {
                self.state.update_tool_activity(
                    &tool_call_id,
                    "running",
                    Some(message.clone()),
                    None,
                    None,
                    None,
                );
                self.state.set_run_state_in_progress();
                self.state.status = message;
                None
            }
            Payload::ToolEnd {
                tool_call_id,
                status,
                error,
                reason,
            } => {
                let (label, success) = match status {
                    EnvelopeToolEndStatus::Complete => ("complete", Some(true)),
                    EnvelopeToolEndStatus::Error => ("failed", Some(false)),
                    EnvelopeToolEndStatus::Skipped => ("skipped", None),
                    EnvelopeToolEndStatus::Aborted => ("aborted", Some(false)),
                };
                let detail = error.or(reason);
                self.state.update_tool_activity(
                    &tool_call_id,
                    label,
                    detail.clone(),
                    detail,
                    success,
                    None,
                );
                self.state.status = format!("Tool {label}: {tool_call_id}");
                None
            }
            Payload::FileAttached {
                path,
                mime,
                size_bytes,
            } => {
                self.state.push_activity(
                    ActivityItem::new(ActivityKind::Tool, path.clone(), "attached")
                        .with_detail(format!("{mime}, {size_bytes} bytes")),
                );
                self.state.status = format!("File attached: {path}");
                None
            }
            Payload::TurnCompleted { .. } => {
                // GAP 2: this envelope is a hard terminal barrier for this
                // session's thread. Heal any stranded running tool item the
                // envelope path started for this session+thread (a `ToolStart`
                // whose `ToolEnd` never arrived) so it can no longer pin a
                // turn-less "Orchestrating…" chip. Scoped to `session_id` so a
                // thread_id shared with another session cannot suppress that
                // sibling's genuinely-active chip.
                self.state
                    .reconcile_envelope_thread_running_activity(&session_id, &thread_id);
                self.state.status = format!("Turn completed for {thread_id}");
                self.state.set_run_state_success();
                self.submit_next_pending_if_idle()
            }
        }
    }

    fn upsert_envelope_assistant_message(
        &mut self,
        session_id: &SessionKey,
        thread_id: &str,
        text: String,
        replace: bool,
    ) {
        let follow_tail = self.state.transcript_scroll == 0;
        let Some(session) = self.find_session_mut(session_id) else {
            return;
        };
        if let Some(message) = session.messages.iter_mut().rev().find(|message| {
            message.role == MessageRole::Assistant
                && message.thread_id.as_deref() == Some(thread_id)
        }) {
            if replace {
                message.content = text;
            } else {
                message.content.push_str(&text);
            }
        } else {
            session.messages.push(Message::assistant_with_thread(
                text,
                ThreadId::new(thread_id.to_owned()),
            ));
        }
        if follow_tail {
            self.state.scroll_transcript_to_latest();
        } else {
            self.state.preserve_transcript_position_after_append(1);
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

    /// UPCR-2026-023: surface a `user_question/requested` as an open picker,
    /// mirroring [`Self::apply_notification`]'s `ApprovalRequested` handling. A
    /// garbled/empty event still renders via the mandatory `title`/`body`.
    fn apply_user_question_requested(
        &mut self,
        event: UserQuestionRequestedEvent,
    ) -> Option<AppUiCommand> {
        let title = event.title.clone();
        let detail = if event.questions.is_empty() {
            "free text".to_string()
        } else {
            format!("{} question(s)", event.questions.len())
        };
        self.state.push_activity(
            ActivityItem::new(ActivityKind::Approval, "ask_user_question", title.clone())
                .with_turn(event.turn_id.clone())
                .with_detail(detail),
        );
        let mut picker = UserQuestionPickerState::from_event(event);
        picker.visible = self.state.user_question_auto_open;
        self.state.user_question = Some(picker);
        self.state.focus = FocusPane::Composer;
        self.state.set_run_state_blocked(title.clone());
        self.state.status = t!("status.question_asked", title = title).into_owned();
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

    /// Reconcile a `session.tasks` entry's lifecycle state from a durable
    /// `agent/updated` record. See the call site in the `AgentUpdated` arm for
    /// why this exists (terminal flip for a task whose per-turn task-progress
    /// channel was already gone). Only TERMINAL agent statuses flip a task that
    /// is still pending/running — a non-terminal record never overwrites a task
    /// the client already saw settle, and a record with no `task_id` is a no-op.
    fn reconcile_task_from_agent_record(
        &mut self,
        session_id: &SessionKey,
        agent: &octos_core::ui_protocol::UiAgentRecord,
    ) {
        let Some(terminal_state) = terminal_task_state_from_agent_status(&agent.status) else {
            return;
        };
        let Some(task_id) = agent
            .task_id
            .as_deref()
            .and_then(|id| id.parse::<TaskId>().ok())
        else {
            return;
        };
        let Some(session) = self.find_session_mut(session_id) else {
            return;
        };
        if let Some(task) = session.tasks.iter_mut().find(|task| task.id == task_id) {
            if matches!(task_state_label(task.state), "pending" | "running") {
                task.state = terminal_state;
            }
        }
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
            // C5: keep the originating turn so the chip can attribute this task
            // per-turn. Never clobber a known turn with a later turn-less update
            // (synthetic / replay emitters send `turn_id: None`).
            if event.turn_id.is_some() {
                task.turn_id = event.turn_id;
            }
        } else {
            session.tasks.push(TaskView {
                id: event.task_id,
                title: event.title,
                state: event.state,
                runtime_detail: event.runtime_detail,
                output_tail: String::new(),
                turn_id: event.turn_id,
            });
        }
    }

    fn apply_task_output(&mut self, event: TaskOutputDeltaEvent) {
        let TaskOutputDeltaEvent {
            session_id,
            task_id,
            cursor,
            text,
            ..
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
        // Idempotence vs a turn-switch: if this turn was ALREADY finalized
        // (committed or dropped) by `commit_pending_live_reply_for_turn_switch`,
        // its late terminal is a no-op. Without this the late terminal would hit
        // the `None` arm (when its successor already completed) and emit a FALSE
        // "did not receive a final assistant answer" fallback for a turn that
        // committed cleanly, or be swallowed by the `Some(mismatched)` arm —
        // either way mishandling the already-closed turn. Consume the marker so
        // a genuine future terminal for the same turn id is unaffected.
        //
        // A pending AskUserQuestion picker for this terminal turn is now stale —
        // clear it BEFORE the finalized-by-switch early return, so a late
        // terminal for an already-finalized turn still dismisses the picker
        // instead of leaving it wedged (nit).
        self.clear_question_for_turn(&event.session_id, &event.turn_id);
        if self
            .state
            .take_turn_finalized_by_switch(&event.session_id, &event.turn_id)
        {
            return None;
        }
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
                    let text = finalize_live_reply_text(
                        live_reply.text,
                        complete_live_plan,
                        &fallback_summary,
                        &partial_fallback_summary,
                    );
                    session.messages.push(Message::assistant(text));
                    (
                        t!("status.turn_completed", title = title, seq = seq).into_owned(),
                        true,
                        true,
                    )
                }
                Some(live_reply) => {
                    session.live_reply = Some(live_reply);
                    (
                        t!("status.turn_completed_stale", title = title).into_owned(),
                        false,
                        false,
                    )
                }
                None => (
                    {
                        session.messages.push(Message::assistant(fallback_summary));
                        t!("status.turn_completed", title = title, seq = seq).into_owned()
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
            // Blocking bug 2: terminal completion clears any stale retry/backoff
            // for the session. `session_retry` was only cleared on the next
            // non-retry PROGRESS event, so a retry immediately followed by
            // `TurnCompleted` left a stale entry that could render "retrying" on
            // a LATER active orchestration. Terminal = no retry in flight.
            //
            // Over-clear fix: only clear when this terminal applies to the LIVE
            // turn. A STALE `TurnCompleted` (mismatched turn_id) preserves the
            // live reply above, so it must also preserve the live turn's retry.
            self.state.session_retry.remove(&event.session_id);
            self.state
                .capture_completed_turn_activity(&event.session_id, &event.turn_id);
            self.state.set_run_state_success();
        }
        self.submit_next_pending_if_idle()
    }

    fn fail_live_reply(&mut self, event: TurnErrorEvent) -> Option<AppUiCommand> {
        // Idempotence vs a turn-switch (see `commit_live_reply`): the
        // finalized-by-switch marker suppresses only a false COMPLETION
        // fallback — it must NEVER hide a real ERROR. A turn finalized at a
        // switch boundary (its text already committed, or it was an empty turn
        // that was dropped) can still genuinely ERROR afterwards, and that
        // failure must be surfaced (failure card + run-state Error), even though
        // its partial text may already stand. So we CONSUME the marker for
        // cleanup (so it cannot leak and mishandle a later same-id event) but
        // then FALL THROUGH to the normal fail-path arms rather than
        // early-returning `None`. Both orderings are handled below: B already
        // completed (live_reply == None → the `None` arm pushes the failure
        // card) and B still live (the `Some(mismatched)` arm preserves B's
        // live_reply but still surfaces A's failure via the run-state error).
        let was_finalized_by_switch = self
            .state
            .take_turn_finalized_by_switch(&event.session_id, &event.turn_id);
        // A turn error cancels any pending AskUserQuestion picker for this turn
        // (design §4.2: the turn-interrupt/error path is Phase-1's cancellation).
        self.clear_question_for_turn(&event.session_id, &event.turn_id);
        let follow_tail = self.state.transcript_scroll == 0;
        let fallback_summary =
            self.turn_error_fallback_message(&event.turn_id, &event.code, &event.message);
        let Some(session) = self.find_session_mut(&event.session_id) else {
            return None;
        };
        let title = session.title.clone();
        // `failed_current_turn`: this error terminates the LIVE turn — it owns
        // the retry-clear and the live-turn run-state transition.
        // `surfaced_failure`: a failure card was pushed for this error. These
        // coincide except for a switch-finalized turn whose error arrives while
        // a DIFFERENT successor turn is still live: there we surface the failure
        // (card + error run-state) without disturbing the live successor.
        let (status, failed_current_turn, surfaced_failure) = match session.live_reply.take() {
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
                    true,
                )
            }
            Some(live_reply) if was_finalized_by_switch => {
                // A switch-finalized turn (A) errors while a different successor
                // turn (B) is still live: surface A's failure card but PRESERVE
                // B's in-flight live_reply untouched — the error is A's, not B's.
                // A's (partial) text was already committed at switch time, so
                // the card is the bare error summary (no partial-response tail
                // here; that tail belongs to the live-turn arm above).
                session.messages.push(Message::assistant(fallback_summary));
                session.live_reply = Some(live_reply);
                (
                    format!("Turn error {}: {}", event.code, event.message),
                    false,
                    true,
                )
            }
            Some(live_reply) => {
                session.live_reply = Some(live_reply);
                (
                    format!("Ignored stale turn error in {title}: {}", event.code),
                    false,
                    false,
                )
            }
            None => {
                session.messages.push(Message::assistant(fallback_summary));
                (
                    format!("Turn error {}: {}", event.code, event.message),
                    true,
                    true,
                )
            }
        };
        if surfaced_failure {
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
            // Blocking bug 2: terminal error also clears any stale retry/backoff
            // for the session (see `commit_live_reply`) so it can never linger
            // and render on a later orchestration.
            //
            // Over-clear fix: only clear when this terminal applies to the LIVE
            // turn. A STALE `TurnError` (mismatched turn_id) preserves the live
            // reply above, so it must also preserve the live turn's retry. A
            // switch-finalized turn's late error that surfaces while a different
            // turn is live (`!failed_current_turn`) likewise leaves the live
            // turn's retry alone.
            self.state.session_retry.remove(&event.session_id);
        }
        if surfaced_failure {
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
        let command =
            self.start_prompt_turn(prompt, t!("status.submitted_staged_message").into_owned());
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

    fn find_session(&self, session_id: &octos_core::SessionKey) -> Option<&SessionView> {
        self.state
            .sessions
            .iter()
            .find(|session| &session.id == session_id)
    }

    /// Before binding a fresh `live_reply` for `new_turn`, commit any currently
    /// bound live_reply that belongs to a DIFFERENT turn. This is the
    /// turn-switch boundary for the lazy-bind path: when a session's turns run
    /// sequentially (an original user turn followed by server-initiated
    /// master-continuation turns), each turn must produce its OWN assistant
    /// message — the prior turn's accumulated answer is neither lost nor merged
    /// into the next turn. A prior turn that streamed NO text is dropped
    /// silently (its eventual `TurnCompleted`, if any, is handled by the
    /// fallback path); we only persist a prior turn that actually produced a
    /// visible answer.
    ///
    /// Activity vs. assistant-message decision (DO-NOT-SHIP #2): the assistant
    /// MESSAGE and the tool ACTIVITY are two independent artifacts. A
    /// switch-dropped EMPTY continuation turn produces NO assistant message and
    /// NO "did not receive a final assistant answer" fallback card by design (it
    /// was superseded by `new_turn`). But its tool activity must NOT be lost: we
    /// capture it into `turn_activity_logs` (the chip source) on BOTH the
    /// non-empty and the empty-drop path, identically to the live commit path.
    /// `capture_completed_turn_activity` archives the turn's items out of the
    /// live `activity` flow (which is filtered to the *active* turn, so once
    /// `new_turn` is live the prior turn's items would otherwise be orphaned —
    /// invisible — until a late terminal that, being a no-op for a marked turn,
    /// never captures them). Capturing here keeps the dropped-empty turn's
    /// activity chip visible without re-introducing the false-completion card.
    fn commit_pending_live_reply_for_turn_switch(
        &mut self,
        session_id: &octos_core::SessionKey,
        new_turn: &TurnId,
    ) {
        let prior_turn = match self.find_session(session_id).and_then(|session| {
            session
                .live_reply
                .as_ref()
                .filter(|live_reply| &live_reply.turn_id != new_turn)
                .map(|live_reply| live_reply.turn_id.clone())
        }) {
            Some(turn_id) => turn_id,
            None => return,
        };
        // The switch finalizes the prior turn here (committed if non-empty,
        // dropped if empty). Record it so its OWN late `TurnCompleted`/
        // `TurnError` — which may still arrive on an out-of-order stream — is
        // recognized as already-handled and no-ops instead of emitting a false
        // fallback card or mishandling the dropped-empty case.
        self.state
            .mark_turn_finalized_by_switch(session_id, &prior_turn);
        let complete_live_plan = self.turn_had_completion_activity(&prior_turn);
        let fallback_summary = self.turn_completion_fallback_message(&prior_turn);
        let partial_fallback_summary = self.turn_partial_completion_fallback_message(&prior_turn);
        let follow_tail = self.state.transcript_scroll == 0;
        let mut committed = false;
        if let Some(session) = self.find_session_mut(session_id) {
            // Drop a prior turn that streamed NO visible text: its eventual
            // terminal event (if it arrives) handles the empty/fallback case;
            // only persist a prior turn that produced a real answer.
            if let Some(live_reply) = session
                .live_reply
                .take()
                .filter(|live_reply| !live_reply.text.trim().is_empty())
            {
                let text = finalize_live_reply_text(
                    live_reply.text,
                    complete_live_plan,
                    &fallback_summary,
                    &partial_fallback_summary,
                );
                session.messages.push(Message::assistant(text));
                committed = true;
            }
        }
        // Capture the prior turn's tool activity on BOTH paths (committed
        // non-empty AND dropped empty) so a switch-dropped empty turn's chip
        // stays visible — see the method doc. This is a no-op when the turn ran
        // no activity (`capture_completed_turn_activity` returns early), so the
        // genuinely-empty no-activity case is unaffected.
        self.state
            .capture_completed_turn_activity(session_id, &prior_turn);
        if committed {
            if follow_tail {
                self.state.scroll_transcript_to_latest();
            } else {
                self.state.preserve_transcript_position_after_append(3);
            }
        }
    }

    fn turn_completion_fallback_message(&self, turn_id: &TurnId) -> String {
        let summary = self.summarize_turn_activity(turn_id);
        t!(
            "status.summary_completed_no_answer",
            count = summary.action_count,
            files =
                format_limited_list(&summary.files_changed, &t!("status.summary_none_observed")),
            validation =
                format_limited_list(&summary.validation, &t!("status.summary_not_reported")),
        )
        .into_owned()
    }

    fn turn_partial_completion_fallback_message(&self, turn_id: &TurnId) -> String {
        let summary = self.summarize_turn_activity(turn_id);
        t!(
            "status.summary_partial_answer",
            count = summary.action_count,
            files =
                format_limited_list(&summary.files_changed, &t!("status.summary_none_observed")),
            validation =
                format_limited_list(&summary.validation, &t!("status.summary_not_reported")),
        )
        .into_owned()
    }

    fn turn_error_fallback_message(&self, turn_id: &TurnId, code: &str, message: &str) -> String {
        let summary = self.summarize_turn_activity(turn_id);
        let failed = format_limited_list(&summary.failures, &t!("status.summary_none_recorded"));
        t!(
            "status.summary_failed",
            code = code,
            message = message,
            count = summary.action_count,
            failed = failed,
        )
        .into_owned()
    }

    fn summarize_turn_activity(&self, turn_id: &TurnId) -> TurnActivitySummary {
        let mut summary = TurnActivitySummary::default();
        // Count from where the turn's items actually live. A switch-finalized
        // turn has ALREADY had `capture_completed_turn_activity` move its items
        // out of the live `state.activity` into `turn_activity_logs`, so the
        // live set is empty for it — summarizing live would report "0 action(s)"
        // for a turn that genuinely ran N. Prefer the archived log when present;
        // otherwise fall back to live activity (the normal in-turn path, whose
        // items have NOT yet been archived, is therefore unchanged).
        let archived = self
            .state
            .turn_activity_logs
            .iter()
            .find(|log| &log.turn_id == turn_id);
        let items: &[ActivityItem] = match archived {
            Some(log) => &log.items,
            None => &self.state.activity,
        };
        for activity in items
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

    /// Phase-1 AskUserQuestion has no dedicated `user_question/cancelled`
    /// notification: a turn interrupt/error that terminates the paused turn is
    /// the cancellation signal (design §4.2). When a turn terminates we drop any
    /// pending picker bound to that turn so a stale question never wedges the UI.
    fn clear_question_for_turn(&mut self, session_id: &SessionKey, turn_id: &TurnId) -> bool {
        let matches =
            self.state.user_question.as_ref().is_some_and(|picker| {
                &picker.session_id == session_id && &picker.turn_id == turn_id
            });
        if matches {
            self.state.user_question = None;
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
        0 => t!("status.hint_registered_command").into_owned(),
        1 => names[0].clone(),
        2 => format!("{} or {}", names[0], names[1]),
        _ => {
            let last = names.last().expect("non-empty command names");
            format!("{}, or {last}", names[..names.len() - 1].join(", "))
        }
    }
}

/// M22-E: extract a user-facing failure reason from a failed
/// provider mutation event (test or save). Prefers
/// `result.error.message` when the server provides structured
/// detail; falls back to the bare message otherwise. The reason
/// MUST NOT include the raw API key — `ProfileLlmMutationResult`
/// is already redacted by the server but we double-check here by
/// stripping any value that matches the staged secret.
fn provider_failure_reason(
    event: &ProfileLlmMutationClientEvent,
    staged_secret: Option<&crate::model::SecretString>,
) -> String {
    let raw = event
        .result
        .error
        .as_deref()
        .map(str::trim)
        .filter(|err| !err.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            event
                .result
                .message
                .as_deref()
                .map(str::trim)
                .filter(|msg| !msg.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| event.message.clone());
    // M22-E: belt-and-suspenders for the redaction contract.
    // Even though `ProfileLlmMutationResult` is server-redacted,
    // a misbehaving or future-regressing backend could echo the
    // staged key. Strip any literal match before storing.
    match staged_secret {
        Some(secret) if !secret.is_empty() => {
            let exposed = secret.expose_for_transport();
            if raw.contains(exposed) {
                raw.replace(exposed, "********")
            } else {
                raw
            }
        }
        _ => raw,
    }
}

/// M22-D: parse a `/onboard permissions <mode>` token into a typed
/// `PermissionProfileUpdate`. Accepted modes mirror the labels in
/// `permission_profile_items` so the onboarding step and the
/// `/permissions` menu speak the same vocabulary. Returns `Ok(None)`
/// when the user passed `clear`/`reset`/empty to drop the staged
/// choice.
fn parse_onboarding_permission_mode(
    raw: &str,
) -> Result<Option<octos_core::ui_protocol::PermissionProfileUpdate>, String> {
    use octos_core::ui_protocol::{
        PermissionNetworkPolicy, PermissionProfileMode, PermissionProfileUpdate,
    };
    let token = raw.trim().to_ascii_lowercase();
    if token.is_empty() || matches!(token.as_str(), "clear" | "reset" | "none") {
        return Ok(None);
    }
    let update = match token.as_str() {
        "default" => PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::WorkspaceWrite),
            network: Some(PermissionNetworkPolicy::Deny),
            approval_policy: Some("on-request".into()),
        },
        "read-only" | "read_only" | "readonly" => PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::ReadOnly),
            network: None,
            approval_policy: Some("on-request".into()),
        },
        "workspace-write" | "workspace_write" | "ws-write" => PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::WorkspaceWrite),
            network: None,
            approval_policy: Some("on-request".into()),
        },
        "workspace-write-never" | "workspace_write_never" | "ws-write-never" => {
            PermissionProfileUpdate {
                mode: Some(PermissionProfileMode::WorkspaceWrite),
                network: Some(PermissionNetworkPolicy::Deny),
                approval_policy: Some("never".into()),
            }
        }
        "danger-full-access" | "danger_full_access" | "full-access" | "full_access" => {
            PermissionProfileUpdate {
                mode: Some(PermissionProfileMode::DangerFullAccess),
                network: Some(PermissionNetworkPolicy::Allow),
                approval_policy: Some("never".into()),
            }
        }
        other => {
            return Err(t!("status.unknown_permission_profile_mode", mode = other).into_owned());
        }
    };
    Ok(Some(update))
}

/// M22-D: compare a `PermissionProfileUpdate` against the server-
/// effective fields in a `RuntimePolicyStamp`. Returns a typed
/// mismatch reason when the server clamped or rejected the staged
/// choice, `None` when the stamp matches what the user asked for.
fn permission_profile_stamp_mismatch(
    staged: &octos_core::ui_protocol::PermissionProfileUpdate,
    stamp: &crate::model::RuntimePolicyStamp,
) -> Option<String> {
    use octos_core::ui_protocol::PermissionProfileMode;
    let mut mismatches: Vec<String> = Vec::new();
    if let Some(mode) = staged.mode {
        let expected = match mode {
            PermissionProfileMode::ReadOnly => "read_only",
            PermissionProfileMode::WorkspaceWrite => "workspace_write",
            PermissionProfileMode::DangerFullAccess => "danger_full_access",
        };
        let actual = stamp.permission_profile.as_deref().unwrap_or("");
        // Tolerate aliasing (server may publish kebab-case).
        let actual_normalized = actual.replace('-', "_");
        if !actual_normalized.eq_ignore_ascii_case(expected) {
            mismatches.push(format!(
                "permission_profile: staged {expected}, server effective {}",
                if actual.is_empty() { "(unset)" } else { actual }
            ));
        }
    }
    if let Some(approval) = staged.approval_policy.as_deref() {
        let actual = stamp.approval_policy.as_deref().unwrap_or("");
        let staged_norm = approval.replace('_', "-");
        let actual_norm = actual.replace('_', "-");
        if !actual_norm.eq_ignore_ascii_case(&staged_norm) {
            mismatches.push(format!(
                "approval_policy: staged {approval}, server effective {}",
                if actual.is_empty() { "(unset)" } else { actual }
            ));
        }
    }
    if let Some(network) = staged.network {
        // Backend publishes `allowed`/`blocked` (past-tense) in
        // the stamp, while the request shape uses `allow`/`deny`.
        // Accept both spellings so a correctly-applied policy
        // never reads as clamped.
        let expected_aliases: &[&str] = match network {
            octos_core::ui_protocol::PermissionNetworkPolicy::Allow => {
                &["allow", "allowed", "network_allowed", "network-allowed"]
            }
            octos_core::ui_protocol::PermissionNetworkPolicy::Deny => &[
                "deny",
                "denied",
                "blocked",
                "network_blocked",
                "network-blocked",
            ],
        };
        let actual = stamp.network.as_deref().unwrap_or("");
        if !actual.is_empty()
            && !expected_aliases
                .iter()
                .any(|alias| actual.eq_ignore_ascii_case(alias))
        {
            mismatches.push(format!(
                "network: staged {}, server effective {actual}",
                expected_aliases[0]
            ));
        }
    }
    if mismatches.is_empty() {
        None
    } else {
        Some(mismatches.join("; "))
    }
}

fn onboarding_pending_status(pending: OnboardingProviderPending) -> String {
    match pending {
        OnboardingProviderPending::Test => t!("status.provider_test_in_progress").into_owned(),
        OnboardingProviderPending::Save => t!("status.provider_save_in_progress").into_owned(),
    }
}

/// M22-B: structured error codes the backend may publish for a
/// failing `profile/local/create`. The transport layer surfaces these
/// via `AppUiError::code` (preferring `data.kind` over the numeric
/// JSON-RPC code) so the TUI can attribute them back to the profile
/// step. `apply_local_profile_error` maps each one to the offending
/// field and a typed recovery message.
fn is_local_create_error_code(code: &str) -> bool {
    matches!(
        code,
        "profile_local_collision"
            | "profile_local_unsupported"
            | "profile_local_invalid_name"
            | "profile_local_invalid_username"
            | "profile_local_invalid_email"
    )
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

/// Resolve the workspace cwd to seed the onboarding candidate from at launch.
///
/// Precedence:
///   1. an explicit, non-empty `--cwd` (always wins, including remote launches
///      where the user named a server-side path on purpose);
///   2. otherwise — only for transport-local launches (stdio / `ws://localhost`),
///      gated by `allow_process_cwd_fallback` — the process working directory
///      via `process_cwd`.
///
/// Returns `None` when neither yields a non-empty path (e.g. a remote launch
/// with no `--cwd`), preserving the prior label/root-based behavior for
/// remote/WS workspaces whose root lives on the server. `process_cwd` is
/// injected so the fallback is deterministic under test.
fn resolve_launch_workspace_cwd(
    explicit: Option<String>,
    allow_process_cwd_fallback: bool,
    process_cwd: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if let Some(explicit) = explicit {
        if !explicit.trim().is_empty() {
            return Some(explicit);
        }
    }
    if !allow_process_cwd_fallback {
        return None;
    }
    process_cwd().filter(|cwd| !cwd.trim().is_empty())
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
    t!("status.usage_onboard").into_owned()
}

fn login_usage() -> String {
    t!("status.usage_login").into_owned()
}

fn provider_usage() -> String {
    t!("status.usage_provider").into_owned()
}

fn skills_usage() -> String {
    t!("status.usage_skills").into_owned()
}

fn mcp_usage() -> String {
    t!("status.usage_mcp").into_owned()
}

fn tools_usage() -> String {
    t!("status.usage_tools").into_owned()
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
                return Err(t!("status.usage_skills_install").into_owned());
            };
            branch = Some(value);
        } else if let Some(value) = part.strip_prefix("--branch=") {
            let Some(value) = non_empty_string(value.to_owned()) else {
                return Err(t!("status.usage_skills_install").into_owned());
            };
            branch = Some(value);
        } else if part.starts_with('-') {
            return Err(t!("status.unknown_skills_install_flag", flag = part).into_owned());
        } else if repo.is_none() {
            repo = Some(part.to_owned());
        } else {
            return Err(t!("status.usage_skills_install").into_owned());
        }
    }

    let Some(repo) = repo.and_then(non_empty_string) else {
        return Err(t!("status.usage_skills_install").into_owned());
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

enum HydratedProjection {
    Message(HydratedMessage),
    SpawnComplete(TurnSpawnCompleteEvent),
}

impl HydratedProjection {
    fn seq(&self) -> u64 {
        match self {
            Self::Message(message) => message.seq,
            Self::SpawnComplete(event) => event.seq,
        }
    }
}

fn hydrated_projection_messages(result: &SessionHydrateResult) -> Option<Vec<Message>> {
    let rows = result.messages.as_ref()?;
    let envelopes = result.replayed_envelopes.as_deref().unwrap_or_default();
    let envelope_message_ids = envelopes
        .iter()
        .map(|event| event.message_id.clone())
        .collect::<BTreeSet<_>>();

    let mut projections = rows
        .iter()
        .filter(|row| !hydrated_row_is_covered_by_envelope(row, envelopes, &envelope_message_ids))
        .filter(|row| hydrated_row_is_displayable(row))
        .cloned()
        .map(HydratedProjection::Message)
        .collect::<Vec<_>>();
    projections.extend(
        envelopes
            .iter()
            .cloned()
            .map(HydratedProjection::SpawnComplete),
    );
    projections.sort_by_key(HydratedProjection::seq);
    Some(
        projections
            .into_iter()
            .map(|projection| match projection {
                HydratedProjection::Message(row) => hydrated_row_to_message(row),
                HydratedProjection::SpawnComplete(event) => spawn_complete_to_message(event),
            })
            .collect(),
    )
}

/// Whether a hydrated message row should render as a transcript bubble. The
/// live transcript only commits user + assistant *answer* messages; the
/// intermediate turn machinery — tool-result rows and tool-call-only assistant
/// rows (empty text) — is surfaced as activity chips, not chat bubbles. Hydrate
/// must match, otherwise a reconnect double-renders the turn (e.g. a tool's raw
/// output bubble AND the assistant's formatted answer — the "repeat" bug seen on
/// the mini5 soak: a tool turn went 2 live msgs -> 4 on reconnect).
fn hydrated_row_is_displayable(row: &HydratedMessage) -> bool {
    match row.role.as_str() {
        "tool" => false,
        "assistant" => !row.content.trim().is_empty(),
        _ => true,
    }
}

fn hydrated_row_is_covered_by_envelope(
    row: &HydratedMessage,
    envelopes: &[TurnSpawnCompleteEvent],
    envelope_message_ids: &BTreeSet<String>,
) -> bool {
    if row
        .message_id
        .as_ref()
        .is_some_and(|message_id| envelope_message_ids.contains(message_id))
    {
        return true;
    }
    if row.source.as_deref() != Some("background") {
        return false;
    }
    let Some(thread_id) = row.thread_id.as_deref() else {
        return false;
    };
    envelopes.iter().any(|event| {
        event.thread_id.as_deref() == Some(thread_id)
            && row.seq < event.seq
            && row
                .message_id
                .as_ref()
                .is_none_or(|message_id| !envelope_message_ids.contains(message_id))
    })
}

fn hydrated_row_to_message(row: HydratedMessage) -> Message {
    Message {
        role: hydrated_role(&row.role),
        content: row.content,
        media: row.media,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        client_message_id: row.client_message_id,
        thread_id: row.thread_id,
        timestamp: row.persisted_at,
    }
}

fn spawn_complete_to_message(event: TurnSpawnCompleteEvent) -> Message {
    let mut message = match event.thread_id {
        Some(thread_id) => Message::assistant_with_thread(event.content, ThreadId::new(thread_id)),
        None => Message::assistant(event.content),
    };
    message.media = event.media;
    message.timestamp = event.persisted_at;
    message
}

fn hydrated_role(role: &str) -> MessageRole {
    match role {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        "system" => MessageRole::System,
        _ => MessageRole::System,
    }
}

fn context_lifecycle_state_from_ui(state: &UiContextState) -> crate::model::ContextLifecycleState {
    crate::model::ContextLifecycleState {
        session_id: state.session_id.clone(),
        thread_id: state.thread_id.clone(),
        generation: state.generation,
        transcript_hash: state.transcript_hash.clone(),
        item_count: state.item_count,
        token_estimate: state.token_estimate,
        recovery_state: state.recovery_state.clone(),
        last_checkpoint_id: state.last_checkpoint_id.clone(),
        last_compaction_id: state.last_compaction_id.clone(),
    }
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
        return Some(t!("status.recovery_npm_dns").into_owned());
    }

    if output.contains("command timed out") {
        return Some(t!("status.recovery_command_timeout").into_owned());
    }

    if output.contains("permission denied")
        || output.contains("operation not permitted")
        || output.contains("eacces")
    {
        return Some(t!("status.recovery_permission_blocked").into_owned());
    }

    if output.contains("could not resolve host")
        || output.contains("network is unreachable")
        || output.contains("network request")
        || output.contains("timeout")
    {
        return Some(t!("status.recovery_network_failed").into_owned());
    }

    if matches!(tool_name, "web_search" | "web_fetch" | "deep_search")
        && (output.contains("restricted") || output.contains("not configured"))
    {
        return Some(t!("status.recovery_search_restricted").into_owned());
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
    let mut text = t!(
        "status.diff_hunk_prompt",
        path = path,
        status = context.file_status,
        hunk = context.hunk_header
    )
    .into_owned();
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
        // Persona spinner words (`progress/updated{kind:"status_word"}`,
        // octos-core progress_kinds::STATUS_WORD) belong in the status line, not
        // the activity log. The words themselves are dynamic (LLM-generated), so
        // filter on the stable `kind`, never the word — otherwise the chip counts
        // "Composing/Contemplating/…" as fake "active actions" with no real work.
        "status_word"
            | "thinking"
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
        ApprovalDiffDetails, ApprovalId, ApprovalRequestedEvent, ApprovalTypedDetails, Envelope,
        EnvelopeNotification, HydratedMessage, HydratedTurn, OutputCursor, Payload, PreviewId,
        ReplayLossyEvent, SessionHydrateResult, TaskRuntimeState, ThreadGraphEntry,
        ToolCompletedEvent, ToolStartedEvent, TurnId, TurnLifecycleState, TurnSpawnCompleteEvent,
        TurnStartedEvent, UiContextState, UiCursor, UiFileMutationNotice, UiProgressMetadata,
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
                turn_id: None,
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

    fn store_with_assistant_message(text: &str) -> Store {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![Message::user("prompt"), Message::assistant(text)],
            tasks: vec![],
            live_reply: None,
        };
        Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        }
    }

    #[test]
    fn copy_last_reply_stages_assistant_text_for_clipboard() {
        let mut store = store_with_assistant_message("the deep-research report");
        assert!(store.state.pending_clipboard.is_none());

        store.copy_last_reply();

        assert_eq!(
            store.state.pending_clipboard.as_deref(),
            Some("the deep-research report")
        );
        assert!(
            store
                .state
                .status
                .contains(t!("status.copied_last_reply", chars = 24).as_ref()),
            "status should confirm the copy, got: {}",
            store.state.status
        );
    }

    #[test]
    fn copy_last_reply_reports_when_there_is_nothing_to_copy() {
        let mut store = store_with_empty_session();

        store.copy_last_reply();

        assert!(store.state.pending_clipboard.is_none());
        assert!(
            store
                .state
                .status
                .contains(t!("status.nothing_to_copy").as_ref()),
            "status should explain the no-op, got: {}",
            store.state.status
        );
    }

    #[test]
    fn copy_command_dispatch_stages_clipboard() {
        let mut store = store_with_assistant_message("final answer");

        let command =
            store.dispatch_local_action(crate::menu::types::LocalAction::CopyLastReply, None);

        assert!(command.is_none(), "/copy is local-only, sends no command");
        assert_eq!(
            store.state.pending_clipboard.as_deref(),
            Some("final answer")
        );
    }

    /// `/lang` with no/unknown code must NOT mutate the process-global locale
    /// (which would flake every other test that renders English). The success
    /// path (which calls `set_locale`) is covered by the lib's i18n_tests +
    /// the live `--lang`/`/lang` smoke, not here, to keep tests isolated.
    #[test]
    fn lang_command_usage_and_unknown_do_not_switch_locale() {
        let before = rust_i18n::locale().to_string();
        let mut store = store_with_empty_session();

        // Empty arg now opens the language selection menu (no inline switch).
        store.dispatch_set_language("");
        assert_eq!(rust_i18n::locale().to_string(), before);

        store.dispatch_set_language("klingon");
        assert!(
            store.state.status.contains("klingon"),
            "unknown code should be echoed, got: {}",
            store.state.status
        );
        assert_eq!(rust_i18n::locale().to_string(), before);
    }

    #[test]
    fn onboarding_language_step_is_first() {
        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        ]));
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));

        let root = store
            .state
            .active_menu
            .as_ref()
            .expect("onboarding menu is built");
        let MenuBuildResult::Ready(spec) = root else {
            panic!("expected ready onboarding menu");
        };
        assert_eq!(
            spec.items.first().map(|item| item.id.as_str()),
            Some("onboard.language"),
            "language must be the first onboarding row"
        );
    }

    #[test]
    fn onboarding_language_selection_sets_zh_locale_in_child_process() {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary path"))
            .args([
                "--exact",
                "store::tests::onboarding_language_selection_child_sets_zh_locale",
                "--ignored",
                "--test-threads=1",
            ])
            .output()
            .expect("run child locale-selection test");
        assert!(
            output.status.success(),
            "child locale-selection test failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "spawned by onboarding_language_selection_sets_zh_locale_in_child_process"]
    fn onboarding_language_selection_child_sets_zh_locale() {
        rust_i18n::set_locale("en");

        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        ]));
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));

        assert!(store.accept_active_menu_item().is_none());
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_ONBOARD_LANGUAGE)
        );

        store.select_next_menu_item();
        let language_menu = store
            .state
            .active_menu
            .as_ref()
            .expect("language menu is built");
        let MenuBuildResult::Ready(spec) = language_menu else {
            panic!("expected ready language menu");
        };
        assert_eq!(
            spec.items.get(1).map(|item| item.id.as_str()),
            Some("onboard.language.zh"),
            "zh should be the second available onboarding language"
        );

        assert!(store.accept_active_menu_item().is_none());
        assert_eq!(rust_i18n::locale().to_string(), "zh");
    }

    #[test]
    fn set_theme_applies_and_marks_current() {
        use crate::cli::ThemeName;
        let mut store = store_with_empty_session();

        // Default runtime theme is Codex (the event loop derives the per-frame
        // palette from `state.theme`, so this is what actually paints).
        assert_eq!(store.state.theme, ThemeName::Codex);

        // Selecting a theme applies it to runtime state and echoes the choice.
        store.dispatch_local_action(LocalAction::SetTheme("claude".into()), None);
        assert_eq!(store.state.theme, ThemeName::Claude);
        assert!(
            store.state.status.contains("claude"),
            "status should echo the theme, got: {}",
            store.state.status
        );

        // An unknown id leaves the active theme intact and reports it.
        store.dispatch_local_action(LocalAction::SetTheme("nope".into()), None);
        assert_eq!(store.state.theme, ThemeName::Claude);
        assert!(
            store.state.status.to_lowercase().contains("unknown"),
            "unknown theme should be reported, got: {}",
            store.state.status
        );

        // The /theme menu marks the active theme as current (drives the `*`).
        store.open_menu(MenuId::from(crate::menu::registry::MENU_THEME));
        let MenuBuildResult::Ready(spec) =
            store.state.active_menu.as_ref().expect("theme menu open")
        else {
            panic!("theme menu should build Ready");
        };
        let current_of = |id: &str| {
            spec.items
                .iter()
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("{id} item present"))
                .state
                .current
        };
        assert!(
            current_of("claude"),
            "active theme should be marked current"
        );
        assert!(!current_of("codex"), "non-active theme not current");

        // The active theme survives a snapshot replay (reconnect/refresh): the
        // server never echoes it, so the client must preserve it locally.
        let sessions = store.state.sessions.clone();
        store.apply_event(AppUiEvent::Snapshot(AppUiSnapshot {
            sessions,
            selected_session: 0,
            status: "snapshot replay".into(),
            target: None,
            readonly: false,
        }));
        assert_eq!(
            store.state.theme,
            ThemeName::Claude,
            "theme must survive snapshot replay"
        );
    }

    #[test]
    fn thinking_command_sets_clears_and_rejects_unknown() {
        use octos_core::ui_protocol::ReasoningEffortLevel as L;
        let mut store = store_with_empty_session();
        let key = store.active_session().expect("active session").id.clone();
        assert!(store.state.session_reasoning_effort.get(&key).is_none());

        store.dispatch_set_thinking("high");
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::High)
        );
        store.dispatch_set_thinking("max");
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::Max)
        );

        // `default` clears the override (server default applies).
        store.dispatch_set_thinking("default");
        assert!(store.state.session_reasoning_effort.get(&key).is_none());

        // Unknown arg is echoed and does not change the current level.
        store.dispatch_set_thinking("high");
        store.dispatch_set_thinking("bogus");
        assert!(
            store.state.status.contains("bogus"),
            "unknown effort should be echoed, got: {}",
            store.state.status
        );
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::High)
        );

        // Empty arg opens the selection menu and does NOT change the level.
        store.dispatch_set_thinking("");
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::High)
        );

        // Per-session: the level is keyed by SessionKey, not global.
        let other = SessionKey("local:other".into());
        assert!(store.state.session_reasoning_effort.get(&other).is_none());
    }

    #[test]
    fn thinking_menu_action_sets_and_clears_level() {
        // The /thinking selection menu dispatches SetThinkingLevel directly.
        use octos_core::ui_protocol::ReasoningEffortLevel as L;
        let mut store = store_with_empty_session();
        let key = store.active_session().expect("active session").id.clone();

        store.dispatch_set_thinking_level(Some(L::Max));
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::Max)
        );
        assert!(store.state.status.contains("max"));

        // The "Default" menu item clears the override.
        store.dispatch_set_thinking_level(None);
        assert!(store.state.session_reasoning_effort.get(&key).is_none());
    }

    #[test]
    fn thinking_level_survives_snapshot_replay() {
        use octos_core::ui_protocol::ReasoningEffortLevel as L;
        let mut store = store_with_empty_session();
        let key = store.active_session().expect("active session").id.clone();
        store.dispatch_set_thinking("max");
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::Max)
        );

        // A snapshot replay (reconnect/refresh) rebuilds AppState from scratch;
        // the local-only /thinking level must be preserved (codex P1).
        store.apply_event(AppUiEvent::Snapshot(AppUiSnapshot {
            sessions: vec![SessionView {
                id: key.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            selected_session: 0,
            status: "replayed".into(),
            target: None,
            readonly: false,
        }));
        assert_eq!(
            store.state.session_reasoning_effort.get(&key),
            Some(&L::Max),
            "/thinking level must survive a snapshot replay"
        );
    }

    fn store_with_two_sessions(first: &str, second: &str) -> Store {
        let make = |id: &str| SessionView {
            id: SessionKey(id.into()),
            title: id.into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        Store {
            state: AppState::new(
                vec![make(first), make(second)],
                0,
                "ready".into(),
                None,
                false,
            ),
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
                "Octos UI connected".into(),
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
        assert_eq!(store.state.status, t!("status.testing_provider_connection"));

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
        // M22-C: pre-set workspace validation so the legacy test
        // does not exercise the new filesystem probe.
        store.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/workspace".into(),
                writable: true,
                has_workspace_toml: false,
            };

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
        // M22-C: pre-set workspace validation; this test focuses on
        // cwd extraction from a stdio target string and not on the
        // new filesystem probe.
        store.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/octos/workspace".into(),
                writable: true,
                has_workspace_toml: false,
            };

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
        // M22-C: same as above — pre-set validation so the existing
        // cwd extraction coverage stays focused.
        store.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/octos/workspace dir".into(),
                writable: true,
                has_workspace_toml: false,
            };

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
        // M22-C: pre-validated workspace so the open-session path
        // can proceed; this test predates the workspace probe.
        store.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/alice-workspace".into(),
                writable: true,
                has_workspace_toml: false,
            };
        store.state.workspace.root = "/tmp/alice-workspace".into();
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

    /// Regression (mini4 solo-onboarding "can't activate the workspace"): an
    /// explicit launch `--cwd` must seed the onboarding workspace candidate so
    /// the first-launch probe validates the real cwd, not the bogus stdio
    /// transport label. Pre-fix, `workspace.root` was the `stdio:...` label and
    /// the candidate stayed `None`, so `workspace_target` returned the label →
    /// the probe could never validate → `/onboard finish` never sent
    /// `session/open` → the profile runtime never bootstrapped.
    #[test]
    fn launch_cwd_seeds_onboarding_workspace_candidate() {
        let stdio_label = "stdio:/abs/octos serve --stdio --solo --data-dir /d";
        let mut store = Store::from_snapshot(AppUiSnapshot {
            sessions: vec![],
            selected_session: 0,
            status: "ready".into(),
            target: Some(stdio_label.into()),
            readonly: false,
        });

        // No candidate yet → the onboarding target falls back to the (bogus)
        // transport label that backs `workspace.root`.
        assert!(store.state.onboarding.workspace_candidate.is_none());
        let root = store.state.workspace.root.clone();
        assert_eq!(store.state.onboarding.workspace_target(&root), root.trim());

        // Seeding from the explicit --cwd makes the onboarding target the cwd.
        store.seed_onboarding_workspace_cwd(Some("/Users/cloud/proj".into()));
        assert_eq!(
            store.state.onboarding.workspace_candidate.as_deref(),
            Some("/Users/cloud/proj"),
        );
        let root = store.state.workspace.root.clone();
        assert_eq!(
            store.state.onboarding.workspace_target(&root),
            "/Users/cloud/proj",
        );

        // `get_or_insert`: a second seed does NOT clobber an existing choice
        // (so a later explicit `/onboard workspace <path>` stays authoritative).
        store.seed_onboarding_workspace_cwd(Some("/other".into()));
        assert_eq!(
            store.state.onboarding.workspace_candidate.as_deref(),
            Some("/Users/cloud/proj"),
        );
    }

    /// Absent/empty `--cwd` is a no-op: the candidate stays `None`, so launches
    /// without `--cwd` keep the prior label/root-based workspace behavior
    /// (important for remote/WS workspaces whose root lives on the server).
    #[test]
    fn absent_launch_cwd_leaves_onboarding_candidate_unset() {
        let mut store = Store::from_snapshot(AppUiSnapshot {
            sessions: vec![],
            selected_session: 0,
            status: "ready".into(),
            target: Some("wss://example.test/ui-protocol".into()),
            readonly: false,
        });
        store.seed_onboarding_workspace_cwd(None);
        assert!(store.state.onboarding.workspace_candidate.is_none());
        store.seed_onboarding_workspace_cwd(Some("   ".into()));
        assert!(store.state.onboarding.workspace_candidate.is_none());
    }

    /// UX2 B.1: the documented `octos serve --stdio --solo` launch carries no
    /// `--cwd` and its transport label resolves to `"stdio"`/empty. With the
    /// process-cwd fallback the onboarding candidate is seeded to the launch
    /// directory, so the first-launch probe validates a genuine folder instead
    /// of dead-ending on "no usable workspace cwd".
    #[test]
    fn stdio_launch_without_cwd_falls_back_to_process_cwd() {
        let stdio_label = "stdio:/abs/octos serve --stdio --solo";
        let mut store = Store::from_snapshot(AppUiSnapshot {
            sessions: vec![],
            selected_session: 0,
            status: "ready".into(),
            target: Some(stdio_label.into()),
            readonly: false,
        });
        // Pre-seed deterministically (the public seed reads the real process
        // cwd; the resolver itself is unit-tested below with an injected cwd).
        let resolved = resolve_launch_workspace_cwd(None, true, || Some("/launch/dir".into()));
        store.state.onboarding.workspace_candidate = resolved;
        assert_eq!(
            store.state.onboarding.workspace_candidate.as_deref(),
            Some("/launch/dir"),
        );
        let root = store.state.workspace.root.clone();
        assert_eq!(
            store.state.onboarding.workspace_target(&root),
            "/launch/dir",
        );
    }

    /// UX2 B.1: the real `seed_onboarding_workspace_cwd` path resolves a usable
    /// directory for an stdio launch with no `--cwd`. The seeded candidate must
    /// be a genuine, existing directory so the probe would validate it (here we
    /// just assert it is non-empty and exists, since the process cwd at test
    /// time is the crate root).
    #[test]
    fn seed_without_cwd_resolves_a_real_directory_for_local_transport() {
        let mut store = Store::from_snapshot(AppUiSnapshot {
            sessions: vec![],
            selected_session: 0,
            status: "ready".into(),
            target: Some("stdio:/abs/octos serve --stdio --solo".into()),
            readonly: false,
        });
        store.seed_onboarding_workspace_cwd(None);
        let candidate = store
            .state
            .onboarding
            .workspace_candidate
            .clone()
            .expect("stdio launch without --cwd seeds the process cwd");
        assert!(!candidate.trim().is_empty());
        assert!(
            std::path::Path::new(&candidate).is_dir(),
            "seeded candidate must be an existing directory: {candidate}"
        );
    }

    /// UX2 B.1: an explicit `--cwd` override always wins, even over the
    /// process-cwd fallback, and a remote transport without `--cwd` still
    /// resolves to nothing (the server owns the workspace root).
    #[test]
    fn resolve_launch_workspace_cwd_precedence() {
        // Explicit override wins regardless of fallback availability.
        assert_eq!(
            resolve_launch_workspace_cwd(Some("/explicit".into()), true, || Some(
                "/fallback".into()
            )),
            Some("/explicit".into()),
        );
        // Blank explicit falls through to the process cwd for local transports.
        assert_eq!(
            resolve_launch_workspace_cwd(Some("   ".into()), true, || Some("/fallback".into())),
            Some("/fallback".into()),
        );
        // No explicit + local transport → process cwd.
        assert_eq!(
            resolve_launch_workspace_cwd(None, true, || Some("/fallback".into())),
            Some("/fallback".into()),
        );
        // Remote transport (fallback disallowed) + no explicit → None.
        assert_eq!(
            resolve_launch_workspace_cwd(None, false, || Some("/fallback".into())),
            None,
        );
        // Remote transport + explicit cwd still honors the explicit path.
        assert_eq!(
            resolve_launch_workspace_cwd(Some("/explicit".into()), false, || None),
            Some("/explicit".into()),
        );
        // Empty process cwd is treated as no fallback.
        assert_eq!(
            resolve_launch_workspace_cwd(None, true, || Some("  ".into())),
            None,
        );
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
        // M22-C: pre-validated workspace lets the auto-finish path
        // after profile/local/create proceed without exercising the
        // new filesystem probe in this regression test.
        store.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/solo-project".into(),
                writable: true,
                has_workspace_toml: false,
            };

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
    fn provider_step_opens_on_model_family_after_local_profile_create() {
        // First launch advertises local-solo profile create, which auto-opens
        // the onboarding wizard on the local-profile (Step 1) screen.
        let mut store = protocol_store_without_sessions();
        store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[
                        crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
                        crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
                    ],
                    &[],
                ),
            },
            message: "Octos UI capabilities refreshed".into(),
        }));

        // The user fills the fields and lands the cursor on the final
        // "Continue" row (the create action) before pressing Enter. That row's
        // index in the local-profile menu happens to line up with "API key" in
        // the provider menu — which is the bug we are guarding against.
        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected local-profile onboarding menu");
        };
        let create_index = spec
            .items
            .iter()
            .position(|item| item.id == "onboard.local.create")
            .expect("local create row");
        store
            .state
            .menu_stack
            .active_mut()
            .expect("active menu")
            .selected_index = create_index;

        // The server confirms the profile; the wizard transitions in-place to
        // the provider (LLM config) step.
        store.apply_client_event(ClientEvent::ProfileLocalCreate(
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

        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("expected provider setup menu after local profile create");
        };
        // Sanity: this really is the provider step, not the local-profile step.
        assert!(
            spec.items
                .iter()
                .any(|item| item.id == "onboard.provider.family"),
            "expected provider step with the Model family row"
        );
        let selected = store
            .state
            .menu_stack
            .active()
            .expect("active menu")
            .selected_index;
        assert_eq!(
            spec.items[selected].id, "onboard.provider.family",
            "fresh provider step must land on Model family, not the stale row carried over from the local-profile Continue button"
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
            message: "Octos UI capabilities refreshed: 1 methods".into(),
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

    #[test]
    fn esc_on_root_onboarding_menu_keeps_the_wizard_open() {
        // Issue #5: the onboarding wizard is only auto-opened on first launch, so
        // if Esc closed the root step the user would be stranded with no relaunch.
        // Esc on the ROOT onboarding menu must be a no-op (wizard stays open).
        let mut store = protocol_store_without_sessions();
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
        assert!(store.state.sessions.is_empty(), "onboarding-in-progress");
        assert!(store.active_menu_id_is(crate::menu::registry::MENU_ONBOARD));

        let closed = store.handle_menu_escape();

        assert!(!closed, "Esc on the root onboarding menu must not close it");
        assert!(
            store.active_menu_id_is(crate::menu::registry::MENU_ONBOARD),
            "root onboarding wizard should stay open after Esc"
        );
    }

    #[test]
    fn esc_on_child_onboarding_step_goes_back_to_the_root_wizard() {
        // Esc on a CHILD step (family/model/route/workspace) should still pop back
        // to the parent (root) wizard step — not quit the wizard.
        let mut store = protocol_store_without_sessions();
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL));
        assert!(store.active_menu_id_is(crate::menu::registry::MENU_ONBOARD_MODEL));

        let closed = store.handle_menu_escape();

        assert!(closed, "Esc on a child step should pop a frame");
        assert!(
            store.active_menu_id_is(crate::menu::registry::MENU_ONBOARD),
            "Esc on a child onboarding step should return to the root wizard"
        );
    }

    #[test]
    fn esc_on_non_onboarding_menu_closes_it_as_before() {
        // Regression guard: Esc behavior is unchanged for non-onboarding menus.
        // A store with a session is NOT onboarding-in-progress, so even the
        // generic Esc trap does not apply.
        let mut store = protocol_store_with_methods(&[]);
        assert!(!store.state.sessions.is_empty());
        store.open_menu(MenuId::from(crate::menu::registry::MENU_THEME));
        assert!(store.active_menu_id_is(crate::menu::registry::MENU_THEME));

        let closed = store.handle_menu_escape();

        assert!(closed, "Esc on a non-onboarding menu should close it");
        assert!(
            !store.state.menu_stack.is_active(),
            "the theme menu should be closed after Esc"
        );
    }

    /// M22-B: client-side pre-flight validation rejects obviously
    /// malformed fields before any backend round-trip. The wizard
    /// surfaces a typed recovery (focus field + message) so the user
    /// is not stuck staring at "Local profile is incomplete".
    #[test]
    fn local_profile_invalid_email_is_blocked_pre_flight() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);

        for command in [
            "/onboard name Ada Lovelace",
            "/onboard username ada",
            "/onboard email not-an-email",
            "/onboard finish",
        ] {
            store.state.composer = command.into();
            assert!(
                store.compose_command().is_none(),
                "no RPC should be issued when pre-flight validation fails: {command}"
            );
        }

        let recovery = store
            .state
            .onboarding
            .local_profile_recovery
            .as_ref()
            .expect("recovery should be set after invalid-email finish");
        assert_eq!(
            recovery.kind,
            crate::model::OnboardingLocalProfileErrorKind::InvalidField
        );
        assert_eq!(
            recovery.focus_field,
            crate::model::OnboardingLocalProfileField::Email
        );
        assert!(
            recovery.message.contains("Email must contain"),
            "expected typed recovery message, got: {}",
            recovery.message
        );
    }

    /// M22-B: a backend `profile_local_collision` error keeps the
    /// user on the profile step with a typed recovery focused on
    /// `username`. Generic status text would have shoved the user out
    /// of the wizard.
    #[test]
    fn local_profile_collision_keeps_user_on_profile_step_and_focuses_username() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        for command in [
            "/onboard name Ada Lovelace",
            "/onboard username ada",
            "/onboard email ada@example.com",
            "/onboard finish",
        ] {
            store.state.composer = command.into();
            // The first three are field setters (None); the last is
            // the create RPC dispatch.
            let _ = store.compose_command();
        }
        assert!(store.state.onboarding.local_profile_create_pending);

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "profile_local_collision".into(),
            message: "profile/local/create request tui-3 failed: username already taken".into(),
        }));

        let recovery = store
            .state
            .onboarding
            .local_profile_recovery
            .as_ref()
            .expect("collision error must populate recovery");
        assert_eq!(
            recovery.kind,
            crate::model::OnboardingLocalProfileErrorKind::Collision
        );
        assert_eq!(
            recovery.focus_field,
            crate::model::OnboardingLocalProfileField::Username
        );
        assert!(
            recovery.message.contains("collision for 'ada'"),
            "expected collision message naming the submitted username, got: {}",
            recovery.message
        );
        // Pending flag clears so a follow-up create can fire after edit.
        assert!(!store.state.onboarding.local_profile_create_pending);
        // local_profile_created stays false so the user is held on
        // the profile step.
        assert!(!store.state.onboarding.local_profile_created);
        // Status text is the typed one, not the raw `Error [...]`.
        assert!(
            store.state.status.contains("Local profile setup blocked"),
            "expected typed status, got: {}",
            store.state.status
        );
    }

    /// M22-B: pre-flight validation must drop the user onto the
    /// offending row so they can edit it immediately. Without this
    /// the selected row stays on `onboard.local.create` after the
    /// "finish" press and the keyboard user has no signal where to go.
    #[test]
    fn pre_flight_invalid_email_focuses_email_row() {
        // Use a no-sessions store so the onboarding menu renders the
        // local-profile sub-menu (which has the email row) rather
        // than the provider-setup menu that fires when a profile is
        // already resolved.
        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        ]));
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "not-an-email".into();

        store.state.composer = "/onboard finish".into();
        assert!(store.compose_command().is_none());

        // The selected row index must now correspond to the email
        // row, not the create/continue row.
        let MenuBuildResult::Ready(spec) = store
            .state
            .active_menu
            .as_ref()
            .expect("active menu after validation failure")
        else {
            panic!("expected ready menu");
        };
        let email_index = spec
            .items
            .iter()
            .position(|item| item.id == "onboard.local.email")
            .expect("email row exists");
        let selected = store
            .state
            .menu_stack
            .active()
            .expect("active menu frame")
            .selected_index;
        assert_eq!(
            selected, email_index,
            "pre-flight validation should focus the email row"
        );
    }

    /// M22-B: when the user edits the username while a create is
    /// still in flight, a late `profile_local_collision` for the OLD
    /// username must surface the OLD username in the recovery copy.
    /// The pending-username snapshot captured at submit time is the
    /// source of truth so the message never claims the freshly-edited
    /// new username was rejected.
    #[test]
    fn late_collision_uses_pending_username_snapshot_not_edited_value() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada Lovelace".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();

        store.state.composer = "/onboard finish".into();
        let _ = store
            .compose_command()
            .expect("finish issues profile/local/create");
        // Snapshot captured at submit time.
        assert_eq!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .as_deref(),
            Some("ada")
        );

        // Simulate the user editing the username before the server
        // response arrives. The snapshot must SURVIVE this edit so a
        // late response can still render the recovery against the
        // username actually submitted.
        store.state.composer = "/onboard username ada2".into();
        let _ = store.compose_command();
        assert_eq!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .as_deref(),
            Some("ada"),
            "pending-username snapshot must survive a staged edit"
        );

        // Late collision arrives.
        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "profile_local_collision".into(),
            message: "profile/local/create request tui-1 failed: collision".into(),
        }));

        let recovery = store
            .state
            .onboarding
            .local_profile_recovery
            .as_ref()
            .expect("late collision still routes to recovery");
        assert_eq!(
            recovery.focus_field,
            crate::model::OnboardingLocalProfileField::Username
        );
        // The message MUST attribute the rejection to the username
        // that was actually submitted, not the freshly-edited new
        // value.
        assert!(
            recovery.message.contains("for 'ada'"),
            "expected recovery to reference submitted username 'ada', got: {}",
            recovery.message
        );
        assert!(
            !recovery.message.contains("'ada2'"),
            "recovery must not misattribute collision to edited value: {}",
            recovery.message
        );
    }

    /// M22-B: a second `/onboard finish` press while a create is
    /// still in flight must NOT fire another RPC or overwrite the
    /// pending-username snapshot — the backend would otherwise see
    /// a duplicate create, and a late collision for the first
    /// request would blame the wrong username.
    #[test]
    fn overlapping_local_profile_create_is_blocked() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada Lovelace".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();

        store.state.composer = "/onboard finish".into();
        let first = store
            .compose_command()
            .expect("first finish issues profile/local/create");
        assert!(matches!(first, AppUiCommand::ProfileLocalCreate(_)));
        let pending_username = store
            .state
            .onboarding
            .local_profile_create_pending_username
            .clone();
        assert_eq!(pending_username.as_deref(), Some("ada"));

        // User edits to ada2 and presses finish again.
        store.state.composer = "/onboard username ada2".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard finish".into();
        let second = store.compose_command();

        assert!(
            second.is_none(),
            "second finish must not fire an RPC while a create is pending"
        );
        // Pending snapshot is unchanged — still 'ada', not 'ada2'.
        assert_eq!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .as_deref(),
            Some("ada"),
            "snapshot must not be overwritten by a blocked overlapping create"
        );
        assert!(
            store.state.status.contains("already in progress"),
            "expected blocked-overlap status, got: {}",
            store.state.status
        );
    }

    /// M22-B: when the transport disconnects mid-flight,
    /// `cancel_pending_requests` emits `request_cancelled` with the
    /// method-prefixed message. That cancellation is NOT a profile
    /// rejection — the substring match must not route it through
    /// the typed recovery, otherwise the user sees a username
    /// collision message for a network drop.
    ///
    /// Only `request_cancelled` events that name `profile/local/create`
    /// clear the pending state — cancellations of OTHER tracked
    /// requests must not touch the local-create snapshot.
    #[test]
    fn request_cancelled_during_pending_create_clears_pending_without_misattribution() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "request_cancelled".into(),
            message: "profile/local/create request tui-1 cancelled: transport disconnected".into(),
        }));

        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .is_none()
        );
        assert!(store.state.onboarding.local_profile_recovery.is_none());
    }

    /// A `frame_too_large` pre-send rejection (an oversized inline turn input
    /// or paste over the 1 MB UI-protocol cap) must be RECOVERABLE: surface an
    /// actionable message and keep the session usable, NOT wedge it in Error
    /// (mini5: a 1.1 MB inline send left the session stuck in Error).
    #[test]
    fn frame_too_large_does_not_wedge_session_in_error() {
        use octos_core::app_ui::AppUiError;

        let mut store = store_with_empty_session();
        store.state.set_run_state_in_progress();
        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "frame_too_large".into(),
            message: "UI protocol frame is 1106897 bytes; max is 1048576".into(),
        }));

        assert!(
            !matches!(
                store.state.run_state,
                crate::model::SessionRunState::Error { .. }
            ),
            "frame_too_large must not wedge the session in Error, got {:?}",
            store.state.run_state
        );
        assert!(
            store
                .state
                .status
                .to_ascii_lowercase()
                .contains("too large"),
            "status should be actionable: {}",
            store.state.status
        );
    }

    /// M22-B: a pre-send rejection (e.g. `frame_too_large`) for the
    /// local-create request itself must clear the pending snapshot
    /// — the request is gone and a retry must be possible.
    /// Otherwise the wizard sits in "already in progress" until the
    /// user manually resets state.
    #[test]
    fn frame_too_large_for_local_create_clears_pending() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "frame_too_large".into(),
            message: "profile/local/create request encoded payload exceeds 64KB".into(),
        }));

        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .is_none()
        );
        assert!(store.state.onboarding.local_profile_recovery.is_none());
    }

    /// M22-B: when the transport rejects `profile/local/create`
    /// because the pending-request queue is saturated, the error
    /// message now names the method (thanks to a small transport
    /// fix that includes the rejected method in the
    /// `too_many_pending_requests` text). The store must clear the
    /// pending flag so the user can retry — otherwise the wizard
    /// sits in "already in progress" indefinitely.
    #[test]
    fn too_many_pending_requests_for_local_create_clears_pending() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "too_many_pending_requests".into(),
            message: "UI protocol has 8 pending request(s); refusing to enqueue profile/local/create request".into(),
        }));

        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .is_none()
        );
        assert!(store.state.onboarding.local_profile_recovery.is_none());
    }

    /// M22-B: a `readonly` or `too_many_pending_requests` error on
    /// an UNRELATED command while a local-create is in flight must
    /// NOT touch the local-create pending state. Clearing it would
    /// allow a second create to dispatch (overlapping submits) and
    /// would misattribute the eventual real response.
    #[test]
    fn unrelated_client_error_does_not_clear_pending_create() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);
        let pending_username = store
            .state
            .onboarding
            .local_profile_create_pending_username
            .clone();
        assert_eq!(pending_username.as_deref(), Some("ada"));

        // A `too_many_pending_requests` error on an unrelated
        // command must NOT touch the pending local-create snapshot.
        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "too_many_pending_requests".into(),
            message: "queue full".into(),
        }));

        assert!(
            store.state.onboarding.local_profile_create_pending,
            "pending must persist across unrelated client errors"
        );
        assert_eq!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .as_deref(),
            Some("ada"),
            "pending-username snapshot must persist across unrelated client errors"
        );
    }

    /// M22-B: a `readonly` client-synth error names
    /// `profile/local/create` in its message (the transport
    /// formats "Read-only mode blocks <method>; …") but is NOT a
    /// profile-level rejection. Code-level signal MUST take
    /// precedence over the method substring so the user sees the
    /// readonly status, not a fake username collision.
    #[test]
    fn readonly_with_method_message_is_not_attributed_to_local_create() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "readonly".into(),
            message: "Read-only mode blocks profile/local/create; no network request was sent."
                .into(),
        }));

        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(store.state.onboarding.local_profile_recovery.is_none());
        assert!(
            store.state.status.contains("cancelled") || store.state.status.contains("blocked"),
            "expected cancellation status, got: {}",
            store.state.status
        );
    }

    /// M22-B: even when a transport-level error message names
    /// `profile/local/create` (because the transport built the
    /// outbound payload before failing to send it), the code-level
    /// signal `transport_send`/`transport_read`/`malformed_frame`
    /// MUST take precedence over the method-substring attribution.
    /// Otherwise the typed recovery would falsely blame the
    /// username field for a wire-level fault.
    #[test]
    fn transport_send_with_method_message_is_not_attributed_to_local_create() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);

        // Transport_send may include the method name when the send
        // failed during outbound encoding.
        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "transport_send".into(),
            message: "failed to send profile/local/create request tui-1".into(),
        }));

        // Transport precedence wins: pending cleared, no recovery,
        // status names the transport error.
        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(store.state.onboarding.local_profile_recovery.is_none());
        assert!(
            store.state.status.contains("transport error"),
            "expected transport-error status, got: {}",
            store.state.status
        );
    }

    /// M22-B: a transport-level `AppUiError` (e.g. `transport_read`,
    /// `malformed_frame`) that fires while a local-profile create is
    /// pending must NOT be misclassified as a profile rejection. It
    /// unblocks the pending flag so the user can retry, but renders
    /// a transport-error status instead of typed local-profile
    /// recovery.
    #[test]
    fn transport_error_during_pending_create_clears_pending_without_misattribution() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.composer = "/onboard finish".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.local_profile_create_pending);

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "transport_read".into(),
            message: "failed to read UI protocol transport message: pipe closed".into(),
        }));

        // Pending cleared so retry is possible…
        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(
            store
                .state
                .onboarding
                .local_profile_create_pending_username
                .is_none()
        );
        // …but recovery is NOT populated — transport errors are not
        // profile rejections.
        assert!(store.state.onboarding.local_profile_recovery.is_none());
        assert!(
            store.state.status.contains("transport error"),
            "expected transport-error status, got: {}",
            store.state.status
        );
    }

    /// M22-B: editing the username after a collision must clear the
    /// recovery state so the next create attempt starts fresh.
    #[test]
    fn editing_username_after_collision_clears_recovery() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.name = "Ada Lovelace".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.onboarding.local_profile_create_pending = true;

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "profile_local_collision".into(),
            message: "profile/local/create failed: username already taken".into(),
        }));
        assert!(store.state.onboarding.local_profile_recovery.is_some());

        store.state.composer = "/onboard username ada2".into();
        let _ = store.compose_command();

        assert!(store.state.onboarding.local_profile_recovery.is_none());
    }

    /// M22-B: `profile_local_unsupported` from the backend renders a
    /// typed "this server does not advertise profile/local/create"
    /// recovery instead of a generic `Error [...]` status line.
    #[test]
    fn local_profile_unsupported_renders_typed_recovery() {
        use octos_core::app_ui::AppUiError;

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.local_profile_create_pending = true;

        store.apply_event(AppUiEvent::Error(AppUiError {
            code: "profile_local_unsupported".into(),
            message: "profile/local/create request tui-4 failed: not supported".into(),
        }));

        let recovery = store
            .state
            .onboarding
            .local_profile_recovery
            .as_ref()
            .expect("unsupported error must populate recovery");
        assert_eq!(
            recovery.kind,
            crate::model::OnboardingLocalProfileErrorKind::Unsupported
        );
        assert!(
            recovery.message.contains("misconfigured")
                || recovery.message.contains("profile_local_unsupported"),
            "expected unsupported recovery text, got: {}",
            recovery.message
        );
    }

    /// M22-B: solo onboarding never issues `auth/send_code` or
    /// `auth/verify`, even when the backend advertises them, when
    /// `profile/local/create` is also advertised. The transcript-
    /// equivalent assertion is that `compose_command` returns no
    /// AppUiCommand for the OTP slash subcommands.
    #[test]
    fn solo_onboarding_emits_no_otp_methods_when_local_create_is_supported() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::model::APPUI_METHOD_AUTH_SEND_CODE,
            crate::model::APPUI_METHOD_AUTH_VERIFY,
        ]);
        store.state.workspace.root = "/tmp/solo".into();

        for command in [
            "/onboard name Ada Lovelace",
            "/onboard username ada",
            "/onboard email ada@example.com",
        ] {
            store.state.composer = command.into();
            assert!(store.compose_command().is_none());
        }

        // OTP send-code and verify are explicitly hidden when local
        // profile create is advertised. They must NOT emit any AppUi
        // command.
        for otp_command in ["/onboard send-code", "/onboard verify"] {
            store.state.composer = otp_command.into();
            let result = store.compose_command();
            assert!(
                result.is_none(),
                "{otp_command} must not emit any AppUiCommand in solo mode"
            );
        }

        // Finish path emits exactly `profile/local/create`, no OTP.
        store.state.composer = "/onboard finish".into();
        let command = store
            .compose_command()
            .expect("finish emits profile/local/create");
        assert!(
            matches!(command, AppUiCommand::ProfileLocalCreate(_)),
            "expected ProfileLocalCreate, got: {command:?}"
        );
    }

    /// M22-B: an idempotent backend response (existing local owner)
    /// — `profile/local/create` returns `created: false` for the same
    /// owner — must NOT strand the user on the profile step. The
    /// wizard treats it as a resume and continues to provider setup,
    /// proven by the auto-loaded provider catalog follow-up.
    #[test]
    fn local_profile_idempotent_existing_owner_resumes_to_provider_step() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
        ]);
        store.state.onboarding.name = "Ada Lovelace".into();
        store.state.onboarding.username = "ada".into();
        store.state.onboarding.email = "ada@example.com".into();
        store.state.onboarding.local_profile_create_pending = true;

        let follow_up = store.apply_client_event(ClientEvent::ProfileLocalCreate(
            crate::client_event::ProfileLocalCreateClientEvent {
                result: ProfileLocalCreateResult {
                    profile_id: "ada".into(),
                    user_id: "ada-user".into(),
                    name: "Ada Lovelace".into(),
                    username: "ada".into(),
                    email: "ada@example.com".into(),
                    created: false,
                    runtime_mode: "solo".into(),
                },
                message: "Local profile already exists: ada".into(),
            },
        ));

        // After idempotent response, pending flag and recovery clear,
        // local_profile_created is true (proves we treat existing
        // owner as resumed), and the follow-up auto-loads the
        // provider catalog so the user lands on provider setup.
        assert!(!store.state.onboarding.local_profile_create_pending);
        assert!(store.state.onboarding.local_profile_recovery.is_none());
        assert!(store.state.onboarding.local_profile_created);
        assert_eq!(store.state.onboarding.profile_id.as_deref(), Some("ada"));
        assert!(
            matches!(follow_up, Some(AppUiCommand::ProfileLlmCatalog(_))),
            "expected ProfileLlmCatalog follow-up, got: {follow_up:?}"
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
            message: "Octos UI capabilities refreshed: 3 methods".into(),
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
            message: "Octos UI capabilities refreshed: 3 methods".into(),
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
            message: "Octos UI capabilities refreshed: 1 method".into(),
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
            message: "Octos UI capabilities refreshed: 2 methods".into(),
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

    /// M22-E: `provider_status()` reports `NotSelected` when no
    /// family/model/route has been picked yet.
    #[test]
    fn provider_status_not_selected_for_blank_wizard() {
        let store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG]);
        assert_eq!(
            store.state.onboarding.provider_status(),
            crate::model::OnboardingProviderStatus::NotSelected
        );
    }

    /// M22-E: `provider_status()` reports `KeyMissing` once a
    /// selection is ready but no API key is staged.
    #[test]
    fn provider_status_key_missing_after_selection() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG]);
        store.state.onboarding.provider.family_id = "deepseek".into();
        store.state.onboarding.provider.model_id = "deepseek-reasoner".into();
        store.state.onboarding.provider.route.route_id = "deepseek".into();
        assert_eq!(
            store.state.onboarding.provider_status(),
            crate::model::OnboardingProviderStatus::KeyMissing
        );
    }

    /// M22-E: a failed `profile/llm/test` does NOT mark the
    /// provider as ready. `provider_status()` reports
    /// `TestFailed` with the server reason, and the saved-primary
    /// state stays false so finish is blocked.
    #[test]
    fn provider_status_test_failed_keeps_provider_unready() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
            crate::model::APPUI_METHOD_PROFILE_LLM_TEST,
        ]);
        // Pretend the wizard already issued a test (pending=Test).
        store.state.onboarding.provider.family_id = "openai".into();
        store.state.onboarding.provider.model_id = "gpt-test".into();
        store.state.onboarding.provider.route.route_id = "openai".into();
        store.state.onboarding.api_key = Some(crate::model::SecretString::new("redacted"));
        store.state.onboarding.provider_pending =
            Some(crate::model::OnboardingProviderPending::Test);

        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: crate::model::ProfileLlmMutationResult {
                    profile_id: Some("alice".into()),
                    primary: None,
                    fallbacks: Vec::new(),
                    applied: false,
                    llm: None,
                    runtime_policy_stamp: None,
                    message: Some("invalid API key for openai route".into()),
                    error: Some("invalid_api_key".into()),
                },
                message: "profile/llm/test failed: invalid API key".into(),
            },
        ));

        // Provider NOT marked tested/saved.
        assert!(!store.state.onboarding.provider_tested);
        assert!(!store.state.onboarding.provider_saved);
        let status = store.state.onboarding.provider_status();
        match status {
            crate::model::OnboardingProviderStatus::TestFailed { reason } => {
                assert!(
                    reason.contains("invalid_api_key") || reason.contains("invalid API key"),
                    "expected server reason in test failure, got: {reason}"
                );
            }
            other => panic!("expected TestFailed, got: {other:?}"),
        }
    }

    /// M22-E: saving the provider as PRIMARY puts the wizard in
    /// `SavedPrimary`, which is the only status that lets finish
    /// proceed.
    #[test]
    fn provider_status_saved_primary_unlocks_finish() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
            crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        store.state.onboarding.provider.family_id = "openai".into();
        store.state.onboarding.provider.model_id = "gpt-test".into();
        store.state.onboarding.provider.route.route_id = "openai".into();
        store.state.onboarding.api_key = Some(crate::model::SecretString::new("redacted"));
        store.state.onboarding.provider_pending =
            Some(crate::model::OnboardingProviderPending::Save);
        store.state.onboarding.provider_save_target =
            Some(crate::model::OnboardingProviderSaveTarget::Primary);

        store.apply_client_event(ClientEvent::ProfileLlmMutation(
            ProfileLlmMutationClientEvent {
                result: applied_profile_llm_result(),
                message: "profile/llm/upsert saved".into(),
            },
        ));

        assert!(store.state.onboarding.provider_saved);
        assert_eq!(
            store.state.onboarding.provider_status(),
            crate::model::OnboardingProviderStatus::SavedPrimary
        );
    }

    /// M22-E: saving as FALLBACK is visually distinct — the
    /// wizard reports `SavedFallback`, not `SavedPrimary`, so the
    /// menu can label the row "fallback only".
    #[test]
    fn provider_status_saved_fallback_is_distinct_from_primary() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
            crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        // After a fallback save, the staged provider is reset
        // but `last_saved_provider_target` is Fallback and
        // `provider_saved` is NOT set (per the existing handler).
        // To exercise the SavedFallback status path we have to
        // emulate the post-save state where primary is also set
        // (so provider_saved is true and target is Fallback) —
        // that maps to a scenario where the user pinned a
        // primary and then added a fallback. Set both:
        store.state.onboarding.provider_saved = true;
        store.state.onboarding.last_saved_provider_target =
            Some(crate::model::OnboardingProviderSaveTarget::Fallback);
        store.state.onboarding.provider.family_id = "openai".into();
        store.state.onboarding.provider.model_id = "gpt-test".into();
        store.state.onboarding.provider.route.route_id = "openai".into();
        store.state.onboarding.api_key = Some(crate::model::SecretString::new("redacted"));

        assert_eq!(
            store.state.onboarding.provider_status(),
            crate::model::OnboardingProviderStatus::SavedFallback
        );
    }

    /// M22-E: a server-echoed API key in a test-failure reason
    /// MUST be redacted before being stored, even though the
    /// backend normally redacts. Belt-and-suspenders contract.
    #[test]
    fn provider_failure_reason_strips_echoed_api_key() {
        let staged = crate::model::SecretString::new("sk-leaked-12345");
        let event = ProfileLlmMutationClientEvent {
            result: crate::model::ProfileLlmMutationResult {
                profile_id: Some("alice".into()),
                primary: None,
                fallbacks: Vec::new(),
                applied: false,
                llm: None,
                runtime_policy_stamp: None,
                message: Some("server rejected key sk-leaked-12345".into()),
                error: Some("auth_invalid: key sk-leaked-12345 was not accepted".into()),
            },
            message: "profile/llm/test failed".into(),
        };
        let reason = provider_failure_reason(&event, Some(&staged));
        assert!(
            !reason.contains("sk-leaked-12345"),
            "raw API key must be stripped, got: {reason}"
        );
        assert!(reason.contains("********"));
    }

    /// M22-E: a `SavedFallback` save resets staged input via
    /// `reset_staged_provider()`. After the reset
    /// `provider_status()` must still report `SavedFallback`,
    /// NOT `NotSelected` — otherwise the menu can't tell
    /// "fallback only" from "nothing chosen".
    #[test]
    fn provider_status_reports_saved_fallback_even_after_staged_reset() {
        let mut state = crate::model::OnboardingWizardState::default();
        // Simulate the post-fallback-save state directly: reset
        // staged provider AND set last_saved_provider_target to
        // Fallback.
        state.reset_staged_provider();
        state.last_saved_provider_target =
            Some(crate::model::OnboardingProviderSaveTarget::Fallback);
        assert_eq!(
            state.provider_status(),
            crate::model::OnboardingProviderStatus::SavedFallback
        );
    }

    /// M22-E: editing the API key (or any staged input) after a
    /// failed test must clear `provider_test_failure_reason`
    /// so the menu does not keep showing the stale "Test
    /// failed — ..." label.
    #[test]
    fn editing_api_key_clears_stale_test_failure_reason() {
        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
            crate::model::APPUI_METHOD_PROFILE_LLM_TEST,
        ]);
        store.state.onboarding.provider.family_id = "openai".into();
        store.state.onboarding.provider.model_id = "gpt-test".into();
        store.state.onboarding.provider.route.route_id = "openai".into();
        store.state.onboarding.api_key = Some(crate::model::SecretString::new("old-key"));
        store.state.onboarding.provider_test_failure_reason = Some("invalid key".into());

        store.state.composer = "/onboard key new-key".into();
        let _ = store.compose_command();

        assert!(
            store
                .state
                .onboarding
                .provider_test_failure_reason
                .is_none(),
            "editing the key must clear stale test-failure recovery"
        );
    }

    /// M22-E: the wizard's debug/state snapshot never contains
    /// the raw API key — `SecretString::Debug` masks it. This
    /// regression test pins the redaction contract.
    #[test]
    fn provider_api_key_is_redacted_in_debug_output() {
        let mut state = crate::model::OnboardingWizardState::default();
        state.api_key = Some(crate::model::SecretString::new("sk-very-secret-value-xyz"));
        let formatted = format!("{state:?}");
        assert!(
            !formatted.contains("sk-very-secret-value-xyz"),
            "debug output must not contain the raw API key: {formatted}"
        );
    }

    /// M22-C: `/onboard workspace <path>` stages the candidate
    /// without mutating the active workspace pane, and resets any
    /// prior validation so the user must re-validate before finish.
    #[test]
    fn onboard_workspace_command_stages_candidate_and_resets_validation() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        // Pre-existing validation must be cleared by staging a new
        // candidate.
        store.state.onboarding.workspace_validation =
            crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/old".into(),
                writable: true,
                has_workspace_toml: false,
            };

        store.state.composer = "/onboard workspace /some/new/path".into();
        assert!(store.compose_command().is_none());

        assert_eq!(
            store.state.onboarding.workspace_candidate.as_deref(),
            Some("/some/new/path")
        );
        assert!(store.state.onboarding.workspace_validation.is_unvalidated());
    }

    /// M22-C: validating a candidate that does NOT exist yields
    /// `Invalid` and does not crash. `onboarding_finish_command`
    /// then refuses to emit `session/open` until the user fixes it.
    #[test]
    fn validate_workspace_missing_path_marks_invalid() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        // Use a local stdio target so the client filesystem probe
        // runs (remote transports defer validation to the server).
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.onboarding.provider_saved = true;
        store.state.composer = "/onboard workspace /tmp/this/path/does/not/exist".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard workspace-validate".into();
        assert!(store.compose_command().is_none());

        let validation = &store.state.onboarding.workspace_validation;
        match validation {
            crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
                assert!(
                    reason.contains("not accessible"),
                    "expected access error, got: {reason}"
                );
            }
            other => panic!("expected Invalid validation, got: {other:?}"),
        }

        // Finish must NOT emit session/open while invalid.
        store.state.composer = "/onboard profile alice".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard finish".into();
        let result = store.compose_command();
        assert!(
            result.is_none(),
            "session/open must not fire while workspace validation is Invalid"
        );
        let expected_status =
            if let crate::model::OnboardingWorkspaceValidation::Invalid { reason } =
                &store.state.onboarding.workspace_validation
            {
                t!("status.cannot_open_workspace_invalid", reason = reason).into_owned()
            } else {
                panic!("expected invalid workspace validation");
            };
        assert!(
            store.state.status == expected_status,
            "expected blocked-status message, got: {}",
            store.state.status
        );
    }

    /// M22-D: `/onboard permissions <mode>` stages a permission
    /// profile update. The wizard does NOT claim the choice is
    /// effective — the staged update is just held for application
    /// after `session/open`.
    #[test]
    fn onboard_permissions_command_stages_workspace_write_never() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);

        store.state.composer = "/onboard permissions workspace-write-never".into();
        assert!(store.compose_command().is_none());

        let staged = store
            .state
            .onboarding
            .staged_permission_profile
            .clone()
            .expect("staged permission profile");
        assert_eq!(
            staged.mode,
            Some(octos_core::ui_protocol::PermissionProfileMode::WorkspaceWrite)
        );
        assert_eq!(staged.approval_policy.as_deref(), Some("never"));
        assert!(store.state.status.contains("staged"));
    }

    /// M22-D: `/onboard permissions clear` removes the staged
    /// permission profile.
    #[test]
    fn onboard_permissions_clear_drops_staged_profile() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.composer = "/onboard permissions read-only".into();
        let _ = store.compose_command();
        assert!(store.state.onboarding.staged_permission_profile.is_some());

        store.state.composer = "/onboard permissions clear".into();
        let _ = store.compose_command();

        assert!(store.state.onboarding.staged_permission_profile.is_none());
    }

    /// M22-D: an unknown mode is rejected with a typed status that
    /// names the accepted modes.
    #[test]
    fn onboard_permissions_rejects_unknown_mode() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.composer = "/onboard permissions yolo".into();
        assert!(store.compose_command().is_none());

        assert!(store.state.onboarding.staged_permission_profile.is_none());
        assert!(
            store
                .state
                .status
                .contains("Unknown permission profile mode"),
            "expected typed error, got: {}",
            store.state.status
        );
    }

    /// M22-C: validating a candidate that exists and is writable
    /// marks it `Valid`, and `/onboard finish` then emits
    /// `session/open` with the validated canonical cwd. Paths with
    /// spaces are supported.
    #[test]
    fn validate_workspace_with_existing_temp_dir_unlocks_finish() {
        // Use the system temp dir which is guaranteed to exist and
        // (almost always) writable.
        let temp_dir = std::env::temp_dir();
        let temp_str = temp_dir.to_string_lossy().into_owned();
        if temp_str.is_empty() {
            // Skip on a hypothetical platform where temp_dir is empty.
            return;
        }
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.onboarding.provider_saved = true;

        store.state.composer = format!("/onboard workspace {temp_str}");
        let _ = store.compose_command();
        store.state.composer = "/onboard workspace-validate".into();
        let _ = store.compose_command();

        assert!(
            store.state.onboarding.workspace_validation.is_valid(),
            "expected Valid validation, got: {:?}",
            store.state.onboarding.workspace_validation
        );

        store.state.composer = "/onboard profile alice".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard finish".into();
        let command = store
            .compose_command()
            .expect("finish emits session/open after workspace validation");
        let AppUiCommand::OpenSession(params) = command else {
            panic!("expected session/open, got: {command:?}");
        };
        // Promoted candidate is now the cwd.
        assert!(params.cwd.is_some());
    }

    /// M22-C: filesystem root `/` is rejected as a workspace
    /// (root-escape protection).
    #[test]
    fn validate_workspace_rejects_filesystem_root() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.composer = "/onboard workspace /".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard workspace-validate".into();
        let _ = store.compose_command();

        match &store.state.onboarding.workspace_validation {
            crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
                assert!(
                    reason.contains("filesystem root") || reason.contains("/"),
                    "expected root-escape reason, got: {reason}"
                );
            }
            other => panic!("expected Invalid, got: {other:?}"),
        }
    }

    /// M22-C: a stdio launch label like
    /// `stdio:octos serve --stdio --cwd <path>` carries the cwd in
    /// the command string. The probe must extract the embedded cwd
    /// before validating so the user does not have to retype it.
    #[test]
    fn validate_workspace_extracts_cwd_from_stdio_target_label() {
        let temp = std::env::temp_dir();
        let temp_str = temp.to_string_lossy().into_owned();
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.workspace.root = format!("stdio:/opt/octos serve --stdio --cwd {temp_str}");
        store.state.composer = "/onboard workspace-validate".into();
        let _ = store.compose_command();

        assert!(
            store.state.onboarding.workspace_validation.is_valid(),
            "expected Valid after extracting cwd from stdio label, got: {:?}",
            store.state.onboarding.workspace_validation
        );
    }

    /// M22-C: a remote `wss://` transport target means the workspace
    /// is on the SERVER host, not the client. The probe must skip
    /// the local filesystem stat and trust the server to validate
    /// on `session/open`, otherwise valid remote workflows are
    /// blocked.
    #[test]
    fn validate_workspace_skips_local_probe_on_remote_transport() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("wss://remote.example/ui-protocol".into());
        store.state.composer = "/onboard workspace /srv/project".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard workspace-validate".into();
        let _ = store.compose_command();

        let validation = &store.state.onboarding.workspace_validation;
        assert!(
            validation.is_valid(),
            "remote workspaces must be marked Valid without a local stat, got: {validation:?}"
        );
        match validation {
            crate::model::OnboardingWorkspaceValidation::Valid { canonical, .. } => {
                assert_eq!(canonical, "/srv/project");
            }
            _ => unreachable!(),
        }
    }

    /// M22-C: a stdio-target workspace.root (e.g. raw `stdio:`
    /// command line without an embedded cwd) is unusable as a cwd.
    /// Validation must mark it Invalid so finish is blocked,
    /// prompting the user to stage a real path with
    /// `/onboard workspace <path>`.
    #[test]
    fn validate_workspace_rejects_stdio_target_workspace_root() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.workspace.root = "stdio".into();
        store.state.composer = "/onboard workspace-validate".into();
        let _ = store.compose_command();

        match &store.state.onboarding.workspace_validation {
            crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
                let expected = t!("status.no_usable_workspace_cwd", target = "stdio");
                assert_eq!(reason, expected.as_ref());
                assert!(
                    reason.contains("/onboard workspace"),
                    "reason should name the override command: {reason}"
                );
            }
            other => panic!("expected Invalid, got: {other:?}"),
        }
    }

    /// M22-C: a relative-path candidate (`/onboard workspace .`) is
    /// canonicalised by the probe; finish must promote the
    /// CANONICAL value into `state.workspace.root` so `session/open`
    /// sends exactly what the user validated, not the raw candidate.
    #[test]
    fn finish_promotes_canonical_workspace_path_not_raw_candidate() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.onboarding.provider_saved = true;
        // Stage a relative path that the canonicaliser will expand.
        store.state.composer = "/onboard workspace .".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard workspace-validate".into();
        let _ = store.compose_command();
        // Probe should succeed against the current cwd, producing
        // a canonical absolute path.
        let canonical = match &store.state.onboarding.workspace_validation {
            crate::model::OnboardingWorkspaceValidation::Valid { canonical, .. } => {
                canonical.clone()
            }
            other => panic!("expected Valid validation for '.', got: {other:?}"),
        };
        assert!(
            canonical.starts_with('/'),
            "canonical path must be absolute, got: {canonical}"
        );

        store.state.composer = "/onboard profile alice".into();
        let _ = store.compose_command();
        store.state.composer = "/onboard finish".into();
        let command = store
            .compose_command()
            .expect("finish emits session/open with canonical cwd");
        let AppUiCommand::OpenSession(params) = command else {
            panic!("expected session/open, got: {command:?}");
        };
        assert_eq!(
            params.cwd.as_deref(),
            Some(canonical.as_str()),
            "session/open must receive the canonical path, not the raw '.' candidate"
        );
        assert_eq!(store.state.workspace.root, canonical);
    }

    /// M22-C: pressing `/onboard finish` without manual validation
    /// auto-runs the probe and reports the result. The user is
    /// dropped on the workspace-blocked status if invalid, without
    /// needing to know about the workspace-validate sub-command.
    #[test]
    fn onboarding_finish_auto_validates_workspace_when_unvalidated() {
        let mut store = protocol_store_with_methods(&[crate::model::APPUI_METHOD_AUTH_STATUS]);
        store.state.target = Some("stdio:octos serve --stdio".into());
        store.state.onboarding.provider_saved = true;
        store.state.workspace.root = "/tmp/this/path/does/not/exist".into();
        store.state.composer = "/onboard profile alice".into();
        let _ = store.compose_command();

        store.state.composer = "/onboard finish".into();
        let result = store.compose_command();

        assert!(result.is_none(), "finish must block on Invalid workspace");
        // The validation state is now Invalid (auto-probed).
        assert!(matches!(
            store.state.onboarding.workspace_validation,
            crate::model::OnboardingWorkspaceValidation::Invalid { .. }
        ));
    }

    /// M22-D: after `session/open` succeeds and `permission/profile/set`
    /// is advertised, the store emits a follow-up `permission/profile/set`
    /// RPC carrying the staged update. Without the capability, the
    /// staged choice remains held but no RPC fires.
    #[test]
    fn session_opened_emits_permission_profile_set_when_staged() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::{
            PermissionNetworkPolicy, PermissionProfileMode, PermissionProfileUpdate, SessionOpened,
        };

        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::menu::registry::APPUI_METHOD_PERMISSION_PROFILE_SET,
        ]);
        store.state.onboarding.staged_permission_profile = Some(PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::ReadOnly),
            network: Some(PermissionNetworkPolicy::Deny),
            approval_policy: Some("on-request".into()),
        });

        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": SessionKey("alice:local:tui#coding".into()),
            "active_profile_id": "alice",
        }))
        .expect("session/opened payload");
        let follow_up =
            store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

        let Some(AppUiCommand::SetPermissionProfile(params)) = follow_up else {
            panic!("expected permission/profile/set follow-up, got: {follow_up:?}");
        };
        assert_eq!(params.update.mode, Some(PermissionProfileMode::ReadOnly));
        assert_eq!(params.update.approval_policy.as_deref(), Some("on-request"));
    }

    /// Issue #4: finishing the wizard must drop the user into the working
    /// surface, not leave the onboarding menu stacked over the chat. When a
    /// session opens while an onboarding menu is active, the wizard is torn
    /// down and the composer is focused.
    #[test]
    fn session_opened_closes_onboarding_wizard_and_focuses_composer() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::SessionOpened;

        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::model::APPUI_METHOD_AUTH_STATUS,
        ]);
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
        assert!(store.state.menu_stack.is_active(), "wizard menu is open");
        store.state.focus = FocusPane::Sessions;

        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": SessionKey("alice:local:tui#coding".into()),
            "active_profile_id": "alice",
        }))
        .expect("session/opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

        assert!(
            !store.state.menu_stack.is_active(),
            "onboarding wizard is torn down after the session opens"
        );
        assert_eq!(
            store.state.focus,
            FocusPane::Composer,
            "user lands focused on the composer, ready to code"
        );
    }

    /// A session opening while a NON-onboarding menu (or no menu) is active
    /// must not be force-closed — only the wizard tears itself down.
    #[test]
    fn session_opened_leaves_non_onboarding_menu_open() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::SessionOpened;

        let mut store = store_with_empty_session();
        store.open_menu(MenuId::from(crate::menu::registry::MENU_THEME));

        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": SessionKey("alice:local:tui#coding".into()),
            "active_profile_id": "alice",
        }))
        .expect("session/opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

        assert!(
            store.state.menu_stack.is_active(),
            "a non-onboarding menu stays open across session open"
        );
    }

    /// UX2 B.2: Activate moved to the WORKSPACE step screen, so opening a
    /// session while THAT menu is active must also tear the wizard down (drop
    /// the user into the coding surface) — the same as the other onboarding
    /// menus. Without `MENU_ONBOARD_WORKSPACE` in `active_menu_is_onboarding`,
    /// the workspace menu would stay stacked over the chat after activation.
    #[test]
    fn session_opened_tears_down_workspace_step_menu() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::SessionOpened;

        let mut store = store_with_empty_session();
        store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD_WORKSPACE));
        assert!(store.state.menu_stack.is_active());

        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": SessionKey("alice:local:tui#coding".into()),
            "active_profile_id": "alice",
        }))
        .expect("session/opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

        assert!(
            !store.state.menu_stack.is_active(),
            "the workspace step menu is torn down once the session opens"
        );
        assert_eq!(store.state.focus, FocusPane::Composer);
    }

    #[test]
    fn session_opened_restores_persisted_reasoning_effort() {
        use octos_core::ui_protocol::{ReasoningEffortLevel as L, SessionOpened};
        let mut store = store_with_empty_session();
        let sid = SessionKey("alice:local:tui#coding".into());

        // Server surfaces a persisted effort on open -> client restores it.
        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": sid,
            "active_profile_id": "alice",
            "reasoning_effort": "high",
        }))
        .expect("session/opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));
        assert_eq!(
            store.state.session_reasoning_effort.get(&sid),
            Some(&L::High)
        );

        // Reopening with no persisted effort clears the client override.
        let opened2: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": sid,
            "active_profile_id": "alice",
        }))
        .expect("session/opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened2)));
        assert!(store.state.session_reasoning_effort.get(&sid).is_none());
    }

    /// M22-D: when `permission/profile/set` is NOT advertised, the
    /// SessionOpened handler does NOT emit a follow-up — the
    /// staged choice stays in the wizard state but the server is
    /// trusted to fall back to its default policy.
    #[test]
    fn session_opened_without_set_capability_does_not_emit_follow_up() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::{
            PermissionProfileMode, PermissionProfileUpdate, SessionOpened,
        };

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.staged_permission_profile = Some(PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::WorkspaceWrite),
            network: None,
            approval_policy: Some("on-request".into()),
        });

        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": SessionKey("alice:local:tui#coding".into()),
            "active_profile_id": "alice",
        }))
        .expect("session/opened payload");
        let follow_up =
            store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

        assert!(follow_up.is_none());
        // Staged choice stays present so a later capability
        // advertisement can still apply it.
        assert!(store.state.onboarding.staged_permission_profile.is_some());
    }

    /// M22-D: when the runtime policy stamp disagrees with the
    /// staged permission profile (server clamped or rejected), the
    /// wizard records a typed `permission_profile_mismatch` reason
    /// so the UI can surface "your staged choice was rejected".
    #[test]
    fn runtime_policy_stamp_mismatch_populates_typed_reason() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::{PermissionProfileMode, PermissionProfileUpdate};

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.staged_permission_profile = Some(PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::DangerFullAccess),
            network: None,
            approval_policy: Some("never".into()),
        });
        // Server clamps to workspace_write + on-request (typical
        // tenant policy that rejects danger-full-access).
        store.apply_client_event(ClientEvent::SessionStatus(
            crate::client_event::SessionStatusClientEvent {
                result: crate::model::SessionStatusReadResult {
                    session_id: SessionKey("alice:local:tui#coding".into()),
                    profile_id: Some("alice".into()),
                    runtime_mode: Some("tenant".into()),
                    cwd: None,
                    workspace_root: None,
                    active_turn_id: None,
                    runtime_policy_stamp: Some(crate::model::RuntimePolicyStamp {
                        permission_profile: Some("workspace_write".into()),
                        approval_policy: Some("on-request".into()),
                        ..Default::default()
                    }),
                    model: None,
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
                    capabilities: None,
                    mcp_summary: None,
                    tool_summary: None,
                    usage: None,
                    cursor: None,
                },
                message: "runtime status".into(),
            },
        ));

        let mismatch = store
            .state
            .onboarding
            .permission_profile_mismatch
            .as_ref()
            .expect("mismatch should be recorded");
        assert!(
            mismatch.contains("permission_profile"),
            "expected mismatch to name the field, got: {mismatch}"
        );
        assert!(mismatch.contains("danger_full_access"));
    }

    /// M22-D: the runtime policy stamp publishes
    /// `"allowed"`/`"blocked"` for network, but the request shape
    /// uses `"allow"`/`"deny"`. The comparator must accept both so
    /// a correctly-applied policy never reads as clamped.
    #[test]
    fn runtime_policy_stamp_network_aliases_accepted_as_match() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::{
            PermissionNetworkPolicy, PermissionProfileMode, PermissionProfileUpdate,
        };

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.staged_permission_profile = Some(PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::WorkspaceWrite),
            network: Some(PermissionNetworkPolicy::Deny),
            approval_policy: Some("on-request".into()),
        });
        store.apply_client_event(ClientEvent::SessionStatus(
            crate::client_event::SessionStatusClientEvent {
                result: crate::model::SessionStatusReadResult {
                    session_id: SessionKey("alice:local:tui#coding".into()),
                    profile_id: Some("alice".into()),
                    runtime_mode: Some("solo".into()),
                    cwd: None,
                    workspace_root: None,
                    active_turn_id: None,
                    runtime_policy_stamp: Some(crate::model::RuntimePolicyStamp {
                        permission_profile: Some("workspace_write".into()),
                        approval_policy: Some("on-request".into()),
                        // Backend publishes "blocked" (past tense)
                        // for network=Deny — the comparator must
                        // recognize the alias.
                        network: Some("blocked".into()),
                        ..Default::default()
                    }),
                    model: None,
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
                    capabilities: None,
                    mcp_summary: None,
                    tool_summary: None,
                    usage: None,
                    cursor: None,
                },
                message: "runtime status".into(),
            },
        ));

        assert!(
            store.state.onboarding.permission_profile_mismatch.is_none(),
            "'blocked' must be accepted as matching network=Deny, got: {:?}",
            store.state.onboarding.permission_profile_mismatch
        );
    }

    /// M22-D: after `permission/profile/set` resolves, the store
    /// must refresh `session/status/read` so the runtime policy
    /// stamp arrives and the mismatch validator can run. Without
    /// this follow-up the user never sees a clamp warning in the
    /// normal onboarding flow.
    #[test]
    fn permission_profile_set_response_refreshes_session_status() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::{
            PermissionProfileMode, PermissionProfileSelection, PermissionProfileUpdate,
        };

        let mut store = protocol_store_with_methods(&[
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            crate::menu::registry::APPUI_METHOD_PERMISSION_PROFILE_SET,
            crate::model::APPUI_METHOD_SESSION_STATUS_READ,
        ]);
        store.state.onboarding.staged_permission_profile = Some(PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::WorkspaceWrite),
            network: None,
            approval_policy: Some("on-request".into()),
        });

        let follow_up = store.apply_client_event(ClientEvent::PermissionProfile(
            crate::client_event::PermissionProfileClientEvent {
                session_id: SessionKey("alice:local:tui#coding".into()),
                current: PermissionProfileSelection::default(),
                message: "permission/profile/set applied".into(),
            },
        ));

        let Some(AppUiCommand::ReadSessionStatus(params)) = follow_up else {
            panic!(
                "expected session/status/read follow-up after permission set, got: {follow_up:?}"
            );
        };
        assert_eq!(
            params.session_id,
            SessionKey("alice:local:tui#coding".into())
        );
    }

    /// M22-D: matching stamps leave `permission_profile_mismatch`
    /// as `None`. The wizard only flags divergence.
    #[test]
    fn runtime_policy_stamp_match_clears_mismatch() {
        use octos_core::SessionKey;
        use octos_core::ui_protocol::{PermissionProfileMode, PermissionProfileUpdate};

        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.onboarding.staged_permission_profile = Some(PermissionProfileUpdate {
            mode: Some(PermissionProfileMode::WorkspaceWrite),
            network: None,
            approval_policy: Some("on-request".into()),
        });
        store.apply_client_event(ClientEvent::SessionStatus(
            crate::client_event::SessionStatusClientEvent {
                result: crate::model::SessionStatusReadResult {
                    session_id: SessionKey("alice:local:tui#coding".into()),
                    profile_id: Some("alice".into()),
                    runtime_mode: Some("solo".into()),
                    cwd: None,
                    workspace_root: None,
                    active_turn_id: None,
                    runtime_policy_stamp: Some(crate::model::RuntimePolicyStamp {
                        permission_profile: Some("workspace-write".into()),
                        approval_policy: Some("on-request".into()),
                        ..Default::default()
                    }),
                    model: None,
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
                    capabilities: None,
                    mcp_summary: None,
                    tool_summary: None,
                    usage: None,
                    cursor: None,
                },
                message: "runtime status".into(),
            },
        ));

        assert!(
            store.state.onboarding.permission_profile_mismatch.is_none(),
            "matching stamp should clear mismatch, got: {:?}",
            store.state.onboarding.permission_profile_mismatch
        );
    }

    /// M22-F: doctor report for a fresh store with a local-create
    /// capability surfaces a FAIL for profile (no profile yet)
    /// and FAIL for provider, but PASS for transport/capabilities/
    /// workspace. Uses `protocol_store_without_sessions` so no
    /// session-resolved profile exists to obscure the FAIL.
    #[test]
    fn doctor_report_for_fresh_store_flags_profile_and_provider() {
        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        ]));
        store.state.workspace.root = "/tmp/project".into();
        let report = store.onboarding_doctor_report();
        assert!(
            report.any_failures(),
            "fresh store must flag at least one FAIL"
        );
        let profile = report
            .checks
            .iter()
            .find(|check| check.id == "profile")
            .expect("profile check exists");
        assert!(matches!(
            profile.outcome,
            crate::model::OnboardingDoctorOutcome::Fail { .. }
        ));
        let provider = report
            .checks
            .iter()
            .find(|check| check.id == "provider")
            .expect("provider check exists");
        assert!(matches!(
            provider.outcome,
            crate::model::OnboardingDoctorOutcome::Fail { .. }
        ));
        let workspace = report
            .checks
            .iter()
            .find(|check| check.id == "workspace")
            .expect("workspace check exists");
        assert!(workspace.outcome.is_pass());
    }

    /// M22-F: `/onboard doctor` writes a status summary line that
    /// names each check id and outcome, and pushes per-check
    /// activity entries.
    #[test]
    fn onboard_doctor_writes_status_summary_and_activity_entries() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.workspace.root = "/tmp/project".into();
        let activity_before = store.state.activity.len();

        store.state.composer = "/onboard doctor".into();
        let result = store.compose_command();
        assert!(result.is_none(), "doctor is a local read; no RPC");

        assert!(
            store.state.status.starts_with("Onboarding doctor"),
            "doctor must update status line, got: {}",
            store.state.status
        );
        assert!(
            store.state.activity.len() > activity_before,
            "doctor must push per-check activity entries"
        );
    }

    /// M22-F: when the profile is created and provider saved, the
    /// doctor report has zero FAILs (workspace and transport
    /// still pass, capabilities OK).
    #[test]
    fn doctor_report_passes_once_profile_and_provider_ready() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.workspace.root = "/tmp/project".into();
        store.state.onboarding.profile_id = Some("alice".into());
        store.state.onboarding.local_profile_created = true;
        store.state.onboarding.provider_saved = true;
        store.state.onboarding.saved_primary_provider_label = Some("openai / gpt / openai".into());

        let report = store.onboarding_doctor_report();
        assert!(!report.any_failures(), "report: {:?}", report);
        let profile = report
            .checks
            .iter()
            .find(|check| check.id == "profile")
            .unwrap();
        assert!(profile.outcome.is_pass());
        let provider = report
            .checks
            .iter()
            .find(|check| check.id == "provider")
            .unwrap();
        assert!(provider.outcome.is_pass());
    }

    /// M22-F: when the profile is resolved from an active session
    /// rather than `onboarding.profile_id`, the doctor still
    /// reports PASS — the same resolved-profile source as
    /// `onboarding_finish_command` must be used.
    #[test]
    fn doctor_report_uses_resolved_profile_from_session() {
        // Default store has an empty session with profile_id = "coding".
        let mut store = store_with_empty_session();
        store.state.target = Some("ws://example.test/ui-protocol".into());
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        ]));
        store.state.workspace.root = "/tmp/project".into();
        // `onboarding.profile_id` is blank; the session carries
        // `profile_id = Some("coding")` which the resolver picks up.

        let report = store.onboarding_doctor_report();
        let profile = report
            .checks
            .iter()
            .find(|check| check.id == "profile")
            .expect("profile check exists");
        assert!(
            profile.outcome.is_pass(),
            "doctor must accept the session-resolved profile, got: {:?}",
            profile.outcome
        );
    }

    /// M22-F: when the server has published a primary provider via
    /// `profile_llm_state` (post-`/onboard providers`), the doctor
    /// recognises it even though `onboarding.provider_saved` is
    /// still false.
    #[test]
    fn doctor_report_honors_server_published_primary_provider() {
        let mut store =
            protocol_store_with_methods(&[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        store.state.workspace.root = "/tmp/project".into();
        store.state.profile_llm_state = Some(crate::model::ProfileLlmListResult {
            profile_id: Some("alice".into()),
            primary: Some(crate::model::LlmConfiguredProvider {
                provider: "openai".into(),
                model: "gpt-test".into(),
                family_id: Some("openai".into()),
                model_id: Some("gpt-test".into()),
                route: None,
                route_id: Some("openai".into()),
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

        let report = store.onboarding_doctor_report();
        let provider = report
            .checks
            .iter()
            .find(|check| check.id == "provider")
            .expect("provider check exists");
        assert!(
            provider.outcome.is_pass(),
            "doctor must accept server-published primary, got: {:?}",
            provider.outcome
        );
    }

    /// M22-F: a server that does not advertise
    /// `profile/local/create` is non-solo-onboarding; the
    /// profile check skips rather than failing.
    #[test]
    fn doctor_report_skips_profile_when_capability_absent() {
        let mut store = protocol_store_without_sessions();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods(
            std::iter::empty::<&str>(),
        ));
        store.state.workspace.root = "/tmp/project".into();
        let report = store.onboarding_doctor_report();
        let profile = report
            .checks
            .iter()
            .find(|check| check.id == "profile")
            .unwrap();
        assert!(matches!(
            profile.outcome,
            crate::model::OnboardingDoctorOutcome::Skipped { .. }
        ));
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
            message: "Octos UI capabilities refreshed: 1 methods".into(),
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
            message: "Octos UI capabilities refreshed: 1 methods".into(),
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
            message: "Octos UI capabilities refreshed: 1 methods".into(),
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
                topic: None,
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
    fn server_initiated_continuation_turn_with_turn_started_renders_its_answer() {
        // Live-rendering bug (mini5): a single user prompt expanded into
        // server-INITIATED master-continuation turns (reason=child_completed /
        // scatter_join_complete). When the continuation turn's TurnStarted IS
        // delivered, its deltas must accumulate and commit as a real assistant
        // message — never the "did not receive a final assistant answer"
        // fallback card.
        let original_turn = TurnId::new();
        let continuation = TurnId::new();
        let mut store = store_with_live_reply(original_turn.clone(), "original answer");
        let session_id = store.state.sessions[0].id.clone();

        // Original user turn completes first and clears live_reply.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: original_turn,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        // Server-initiated continuation turn: TurnStarted delivered, then deltas.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: continuation.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        for chunk in ["the ", "answer"] {
            store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
                MessageDeltaEvent {
                    session_id: session_id.clone(),
                    topic: None,
                    turn_id: continuation.clone(),
                    text: chunk.into(),
                },
            )));
        }
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: continuation,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let contents: Vec<&str> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert!(
            contents.iter().any(|c| c.contains("the answer")),
            "continuation answer must render: {contents:?}"
        );
        assert!(
            !contents
                .iter()
                .any(|c| c.contains("did not receive a final assistant answer")),
            "continuation turn with deltas must NOT show the fallback card: {contents:?}"
        );
    }

    #[test]
    fn continuation_turn_without_turn_started_lazy_binds_and_renders_answer() {
        // Decisive sub-case: the continuation turn's TurnStarted is NOT delivered
        // to this connection, so deltas arrive against `live_reply == None`. They
        // must LAZY-BIND a fresh live_reply for the turn and accumulate, so the
        // turn commits its real answer instead of the fallback card.
        let original_turn = TurnId::new();
        let continuation = TurnId::new();
        let mut store = store_with_live_reply(original_turn.clone(), "original answer");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: original_turn,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));
        assert!(store.state.sessions[0].live_reply.is_none());

        // No TurnStarted for the continuation: deltas arrive first.
        for chunk in ["the ", "answer"] {
            store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
                MessageDeltaEvent {
                    session_id: session_id.clone(),
                    topic: None,
                    turn_id: continuation.clone(),
                    text: chunk.into(),
                },
            )));
        }
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: continuation,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let contents: Vec<&str> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert!(
            contents.iter().any(|c| c.contains("the answer")),
            "lazy-bound continuation answer must render: {contents:?}"
        );
        assert!(
            !contents
                .iter()
                .any(|c| c.contains("did not receive a final assistant answer")),
            "lazy-bound continuation must NOT show the fallback card: {contents:?}"
        );
    }

    #[test]
    fn continuation_turn_with_no_deltas_keeps_fallback_summary() {
        // Regression guard: a turn that genuinely produces NO assistant deltas
        // must still yield the fallback "did not receive a final assistant
        // answer" summary — lazy-binding must not swallow the legitimate
        // empty-turn case.
        let continuation = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: continuation.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: continuation,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let message = store.state.sessions[0]
            .messages
            .last()
            .expect("fallback assistant message for empty turn");
        assert!(
            message
                .content
                .contains("did not receive a final assistant answer"),
            "empty turn must keep its fallback card: {}",
            message.content
        );
    }

    #[test]
    fn two_sequential_continuation_turns_each_render_their_own_answer() {
        // Two server-initiated continuation turns (e.g. child_completed then
        // scatter_join_complete), each streaming its own deltas, must each
        // commit a DISTINCT assistant message — neither overwritten nor lost.
        let first = TurnId::new();
        let second = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        for (turn, body) in [(&first, "first reply"), (&second, "second reply")] {
            store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
                MessageDeltaEvent {
                    session_id: session_id.clone(),
                    topic: None,
                    turn_id: turn.clone(),
                    text: body.into(),
                },
            )));
            store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
                TurnCompletedEvent {
                    session_id: session_id.clone(),
                    topic: None,
                    turn_id: turn.clone(),
                    cursor: None,
                    tokens_in: None,
                    tokens_out: None,
                    session_result: None,
                },
            )));
        }

        let contents: Vec<&str> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert!(
            contents.iter().any(|c| c.contains("first reply")),
            "first continuation answer lost: {contents:?}"
        );
        assert!(
            contents.iter().any(|c| c.contains("second reply")),
            "second continuation answer lost: {contents:?}"
        );
        assert!(
            !contents
                .iter()
                .any(|c| c.contains("did not receive a final assistant answer")),
            "sequential continuations must not fall back: {contents:?}"
        );
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
                topic: None,
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
    fn turn_completed_reconciles_leaked_running_activity_item() {
        // Orphan activity-chip self-heal: a `ToolStarted` whose matching
        // `ToolCompleted` never arrived (a leaked spawn_only chip / any future
        // uncovered path) leaves a "running"-status item bound to the turn. When
        // the turn reaches its terminal state (`TurnCompleted`) WITHOUT a
        // completing event for that item, capturing the turn's activity must
        // reconcile the stranded running item to a terminal display status so it
        // can no longer count as in-flight ("Orchestrating…").
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "done");
        let session_id = store.state.sessions[0].id.clone();
        store.state.record_submitted_user_prompt(
            session_id.clone(),
            turn_id.clone(),
            "run job".into(),
        );
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-leaked"),
        );

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: turn_id.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let log = store
            .state
            .turn_activity_logs
            .iter()
            .find(|log| log.turn_id == turn_id)
            .expect("turn activity log");
        let leaked = log
            .items
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-leaked"))
            .expect("leaked item retained in log");
        assert_eq!(
            leaked.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "the leaked running item must be reconciled SPECIFICALLY to interrupted \
             (not a false complete/failed), so it renders neutrally and reads as not-running"
        );
    }

    #[test]
    fn turn_error_reconciles_leaked_running_activity_item() {
        // Same self-heal on the error terminal path: a stranded running item
        // must be reconciled when the turn fails too.
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "partial");
        let session_id = store.state.sessions[0].id.clone();
        store.state.record_submitted_user_prompt(
            session_id.clone(),
            turn_id.clone(),
            "run job".into(),
        );
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-leaked"),
        );

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id,
                topic: None,
                turn_id: turn_id.clone(),
                code: "internal".into(),
                message: "boom".into(),
            },
        )));

        let log = store
            .state
            .turn_activity_logs
            .iter()
            .find(|log| log.turn_id == turn_id)
            .expect("turn activity log");
        let leaked = log
            .items
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-leaked"))
            .expect("leaked item retained in log");
        assert_eq!(
            leaked.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "the leaked running item must be reconciled SPECIFICALLY to interrupted on turn error \
             (not a false complete/failed)"
        );
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
                topic: None,
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
                topic: None,
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
                topic: None,
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
                topic: None,
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
    fn message_delta_for_new_turn_commits_prior_and_lazy_binds() {
        // A delta whose turn_id differs from the bound live_reply is NOT a
        // stale frame to drop — on the ordered WS stream it marks the next
        // (e.g. server-initiated continuation) turn. The prior turn's answer is
        // committed as its own assistant message, and a fresh live_reply binds
        // to the new turn so its text is never lost. (Previously this delta was
        // silently dropped — the live-rendering bug.)
        let live_turn_id = TurnId::new();
        let next_turn_id = TurnId::new();
        let mut store = store_with_live_reply(live_turn_id, "hello");
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id,
                topic: None,
                turn_id: next_turn_id.clone(),
                text: "next answer".into(),
            },
        )));

        assert_eq!(
            store.state.sessions[0]
                .messages
                .last()
                .map(|m| m.content.as_str()),
            Some("hello"),
            "prior turn's streamed answer must be committed before the switch"
        );
        let live_reply = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("a fresh live reply binds to the new turn");
        assert_eq!(live_reply.turn_id, next_turn_id);
        assert_eq!(live_reply.text, "next answer");
    }

    #[test]
    fn late_prior_terminal_after_switch_does_not_emit_false_fallback() {
        // Out-of-order lifecycle: deltas lazy-bind turn A (non-empty), then a
        // delta for turn B switches turns — A is committed by the switch — then
        // B completes (live_reply == None). A LATE `TurnCompleted{A}` then
        // arrives. Because A was already finalized at switch time, the late
        // terminal must be a NO-OP: no spurious "did not receive a final
        // assistant answer" fallback card (which the `None` arm would otherwise
        // emit), and no duplicate commit of A's answer.
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        // Deltas lazy-bind turn A (non-empty).
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                text: "answer A".into(),
            },
        )));
        // A delta for turn B switches turns: A is committed at the switch.
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                text: "answer B".into(),
            },
        )));
        // B completes — live_reply is now None.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let before: Vec<String> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect();
        // A's committed answer is present (one copy).
        assert_eq!(
            before.iter().filter(|c| c.contains("answer A")).count(),
            1,
            "turn A must be committed exactly once before the late terminal: {before:?}"
        );

        // LATE TurnCompleted{A} arrives after B already committed.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: turn_a.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let after: Vec<String> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect();
        assert!(
            !after
                .iter()
                .any(|c| c.contains("did not receive a final assistant answer")),
            "late prior terminal after switch must NOT emit a false fallback card: {after:?}"
        );
        assert_eq!(
            after, before,
            "late prior terminal after switch must be a no-op (no new/duplicated messages)"
        );
    }

    #[test]
    fn empty_prior_dropped_on_switch_late_terminal_is_noop() {
        // Empty turn A is bound (TurnStarted, no deltas), then a delta for turn
        // B switches turns: the empty A is DROPPED at the switch and marked as
        // finalized-by-switch. Turn B then completes (live_reply == None). A
        // LATE `TurnCompleted{A}` then arrives. Intended behavior: the late
        // terminal is a NO-OP — the empty turn was already handled (dropped) at
        // switch time. Pre-fix, this hit the `None` arm and emitted a spurious
        // "did not receive a final assistant answer" fallback for a turn that
        // was intentionally dropped; the finalized-by-switch marker suppresses
        // that.
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        // Empty turn A: TurnStarted, no deltas.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_a.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        // Switch to turn B via a delta — empty A is dropped at the switch.
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                text: "answer B".into(),
            },
        )));
        // Turn B completes — live_reply is now None.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let before: Vec<String> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect();
        // B committed its own answer exactly once; no fallback so far.
        assert_eq!(
            before.iter().filter(|c| c.contains("answer B")).count(),
            1,
            "turn B must commit its own answer exactly once: {before:?}"
        );
        assert!(
            !before
                .iter()
                .any(|c| c.contains("did not receive a final assistant answer")),
            "no fallback should exist before the late terminal: {before:?}"
        );

        // LATE TurnCompleted{A} arrives after B has completed (live_reply None).
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: turn_a.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let after: Vec<String> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect();
        // No spurious/duplicate fallback for the dropped empty turn.
        assert!(
            !after
                .iter()
                .any(|c| c.contains("did not receive a final assistant answer")),
            "dropped-empty prior turn's late terminal must NOT emit a fallback card: {after:?}"
        );
        assert_eq!(
            after, before,
            "late terminal for a dropped-empty prior turn must be a no-op"
        );
    }

    #[test]
    fn late_turn_error_after_switch_surfaces_failure_not_swallowed() {
        // The finalized-by-switch marker must suppress only a false COMPLETION
        // fallback — never hide a real ERROR. Turn A streams non-empty text, a
        // delta for turn B switches turns (A is committed + marked at the
        // switch), B completes (live_reply == None), and then a LATE
        // `TurnError{A}` arrives. A switch-finalized turn that genuinely errored
        // MUST surface its failure (its committed text already stands), so the
        // late error is NOT a no-op: it commits a failure card AND flips the
        // session run-state to Error.
        //
        // Pre-fix RED: `fail_live_reply` consumed the marker and early-returned
        // `None`, swallowing the error entirely — no failure card, run-state
        // never went to Error.
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        // Deltas lazy-bind turn A (non-empty).
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                text: "answer A".into(),
            },
        )));
        // A delta for turn B switches turns: A is committed + marked at switch.
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                text: "answer B".into(),
            },
        )));
        // B completes — live_reply is now None.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let committed_a_before = store.state.sessions[0]
            .messages
            .iter()
            .filter(|m| m.content.contains("answer A"))
            .count();
        assert_eq!(
            committed_a_before, 1,
            "turn A must be committed once before the late error"
        );

        // LATE TurnError{A} arrives after B already completed.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id,
                topic: None,
                turn_id: turn_a.clone(),
                code: "provider_error".into(),
                message: "upstream 500 on turn A".into(),
            },
        )));

        let messages: Vec<String> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect();
        // Failure IS surfaced (not swallowed): a failure card for A's error.
        assert!(
            messages.iter().any(
                |c| c.contains("Turn failed before producing a final answer")
                    && c.contains("provider_error: upstream 500 on turn A")
            ),
            "late error for a switch-finalized turn must surface a failure card: {messages:?}"
        );
        // A's already-committed text still stands.
        assert_eq!(
            messages.iter().filter(|c| c.contains("answer A")).count(),
            1,
            "A's committed text must remain after its late error: {messages:?}"
        );
        // Run-state reflects the error.
        assert!(
            matches!(
                store.state.run_state,
                crate::model::SessionRunState::Error { .. }
            ),
            "late error must drive run-state to Error, got {:?}",
            store.state.run_state
        );
    }

    #[test]
    fn late_turn_error_after_switch_surfaces_failure_with_b_still_live() {
        // Same as above but the successor turn B is STILL streaming (live_reply
        // bound to B) when the late `TurnError{A}` arrives. The error for the
        // switch-finalized turn A must still be surfaced, and B's in-flight
        // live_reply must be preserved untouched (the error is for A, not B).
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                text: "answer A".into(),
            },
        )));
        // Switch to B (A committed + marked); B keeps streaming (still live).
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                text: "answer B in progress".into(),
            },
        )));

        // LATE TurnError{A} arrives while B is still live.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id,
                topic: None,
                turn_id: turn_a.clone(),
                code: "provider_error".into(),
                message: "upstream 500 on turn A".into(),
            },
        )));

        let messages: Vec<String> = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect();
        assert!(
            messages.iter().any(
                |c| c.contains("Turn failed before producing a final answer")
                    && c.contains("provider_error: upstream 500 on turn A")
            ),
            "late error for A must surface even while B is still live: {messages:?}"
        );
        // B's in-flight live_reply is preserved (untouched by A's error).
        let live = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("B's live_reply must remain bound");
        assert_eq!(live.turn_id, turn_b, "B must remain the live turn");
        assert_eq!(
            live.text, "answer B in progress",
            "B's streamed text must be preserved"
        );
    }

    #[test]
    fn switch_dropped_empty_turn_with_activity_keeps_activity_visible() {
        // An empty (no assistant deltas) turn A that nevertheless DID run tool
        // activity is switched away from to turn B. The empty live_reply text is
        // dropped at the switch (no assistant message, no "did not receive
        // answer" card — A was superseded). But A's tool ACTIVITY must NOT be
        // lost: it must be captured into `turn_activity_logs` (the chip source)
        // so it stays visible, exactly as the non-empty commit path captures it.
        //
        // Pre-fix RED: the empty-drop path skipped `capture_completed_turn_
        // activity`, so A's tool items were left orphaned in `state.activity`
        // (filtered out of the live flow once B is the active turn) and never
        // archived to a log — invisible after the switch.
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        // Turn A starts and runs a tool, but streams NO assistant deltas.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_a.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
            ToolStartedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                tool_call_id: "call-A".into(),
                tool_name: "shell".into(),
                arguments: Some(serde_json::json!({"command": "ls"})),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                tool_call_id: "call-A".into(),
                tool_name: "shell".into(),
                success: Some(true),
                output_preview: Some("files".into()),
                duration_ms: Some(10),
            },
        )));

        // A delta for turn B switches turns: empty A's text is dropped.
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                text: "answer B".into(),
            },
        )));

        // A produced no assistant message (it was an empty, superseded turn).
        let has_a_message = store.state.sessions[0]
            .messages
            .iter()
            .any(|m| m.role.as_str() == "assistant");
        assert!(
            !has_a_message,
            "empty switch-dropped turn A must not produce an assistant message"
        );

        // A's tool activity is preserved in the chip source (turn_activity_logs)
        // — not lost. The non-empty path would capture it the same way.
        let a_log = store
            .state
            .turn_activity_logs
            .iter()
            .find(|log| log.turn_id == turn_a);
        assert!(
            a_log.is_some(),
            "empty switch-dropped turn A's activity must be captured to turn_activity_logs"
        );
        assert!(
            a_log
                .unwrap()
                .items
                .iter()
                .any(|item| item.title == "shell"),
            "turn A's shell tool activity must be represented in its captured log"
        );

        // A's late terminal (a COMPLETION) is still a no-op (marker consumed) —
        // it does not double-capture or emit a fallback card.
        let logs_before = store.state.turn_activity_logs.len();
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: turn_a.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));
        assert!(
            !store.state.sessions[0].messages.iter().any(|m| m
                .content
                .contains("did not receive a final assistant answer")),
            "late COMPLETION for the dropped-empty turn must remain a no-op (no fallback card)"
        );
        assert_eq!(
            store.state.turn_activity_logs.len(),
            logs_before,
            "late COMPLETION for the dropped-empty turn must not change captured logs"
        );
    }

    #[test]
    fn late_error_card_for_switch_finalized_turn_reports_real_action_count() {
        // Count-honesty nit: a late `TurnError` for a SWITCH-FINALIZED turn must
        // report the turn's TRUE action count on its failure card, not 0. Turn A
        // runs N (5) tools, then a delta for turn B switches turns — which
        // ARCHIVES A's activity into `turn_activity_logs` and removes it from the
        // live `state.activity`. B completes, then a LATE `TurnError{A}` surfaces
        // A's failure card. The card's "Activity: N action(s)" must read the
        // archived log (where A's items now live), not the empty live set.
        //
        // Pre-fix RED: `turn_error_fallback_message` summarized only the live
        // `state.activity`, which no longer holds A's items, so the card reported
        // "Activity: 0 action(s) recorded." even though A really ran 5 tools.
        const N: usize = 5;
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        // Turn A starts and runs N completed tools (no assistant deltas).
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_a.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        for i in 0..N {
            let call_id = format!("call-A-{i}");
            store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
                ToolStartedEvent {
                    session_id: session_id.clone(),
                    topic: None,
                    turn_id: turn_a.clone(),
                    tool_call_id: call_id.clone(),
                    tool_name: "shell".into(),
                    arguments: Some(serde_json::json!({"command": "ls"})),
                },
            )));
            store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
                ToolCompletedEvent {
                    session_id: session_id.clone(),
                    topic: None,
                    turn_id: turn_a.clone(),
                    tool_call_id: call_id,
                    tool_name: "shell".into(),
                    success: Some(true),
                    output_preview: Some("files".into()),
                    duration_ms: Some(10),
                },
            )));
        }

        // A delta for turn B switches turns: A is finalized + its activity is
        // archived to turn_activity_logs and removed from live state.activity.
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                text: "answer B".into(),
            },
        )));
        // Sanity: A's items are no longer in the live activity (archived).
        assert!(
            !store
                .state
                .activity
                .iter()
                .any(|item| item.turn_id.as_ref() == Some(&turn_a)),
            "turn A's activity must have been archived out of live state.activity"
        );
        // B completes — live_reply is now None.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_b.clone(),
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        // LATE TurnError{A} arrives after B already completed.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id,
                topic: None,
                turn_id: turn_a.clone(),
                code: "provider_error".into(),
                message: "upstream 500 on turn A".into(),
            },
        )));

        let card = store.state.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.clone())
            .find(|c| c.contains("Turn failed before producing a final answer"))
            .expect("a failure card for A's late error must be surfaced");
        // The count must reflect A's TRUE action count from the archived log.
        assert!(
            card.contains(&format!("Activity: {N} action(s) recorded.")),
            "switch-finalized turn's failure card must report its real action \
             count ({N}), not 0: {card:?}"
        );
        assert!(
            !card.contains("Activity: 0 action(s) recorded."),
            "switch-finalized turn's failure card must NOT report 0 actions: {card:?}"
        );
    }

    #[test]
    fn turn_started_after_delta_preserves_same_turn_buffer() {
        // nit 1: a MessageDelta lazy-binds turn A with "partial ", then a
        // TurnStarted for the SAME turn A is delivered/replayed. The previously
        // accumulated text must be PRESERVED (not wiped), so a subsequent delta
        // appends and the committed assistant message is the full "partial rest".
        let turn_a = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                text: "partial ".into(),
            },
        )));
        // Same-turn TurnStarted arrives AFTER the lazy-bound delta.
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_a.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::MessageDelta(
            MessageDeltaEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_a.clone(),
                text: "rest".into(),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id: turn_a,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        let committed = store.state.sessions[0]
            .messages
            .last()
            .expect("turn A commits an assistant message")
            .content
            .clone();
        assert!(
            committed.contains("partial rest"),
            "same-turn TurnStarted must preserve the lazy-bound buffer (got: {committed:?})"
        );
    }

    fn envelope_notification(session_id: SessionKey, seq: u64, payload: Payload) -> UiNotification {
        UiNotification::Envelope(EnvelopeNotification {
            session_id,
            topic: None,
            envelope: Envelope {
                thread_id: "thread-1".into(),
                seq,
                client_message_id: None,
                payload,
            },
        })
    }

    #[test]
    fn envelope_assistant_delta_projects_into_threaded_message() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id.clone(),
            1,
            Payload::AssistantDelta {
                text: "hello".into(),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id,
            2,
            Payload::AssistantDelta {
                text: " world".into(),
            },
        )));

        let messages = &store.state.sessions[0].messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::Assistant);
        assert_eq!(messages[0].thread_id.as_deref(), Some("thread-1"));
        assert_eq!(messages[0].content, "hello world");
    }

    #[test]
    fn envelope_assistant_delta_does_not_churn_status_bar() {
        // mini5 soak UX: streaming deltas arrive many times/sec and used to
        // overwrite the status bar with "Assistant delta projected for
        // <thread_id>", churning the bottom-of-composer line. The projection is
        // internal bookkeeping — it must NOT touch the status bar (the streamed
        // text is already visible in the transcript).
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        store.state.status = "Working".into();

        for seq in 1..=3 {
            store.apply_event(AppUiEvent::Protocol(envelope_notification(
                session_id.clone(),
                seq,
                Payload::AssistantDelta {
                    text: "tok ".into(),
                },
            )));
        }

        assert_eq!(
            store.state.status, "Working",
            "assistant-delta projection must not overwrite the status bar"
        );
        // The delta still projects into the transcript — only the status churn is gone.
        assert_eq!(store.state.sessions[0].messages.len(), 1);
    }

    #[test]
    fn envelope_assistant_persisted_replaces_streamed_text() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id.clone(),
            1,
            Payload::AssistantDelta {
                text: "draft".into(),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id,
            2,
            Payload::AssistantPersisted {
                text: "final answer".into(),
                meta: octos_core::ui_protocol::MessageMeta {
                    message_id: "message-1".into(),
                    persisted_at: chrono::Utc::now(),
                    media: vec![],
                },
            },
        )));

        let messages = &store.state.sessions[0].messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::Assistant);
        assert_eq!(messages[0].thread_id.as_deref(), Some("thread-1"));
        assert_eq!(messages[0].content, "final answer");
    }

    #[test]
    fn envelope_turn_completed_reconciles_stranded_running_tool_item() {
        // GAP 2: the M9-γ projection-envelope path creates tool activity with NO
        // turn_id (the envelope is keyed on thread_id/seq — there is no turn
        // identity on the wire). A `ToolStart` whose `ToolEnd` never arrived
        // leaves a turn-less "running" Tool item. When `Payload::TurnCompleted`
        // closes the thread (a hard terminal barrier), that stranded running item
        // must be reconciled so it can no longer pin a turn-less "Orchestrating…"
        // chip.
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id.clone(),
            1,
            Payload::ToolStart {
                tool_call_id: "call-leaked".into(),
                name: "run_pipeline".into(),
            },
        )));
        // Terminal barrier for the thread — no ToolEnd ever came for call-leaked.
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id,
            2,
            Payload::TurnCompleted {
                token_usage: octos_core::ui_protocol::EnvelopeTokenUsage::default(),
            },
        )));

        let leaked = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-leaked"))
            .expect("leaked envelope tool item retained");
        assert!(
            !crate::model::activity_status_is_running(&leaked.status),
            "an envelope TurnCompleted must leave no running-status chip pinned, got {:?}",
            leaked.status
        );
        assert_eq!(
            leaked.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "the stranded envelope tool item must be reconciled to interrupted"
        );
    }

    #[test]
    fn envelope_turn_completed_does_not_touch_settled_tool_item() {
        // GAP 2 guard: a tool that DID complete (ToolEnd arrived) must keep its
        // settled status across a TurnCompleted barrier — the reconcile only
        // sweeps still-running items.
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id.clone(),
            1,
            Payload::ToolStart {
                tool_call_id: "call-done".into(),
                name: "run_pipeline".into(),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id.clone(),
            2,
            Payload::ToolEnd {
                tool_call_id: "call-done".into(),
                status: EnvelopeToolEndStatus::Complete,
                error: None,
                reason: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id,
            3,
            Payload::TurnCompleted {
                token_usage: octos_core::ui_protocol::EnvelopeTokenUsage::default(),
            },
        )));

        let done = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-done"))
            .expect("settled envelope tool item retained");
        assert_eq!(
            done.status, "complete",
            "a settled envelope tool item must keep its terminal status across TurnCompleted"
        );
    }

    #[test]
    fn envelope_turn_completed_does_not_suppress_other_session_same_thread() {
        // GAP 2 over-suppression guard: two sessions can each be running an
        // envelope tool under the SAME thread_id (thread_id is NOT globally
        // unique — it is scoped to a session's projection). A `TurnCompleted`
        // for session A's thread must heal ONLY session A's stranded running
        // envelope tool item; session B's genuinely-active chip on the same
        // thread_id must stay running.
        let mut store = store_with_two_sessions("local:a", "local:b");
        let session_a = store.state.sessions[0].id.clone();
        let session_b = store.state.sessions[1].id.clone();
        assert_eq!(session_a, SessionKey("local:a".into()));
        assert_eq!(session_b, SessionKey("local:b".into()));

        // Both sessions start a turn-less running envelope tool on "thread-1"
        // (envelope_notification hardcodes thread_id = "thread-1").
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_a.clone(),
            1,
            Payload::ToolStart {
                tool_call_id: "call-a".into(),
                name: "run_pipeline".into(),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_b.clone(),
            1,
            Payload::ToolStart {
                tool_call_id: "call-b".into(),
                name: "run_pipeline".into(),
            },
        )));

        // Terminal barrier for session A's thread only.
        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_a,
            2,
            Payload::TurnCompleted {
                token_usage: octos_core::ui_protocol::EnvelopeTokenUsage::default(),
            },
        )));

        let item_a = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-a"))
            .expect("session A envelope tool item retained");
        assert_eq!(
            item_a.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "session A's stranded envelope tool item must be reconciled"
        );

        let item_b = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-b"))
            .expect("session B envelope tool item retained");
        assert!(
            crate::model::activity_status_is_running(&item_b.status),
            "session B's genuinely-active envelope tool item must NOT be suppressed \
             by session A's TurnCompleted on the same thread_id, got {:?}",
            item_b.status
        );
    }

    /// Build the FLATTENED `projection/envelope` wire `params` the
    /// server now emits (feat(envelope-wire-routing)): bare Envelope
    /// fields at the top level PLUS `session_id` + optional `topic`.
    fn envelope_wire_params(
        session_id: &str,
        thread_id: &str,
        seq: u64,
        payload: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "session_id": session_id,
            "thread_id": thread_id,
            "seq": seq,
            "payload": payload,
        })
    }

    /// Decode a wire `projection/envelope` frame through the SAME
    /// `from_method_and_params` path the transport uses, then apply it.
    /// This exercises the real DECODE — not a directly-constructed
    /// `EnvelopeNotification` — so the wire-routing contract is under
    /// test end-to-end.
    fn apply_envelope_wire_frame(store: &mut Store, params: serde_json::Value) {
        let notif = UiNotification::from_method_and_params(
            octos_core::ui_protocol::methods::PROJECTION_ENVELOPE,
            params,
        )
        .expect("wire envelope frame must decode");
        store.apply_event(AppUiEvent::Protocol(notif));
    }

    /// feat(envelope-wire-routing) — THE full-decode-path guard codex
    /// asked for. Two sessions share `thread_id = "thread-1"`. We drive
    /// the REAL wire path (build flattened `projection/envelope` params
    /// → `from_method_and_params` → `apply_envelope`) and assert:
    ///   (a) each session's envelope routes to the CORRECT session — not
    ///       dropped on an empty `session_id` key, and
    ///   (b) session A's `TurnCompleted` reconciles A's stranded chip but
    ///       NOT session B's genuinely-running chip on the same thread.
    ///
    /// RED on the pre-change core: the wire stripped `session_id`, so
    /// every decoded envelope arrived with an EMPTY `SessionKey`,
    /// `find_session_mut` failed, messages were dropped, and the
    /// session-scoped reconcile could not distinguish A from B.
    #[test]
    fn envelope_wire_decode_routes_to_correct_session_and_scopes_reconcile() {
        let mut store = store_with_two_sessions("local:a", "local:b");
        let session_a = store.state.sessions[0].id.clone();
        let session_b = store.state.sessions[1].id.clone();
        assert_eq!(session_a, SessionKey("local:a".into()));
        assert_eq!(session_b, SessionKey("local:b".into()));

        // (a) Route a user_message to each session via the wire decode.
        apply_envelope_wire_frame(
            &mut store,
            envelope_wire_params(
                "local:a",
                "thread-1",
                1,
                serde_json::json!({
                    "type": "user_message",
                    "data": { "text": "hello from A", "files": [] }
                }),
            ),
        );
        apply_envelope_wire_frame(
            &mut store,
            envelope_wire_params(
                "local:b",
                "thread-1",
                1,
                serde_json::json!({
                    "type": "user_message",
                    "data": { "text": "hello from B", "files": [] }
                }),
            ),
        );

        // Each message landed in its OWN session (would be dropped on an
        // empty session_id on the pre-change wire).
        let a_msgs = &store.state.sessions[0].messages;
        let b_msgs = &store.state.sessions[1].messages;
        assert_eq!(
            a_msgs.len(),
            1,
            "session A must receive exactly its message"
        );
        assert_eq!(a_msgs[0].content, "hello from A");
        assert_eq!(
            b_msgs.len(),
            1,
            "session B must receive exactly its message"
        );
        assert_eq!(b_msgs[0].content, "hello from B");

        // Both sessions start a turn-less running envelope tool on the
        // SHARED thread-1, routed via the wire decode.
        apply_envelope_wire_frame(
            &mut store,
            envelope_wire_params(
                "local:a",
                "thread-1",
                2,
                serde_json::json!({
                    "type": "tool_start",
                    "data": { "tool_call_id": "call-a", "name": "run_pipeline" }
                }),
            ),
        );
        apply_envelope_wire_frame(
            &mut store,
            envelope_wire_params(
                "local:b",
                "thread-1",
                2,
                serde_json::json!({
                    "type": "tool_start",
                    "data": { "tool_call_id": "call-b", "name": "run_pipeline" }
                }),
            ),
        );

        // (b) TurnCompleted for session A's thread ONLY — routed by the
        // decoded session_id.
        apply_envelope_wire_frame(
            &mut store,
            envelope_wire_params(
                "local:a",
                "thread-1",
                3,
                serde_json::json!({
                    "type": "turn_completed",
                    "data": { "token_usage": {} }
                }),
            ),
        );

        let item_a = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-a"))
            .expect("session A envelope tool item retained");
        assert_eq!(
            item_a.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "session A's stranded chip must reconcile via the decoded session_id",
        );

        let item_b = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-b"))
            .expect("session B envelope tool item retained");
        assert!(
            crate::model::activity_status_is_running(&item_b.status),
            "session B's chip on the SAME thread must NOT be suppressed by \
             session A's TurnCompleted; got {:?}",
            item_b.status
        );
    }

    /// Build the wire `projection/envelope` `params` the SERVER actually
    /// emits when the turn is TOPIC-scoped: the emitting
    /// `EnvelopeNotification.session_id` carries the `"base#topic"`
    /// composite key (`turn/start` folds the topic in). Driving it
    /// through the REAL `into_rpc_notification` boundary exercises the
    /// core wire-normalization (session_id → bare base, topic preserved),
    /// so this guard fails if that normalization regresses.
    fn topic_folded_envelope_wire_params(
        base_topic_session_id: &str,
        thread_id: &str,
        seq: u64,
        payload: Payload,
    ) -> serde_json::Value {
        let notif = UiNotification::Envelope(EnvelopeNotification {
            session_id: SessionKey(base_topic_session_id.into()),
            topic: None,
            envelope: Envelope {
                thread_id: thread_id.into(),
                seq,
                client_message_id: None,
                payload,
            },
        });
        notif
            .into_rpc_notification()
            .expect("topic-folded envelope serializes to the wire")
            .params
    }

    /// feat(envelope-wire-routing) — codex BLOCKER, TUI decode-path guard.
    /// On a TOPIC turn the server's `EnvelopeNotification.session_id` is
    /// the composite `"local:a#research"` key. The TUI knows only the
    /// BARE base session `"local:a"`, so it must route by the base key.
    /// We drive the FULL real path: build the wire from the topic-folded
    /// notification → `into_rpc_notification` (core normalizes the wire
    /// session_id to the base) → `from_method_and_params` →
    /// `apply_envelope`, and assert:
    ///   (a) the message lands in the BARE `"local:a"` session
    ///       (`find_session_mut` succeeds), and
    ///   (b) the topic-turn's `TurnCompleted` reconcile scopes to the
    ///       BARE `"local:a"` session — healing its own stranded chip
    ///       without touching a sibling.
    ///
    /// RED before the core fix: the wire carried `"local:a#research"`, so
    /// `find_session_mut` and the reconcile both searched the composite
    /// key, missing the real `"local:a"` session → message dropped and
    /// the self-heal scoped to the wrong key.
    #[test]
    fn topic_folded_envelope_wire_routes_to_base_session_and_scopes_reconcile() {
        let mut store = store_with_two_sessions("local:a", "local:b");
        let session_a = store.state.sessions[0].id.clone();
        assert_eq!(session_a, SessionKey("local:a".into()));

        // (a) A topic-scoped user_message for session A's research topic.
        apply_envelope_wire_frame(
            &mut store,
            topic_folded_envelope_wire_params(
                "local:a#research",
                "thread-topic",
                1,
                Payload::UserMessage {
                    text: "topic msg for A".into(),
                    files: vec![],
                },
            ),
        );
        let a_msgs = &store.state.sessions[0].messages;
        assert_eq!(
            a_msgs.len(),
            1,
            "topic-scoped message must route to the BARE base session, \
             not a composite base#topic key",
        );
        assert_eq!(a_msgs[0].content, "topic msg for A");
        assert!(
            store.state.sessions[1].messages.is_empty(),
            "session B must not receive session A's topic message",
        );

        // A turn-less running tool on session A's topic thread.
        apply_envelope_wire_frame(
            &mut store,
            topic_folded_envelope_wire_params(
                "local:a#research",
                "thread-topic",
                2,
                Payload::ToolStart {
                    tool_call_id: "call-topic".into(),
                    name: "run_pipeline".into(),
                },
            ),
        );

        // The chip must be tagged with the BARE base session key so the
        // session-scoped reconcile (which matches on the base key) finds
        // it. RED on the pre-fix wire (tagged "local:a#research").
        let chip = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-topic"))
            .expect("topic envelope tool item retained");
        assert_eq!(
            chip.session_id.as_ref(),
            Some(&SessionKey("local:a".into())),
            "chip must be scoped to the bare base session key",
        );

        // (b) The topic turn's TurnCompleted reconciles A's stranded chip
        // via the BARE base key.
        apply_envelope_wire_frame(
            &mut store,
            topic_folded_envelope_wire_params(
                "local:a#research",
                "thread-topic",
                3,
                Payload::TurnCompleted {
                    token_usage: Default::default(),
                },
            ),
        );
        let chip = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-topic"))
            .expect("topic envelope tool item retained after turn complete");
        assert_eq!(
            chip.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "topic turn's TurnCompleted must reconcile the chip scoped to \
             the bare base session key",
        );
    }

    /// feat(envelope-wire-routing) backward-compat at the consumer: an
    /// OLD bare-envelope wire frame (no `session_id`) still decodes
    /// without error through the transport path. The routing key is
    /// empty so it cannot match a session — but it must NOT crash the
    /// decode (it is silently un-routed, the legacy behaviour).
    #[test]
    fn legacy_bare_envelope_wire_frame_decodes_without_routing() {
        let mut store = store_with_two_sessions("local:a", "local:b");
        // No session_id key — the pre-change wire shape.
        let legacy = serde_json::json!({
            "thread_id": "thread-1",
            "seq": 1,
            "payload": {
                "type": "assistant_delta",
                "data": { "text": "orphaned delta" }
            }
        });
        apply_envelope_wire_frame(&mut store, legacy);
        // Un-routed (empty session_id matches no session) — neither
        // session gains a message, and nothing panicked.
        assert!(store.state.sessions[0].messages.is_empty());
        assert!(store.state.sessions[1].messages.is_empty());
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
                    topic: None,
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
                    topic: None,
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
                topic: None,
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

    // ── UPCR-2026-023 AskUserQuestion picker reducer tests ──────────────

    use octos_core::ui_protocol::{
        QuestionId, UserQuestion, UserQuestionOption, UserQuestionRequestedEvent,
    };

    fn option(label: &str, description: &str) -> UserQuestionOption {
        UserQuestionOption {
            label: label.into(),
            description: description.into(),
        }
    }

    fn single_select(
        header: &str,
        question: &str,
        options: Vec<UserQuestionOption>,
    ) -> UserQuestion {
        UserQuestion {
            header: header.into(),
            question: question.into(),
            options,
            multi_select: false,
            allow_free_text: true,
        }
    }

    fn multi_select(
        header: &str,
        question: &str,
        options: Vec<UserQuestionOption>,
    ) -> UserQuestion {
        UserQuestion {
            header: header.into(),
            question: question.into(),
            options,
            multi_select: true,
            allow_free_text: true,
        }
    }

    fn open_user_question(
        store: &mut Store,
        questions: Vec<UserQuestion>,
    ) -> (SessionKey, QuestionId, TurnId) {
        let session_id = store.state.sessions[0].id.clone();
        let question_id = QuestionId::new();
        let turn_id = TurnId::new();
        store.apply_event(AppUiEvent::Protocol(UiNotification::UserQuestionRequested(
            UserQuestionRequestedEvent::new(
                session_id.clone(),
                question_id.clone(),
                turn_id.clone(),
                "Pick a framework",
                "The agent needs a framework choice to proceed.",
                questions,
            ),
        )));
        (session_id, question_id, turn_id)
    }

    #[test]
    fn user_question_requested_puts_picker_in_state_with_questions_and_options() {
        let mut store = store_with_live_reply(TurnId::new(), "working");
        let (session_id, question_id, turn_id) = open_user_question(
            &mut store,
            vec![single_select(
                "Framework",
                "Which web framework?",
                vec![
                    option("axum", "tokio-native"),
                    option("actix", "actor-based"),
                ],
            )],
        );

        let picker = store.state.user_question.as_ref().expect("picker visible");
        assert!(picker.visible);
        assert_eq!(picker.session_id, session_id);
        assert_eq!(picker.question_id, question_id);
        assert_eq!(picker.turn_id, turn_id);
        assert_eq!(picker.title, "Pick a framework");
        assert_eq!(picker.questions.len(), 1);
        let entry = picker.active_question().expect("active question");
        assert_eq!(entry.header, "Framework");
        assert_eq!(entry.options.len(), 2);
        assert!(!entry.multi_select);
        // Picker pauses the turn like an open approval.
        assert_eq!(store.state.run_state.label(), "blocked");
    }

    #[test]
    fn user_question_single_select_takes_at_most_one_label() {
        let mut store = store_with_live_reply(TurnId::new(), "working");
        let (session_id, question_id, _) = open_user_question(
            &mut store,
            vec![single_select(
                "Framework",
                "Which web framework?",
                vec![option("axum", "tokio"), option("actix", "actor")],
            )],
        );

        // Highlight + select the first option, then select the second: single
        // select must drop the first.
        store.user_question_toggle(); // axum
        store.user_question_cursor_down(); // -> actix
        store.user_question_toggle(); // actix (clears axum)

        let command = store
            .respond_user_question_command()
            .expect("respond command");
        let AppUiCommand::RespondUserQuestion(params) = command else {
            panic!("expected user_question respond command");
        };
        assert_eq!(params.session_id, session_id);
        assert_eq!(params.question_id, question_id);
        assert_eq!(params.answers.len(), 1);
        assert_eq!(params.answers[0].selected_labels, vec!["actix".to_string()]);
        assert_eq!(params.answers[0].free_text, None);
        // Picker cleared, run-state resumed.
        assert!(store.state.user_question.is_none());
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn user_question_multi_select_takes_multiple_labels() {
        let mut store = store_with_live_reply(TurnId::new(), "working");
        let (_, _, _) = open_user_question(
            &mut store,
            vec![multi_select(
                "Targets",
                "Which build targets?",
                vec![
                    option("stable", ""),
                    option("msrv", ""),
                    option("nightly", ""),
                ],
            )],
        );

        store.user_question_toggle(); // stable
        store.user_question_cursor_down();
        store.user_question_cursor_down();
        store.user_question_toggle(); // nightly

        let command = store
            .respond_user_question_command()
            .expect("respond command");
        let AppUiCommand::RespondUserQuestion(params) = command else {
            panic!("expected respond command");
        };
        assert_eq!(params.answers.len(), 1);
        assert_eq!(
            params.answers[0].selected_labels,
            vec!["stable".to_string(), "nightly".to_string()]
        );
    }

    #[test]
    fn user_question_free_text_other_path_carries_free_text() {
        let mut store = store_with_live_reply(TurnId::new(), "working");
        open_user_question(
            &mut store,
            vec![single_select(
                "Framework",
                "Which?",
                vec![option("axum", ""), option("actix", "")],
            )],
        );

        // Type into the "Other" box without selecting any option.
        for ch in "rocket".chars() {
            store.user_question_push_free_text(ch);
        }
        assert!(store.user_question_editing_free_text());

        let command = store
            .respond_user_question_command()
            .expect("respond command");
        let AppUiCommand::RespondUserQuestion(params) = command else {
            panic!("expected respond command");
        };
        assert!(params.answers[0].selected_labels.is_empty());
        assert_eq!(params.answers[0].free_text.as_deref(), Some("rocket"));
    }

    #[test]
    fn user_question_multi_question_carries_per_question_answers_in_order() {
        let mut store = store_with_live_reply(TurnId::new(), "working");
        open_user_question(
            &mut store,
            vec![
                single_select(
                    "Q1",
                    "Framework?",
                    vec![option("axum", ""), option("actix", "")],
                ),
                multi_select(
                    "Q2",
                    "Targets?",
                    vec![option("stable", ""), option("nightly", "")],
                ),
            ],
        );

        // Q1: pick axum, then Enter steps to Q2 (does not submit).
        store.user_question_toggle();
        assert!(!store.user_question_advance());
        assert_eq!(store.state.user_question.as_ref().unwrap().active, 1);

        // Q2: pick stable + nightly.
        store.user_question_toggle(); // stable
        store.user_question_cursor_down();
        store.user_question_toggle(); // nightly

        // On the last question advance signals ready-to-submit.
        assert!(store.user_question_advance());
        let command = store
            .respond_user_question_command()
            .expect("respond command");
        let AppUiCommand::RespondUserQuestion(params) = command else {
            panic!("expected respond command");
        };
        assert_eq!(params.answers.len(), 2);
        assert_eq!(params.answers[0].selected_labels, vec!["axum".to_string()]);
        assert_eq!(
            params.answers[1].selected_labels,
            vec!["stable".to_string(), "nightly".to_string()]
        );
    }

    #[test]
    fn unknown_garbled_user_question_still_renders_title_body_and_is_recoverable() {
        // A client that gets an event with NO structured questions must still
        // show title/body as an informational fallback card, and stay
        // dismissible/recoverable — but it must NOT submit, since a respond for
        // a garbled 0-question event cannot form a valid (count-matched) answer
        // set. (DO-NOT-SHIP #2.)
        let mut store = store_with_live_reply(TurnId::new(), "working");
        let (_, _, _) = open_user_question(&mut store, Vec::new());

        let picker = store.state.user_question.as_ref().expect("picker visible");
        assert_eq!(picker.title, "Pick a framework");
        assert_eq!(
            picker.body,
            "The agent needs a framework choice to proceed."
        );
        assert!(picker.questions.is_empty());

        // No submit: the picker is NOT consumed and no command is produced, so
        // we never send a mismatched respond.
        assert!(store.respond_user_question_command().is_none());
        assert!(
            store.state.user_question.is_some(),
            "picker must not be consumed on an unsubmittable garbled event"
        );

        // Still dismissible (Esc) and recoverable (#1) without submitting.
        assert!(store.close_modal());
        assert!(!store.state.user_question.as_ref().unwrap().visible);
        assert!(store.show_pending_user_question());
        assert!(store.state.user_question.as_ref().unwrap().visible);
    }

    #[test]
    fn zero_question_event_does_not_send_invalid_respond() {
        // DO-NOT-SHIP #2: a `user_question/requested` with empty `questions` is a
        // protocol-violation / garbled event. `to_respond_params()` must emit
        // EXACTLY questions.len() answers (== 0 here), never a manufactured empty
        // answer that the backend validator (answers.len()==questions.len())
        // would reject; and the submit path must NOT fire (no command, picker
        // preserved + recoverable).
        let mut store = store_with_live_reply(TurnId::new(), "working");
        open_user_question(&mut store, Vec::new());

        let picker = store.state.user_question.as_ref().expect("picker visible");
        assert!(picker.questions.is_empty());
        // The respond params for 0 questions carry 0 answers (never 1).
        assert_eq!(picker.to_respond_params().answers.len(), 0);

        // The submit path is a no-op: no RespondUserQuestion is produced and the
        // picker is preserved (so it stays dismissible + recoverable).
        assert!(store.respond_user_question_command().is_none());
        assert!(store.state.user_question.is_some());
        // The run-state must not be falsely resumed by a non-submit.
        assert_eq!(store.state.run_state.label(), "blocked");
    }

    #[test]
    fn user_question_cleared_on_finalized_by_switch_terminal() {
        // nit: commit_live_reply() may early-return when the turn was already
        // finalized by a turn-switch. A pending question for that terminal turn
        // must still be cleared BEFORE that early return, so a stale terminal
        // does not leave the picker wedged.
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "working");
        let (session_id, _, _) = open_user_question(
            &mut store,
            vec![single_select(
                "Q",
                "?",
                vec![option("a", ""), option("b", "")],
            )],
        );
        // Bind the picker to the turn that is about to terminate.
        store.state.user_question.as_mut().unwrap().turn_id = turn_id.clone();
        assert!(store.state.user_question.is_some());

        // Mark the turn as already-finalized-by-switch so commit_live_reply takes
        // the early-return branch.
        store
            .state
            .mark_turn_finalized_by_switch(&session_id, &turn_id);

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id,
                topic: None,
                turn_id,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        // Even though the terminal early-returned, the stale picker is cleared.
        assert!(
            store.state.user_question.is_none(),
            "stale picker must clear even on the finalized-by-switch early return"
        );
    }

    #[test]
    fn stale_user_question_clears_on_turn_error_without_wedging() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "working");
        let (session_id, _, _) = open_user_question(
            &mut store,
            vec![single_select(
                "Q",
                "?",
                vec![option("a", ""), option("b", "")],
            )],
        );
        // Bind the picker turn to the live turn so the terminal clears it.
        store.state.user_question.as_mut().unwrap().turn_id = turn_id.clone();
        assert!(store.state.user_question.is_some());

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id,
                code: "cancelled".into(),
                message: "turn interrupted".into(),
            },
        )));

        // No wedge: the picker is gone and a later respond is a clean no-op.
        assert!(store.state.user_question.is_none());
        assert!(store.respond_user_question_command().is_none());
        assert_eq!(store.state.status, "No active question to answer");
    }

    #[test]
    fn close_modal_hides_pending_user_question_without_responding() {
        let mut store = store_with_empty_session();
        open_user_question(
            &mut store,
            vec![single_select(
                "Q",
                "?",
                vec![option("a", ""), option("b", "")],
            )],
        );
        assert!(store.state.user_question.as_ref().unwrap().visible);

        assert!(store.close_modal());
        // Still present (not answered) but hidden, and auto-open disabled.
        let picker = store.state.user_question.as_ref().expect("still pending");
        assert!(!picker.visible);
        assert!(!store.state.user_question_auto_open);
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
                topic: None,
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                tool_name: "shell".into(),
                arguments: Some(serde_json::json!({"command": "cargo test"})),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
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
                topic: None,
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
                topic: None,
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                tool_name: "shell".into(),
                arguments: Some(serde_json::json!({"command": "cargo test"})),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolProgress(
            octos_core::ui_protocol::ToolProgressEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: turn_id.clone(),
                tool_call_id: "call-1".into(),
                message: Some("cargo test".into()),
                progress_pct: Some(50.0),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id,
                topic: None,
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
                topic: None,
                turn_id: turn_id.clone(),
                tool_call_id: tool_call_id.clone(),
                tool_name: "shell".into(),
                arguments: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolCompleted(
            ToolCompletedEvent {
                session_id,
                topic: None,
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
                topic: None,
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
    fn cancel_task_command_targets_selected_running_task() {
        let task_id = TaskId::new();
        let mut store = store_with_task(task_id.clone());
        store
            .state
            .set_capabilities(UiProtocolCapabilities::new(&[methods::TASK_CANCEL], &[]));
        let session_id = store.state.sessions[0].id.clone();

        let command = store
            .cancel_task_command()
            .expect("a running task is cancellable");

        let AppUiCommand::CancelTask(params) = command else {
            panic!("expected task cancel command");
        };
        assert_eq!(params.task_id, task_id);
        assert_eq!(params.session_id, Some(session_id));
        assert!(store.state.status.starts_with("Requested cancel"));
    }

    /// octos#1380: if the server has not advertised task control (e.g. before
    /// config/capabilities/list lands, or a non-negotiating server), `x` must
    /// not send a doomed task/cancel — it reports the affordance is
    /// unavailable instead (codex P1).
    #[test]
    fn cancel_task_command_disabled_without_task_control_capability() {
        let task_id = TaskId::new();
        let mut store = store_with_task(task_id);
        // Capabilities present, but task/cancel is NOT advertised.
        store
            .state
            .set_capabilities(UiProtocolCapabilities::new(&[methods::SESSION_OPEN], &[]));

        let command = store.cancel_task_command();

        assert!(command.is_none());
        assert!(store.state.status.contains("not available"));
    }

    /// octos#1380: with no capabilities negotiated yet (capabilities == None),
    /// cancel is conservatively disabled rather than sending a doomed RPC.
    #[test]
    fn cancel_task_command_disabled_when_capabilities_unknown() {
        let task_id = TaskId::new();
        let mut store = store_with_task(task_id);
        assert!(store.state.capabilities.is_none());

        let command = store.cancel_task_command();

        assert!(command.is_none());
        assert!(store.state.status.contains("not available"));
    }

    #[test]
    fn cancel_task_command_skips_terminal_task() {
        let task_id = TaskId::new();
        let mut store = store_with_task(task_id);
        store.state.sessions[0].tasks[0].state = TaskRuntimeState::Completed;

        let command = store.cancel_task_command();

        assert!(command.is_none());
        assert!(store.state.status.contains("nothing to cancel"));
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
                topic: None,
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
    fn status_word_persona_spinner_updates_status_without_activity() {
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        // Persona spinner: kind="status_word", dynamic label (LLM-generated). It
        // must update the status line but NOT pile up as counted activity actions
        // (otherwise the agent-task chip shows "N active" with no real work).
        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id,
            Some(TurnId::new()),
            UiProgressMetadata::new(progress_kinds::STATUS_WORD).with_message("Composing"),
        )));

        assert_eq!(store.state.status, "Composing");
        assert!(
            store.state.activity.is_empty(),
            "status_word spinner must not be recorded as activity"
        );
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
                topic: None,
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
                topic: None,
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

    /// Codex-style surface (mini5 soak follow-up): a real context compaction
    /// must leave a PERSISTENT, visible activity row — not just the shared
    /// one-line `status` string that the per-turn
    /// `context/normalization_reported` (and the next user action) immediately
    /// overwrites — so the user actually sees that the context was compacted.
    #[test]
    fn context_compaction_pushes_persistent_activity_notice() {
        use octos_core::ui_protocol::{
            ContextCompactionCompletedEvent, UiContextCompactionRecord, UiContextState,
        };

        let session_id = SessionKey("local:test".into());
        // Compaction is reported DURING a turn: give the session a live reply
        // so the notice must be stamped with that turn (codex P2) — otherwise
        // a turnless notice is suppressed mid-turn and never archived.
        let turn_id = TurnId::new();
        let session = SessionView {
            id: session_id.clone(),
            title: "test".into(),
            profile_id: None,
            messages: vec![],
            tasks: vec![],
            live_reply: Some(crate::model::LiveReply {
                turn_id: turn_id.clone(),
                text: String::new(),
            }),
        };
        let mut store = Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        };

        assert!(store.state.activity.is_empty());

        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextCompactionCompleted(ContextCompactionCompletedEvent {
                session_id: session_id.clone(),
                context_state: UiContextState {
                    session_id: session_id.clone(),
                    thread_id: None,
                    generation: 4,
                    transcript_hash: "abc123".into(),
                    item_count: 42,
                    token_estimate: 9100,
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
                    token_estimate_before: 31200,
                    token_estimate_after: Some(9100),
                    error: None,
                },
            }),
        ));

        let notice = store
            .state
            .activity
            .iter()
            .find(|item| item.title.eq_ignore_ascii_case("context compacted"))
            .expect("a persistent compaction activity row must be pushed");
        // Must read as a completed notice, not a perpetual spinner.
        assert!(
            !matches!(
                notice.status.trim().to_ascii_lowercase().as_str(),
                "running" | "active" | "pending" | "queued" | "streaming" | "in_progress"
            ),
            "compaction notice must not render as a running spinner: {}",
            notice.status
        );
        // Surfaces the token reduction so the user sees what happened.
        assert!(
            notice.status.contains('→') || notice.status.to_ascii_lowercase().contains("token"),
            "status should show the token delta: {}",
            notice.status
        );
        // The detail line carries kept/dropped counts + the trigger.
        let detail = notice.detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("88") && detail.contains("token_budget"),
            "detail should show dropped count + trigger: {detail}"
        );
        assert_eq!(notice.success, Some(true));
        // codex P2: stamped with the in-flight turn so the renderer shows it
        // mid-turn and `capture_completed_turn_activity` archives it.
        assert_eq!(
            notice.turn_id.as_ref(),
            Some(&turn_id),
            "compaction notice must be stamped with the session's live turn"
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

    /// Gap 2 fix #4: `context/normalization_reported` must NOT clobber a
    /// pre-set shared status line (e.g. a "compacting…"/meaningful status).
    /// Normalization is a background lifecycle signal — it churns every turn
    /// and previously overwrote the user-visible status string.
    #[test]
    fn context_normalization_event_does_not_overwrite_status_line() {
        use octos_core::ui_protocol::{
            ContextNormalizationReportedEvent, UiContextNormalizationReport, UiContextState,
        };

        let session_id = SessionKey("local:test".into());
        let mut store = store_with_empty_session();
        store.state.status = "compacting…".into();

        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextNormalizationReported(ContextNormalizationReportedEvent {
                session_id: session_id.clone(),
                context_state: UiContextState {
                    session_id: session_id.clone(),
                    thread_id: None,
                    generation: 7,
                    transcript_hash: "h".into(),
                    item_count: 12,
                    token_estimate: 5000,
                    recovery_state: "healthy".into(),
                    last_checkpoint_id: None,
                    last_compaction_id: None,
                },
                normalization: UiContextNormalizationReport {
                    generation: 7,
                    input_transcript_hash: "h".into(),
                    output_prompt_hash: "out".into(),
                    model_capability_id: "anthropic/sonnet-4.7".into(),
                    prompt_message_count: 12,
                    token_estimate: 5000,
                    repaired_count: 0,
                    dropped_count: 0,
                    synthetic_count: 0,
                    truncated_count: 0,
                },
            }),
        ));

        // The ledger still records the normalization (surfaced in the
        // inspector / lifecycle pane), but the shared status line is intact.
        assert!(
            store
                .state
                .context_lifecycle_for(&session_id)
                .and_then(|l| l.last_normalization.as_ref())
                .is_some(),
            "normalization still stored in the lifecycle ledger"
        );
        assert_eq!(
            store.state.status, "compacting…",
            "normalization must not churn the shared status line"
        );
    }

    /// Gap 2 fix #3: a progress event carrying `metadata.retry`
    /// (`UiRetryBackoff`) must populate `AppState.session_retry` so the
    /// harness status row can render "retrying (attempt N)". The info was on
    /// the wire but previously ignored.
    #[test]
    fn progress_retry_metadata_populates_session_retry() {
        use octos_core::ui_protocol::UiRetryBackoff;

        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();

        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(2);
        retry.max_attempts = Some(5);
        retry.reason = Some("rate_limited".into());

        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id.clone(),
            Some(TurnId::new()),
            UiProgressMetadata::retry_backoff(retry),
        )));

        let stored = store
            .state
            .session_retry
            .get(&session_id)
            .expect("retry recorded for session");
        assert_eq!(stored.attempt, Some(2));
        assert_eq!(stored.max_attempts, Some(5));

        // A subsequent non-retry progress event clears the stale retry so a
        // settled turn doesn't linger as "retrying".
        store.apply_event(AppUiEvent::Progress(UiProgressEvent::new(
            session_id.clone(),
            Some(TurnId::new()),
            UiProgressMetadata::new(progress_kinds::STREAM_END).with_message("stream closed"),
        )));
        assert!(
            !store.state.session_retry.contains_key(&session_id),
            "non-retry progress clears the retry entry"
        );
    }

    /// Blocking bug 2: `session_retry` was only cleared on the next NON-retry
    /// progress event. A retry immediately followed by terminal
    /// `TurnCompleted` left stale retry that could render "retrying" on a LATER
    /// active orchestration. Terminal completion must clear it. (RED on
    /// f588b6f.)
    #[test]
    fn turn_completed_clears_stale_session_retry() {
        use octos_core::ui_protocol::UiRetryBackoff;

        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "hello");
        let session_id = store.state.sessions[0].id.clone();

        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(2);
        retry.max_attempts = Some(5);
        store.state.session_retry.insert(session_id.clone(), retry);

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        assert!(
            !store.state.session_retry.contains_key(&session_id),
            "TurnCompleted must clear the stale retry so it cannot render later"
        );
    }

    /// Blocking bug 2: terminal `TurnError` must also clear `session_retry`.
    /// (RED on f588b6f.)
    #[test]
    fn turn_error_clears_stale_session_retry() {
        use octos_core::ui_protocol::UiRetryBackoff;

        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "hello");
        let session_id = store.state.sessions[0].id.clone();

        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(1);
        store.state.session_retry.insert(session_id.clone(), retry);

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id,
                code: "provider_error".into(),
                message: "upstream 500".into(),
            },
        )));

        assert!(
            !store.state.session_retry.contains_key(&session_id),
            "TurnError must clear the stale retry so it cannot render later"
        );
    }

    /// Over-clear fix: a STALE `TurnCompleted` (for an OLD turn, not the live
    /// one) must NOT wipe the live turn's retry indicator. `commit_live_reply`
    /// preserves the live reply on a turn_id mismatch, but previously the
    /// retry clear ran unconditionally and wrongly erased the active retry.
    /// (RED on 4022a7c.)
    #[test]
    fn stale_terminal_does_not_clear_live_retry() {
        use octos_core::ui_protocol::UiRetryBackoff;

        let live_turn = TurnId::new();
        let stale_turn = TurnId::new();
        let mut store = store_with_live_reply(live_turn.clone(), "hello");
        let session_id = store.state.sessions[0].id.clone();

        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(2);
        retry.max_attempts = Some(5);
        store.state.session_retry.insert(session_id.clone(), retry);

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnCompleted(
            TurnCompletedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: stale_turn,
                cursor: None,
                tokens_in: None,
                tokens_out: None,
                session_result: None,
            },
        )));

        assert!(
            store.state.session_retry.contains_key(&session_id),
            "stale TurnCompleted (mismatched turn) must NOT clear the live turn's retry"
        );
        assert!(
            store.state.sessions[0].live_reply.is_some(),
            "stale TurnCompleted preserves the live reply"
        );
    }

    /// Over-clear fix (error path): a STALE `TurnError` (for an OLD turn) must
    /// NOT wipe the live turn's retry indicator. `fail_live_reply` keeps the
    /// live reply on a turn_id mismatch, but the retry clear previously ran
    /// regardless. (RED on 4022a7c.)
    #[test]
    fn stale_terminal_error_does_not_clear_live_retry() {
        use octos_core::ui_protocol::UiRetryBackoff;

        let live_turn = TurnId::new();
        let stale_turn = TurnId::new();
        let mut store = store_with_live_reply(live_turn.clone(), "hello");
        let session_id = store.state.sessions[0].id.clone();

        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(1);
        store.state.session_retry.insert(session_id.clone(), retry);

        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnError(
            TurnErrorEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: stale_turn,
                code: "provider_error".into(),
                message: "upstream 500".into(),
            },
        )));

        assert!(
            store.state.session_retry.contains_key(&session_id),
            "stale TurnError (mismatched turn) must NOT clear the live turn's retry"
        );
        assert!(
            store.state.sessions[0].live_reply.is_some(),
            "stale TurnError preserves the live reply"
        );
    }

    // ---------- M15-E autonomy dispatch + hydration tests ----------

    fn protocol_store_with_autonomy() -> Store {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        };
        let mut store = Store {
            state: AppState::new(
                vec![session],
                0,
                "ready".into(),
                Some("ws://example.test/ui-protocol".into()),
                false,
            ),
        };
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [
                crate::model::APPUI_METHOD_AGENT_LIST,
                crate::model::APPUI_METHOD_AGENT_STATUS_READ,
                crate::model::APPUI_METHOD_AGENT_OUTPUT_READ,
                crate::model::APPUI_METHOD_AGENT_ARTIFACT_LIST,
                crate::model::APPUI_METHOD_AGENT_ARTIFACT_READ,
                crate::model::APPUI_METHOD_TASK_ARTIFACT_READ,
                crate::model::APPUI_METHOD_THREAD_GRAPH_GET,
                crate::model::APPUI_METHOD_TURN_STATE_GET,
                crate::model::APPUI_METHOD_REVIEW_START,
                crate::model::APPUI_METHOD_AGENT_INTERRUPT,
                crate::model::APPUI_METHOD_AGENT_CLOSE,
                crate::model::APPUI_METHOD_SESSION_GOAL_GET,
                crate::model::APPUI_METHOD_SESSION_GOAL_SET,
                crate::model::APPUI_METHOD_SESSION_GOAL_CLEAR,
                crate::model::APPUI_METHOD_LOOP_CREATE,
                crate::model::APPUI_METHOD_LOOP_LIST,
                crate::model::APPUI_METHOD_LOOP_DELETE,
                crate::model::APPUI_METHOD_LOOP_PAUSE,
                crate::model::APPUI_METHOD_LOOP_RESUME,
                crate::model::APPUI_METHOD_LOOP_FIRE_NOW,
            ],
            [
                crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1,
                crate::model::APPUI_FEATURE_TASK_ARTIFACTS_V1,
                crate::model::APPUI_FEATURE_THREAD_GRAPH_V1,
                crate::model::APPUI_FEATURE_TURN_STATE_GET_V1,
                crate::model::APPUI_FEATURE_REVIEW_START_V1,
            ],
        ));
        store
    }

    #[test]
    fn agents_list_dispatches_agent_list_rpc() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents".into();
        let command = store.compose_command().expect("dispatch returns command");
        match command {
            AppUiCommand::ListAgents(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
            }
            other => panic!("expected ListAgents, got {other:?}"),
        }
    }

    #[test]
    fn agents_list_subcommand_also_dispatches() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents list".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ListAgents(_))
        ));
    }

    #[test]
    fn agents_status_without_id_falls_back_to_list() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents status".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ListAgents(_))
        ));
    }

    #[test]
    fn agents_status_with_id_dispatches_status_read() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents status reviewer-1".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ReadAgentStatus(params) => {
                assert_eq!(params.agent_id, "reviewer-1");
            }
            other => panic!("expected ReadAgentStatus, got {other:?}"),
        }
    }

    #[test]
    fn agents_output_dispatches_output_read() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents output ag-7".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ReadAgentOutput(params) => {
                assert_eq!(params.agent_id, "ag-7");
                assert!(params.cursor.is_none());
            }
            other => panic!("expected ReadAgentOutput, got {other:?}"),
        }
    }

    #[test]
    fn agents_artifacts_dispatches_artifact_list() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents artifacts ag-7".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ListAgentArtifacts(params) => {
                assert_eq!(params.agent_id, "ag-7");
            }
            other => panic!("expected ListAgentArtifacts, got {other:?}"),
        }
    }

    #[test]
    fn agents_artifact_dispatches_artifact_read() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents artifact ag-7 artifact-1".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ReadAgentArtifact(params) => {
                assert_eq!(params.agent_id, "ag-7");
                assert_eq!(params.artifact_id.as_deref(), Some("artifact-1"));
                assert!(params.path.is_none());
            }
            other => panic!("expected ReadAgentArtifact, got {other:?}"),
        }
    }

    #[test]
    fn agents_artifact_dispatches_path_selector() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents artifact ag-7 path:reports/out.md".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ReadAgentArtifact(params) => {
                assert_eq!(params.agent_id, "ag-7");
                assert!(params.artifact_id.is_none());
                assert_eq!(params.path.as_deref(), Some("reports/out.md"));
            }
            other => panic!("expected ReadAgentArtifact, got {other:?}"),
        }
    }

    #[test]
    fn task_artifact_dispatches_task_artifact_read() {
        let mut store = protocol_store_with_autonomy();
        let task_id = "00000000-0000-0000-0000-000000000007";
        store.state.composer = format!("/task artifact {task_id} summary");
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ReadTaskArtifact(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
                assert_eq!(params.task_id.to_string(), task_id);
                assert_eq!(params.artifact_id.as_deref(), Some("summary"));
                assert!(params.path.is_none());
                assert_eq!(params.profile_id.as_deref(), Some("coding"));
                assert_eq!(params.limit_bytes, Some(TASK_ARTIFACT_READ_LIMIT_BYTES));
            }
            other => panic!("expected ReadTaskArtifact, got {other:?}"),
        }
    }

    #[test]
    fn task_artifact_dispatches_path_selector() {
        let mut store = protocol_store_with_autonomy();
        let task_id = "00000000-0000-0000-0000-000000000007";
        store.state.composer = format!("/task read-artifact {task_id} path:reports/out.md");
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ReadTaskArtifact(params) => {
                assert_eq!(params.task_id.to_string(), task_id);
                assert!(params.artifact_id.is_none());
                assert_eq!(params.path.as_deref(), Some("reports/out.md"));
            }
            other => panic!("expected ReadTaskArtifact, got {other:?}"),
        }
    }

    #[test]
    fn task_artifact_requires_task_artifact_feature() {
        let mut store = protocol_store_with_autonomy();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_TASK_ARTIFACT_READ,
        ]));
        store.state.composer = "/task artifact 00000000-0000-0000-0000-000000000007 summary".into();

        assert!(store.compose_command().is_none());
        assert!(
            store
                .state
                .status
                .contains(crate::model::APPUI_FEATURE_TASK_ARTIFACTS_V1)
        );
    }

    #[test]
    fn thread_graph_dispatches_thread_graph_get() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/threads".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetThreadGraph(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
                assert!(params.at.is_none());
            }
            other => panic!("expected GetThreadGraph, got {other:?}"),
        }
    }

    #[test]
    fn thread_graph_requires_thread_graph_feature() {
        let mut store = protocol_store_with_autonomy();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_THREAD_GRAPH_GET,
        ]));
        store.state.composer = "/thread graph".into();

        assert!(store.compose_command().is_none());
        assert!(
            store
                .state
                .status
                .contains(crate::model::APPUI_FEATURE_THREAD_GRAPH_V1)
        );
    }

    #[test]
    fn turn_state_dispatches_active_turn_state_get() {
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "working");
        store.state.target = Some("ws://example.test/ui-protocol".into());
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [crate::model::APPUI_METHOD_TURN_STATE_GET],
            [crate::model::APPUI_FEATURE_TURN_STATE_GET_V1],
        ));
        store.state.composer = "/turn state".into();

        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetTurnState(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
                assert_eq!(params.turn_id, turn_id);
            }
            other => panic!("expected GetTurnState, got {other:?}"),
        }
    }

    #[test]
    fn turn_state_dispatches_explicit_turn_id() {
        let mut store = protocol_store_with_autonomy();
        let turn_id = "00000000-0000-0000-0000-000000000011";
        store.state.composer = format!("/turn state {turn_id}");

        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetTurnState(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
                assert_eq!(params.turn_id.0.to_string(), turn_id);
            }
            other => panic!("expected GetTurnState, got {other:?}"),
        }
    }

    #[test]
    fn turn_state_requires_turn_state_feature() {
        let mut store = protocol_store_with_autonomy();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_TURN_STATE_GET,
        ]));
        store.state.composer = "/turn state 00000000-0000-0000-0000-000000000011".into();

        assert!(store.compose_command().is_none());
        assert!(
            store
                .state
                .status
                .contains(crate::model::APPUI_FEATURE_TURN_STATE_GET_V1)
        );
    }

    #[test]
    fn review_start_dispatches_review_rpc() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/review Check regressions and missing tests".into();

        match store.compose_command().expect("dispatch") {
            AppUiCommand::StartReview(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
                assert_eq!(params.profile_id.as_deref(), Some("coding"));
                assert!(params.turn_id.is_some());
                assert!(params.target.is_none());
                assert_eq!(
                    params.prompt.as_deref(),
                    Some("Check regressions and missing tests")
                );
                assert_eq!(params.delivery.as_deref(), Some("inline"));
            }
            other => panic!("expected StartReview, got {other:?}"),
        }
        assert_eq!(store.state.status, "Starting backend code review");
        assert_eq!(store.state.run_state.label(), "running");
    }

    #[test]
    fn review_start_requires_review_feature() {
        let mut store = protocol_store_with_autonomy();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_REVIEW_START,
        ]));
        store.state.composer = "/review".into();

        assert!(store.compose_command().is_none());
        assert!(
            store
                .state
                .status
                .contains(crate::model::APPUI_FEATURE_REVIEW_START_V1)
        );
    }

    #[test]
    fn review_start_result_updates_status_and_activity() {
        use crate::client_event::ClientEvent;

        let mut store = protocol_store_with_autonomy();
        let turn_id = TurnId::new();
        store.apply_client_event(ClientEvent::ReviewStart(ReviewStartResult {
            accepted: true,
            session_id: SessionKey("local:test".into()),
            turn_id: turn_id.clone(),
            workflow: Some("code_review".into()),
            backend: Some("native".into()),
            agent_count: Some(3),
        }));

        assert_eq!(
            store.state.status,
            "Review started: 3 specialist(s) via native"
        );
        assert_eq!(store.state.run_state.label(), "running");
        let activity = store.state.activity.last().expect("review activity");
        assert_eq!(activity.title, "code review");
        assert_eq!(activity.turn_id.as_ref(), Some(&turn_id));
        assert!(
            activity
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("workflow=code_review"))
        );
    }

    #[test]
    fn session_hydrate_command_requires_feature_and_method() {
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_SESSION_HYDRATE,
        ]));
        assert!(store.hydrate_session_state_command(&session_id).is_none());

        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [crate::model::APPUI_METHOD_SESSION_HYDRATE],
            [crate::model::APPUI_FEATURE_SESSION_HYDRATE_V1],
        ));
        match store
            .hydrate_session_state_command(&session_id)
            .expect("hydrate command")
        {
            AppUiCommand::HydrateSession(params) => {
                assert_eq!(params.session_id, session_id);
                assert!(params.after.is_none());
                assert_eq!(
                    params.include,
                    vec![
                        octos_core::ui_protocol::hydrate_sections::MESSAGES.to_string(),
                        octos_core::ui_protocol::hydrate_sections::THREADS.to_string(),
                        octos_core::ui_protocol::hydrate_sections::TURNS.to_string(),
                        octos_core::ui_protocol::hydrate_sections::PENDING_APPROVALS.to_string(),
                    ]
                );
            }
            other => panic!("expected HydrateSession, got {other:?}"),
        }
    }

    #[test]
    fn agents_interrupt_dispatches_agent_interrupt() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents interrupt ag-7".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::InterruptAgent(params) => {
                assert_eq!(params.agent_id, "ag-7");
            }
            other => panic!("expected InterruptAgent, got {other:?}"),
        }
    }

    #[test]
    fn agents_close_dispatches_agent_close() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents close ag-7".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::CloseAgent(params) => {
                assert_eq!(params.agent_id, "ag-7");
            }
            other => panic!("expected CloseAgent, got {other:?}"),
        }
    }

    #[test]
    fn agents_missing_id_records_parse_error_status() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/agents output".into();
        assert!(store.compose_command().is_none());
        // Registry hides the command on the missing-id error first; the
        // composer is cleared either way.
        assert!(store.state.composer.is_empty());
    }

    #[test]
    fn goal_bare_dispatches_goal_get() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/goal".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetSessionGoal(params) => {
                assert_eq!(params.profile_id.as_deref(), Some("coding"));
            }
            other => panic!("expected GetSessionGoal, got {other:?}"),
        }
    }

    #[test]
    fn goal_set_dispatches_goal_set_with_objective() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/goal finish the review by Friday".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::SetSessionGoal(params) => {
                assert_eq!(params.action, crate::model::SessionGoalSetAction::Set);
                assert_eq!(params.objective, "finish the review by Friday");
                assert_eq!(params.status.as_deref(), Some("active"));
                assert_eq!(params.transition_actor.as_deref(), Some("user"));
            }
            other => panic!("expected SetSessionGoal, got {other:?}"),
        }
    }

    #[test]
    fn goal_pause_refreshes_via_goal_get_first() {
        // Audit follow-up: pause/resume must NOT forward a cached
        // objective straight to the backend — the cached mirror can
        // drift. The dispatch issues `session/goal/get` first; the
        // staged transition fires from the response handler.
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.state.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: "ongoing work".into(),
                status: "active".into(),
                token_budget: 1000,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        store.state.composer = "/goal pause".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetSessionGoal(params) => {
                assert_eq!(params.session_id, session_id);
                assert_eq!(params.profile_id.as_deref(), Some("coding"));
            }
            other => panic!("expected GetSessionGoal, got {other:?}"),
        }
        let pending = store
            .state
            .pending_goal_transition
            .as_ref()
            .expect("pause stages a transition");
        assert_eq!(pending.session_id, session_id);
        assert_eq!(pending.status, "paused");
        assert_eq!(pending.action, crate::model::SessionGoalSetAction::Pause);
    }

    #[test]
    fn goal_pause_without_cached_goal_records_status_and_returns_none() {
        let mut store = protocol_store_with_autonomy();
        // No goal cached.
        store.state.composer = "/goal pause".into();
        assert!(store.compose_command().is_none());
        assert!(
            store.state.status.to_lowercase().contains("no goal cached"),
            "expected guidance, got: {}",
            store.state.status
        );
    }

    #[test]
    fn goal_pause_rejects_completed_goal_without_dispatch() {
        // The model owns the `complete` transition; the TUI must NEVER
        // pause or resume a completed goal back into an active state.
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.state.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "g1".into(),
                objective: "done work".into(),
                status: "complete".into(),
                token_budget: 1,
                tokens_used: 1,
                time_used_seconds: 1,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("model".into()),
        );
        store.state.composer = "/goal pause".into();
        assert!(store.compose_command().is_none());
        assert!(
            store.state.status.to_lowercase().contains("complete"),
            "expected complete-state message, got: {}",
            store.state.status
        );
    }

    #[test]
    fn goal_resume_rejects_completed_goal_without_dispatch() {
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.state.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "g1".into(),
                objective: "done work".into(),
                status: "complete".into(),
                token_budget: 1,
                tokens_used: 1,
                time_used_seconds: 1,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("model".into()),
        );
        store.state.composer = "/goal resume".into();
        assert!(store.compose_command().is_none());
        assert!(
            store.state.status.to_lowercase().contains("complete"),
            "expected complete-state message, got: {}",
            store.state.status
        );
    }

    #[test]
    fn goal_resume_refreshes_via_goal_get_first() {
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.state.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: "ongoing work".into(),
                status: "paused".into(),
                token_budget: 1000,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        store.state.composer = "/goal resume".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetSessionGoal(params) => {
                assert_eq!(params.session_id, session_id);
            }
            other => panic!("expected GetSessionGoal, got {other:?}"),
        }
        let pending = store
            .state
            .pending_goal_transition
            .as_ref()
            .expect("resume stages a transition");
        assert_eq!(pending.status, "active");
        assert_eq!(pending.action, crate::model::SessionGoalSetAction::Resume);
    }

    #[test]
    fn goal_pause_carries_server_objective_not_stale_cache() {
        // Audit follow-up: when the cached mirror has drifted (local
        // says "X", server says "Y"), the pause/resume transition must
        // forward server truth — not the stale cache — to the backend.
        // Otherwise the set call would silently overwrite the server's
        // current objective with the TUI's outdated copy.
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());

        // Cache holds the stale "X".
        let stale_goal = octos_core::ui_protocol::UiGoalRecord {
            profile_id: Some("coding".into()),
            goal_id: "goal_01".into(),
            objective: "X (stale cache)".into(),
            status: "active".into(),
            token_budget: 1000,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store
            .state
            .set_session_goal(&session_id, Some(stale_goal), Some("user".into()));

        // User issues /goal pause — dispatch returns `session/goal/get`
        // (not `session/goal/set`), and stages the transition.
        store.state.composer = "/goal pause".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::GetSessionGoal(_) => {}
            other => panic!("pause should refresh first, got {other:?}"),
        }

        // Server responds with the FRESH objective "Y" (the cache has
        // drifted — what the server actually has on file).
        let fresh_goal = octos_core::ui_protocol::UiGoalRecord {
            profile_id: Some("coding".into()),
            goal_id: "goal_01".into(),
            objective: "Y (server truth)".into(),
            status: "active".into(),
            token_budget: 1000,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at_ms: 1,
            updated_at_ms: 7,
        };
        let follow_up = store
            .apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
                result: AutonomyResult::GoalGet(crate::model::SessionGoalGetResult {
                    session_id: session_id.clone(),
                    profile_id: Some("coding".into()),
                    goal: Some(fresh_goal),
                }),
            }))
            .expect("goal_get response triggers the staged transition");

        // The follow-up must be `session/goal/set` carrying SERVER's
        // "Y", NOT the cached "X".
        match follow_up {
            AppUiCommand::SetSessionGoal(params) => {
                assert_eq!(
                    params.objective, "Y (server truth)",
                    "pause must forward refreshed server objective, not stale cache"
                );
                assert_eq!(params.status.as_deref(), Some("paused"));
                assert_eq!(params.action, crate::model::SessionGoalSetAction::Pause);
            }
            other => panic!("expected SetSessionGoal follow-up, got {other:?}"),
        }
        // Pending transition is consumed.
        assert!(store.state.pending_goal_transition.is_none());
    }

    #[test]
    fn goal_pause_aborts_when_refresh_returns_complete_status() {
        // If the server reports the goal is now `complete` between
        // dispatch and refresh, the staged pause must abort — the TUI
        // never reactivates a model-completed goal.
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());

        store.state.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "g".into(),
                objective: "ongoing".into(),
                status: "active".into(),
                token_budget: 1000,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        store.state.composer = "/goal pause".into();
        let _ = store.compose_command().expect("dispatch get");

        // Server's refresh shows the model marked the goal complete
        // while the user was typing.
        let follow_up = store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::GoalGet(crate::model::SessionGoalGetResult {
                session_id: session_id.clone(),
                profile_id: Some("coding".into()),
                goal: Some(octos_core::ui_protocol::UiGoalRecord {
                    profile_id: Some("coding".into()),
                    goal_id: "g".into(),
                    objective: "ongoing".into(),
                    status: "complete".into(),
                    token_budget: 1000,
                    tokens_used: 1000,
                    time_used_seconds: 1,
                    created_at_ms: 1,
                    updated_at_ms: 9,
                }),
            }),
        }));
        assert!(
            follow_up.is_none(),
            "must not fire pause against a complete goal"
        );
        assert!(store.state.pending_goal_transition.is_none());
        assert!(
            store.state.status.to_lowercase().contains("complete"),
            "expected complete-state diagnostic, got: {}",
            store.state.status
        );
    }

    #[test]
    fn goal_pause_aborts_when_refresh_reports_no_goal() {
        // If the goal vanished between dispatch and refresh (cleared,
        // expired, etc.), there is nothing to pause — the staged
        // transition must be dropped silently.
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());

        store.state.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "g".into(),
                objective: "ongoing".into(),
                status: "active".into(),
                token_budget: 1000,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        store.state.composer = "/goal pause".into();
        let _ = store.compose_command().expect("dispatch get");

        let follow_up = store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::GoalGet(crate::model::SessionGoalGetResult {
                session_id: session_id.clone(),
                profile_id: Some("coding".into()),
                goal: None,
            }),
        }));
        assert!(follow_up.is_none());
        assert!(store.state.pending_goal_transition.is_none());
    }

    #[test]
    fn goal_clear_drops_any_pending_goal_transition() {
        // A staged pause/resume against this session no longer makes
        // sense once the goal is cleared.
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.state.pending_goal_transition = Some(crate::model::PendingGoalTransition {
            session_id: session_id.clone(),
            profile_id: Some("coding".into()),
            status: "paused",
            action: crate::model::SessionGoalSetAction::Pause,
        });
        let _ = store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::GoalClear(crate::model::SessionGoalClearResult {
                session_id,
                cleared: true,
                transition_actor: Some("user".into()),
            }),
        }));
        assert!(store.state.pending_goal_transition.is_none());
    }

    #[test]
    fn goal_clear_dispatches_goal_clear() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/goal clear".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::ClearSessionGoal(params) => {
                assert_eq!(params.session_id, SessionKey("local:test".into()));
            }
            other => panic!("expected ClearSessionGoal, got {other:?}"),
        }
    }

    #[test]
    fn loop_bare_dispatches_create_maintenance() {
        // Per UPCR-2026-021 §"Parsing rules" line 298: bare `/loop`
        // creates a maintenance loop with an empty prompt — the
        // backend resolves the prompt from `.octos/loop.md`, then
        // `~/.octos/loop.md`, then a built-in fallback. The TUI must
        // dispatch `loop/create` (not `loop/list`).
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::CreateLoop(params) => {
                assert_eq!(params.prompt, "");
                assert_eq!(params.mode, crate::model::LoopMode::Maintenance);
                assert!(params.interval_seconds.is_none());
            }
            other => panic!("expected CreateLoop maintenance, got {other:?}"),
        }
    }

    #[test]
    fn loop_list_lists_loops() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop list".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ListLoops(_))
        ));
    }

    #[test]
    fn loop_with_self_paced_prompt_dispatches_create_self_paced() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop check the deploy".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::CreateLoop(params) => {
                assert_eq!(params.prompt, "check the deploy");
                assert_eq!(params.mode, crate::model::LoopMode::SelfPaced);
                assert!(params.interval_seconds.is_none());
            }
            other => panic!("expected CreateLoop self-paced, got {other:?}"),
        }
    }

    #[test]
    fn loop_with_leading_interval_dispatches_fixed_interval() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop 5m run tests".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::CreateLoop(params) => {
                assert_eq!(params.prompt, "run tests");
                assert_eq!(params.mode, crate::model::LoopMode::FixedInterval);
                assert_eq!(params.interval_seconds, Some(300));
            }
            other => panic!("expected CreateLoop fixed, got {other:?}"),
        }
    }

    #[test]
    fn loop_with_suffix_every_dispatches_fixed_interval() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop check queue every 2h".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::CreateLoop(params) => {
                assert_eq!(params.prompt, "check queue");
                assert_eq!(params.mode, crate::model::LoopMode::FixedInterval);
                assert_eq!(params.interval_seconds, Some(7200));
            }
            other => panic!("expected CreateLoop suffix fixed, got {other:?}"),
        }
    }

    #[test]
    fn loop_with_maintenance_cadence_dispatches_maintenance() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop maintenance prune old artifacts".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::CreateLoop(params) => {
                assert_eq!(params.prompt, "prune old artifacts");
                assert_eq!(params.mode, crate::model::LoopMode::Maintenance);
                assert!(params.interval_seconds.is_none());
            }
            other => panic!("expected CreateLoop maintenance, got {other:?}"),
        }
    }

    #[test]
    fn loop_delete_dispatches_delete_with_id() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop delete loop-7".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::DeleteLoop(params) => {
                assert_eq!(params.loop_id, "loop-7");
            }
            other => panic!("expected DeleteLoop, got {other:?}"),
        }
    }

    #[test]
    fn loop_pause_dispatches_pause_with_id() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop pause loop-7".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::PauseLoop(_))
        ));
    }

    #[test]
    fn loop_resume_dispatches_resume_with_id() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop resume loop-7".into();
        assert!(matches!(
            store.compose_command(),
            Some(AppUiCommand::ResumeLoop(_))
        ));
    }

    #[test]
    fn loop_fire_now_dispatches_fire_with_id() {
        let mut store = protocol_store_with_autonomy();
        store.state.composer = "/loop fire-now loop-7".into();
        match store.compose_command().expect("dispatch") {
            AppUiCommand::FireLoopNow(params) => {
                assert_eq!(params.loop_id, "loop-7");
            }
            other => panic!("expected FireLoopNow, got {other:?}"),
        }
    }

    #[test]
    fn autonomy_dispatch_without_session_records_status_and_returns_none() {
        // Empty session list — the dispatcher must NOT emit an RPC.
        let mut store = Store {
            state: AppState::new(vec![], 0, "ready".into(), None, false),
        };
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [crate::model::APPUI_METHOD_AGENT_LIST],
            [crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1],
        ));
        store.state.composer = "/agents list".into();
        assert!(store.compose_command().is_none());
    }

    #[test]
    fn autonomy_commands_hidden_without_capability() {
        // Capability set has methods but NOT the feature; the registry
        // gate must hide `/agents`, `/goal`, `/loop`.
        let mut store = store_with_empty_session();
        store.state.target = Some("ws://example.test/ui-protocol".into());
        // Methods are present, but `coding.autonomy.v1` is NOT.
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_AGENT_LIST,
            crate::model::APPUI_METHOD_SESSION_GOAL_GET,
            crate::model::APPUI_METHOD_LOOP_LIST,
        ]));

        for cmd in ["/agents", "/goal", "/loop"] {
            store.state.composer = cmd.into();
            assert!(
                store.compose_command().is_none(),
                "{cmd} must be hidden without coding.autonomy.v1"
            );
        }
    }

    #[test]
    fn autonomy_interrupt_blocked_in_readonly_mode() {
        let mut store = protocol_store_with_autonomy();
        store.state.readonly = true;
        store.state.composer = "/agents interrupt ag-7".into();
        assert!(store.compose_command().is_none());
        assert!(
            store.state.status.to_lowercase().contains("read-only")
                || store.state.status.contains("disabled"),
            "expected readonly status, got: {}",
            store.state.status
        );
    }

    #[test]
    fn autonomy_hydration_enqueues_three_commands_on_supported_caps() {
        let store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let commands = store.hydrate_autonomy_state_commands(&session_id);
        assert_eq!(commands.len(), 3);
        assert!(matches!(commands[0], AppUiCommand::ListAgents(_)));
        assert!(matches!(commands[1], AppUiCommand::GetSessionGoal(_)));
        assert!(matches!(commands[2], AppUiCommand::ListLoops(_)));
    }

    #[test]
    fn autonomy_hydration_skipped_without_autonomy_feature() {
        let mut store = protocol_store_with_autonomy();
        // Strip the feature.
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            crate::model::APPUI_METHOD_AGENT_LIST,
            crate::model::APPUI_METHOD_SESSION_GOAL_GET,
            crate::model::APPUI_METHOD_LOOP_LIST,
        ]));
        let commands = store.hydrate_autonomy_state_commands(&SessionKey("local:test".into()));
        assert!(commands.is_empty());
    }

    #[test]
    fn autonomy_hydration_only_enqueues_supported_subset() {
        let mut store = protocol_store_with_autonomy();
        // Only `agent/list` advertised (plus the feature).
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [crate::model::APPUI_METHOD_AGENT_LIST],
            [crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1],
        ));
        let commands = store.hydrate_autonomy_state_commands(&SessionKey("local:test".into()));
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], AppUiCommand::ListAgents(_)));
    }

    #[test]
    fn agent_updated_notification_upserts_session_mirror() {
        use octos_core::ui_protocol::{AgentUpdatedEvent, UiAgentRecord};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let agent = UiAgentRecord {
            agent_id: "ag-1".into(),
            parent_agent_id: None,
            session_id: session_id.clone(),
            task_id: None,
            path: "/root".into(),
            role: "reviewer".into(),
            nickname: "rev".into(),
            title: None,
            backend_kind: "native".into(),
            status: "running".into(),
            last_task: None,
            summary: None,
            output_tail: None,
            cwd: None,
            profile_id: "coding".into(),
            runtime_policy_stamp: None,
            artifact_count: 0,
            artifacts: vec![],
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.apply_event(AppUiEvent::Protocol(UiNotification::AgentUpdated(
            AgentUpdatedEvent {
                session_id: session_id.clone(),
                agent: agent.clone(),
            },
        )));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror created");
        assert_eq!(mirror.agents.len(), 1);
        assert_eq!(mirror.agents[0].agent_id, "ag-1");
        assert_eq!(mirror.agents[0].status, "running");
    }

    /// Stuck-chip safety net: a spawn_only background task that outlives its
    /// spawning turn only goes terminal AFTER the per-turn task-progress
    /// channel was torn down, so the terminal `task/updated` never reaches the
    /// client and `session.tasks` stays "running" — pinning the chip on
    /// "Orchestrating…". The DURABLE terminal `agent/updated` (which now does
    /// reach the client via the ledger) carries `task_id` + a terminal
    /// `status`; the store reconciles the matching task to its terminal state
    /// so the chip flips. This pins that reconcile.
    #[test]
    fn terminal_agent_update_reconciles_stuck_running_task() {
        use octos_core::ui_protocol::{AgentUpdatedEvent, TaskRuntimeState, UiAgentRecord};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let task_id: TaskId = "01900000-0000-7000-8000-0000000000fc"
            .parse()
            .expect("valid task id");

        // Seed a running spawn_only task — the chip currently reads this as
        // "running" and stays on "Orchestrating…".
        if let Some(session) = store.find_session_mut(&session_id) {
            session.tasks.push(TaskView {
                id: task_id.clone(),
                title: "octos-code-review-retry".into(),
                state: TaskRuntimeState::Running,
                runtime_detail: None,
                output_tail: String::new(),
                turn_id: None,
            });
        }

        let agent = UiAgentRecord {
            agent_id: "task-01900000-0000-7000-8000-0000000000fc".into(),
            parent_agent_id: Some("master".into()),
            session_id: session_id.clone(),
            task_id: Some(task_id.to_string()),
            path: "master/task-fc".into(),
            role: "background_task".into(),
            nickname: "spawn".into(),
            title: None,
            backend_kind: "task_supervisor:spawn".into(),
            status: "failed".into(),
            last_task: Some("spawn_only failure".into()),
            summary: None,
            output_tail: None,
            cwd: None,
            profile_id: "coding".into(),
            runtime_policy_stamp: None,
            artifact_count: 0,
            artifacts: vec![],
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.apply_event(AppUiEvent::Protocol(UiNotification::AgentUpdated(
            AgentUpdatedEvent {
                session_id: session_id.clone(),
                agent,
            },
        )));

        let session = store
            .state
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .expect("session present");
        let task = session
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .expect("seeded task present");
        assert_eq!(
            task.state,
            TaskRuntimeState::Failed,
            "the terminal agent/updated must reconcile the stuck running task to its terminal state so the chip flips off Orchestrating…",
        );
    }

    #[test]
    fn agent_output_delta_appends_to_session_mirror() {
        use octos_core::ui_protocol::AgentOutputDeltaEvent;
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.apply_event(AppUiEvent::Protocol(UiNotification::AgentOutputDelta(
            AgentOutputDeltaEvent {
                session_id: session_id.clone(),
                agent_id: "ag-1".into(),
                cursor: OutputCursor { offset: 5 },
                text: "hello".into(),
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::AgentOutputDelta(
            AgentOutputDeltaEvent {
                session_id: session_id.clone(),
                agent_id: "ag-1".into(),
                cursor: OutputCursor { offset: 11 },
                text: " world".into(),
            },
        )));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        assert_eq!(mirror.agent_outputs.len(), 1);
        assert_eq!(mirror.agent_outputs[0].text, "hello world");
        assert_eq!(mirror.agent_outputs[0].cursor.offset, 11);
    }

    #[test]
    fn session_goal_updated_notification_replaces_mirror_goal() {
        use octos_core::ui_protocol::{SessionGoalUpdatedEvent, UiGoalRecord};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let goal = UiGoalRecord {
            profile_id: Some("coding".into()),
            goal_id: "goal_01".into(),
            objective: "finish review".into(),
            status: "active".into(),
            token_budget: 50000,
            tokens_used: 100,
            time_used_seconds: 10,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionGoalUpdated(
            SessionGoalUpdatedEvent {
                session_id: session_id.clone(),
                profile_id: Some("coding".into()),
                goal: goal.clone(),
                transition_actor: "user".into(),
            },
        )));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        let stored_goal = mirror.goal.as_ref().expect("goal cached");
        assert_eq!(stored_goal.objective, "finish review");
        assert_eq!(stored_goal.status, "active");
        assert_eq!(mirror.goal_transition_actor.as_deref(), Some("user"));
    }

    #[test]
    fn session_goal_cleared_notification_clears_mirror_goal() {
        use octos_core::ui_protocol::{
            SessionGoalClearedEvent, SessionGoalUpdatedEvent, UiGoalRecord,
        };
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        // Seed.
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionGoalUpdated(
            SessionGoalUpdatedEvent {
                session_id: session_id.clone(),
                profile_id: None,
                goal: UiGoalRecord {
                    profile_id: None,
                    goal_id: "g1".into(),
                    objective: "foo".into(),
                    status: "active".into(),
                    token_budget: 1000,
                    tokens_used: 0,
                    time_used_seconds: 0,
                    created_at_ms: 1,
                    updated_at_ms: 2,
                },
                transition_actor: "user".into(),
            },
        )));
        // Now clear with cleared=true.
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionGoalCleared(
            SessionGoalClearedEvent {
                session_id: session_id.clone(),
                profile_id: None,
                cleared: true,
                goal: None,
                transition_actor: "user".into(),
            },
        )));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        assert!(mirror.goal.is_none());
    }

    #[test]
    fn loop_updated_notification_upserts_mirror_loop() {
        use octos_core::ui_protocol::{LoopUpdatedEvent, UiLoopRecord};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let loop_record = UiLoopRecord {
            loop_id: "loop_01".into(),
            session_id: session_id.clone(),
            profile_id: None,
            prompt: "check deploy".into(),
            mode: "self_paced".into(),
            interval_seconds: None,
            status: "active".into(),
            next_run_at_ms: None,
            last_run_at_ms: None,
            expires_at_ms: 999,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.apply_event(AppUiEvent::Protocol(UiNotification::LoopUpdated(
            LoopUpdatedEvent {
                session_id: session_id.clone(),
                profile_id: None,
                loop_id: Some("loop_01".into()),
                loop_state: loop_record.clone(),
                ok: Some(true),
                status: Some("active".into()),
                deleted: None,
            },
        )));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        assert_eq!(mirror.loops.len(), 1);
        assert_eq!(mirror.loops[0].loop_id, "loop_01");
    }

    #[test]
    fn loop_updated_with_deleted_flag_removes_mirror_loop() {
        use octos_core::ui_protocol::{LoopUpdatedEvent, UiLoopRecord};
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        // Seed first.
        let loop_record = UiLoopRecord {
            loop_id: "loop_01".into(),
            session_id: session_id.clone(),
            profile_id: None,
            prompt: "check".into(),
            mode: "self_paced".into(),
            interval_seconds: None,
            status: "active".into(),
            next_run_at_ms: None,
            last_run_at_ms: None,
            expires_at_ms: 999,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store
            .state
            .upsert_session_loop(&session_id, loop_record.clone());
        // Now notify deleted=true.
        store.apply_event(AppUiEvent::Protocol(UiNotification::LoopUpdated(
            LoopUpdatedEvent {
                session_id: session_id.clone(),
                profile_id: None,
                loop_id: Some("loop_01".into()),
                loop_state: loop_record,
                ok: Some(true),
                status: Some("deleted".into()),
                deleted: Some(true),
            },
        )));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        assert!(mirror.loops.is_empty());
    }

    #[test]
    fn autonomy_agent_list_result_replaces_mirror_agents() {
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        use octos_core::ui_protocol::UiAgentRecord;
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let agent = UiAgentRecord {
            agent_id: "ag-1".into(),
            parent_agent_id: None,
            session_id: session_id.clone(),
            task_id: None,
            path: "/root".into(),
            role: "reviewer".into(),
            nickname: "rev".into(),
            title: None,
            backend_kind: "native".into(),
            status: "running".into(),
            last_task: None,
            summary: None,
            output_tail: None,
            cwd: None,
            profile_id: "coding".into(),
            runtime_policy_stamp: None,
            artifact_count: 0,
            artifacts: vec![],
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::AgentList(crate::model::AgentListResult {
                session_id: session_id.clone(),
                agents: vec![agent],
            }),
        }));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        assert_eq!(mirror.agents.len(), 1);
        assert!(store.state.status.contains("1 agent"));
    }

    #[test]
    fn autonomy_agent_artifact_read_opens_detail_modal() {
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        use octos_core::ui_protocol::UiAgentArtifact;
        use std::collections::BTreeMap;

        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::AgentArtifactRead(crate::model::AgentArtifactReadResult {
                session_id,
                agent_id: "ag-7".into(),
                artifact: UiAgentArtifact {
                    id: "artifact-1".into(),
                    title: "notes.md".into(),
                    kind: "markdown".into(),
                    status: "ready".into(),
                    path: Some("notes.md".into()),
                    content: None,
                    extra: BTreeMap::new(),
                },
                content: Some("artifact body".into()),
            }),
        }));

        assert!(store.state.artifact_detail.active);
        assert_eq!(store.state.artifact_detail.title, "notes.md");
        assert!(store.state.artifact_detail.subtitle.contains("ag-7"));
        assert_eq!(store.state.artifact_detail.content, "artifact body");
        assert_eq!(store.state.status, "Agent ag-7 artifact loaded: notes.md");
    }

    #[test]
    fn autonomy_task_artifact_read_opens_detail_modal() {
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        use octos_core::ui_protocol::{TaskArtifactReadResult, TaskArtifactRecord};
        use std::collections::BTreeMap;

        let mut store = protocol_store_with_autonomy();
        let task_id: TaskId = "00000000-0000-0000-0000-000000000007"
            .parse()
            .expect("task id");
        store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::TaskArtifactRead(TaskArtifactReadResult {
                session_id: SessionKey("local:test".into()),
                task_id: task_id.clone(),
                agent_id: None,
                artifact: TaskArtifactRecord {
                    id: "summary".into(),
                    title: "Summary".into(),
                    kind: "markdown".into(),
                    status: "ready".into(),
                    path: None,
                    content: None,
                    extra: BTreeMap::new(),
                },
                content: Some("task artifact body".into()),
                cursor: None,
                next_cursor: None,
                has_more: false,
            }),
        }));

        assert!(store.state.artifact_detail.active);
        assert_eq!(store.state.artifact_detail.title, "Summary");
        assert!(
            store
                .state
                .artifact_detail
                .subtitle
                .contains(&task_id.to_string())
        );
        assert_eq!(store.state.artifact_detail.content, "task artifact body");
        assert_eq!(
            store.state.status,
            format!("Task {task_id} artifact loaded: Summary")
        );
    }

    #[test]
    fn autonomy_thread_graph_opens_detail_modal() {
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        use octos_core::ui_protocol::{ThreadGraphEntry, ThreadGraphGetResult, UiCursor};

        let mut store = protocol_store_with_autonomy();
        store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::ThreadGraph(ThreadGraphGetResult {
                session_id: SessionKey("local:test".into()),
                cursor: UiCursor {
                    stream: "session".into(),
                    seq: 7,
                },
                threads: vec![ThreadGraphEntry {
                    thread_id: "thread-1".into(),
                    root_seq: 1,
                    root_client_message_id: None,
                    turn_id: None,
                    message_seqs: vec![1, 2],
                    status: "active".into(),
                }],
                orphans: vec![99],
            }),
        }));

        assert!(store.state.thread_graph_detail.active);
        assert_eq!(store.state.thread_graph_detail.title, "Thread Graph");
        assert!(
            store
                .state
                .thread_graph_detail
                .subtitle
                .contains("1 thread")
        );
        assert!(store.state.thread_graph_detail.content.contains("thread-1"));
        assert!(
            store
                .state
                .thread_graph_detail
                .content
                .contains("Orphans: 99")
        );
        assert_eq!(store.state.status, "Thread graph loaded: 1 thread(s)");
    }

    #[test]
    fn autonomy_turn_state_opens_detail_modal() {
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        use octos_core::ui_protocol::{TurnStateGetResult, UiContextState};

        let mut store = protocol_store_with_autonomy();
        let turn_id = TurnId::new();
        store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::TurnState(TurnStateGetResult {
                session_id: SessionKey("local:test".into()),
                turn_id: turn_id.clone(),
                state: TurnLifecycleState::Active,
                context: Some(serde_json::json!({"phase": "planning"})),
                context_state: Some(UiContextState {
                    session_id: SessionKey("local:test".into()),
                    thread_id: Some("thread-1".into()),
                    generation: 3,
                    transcript_hash: "abc123".into(),
                    item_count: 4,
                    token_estimate: 512,
                    recovery_state: "ready".into(),
                    last_checkpoint_id: None,
                    last_compaction_id: None,
                }),
                started_at: None,
                completed_at: None,
                thread_id: Some("thread-1".into()),
                committed_seqs: vec![1, 2],
            }),
        }));

        assert!(store.state.turn_state_detail.active);
        assert_eq!(store.state.turn_state_detail.title, "Turn State");
        assert!(
            store
                .state
                .turn_state_detail
                .subtitle
                .contains(&turn_id.0.to_string())
        );
        assert!(
            store
                .state
                .turn_state_detail
                .content
                .contains("state: active")
        );
        assert!(
            store
                .state
                .turn_state_detail
                .content
                .contains("thread: thread-1")
        );
        assert!(
            store
                .state
                .turn_state_detail
                .content
                .contains("committed seqs: 1, 2")
        );
        assert!(
            store
                .state
                .status
                .contains(&short_id(&turn_id.0.to_string()))
        );
    }

    #[test]
    fn hydrated_row_filter_drops_tool_and_empty_assistant_rows() {
        // mini5 soak: hydrate must render only the chat bubbles the live
        // transcript shows (user + assistant answers). Tool-result rows and
        // tool-call-only assistant rows (empty text) are activity, not bubbles —
        // rendering them double-shows a turn on reconnect (the "repeat" bug:
        // a tool turn went 2 live msgs -> 4 on hydrate before this filter).
        let now = chrono::Utc::now();
        let row = |role: &str, content: &str| HydratedMessage {
            seq: 1,
            role: role.into(),
            content: content.into(),
            turn_id: None,
            thread_id: None,
            client_message_id: None,
            persisted_at: now,
            message_id: None,
            source: None,
            media: Vec::new(),
        };
        assert!(hydrated_row_is_displayable(&row(
            "user",
            "search beijing weather"
        )));
        assert!(hydrated_row_is_displayable(&row(
            "assistant",
            "Here's the forecast"
        )));
        assert!(
            !hydrated_row_is_displayable(&row("tool", "Beijing — 7-day forecast ...")),
            "tool rows surface as activity chips, not transcript bubbles"
        );
        assert!(
            !hydrated_row_is_displayable(&row("assistant", "   ")),
            "tool-call-only assistant rows (empty text) are not bubbles"
        );
    }

    #[test]
    fn hydrate_preserves_an_in_flight_live_reply() {
        use crate::client_event::ClientEvent;
        // codex P1: a hydrate that lands MID-TURN must not drop the streaming
        // turn. `live_reply` (the in-flight, not-yet-committed turn) must survive
        // so its remaining deltas keep appending instead of freezing.
        let turn_id = TurnId::new();
        let mut store = store_with_live_reply(turn_id.clone(), "streaming so far");
        let session_id = store.state.sessions[0].id.clone();

        let result = SessionHydrateResult {
            session_id: session_id.clone(),
            cursor: octos_core::ui_protocol::UiCursor {
                stream: session_id.0.clone(),
                seq: 1,
            },
            context: None,
            context_state: None,
            messages: Some(vec![HydratedMessage {
                seq: 1,
                role: "user".into(),
                content: "earlier committed prompt".into(),
                turn_id: None,
                thread_id: None,
                client_message_id: None,
                persisted_at: chrono::Utc::now(),
                message_id: None,
                source: None,
                media: Vec::new(),
            }]),
            threads: None,
            turns: None,
            pending_approvals: None,
            pending_questions: None,
            replayed_envelopes: None,
        };
        store.apply_client_event(ClientEvent::SessionHydrate(result));

        let live_reply = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("in-flight live_reply must survive a hydrate");
        assert_eq!(
            live_reply.turn_id, turn_id,
            "the same streaming turn is preserved across hydrate"
        );
        assert_eq!(live_reply.text, "streaming so far");
    }

    #[test]
    fn session_hydrate_result_replaces_messages_and_pending_approval() {
        use crate::client_event::ClientEvent;

        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let now = chrono::Utc::now();
        let approval = ApprovalRequestedEvent::generic(
            session_id.clone(),
            ApprovalId::new(),
            turn_id.clone(),
            "write_file",
            "Approve write",
            "Allow write_file",
        );
        let result = SessionHydrateResult {
            session_id: session_id.clone(),
            cursor: UiCursor {
                stream: "session".into(),
                seq: 4,
            },
            context: None,
            context_state: Some(UiContextState {
                session_id: session_id.clone(),
                thread_id: Some("thread-1".into()),
                generation: 2,
                transcript_hash: "hash".into(),
                item_count: 2,
                token_estimate: 128,
                recovery_state: "healthy".into(),
                last_checkpoint_id: None,
                last_compaction_id: None,
            }),
            messages: Some(vec![
                HydratedMessage {
                    seq: 1,
                    role: "user".into(),
                    content: "build this".into(),
                    turn_id: Some(turn_id.clone()),
                    thread_id: Some("thread-1".into()),
                    client_message_id: Some("cmid-1".into()),
                    persisted_at: now,
                    message_id: Some("msg-user".into()),
                    source: Some("user".into()),
                    media: Vec::new(),
                },
                HydratedMessage {
                    seq: 2,
                    role: "assistant".into(),
                    content: "legacy companion".into(),
                    turn_id: Some(turn_id.clone()),
                    thread_id: Some("thread-1".into()),
                    client_message_id: None,
                    persisted_at: now,
                    message_id: Some("companion".into()),
                    source: Some("background".into()),
                    media: vec!["companion.md".into()],
                },
                HydratedMessage {
                    seq: 3,
                    role: "assistant".into(),
                    content: "legacy ack".into(),
                    turn_id: Some(turn_id.clone()),
                    thread_id: Some("thread-1".into()),
                    client_message_id: None,
                    persisted_at: now,
                    message_id: Some("spawn-ack".into()),
                    source: Some("background".into()),
                    media: Vec::new(),
                },
            ]),
            threads: Some(vec![ThreadGraphEntry {
                thread_id: "thread-1".into(),
                root_seq: 1,
                root_client_message_id: Some("cmid-1".into()),
                turn_id: Some(turn_id.clone()),
                message_seqs: vec![1, 3],
                status: "active".into(),
            }]),
            turns: Some(vec![HydratedTurn {
                turn_id: turn_id.clone(),
                state: TurnLifecycleState::Active,
                started_at: None,
                completed_at: None,
                thread_id: Some("thread-1".into()),
            }]),
            pending_approvals: Some(vec![approval]),
            pending_questions: None,
            replayed_envelopes: Some(vec![TurnSpawnCompleteEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id: Some(turn_id.clone()),
                thread_id: Some("thread-1".into()),
                task_id: "task-1".into(),
                tool_call_id: None,
                response_to_client_message_id: Some("cmid-1".into()),
                seq: 3,
                message_id: "spawn-ack".into(),
                source: "background".into(),
                cursor: UiCursor {
                    stream: "session".into(),
                    seq: 3,
                },
                persisted_at: now,
                content: "background result".into(),
                media: vec!["out.md".into()],
            }]),
        };

        store.apply_client_event(ClientEvent::SessionHydrate(result));

        let session = store.state.active_session().expect("active session");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].content, "build this");
        assert_eq!(session.messages[1].content, "background result");
        assert_eq!(session.messages[1].media, vec!["out.md".to_string()]);
        assert_eq!(session.messages[1].thread_id.as_deref(), Some("thread-1"));
        assert!(store.state.approval.is_some());
        assert_eq!(
            store.state.approval.as_ref().unwrap().title,
            "Approve write"
        );
        assert!(
            store
                .state
                .context_lifecycle_for(&session_id)
                .and_then(|ledger| ledger.state.as_ref())
                .is_some_and(|state| state.generation == 2)
        );
        assert!(store.state.status.contains("2 message(s)"));
        assert!(store.state.status.contains("1 pending approval"));
    }

    #[test]
    fn hydrate_reconciles_leaked_running_activity_in_terminal_turn() {
        use crate::client_event::ClientEvent;
        // GAP 1: a client rehydrating a session whose turn is already TERMINAL
        // (Completed/Errored/Interrupted) but that still carries a stranded
        // running-status activity item (a `ToolStarted` whose `ToolCompleted`
        // never arrived — leaked spawn_only chip / any uncovered path) must heal
        // the orphan. `apply_session_hydrate_result` never called the terminal
        // reconcile, so the chip pinned "Orchestrating…" forever after reconnect.
        let turn_id = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        // Leaked running item bound to the (about-to-be-terminal) turn.
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-leaked"),
        );

        let result = SessionHydrateResult {
            session_id: session_id.clone(),
            cursor: octos_core::ui_protocol::UiCursor {
                stream: session_id.0.clone(),
                seq: 1,
            },
            context: None,
            context_state: None,
            messages: None,
            threads: None,
            turns: Some(vec![HydratedTurn {
                turn_id: turn_id.clone(),
                state: TurnLifecycleState::Completed,
                started_at: None,
                completed_at: None,
                thread_id: Some("thread-1".into()),
            }]),
            pending_approvals: None,
            pending_questions: None,
            replayed_envelopes: None,
        };
        store.apply_client_event(ClientEvent::SessionHydrate(result));

        let leaked = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-leaked"))
            .expect("leaked item retained");
        assert_eq!(
            leaked.status,
            crate::model::ACTIVITY_STATUS_INTERRUPTED,
            "a terminal hydrated turn's stranded running item must be reconciled to interrupted"
        );
        assert!(
            !crate::model::activity_status_is_running(&leaked.status),
            "the reconciled item must no longer read as running"
        );
    }

    #[test]
    fn hydrate_does_not_reconcile_running_activity_in_active_turn() {
        use crate::client_event::ClientEvent;
        // GAP 1 guard against over-suppression: a hydrated turn that is still the
        // live/active turn (state == Active) carries genuine in-flight work. The
        // hydrate reconcile must touch ONLY terminal turns, so this running item
        // must stay running.
        let turn_id = TurnId::new();
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-live"),
        );

        let result = SessionHydrateResult {
            session_id: session_id.clone(),
            cursor: octos_core::ui_protocol::UiCursor {
                stream: session_id.0.clone(),
                seq: 1,
            },
            context: None,
            context_state: None,
            messages: None,
            threads: None,
            turns: Some(vec![HydratedTurn {
                turn_id: turn_id.clone(),
                state: TurnLifecycleState::Active,
                started_at: None,
                completed_at: None,
                thread_id: Some("thread-1".into()),
            }]),
            pending_approvals: None,
            pending_questions: None,
            replayed_envelopes: None,
        };
        store.apply_client_event(ClientEvent::SessionHydrate(result));

        let live = store
            .state
            .activity
            .iter()
            .find(|item| item.tool_call_id.as_deref() == Some("call-live"))
            .expect("live item retained");
        assert_eq!(
            live.status, "running",
            "an active (non-terminal) hydrated turn's running item must NOT be reconciled"
        );
    }

    #[test]
    fn hydrate_reconciles_leaked_running_activity_in_errored_and_interrupted_turns() {
        use crate::client_event::ClientEvent;
        // GAP 1 coverage: the hydrate reconcile treats ALL three terminal
        // lifecycle states identically via the shared sweep. The existing test
        // exercises `Completed`; assert `Errored` and `Interrupted` heal too.
        for terminal in [TurnLifecycleState::Errored, TurnLifecycleState::Interrupted] {
            let turn_id = TurnId::new();
            let mut store = store_with_empty_session();
            let session_id = store.state.sessions[0].id.clone();
            store.state.push_activity(
                ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                    .with_turn(turn_id.clone())
                    .with_tool_call("call-leaked"),
            );

            let result = SessionHydrateResult {
                session_id: session_id.clone(),
                cursor: octos_core::ui_protocol::UiCursor {
                    stream: session_id.0.clone(),
                    seq: 1,
                },
                context: None,
                context_state: None,
                messages: None,
                threads: None,
                turns: Some(vec![HydratedTurn {
                    turn_id: turn_id.clone(),
                    state: terminal,
                    started_at: None,
                    completed_at: None,
                    thread_id: Some("thread-1".into()),
                }]),
                pending_approvals: None,
                pending_questions: None,
                replayed_envelopes: None,
            };
            store.apply_client_event(ClientEvent::SessionHydrate(result));

            let leaked = store
                .state
                .activity
                .iter()
                .find(|item| item.tool_call_id.as_deref() == Some("call-leaked"))
                .expect("leaked item retained");
            assert_eq!(
                leaked.status,
                crate::model::ACTIVITY_STATUS_INTERRUPTED,
                "a {terminal:?} hydrated turn's stranded running item must be reconciled"
            );
        }
    }

    #[test]
    fn envelope_turn_completed_does_not_touch_non_tool_turn_less_row() {
        // GAP 2 kind guard: the envelope reconcile is filtered to
        // `ActivityKind::Tool`. A turn-less NON-tool row carrying this thread's
        // marker (e.g. a Progress row) must never be flipped, even when its
        // session+thread match the TurnCompleted barrier.
        let mut store = store_with_empty_session();
        let session_id = store.state.sessions[0].id.clone();
        store.state.push_activity(
            ActivityItem::new(ActivityKind::Progress, "sub-agent", "running")
                .with_session(session_id.clone())
                .with_detail(crate::model::AppState::envelope_tool_detail_for_thread(
                    "thread-1",
                )),
        );

        store.apply_event(AppUiEvent::Protocol(envelope_notification(
            session_id,
            1,
            Payload::TurnCompleted {
                token_usage: octos_core::ui_protocol::EnvelopeTokenUsage::default(),
            },
        )));

        let row = store
            .state
            .activity
            .iter()
            .find(|item| item.kind == ActivityKind::Progress && item.title == "sub-agent")
            .expect("non-tool progress row retained");
        assert!(
            crate::model::activity_status_is_running(&row.status),
            "a non-tool turn-less row must NOT be reconciled by the envelope sweep, got {:?}",
            row.status
        );
    }

    #[test]
    fn autonomy_loop_list_result_replaces_mirror_loops() {
        use crate::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
        use octos_core::ui_protocol::UiLoopRecord;
        let mut store = protocol_store_with_autonomy();
        let session_id = SessionKey("local:test".into());
        store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::LoopList(crate::model::LoopListResult {
                session_id: session_id.clone(),
                loops: vec![UiLoopRecord {
                    loop_id: "loop_a".into(),
                    session_id: session_id.clone(),
                    profile_id: None,
                    prompt: "p".into(),
                    mode: "fixed_interval".into(),
                    interval_seconds: Some(60),
                    status: "active".into(),
                    next_run_at_ms: None,
                    last_run_at_ms: None,
                    expires_at_ms: 1,
                    created_at_ms: 1,
                    updated_at_ms: 2,
                }],
            }),
        }));
        let mirror = store
            .state
            .session_autonomy_for(&session_id)
            .expect("mirror");
        assert_eq!(mirror.loops.len(), 1);
        assert!(store.state.status.contains("1 loop"));
    }

    #[test]
    fn session_open_enqueues_autonomy_hydration_when_advertised() {
        use octos_core::ui_protocol::SessionOpened;
        let mut store = protocol_store_with_autonomy();
        // Drop the live session so SessionOpened pushes a new one.
        store.state.sessions.clear();
        let session_id = SessionKey("local:fresh".into());
        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "active_profile_id": "coding",
            "workspace_root": null,
            "cursor": null,
            "panes": null,
        }))
        .expect("session_opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));
        // Three commands queued: agent/list, session/goal/get, loop/list.
        assert_eq!(store.state.pending_autonomy_hydration.len(), 3);
        let drained: Vec<_> =
            std::iter::from_fn(|| store.state.dequeue_autonomy_hydration()).collect();
        assert!(matches!(drained[0], AppUiCommand::ListAgents(_)));
        assert!(matches!(drained[1], AppUiCommand::GetSessionGoal(_)));
        assert!(matches!(drained[2], AppUiCommand::ListLoops(_)));
    }

    #[test]
    fn session_open_enqueues_session_hydrate_when_advertised() {
        use octos_core::ui_protocol::SessionOpened;

        let mut store = protocol_store_with_autonomy();
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods_and_features(
            [crate::model::APPUI_METHOD_SESSION_HYDRATE],
            [crate::model::APPUI_FEATURE_SESSION_HYDRATE_V1],
        ));
        store.state.sessions.clear();
        let session_id = SessionKey("local:test".into());
        let opened: SessionOpened = serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "active_profile_id": "coding",
            "workspace_root": null,
            "cursor": null,
            "panes": null,
        }))
        .expect("session_opened payload");
        store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

        let drained: Vec<_> =
            std::iter::from_fn(|| store.state.dequeue_autonomy_hydration()).collect();
        assert_eq!(drained.len(), 1);
        assert!(matches!(drained[0], AppUiCommand::HydrateSession(_)));
    }

    #[test]
    fn agent_list_command_method_matches_constant() {
        let session_id = SessionKey("local:test".into());
        let cmd = AppUiCommand::ListAgents(crate::model::AgentListParams {
            session_id,
            parent_agent_id: None,
        });
        assert_eq!(cmd.method(), crate::model::APPUI_METHOD_AGENT_LIST);
    }

    #[test]
    fn loop_create_command_method_matches_constant() {
        let session_id = SessionKey("local:test".into());
        let cmd = AppUiCommand::CreateLoop(crate::model::LoopCreateParams {
            session_id,
            profile_id: None,
            prompt: "p".into(),
            mode: crate::model::LoopMode::SelfPaced,
            interval_seconds: None,
        });
        assert_eq!(cmd.method(), crate::model::APPUI_METHOD_LOOP_CREATE);
    }

    #[test]
    fn goal_set_command_method_matches_constant() {
        let session_id = SessionKey("local:test".into());
        let cmd = AppUiCommand::SetSessionGoal(crate::model::SessionGoalSetParams {
            session_id,
            profile_id: None,
            objective: "foo".into(),
            status: Some("active".into()),
            token_budget: None,
            transition_actor: Some("user".into()),
            action: crate::model::SessionGoalSetAction::Set,
        });
        assert_eq!(cmd.method(), crate::model::APPUI_METHOD_SESSION_GOAL_SET);
    }

    #[test]
    fn goal_set_params_serializes_without_action_field() {
        // The `action` classifier is `#[serde(skip)]` — it stays
        // local. The wire shape carries `objective` + `status`.
        let params = crate::model::SessionGoalSetParams {
            session_id: SessionKey("local:test".into()),
            profile_id: Some("coding".into()),
            objective: "ship it".into(),
            status: Some("active".into()),
            token_budget: None,
            transition_actor: Some("user".into()),
            action: crate::model::SessionGoalSetAction::Set,
        };
        let json = serde_json::to_value(&params).expect("serialize");
        assert!(json.get("objective").is_some());
        assert!(json.get("status").is_some());
        assert!(
            json.get("action").is_none(),
            "action must NOT appear on the wire: {json}"
        );
    }
}
