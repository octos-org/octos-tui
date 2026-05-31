use std::time::Instant;

use octos_core::app_ui::{APP_UI_API_V1, AppUiLiveReply, AppUiSession, AppUiSnapshot, AppUiTask};
use octos_core::ui_protocol::{
    ApprovalDecision, ApprovalId, ApprovalRenderHints, ApprovalRequestedEvent,
    ApprovalScopesListParams, ApprovalTypedDetails, DiffPreviewGetParams, OutputCursor,
    PermissionProfileListParams, PermissionProfileSelection, PermissionProfileSetParams, PreviewId,
    TaskArtifactReadParams, TaskCancelParams, TaskListParams, TaskOutputReadParams,
    TaskRestartFromNodeParams, TaskRuntimeState, ThreadGraphGetParams, ThreadGraphGetResult,
    TurnId, TurnInterruptParams, TurnStartParams, TurnStateGetParams, TurnStateGetResult,
    UiPaneSnapshot, UiProtocolCapabilities, approval_scopes,
};
use octos_core::{Message, SessionKey, TaskId};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;
use unicode_width::UnicodeWidthStr;

use crate::menu::{
    AvailabilityContext, CapabilitySet, ConnectionState, MenuBuildResult, MenuStack, RuntimeMode,
    TaskActivity,
};

pub type LiveReply = AppUiLiveReply;
pub type SessionView = AppUiSession;
pub type TaskView = AppUiTask;

pub const APPUI_METHOD_CONFIG_CAPABILITIES_LIST: &str = "config/capabilities/list";
pub const APPUI_METHOD_SESSION_STATUS_READ: &str = "session/status/read";
pub const APPUI_METHOD_MODEL_LIST: &str = "profile/llm/list";
pub const APPUI_METHOD_MODEL_SELECT: &str = "profile/llm/select";
pub const APPUI_METHOD_MCP_STATUS_LIST: &str = "mcp/status/list";
pub const APPUI_METHOD_TOOL_STATUS_LIST: &str = "tool/status/list";
pub const APPUI_METHOD_MCP_CONFIG_LIST: &str = "mcp/config/list";
pub const APPUI_METHOD_MCP_CONFIG_UPSERT: &str = "mcp/config/upsert";
pub const APPUI_METHOD_MCP_CONFIG_DELETE: &str = "mcp/config/delete";
pub const APPUI_METHOD_MCP_CONFIG_SET_ENABLED: &str = "mcp/config/set_enabled";
pub const APPUI_METHOD_MCP_CONFIG_TEST: &str = "mcp/config/test";
pub const APPUI_METHOD_TOOL_CONFIG_LIST: &str = "tool/config/list";
pub const APPUI_METHOD_TOOL_CONFIG_SET_ENABLED: &str = "tool/config/set_enabled";
pub const APPUI_METHOD_TOOL_CONFIG_UPSERT: &str = "tool/config/upsert";
pub const APPUI_METHOD_TOOL_CONFIG_DELETE: &str = "tool/config/delete";
pub const APPUI_METHOD_TOOL_CONFIG_TEST: &str = "tool/config/test";
pub const APPUI_METHOD_AUTH_STATUS: &str = "auth/status";
pub const APPUI_METHOD_AUTH_SEND_CODE: &str = "auth/send_code";
pub const APPUI_METHOD_AUTH_VERIFY: &str = "auth/verify";
pub const APPUI_METHOD_AUTH_ME: &str = "auth/me";
pub const APPUI_METHOD_AUTH_LOGOUT: &str = "auth/logout";
pub const APPUI_METHOD_PROFILE_LOCAL_CREATE: &str = "profile/local/create";
pub const APPUI_METHOD_PROFILE_LLM_CATALOG: &str = "profile/llm/catalog";
pub const APPUI_METHOD_PROFILE_LLM_UPSERT: &str = "profile/llm/upsert";
pub const APPUI_METHOD_PROFILE_LLM_DELETE: &str = "profile/llm/delete";
pub const APPUI_METHOD_PROFILE_LLM_TEST: &str = "profile/llm/test";
pub const APPUI_METHOD_PROFILE_LLM_FETCH_MODELS: &str = "profile/llm/fetch_models";
pub const APPUI_METHOD_PROFILE_SKILLS_LIST: &str = "profile/skills/list";
pub const APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH: &str = "profile/skills/registry/search";
pub const APPUI_METHOD_PROFILE_SKILLS_INSTALL: &str = "profile/skills/install";
pub const APPUI_METHOD_PROFILE_SKILLS_REMOVE: &str = "profile/skills/remove";

/// M12-E feature flag for per-session workspace cwd requests
/// (`session.workspace_cwd.v1`, UPCR-2026-003). The TUI must NOT
/// include `cwd` in `session/open` until the server advertises this
/// feature — otherwise compatible-but-old servers reject the request
/// or worse, ignore the cwd silently and run against the wrong root.
pub const APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1: &str = "session.workspace_cwd.v1";

/// Returns `true` when the negotiated capabilities permit attaching
/// a `cwd` to `session/open`. Per UPCR-2026-003, the client must NOT
/// emit `cwd` until the server advertises
/// [`APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1`]. Callers pass the
/// `supported_features` slice from
/// [`octos_core::ui_protocol::UiProtocolCapabilities`] (or the
/// equivalent slice the TUI's `CapabilitySet` tracks).
pub fn session_open_may_include_cwd<S: AsRef<str>>(supported_features: &[S]) -> bool {
    supported_features
        .iter()
        .any(|feature| feature.as_ref() == APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1)
}

/// Returns the displayable workspace root for a session: the
/// server-confirmed `workspace_root` from `session/status/read`
/// wins. Only when the server omits it does the TUI fall back to the
/// `cwd` it requested. The TUI must NOT silently substitute the
/// requested cwd for the server truth in any other case — it can
/// only render what the server said. This helper is the canonical
/// "what cwd should we show" decision the TUI must use.
pub fn effective_workspace_root_for_display<'a>(
    server_workspace_root: Option<&'a str>,
    requested_cwd: Option<&'a str>,
) -> Option<&'a str> {
    server_workspace_root.or(requested_cwd)
}

/// Scrub `cwd` from a [`octos_core::ui_protocol::SessionOpenParams`]
/// when the negotiated capabilities do not advertise
/// [`APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1`]. Returns the params
/// unchanged when the feature is present (or when `cwd` was already
/// `None`). The TUI uses this immediately before serializing
/// `session/open` so that compatible-but-old servers do not silently
/// ignore the requested cwd.
pub fn scrub_session_open_cwd_for_capabilities<S: AsRef<str>>(
    mut params: octos_core::ui_protocol::SessionOpenParams,
    supported_features: &[S],
) -> octos_core::ui_protocol::SessionOpenParams {
    if params.cwd.is_some() && !session_open_may_include_cwd(supported_features) {
        params.cwd = None;
    }
    params
}

/// M13-D backend-owned supervised task inspection methods. The TUI calls the
/// `task/artifact/*` aliases per UPCR-2026-019 §4 (servers dispatch both
/// `task/artifact/list` and `agent/artifact/list` into the same handler).
pub const APPUI_METHOD_TASK_ARTIFACT_LIST: &str = "task/artifact/list";
pub const APPUI_METHOD_TASK_ARTIFACT_READ: &str = "task/artifact/read";
/// Optional M13-D review entrypoint (`review.start.v1` capability-gated).
pub const APPUI_METHOD_REVIEW_START: &str = "review/start";

/// M13-D capability flag for backend-owned supervised task list/status
/// inspection (`harness.task_supervision_inspection.v1`). When absent, the
/// TUI must hide M13 inspection controls and never invent local supervisor
/// state.
pub const APPUI_FEATURE_TASK_SUPERVISION_INSPECTION_V1: &str =
    "harness.task_supervision_inspection.v1";

/// M13-D capability flag for `task/artifact/list` and `task/artifact/read`
/// (`harness.task_artifacts.v1`). When absent, the TUI must hide the
/// artifact browser entry points.
pub const APPUI_FEATURE_TASK_ARTIFACTS_V1: &str = "harness.task_artifacts.v1";

/// M16-G2 capability flag for backend-owned context generation,
/// checkpoint, and compaction lifecycle inspection
/// (`context.lifecycle.v1`). When absent, the TUI must hide the
/// compact-context status surface and never invent a generation number
/// from local heuristics.
pub const APPUI_FEATURE_CONTEXT_LIFECYCLE_V1: &str = "context.lifecycle.v1";

/// M16-G2 notification methods. The TUI listens for these to bump the
/// compact-context status surface; it must not call them as RPC.
pub const APPUI_METHOD_CONTEXT_COMPACTION_COMPLETED: &str = "context/compaction_completed";
pub const APPUI_METHOD_CONTEXT_NORMALIZATION_REPORTED: &str = "context/normalization_reported";

/// UPCR-2026-010 thread graph read surface (`state.thread_graph.v1`).
pub const APPUI_FEATURE_THREAD_GRAPH_V1: &str = "state.thread_graph.v1";
pub const APPUI_METHOD_THREAD_GRAPH_GET: &str = "thread/graph/get";

/// UPCR-2026-011 turn lifecycle state read surface.
pub const APPUI_FEATURE_TURN_STATE_GET_V1: &str = "state.turn_state_get.v1";
pub const APPUI_METHOD_TURN_STATE_GET: &str = "turn/state/get";

/// M15-E required capability flag for backend-owned agent inspection /
/// goal / loop UX (`coding.autonomy.v1`). When absent, the TUI must
/// hide M15 controls instead of probing unsupported methods.
pub const APPUI_FEATURE_CODING_AUTONOMY_V1: &str = "coding.autonomy.v1";

/// M15-E optional capability flags. Each gates one slice of UX:
/// `agent_control_v1` -> `/agents interrupt`, `/agents close`.
/// `goal_runtime_v1`  -> `/goal` family.
/// `loop_runtime_v1`  -> `/loop` family.
pub const APPUI_FEATURE_CODING_AGENT_CONTROL_V1: &str = "coding.agent_control.v1";
pub const APPUI_FEATURE_CODING_GOAL_RUNTIME_V1: &str = "coding.goal_runtime.v1";
pub const APPUI_FEATURE_CODING_LOOP_RUNTIME_V1: &str = "coding.loop_runtime.v1";

/// M15-E backend-owned agent inspection methods (UPCR-2026-021).
pub const APPUI_METHOD_AGENT_LIST: &str = "agent/list";
pub const APPUI_METHOD_AGENT_STATUS_READ: &str = "agent/status/read";
pub const APPUI_METHOD_AGENT_OUTPUT_READ: &str = "agent/output/read";
pub const APPUI_METHOD_AGENT_ARTIFACT_LIST: &str = "agent/artifact/list";
pub const APPUI_METHOD_AGENT_ARTIFACT_READ: &str = "agent/artifact/read";

/// M15-E backend-owned agent control methods (UPCR-2026-021 §"Agent
/// Lifecycle Surface"). These are gated on
/// `coding.agent_control.v1`.
pub const APPUI_METHOD_AGENT_INTERRUPT: &str = "agent/interrupt";
pub const APPUI_METHOD_AGENT_CLOSE: &str = "agent/close";

/// M15-E backend-owned goal runtime methods (UPCR-2026-021 §"Goal
/// Runtime Surface"). These are gated on `coding.goal_runtime.v1`.
pub const APPUI_METHOD_SESSION_GOAL_GET: &str = "session/goal/get";
pub const APPUI_METHOD_SESSION_GOAL_SET: &str = "session/goal/set";
pub const APPUI_METHOD_SESSION_GOAL_CLEAR: &str = "session/goal/clear";

/// M15-E backend-owned loop runtime methods (UPCR-2026-021 §"Loop
/// Runtime Surface"). These are gated on `coding.loop_runtime.v1`.
pub const APPUI_METHOD_LOOP_CREATE: &str = "loop/create";
pub const APPUI_METHOD_LOOP_LIST: &str = "loop/list";
pub const APPUI_METHOD_LOOP_DELETE: &str = "loop/delete";
pub const APPUI_METHOD_LOOP_PAUSE: &str = "loop/pause";
pub const APPUI_METHOD_LOOP_RESUME: &str = "loop/resume";
pub const APPUI_METHOD_LOOP_FIRE_NOW: &str = "loop/fire_now";

/// M15-E notification methods the TUI listens for to update agent /
/// goal / loop state. It must not call these as RPC.
pub const APPUI_METHOD_AGENT_UPDATED: &str = "agent/updated";
pub const APPUI_METHOD_AGENT_OUTPUT_DELTA: &str = "agent/output/delta";
pub const APPUI_METHOD_AGENT_ARTIFACT_UPDATED: &str = "agent/artifact/updated";
pub const APPUI_METHOD_SESSION_GOAL_UPDATED: &str = "session/goal/updated";
pub const APPUI_METHOD_SESSION_GOAL_CLEARED: &str = "session/goal/cleared";
pub const APPUI_METHOD_LOOP_UPDATED: &str = "loop/updated";
pub const APPUI_METHOD_LOOP_FIRED: &str = "loop/fired";
pub const APPUI_METHOD_LOOP_COMPLETED: &str = "loop/completed";

