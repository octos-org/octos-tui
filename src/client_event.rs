use octos_core::{
    SessionKey,
    app_ui::AppUiEvent,
    ui_protocol::{
        PermissionProfileSelection, TaskArtifactReadResult, ThreadGraphGetResult,
        TurnStateGetResult,
    },
};

use crate::model::{
    AgentArtifactListResult, AgentArtifactReadResult, AgentCloseResult, AgentInterruptResult,
    AgentListResult, AgentOutputReadResult, AgentStatusReadResult, AuthLogoutResult, AuthMeResult,
    AuthSendCodeResult, AuthStatusResult, AuthVerifyResult, ConfigCapabilitiesListResult,
    DiffPreviewGetResult, LoopCreateResult, LoopListResult, LoopMutationResult,
    McpConfigListResult, McpConfigMutationResult, McpStatusListResult, ModelListResult,
    ModelSelectResult, ProfileLlmCatalogResult, ProfileLlmListResult, ProfileLlmMutationResult,
    ProfileLocalCreateResult, ProfileSkillsListResult, ProfileSkillsMutationResult,
    ProfileSkillsRegistrySearchResult, SessionGoalClearResult, SessionGoalGetResult,
    SessionGoalSetResult, SessionStatusReadResult, ToolConfigListResult, ToolConfigMutationResult,
    ToolStatusListResult,
};

#[derive(Debug, Clone)]
pub enum ClientEvent {
    App(Box<AppUiEvent>),
    Capabilities(CapabilitiesClientEvent),
    DiffPreview(DiffPreviewGetResult),
    ModelList(ModelListClientEvent),
    ModelSelect(ModelSelectClientEvent),
    McpStatus(McpStatusClientEvent),
    McpConfigList(McpConfigListClientEvent),
    McpConfigMutation(McpConfigMutationClientEvent),
    PermissionProfile(PermissionProfileClientEvent),
    AuthStatus(AuthStatusClientEvent),
    AuthSendCode(AuthSendCodeClientEvent),
    AuthVerify(AuthVerifyClientEvent),
    AuthMe(AuthMeClientEvent),
    AuthLogout(AuthLogoutClientEvent),
    ProfileLocalCreate(ProfileLocalCreateClientEvent),
    ProfileLlmCatalog(ProfileLlmCatalogClientEvent),
    ProfileLlmList(ProfileLlmListClientEvent),
    ProfileLlmMutation(ProfileLlmMutationClientEvent),
    ProfileSkillsList(ProfileSkillsListClientEvent),
    ProfileSkillsRegistrySearch(ProfileSkillsRegistrySearchClientEvent),
    ProfileSkillsMutation(ProfileSkillsMutationClientEvent),
    SessionStatus(SessionStatusClientEvent),
    ToolStatus(ToolStatusClientEvent),
    ToolConfigList(ToolConfigListClientEvent),
    ToolConfigMutation(ToolConfigMutationClientEvent),
    /// M15-E backend-owned autonomy result event. Carries the raw
    /// typed result from one of the `/agents`, `/goal`, or `/loop`
    /// RPCs so the store can update its per-session autonomy mirror.
    Autonomy(AutonomyClientEvent),
}

impl From<AppUiEvent> for ClientEvent {
    fn from(event: AppUiEvent) -> Self {
        Self::App(Box::new(event))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitiesClientEvent {
    pub result: ConfigCapabilitiesListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelListClientEvent {
    pub result: ModelListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectClientEvent {
    pub result: ModelSelectResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStatusClientEvent {
    pub result: McpStatusListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigListClientEvent {
    pub result: McpConfigListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigMutationClientEvent {
    pub result: McpConfigMutationResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionProfileClientEvent {
    pub session_id: SessionKey,
    pub current: PermissionProfileSelection,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatusClientEvent {
    pub result: AuthStatusResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSendCodeClientEvent {
    pub result: AuthSendCodeResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthVerifyClientEvent {
    pub result: AuthVerifyResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthMeClientEvent {
    pub result: AuthMeResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLogoutClientEvent {
    pub result: AuthLogoutResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileLocalCreateClientEvent {
    pub result: ProfileLocalCreateResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileLlmCatalogClientEvent {
    pub result: ProfileLlmCatalogResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileLlmListClientEvent {
    pub result: ProfileLlmListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileLlmMutationClientEvent {
    pub result: ProfileLlmMutationResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSkillsListClientEvent {
    pub result: ProfileSkillsListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSkillsRegistrySearchClientEvent {
    pub result: ProfileSkillsRegistrySearchResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSkillsMutationClientEvent {
    pub result: ProfileSkillsMutationResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStatusClientEvent {
    pub result: SessionStatusReadResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatusClientEvent {
    pub result: ToolStatusListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConfigListClientEvent {
    pub result: ToolConfigListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConfigMutationClientEvent {
    pub result: ToolConfigMutationResult,
    pub message: String,
}

/// M15-E typed autonomy result. We keep one variant per RPC so the
/// store can pattern-match on the precise wire shape rather than
/// reparsing a generic JSON blob.
#[derive(Debug, Clone, PartialEq)]
pub enum AutonomyResult {
    AgentList(AgentListResult),
    AgentStatus(AgentStatusReadResult),
    AgentOutput(AgentOutputReadResult),
    AgentArtifacts(AgentArtifactListResult),
    AgentArtifactRead(AgentArtifactReadResult),
    TaskArtifactRead(TaskArtifactReadResult),
    ThreadGraph(ThreadGraphGetResult),
    TurnState(TurnStateGetResult),
    AgentInterrupt(AgentInterruptResult),
    AgentClose(AgentCloseResult),
    GoalGet(SessionGoalGetResult),
    GoalSet(SessionGoalSetResult),
    GoalClear(SessionGoalClearResult),
    LoopCreate(LoopCreateResult),
    LoopList(LoopListResult),
    /// `loop/delete`, `loop/pause`, `loop/resume`, `loop/fire_now`
    /// share one wire shape; we keep the method around so the store
    /// can emit a precise status line.
    LoopMutation {
        method: String,
        result: LoopMutationResult,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutonomyClientEvent {
    pub result: AutonomyResult,
}
