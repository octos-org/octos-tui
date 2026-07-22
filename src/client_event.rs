use octos_core::{
    SessionKey,
    app_ui::AppUiEvent,
    ui_protocol::{
        PermissionProfileSelection, SessionHydrateResult, SessionListResult, SessionRollbackResult,
        TaskArtifactReadResult, ThreadGraphGetResult, TurnStateGetResult,
    },
};

use crate::model::{
    AgentArtifactListResult, AgentArtifactReadResult, AgentCloseResult, AgentInterruptResult,
    AgentListResult, AgentOutputReadResult, AgentStatusReadResult, AuthLogoutResult, AuthMeResult,
    AuthSendCodeResult, AuthStatusResult, AuthVerifyResult, ConfigCapabilitiesListResult,
    DiffPreviewGetResult, LaunchResolveResult, LoopCreateResult, LoopListResult,
    LoopMutationResult, McpConfigListResult, McpConfigMutationResult, McpStatusListResult,
    ModelListResult, ModelSelectResult, ProfileLlmCatalogResult, ProfileLlmListResult,
    ProfileLlmMutationResult, ProfileLocalCreateResult, ProfileSkillsListResult,
    ProfileSkillsMutationResult, ProfileSkillsRegistrySearchResult, ReviewStartResult,
    SessionGoalClearResult, SessionGoalGetResult, SessionGoalSetResult, SessionStatusReadResult,
    SubProvidersListResult, SubProvidersMutationResult, ToolConfigListResult,
    ToolConfigMutationResult, ToolStatusListResult,
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
    SessionHydrate(SessionHydrateResult),
    /// Result of a `session/list` request, used to populate the `/resume`
    /// session picker.
    SessionList(SessionListResult),
    /// Result of a `launch/resolve` request: the per-project launch decision
    /// that drives whether the client resumes the folder's brain, prompts to
    /// activate the space, offers a cross-profile switch, or opens onboarding
    /// on first launch.
    LaunchResolve(LaunchResolveResult),
    /// Result of a `session/rollback` request (`/rewind`): the later user turns
    /// were dropped from the session and `thread` carries the trimmed
    /// transcript to re-render (same shape as `session/hydrate`).
    SessionRollback(SessionRollbackResult),
    ReviewStart(ReviewStartResult),
    AuthStatus(AuthStatusClientEvent),
    AuthSendCode(AuthSendCodeClientEvent),
    AuthVerify(AuthVerifyClientEvent),
    AuthMe(AuthMeClientEvent),
    AuthLogout(AuthLogoutClientEvent),
    ProfileLocalCreate(ProfileLocalCreateClientEvent),
    ProfileLlmCatalog(ProfileLlmCatalogClientEvent),
    ProfileLlmList(ProfileLlmListClientEvent),
    ProfileLlmMutation(ProfileLlmMutationClientEvent),
    SubProvidersList(SubProvidersListClientEvent),
    /// #1768: snapshot list/restore result (restore echoes refreshed rows).
    SnapshotList(SnapshotListClientEvent),
    /// #395: `peer/prepare` result — the server minted a peer slug/topic and
    /// wrote the durable brief; the store mints the peer session key, stashes
    /// the kickoff, and follows up with `session/open`.
    PeerPrepared(PeerPreparedClientEvent),
    SubProvidersMutation(SubProvidersMutationClientEvent),
    ProfileSkillsList(ProfileSkillsListClientEvent),
    ProfileSkillsRegistrySearch(ProfileSkillsRegistrySearchClientEvent),
    ProfileSkillsMutation(ProfileSkillsMutationClientEvent),
    SessionStatus(SessionStatusClientEvent),
    SessionBtw(SessionBtwClientEvent),
    ToolStatus(ToolStatusClientEvent),
    ToolConfigList(ToolConfigListClientEvent),
    ToolConfigMutation(ToolConfigMutationClientEvent),
    /// M15-E backend-owned autonomy result event. Carries the raw
    /// typed result from one of the `/agents`, `/goal`, or `/loop`
    /// RPCs so the store can update its per-session autonomy mirror.
    Autonomy(AutonomyClientEvent),
    /// `!`-bang local-shell completion. Carries the captured output of a
    /// client-local shell command (run where octos-tui runs, NOT the
    /// agent's sandboxed server `shell` tool). Surfaced into the same
    /// `queue` that `next_event()` drains, so the synchronous render loop
    /// never blocks on a running command. The store folds this back into
    /// the matching "running" activity chip via its `local_id`.
    LocalShellResult(LocalShellResultEvent),
    /// The stdio transport relaunched its `serve --stdio` child after a
    /// disconnect. A freshly spawned child has no in-flight turns by
    /// construction, so any turn the client still shows as live belongs to
    /// the dead process and its terminal event will never arrive — the store
    /// must fail those latched turns and drain the staged prompt queue, or
    /// every subsequent prompt wedges behind the phantom turn forever.
    BackendRelaunched,
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
    /// The session that initiated the select (correlated by JSON-RPC request
    /// id in the transport). `None` only for unsolicited/legacy shapes —
    /// which the store ignores rather than guessing a target.
    pub initiating_session: Option<octos_core::SessionKey>,
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

/// #1768 snapshot undo list (also carries restore acknowledgements).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotListClientEvent {
    pub message: String,
    pub result: crate::model::SnapshotListResult,
}

/// #395 `peer/prepare` result for the `/peer` flow.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerPreparedClientEvent {
    pub message: String,
    pub result: crate::model::PeerPrepareResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubProvidersListClientEvent {
    pub result: SubProvidersListResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubProvidersMutationClientEvent {
    pub result: SubProvidersMutationResult,
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

/// Result of a `session/btw` aside — the out-of-band answer to a quick
/// question asked while the session's turn keeps running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBtwClientEvent {
    pub result: octos_core::ui_protocol::SessionBtwResult,
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

/// Result of a `!`-bang client-local shell command. The transport spawns the
/// command on its tokio runtime and emits one of these on completion (or on
/// timeout / spawn failure), keyed by the `local_id` the store stamped on the
/// "running" activity chip so the store can complete that chip in place.
///
/// Output is captured (stdout then stderr) and already truncated by the
/// transport at the 10 KB combined cap; `truncated` records whether the cap
/// fired. The output is shown locally only and is NOT injected into the next
/// turn's context (ephemeral, by design).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalShellResultEvent {
    /// Local chip id stamped by `Store::dispatch_bang_command`.
    pub local_id: String,
    /// The command line as typed (after the `!`), for display.
    pub cmdline: String,
    /// Display form of the directory the command ran in (the TUI process cwd),
    /// so the transcript card can label WHERE the local command executed.
    /// `None` only when the cwd could not be resolved at spawn time.
    pub cwd: Option<String>,
    /// Captured stdout (already truncated to fit the combined 10 KB cap).
    pub stdout: String,
    /// Captured stderr (already truncated to fit the combined 10 KB cap).
    pub stderr: String,
    /// Process exit code, or `None` if it was killed (e.g. timeout) or never
    /// produced one.
    pub exit_code: Option<i32>,
    /// Wall-clock duration of the command, in milliseconds.
    pub duration_ms: u64,
    /// Whether the combined output was truncated at the 10 KB cap.
    pub truncated: bool,
}