// ---------- M15-E AppUI param + result types ----------
//
// These params types model the request side of the autonomy surface
// (`/agents`, `/goal`, `/loop`). Upstream `octos-core` already owns
// the wire shape for notifications (`UiAgentRecord`, `UiGoalRecord`,
// `UiLoopRecord`, etc.) — we re-use those for results so the
// rendered state stays in lockstep with what the backend stamps.
//
// All TUI-side mutating dispatch goes through `require_appui_method`
// in `store.rs`. Servers that do not advertise the methods will see
// the slash command rendered as `Unsupported` instead of being
// probed.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentListParams {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentListResult {
    pub session_id: SessionKey,
    #[serde(default)]
    pub agents: Vec<octos_core::ui_protocol::UiAgentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatusReadParams {
    pub session_id: SessionKey,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusReadResult {
    pub session_id: SessionKey,
    pub agent: octos_core::ui_protocol::UiAgentRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutputReadParams {
    pub session_id: SessionKey,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OutputCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutputReadResult {
    pub session_id: SessionKey,
    pub agent_id: String,
    pub cursor: OutputCursor,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArtifactListParams {
    pub session_id: SessionKey,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentArtifactListResult {
    pub session_id: SessionKey,
    pub agent_id: String,
    #[serde(default)]
    pub artifacts: Vec<octos_core::ui_protocol::UiAgentArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArtifactReadParams {
    pub session_id: SessionKey,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentArtifactReadResult {
    pub session_id: SessionKey,
    pub agent_id: String,
    pub artifact: octos_core::ui_protocol::UiAgentArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInterruptParams {
    pub session_id: SessionKey,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInterruptResult {
    pub session_id: SessionKey,
    pub agent_id: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<octos_core::ui_protocol::UiAgentRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCloseParams {
    pub session_id: SessionKey,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCloseResult {
    pub session_id: SessionKey,
    pub agent_id: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<octos_core::ui_protocol::UiAgentRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGoalGetParams {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionGoalGetResult {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<octos_core::ui_protocol::UiGoalRecord>,
}

/// Logical action a `/goal` subcommand performed. This is a TUI-side
/// classifier; the wire shape itself is the `(objective, status)`
/// pair the backend expects. We keep `SessionGoalSetAction` around for
/// the dispatch tests so they can assert the intended verb without
/// re-parsing the serialized params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionGoalSetAction {
    /// `/goal <objective>` — establish a new active goal.
    #[default]
    Set,
    /// `/goal pause` — pause an active goal.
    Pause,
    /// `/goal resume` — resume a paused goal.
    Resume,
}

/// `session/goal/set` wire shape (UPCR-2026-021 §"Goal Runtime Surface").
/// Matches the backend `RawGoalSetParams` exactly: `objective` is
/// REQUIRED, and `status` ("active"/"paused") is what drives
/// pause/resume transitions. `transition_actor` is always `"user"`
/// from the TUI — the backend marks model-completed goals with
/// `"model"` itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGoalSetParams {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_actor: Option<String>,
    /// Non-wire classifier used by the dispatch tests. `#[serde(skip)]`
    /// keeps it out of the JSON-RPC payload while still letting tests
    /// assert which subcommand produced this params instance.
    #[serde(skip)]
    pub action: SessionGoalSetAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionGoalSetResult {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<octos_core::ui_protocol::UiGoalRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_actor: Option<String>,
}

/// Two-step goal pause/resume state. Pause/resume must NOT carry a
/// possibly-stale cached objective to the backend (the cached mirror
/// can drift between `session/goal/get` refreshes). Instead, the
/// dispatch issues a `session/goal/get` first and stages the desired
/// transition here; when the `GoalGet` response arrives, the store
/// emits the follow-up `session/goal/set` with the freshly-fetched
/// objective and the staged status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGoalTransition {
    pub session_id: SessionKey,
    pub profile_id: Option<String>,
    /// `"paused"` for `/goal pause`, `"active"` for `/goal resume`.
    pub status: &'static str,
    /// TUI-side classifier echoed into the emitted
    /// [`SessionGoalSetParams::action`].
    pub action: SessionGoalSetAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGoalClearParams {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionGoalClearResult {
    pub session_id: SessionKey,
    #[serde(default)]
    pub cleared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_actor: Option<String>,
}

/// Loop cadence parsed from `/loop`. `interval_seconds` is `None` for
/// self-paced loops and for maintenance loops. The backend decides
/// the cadence for those two modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    FixedInterval,
    SelfPaced,
    Maintenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopCreateParams {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub prompt: String,
    pub mode: LoopMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopCreateResult {
    pub session_id: SessionKey,
    #[serde(rename = "loop")]
    pub loop_state: octos_core::ui_protocol::UiLoopRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopListParams {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopListResult {
    pub session_id: SessionKey,
    #[serde(default)]
    pub loops: Vec<octos_core::ui_protocol::UiLoopRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopIdParams {
    pub session_id: SessionKey,
    pub loop_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopMutationResult {
    pub session_id: SessionKey,
    pub loop_id: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_state: Option<octos_core::ui_protocol::UiLoopRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire: Option<octos_core::ui_protocol::UiLoopFire>,
}

/// Per-session autonomy mirror state. Populated by `agent/list`,
/// `session/goal/get`, `loop/list` responses and by the matching
/// notifications. The TUI re-fetches this on session open and on
/// reconnect — local config is never used to fill it in.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionAutonomyState {
    pub session_id: SessionKey,
    pub agents: Vec<octos_core::ui_protocol::UiAgentRecord>,
    pub agent_outputs: Vec<AutonomyAgentOutputCache>,
    pub agent_artifacts: Vec<AutonomyAgentArtifactCache>,
    pub goal: Option<octos_core::ui_protocol::UiGoalRecord>,
    pub goal_transition_actor: Option<String>,
    pub loops: Vec<octos_core::ui_protocol::UiLoopRecord>,
}

impl SessionAutonomyState {
    pub fn new(session_id: SessionKey) -> Self {
        Self {
            session_id,
            agents: Vec::new(),
            agent_outputs: Vec::new(),
            agent_artifacts: Vec::new(),
            goal: None,
            goal_transition_actor: None,
            loops: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomyAgentOutputCache {
    pub agent_id: String,
    pub text: String,
    pub cursor: OutputCursor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutonomyAgentArtifactCache {
    pub agent_id: String,
    pub artifacts: Vec<octos_core::ui_protocol::UiAgentArtifact>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_for_transport(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn masked(&self) -> &'static str {
        if self.0.is_empty() { "" } else { "********" }
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"********\"")
    }
}

fn string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn is_empty_string(value: &str) -> bool {
    value.trim().is_empty()
}

fn route_or_default<'de, D>(deserializer: D) -> Result<LlmRouteConfig, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<LlmRouteConfig>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppUiCommand {
    OpenSession(octos_core::ui_protocol::SessionOpenParams),
    SubmitPrompt(TurnStartParams),
    InterruptTurn(TurnInterruptParams),
    RespondApproval(octos_core::ui_protocol::ApprovalRespondParams),
    ListApprovalScopes(ApprovalScopesListParams),
    GetDiffPreview(DiffPreviewGetParams),
    ListTasks(TaskListParams),
    CancelTask(TaskCancelParams),
    RestartTaskFromNode(TaskRestartFromNodeParams),
    ReadTaskOutput(TaskOutputReadParams),
    ReadTaskArtifact(TaskArtifactReadParams),
    GetThreadGraph(ThreadGraphGetParams),
    GetTurnState(TurnStateGetParams),
    ListConfigCapabilities(ConfigCapabilitiesListParams),
    ReadSessionStatus(SessionStatusReadParams),
    ListModels(ModelListParams),
    SelectModel(ModelSelectParams),
    ListPermissionProfiles(PermissionProfileListParams),
    SetPermissionProfile(PermissionProfileSetParams),
    ListMcpStatus(McpStatusListParams),
    ListToolStatus(ToolStatusListParams),
    ListMcpConfig(McpConfigListParams),
    UpsertMcpConfig(McpConfigUpsertParams),
    DeleteMcpConfig(McpConfigDeleteParams),
    SetMcpConfigEnabled(McpConfigSetEnabledParams),
    TestMcpConfig(McpConfigTestParams),
    ListToolConfig(ToolConfigListParams),
    SetToolConfigEnabled(ToolConfigSetEnabledParams),
    UpsertToolConfig(ToolConfigUpsertParams),
    DeleteToolConfig(ToolConfigDeleteParams),
    TestToolConfig(ToolConfigTestParams),
    AuthStatus(AuthStatusParams),
    AuthSendCode(AuthSendCodeParams),
    AuthVerify(AuthVerifyParams),
    AuthMe(AuthMeParams),
    AuthLogout(AuthLogoutParams),
    ProfileLocalCreate(ProfileLocalCreateParams),
    ProfileLlmCatalog(ProfileLlmCatalogParams),
    ProfileLlmList(ProfileLlmListParams),
    ProfileLlmUpsert(ProfileLlmUpsertParams),
    ProfileLlmDelete(ProfileLlmDeleteParams),
    ProfileLlmSelect(ProfileLlmSelectParams),
    ProfileLlmTest(ProfileLlmTestParams),
    ProfileLlmFetchModels(ProfileLlmFetchModelsParams),
    ProfileSkillsList(ProfileSkillsListParams),
    ProfileSkillsRegistrySearch(ProfileSkillsRegistrySearchParams),
    ProfileSkillsInstall(ProfileSkillsInstallParams),
    ProfileSkillsRemove(ProfileSkillsRemoveParams),
    // M15-E backend-owned autonomy surface (UPCR-2026-021).
    ListAgents(AgentListParams),
    ReadAgentStatus(AgentStatusReadParams),
    ReadAgentOutput(AgentOutputReadParams),
    ListAgentArtifacts(AgentArtifactListParams),
    ReadAgentArtifact(AgentArtifactReadParams),
    InterruptAgent(AgentInterruptParams),
    CloseAgent(AgentCloseParams),
    GetSessionGoal(SessionGoalGetParams),
    SetSessionGoal(SessionGoalSetParams),
    ClearSessionGoal(SessionGoalClearParams),
    CreateLoop(LoopCreateParams),
    ListLoops(LoopListParams),
    DeleteLoop(LoopIdParams),
    PauseLoop(LoopIdParams),
    ResumeLoop(LoopIdParams),
    FireLoopNow(LoopIdParams),
}

impl AppUiCommand {
    pub fn method(&self) -> &'static str {
        match self {
            Self::OpenSession(_) => octos_core::ui_protocol::methods::SESSION_OPEN,
            Self::SubmitPrompt(_) => octos_core::ui_protocol::methods::TURN_START,
            Self::InterruptTurn(_) => octos_core::ui_protocol::methods::TURN_INTERRUPT,
            Self::RespondApproval(_) => octos_core::ui_protocol::methods::APPROVAL_RESPOND,
            Self::ListApprovalScopes(_) => octos_core::ui_protocol::methods::APPROVAL_SCOPES_LIST,
            Self::GetDiffPreview(_) => octos_core::ui_protocol::methods::DIFF_PREVIEW_GET,
            Self::ListTasks(_) => octos_core::ui_protocol::methods::TASK_LIST,
            Self::CancelTask(_) => octos_core::ui_protocol::methods::TASK_CANCEL,
            Self::RestartTaskFromNode(_) => {
                octos_core::ui_protocol::methods::TASK_RESTART_FROM_NODE
            }
            Self::ReadTaskOutput(_) => octos_core::ui_protocol::methods::TASK_OUTPUT_READ,
            Self::ReadTaskArtifact(_) => APPUI_METHOD_TASK_ARTIFACT_READ,
            Self::GetThreadGraph(_) => APPUI_METHOD_THREAD_GRAPH_GET,
            Self::GetTurnState(_) => APPUI_METHOD_TURN_STATE_GET,
            Self::ListConfigCapabilities(_) => APPUI_METHOD_CONFIG_CAPABILITIES_LIST,
            Self::ReadSessionStatus(_) => APPUI_METHOD_SESSION_STATUS_READ,
            Self::ListModels(_) | Self::ProfileLlmList(_) => APPUI_METHOD_MODEL_LIST,
            Self::SelectModel(_) | Self::ProfileLlmSelect(_) => APPUI_METHOD_MODEL_SELECT,
            Self::ListPermissionProfiles(_) => {
                octos_core::ui_protocol::methods::PERMISSION_PROFILE_LIST
            }
            Self::SetPermissionProfile(_) => {
                octos_core::ui_protocol::methods::PERMISSION_PROFILE_SET
            }
            Self::ListMcpStatus(_) => APPUI_METHOD_MCP_STATUS_LIST,
            Self::ListToolStatus(_) => APPUI_METHOD_TOOL_STATUS_LIST,
            Self::ListMcpConfig(_) => APPUI_METHOD_MCP_CONFIG_LIST,
            Self::UpsertMcpConfig(_) => APPUI_METHOD_MCP_CONFIG_UPSERT,
            Self::DeleteMcpConfig(_) => APPUI_METHOD_MCP_CONFIG_DELETE,
            Self::SetMcpConfigEnabled(_) => APPUI_METHOD_MCP_CONFIG_SET_ENABLED,
            Self::TestMcpConfig(_) => APPUI_METHOD_MCP_CONFIG_TEST,
            Self::ListToolConfig(_) => APPUI_METHOD_TOOL_CONFIG_LIST,
            Self::SetToolConfigEnabled(_) => APPUI_METHOD_TOOL_CONFIG_SET_ENABLED,
            Self::UpsertToolConfig(_) => APPUI_METHOD_TOOL_CONFIG_UPSERT,
            Self::DeleteToolConfig(_) => APPUI_METHOD_TOOL_CONFIG_DELETE,
            Self::TestToolConfig(_) => APPUI_METHOD_TOOL_CONFIG_TEST,
            Self::AuthStatus(_) => APPUI_METHOD_AUTH_STATUS,
            Self::AuthSendCode(_) => APPUI_METHOD_AUTH_SEND_CODE,
            Self::AuthVerify(_) => APPUI_METHOD_AUTH_VERIFY,
            Self::AuthMe(_) => APPUI_METHOD_AUTH_ME,
            Self::AuthLogout(_) => APPUI_METHOD_AUTH_LOGOUT,
            Self::ProfileLocalCreate(_) => APPUI_METHOD_PROFILE_LOCAL_CREATE,
            Self::ProfileLlmCatalog(_) => APPUI_METHOD_PROFILE_LLM_CATALOG,
            Self::ProfileLlmUpsert(_) => APPUI_METHOD_PROFILE_LLM_UPSERT,
            Self::ProfileLlmDelete(_) => APPUI_METHOD_PROFILE_LLM_DELETE,
            Self::ProfileLlmTest(_) => APPUI_METHOD_PROFILE_LLM_TEST,
            Self::ProfileLlmFetchModels(_) => APPUI_METHOD_PROFILE_LLM_FETCH_MODELS,
            Self::ProfileSkillsList(_) => APPUI_METHOD_PROFILE_SKILLS_LIST,
            Self::ProfileSkillsRegistrySearch(_) => APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
            Self::ProfileSkillsInstall(_) => APPUI_METHOD_PROFILE_SKILLS_INSTALL,
            Self::ProfileSkillsRemove(_) => APPUI_METHOD_PROFILE_SKILLS_REMOVE,
            Self::ListAgents(_) => APPUI_METHOD_AGENT_LIST,
            Self::ReadAgentStatus(_) => APPUI_METHOD_AGENT_STATUS_READ,
            Self::ReadAgentOutput(_) => APPUI_METHOD_AGENT_OUTPUT_READ,
            Self::ListAgentArtifacts(_) => APPUI_METHOD_AGENT_ARTIFACT_LIST,
            Self::ReadAgentArtifact(_) => APPUI_METHOD_AGENT_ARTIFACT_READ,
            Self::InterruptAgent(_) => APPUI_METHOD_AGENT_INTERRUPT,
            Self::CloseAgent(_) => APPUI_METHOD_AGENT_CLOSE,
            Self::GetSessionGoal(_) => APPUI_METHOD_SESSION_GOAL_GET,
            Self::SetSessionGoal(_) => APPUI_METHOD_SESSION_GOAL_SET,
            Self::ClearSessionGoal(_) => APPUI_METHOD_SESSION_GOAL_CLEAR,
            Self::CreateLoop(_) => APPUI_METHOD_LOOP_CREATE,
            Self::ListLoops(_) => APPUI_METHOD_LOOP_LIST,
            Self::DeleteLoop(_) => APPUI_METHOD_LOOP_DELETE,
            Self::PauseLoop(_) => APPUI_METHOD_LOOP_PAUSE,
            Self::ResumeLoop(_) => APPUI_METHOD_LOOP_RESUME,
            Self::FireLoopNow(_) => APPUI_METHOD_LOOP_FIRE_NOW,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCapabilitiesListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCapabilitiesListResult {
    pub capabilities: UiProtocolCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusReadParams {
    pub session_id: SessionKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListParams {
    pub session_id: SessionKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelectParams {
    pub session_id: SessionKey,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelStatus {
    pub model: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default)]
    pub selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qoe_policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListResult {
    pub session_id: SessionKey,
    #[serde(default)]
    pub models: Vec<ModelStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelectResult {
    pub session_id: SessionKey,
    pub selected: ModelStatus,
    #[serde(default)]
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_policy_stamp: Option<RuntimePolicyStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusReadResult {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_policy_stamp: Option<RuntimePolicyStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<RuntimeHealthStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_summary: Option<McpStatusSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_summary: Option<ToolStatusSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsageStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<SessionCursorStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<UiProtocolCapabilities>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePolicyStamp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy_id: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<RuntimePolicyMcpServer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qoe_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_contract_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_contract_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_toolset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_tool_discovery: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RuntimePolicyMcpServer {
    Name(String),
    Detail {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_count: Option<u32>,
    },
}

impl RuntimePolicyMcpServer {
    pub fn name(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    pub fn label(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Detail {
                id,
                display_name,
                status,
                tool_count,
            } => {
                let name = display_name.as_deref().unwrap_or(id);
                match (status.as_deref(), tool_count) {
                    (Some(status), Some(tool_count)) => {
                        format!("{name} ({status}, {tool_count} tools)")
                    }
                    (Some(status), None) => format!("{name} ({status})"),
                    (None, Some(tool_count)) => format!("{name} ({tool_count} tools)"),
                    (None, None) => name.to_owned(),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealthStatus {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStatusSummary {
    pub connected: u32,
    pub connecting: u32,
    pub failed: u32,
    pub disabled: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatusSummary {
    pub visible: u32,
    pub enabled: u32,
    pub denied: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUsageStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_micros_usd: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCursorStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<octos_core::ui_protocol::UiCursor>,
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub replay_supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStatusListParams {
    pub session_id: SessionKey,
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStatusListResult {
    pub session_id: SessionKey,
    #[serde(default)]
    pub servers: Vec<McpStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStatus {
    pub server: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatusListParams {
    pub session_id: SessionKey,
    #[serde(default)]
    pub include_denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatusListResult {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_tool_contract: Option<CodingToolContract>,
    #[serde(default)]
    pub tools: Vec<ToolStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingToolContract {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub required_tool_names: Vec<String>,
    #[serde(default)]
    pub required_tools: Vec<CodingToolContractTool>,
    #[serde(default)]
    pub missing_required_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<CodingToolContractPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingToolContractPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingToolContractTool {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub capability: String,
    #[serde(default)]
    pub policy: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// M13-D supervised task list entry shape. This mirrors the server-emitted
/// `TaskListResult.tasks[*]` envelope so the TUI can deserialize the new
/// `source` / `role` / `summary` / `artifact_count` / `runtime_policy_stamp`
/// fields backend sibling shipped on `task/list` and `task/updated` payloads
/// without taking a hard dependency on the protocol type. The struct is
/// permissive (`#[serde(default)]` on optionals, unknown fields tolerated)
/// so the TUI never crashes when the server adds more inspection metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupervisedTaskEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_state: Option<String>,
    /// `"model"` (LLM-scheduled child), `"supervisor"` (backend-scheduled,
    /// e.g. review/start), or `"user"` (explicit user-driven). Used to
    /// indent / badge children under the parent request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Role label assigned at spawn time (e.g. `"reviewer"`,
    /// `"implementer"`). Pairs with M14-C role templates so the TUI can
    /// render "Reviewer running" instead of `task-xxx running`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Bounded summary capsule for the task (mirrors
    /// `ChildResultSummary.summary` for terminal children). Short text
    /// that clients render inline without fetching the full artifact list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Number of artifacts the child has emitted. Lets the UX badge tasks
    /// without resolving `task/artifact/list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_count: Option<u32>,
    /// Runtime policy stamp captured at spawn time. Reconnect hydration
    /// surfaces the same effective state the original `task/updated`
    /// notifications announced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_policy_stamp: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_key: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_terminal_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_join_state: Option<String>,
}

impl SupervisedTaskEntry {
    /// Returns `true` when this entry was scheduled by the LLM (model) or by
    /// a backend supervisor (review/start). User-scheduled tasks are not
    /// supervised children in the M13 sense.
    pub fn is_backend_supervised(&self) -> bool {
        matches!(self.source.as_deref(), Some("model") | Some("supervisor"))
    }

    /// Display label preferring role over tool name. Falls back to the
    /// tool name, then `"task"`. Never invents text that is not on the
    /// wire.
    pub fn display_label(&self) -> String {
        if let Some(role) = self.role.as_deref().filter(|r| !r.trim().is_empty()) {
            return role.to_string();
        }
        if let Some(tool) = self.tool_name.as_deref().filter(|t| !t.trim().is_empty()) {
            return tool.to_string();
        }
        "task".to_string()
    }
}

/// M13-D artifact summary returned by `task/artifact/list`. Permissive
/// (`Value` for `extra`-style fields) so the TUI keeps deserializing as the
/// backend adds richer metadata in later M13 milestones.
/// M16-G2 active context state snapshot. Carries hashes and counts
/// only — never raw transcript content, per spec §16. The TUI keeps
/// this in a bounded status surface; it is NOT appended to chat
/// history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLifecycleState {
    pub session_id: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub generation: u64,
    #[serde(default)]
    pub transcript_hash: String,
    #[serde(default)]
    pub item_count: usize,
    #[serde(default)]
    pub token_estimate: usize,
    /// `"healthy"`, `"recovering"`, `"degraded"` etc. — the backend is
    /// authoritative; the TUI just labels it.
    #[serde(default)]
    pub recovery_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_compaction_id: Option<String>,
}

/// M16-G2 last-compaction record summary (truncated). The full record
/// carries hashes and counts the TUI never renders to chat — only the
/// bounded labels reach the status surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompactionSummary {
    #[serde(default)]
    pub compaction_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub input_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_generation: Option<u64>,
    #[serde(default)]
    pub retained_count: usize,
    #[serde(default)]
    pub dropped_count: usize,
    #[serde(default)]
    pub token_estimate_before: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_estimate_after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// M16-G2 normalization report summary. Same containment policy as
/// compaction: counts only, never raw items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextNormalizationSummary {
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub model_capability_id: String,
    #[serde(default)]
    pub prompt_message_count: usize,
    #[serde(default)]
    pub token_estimate: usize,
    #[serde(default)]
    pub repaired_count: usize,
    #[serde(default)]
    pub dropped_count: usize,
    #[serde(default)]
    pub synthetic_count: usize,
    #[serde(default)]
    pub truncated_count: usize,
}

/// M16-G2 per-session lifecycle ledger. Holds the latest context
/// state plus the most recent compaction/normalization summaries. The
/// TUI renders these in a bounded status surface (NOT chat history).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionContextLifecycle {
    pub state: Option<ContextLifecycleState>,
    pub last_compaction: Option<ContextCompactionSummary>,
    pub last_normalization: Option<ContextNormalizationSummary>,
}

impl SessionContextLifecycle {
    /// Apply a `context/compaction_completed` notification.
    pub fn apply_compaction(
        &mut self,
        state: ContextLifecycleState,
        compaction: ContextCompactionSummary,
    ) {
        self.state = Some(state);
        self.last_compaction = Some(compaction);
    }

    /// Apply a `context/normalization_reported` notification.
    pub fn apply_normalization(
        &mut self,
        state: ContextLifecycleState,
        normalization: ContextNormalizationSummary,
    ) {
        self.state = Some(state);
        self.last_normalization = Some(normalization);
    }

    /// Bounded one-line summary suitable for the status surface. Empty
    /// when the server has not advertised lifecycle state yet (the TUI
    /// must hide the surface in that case rather than render zeros).
    pub fn summary_line(&self) -> Option<String> {
        let state = self.state.as_ref()?;
        let mut line = format!(
            "context gen={} items={} ~{} tok",
            state.generation, state.item_count, state.token_estimate
        );
        if !state.recovery_state.is_empty() && state.recovery_state != "healthy" {
            line.push_str(&format!(" ({})", state.recovery_state));
        }
        if let Some(compaction) = &self.last_compaction {
            line.push_str(&format!(
                " | compacted {}->{} retained={} dropped={}",
                compaction.input_generation,
                compaction
                    .output_generation
                    .map(|g| g.to_string())
                    .unwrap_or_else(|| "?".into()),
                compaction.retained_count,
                compaction.dropped_count,
            ));
        }
        Some(line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedTaskArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial: Option<ToolPolicyDenial>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicyDenial {
    pub code: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub recoverable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigUpsertParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub server: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigDeleteParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub server: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigSetEnabledParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub server: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigTestParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub server: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigListResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, alias = "mcp_servers", alias = "configs")]
    pub servers: Vec<McpConfigEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigEntry {
    #[serde(default, alias = "server", alias = "id")]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, alias = "url", skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigMutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub applied: bool,
    #[serde(
        default,
        alias = "id",
        alias = "deleted",
        skip_serializing_if = "Option::is_none"
    )]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<McpConfigEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigSetEnabledParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub tool: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigUpsertParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigDeleteParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigTestParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub tool: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigListResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, alias = "configs")]
    pub tools: Vec<ToolConfigEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigEntry {
    #[serde(default, alias = "name", alias = "id")]
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfigMutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub applied: bool,
    #[serde(
        default,
        alias = "name",
        alias = "deleted",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<ToolConfigEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStatusParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSendCodeParams {
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthVerifyParams {
    pub email: String,
    pub code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<AppUiAuthToken>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthLogoutParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<AppUiAuthToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStatusResult {
    #[serde(default)]
    pub bootstrap_mode: bool,
    #[serde(default)]
    pub email_login_enabled: bool,
    #[serde(default)]
    pub admin_token_login_enabled: bool,
    #[serde(default)]
    pub allow_self_registration: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_profile: Option<AuthScopedProfile>,
    #[serde(default)]
    pub authenticated: bool,
    #[serde(default)]
    pub email_otp: bool,
    #[serde(default)]
    pub token_login: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthScopedProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub email_login_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSendCodeResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppUiAuthToken(String);

impl AppUiAuthToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_for_transport(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AppUiAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"********\"")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthVerifyResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<AppUiAuthToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthMeResult {
    Dashboard {
        user: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<Value>,
        portal: Value,
    },
    Legacy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthLogoutResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLocalCreateParams {
    pub name: String,
    pub username: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLocalCreateResult {
    pub profile_id: String,
    pub user_id: String,
    pub name: String,
    pub username: String,
    pub email: String,
    #[serde(default)]
    pub created: bool,
    pub runtime_mode: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OnboardingAction {
    Open,
    OpenLogin,
    OpenProvider,
    SetName(String),
    SetUsername(String),
    SetEmail(String),
    SetOtpCode(String),
    SetProfileId(String),
    SetProviderSelection(LlmSelectionConfig),
    SetFamilyId(String),
    SetModelId(String),
    SetRouteId(String),
    SetRouteLabel(String),
    SetBaseUrl(String),
    SetApiKeyEnv(String),
    SetApiType(String),
    SetApiKey(SecretString),
    ClearApiKey,
    SendCode,
    VerifyCode,
    CreateLocalProfile,
    RefreshCatalog,
    RefreshProviders,
    FetchModels,
    SaveProvider,
    SaveProviderFallback,
    TestProvider,
    /// M22-C: stage a candidate workspace path.
    SetWorkspace(String),
    /// M22-C: probe the staged candidate (or the active
    /// `state.workspace.root` if no candidate) and update
    /// `workspace_validation`.
    ValidateWorkspace,
    /// M22-C: clear staged candidate and reset validation status.
    ResetWorkspace,
    /// M22-D: stage a permission-profile update to apply after the
    /// first session opens. `None` clears the staged choice.
    StagePermissionProfile(Option<octos_core::ui_protocol::PermissionProfileUpdate>),
    /// M22-F: render the doctor report (pass/warn/fail/skip per
    /// onboarding category) in the status line and as an
    /// activity entry.
    Doctor,
    Finish,
    Reset,
}

/// M22-F: outcome of a single doctor check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingDoctorOutcome {
    /// Check passed; `detail` carries a short summary.
    Pass { detail: String },
    /// Check is recoverable; `recovery` names the user action.
    Warn { reason: String, recovery: String },
    /// Check failed; `recovery` names the user action.
    Fail { reason: String, recovery: String },
    /// Check could not run (capability missing); `detail` names
    /// the unsupported method.
    Skipped { detail: String },
}

impl OnboardingDoctorOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass { .. })
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pass { .. } => "PASS",
            Self::Warn { .. } => "WARN",
            Self::Fail { .. } => "FAIL",
            Self::Skipped { .. } => "SKIP",
        }
    }
}

/// M22-F: a single doctor check row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingDoctorCheck {
    pub id: &'static str,
    pub title: &'static str,
    pub outcome: OnboardingDoctorOutcome,
}

/// M22-F: aggregated doctor report. The wizard owns the
/// aggregation so the doctor surface is just a typed projection
/// of existing state — there is no new mutable repair step,
/// only typed recovery copy that points at the existing
/// `/onboard <step>` actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingDoctorReport {
    pub checks: Vec<OnboardingDoctorCheck>,
}

impl OnboardingDoctorReport {
    pub fn any_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|check| matches!(check.outcome, OnboardingDoctorOutcome::Fail { .. }))
    }
    pub fn any_warnings(&self) -> bool {
        self.checks
            .iter()
            .any(|check| matches!(check.outcome, OnboardingDoctorOutcome::Warn { .. }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingProviderPending {
    Test,
    Save,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingProviderSaveTarget {
    Primary,
    Fallback,
}

/// M22-E: product-grade lifecycle status for the provider setup
/// step. Computed from existing fields (`selection_ready`,
/// `has_api_key`, `provider_pending`, `provider_tested`,
/// `provider_saved`, `last_saved_provider_target`) so we do NOT
/// introduce a separate state machine. The variants map directly
/// to the menu rows and status-bar copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingProviderStatus {
    /// No family/model/route selected yet.
    NotSelected,
    /// Family/model/route chosen but no API key staged.
    KeyMissing,
    /// `profile/llm/test` in flight.
    Testing,
    /// Last test failed; reason is the server message.
    TestFailed { reason: String },
    /// `profile/llm/upsert` in flight (primary or fallback).
    Saving(OnboardingProviderSaveTarget),
    /// Saved primary provider — finish is unlocked.
    SavedPrimary,
    /// Saved as a fallback only — primary save is still needed
    /// before finish.
    SavedFallback,
    /// Selection + key staged, ready to test/save.
    Ready,
}

/// M22-C: workspace validation status for the onboarding step.
/// Backend-owned workspace/probe methods are not yet wired (see
/// the contract slice-0 note), so the TUI does its own client-side
/// probe and flags the result so `session/open` is only invoked
/// once we have a `Valid` status. When the backend adds a workspace-
/// probe RPC this enum stays the same — only the producer of the
/// status changes from client-side to RPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingWorkspaceValidation {
    /// No candidate has been staged or validated yet.
    Unvalidated,
    /// Probe is in flight (reserved for the future RPC path).
    Validating,
    /// Path exists, is a directory, and meets the policy preview.
    Valid {
        canonical: String,
        writable: bool,
        has_workspace_toml: bool,
    },
    /// Probe failed. The user must address `reason` before finish.
    Invalid { reason: String },
}

impl OnboardingWorkspaceValidation {
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    pub fn is_unvalidated(&self) -> bool {
        matches!(self, Self::Unvalidated)
    }
}

/// M22-B local-profile recovery state. Set when `profile/local/create`
/// fails or pre-flight validation rejects the staged owner. The wizard
/// renders this as the focused field plus a typed recovery message so
/// the user is not shoved out of the profile step on a generic error
/// status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingLocalProfileRecovery {
    pub kind: OnboardingLocalProfileErrorKind,
    pub focus_field: OnboardingLocalProfileField,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingLocalProfileErrorKind {
    /// Backend rejected the username with `profile_local_collision`.
    Collision,
    /// Backend does not advertise `profile/local/create`
    /// (`profile_local_unsupported`).
    Unsupported,
    /// Server-side `invalid_params` rejected a staged field.
    InvalidParams,
    /// Pre-flight client-side validation rejected a field before any
    /// RPC was issued.
    InvalidField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingLocalProfileField {
    Name,
    Username,
    Email,
}

impl OnboardingLocalProfileField {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Username => "username",
            Self::Email => "email",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnboardingWizardState {
    pub name: String,
    pub username: String,
    pub email: String,
    pub otp_code: String,
    pub profile_id: Option<String>,
    pub local_profile_created: bool,
    pub open_session_after_profile_create: bool,
    /// M22-C: workspace candidate the user has staged via
    /// `/onboard workspace <path>`. `None` means the active
    /// `state.workspace.root` is used. Held separately from
    /// `state.workspace` so the candidate can be probed and either
    /// accepted (replacing `workspace.root`) or rejected without
    /// mutating the active workspace pane.
    pub workspace_candidate: Option<String>,
    /// M22-C: result of the most recent workspace probe. Defaults
    /// to `Unvalidated`. `onboarding_finish_command` refuses to
    /// emit `session/open` unless this is `Valid`.
    pub workspace_validation: OnboardingWorkspaceValidation,
    /// M22-D: permission profile the user has staged for the first
    /// session. Held in the wizard so the choice renders before the
    /// session opens, without claiming the policy is yet effective.
    /// After `session/open` succeeds and `permission/profile/set`
    /// is supported, the store sends the staged update; the
    /// server's runtime policy stamp is the final authority.
    pub staged_permission_profile: Option<octos_core::ui_protocol::PermissionProfileUpdate>,
    /// M22-D: human-readable mismatch reason when the runtime
    /// policy stamp diverges from the staged permission profile
    /// (server clamped or rejected the user's choice). `None`
    /// while no mismatch has been observed.
    pub permission_profile_mismatch: Option<String>,
    /// M22-B: true while a `profile/local/create` RPC is in flight.
    /// Lets `AppUiEvent::Error` attribute typed onboarding errors back
    /// to the profile step without inspecting error message strings.
    pub local_profile_create_pending: bool,
    /// M22-B: username captured at the moment `profile/local/create`
    /// was submitted. The collision recovery message uses THIS value
    /// so a late server error never claims the freshly-edited staged
    /// username was the one rejected. `None` when no create RPC has
    /// been submitted (or after a success/failure has cleared it).
    pub local_profile_create_pending_username: Option<String>,
    /// M22-B: typed recovery for the local-profile step. `None` when
    /// the step is clean; populated by server `profile_local_*` errors
    /// or client-side validation.
    pub local_profile_recovery: Option<OnboardingLocalProfileRecovery>,
    pub auth_email_enabled: Option<bool>,
    pub auth_code_sent: bool,
    pub auth_verified: bool,
    pub auth_token: Option<AppUiAuthToken>,
    pub provider: LlmSelectionConfig,
    pub api_key: Option<SecretString>,
    pub provider_saved: bool,
    pub provider_tested: bool,
    pub provider_pending: Option<OnboardingProviderPending>,
    pub provider_save_target: Option<OnboardingProviderSaveTarget>,
    pub last_saved_provider_label: Option<String>,
    pub last_saved_provider_target: Option<OnboardingProviderSaveTarget>,
    pub saved_primary_provider_label: Option<String>,
    /// M22-E: typed failure reason for the most recent
    /// `profile/llm/test`. Populated when the test resolves with
    /// `ok = false`; cleared on a successful test, re-selection,
    /// or save. Used by `provider_status` to render the
    /// `TestFailed` variant with the server reason and by the menu
    /// to surface a recovery message.
    pub provider_test_failure_reason: Option<String>,
    pub last_message: Option<String>,
}

impl Default for OnboardingWizardState {
    fn default() -> Self {
        Self {
            name: String::new(),
            username: String::new(),
            email: String::new(),
            otp_code: String::new(),
            profile_id: None,
            local_profile_created: false,
            open_session_after_profile_create: false,
            workspace_candidate: None,
            workspace_validation: OnboardingWorkspaceValidation::Unvalidated,
            staged_permission_profile: None,
            permission_profile_mismatch: None,
            local_profile_create_pending: false,
            local_profile_create_pending_username: None,
            local_profile_recovery: None,
            auth_email_enabled: None,
            auth_code_sent: false,
            auth_verified: false,
            auth_token: None,
            provider: empty_llm_selection_config(),
            api_key: None,
            provider_saved: false,
            provider_tested: false,
            provider_pending: None,
            provider_save_target: None,
            last_saved_provider_label: None,
            last_saved_provider_target: None,
            saved_primary_provider_label: None,
            provider_test_failure_reason: None,
            last_message: None,
        }
    }
}

fn empty_llm_selection_config() -> LlmSelectionConfig {
    LlmSelectionConfig {
        family_id: String::new(),
        model_id: String::new(),
        route: LlmRouteConfig {
            route_id: String::new(),
            label: None,
            base_url: None,
            api_key_env: None,
            api_type: Some("openai".into()),
        },
        ..LlmSelectionConfig::default()
    }
}

impl OnboardingWizardState {
    pub fn effective_profile_id(&self, current_profile: Option<&str>) -> Option<String> {
        self.profile_id
            .as_deref()
            .filter(|profile| !profile.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| current_profile.map(str::to_owned))
    }

    pub fn has_email(&self) -> bool {
        !self.email.trim().is_empty()
    }

    pub fn has_name(&self) -> bool {
        !self.name.trim().is_empty()
    }

    pub fn has_username(&self) -> bool {
        !self.username.trim().is_empty()
    }

    pub fn local_profile_ready(&self) -> bool {
        // The contract calls email "optional metadata" for solo
        // mode, but the current backend implementation of
        // `profile/local/create` still validates `email` as
        // non-empty and rejects `""` with
        // `profile_local_invalid_email`. Until the backend relaxes
        // that, the TUI must keep email required so the menu does
        // not invite the user into a guaranteed-failure submission.
        self.has_name() && self.has_username() && self.has_email()
    }

    /// M22-C: the path the user wants to use for the session. Falls
    /// back to the active workspace root when no candidate has been
    /// staged so the wizard can re-validate a previously-accepted
    /// workspace.
    pub fn workspace_target<'a>(&'a self, active_workspace: &'a str) -> &'a str {
        self.workspace_candidate
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| active_workspace.trim())
    }

    /// M22-C: only valid workspaces unlock `session/open`. Pre-
    /// flight validation produces this status; the contract slice
    /// 7 requires it.
    pub fn workspace_ready_for_finish(&self) -> bool {
        self.workspace_validation.is_valid()
    }

    pub fn has_otp_code(&self) -> bool {
        !self.otp_code.trim().is_empty()
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.as_ref().is_some_and(|key| !key.is_empty())
    }

    pub fn selection_ready(&self) -> bool {
        !self.provider.family_id.trim().is_empty()
            && !self.provider.model_id.trim().is_empty()
            && (!self.provider.route.route_id.trim().is_empty()
                || self
                    .provider
                    .route
                    .base_url
                    .as_deref()
                    .is_some_and(|url| !url.trim().is_empty()))
    }

    pub fn profile_label(&self, current_profile: Option<&str>) -> String {
        self.effective_profile_id(current_profile)
            .unwrap_or_else(|| "<server authenticated profile>".into())
    }

    pub fn provider_label(&self) -> String {
        if self.selection_ready() {
            format!(
                "{} / {} via {}",
                self.provider.family_id, self.provider.model_id, self.provider.route.route_id
            )
        } else {
            "not selected".into()
        }
    }

    pub fn api_key_label(&self) -> &'static str {
        self.api_key
            .as_ref()
            .map(SecretString::masked)
            .unwrap_or("")
    }

    /// M22-E: compute the product-grade provider lifecycle status
    /// from the existing wizard fields. Order of checks is
    /// deliberate:
    ///
    /// 1. Pending operations win (Testing/Saving).
    /// 2. Test failures win over post-save states — the user must
    ///    react before continuing.
    /// 3. Post-save states win over pre-save selection checks
    ///    because a successful fallback save resets the staged
    ///    selection. Without this order a fallback save would
    ///    report `NotSelected` and the menu could not
    ///    distinguish fallback-only state from "nothing chosen".
    /// 4. Pre-save selection/key checks for the unsaved path.
    pub fn provider_status(&self) -> OnboardingProviderStatus {
        if let Some(pending) = self.provider_pending {
            return match pending {
                OnboardingProviderPending::Test => OnboardingProviderStatus::Testing,
                OnboardingProviderPending::Save => OnboardingProviderStatus::Saving(
                    self.provider_save_target
                        .unwrap_or(OnboardingProviderSaveTarget::Primary),
                ),
            };
        }
        if let Some(reason) = self.provider_test_failure_reason.as_deref() {
            return OnboardingProviderStatus::TestFailed {
                reason: reason.to_owned(),
            };
        }
        // Saved-state check must run BEFORE selection/key checks
        // because a successful fallback save resets staged input
        // (see `apply_profile_llm_mutation_event` in store.rs).
        if self.provider_saved
            || matches!(
                self.last_saved_provider_target,
                Some(OnboardingProviderSaveTarget::Fallback)
            )
        {
            return match self.last_saved_provider_target {
                Some(OnboardingProviderSaveTarget::Fallback) => {
                    OnboardingProviderStatus::SavedFallback
                }
                Some(OnboardingProviderSaveTarget::Primary) | None => {
                    OnboardingProviderStatus::SavedPrimary
                }
            };
        }
        if !self.selection_ready() {
            return OnboardingProviderStatus::NotSelected;
        }
        if !self.has_api_key() {
            return OnboardingProviderStatus::KeyMissing;
        }
        OnboardingProviderStatus::Ready
    }

    pub fn apply_selection(&mut self, selection: LlmSelectionConfig) {
        self.provider = selection;
        self.provider_tested = false;
        self.provider_pending = None;
        self.provider_save_target = None;
        // M22-E: a fresh selection invalidates the last test
        // failure — the user is implicitly retrying.
        self.provider_test_failure_reason = None;
        self.last_message = Some("Provider selection updated from AppUI catalog".into());
    }

    pub fn reset_staged_provider(&mut self) {
        self.provider = empty_llm_selection_config();
        self.api_key = None;
        self.provider_tested = false;
        self.provider_pending = None;
        self.provider_save_target = None;
    }

    pub fn build_upsert_params(
        &self,
        current_profile: Option<&str>,
    ) -> Option<ProfileLlmUpsertParams> {
        self.build_upsert_params_with_primary(current_profile, true)
    }

    pub fn build_fallback_upsert_params(
        &self,
        current_profile: Option<&str>,
    ) -> Option<ProfileLlmUpsertParams> {
        self.build_upsert_params_with_primary(current_profile, false)
    }

    fn build_upsert_params_with_primary(
        &self,
        current_profile: Option<&str>,
        set_primary: bool,
    ) -> Option<ProfileLlmUpsertParams> {
        self.selection_ready().then(|| ProfileLlmUpsertParams {
            profile_id: self.effective_profile_id(current_profile),
            selection: self.provider.clone(),
            api_key: self.api_key.clone(),
            set_primary,
        })
    }

    pub fn build_test_params(&self, current_profile: Option<&str>) -> Option<ProfileLlmTestParams> {
        self.selection_ready().then(|| ProfileLlmTestParams {
            profile_id: self.effective_profile_id(current_profile),
            selection: self.provider.clone(),
            api_key: self.api_key.clone(),
        })
    }

    pub fn build_fetch_models_params(
        &self,
        current_profile: Option<&str>,
    ) -> Option<ProfileLlmFetchModelsParams> {
        self.selection_ready().then(|| ProfileLlmFetchModelsParams {
            profile_id: self.effective_profile_id(current_profile),
            selection: self.provider.clone(),
            api_key: self.api_key.clone(),
        })
    }

    pub fn apply_auth_status(&mut self, result: &AuthStatusResult) {
        self.auth_email_enabled = Some(result.email_login_enabled || result.email_otp);
        self.auth_verified = result.authenticated || result.scoped_profile.is_some();
        if let Some(profile) = result.scoped_profile.as_ref() {
            self.profile_id = Some(profile.id.clone());
        } else if let Some(profile_id) = result.profile_id.as_ref() {
            self.profile_id = Some(profile_id.clone());
        }
    }

    pub fn apply_auth_verify(&mut self, result: &AuthVerifyResult) {
        self.auth_verified = result.ok;
        if let Some(token) = result.token.clone() {
            self.auth_token = Some(token);
        }
    }

    pub fn apply_auth_me(&mut self, result: &AuthMeResult) {
        if let Some(profile_id) = auth_me_profile_id(result) {
            self.profile_id = Some(profile_id.to_owned());
            self.auth_verified = true;
        }
    }

    pub fn apply_profile_local_create(&mut self, result: &ProfileLocalCreateResult) {
        self.profile_id = Some(result.profile_id.clone());
        self.name = result.name.clone();
        self.username = result.username.clone();
        self.email = result.email.clone();
        self.local_profile_created = true;
        self.auth_verified = true;
        self.local_profile_create_pending = false;
        self.local_profile_create_pending_username = None;
        self.local_profile_recovery = None;
    }

    /// M22-B: pre-flight validation for the local profile step.
    /// Returns the first failing field with a typed recovery so the
    /// TUI never spends a `profile/local/create` round-trip on an
    /// obviously bad shape (empty fields, malformed email).
    ///
    /// Validation rules:
    /// - Name: non-empty, max 128 chars after trim.
    /// - Username: non-empty, max 64 chars, ASCII printable without
    ///   spaces (so it is shell- and path-safe).
    /// - Email: non-empty when supplied; must contain `@` with a non-
    ///   empty local and domain part. Empty email is allowed because
    ///   email is local metadata for the solo-mode profile (the
    ///   contract calls it optional).
    pub fn validate_local_profile(&self) -> Result<(), OnboardingLocalProfileRecovery> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Name,
                message: "Display name is required. Use /onboard name <display name>.".into(),
            });
        }
        if name.chars().count() > 128 {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Name,
                message: "Display name must be 128 characters or fewer.".into(),
            });
        }

        let username = self.username.trim();
        if username.is_empty() {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Username,
                message: "Username is required. Use /onboard username <handle>.".into(),
            });
        }
        if username.len() > 64 {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Username,
                message: "Username must be 64 characters or fewer.".into(),
            });
        }
        if username
            .chars()
            .any(|c| !c.is_ascii() || c.is_ascii_whitespace() || c.is_ascii_control())
        {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Username,
                message: "Username must be ASCII without whitespace or control characters.".into(),
            });
        }

        let email = self.email.trim();
        if email.is_empty() {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Email,
                message: "Email is required by the backend. Use /onboard email <address>.".into(),
            });
        }
        if !looks_like_email(email) {
            return Err(OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidField,
                focus_field: OnboardingLocalProfileField::Email,
                message:
                    "Email must contain a non-empty local-part and domain (e.g. ada@example.com)."
                        .into(),
            });
        }

        Ok(())
    }

    /// M22-B: apply a typed error returned from a pending
    /// `profile/local/create` request. The caller (the store error
    /// handler) is responsible for recognizing the structured code;
    /// this routine decides which field to focus and what recovery
    /// text to display so the user stays on the profile step.
    pub fn apply_local_profile_error(&mut self, code: &str, message: &str) {
        // Prefer the pending-username snapshot captured at submit
        // time so a late error never claims the freshly-edited staged
        // username was the one rejected.
        let collided_username = self
            .local_profile_create_pending_username
            .clone()
            .unwrap_or_else(|| self.username.clone());
        // The backend prepends the failing method as a prefix on
        // method-attributed responses
        // (`profile/local/create request tui-N failed: <reason>`).
        // Strip it so the user does not see the wire protocol leaking
        // into the recovery copy.
        let server_reason = strip_method_prefix(message, "profile/local/create");
        let recovery = match code {
            "profile_local_collision" => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::Collision,
                focus_field: OnboardingLocalProfileField::Username,
                // The backend uses `profile_local_collision` for any
                // existing-owner collision (username, email metadata,
                // or owner id), with the reason in the message. Keep
                // that reason rather than hard-coding "username taken".
                message: format!(
                    "Local profile collision for '{collided_username}': {server_reason}. Edit the fields with /onboard name|username|email and try again."
                ),
            },
            "profile_local_unsupported" => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::Unsupported,
                focus_field: OnboardingLocalProfileField::Username,
                // We do NOT suggest `/login` here because the registry
                // hides OTP slash commands while `profile/local/create`
                // is advertised by the capability set. A backend
                // returning `profile_local_unsupported` despite
                // advertising the method is misconfigured, not a
                // signal that the user can fall back to OTP locally.
                message: "This server returned profile_local_unsupported for profile/local/create. The backend is misconfigured — restart the server with local solo onboarding enabled, or connect to a backend that fully supports it."
                    .into(),
            },
            "profile_local_invalid_name" => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidParams,
                focus_field: OnboardingLocalProfileField::Name,
                message: format!(
                    "Server rejected the display name: {server_reason}. Edit it with /onboard name <display name>."
                ),
            },
            "profile_local_invalid_username" => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidParams,
                focus_field: OnboardingLocalProfileField::Username,
                message: format!(
                    "Server rejected the username: {server_reason}. Edit it with /onboard username <handle>."
                ),
            },
            "profile_local_invalid_email" => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidParams,
                focus_field: OnboardingLocalProfileField::Email,
                message: format!(
                    "Server rejected the email: {server_reason}. Edit it with /onboard email <address>."
                ),
            },
            "invalid_params" => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidParams,
                // Without more granular server data we cannot know
                // which field is at fault; default to username because
                // collision is the highest-prior real-world cause.
                focus_field: OnboardingLocalProfileField::Username,
                message: format!(
                    "Server rejected the profile fields as invalid: {server_reason}. Edit them with /onboard name|username|email."
                ),
            },
            _ => OnboardingLocalProfileRecovery {
                kind: OnboardingLocalProfileErrorKind::InvalidParams,
                focus_field: OnboardingLocalProfileField::Username,
                message: format!("profile/local/create failed: {server_reason}"),
            },
        };
        self.local_profile_create_pending = false;
        self.local_profile_create_pending_username = None;
        self.local_profile_created = false;
        self.local_profile_recovery = Some(recovery);
    }

    /// M22-B: clear local-profile recovery state after the user edits
    /// the offending field. Called from the field setter so the typed
    /// recovery does not linger after the user acts on it.
    ///
    /// The pending-create snapshot (`local_profile_create_pending_username`)
    /// is intentionally NOT cleared here: a late server response for
    /// the in-flight create must continue to render the recovery
    /// against the username that was actually submitted, not the
    /// freshly-edited value. The snapshot is only cleared by the
    /// next create dispatch (replaced with the new value) or by the
    /// success/error response handlers in `apply_profile_local_create`
    /// and `apply_local_profile_error`.
    pub fn clear_local_profile_recovery(&mut self) {
        self.local_profile_recovery = None;
    }
}

/// M22-B: strip the wire-level method prefix that
/// `error_response_to_app_event` prepends to method-attributed
/// failures (`"<method> request <id> failed: <reason>"`). The
/// recovery copy then renders just the server reason, not the raw
/// JSON-RPC wire format.
fn strip_method_prefix(message: &str, method: &str) -> String {
    let prefix = format!("{method} request");
    if let Some(rest) = message.strip_prefix(&prefix) {
        if let Some((_, reason)) = rest.split_once(": ") {
            return reason.trim().to_owned();
        }
    }
    message.to_owned()
}

/// Cheap shape-only check: requires `local@domain` with non-empty
/// parts. Single-label domains (`ada@localhost`, `dev@corp`) are
/// allowed because the backend's `profile/local/create` accepts
/// them — the TUI must not be stricter than the server. The backend
/// remains the source of truth for full RFC validation.
fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.trim().is_empty()
        && !domain.trim().is_empty()
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

pub fn auth_me_email(result: &AuthMeResult) -> Option<&str> {
    match result {
        AuthMeResult::Dashboard { user, .. } => user.get("email").and_then(Value::as_str),
        AuthMeResult::Legacy { email, .. } => email.as_deref(),
    }
}

pub fn auth_me_profile_id(result: &AuthMeResult) -> Option<&str> {
    match result {
        AuthMeResult::Dashboard { profile, user, .. } => profile
            .as_ref()
            .and_then(|profile| {
                profile
                    .get("profile")
                    .and_then(|profile| profile.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| profile.get("id").and_then(Value::as_str))
            })
            .or_else(|| user.get("profile_id").and_then(Value::as_str)),
        AuthMeResult::Legacy { profile_id, .. } => profile_id.as_deref(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLlmCatalogParams {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLlmListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRouteConfig {
    #[serde(
        default,
        deserialize_with = "string_or_default",
        skip_serializing_if = "is_empty_string"
    )]
    pub route_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_type: Option<String>,
}

impl Default for LlmRouteConfig {
    fn default() -> Self {
        Self {
            route_id: String::new(),
            label: None,
            base_url: None,
            api_key_env: None,
            api_type: None,
        }
    }
}

impl LlmRouteConfig {
    pub fn is_empty(&self) -> bool {
        self.route_id.trim().is_empty()
            && self.label.as_deref().is_none_or(str::is_empty)
            && self.base_url.as_deref().is_none_or(str::is_empty)
            && self.api_key_env.as_deref().is_none_or(str::is_empty)
            && self.api_type.as_deref().is_none_or(str::is_empty)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmSelectionConfig {
    #[serde(
        default,
        deserialize_with = "string_or_default",
        skip_serializing_if = "is_empty_string"
    )]
    pub family_id: String,
    #[serde(
        default,
        deserialize_with = "string_or_default",
        skip_serializing_if = "is_empty_string"
    )]
    pub model_id: String,
    #[serde(
        default,
        deserialize_with = "route_or_default",
        skip_serializing_if = "LlmRouteConfig::is_empty"
    )]
    pub route: LlmRouteConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_hints: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_per_m: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong: Option<bool>,
}

impl Default for LlmSelectionConfig {
    fn default() -> Self {
        Self {
            family_id: String::new(),
            model_id: String::new(),
            route: LlmRouteConfig::default(),
            model_hints: None,
            cost_per_m: None,
            strong: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmUpsertParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub selection: LlmSelectionConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretString>,
    #[serde(default)]
    pub set_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmDeleteParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub family_id: String,
    pub model_id: String,
    pub route_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmSelectParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub family_id: String,
    pub model_id: String,
    pub route_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmTestParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub selection: LlmSelectionConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmFetchModelsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub selection: LlmSelectionConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretString>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileLlmCatalogResult {
    #[serde(default)]
    pub families: serde_json::Map<String, Value>,
}

impl<'de> Deserialize<'de> for ProfileLlmCatalogResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let families = match value {
            Value::Object(mut object) => object
                .remove("families")
                .and_then(|families| match families {
                    Value::Object(families) => Some(families),
                    _ => None,
                })
                .unwrap_or(object),
            _ => serde_json::Map::new(),
        };
        Ok(Self { families })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfiguredProvider {
    #[serde(default, skip_serializing)]
    pub provider: String,
    #[serde(default, skip_serializing)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<LlmRouteConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_hints: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_per_m: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong: Option<bool>,
}

impl LlmConfiguredProvider {
    pub fn to_model_status(&self) -> ModelStatus {
        let provider = non_empty(self.provider.clone())
            .or_else(|| self.family_id.clone())
            .unwrap_or_else(|| "unknown".into());
        let model = non_empty(self.model.clone())
            .or_else(|| self.model_id.clone())
            .unwrap_or_else(|| "unknown".into());
        let route = self.route_id.clone().or_else(|| {
            self.route
                .as_ref()
                .and_then(|route| non_empty(route.route_id.clone()))
        });
        ModelStatus {
            model: self.model_id.clone().unwrap_or_else(|| model.clone()),
            provider: provider.clone(),
            title: Some(format!("{provider} / {model}")),
            family: self.family_id.clone(),
            route,
            selected: self.selected,
            available: self.available,
            queue_mode: None,
            qoe_policy: None,
        }
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmListResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<LlmConfiguredProvider>,
    #[serde(default)]
    pub fallbacks: Vec<LlmConfiguredProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmProfileState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_policy_stamp: Option<RuntimePolicyStamp>,
}

impl ProfileLlmListResult {
    pub fn primary_provider(&self) -> Option<&LlmConfiguredProvider> {
        self.primary
            .as_ref()
            .or_else(|| self.llm.as_ref().and_then(|llm| llm.primary.as_ref()))
    }

    pub fn fallback_providers(&self) -> &[LlmConfiguredProvider] {
        if self.fallbacks.is_empty() {
            self.llm
                .as_ref()
                .map(|llm| llm.fallbacks.as_slice())
                .unwrap_or_default()
        } else {
            self.fallbacks.as_slice()
        }
    }

    pub fn models(&self) -> Vec<ModelStatus> {
        self.primary_provider()
            .into_iter()
            .chain(self.fallback_providers().iter())
            .map(LlmConfiguredProvider::to_model_status)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmProfileState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<LlmConfiguredProvider>,
    #[serde(default)]
    pub fallbacks: Vec<LlmConfiguredProvider>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLlmMutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<LlmConfiguredProvider>,
    #[serde(default)]
    pub fallbacks: Vec<LlmConfiguredProvider>,
    #[serde(default)]
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmProfileState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_policy_stamp: Option<RuntimePolicyStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProfileLlmMutationResult {
    pub fn to_list_result(&self) -> ProfileLlmListResult {
        ProfileLlmListResult {
            profile_id: self.profile_id.clone(),
            primary: self.primary.clone(),
            fallbacks: self.fallbacks.clone(),
            llm: self.llm.clone(),
            runtime_policy_stamp: self.runtime_policy_stamp.clone(),
        }
    }

    pub fn models(&self) -> Vec<ModelStatus> {
        self.to_list_result().models()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsRegistrySearchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsInstallParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsRemoveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub tool_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_repo: Option<String>,
    #[serde(default)]
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsListResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub skills: Vec<ProfileSkillEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillRegistryPackage {
    pub name: String,
    pub description: String,
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub provides_tools: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub installed_skills: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsRegistrySearchResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub packages: Vec<ProfileSkillRegistryPackage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSkillsMutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub installed: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<String>,
    #[serde(default)]
    pub deps_installed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Sessions,
    Tasks,
    Artifacts,
    Transcript,
    Workspace,
    Git,
    Composer,
}

impl FocusPane {
    pub fn next(self) -> Self {
        match self {
            Self::Sessions => Self::Tasks,
            Self::Tasks => Self::Artifacts,
            Self::Artifacts => Self::Transcript,
            Self::Transcript => Self::Workspace,
            Self::Workspace => Self::Git,
            Self::Git => Self::Composer,
            Self::Composer => Self::Sessions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRunState {
    Idle,
    InProgress,
    Blocked { message: String },
    Success,
    Error { message: String },
}

impl SessionRunState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::InProgress => "running",
            Self::Blocked { .. } => "blocked",
            Self::Success => "done",
            Self::Error { .. } => "error",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Blocked { message } | Self::Error { message } => Some(message.as_str()),
            Self::Idle | Self::InProgress | Self::Success => None,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::InProgress | Self::Blocked { .. })
    }
}

impl Default for SessionRunState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub sessions: Vec<SessionView>,
    pub selected_session: usize,
    pub selected_task: usize,
    pub transcript_scroll: usize,
    pub focus: FocusPane,
    pub artifacts: ArtifactPaneState,
    pub workspace: WorkspacePaneState,
    pub git: GitPaneState,
    pub composer: String,
    pub composer_cursor: Option<usize>,
    pub composer_drafts: Vec<ComposerDraft>,
    pub pending_messages: Vec<String>,
    pub optimistic_user_messages: Vec<OptimisticUserMessage>,
    pub status: String,
    pub target: Option<String>,
    pub readonly: bool,
    pub protocol_version: &'static str,
    pub run_state: SessionRunState,
    pub run_state_started_at: Option<Instant>,
    pub approval_auto_open: bool,
    pub approval: Option<ApprovalModalState>,
    pub task_output: TaskOutputDetailState,
    pub artifact_detail: ArtifactDetailState,
    pub thread_graph_detail: ThreadGraphDetailState,
    pub turn_state_detail: TurnStateDetailState,
    pub task_output_cursors: Vec<TaskOutputCursor>,
    pub diff_preview: DiffPreviewPaneState,
    pub activity: Vec<ActivityItem>,
    pub turn_activity_logs: Vec<TurnActivityLog>,
    pub expanded_tool_outputs: bool,
    pub menu_stack: MenuStack,
    pub active_menu: Option<MenuBuildResult>,
    pub capabilities: Option<CapabilitySet>,
    pub onboarding: OnboardingWizardState,
    pub permission_profiles: Vec<SessionPermissionProfile>,
    pub session_runtime_statuses: Vec<SessionRuntimeStatus>,
    pub profile_llm_catalog: Option<ProfileLlmCatalogResult>,
    pub profile_llm_state: Option<ProfileLlmListResult>,
    pub profile_skills: Option<ProfileSkillsListResult>,
    pub profile_skill_registry: Option<ProfileSkillsRegistrySearchResult>,
    pub session_model_catalogs: Vec<SessionModelCatalog>,
    pub session_mcp_catalogs: Vec<SessionMcpCatalog>,
    pub session_tool_catalogs: Vec<SessionToolCatalog>,
    pub mcp_config_catalog: Option<McpConfigListResult>,
    pub tool_config_catalog: Option<ToolConfigListResult>,
    /// M16-G2 per-session compact-context lifecycle ledger. Keyed by
    /// session id. Empty when the server has not advertised
    /// [`APPUI_FEATURE_CONTEXT_LIFECYCLE_V1`] or sent any
    /// `context/compaction_completed` / `context/normalization_reported`
    /// notification yet — the TUI hides the status surface in that case
    /// instead of rendering zeroes.
    pub context_lifecycle: Vec<SessionContextLifecycleEntry>,
    /// M15-E per-session autonomy mirror. Populated by `agent/list`,
    /// `session/goal/get`, `loop/list` results and by the matching
    /// notifications. Hydration on reconnect re-requests these and
    /// REPLACES the local mirror — local config never fills this in.
    pub session_autonomy: Vec<SessionAutonomyState>,
    /// M15-E reconnect hydration queue. The store enqueues
    /// follow-up AppUI commands (e.g. `agent/list`,
    /// `session/goal/get`, `loop/list`) when a session opens or after
    /// reconnect, and the event loop drains them one per tick. The
    /// queue is bounded so a misbehaving server cannot cause it to
    /// grow without bound.
    pub pending_autonomy_hydration: std::collections::VecDeque<AppUiCommand>,
    /// M15-E follow-up: pause/resume issues a `session/goal/get` first
    /// to refresh server truth, then emits a `session/goal/set` with
    /// the freshly-fetched objective + this staged status. `None` when
    /// no pause/resume is in flight. Cleared when the next `GoalGet`
    /// response is consumed (success path) or when the user explicitly
    /// clears the goal.
    pub pending_goal_transition: Option<PendingGoalTransition>,
    pub exit_requested: bool,
}

/// M16-G2 per-session lifecycle ledger entry. The TUI keeps these in
/// a flat `Vec` (consistent with `permission_profiles` /
/// `session_runtime_statuses` neighbours) so the renderer can iterate
/// without HashMap lookups in hot paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContextLifecycleEntry {
    pub session_id: SessionKey,
    pub ledger: SessionContextLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerPresentation {
    Empty,
    Inline(String),
    Collapsed(ComposerCollapse),
}

impl ComposerPresentation {
    pub fn cursor_width(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Inline(text) => text.rsplit('\n').next().unwrap_or("").width(),
            Self::Collapsed(collapse) => "[paste] ".width() + collapse.summary.width(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerCollapse {
    pub summary: String,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerDraft {
    pub session_id: SessionKey,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimisticUserMessage {
    pub session_id: SessionKey,
    pub turn_id: TurnId,
    pub content: String,
    pub anchor_index: usize,
    pub prior_matching_user_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnActivityLog {
    pub session_id: SessionKey,
    pub turn_id: TurnId,
    pub request: Option<String>,
    pub anchor_index: Option<usize>,
    pub items: Vec<ActivityItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPermissionProfile {
    pub session_id: SessionKey,
    pub current: PermissionProfileSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRuntimeStatus {
    pub session_id: SessionKey,
    pub runtime_mode: Option<String>,
    pub profile_id: Option<String>,
    pub cwd: Option<String>,
    pub workspace_root: Option<String>,
    pub active_turn_id: Option<TurnId>,
    pub runtime_policy_stamp: Option<RuntimePolicyStamp>,
    pub model: Option<ModelStatus>,
    pub permission_profile: Option<String>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub sandbox: Option<String>,
    pub filesystem_scope: Option<String>,
    pub network: Option<String>,
    pub tool_policy_id: Option<String>,
    pub mcp_servers: Vec<String>,
    pub memory_scope: Option<String>,
    pub health: Option<RuntimeHealthStatus>,
    pub mcp_summary: Option<McpStatusSummary>,
    pub tool_summary: Option<ToolStatusSummary>,
    pub usage: Option<SessionUsageStatus>,
    pub cursor: Option<SessionCursorStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionModelCatalog {
    pub session_id: SessionKey,
    pub models: Vec<ModelStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMcpCatalog {
    pub session_id: SessionKey,
    pub servers: Vec<McpStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionToolCatalog {
    pub session_id: SessionKey,
    pub policy_id: Option<String>,
    pub coding_tool_contract: Option<CodingToolContract>,
    pub tools: Vec<ToolStatus>,
}

impl From<SessionStatusReadResult> for SessionRuntimeStatus {
    fn from(value: SessionStatusReadResult) -> Self {
        Self {
            session_id: value.session_id,
            runtime_mode: value.runtime_mode,
            profile_id: value.profile_id,
            cwd: value.cwd,
            workspace_root: value.workspace_root,
            active_turn_id: value.active_turn_id,
            runtime_policy_stamp: value.runtime_policy_stamp,
            model: value.model,
            permission_profile: value.permission_profile,
            approval_policy: value.approval_policy,
            sandbox_mode: value.sandbox_mode,
            sandbox: value.sandbox,
            filesystem_scope: value.filesystem_scope,
            network: value.network,
            tool_policy_id: value.tool_policy_id,
            mcp_servers: value.mcp_servers,
            memory_scope: value.memory_scope,
            health: value.health,
            mcp_summary: value.mcp_summary,
            tool_summary: value.tool_summary,
            usage: value.usage,
            cursor: value.cursor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Tool,
    Progress,
    Approval,
    Warning,
    Error,
}

impl ActivityKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Progress => "progress",
            Self::Approval => "approval",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub kind: ActivityKind,
    pub title: String,
    pub status: String,
    pub detail: Option<String>,
    pub arguments: Option<Value>,
    pub output_preview: Option<String>,
    pub success: Option<bool>,
    pub duration_ms: Option<u64>,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub completed: bool,
}

impl ActivityItem {
    pub fn new(kind: ActivityKind, title: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            status: status.into(),
            detail: None,
            arguments: None,
            output_preview: None,
            success: None,
            duration_ms: None,
            turn_id: None,
            tool_call_id: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_turn(mut self, turn_id: TurnId) -> Self {
        self.turn_id = Some(turn_id);
        self
    }

    pub fn with_tool_call(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }

    pub fn with_arguments(mut self, arguments: Value) -> Self {
        self.arguments = Some(arguments);
        self
    }

    pub fn with_output_preview(mut self, output_preview: impl Into<String>) -> Self {
        self.output_preview = Some(output_preview.into());
        self
    }

    pub fn with_success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalModalState {
    pub session_id: SessionKey,
    pub approval_id: ApprovalId,
    pub turn_id: TurnId,
    pub tool_name: String,
    pub title: String,
    pub body: String,
    pub approval_kind: Option<String>,
    pub risk: Option<String>,
    pub typed_details: Option<ApprovalTypedDetails>,
    pub render_hints: Option<ApprovalRenderHints>,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalModalAction {
    ApproveRequest,
    ApproveSession,
    DenyRequest,
}

impl ApprovalModalAction {
    pub fn decision(self) -> ApprovalDecision {
        match self {
            Self::ApproveRequest | Self::ApproveSession => ApprovalDecision::Approve,
            Self::DenyRequest => ApprovalDecision::Deny,
        }
    }

    pub fn approval_scope(self) -> &'static str {
        match self {
            Self::ApproveRequest | Self::DenyRequest => approval_scopes::REQUEST,
            Self::ApproveSession => approval_scopes::SESSION,
        }
    }

    pub fn status_label(self) -> &'static str {
        match self {
            Self::ApproveRequest => "approved for this request",
            Self::ApproveSession => "approved for this session",
            Self::DenyRequest => "denied",
        }
    }
}

impl ApprovalModalState {
    pub fn from_event(event: ApprovalRequestedEvent) -> Self {
        Self {
            session_id: event.session_id,
            approval_id: event.approval_id,
            turn_id: event.turn_id,
            tool_name: event.tool_name,
            title: event.title,
            body: event.body,
            approval_kind: event.approval_kind,
            risk: event.risk,
            typed_details: event.typed_details,
            render_hints: event.render_hints,
            visible: true,
        }
    }

    pub fn diff_preview_id(&self) -> Option<PreviewId> {
        self.typed_details
            .as_ref()
            .and_then(|details| details.diff.as_ref())
            .map(|diff| diff.preview_id.clone())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskOutputDetailState {
    pub active: bool,
    pub session_id: Option<SessionKey>,
    pub task_id: Option<TaskId>,
    pub title: String,
    pub output: String,
    pub cursor: Option<OutputCursor>,
    pub scroll: usize,
}

impl TaskOutputDetailState {
    pub fn open(
        &mut self,
        session_id: SessionKey,
        task_id: TaskId,
        title: String,
        output: String,
        cursor: Option<OutputCursor>,
    ) {
        self.active = true;
        self.session_id = Some(session_id);
        self.task_id = Some(task_id);
        self.title = title;
        self.output = output;
        self.cursor = cursor;
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn is_for(&self, session_id: &SessionKey, task_id: &TaskId) -> bool {
        self.active
            && self.session_id.as_ref() == Some(session_id)
            && self.task_id.as_ref() == Some(task_id)
    }

    pub fn append_output(&mut self, text: &str, cursor: OutputCursor) {
        self.output.push_str(text);
        self.cursor = Some(cursor);
        self.scroll = 0;
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactDetailState {
    pub active: bool,
    pub title: String,
    pub subtitle: String,
    pub content: String,
    pub scroll: usize,
}

impl ArtifactDetailState {
    pub fn open_agent_artifact(
        &mut self,
        agent_id: &str,
        artifact: &octos_core::ui_protocol::UiAgentArtifact,
        content: Option<String>,
    ) {
        self.active = true;
        self.title = artifact.title.clone();
        self.subtitle = format!("agent {agent_id} | {} | {}", artifact.kind, artifact.status);
        self.content = content
            .or_else(|| artifact.content.clone())
            .unwrap_or_else(|| "No content returned for this artifact".into());
        self.scroll = 0;
    }

    pub fn open_task_artifact(
        &mut self,
        task_id: &TaskId,
        artifact: &octos_core::ui_protocol::TaskArtifactRecord,
        content: Option<String>,
    ) {
        self.active = true;
        self.title = artifact.title.clone();
        self.subtitle = format!("task {task_id} | {} | {}", artifact.kind, artifact.status);
        self.content = content
            .or_else(|| artifact.content.clone())
            .unwrap_or_else(|| "No content returned for this artifact".into());
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutputCursor {
    pub session_id: SessionKey,
    pub task_id: TaskId,
    pub cursor: OutputCursor,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadGraphDetailState {
    pub active: bool,
    pub title: String,
    pub subtitle: String,
    pub content: String,
    pub scroll: usize,
}

impl ThreadGraphDetailState {
    pub fn open(&mut self, result: &ThreadGraphGetResult) {
        self.active = true;
        self.title = "Thread Graph".into();
        self.subtitle = format!(
            "{} thread(s) @ {}:{}",
            result.threads.len(),
            result.cursor.stream,
            result.cursor.seq
        );
        self.content = thread_graph_content(result);
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }
}

fn thread_graph_content(result: &ThreadGraphGetResult) -> String {
    let mut lines = Vec::new();
    if result.threads.is_empty() {
        lines.push("No threads returned for this session".to_string());
    } else {
        for thread in &result.threads {
            let turn = thread
                .turn_id
                .as_ref()
                .map(|turn_id| format!(" | turn {}", turn_id.0))
                .unwrap_or_default();
            lines.push(format!(
                "{} | {} | root seq {} | {} message(s){}",
                thread.thread_id,
                thread.status,
                thread.root_seq,
                thread.message_seqs.len(),
                turn
            ));
            if !thread.message_seqs.is_empty() {
                let seqs = thread
                    .message_seqs
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("  messages: {seqs}"));
            }
        }
    }
    if !result.orphans.is_empty() {
        let orphans = result
            .orphans
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Orphans: {orphans}"));
    }
    lines.join("\n")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnStateDetailState {
    pub active: bool,
    pub title: String,
    pub subtitle: String,
    pub content: String,
    pub scroll: usize,
}

impl TurnStateDetailState {
    pub fn open(&mut self, result: &TurnStateGetResult) {
        self.active = true;
        self.title = "Turn State".into();
        self.subtitle = format!("turn {}", result.turn_id.0);
        self.content = turn_state_content(result);
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }
}

fn turn_state_content(result: &TurnStateGetResult) -> String {
    let mut lines = vec![format!("state: {}", result.state.as_str())];
    if let Some(thread_id) = result.thread_id.as_deref() {
        lines.push(format!("thread: {thread_id}"));
    }
    if let Some(started_at) = result.started_at.as_ref() {
        lines.push(format!("started: {started_at}"));
    }
    if let Some(completed_at) = result.completed_at.as_ref() {
        lines.push(format!("completed: {completed_at}"));
    }
    if !result.committed_seqs.is_empty() {
        let seqs = result
            .committed_seqs
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("committed seqs: {seqs}"));
    }
    if let Some(context_state) = &result.context_state {
        lines.push(format!(
            "context: generation {} | {} items | {} tokens | {}",
            context_state.generation,
            context_state.item_count,
            context_state.token_estimate,
            context_state.recovery_state
        ));
    }
    if let Some(context) = &result.context {
        lines.push(format!("context payload: {context}"));
    }
    lines.join("\n")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactPaneState {
    pub items: Vec<ArtifactItem>,
    pub selected: usize,
}

impl ArtifactPaneState {
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
    }

    pub fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.items.len() - 1;
        } else {
            self.selected -= 1;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactItem {
    pub title: String,
    pub kind: String,
    pub source: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspacePaneState {
    pub root: String,
    pub contract: Vec<String>,
    pub entries: Vec<WorkspaceEntry>,
    pub selected: usize,
    pub scroll: usize,
}

impl WorkspacePaneState {
    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.entries.len();
        self.scroll = self.selected.saturating_sub(4);
    }

    pub fn select_prev(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.entries.len() - 1;
        } else {
            self.selected -= 1;
        }
        self.scroll = self.selected.saturating_sub(4);
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub depth: usize,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitPaneState {
    pub branch: String,
    pub head: Option<String>,
    pub status: Vec<GitStatusItem>,
    pub history: Vec<GitHistoryItem>,
    pub selected: usize,
    pub scroll: usize,
}

impl GitPaneState {
    pub fn selectable_len(&self) -> usize {
        self.status.len() + self.history.len()
    }

    pub fn select_next(&mut self) {
        let len = self.selectable_len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1) % len;
        self.scroll = self.selected.saturating_sub(4);
    }

    pub fn select_prev(&mut self) {
        let len = self.selectable_len();
        if len == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected -= 1;
        }
        self.scroll = self.selected.saturating_sub(4);
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusItem {
    pub code: String,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHistoryItem {
    pub commit: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffPreviewPaneState {
    pub active: bool,
    pub loading: bool,
    pub turn_id: Option<TurnId>,
    pub requested_preview_id: Option<PreviewId>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub preview: Option<DiffPreview>,
    pub error: Option<String>,
    pub scroll: usize,
    pub selected_file: usize,
    pub selected_hunk: usize,
}

impl DiffPreviewPaneState {
    pub fn open_loading(&mut self, preview_id: PreviewId) {
        self.open_loading_for_turn(preview_id, None);
    }

    pub fn open_loading_for_turn(&mut self, preview_id: PreviewId, turn_id: Option<TurnId>) {
        *self = Self {
            active: true,
            loading: true,
            turn_id,
            requested_preview_id: Some(preview_id),
            status: Some("loading".into()),
            source: None,
            preview: None,
            error: None,
            scroll: 0,
            selected_file: 0,
            selected_hunk: 0,
        };
    }

    pub fn apply_result(&mut self, result: DiffPreviewGetResult) {
        let turn_id = self
            .requested_preview_id
            .as_ref()
            .filter(|preview_id| **preview_id == result.preview.preview_id)
            .and_then(|_| self.turn_id.clone());
        self.active = true;
        self.loading = false;
        self.turn_id = turn_id;
        self.requested_preview_id = Some(result.preview.preview_id.clone());
        self.status = Some(result.status);
        self.source = Some(result.source);
        self.preview = Some(result.preview);
        self.error = None;
        self.scroll = 0;
        self.clamp_selection();
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn select_next_hunk(&mut self) {
        let hunks = self.hunk_locations();
        if hunks.is_empty() {
            return;
        }
        let current = self.selected_location_index(&hunks).unwrap_or(0);
        let (file_idx, hunk_idx) = hunks[(current + 1) % hunks.len()];
        self.selected_file = file_idx;
        self.selected_hunk = hunk_idx;
    }

    pub fn select_prev_hunk(&mut self) {
        let hunks = self.hunk_locations();
        if hunks.is_empty() {
            return;
        }
        let current = self.selected_location_index(&hunks).unwrap_or(0);
        let next = if current == 0 {
            hunks.len() - 1
        } else {
            current - 1
        };
        let (file_idx, hunk_idx) = hunks[next];
        self.selected_file = file_idx;
        self.selected_hunk = hunk_idx;
    }

    pub fn selected_hunk_context(&self) -> Option<DiffHunkContext> {
        let preview = self.preview.as_ref()?;
        let file = preview.files.get(self.selected_file)?;
        let hunk = file.hunks.get(self.selected_hunk)?;
        Some(DiffHunkContext {
            path: file.path.clone(),
            old_path: file.old_path.clone(),
            file_status: file.status.clone(),
            hunk_header: hunk.header.clone(),
            lines: hunk.lines.clone(),
        })
    }

    fn clamp_selection(&mut self) {
        let hunks = self.hunk_locations();
        if let Some((file_idx, hunk_idx)) = hunks.first().copied() {
            self.selected_file = file_idx;
            self.selected_hunk = hunk_idx;
        } else {
            self.selected_file = 0;
            self.selected_hunk = 0;
        }
    }

    fn hunk_locations(&self) -> Vec<(usize, usize)> {
        self.preview
            .as_ref()
            .map(|preview| {
                preview
                    .files
                    .iter()
                    .enumerate()
                    .flat_map(|(file_idx, file)| {
                        file.hunks
                            .iter()
                            .enumerate()
                            .map(move |(hunk_idx, _)| (file_idx, hunk_idx))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn selected_location_index(&self, hunks: &[(usize, usize)]) -> Option<usize> {
        hunks.iter().position(|(file_idx, hunk_idx)| {
            *file_idx == self.selected_file && *hunk_idx == self.selected_hunk
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunkContext {
    pub path: String,
    pub old_path: Option<String>,
    pub file_status: String,
    pub hunk_header: String,
    pub lines: Vec<DiffPreviewLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPreviewGetResult {
    pub status: String,
    pub source: String,
    pub preview: DiffPreview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPreview {
    pub session_id: SessionKey,
    pub preview_id: PreviewId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<DiffPreviewFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPreviewFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    #[serde(default = "unknown_label")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<DiffPreviewHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPreviewHunk {
    pub header: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<DiffPreviewLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPreviewLine {
    #[serde(default = "context_label")]
    pub kind: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
}

fn unknown_label() -> String {
    "unknown".into()
}

fn context_label() -> String {
    "context".into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotPaneSeed {
    artifacts: ArtifactPaneState,
    workspace: WorkspacePaneState,
    git: GitPaneState,
}

impl SnapshotPaneSeed {
    fn from_snapshot(snapshot: &AppUiSnapshot) -> Self {
        Self::from_parts(
            &snapshot.sessions,
            &snapshot.status,
            snapshot.target.as_deref(),
            snapshot.readonly,
        )
    }

    fn from_parts(
        sessions: &[SessionView],
        status: &str,
        target: Option<&str>,
        readonly: bool,
    ) -> Self {
        let source = SnapshotSource::classify(status, target);
        Self {
            artifacts: seed_artifacts(sessions, status, target, readonly, source),
            workspace: seed_workspace(sessions, target, readonly, source),
            git: seed_git(source),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotSource {
    Mock,
    Protocol,
    Unknown,
}

impl SnapshotSource {
    fn classify(status: &str, target: Option<&str>) -> Self {
        let status = status.to_ascii_lowercase();
        let target = target.unwrap_or_default().to_ascii_lowercase();

        if status.contains("mock") || target.contains("mock") {
            Self::Mock
        } else if status.contains("protocol")
            || target.starts_with("ws://")
            || target.starts_with("wss://")
        {
            Self::Protocol
        } else {
            Self::Unknown
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mock => "mock snapshot",
            Self::Protocol => "protocol snapshot",
            Self::Unknown => "app-ui snapshot",
        }
    }
}

fn seed_artifacts(
    sessions: &[SessionView],
    status: &str,
    target: Option<&str>,
    readonly: bool,
    source: SnapshotSource,
) -> ArtifactPaneState {
    let mut items = vec![ArtifactItem {
        title: "AppUi bootstrap snapshot".into(),
        kind: "snapshot".into(),
        source: target.unwrap_or_else(|| source.label()).to_string(),
        status: if readonly {
            "read-only".into()
        } else {
            status.to_string()
        },
    }];

    for session in sessions {
        for task in &session.tasks {
            if let Some(line) = first_non_empty_line(&task.output_tail) {
                items.push(ArtifactItem {
                    title: format!("{} output tail", task.title),
                    kind: "task-output".into(),
                    source: session.title.clone(),
                    status: line.to_string(),
                });
            }

            let preview_id = task
                .runtime_detail
                .as_deref()
                .and_then(preview_id_from_text)
                .or_else(|| preview_id_from_text(&task.output_tail));
            if let Some(preview_id) = preview_id {
                items.push(ArtifactItem {
                    title: format!("{} diff preview", task.title),
                    kind: "diff-preview".into(),
                    source: session.title.clone(),
                    status: preview_id.0.to_string(),
                });
            }
        }
    }

    match source {
        SnapshotSource::Mock => items.push(ArtifactItem {
            title: "M9.7 mock artifact manifest".into(),
            kind: "mock".into(),
            source: "mock backend".into(),
            status: "seeded".into(),
        }),
        SnapshotSource::Protocol => items.push(ArtifactItem {
            title: "Protocol artifact stream".into(),
            kind: "contract".into(),
            source: "app-ui protocol".into(),
            status: "waiting for artifact payloads".into(),
        }),
        SnapshotSource::Unknown => {}
    }

    ArtifactPaneState { items, selected: 0 }
}

fn seed_workspace(
    sessions: &[SessionView],
    target: Option<&str>,
    readonly: bool,
    source: SnapshotSource,
) -> WorkspacePaneState {
    let mut contract = vec![
        format!("api {APP_UI_API_V1}"),
        "snapshot.sessions -> Sessions, Tasks, Transcript".into(),
        "snapshot task tails -> Artifacts hints".into(),
        "snapshot target/status -> Workspace/Git fallback".into(),
    ];

    match source {
        SnapshotSource::Mock => {
            contract.push("mock backend seeds local M9.7 panes".into());
        }
        SnapshotSource::Protocol => {
            contract.push("pane.snapshots.v1 hydrates panes when negotiated".into());
            contract.push("fallback panes render until server snapshot arrives".into());
        }
        SnapshotSource::Unknown => {}
    }
    if readonly {
        contract.push("readonly launch: commands disabled".into());
    }

    let mut entries = vec![WorkspaceEntry {
        depth: 0,
        label: "sessions".into(),
        detail: format!("{} hydrated", sessions.len()),
    }];
    for session in sessions {
        entries.push(WorkspaceEntry {
            depth: 1,
            label: session.title.clone(),
            detail: session.id.0.clone(),
        });
        entries.push(WorkspaceEntry {
            depth: 2,
            label: "messages".into(),
            detail: session.messages.len().to_string(),
        });
        if session.tasks.is_empty() {
            entries.push(WorkspaceEntry {
                depth: 2,
                label: "tasks".into(),
                detail: "none".into(),
            });
        } else {
            for task in &session.tasks {
                entries.push(WorkspaceEntry {
                    depth: 2,
                    label: task.title.clone(),
                    detail: task_state_label(task.state).into(),
                });
            }
        }
    }

    WorkspacePaneState {
        root: target.unwrap_or_else(|| source.label()).to_string(),
        contract,
        entries,
        selected: 0,
        scroll: 0,
    }
}

fn seed_git(source: SnapshotSource) -> GitPaneState {
    match source {
        SnapshotSource::Mock => GitPaneState {
            branch: "m9.7/mock-snapshot".into(),
            head: Some("mock-head".into()),
            status: vec![
                GitStatusItem {
                    code: "M".into(),
                    path: "src/model.rs".into(),
                    detail: "pane state contract".into(),
                },
                GitStatusItem {
                    code: "M".into(),
                    path: "src/app.rs".into(),
                    detail: "pane rendering surface".into(),
                },
            ],
            history: vec![
                GitHistoryItem {
                    commit: "mock-m97".into(),
                    summary: "seed missing pane snapshots".into(),
                },
                GitHistoryItem {
                    commit: "mock-m9".into(),
                    summary: "app-ui protocol TUI scaffold".into(),
                },
            ],
            selected: 0,
            scroll: 0,
        },
        SnapshotSource::Protocol => GitPaneState {
            branch: "not supplied".into(),
            head: None,
            status: vec![GitStatusItem {
                code: "?".into(),
                path: "git status".into(),
                detail: "protocol snapshot does not include git state yet".into(),
            }],
            history: vec![GitHistoryItem {
                commit: "pending".into(),
                summary: "waiting for git history snapshot".into(),
            }],
            selected: 0,
            scroll: 0,
        },
        SnapshotSource::Unknown => GitPaneState {
            branch: "unknown".into(),
            head: None,
            status: vec![GitStatusItem {
                code: "?".into(),
                path: "git status".into(),
                detail: "snapshot source did not include git state".into(),
            }],
            history: vec![GitHistoryItem {
                commit: "pending".into(),
                summary: "no git history in snapshot".into(),
            }],
            selected: 0,
            scroll: 0,
        },
    }
}

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

impl AppState {
    pub fn from_snapshot(snapshot: AppUiSnapshot) -> Self {
        let panes = SnapshotPaneSeed::from_snapshot(&snapshot);
        Self::new_with_panes(
            snapshot.sessions,
            snapshot.selected_session,
            snapshot.status,
            snapshot.target,
            snapshot.readonly,
            panes,
        )
    }

    pub fn new(
        sessions: Vec<SessionView>,
        selected_session: usize,
        status: String,
        target: Option<String>,
        readonly: bool,
    ) -> Self {
        let panes = SnapshotPaneSeed::from_parts(&sessions, &status, target.as_deref(), readonly);
        Self::new_with_panes(sessions, selected_session, status, target, readonly, panes)
    }

    fn new_with_panes(
        sessions: Vec<SessionView>,
        selected_session: usize,
        status: String,
        target: Option<String>,
        readonly: bool,
        panes: SnapshotPaneSeed,
    ) -> Self {
        let selected_session = if sessions.is_empty() {
            0
        } else {
            selected_session.min(sessions.len() - 1)
        };
        let run_state = initial_run_state(&sessions, selected_session);

        let run_state_started_at = run_state.is_active().then(Instant::now);

        Self {
            sessions,
            selected_session,
            selected_task: 0,
            transcript_scroll: 0,
            focus: FocusPane::Composer,
            artifacts: panes.artifacts,
            workspace: panes.workspace,
            git: panes.git,
            composer: String::new(),
            composer_cursor: None,
            composer_drafts: Vec::new(),
            pending_messages: Vec::new(),
            optimistic_user_messages: Vec::new(),
            status,
            target,
            readonly,
            protocol_version: APP_UI_API_V1,
            run_state,
            run_state_started_at,
            approval_auto_open: true,
            approval: None,
            task_output: TaskOutputDetailState::default(),
            artifact_detail: ArtifactDetailState::default(),
            thread_graph_detail: ThreadGraphDetailState::default(),
            turn_state_detail: TurnStateDetailState::default(),
            task_output_cursors: Vec::new(),
            diff_preview: DiffPreviewPaneState::default(),
            activity: Vec::new(),
            turn_activity_logs: Vec::new(),
            expanded_tool_outputs: false,
            menu_stack: MenuStack::new(),
            active_menu: None,
            capabilities: None,
            onboarding: OnboardingWizardState::default(),
            permission_profiles: Vec::new(),
            session_runtime_statuses: Vec::new(),
            profile_llm_catalog: None,
            profile_llm_state: None,
            profile_skills: None,
            profile_skill_registry: None,
            session_model_catalogs: Vec::new(),
            session_mcp_catalogs: Vec::new(),
            session_tool_catalogs: Vec::new(),
            mcp_config_catalog: None,
            tool_config_catalog: None,
            context_lifecycle: Vec::new(),
            session_autonomy: Vec::new(),
            pending_autonomy_hydration: std::collections::VecDeque::new(),
            pending_goal_transition: None,
            exit_requested: false,
        }
    }

    /// M16-G2 helper: returns the context-lifecycle ledger for a
    /// session, or `None` if the server has not yet emitted any
    /// `context/compaction_completed` / `context/normalization_reported`
    /// notifications.
    pub fn context_lifecycle_for(
        &self,
        session_id: &SessionKey,
    ) -> Option<&SessionContextLifecycle> {
        self.context_lifecycle
            .iter()
            .find(|entry| entry.session_id == *session_id)
            .map(|entry| &entry.ledger)
    }

    /// M16-G2 helper: mutably accesses (creating if necessary) the
    /// lifecycle ledger for a session.
    pub fn context_lifecycle_mut(
        &mut self,
        session_id: &SessionKey,
    ) -> &mut SessionContextLifecycle {
        if let Some(pos) = self
            .context_lifecycle
            .iter()
            .position(|entry| entry.session_id == *session_id)
        {
            return &mut self.context_lifecycle[pos].ledger;
        }
        self.context_lifecycle.push(SessionContextLifecycleEntry {
            session_id: session_id.clone(),
            ledger: SessionContextLifecycle::default(),
        });
        &mut self
            .context_lifecycle
            .last_mut()
            .expect("just pushed")
            .ledger
    }

    /// M15-E: read-only access to the autonomy mirror for a session,
    /// or `None` if the backend has not yet emitted any agent / goal
    /// / loop state for it.
    pub fn session_autonomy_for(&self, session_id: &SessionKey) -> Option<&SessionAutonomyState> {
        self.session_autonomy
            .iter()
            .find(|entry| &entry.session_id == session_id)
    }

    /// M15-E: mutable access to the autonomy mirror for a session.
    /// Creates a fresh entry on first access — the mirror is empty
    /// until the backend confirms state.
    pub fn session_autonomy_mut(&mut self, session_id: &SessionKey) -> &mut SessionAutonomyState {
        if let Some(pos) = self
            .session_autonomy
            .iter()
            .position(|entry| &entry.session_id == session_id)
        {
            return &mut self.session_autonomy[pos];
        }
        self.session_autonomy
            .push(SessionAutonomyState::new(session_id.clone()));
        self.session_autonomy
            .last_mut()
            .expect("just pushed autonomy entry")
    }

    /// Replace the entire agent list for a session. Used by the
    /// `agent/list` response and after reconnect-hydration.
    pub fn set_session_agents(
        &mut self,
        session_id: &SessionKey,
        agents: Vec<octos_core::ui_protocol::UiAgentRecord>,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        entry.agents = agents;
    }

    /// Upsert one agent record by `agent_id`. The wire schema may
    /// arrive via `agent/updated` or as part of an `agent/list`
    /// response.
    pub fn upsert_session_agent(
        &mut self,
        session_id: &SessionKey,
        agent: octos_core::ui_protocol::UiAgentRecord,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        if let Some(pos) = entry
            .agents
            .iter()
            .position(|a| a.agent_id == agent.agent_id)
        {
            entry.agents[pos] = agent;
        } else {
            entry.agents.push(agent);
        }
    }

    /// Replace the loop list for a session.
    pub fn set_session_loops(
        &mut self,
        session_id: &SessionKey,
        loops: Vec<octos_core::ui_protocol::UiLoopRecord>,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        entry.loops = loops;
    }

    /// Upsert one loop record by `loop_id`. Removes the loop when its
    /// status becomes `deleted` so reconnect doesn't surface tombstones.
    pub fn upsert_session_loop(
        &mut self,
        session_id: &SessionKey,
        loop_state: octos_core::ui_protocol::UiLoopRecord,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        if loop_state.status == "deleted" {
            entry.loops.retain(|l| l.loop_id != loop_state.loop_id);
            return;
        }
        if let Some(pos) = entry
            .loops
            .iter()
            .position(|l| l.loop_id == loop_state.loop_id)
        {
            entry.loops[pos] = loop_state;
        } else {
            entry.loops.push(loop_state);
        }
    }

    /// Remove a loop entry by id (used for explicit `loop/delete`
    /// responses where the backend doesn't echo a deleted-status loop
    /// record).
    pub fn remove_session_loop(&mut self, session_id: &SessionKey, loop_id: &str) {
        if let Some(entry) = self
            .session_autonomy
            .iter_mut()
            .find(|entry| &entry.session_id == session_id)
        {
            entry.loops.retain(|l| l.loop_id != loop_id);
        }
    }

    /// Set the current goal for a session. `goal = None` clears it.
    pub fn set_session_goal(
        &mut self,
        session_id: &SessionKey,
        goal: Option<octos_core::ui_protocol::UiGoalRecord>,
        transition_actor: Option<String>,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        entry.goal = goal;
        entry.goal_transition_actor = transition_actor;
    }

    /// Replace the cached output tail for an agent. The backend is
    /// authoritative; deltas are appended via [`append_agent_output`].
    pub fn set_agent_output(
        &mut self,
        session_id: &SessionKey,
        agent_id: &str,
        text: String,
        cursor: OutputCursor,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        if let Some(pos) = entry
            .agent_outputs
            .iter()
            .position(|cache| cache.agent_id == agent_id)
        {
            entry.agent_outputs[pos] = AutonomyAgentOutputCache {
                agent_id: agent_id.to_string(),
                text,
                cursor,
            };
        } else {
            entry.agent_outputs.push(AutonomyAgentOutputCache {
                agent_id: agent_id.to_string(),
                text,
                cursor,
            });
        }
    }

    /// Append output deltas from `agent/output/delta`. If the cursor
    /// has rolled past the cached one the entry is overwritten so
    /// stale text never lingers in the mirror.
    pub fn append_agent_output(
        &mut self,
        session_id: &SessionKey,
        agent_id: &str,
        cursor: OutputCursor,
        text: &str,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        if let Some(pos) = entry
            .agent_outputs
            .iter()
            .position(|cache| cache.agent_id == agent_id)
        {
            let cache = &mut entry.agent_outputs[pos];
            if cursor.offset < cache.cursor.offset {
                // Backend rewound; replace.
                cache.text = text.to_string();
            } else {
                cache.text.push_str(text);
            }
            cache.cursor = cursor;
        } else {
            entry.agent_outputs.push(AutonomyAgentOutputCache {
                agent_id: agent_id.to_string(),
                text: text.to_string(),
                cursor,
            });
        }
    }

    /// Enqueue a pending autonomy hydration command. Bounded — extra
    /// commands beyond a small cap are dropped to keep the queue
    /// O(1) — fresh hydration on the next reconnect is cheap.
    pub fn enqueue_autonomy_hydration(&mut self, command: AppUiCommand) {
        const MAX_PENDING_HYDRATION: usize = 16;
        if self.pending_autonomy_hydration.len() >= MAX_PENDING_HYDRATION {
            self.pending_autonomy_hydration.pop_front();
        }
        self.pending_autonomy_hydration.push_back(command);
    }

    /// Dequeue the next pending hydration command. Returns `None` when
    /// the queue is empty.
    pub fn dequeue_autonomy_hydration(&mut self) -> Option<AppUiCommand> {
        self.pending_autonomy_hydration.pop_front()
    }

    /// Replace the artifact cache for a single agent.
    pub fn set_agent_artifacts(
        &mut self,
        session_id: &SessionKey,
        agent_id: &str,
        artifacts: Vec<octos_core::ui_protocol::UiAgentArtifact>,
    ) {
        let entry = self.session_autonomy_mut(session_id);
        if let Some(pos) = entry
            .agent_artifacts
            .iter()
            .position(|cache| cache.agent_id == agent_id)
        {
            entry.agent_artifacts[pos] = AutonomyAgentArtifactCache {
                agent_id: agent_id.to_string(),
                artifacts,
            };
        } else {
            entry.agent_artifacts.push(AutonomyAgentArtifactCache {
                agent_id: agent_id.to_string(),
                artifacts,
            });
        }
    }

    pub fn permission_profile_for(
        &self,
        session_id: &SessionKey,
    ) -> Option<PermissionProfileSelection> {
        self.permission_profiles
            .iter()
            .find(|profile| &profile.session_id == session_id)
            .map(|profile| profile.current)
    }

    pub fn set_permission_profile(
        &mut self,
        session_id: SessionKey,
        current: PermissionProfileSelection,
    ) {
        let current = current.normalized();
        if let Some(profile) = self
            .permission_profiles
            .iter_mut()
            .find(|profile| profile.session_id == session_id)
        {
            profile.current = current;
        } else {
            self.permission_profiles.push(SessionPermissionProfile {
                session_id,
                current,
            });
        }
    }

    pub fn runtime_status_for(&self, session_id: &SessionKey) -> Option<&SessionRuntimeStatus> {
        self.session_runtime_statuses
            .iter()
            .find(|status| &status.session_id == session_id)
    }

    pub fn set_runtime_status(&mut self, status: SessionRuntimeStatus) {
        if let Some(existing) = self
            .session_runtime_statuses
            .iter_mut()
            .find(|existing| existing.session_id == status.session_id)
        {
            *existing = status;
        } else {
            self.session_runtime_statuses.push(status);
        }
    }

    pub fn model_catalog_for(&self, session_id: &SessionKey) -> Option<&SessionModelCatalog> {
        self.session_model_catalogs
            .iter()
            .find(|catalog| &catalog.session_id == session_id)
    }

    pub fn set_model_catalog(&mut self, catalog: SessionModelCatalog) {
        if let Some(existing) = self
            .session_model_catalogs
            .iter_mut()
            .find(|existing| existing.session_id == catalog.session_id)
        {
            *existing = catalog;
        } else {
            self.session_model_catalogs.push(catalog);
        }
    }

    pub fn mcp_catalog_for(&self, session_id: &SessionKey) -> Option<&SessionMcpCatalog> {
        self.session_mcp_catalogs
            .iter()
            .find(|catalog| &catalog.session_id == session_id)
    }

    pub fn set_mcp_catalog(&mut self, catalog: SessionMcpCatalog) {
        if let Some(existing) = self
            .session_mcp_catalogs
            .iter_mut()
            .find(|existing| existing.session_id == catalog.session_id)
        {
            *existing = catalog;
        } else {
            self.session_mcp_catalogs.push(catalog);
        }
    }

    pub fn tool_catalog_for(&self, session_id: &SessionKey) -> Option<&SessionToolCatalog> {
        self.session_tool_catalogs
            .iter()
            .find(|catalog| &catalog.session_id == session_id)
    }

    pub fn set_tool_catalog(&mut self, catalog: SessionToolCatalog) {
        if let Some(existing) = self
            .session_tool_catalogs
            .iter_mut()
            .find(|existing| existing.session_id == catalog.session_id)
        {
            *existing = catalog;
        } else {
            self.session_tool_catalogs.push(catalog);
        }
    }

    pub fn availability_context(&self) -> AvailabilityContext<'_> {
        AvailabilityContext {
            task: if self.active_turn().is_some()
                || self.active_task().is_some_and(|task| {
                    matches!(
                        task.state,
                        TaskRuntimeState::Pending | TaskRuntimeState::Running
                    )
                }) {
                TaskActivity::Running
            } else {
                TaskActivity::Idle
            },
            approval_modal_visible: self
                .approval
                .as_ref()
                .is_some_and(|approval| approval.visible),
            readonly: self.readonly,
            runtime: if self.target.as_deref().is_some_and(is_protocol_target) {
                RuntimeMode::Protocol
            } else {
                RuntimeMode::Mock
            },
            connection: if self.target.as_deref().is_some_and(is_protocol_target) {
                ConnectionState::Connected
            } else {
                ConnectionState::Disconnected
            },
            capabilities: self.capabilities.as_ref(),
            feature_flags: &[],
            session_open: !self.sessions.is_empty(),
        }
    }

    pub fn set_capabilities(&mut self, capabilities: UiProtocolCapabilities) {
        self.capabilities = Some(CapabilitySet::from(&capabilities));
    }

    pub fn apply_pane_snapshot(&mut self, panes: UiPaneSnapshot) {
        if let Some(artifacts) = panes.artifacts {
            self.artifacts.items = artifacts
                .items
                .into_iter()
                .map(|item| ArtifactItem {
                    title: item.title,
                    kind: item.kind,
                    source: item
                        .source
                        .or(item.path)
                        .unwrap_or_else(|| "protocol".into()),
                    status: item.status,
                })
                .collect();
            self.artifacts.selected = self
                .artifacts
                .selected
                .min(self.artifacts.items.len().saturating_sub(1));
        }

        if let Some(workspace) = panes.workspace {
            self.workspace.root = workspace.root;
            self.workspace.contract = workspace.contract;
            self.workspace.entries = workspace
                .entries
                .into_iter()
                .map(|entry| WorkspaceEntry {
                    depth: entry.depth,
                    label: entry.label,
                    detail: entry
                        .detail
                        .unwrap_or_else(|| format!("{} {}", entry.kind, entry.path)),
                })
                .collect();
            self.workspace.selected = self
                .workspace
                .selected
                .min(self.workspace.entries.len().saturating_sub(1));
            self.workspace.scroll = self.workspace.scroll.min(self.workspace.selected);
        }

        if let Some(git) = panes.git {
            self.git.branch = git.branch.unwrap_or_else(|| "not supplied".into());
            self.git.head = git.head;
            self.git.status = git
                .status
                .into_iter()
                .map(|item| GitStatusItem {
                    code: item.code,
                    path: item.path,
                    detail: item.detail,
                })
                .collect();
            self.git.history = git
                .history
                .into_iter()
                .map(|item| GitHistoryItem {
                    commit: item.commit,
                    summary: item.summary,
                })
                .collect();
            self.git.selected = self
                .git
                .selected
                .min(self.git.selectable_len().saturating_sub(1));
            self.git.scroll = self.git.scroll.min(self.git.selected);
        }
    }

    pub fn active_session(&self) -> Option<&SessionView> {
        self.sessions.get(self.selected_session)
    }

    pub fn active_session_mut(&mut self) -> Option<&mut SessionView> {
        self.sessions.get_mut(self.selected_session)
    }

    pub fn active_turn(&self) -> Option<(&SessionKey, &TurnId)> {
        let session = self.active_session()?;
        let live_reply = session.live_reply.as_ref()?;
        Some((&session.id, &live_reply.turn_id))
    }

    pub fn record_submitted_user_prompt(
        &mut self,
        session_id: SessionKey,
        turn_id: TurnId,
        content: String,
    ) {
        let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        else {
            return;
        };
        let optimistic = OptimisticUserMessage {
            prior_matching_user_count: matching_user_message_count(session, &content),
            anchor_index: session.messages.len(),
            session_id,
            turn_id,
            content,
        };
        self.optimistic_user_messages.push(optimistic);
        const MAX_OPTIMISTIC_USER_MESSAGES: usize = 64;
        if self.optimistic_user_messages.len() > MAX_OPTIMISTIC_USER_MESSAGES {
            let excess = self.optimistic_user_messages.len() - MAX_OPTIMISTIC_USER_MESSAGES;
            self.optimistic_user_messages.drain(0..excess);
        }
        self.restore_optimistic_user_messages();
    }

    pub fn restore_optimistic_user_messages(&mut self) {
        let mut retained = Vec::new();
        for optimistic in self.optimistic_user_messages.clone() {
            let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.id == optimistic.session_id)
            else {
                retained.push(optimistic);
                continue;
            };
            if matching_user_message_count(session, &optimistic.content)
                > optimistic.prior_matching_user_count
            {
                continue;
            }

            let insert_at = optimistic.anchor_index.min(session.messages.len());
            session
                .messages
                .insert(insert_at, Message::user(optimistic.content.clone()));
            retained.push(optimistic);
        }
        self.optimistic_user_messages = retained;
    }

    pub fn capture_completed_turn_activity(
        &mut self,
        session_id: &SessionKey,
        turn_id: &TurnId,
    ) -> bool {
        let items = self
            .activity
            .iter()
            .filter(|item| item.turn_id.as_ref() == Some(turn_id))
            .cloned()
            .collect::<Vec<_>>();
        if items.is_empty() {
            return false;
        }

        let optimistic = self
            .optimistic_user_messages
            .iter()
            .rev()
            .find(|message| &message.session_id == session_id && &message.turn_id == turn_id);
        let request = optimistic
            .map(|message| message.content.clone())
            .or_else(|| latest_user_content_for_session(&self.sessions, session_id));
        let anchor_index = optimistic.map(|message| message.anchor_index);
        let log = TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            request,
            anchor_index,
            items,
        };

        if let Some(existing) = self
            .turn_activity_logs
            .iter_mut()
            .find(|existing| &existing.session_id == session_id && &existing.turn_id == turn_id)
        {
            *existing = log;
        } else {
            self.turn_activity_logs.push(log);
        }

        const MAX_TURN_ACTIVITY_LOGS: usize = 32;
        if self.turn_activity_logs.len() > MAX_TURN_ACTIVITY_LOGS {
            let excess = self.turn_activity_logs.len() - MAX_TURN_ACTIVITY_LOGS;
            self.turn_activity_logs.drain(0..excess);
        }

        self.activity
            .retain(|item| item.turn_id.as_ref() != Some(turn_id));
        true
    }

    pub fn has_pending_messages(&self) -> bool {
        !self.pending_messages.is_empty()
    }

    pub fn active_task(&self) -> Option<&TaskView> {
        self.active_session()?.tasks.get(self.selected_task)
    }

    pub fn active_task_context(&self) -> Option<SelectedTaskContext> {
        let session = self.active_session()?;
        let task = session.tasks.get(self.selected_task)?;
        Some(SelectedTaskContext {
            session_id: session.id.clone(),
            task_id: task.id.clone(),
            title: task.title.clone(),
            output_tail: task.output_tail.clone(),
        })
    }

    pub fn active_diff_preview_id(&self) -> Option<PreviewId> {
        let task = self.active_task()?;
        task.runtime_detail
            .as_deref()
            .and_then(preview_id_from_text)
            .or_else(|| preview_id_from_text(&task.output_tail))
    }

    pub fn select_next_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.persist_composer_draft_for_selected_session();
        self.selected_session = (self.selected_session + 1) % self.sessions.len();
        self.selected_task = 0;
        self.transcript_scroll = 0;
        self.load_composer_draft_for_selected_session();
        self.refresh_run_state_from_selection();
    }

    pub fn select_prev_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.persist_composer_draft_for_selected_session();
        if self.selected_session == 0 {
            self.selected_session = self.sessions.len() - 1;
        } else {
            self.selected_session -= 1;
        }
        self.selected_task = 0;
        self.transcript_scroll = 0;
        self.load_composer_draft_for_selected_session();
        self.refresh_run_state_from_selection();
    }

    pub fn select_next_task(&mut self) {
        let Some(session) = self.active_session() else {
            return;
        };
        if session.tasks.is_empty() {
            return;
        }
        self.selected_task = (self.selected_task + 1) % session.tasks.len();
    }

    pub fn select_prev_task(&mut self) {
        let Some(session) = self.active_session() else {
            return;
        };
        if session.tasks.is_empty() {
            return;
        }
        if self.selected_task == 0 {
            self.selected_task = session.tasks.len() - 1;
        } else {
            self.selected_task -= 1;
        }
    }

    pub fn select_next_artifact(&mut self) {
        self.artifacts.select_next();
    }

    pub fn select_prev_artifact(&mut self) {
        self.artifacts.select_prev();
    }

    pub fn select_next_workspace_entry(&mut self) {
        self.workspace.select_next();
    }

    pub fn select_prev_workspace_entry(&mut self) {
        self.workspace.select_prev();
    }

    pub fn select_next_git_entry(&mut self) {
        self.git.select_next();
    }

    pub fn select_prev_git_entry(&mut self) {
        self.git.select_prev();
    }

    pub fn scroll_transcript_up(&mut self, lines: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(lines);
    }

    pub fn scroll_transcript_down(&mut self, lines: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(lines);
    }

    pub fn scroll_transcript_to_latest(&mut self) {
        self.transcript_scroll = 0;
    }

    pub fn set_task_output_cursor(
        &mut self,
        session_id: SessionKey,
        task_id: TaskId,
        cursor: OutputCursor,
    ) {
        if let Some(existing) = self
            .task_output_cursors
            .iter_mut()
            .find(|entry| entry.session_id == session_id && entry.task_id == task_id)
        {
            existing.cursor = cursor;
        } else {
            self.task_output_cursors.push(TaskOutputCursor {
                session_id,
                task_id,
                cursor,
            });
        }
    }

    pub fn task_output_cursor(
        &self,
        session_id: &SessionKey,
        task_id: &TaskId,
    ) -> Option<OutputCursor> {
        self.task_output_cursors
            .iter()
            .find(|entry| &entry.session_id == session_id && &entry.task_id == task_id)
            .map(|entry| entry.cursor)
    }

    pub fn push_activity(&mut self, item: ActivityItem) {
        const MAX_ACTIVITY_ITEMS: usize = 80;
        let estimated_rows = estimated_activity_rows(&item);
        self.activity.push(item);
        self.preserve_transcript_position_after_append(estimated_rows);
        if self.activity.len() > MAX_ACTIVITY_ITEMS {
            let excess = self.activity.len() - MAX_ACTIVITY_ITEMS;
            self.activity.drain(0..excess);
        }
    }

    pub fn preserve_transcript_position_after_append(&mut self, estimated_rows: usize) {
        if self.transcript_scroll > 0 && estimated_rows > 0 {
            self.transcript_scroll = self.transcript_scroll.saturating_add(estimated_rows);
        }
    }

    pub fn update_tool_activity(
        &mut self,
        tool_call_id: &str,
        status: impl Into<String>,
        detail: Option<String>,
        output_preview: Option<String>,
        success: Option<bool>,
        duration_ms: Option<u64>,
    ) {
        let status = status.into();
        let mut updated = false;
        if let Some(item) = self
            .activity
            .iter_mut()
            .rev()
            .find(|item| item.tool_call_id.as_deref() == Some(tool_call_id))
        {
            item.status = status;
            if detail.is_some() {
                item.detail = detail;
            }
            if output_preview.is_some() {
                item.output_preview = output_preview;
            }
            if success.is_some() {
                item.success = success;
            }
            if duration_ms.is_some() {
                item.duration_ms = duration_ms;
            }
            updated = true;
        }
        if updated {
            self.preserve_transcript_position_after_append(1);
        }
    }

    pub fn set_run_state_idle(&mut self) {
        self.run_state = SessionRunState::Idle;
        self.run_state_started_at = None;
    }

    pub fn set_run_state_in_progress(&mut self) {
        if !self.run_state.is_active() {
            self.run_state_started_at = Some(Instant::now());
        }
        self.run_state = SessionRunState::InProgress;
    }

    pub fn set_run_state_blocked(&mut self, message: impl Into<String>) {
        if !self.run_state.is_active() {
            self.run_state_started_at = Some(Instant::now());
        }
        self.run_state = SessionRunState::Blocked {
            message: message.into(),
        };
    }

    pub fn set_run_state_success(&mut self) {
        self.run_state = SessionRunState::Success;
        self.run_state_started_at = None;
    }

    pub fn set_run_state_error(&mut self, message: impl Into<String>) {
        self.run_state = SessionRunState::Error {
            message: message.into(),
        };
        self.run_state_started_at = None;
    }

    pub fn refresh_run_state_from_selection(&mut self) {
        self.run_state = initial_run_state(&self.sessions, self.selected_session);
        self.run_state_started_at = self.run_state.is_active().then(Instant::now);
    }

    pub fn run_state_elapsed_secs(&self) -> Option<u64> {
        self.run_state_started_at
            .filter(|_| self.run_state.is_active())
            .map(|started| started.elapsed().as_secs())
    }

    pub fn toggle_tool_output_expansion(&mut self) {
        self.expanded_tool_outputs = !self.expanded_tool_outputs;
        self.status = if self.expanded_tool_outputs {
            "Expanded tool output cards".into()
        } else {
            "Collapsed tool output cards".into()
        };
    }

    pub fn persist_composer_draft_for_selected_session(&mut self) {
        let Some(session_id) = self.active_session().map(|session| session.id.clone()) else {
            return;
        };
        let text = self.composer.clone();
        if let Some(draft) = self
            .composer_drafts
            .iter_mut()
            .find(|draft| draft.session_id == session_id)
        {
            draft.text = text;
        } else if !text.is_empty() {
            self.composer_drafts
                .push(ComposerDraft { session_id, text });
        }
        self.composer_drafts.retain(|draft| !draft.text.is_empty());
    }

    pub fn load_composer_draft_for_selected_session(&mut self) {
        let Some(session_id) = self.active_session().map(|session| session.id.clone()) else {
            self.composer.clear();
            self.composer_cursor = None;
            return;
        };
        self.composer = self
            .composer_drafts
            .iter()
            .find(|draft| draft.session_id == session_id)
            .map(|draft| draft.text.clone())
            .unwrap_or_default();
        self.composer_cursor = None;
    }

    pub fn clear_current_composer_draft(&mut self) {
        let session_id = self.active_session().map(|session| session.id.clone());
        self.composer.clear();
        self.composer_cursor = None;
        if let Some(session_id) = session_id {
            self.composer_drafts
                .retain(|draft| draft.session_id != session_id);
        }
    }

    pub fn set_composer_text(&mut self, text: impl Into<String>) {
        self.composer = text.into();
        self.composer_cursor = None;
    }

    pub fn composer_cursor_index(&self) -> usize {
        self.clamp_composer_cursor(self.composer_cursor.unwrap_or(self.composer.len()))
    }

    pub fn insert_composer_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let cursor = self.composer_cursor_index();
        self.composer.insert_str(cursor, text);
        self.composer_cursor = Some(cursor + text.len());
    }

    pub fn insert_composer_char(&mut self, ch: char) {
        let cursor = self.composer_cursor_index();
        self.composer.insert(cursor, ch);
        self.composer_cursor = Some(cursor + ch.len_utf8());
    }

    pub fn delete_composer_prev_char(&mut self) {
        let cursor = self.composer_cursor_index();
        let Some(prev) = prev_char_boundary(&self.composer, cursor) else {
            self.composer_cursor = Some(0);
            return;
        };
        self.composer.drain(prev..cursor);
        self.composer_cursor = Some(prev);
    }

    pub fn delete_composer_next_char(&mut self) {
        let cursor = self.composer_cursor_index();
        let Some(next) = next_char_boundary(&self.composer, cursor) else {
            self.composer_cursor = Some(self.composer.len());
            return;
        };
        self.composer.drain(cursor..next);
        self.composer_cursor = Some(cursor);
    }

    pub fn move_composer_cursor_left(&mut self) {
        let cursor = self.composer_cursor_index();
        self.composer_cursor = Some(prev_char_boundary(&self.composer, cursor).unwrap_or(0));
    }

    pub fn move_composer_cursor_right(&mut self) {
        let cursor = self.composer_cursor_index();
        self.composer_cursor =
            Some(next_char_boundary(&self.composer, cursor).unwrap_or(self.composer.len()));
    }

    pub fn move_composer_cursor_line_start(&mut self) {
        let cursor = self.composer_cursor_index();
        let line_start = self.composer[..cursor]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        self.composer_cursor = Some(line_start);
    }

    pub fn move_composer_cursor_line_end(&mut self) {
        let cursor = self.composer_cursor_index();
        let line_end = self.composer[cursor..]
            .find('\n')
            .map(|offset| cursor + offset)
            .unwrap_or(self.composer.len());
        self.composer_cursor = Some(line_end);
    }

    pub fn move_composer_cursor_prev_word(&mut self) {
        let cursor = self.composer_cursor_index();
        self.composer_cursor = Some(prev_word_boundary(&self.composer, cursor));
    }

    pub fn move_composer_cursor_next_word(&mut self) {
        let cursor = self.composer_cursor_index();
        self.composer_cursor = Some(next_word_boundary(&self.composer, cursor));
    }

    pub fn delete_composer_prev_word(&mut self) {
        let cursor = self.composer_cursor_index();
        let start = prev_word_boundary(&self.composer, cursor);
        self.composer.drain(start..cursor);
        self.composer_cursor = Some(start);
    }

    pub fn delete_composer_next_word(&mut self) {
        let cursor = self.composer_cursor_index();
        let end = next_word_boundary(&self.composer, cursor);
        self.composer.drain(cursor..end);
        self.composer_cursor = Some(cursor);
    }

    pub fn kill_composer_to_line_end(&mut self) {
        let cursor = self.composer_cursor_index();
        let end = self.composer[cursor..]
            .find('\n')
            .map(|offset| cursor + offset)
            .unwrap_or(self.composer.len());
        self.composer.drain(cursor..end);
        self.composer_cursor = Some(cursor);
    }

    pub fn composer_presentation(&self) -> ComposerPresentation {
        composer_presentation_for_text(&self.composer)
    }

    fn clamp_composer_cursor(&self, cursor: usize) -> usize {
        let cursor = cursor.min(self.composer.len());
        if self.composer.is_char_boundary(cursor) {
            return cursor;
        }
        prev_char_boundary(&self.composer, cursor).unwrap_or(0)
    }
}

fn is_protocol_target(target: &str) -> bool {
    let target = target.trim_start();
    target
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ws://"))
        || target
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("wss://"))
        || target
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("stdio:"))
}

fn prev_char_boundary(text: &str, cursor: usize) -> Option<usize> {
    let mut cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    text[..cursor].char_indices().last().map(|(idx, _)| idx)
}

fn next_char_boundary(text: &str, cursor: usize) -> Option<usize> {
    let mut cursor = cursor.min(text.len());
    while cursor < text.len() && !text.is_char_boundary(cursor) {
        cursor += 1;
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(idx, _)| cursor + idx)
        .or_else(|| (cursor < text.len()).then_some(text.len()))
}

fn prev_word_boundary(text: &str, cursor: usize) -> usize {
    let mut idx = cursor.min(text.len());
    while let Some(prev) = prev_char_boundary(text, idx) {
        let ch = text[prev..idx].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        idx = prev;
    }
    while let Some(prev) = prev_char_boundary(text, idx) {
        let ch = text[prev..idx].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        idx = prev;
    }
    idx
}

fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut idx = cursor.min(text.len());
    while let Some(next) = next_char_boundary(text, idx) {
        let ch = text[idx..next].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        idx = next;
    }
    while let Some(next) = next_char_boundary(text, idx) {
        let ch = text[idx..next].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        idx = next;
    }
    idx
}

fn composer_presentation_for_text(text: &str) -> ComposerPresentation {
    const COLLAPSE_LINE_THRESHOLD: usize = 32;
    const COLLAPSE_CHAR_THRESHOLD: usize = 4_000;
    const PREVIEW_CHARS: usize = 88;

    if text.is_empty() {
        return ComposerPresentation::Empty;
    }

    let char_count = text.chars().count();
    let line_count = text.lines().count().max(1);
    let should_collapse =
        line_count >= COLLAPSE_LINE_THRESHOLD || char_count >= COLLAPSE_CHAR_THRESHOLD;

    if !should_collapse {
        return ComposerPresentation::Inline(text.to_string());
    }

    let summary = if line_count >= COLLAPSE_LINE_THRESHOLD {
        format!("Pasted block: {line_count} lines, {char_count} chars")
    } else {
        format!("Long prompt: {char_count} chars")
    };
    let preview_source = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("<blank paste>");

    ComposerPresentation::Collapsed(ComposerCollapse {
        summary,
        preview: truncate_chars(preview_source, PREVIEW_CHARS),
    })
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }

    let keep = max_chars.saturating_sub(4);
    let mut preview = text.chars().take(keep).collect::<String>();
    preview.push_str(" ...");
    preview
}

pub fn extract_plan_steps(app: &AppState) -> Vec<PlanStep> {
    let Some(session) = app.active_session() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    if let Some(live_reply) = session.live_reply.as_ref() {
        candidates.push(live_reply.text.as_str());
    }
    candidates.extend(
        session
            .messages
            .iter()
            .rev()
            .filter(|message| message.role.as_str() == "assistant")
            .map(|message| message.content.as_str()),
    );

    let mut plans = candidates.into_iter().filter_map(plan_steps_from_text);
    let Some(mut plan) = plans.next() else {
        return Vec::new();
    };
    for older_plan in plans {
        merge_completed_plan_steps(&mut plan, &older_plan);
    }
    plan
}

pub fn complete_plan_steps_in_text(text: &str) -> String {
    let mut in_plan = false;
    let mut changed = false;
    let mut completed_any = false;
    let mut output = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            output.push(line.to_string());
            if completed_any {
                in_plan = false;
            }
            continue;
        }

        if is_plan_heading(trimmed) {
            in_plan = true;
            output.push(line.to_string());
            continue;
        }

        if let Some(step) = plan_step_from_line(trimmed, in_plan) {
            let indent_len = line.len() - line.trim_start().len();
            let indent = &line[..indent_len];
            output.push(format!("{indent}- [x] {}", step.text));
            changed = true;
            completed_any = true;
            in_plan = true;
            continue;
        }

        output.push(line.to_string());
        if completed_any {
            in_plan = false;
        }
    }

    if changed {
        let mut joined = output.join("\n");
        if text.ends_with('\n') {
            joined.push('\n');
        }
        joined
    } else {
        text.to_string()
    }
}

fn plan_steps_from_text(text: &str) -> Option<Vec<PlanStep>> {
    let mut in_plan = false;
    let mut steps = Vec::new();
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }

        if trimmed.is_empty() {
            if in_plan && !steps.is_empty() {
                break;
            }
            continue;
        }

        if is_plan_heading(trimmed) {
            in_plan = true;
            continue;
        }

        let has_checkbox_marker = line_has_checkbox_marker(trimmed);
        if !in_plan && !has_checkbox_marker {
            continue;
        }

        if let Some(step) = plan_step_from_line(trimmed, in_plan || has_checkbox_marker) {
            steps.push(step);
            in_plan = true;
            continue;
        }

        if in_plan && !steps.is_empty() {
            break;
        }
    }

    (!steps.is_empty()).then_some(steps)
}

fn line_has_checkbox_marker(line: &str) -> bool {
    let mut rest = line.trim();
    for _ in 0..6 {
        rest = rest.trim_start();
        if strip_checkbox(rest).is_some() {
            return true;
        }
        if let Some(next) = strip_bullet(rest) {
            rest = next;
            continue;
        }
        if let Some(next) = strip_number(rest) {
            rest = next;
            continue;
        }
        break;
    }
    false
}

fn merge_completed_plan_steps(plan: &mut [PlanStep], completed_source: &[PlanStep]) {
    for step in plan.iter_mut().filter(|step| !step.completed) {
        if completed_source.iter().any(|candidate| {
            candidate.completed
                && normalize_plan_text(&candidate.text) == normalize_plan_text(&step.text)
        }) {
            step.completed = true;
        }
    }
}

fn normalize_plan_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn plan_step_from_line(line: &str, in_plan: bool) -> Option<PlanStep> {
    let mut rest = line.trim();
    let mut completed = None;
    let mut saw_marker = false;
    let mut saw_number = false;
    let mut saw_checkbox = false;
    let mut saw_plain_bullet = false;

    for _ in 0..6 {
        rest = rest.trim_start();
        if let Some((checked, next)) = strip_checkbox(rest) {
            completed = Some(checked);
            saw_marker = true;
            saw_checkbox = true;
            rest = next;
            continue;
        }
        if let Some(next) = strip_bullet(rest) {
            saw_marker = true;
            saw_plain_bullet = true;
            rest = next;
            continue;
        }
        if let Some(next) = strip_number(rest) {
            saw_marker = true;
            saw_number = true;
            rest = next;
            continue;
        }
        break;
    }

    if !saw_marker {
        return None;
    }
    if saw_plain_bullet && !saw_checkbox && !saw_number && !in_plan {
        return None;
    }

    let text = rest.trim_start_matches(['.', ')', ' ']).trim();
    if text.is_empty() || text.chars().count() > 160 {
        return None;
    }

    Some(PlanStep {
        text: text.to_string(),
        completed: completed.unwrap_or(false),
    })
}

fn strip_checkbox(line: &str) -> Option<(bool, &str)> {
    let rest = line.strip_prefix('[')?;
    let (marker, rest) = rest.split_once(']')?;
    let completed = match marker.trim() {
        "x" | "X" => true,
        "" => false,
        _ => return None,
    };
    Some((completed, rest.trim_start()))
}

fn strip_bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
}

fn strip_number(line: &str) -> Option<&str> {
    let split = line.find(['.', ')'])?;
    let (number, rest) = line.split_at(split);
    if number.is_empty() || number.len() > 3 || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let rest = rest[1..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

fn is_plan_heading(line: &str) -> bool {
    let heading = line
        .trim_start_matches('#')
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_ascii_lowercase();
    matches!(
        heading.as_str(),
        "plan"
            | "steps"
            | "next steps"
            | "implementation plan"
            | "task plan"
            | "todo"
            | "checklist"
    )
}

fn initial_run_state(sessions: &[SessionView], selected_session: usize) -> SessionRunState {
    if sessions
        .get(selected_session)
        .and_then(|session| session.live_reply.as_ref())
        .is_some()
    {
        SessionRunState::InProgress
    } else {
        SessionRunState::Idle
    }
}

fn matching_user_message_count(session: &SessionView, content: &str) -> usize {
    session
        .messages
        .iter()
        .filter(|message| message.role.as_str() == "user" && message.content == content)
        .count()
}

fn latest_user_content_for_session(
    sessions: &[SessionView],
    session_id: &SessionKey,
) -> Option<String> {
    sessions
        .iter()
        .find(|session| &session.id == session_id)
        .and_then(|session| {
            session
                .messages
                .iter()
                .rev()
                .find(|message| message.role.as_str() == "user")
                .map(|message| message.content.clone())
        })
}

fn estimated_activity_rows(item: &ActivityItem) -> usize {
    match item.kind {
        ActivityKind::Tool => {
            let preview_rows = item
                .output_preview
                .as_deref()
                .map(|output| output.lines().count().clamp(1, 4))
                .unwrap_or(1);
            4 + preview_rows
        }
        ActivityKind::Progress => {
            if item.title == "file_mutation" || item.status.starts_with("File mutation: ") {
                3
            } else {
                2
            }
        }
        ActivityKind::Approval | ActivityKind::Warning | ActivityKind::Error => 2,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedTaskContext {
    pub session_id: SessionKey,
    pub task_id: TaskId,
    pub title: String,
    pub output_tail: String,
}

pub fn task_state_label(state: TaskRuntimeState) -> &'static str {
    let wire = serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned));
    match wire.as_deref() {
        Some("pending") => "pending",
        Some("running") => "running",
        Some("completed") => "done",
        Some("failed") => "failed",
        Some("cancelled") => "cancelled",
        _ => "unknown",
    }
}

fn preview_id_from_text(text: &str) -> Option<PreviewId> {
    let lower = text.to_ascii_lowercase();
    let marker_start = ["preview_id", "preview-id", "preview id"]
        .into_iter()
        .filter_map(|marker| lower.find(marker).map(|idx| idx + marker.len()))
        .min()?;
    let suffix = &text[marker_start..];

    suffix
        .split(|ch: char| !(ch.is_ascii_hexdigit() || ch == '-'))
        .find_map(|token| {
            if token.len() < 32 {
                return None;
            }
            serde_json::from_value(serde_json::Value::String(token.to_owned())).ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use octos_core::Message;
    use octos_core::ui_protocol::{
        UiArtifactPaneItem, UiArtifactPaneSnapshot, UiGitHistoryItem, UiGitPaneSnapshot,
        UiGitStatusItem, UiWorkspacePaneEntry, UiWorkspacePaneSnapshot,
    };

    fn state_with_task(task: TaskView) -> AppState {
        AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![task],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        )
    }

    #[test]
    fn snapshot_seeds_artifacts_workspace_and_git_panes_from_mock_data() {
        let preview_id = PreviewId::new();
        let snapshot = AppUiSnapshot {
            sessions: vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "M9 protocol draft".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![TaskView {
                    id: TaskId::new(),
                    title: "protocol spike".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: Some(format!("pending preview_id: {}", preview_id.0)),
                    output_tail: "bootstrap: seeded mock session\n".into(),
                }],
                live_reply: None,
            }],
            selected_session: 0,
            status: "Mock backend ready".into(),
            target: Some("local mock snapshot".into()),
            readonly: false,
        };

        let state = AppState::from_snapshot(snapshot);

        assert!(state.artifacts.items.iter().any(|item| {
            item.title == "AppUi bootstrap snapshot" && item.source == "local mock snapshot"
        }));
        assert!(
            state
                .artifacts
                .items
                .iter()
                .any(|item| item.title == "protocol spike output tail"
                    && item.status == "bootstrap: seeded mock session")
        );
        assert!(state.artifacts.items.iter().any(|item| {
            item.title == "protocol spike diff preview" && item.status == preview_id.0.to_string()
        }));
        assert!(
            state
                .workspace
                .contract
                .iter()
                .any(|line| line.contains(APP_UI_API_V1))
        );
        assert!(
            state
                .workspace
                .entries
                .iter()
                .any(|entry| entry.label == "protocol spike" && entry.detail == "running")
        );
        assert_eq!(state.git.branch, "m9.7/mock-snapshot");
        assert!(
            state
                .git
                .history
                .iter()
                .any(|entry| entry.summary == "seed missing pane snapshots")
        );
    }

    #[test]
    fn protocol_snapshot_seeds_contract_fallbacks_when_pane_payloads_are_absent() {
        let snapshot = AppUiSnapshot {
            sessions: vec![],
            selected_session: 0,
            status: "Protocol backend connected".into(),
            target: Some("wss://example.test/ui-protocol".into()),
            readonly: true,
        };

        let state = AppState::from_snapshot(snapshot);

        assert!(state.artifacts.items.iter().any(|item| {
            item.title == "Protocol artifact stream"
                && item.status == "waiting for artifact payloads"
        }));
        assert_eq!(state.workspace.root, "wss://example.test/ui-protocol");
        assert!(
            state
                .workspace
                .contract
                .iter()
                .any(|line| line.contains("pane.snapshots.v1"))
        );
        assert!(
            state
                .workspace
                .contract
                .iter()
                .any(|line| line == "readonly launch: commands disabled")
        );
        assert_eq!(state.git.branch, "not supplied");
        assert!(
            state
                .git
                .status
                .iter()
                .any(|item| item.detail.contains("protocol snapshot"))
        );
    }

    #[test]
    fn stdio_protocol_target_stays_available_after_status_changes() {
        let mut state = AppState::new(
            vec![SessionView {
                id: SessionKey("coding:local:test".into()),
                title: "stdio".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "AppUI capabilities refreshed: 24 methods".into(),
            Some("stdio:octos serve --stdio".into()),
            false,
        );
        state.set_capabilities(UiProtocolCapabilities::new(
            &[APPUI_METHOD_PROFILE_LLM_CATALOG],
            &[],
        ));

        let ctx = state.availability_context();

        assert_eq!(ctx.runtime, RuntimeMode::Protocol);
        assert_eq!(ctx.connection, ConnectionState::Connected);
        assert!(ctx.supports_method(APPUI_METHOD_PROFILE_LLM_CATALOG));
    }

    #[test]
    fn pane_snapshot_hydrates_workspace_artifacts_and_git() {
        let mut state = AppState::new(vec![], 0, "ready".into(), None, false);
        state.apply_pane_snapshot(UiPaneSnapshot {
            session_id: SessionKey("local:test".into()),
            generated_at: None,
            workspace: Some(UiWorkspacePaneSnapshot {
                root: "/repo".into(),
                readable_roots: vec!["/repo".into()],
                writable_roots: vec!["/repo".into()],
                contract: vec!["feature pane.snapshots.v1".into()],
                entries: vec![UiWorkspacePaneEntry {
                    path: "src/lib.rs".into(),
                    label: "lib.rs".into(),
                    depth: 1,
                    kind: "file".into(),
                    detail: Some("12 KB".into()),
                }],
                limitations: Vec::new(),
            }),
            artifacts: Some(UiArtifactPaneSnapshot {
                items: vec![UiArtifactPaneItem {
                    title: "lib.rs".into(),
                    kind: "file".into(),
                    path: Some("src/lib.rs".into()),
                    uri: None,
                    source: Some("workspace".into()),
                    status: "12 KB".into(),
                    source_task_id: None,
                    preview_id: None,
                    size_bytes: Some(12_288),
                    updated_at: None,
                }],
                limitations: Vec::new(),
            }),
            git: Some(UiGitPaneSnapshot {
                repo_root: Some("/repo".into()),
                branch: Some("coding-green".into()),
                head: Some("abc1234".into()),
                clean: false,
                status: vec![UiGitStatusItem {
                    code: "M".into(),
                    path: "src/lib.rs".into(),
                    detail: "modified".into(),
                }],
                history: vec![UiGitHistoryItem {
                    commit: "abc1234".into(),
                    summary: "pane snapshots".into(),
                }],
                limitations: Vec::new(),
            }),
            limitations: Vec::new(),
        });

        assert_eq!(state.workspace.root, "/repo");
        assert_eq!(state.workspace.entries[0].label, "lib.rs");
        assert_eq!(state.artifacts.items[0].title, "lib.rs");
        assert_eq!(state.git.branch, "coding-green");
        assert_eq!(state.git.status[0].path, "src/lib.rs");
    }

    #[test]
    fn focus_cycle_includes_m9_panes_and_returns_to_sessions() {
        let mut focus = FocusPane::Sessions;
        let mut visited = Vec::new();
        for _ in 0..7 {
            visited.push(focus);
            focus = focus.next();
        }

        assert_eq!(
            visited,
            vec![
                FocusPane::Sessions,
                FocusPane::Tasks,
                FocusPane::Artifacts,
                FocusPane::Transcript,
                FocusPane::Workspace,
                FocusPane::Git,
                FocusPane::Composer,
            ]
        );
        assert_eq!(focus, FocusPane::Sessions);
    }

    #[test]
    fn active_diff_preview_id_extracts_existing_protocol_id_from_task_detail() {
        let preview_id = PreviewId::new();
        let state = state_with_task(TaskView {
            id: TaskId::new(),
            title: "diff".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: Some(format!("pending preview_id: {}", preview_id.0)),
            output_tail: String::new(),
        });

        assert_eq!(state.active_diff_preview_id(), Some(preview_id));
    }

    #[test]
    fn git_scroll_uses_top_origin_like_workspace_pane() {
        let mut git = GitPaneState::default();

        git.scroll_down(8);
        assert_eq!(git.scroll, 8);

        git.scroll_up(3);
        assert_eq!(git.scroll, 5);

        git.scroll_up(99);
        assert_eq!(git.scroll, 0);
    }

    #[test]
    fn diff_preview_result_keeps_future_status_labels_instead_of_rejecting_them() {
        let preview_id = PreviewId::new();
        let json = serde_json::json!({
            "status": "requires_refresh",
            "source": "future_cache",
            "preview": {
                "session_id": "local:test",
                "preview_id": preview_id,
                "title": "Future status",
                "files": [{
                    "path": "src/lib.rs",
                    "status": "copied",
                    "hunks": [{
                        "header": "@@ -1 +1 @@",
                        "lines": [{
                            "kind": "metadata",
                            "content": "mode change",
                            "old_line": null,
                            "new_line": null
                        }]
                    }]
                }]
            }
        });

        let result: DiffPreviewGetResult =
            serde_json::from_value(json).expect("future status labels decode");

        assert_eq!(result.status, "requires_refresh");
        assert_eq!(result.source, "future_cache");
        assert_eq!(result.preview.files[0].status, "copied");
        assert_eq!(result.preview.files[0].hunks[0].lines[0].kind, "metadata");
    }

    #[test]
    fn runtime_policy_stamp_accepts_coding_contract_extensions() {
        let json = serde_json::json!({
            "tool_policy_id": "coding-v3",
            "tool_contract_id": "codex-compatible-coding-v1",
            "tool_contract_version": "1",
            "model_toolset": "coding",
            "dynamic_tool_discovery": "enabled",
            "mcp_servers": [{
                "id": "github",
                "display_name": "GitHub",
                "status": "connected",
                "tool_count": 4
            }]
        });

        let stamp: RuntimePolicyStamp =
            serde_json::from_value(json).expect("runtime policy stamp decodes");

        assert_eq!(stamp.tool_policy_id.as_deref(), Some("coding-v3"));
        assert_eq!(
            stamp.tool_contract_id.as_deref(),
            Some("codex-compatible-coding-v1")
        );
        assert_eq!(stamp.model_toolset.as_deref(), Some("coding"));
        assert_eq!(stamp.dynamic_tool_discovery.as_deref(), Some("enabled"));
        assert_eq!(stamp.mcp_servers[0].label(), "GitHub (connected, 4 tools)");
    }

    #[test]
    fn tool_status_list_result_keeps_coding_tool_contract() {
        let json = serde_json::json!({
            "session_id": "local:test",
            "policy_id": "coding-v3",
            "coding_tool_contract": {
                "id": "codex-compatible-coding-v1",
                "version": "1",
                "feature": "coding.tool_contract.v1",
                "status": "incomplete",
                "required_tool_names": ["apply_patch", "exec_command"],
                "missing_required_tools": ["exec_command"],
                "policy": {
                    "tool_policy_id": "coding-v3",
                    "sandbox_mode": "workspace-write",
                    "approval_policy": "on-request"
                },
                "required_tools": [{
                    "name": "exec_command",
                    "category": "runtime",
                    "aliases": ["shell"],
                    "capability": "coding.exec_session.v1",
                    "policy": "approval_gated",
                    "status": "missing",
                    "backend_tool": null,
                    "detail": "backend has no exec session"
                }]
            },
            "tools": []
        });

        let result: ToolStatusListResult =
            serde_json::from_value(json).expect("tool status list decodes");
        let contract = result
            .coding_tool_contract
            .expect("coding tool contract retained");

        assert_eq!(contract.status, "incomplete");
        assert_eq!(
            contract.missing_required_tools,
            vec!["exec_command".to_string()]
        );
        assert_eq!(
            contract.policy.and_then(|policy| policy.tool_policy_id),
            Some("coding-v3".into())
        );
        assert_eq!(contract.required_tools[0].status, "missing");
        assert_eq!(
            contract.required_tools[0].detail.as_deref(),
            Some("backend has no exec session")
        );
    }

    #[test]
    fn extracted_plan_steps_normalize_numbered_markdown_checkboxes() {
        let state = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n1. [ ] Fix data model\n2) [x] Run focused tests",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        assert_eq!(
            extract_plan_steps(&state),
            vec![
                PlanStep {
                    text: "Fix data model".into(),
                    completed: false,
                },
                PlanStep {
                    text: "Run focused tests".into(),
                    completed: true,
                },
            ]
        );
    }

    #[test]
    fn plan_extraction_rejects_prose_and_long_bullets() {
        let long_line = format!(
            "Plan:\n- {}",
            "This is explanatory prose ".repeat(12).trim()
        );
        let state = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::assistant(
                        "The plan parser should not treat this explanatory paragraph as a task.",
                    ),
                    Message::assistant(long_line),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        assert!(extract_plan_steps(&state).is_empty());
    }

    #[test]
    fn plan_extraction_rejects_clarifying_question_lists() {
        let state = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Could you clarify?\n\n1. Is this a path within the current project/workspace?\n2. Or is it a system path outside the workspace?\n3. Did you mean a different directory?",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        assert!(extract_plan_steps(&state).is_empty());
    }

    #[test]
    fn completing_plan_steps_rewrites_only_real_plan_items() {
        let text = "Plan:\n1. [ ] Fix model\n2. Run tests\n\nReasoning stays unchecked.";

        assert_eq!(
            complete_plan_steps_in_text(text),
            "Plan:\n- [x] Fix model\n- [x] Run tests\n\nReasoning stays unchecked."
        );
        assert_eq!(
            complete_plan_steps_in_text("1. [ ] Fix model\n2. Run tests"),
            "- [x] Fix model\n- [x] Run tests"
        );
    }

    #[test]
    fn composer_presentation_collapses_large_pastes_without_changing_text() {
        let mut state = AppState::new(
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
        let pasted_text = std::iter::once("first pasted line".to_string())
            .chain((2..=40).map(|idx| format!("pasted line {idx}")))
            .collect::<Vec<_>>()
            .join("\n");
        state.composer = pasted_text.clone();

        let ComposerPresentation::Collapsed(collapse) = state.composer_presentation() else {
            panic!("large paste should collapse");
        };

        assert_eq!(state.composer, pasted_text);
        assert!(collapse.summary.contains("40 lines"));
        assert_eq!(collapse.preview, "first pasted line");
    }

    #[test]
    fn composer_presentation_keeps_short_prompts_inline() {
        let mut state = AppState::new(
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
        state.composer = "fix failing tests".into();

        assert_eq!(
            state.composer_presentation(),
            ComposerPresentation::Inline("fix failing tests".into())
        );
    }

    #[test]
    fn composer_inline_cursor_width_uses_last_line() {
        let presentation = ComposerPresentation::Inline("first\nsecond line".into());
        assert_eq!(presentation.cursor_width(), "second line".chars().count());
    }

    #[test]
    fn composer_inline_cursor_width_uses_display_columns_for_chinese() {
        let presentation = ComposerPresentation::Inline("first\n你好abc".into());
        assert_eq!(presentation.cursor_width(), 7);
    }

    #[test]
    fn optimistic_user_prompt_restores_missing_duplicate_at_submit_anchor() {
        let session_id = SessionKey("local:test".into());
        let mut state = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("repeat"), Message::assistant("old answer")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        state.record_submitted_user_prompt(session_id.clone(), TurnId::new(), "repeat".into());
        assert_eq!(state.sessions[0].messages[2].content, "repeat");

        let optimistic_user_messages = state.optimistic_user_messages.clone();
        let mut replayed = AppState::new(
            vec![SessionView {
                id: session_id,
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("repeat"),
                    Message::assistant("old answer"),
                    Message::assistant("server-side output without echoed prompt"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        replayed.optimistic_user_messages = optimistic_user_messages;

        replayed.restore_optimistic_user_messages();

        let messages = &replayed.sessions[0].messages;
        assert_eq!(messages[0].content, "repeat");
        assert_eq!(messages[1].content, "old answer");
        assert_eq!(messages[2].role.as_str(), "user");
        assert_eq!(messages[2].content, "repeat");
        assert_eq!(
            messages[3].content,
            "server-side output without echoed prompt"
        );
    }

    #[test]
    fn optimistic_user_prompt_drops_when_server_echo_confirms_it() {
        let session_id = SessionKey("local:test".into());
        let mut state = AppState::new(
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

        state.record_submitted_user_prompt(session_id, TurnId::new(), "confirmed prompt".into());
        assert_eq!(state.optimistic_user_messages.len(), 1);
        state.sessions[0].messages = vec![
            Message::assistant("ready"),
            Message::user("confirmed prompt"),
            Message::assistant("server echoed the prompt"),
        ];

        state.restore_optimistic_user_messages();

        assert!(state.optimistic_user_messages.is_empty());
        assert_eq!(
            state.sessions[0]
                .messages
                .iter()
                .filter(|message| message.role.as_str() == "user"
                    && message.content == "confirmed prompt")
                .count(),
            1
        );
    }

    /// M22-B: pre-flight validation surfaces the first failing field
    /// in declaration order (name → username → email) so the user
    /// fixes one thing at a time.
    #[test]
    fn validate_local_profile_reports_first_missing_field() {
        let state = OnboardingWizardState::default();
        let err = state
            .validate_local_profile()
            .expect_err("default state has no name");
        assert_eq!(err.focus_field, OnboardingLocalProfileField::Name);
        assert_eq!(err.kind, OnboardingLocalProfileErrorKind::InvalidField);
    }

    #[test]
    fn validate_local_profile_rejects_whitespace_in_username() {
        let mut state = OnboardingWizardState::default();
        state.name = "Ada Lovelace".into();
        state.username = "ada lovelace".into();
        state.email = "ada@example.com".into();
        let err = state
            .validate_local_profile()
            .expect_err("username must reject whitespace");
        assert_eq!(err.focus_field, OnboardingLocalProfileField::Username);
        assert!(
            err.message.contains("ASCII without whitespace"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn validate_local_profile_accepts_single_label_domain() {
        // The backend accepts `ada@localhost` and `dev@corp`; the
        // TUI must NOT be stricter than the server.
        let mut state = OnboardingWizardState::default();
        state.name = "Ada".into();
        state.username = "ada".into();
        state.email = "ada@localhost".into();
        assert!(state.validate_local_profile().is_ok());
    }

    #[test]
    fn validate_local_profile_rejects_malformed_email() {
        let mut state = OnboardingWizardState::default();
        state.name = "Ada".into();
        state.username = "ada".into();
        state.email = "not-an-email".into();
        let err = state.validate_local_profile().expect_err("bad email");
        assert_eq!(err.focus_field, OnboardingLocalProfileField::Email);
    }

    #[test]
    fn validate_local_profile_requires_email_to_match_backend_contract() {
        let mut state = OnboardingWizardState::default();
        state.name = "Ada".into();
        state.username = "ada".into();
        // Empty email is rejected: the current backend
        // implementation of `profile/local/create` returns
        // `profile_local_invalid_email` for `""`. The contract
        // calls email optional but the backend implementation has
        // not relaxed yet.
        let err = state
            .validate_local_profile()
            .expect_err("empty email must be rejected pre-flight");
        assert_eq!(err.focus_field, OnboardingLocalProfileField::Email);
    }

    #[test]
    fn apply_local_profile_error_routes_collision_to_username() {
        let mut state = OnboardingWizardState::default();
        state.username = "ada".into();
        state.local_profile_create_pending = true;
        state.local_profile_create_pending_username = Some("ada".into());
        state.apply_local_profile_error("profile_local_collision", "username already taken");
        let recovery = state.local_profile_recovery.expect("recovery");
        assert_eq!(recovery.kind, OnboardingLocalProfileErrorKind::Collision);
        assert_eq!(recovery.focus_field, OnboardingLocalProfileField::Username);
        assert!(recovery.message.contains("collision for 'ada'"));
        assert!(recovery.message.contains("username already taken"));
        assert!(!state.local_profile_create_pending);
        assert!(!state.local_profile_created);
    }

    #[test]
    fn apply_local_profile_error_routes_invalid_email_to_email_field() {
        let mut state = OnboardingWizardState::default();
        state.apply_local_profile_error(
            "profile_local_invalid_email",
            "profile/local/create request tui-1 failed: email must contain @",
        );
        let recovery = state.local_profile_recovery.expect("recovery");
        assert_eq!(recovery.focus_field, OnboardingLocalProfileField::Email);
        assert!(recovery.message.contains("email must contain"));
    }

    #[test]
    fn apply_local_profile_error_routes_invalid_name_to_name_field() {
        let mut state = OnboardingWizardState::default();
        state.apply_local_profile_error(
            "profile_local_invalid_name",
            "profile/local/create request tui-1 failed: name must be non-empty",
        );
        let recovery = state.local_profile_recovery.expect("recovery");
        assert_eq!(recovery.focus_field, OnboardingLocalProfileField::Name);
    }

    #[test]
    fn apply_local_profile_error_routes_invalid_username_to_username_field() {
        let mut state = OnboardingWizardState::default();
        state.apply_local_profile_error(
            "profile_local_invalid_username",
            "profile/local/create request tui-1 failed: username has whitespace",
        );
        let recovery = state.local_profile_recovery.expect("recovery");
        assert_eq!(recovery.focus_field, OnboardingLocalProfileField::Username);
    }

    #[test]
    fn strip_method_prefix_removes_jsonrpc_envelope() {
        // Helper visibility (it's a free function in the same module).
        let stripped = super::strip_method_prefix(
            "profile/local/create request tui-3 failed: username taken",
            "profile/local/create",
        );
        assert_eq!(stripped, "username taken");
    }

    #[test]
    fn apply_profile_local_create_success_clears_pending_and_recovery() {
        let mut state = OnboardingWizardState::default();
        state.local_profile_create_pending = true;
        state.local_profile_recovery = Some(OnboardingLocalProfileRecovery {
            kind: OnboardingLocalProfileErrorKind::Collision,
            focus_field: OnboardingLocalProfileField::Username,
            message: "stale".into(),
        });
        state.apply_profile_local_create(&ProfileLocalCreateResult {
            profile_id: "ada".into(),
            user_id: "ada-user".into(),
            name: "Ada Lovelace".into(),
            username: "ada".into(),
            email: "ada@example.com".into(),
            created: true,
            runtime_mode: "solo".into(),
        });
        assert!(state.local_profile_created);
        assert!(!state.local_profile_create_pending);
        assert!(state.local_profile_recovery.is_none());
    }
}
