use std::time::Instant;

use octos_core::app_ui::{APP_UI_API_V1, AppUiLiveReply, AppUiSession, AppUiSnapshot, AppUiTask};
use octos_core::ui_protocol::{
    ApprovalDecision, ApprovalId, ApprovalRenderHints, ApprovalRequestedEvent,
    ApprovalTypedDetails, OutputCursor, PreviewId, TaskRuntimeState, TurnId, UiPaneSnapshot,
    UiProtocolCapabilities, approval_scopes,
};
use octos_core::{Message, SessionKey, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::menu::{
    AvailabilityContext, CapabilitySet, ConnectionState, MenuBuildResult, MenuStack, RuntimeMode,
    TaskActivity,
};
use crate::permission_profile::PermissionProfileSelection;

pub type LiveReply = AppUiLiveReply;
pub type SessionView = AppUiSession;
pub type TaskView = AppUiTask;

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
    pub task_output_cursors: Vec<TaskOutputCursor>,
    pub diff_preview: DiffPreviewPaneState,
    pub activity: Vec<ActivityItem>,
    pub expanded_tool_outputs: bool,
    pub menu_stack: MenuStack,
    pub active_menu: Option<MenuBuildResult>,
    pub capabilities: Option<CapabilitySet>,
    pub permission_profiles: Vec<SessionPermissionProfile>,
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
pub struct SessionPermissionProfile {
    pub session_id: SessionKey,
    pub current: PermissionProfileSelection,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutputCursor {
    pub session_id: SessionKey,
    pub task_id: TaskId,
    pub cursor: OutputCursor,
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
        *self = Self {
            active: true,
            loading: true,
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
        self.active = true;
        self.loading = false;
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
            task_output_cursors: Vec::new(),
            diff_preview: DiffPreviewPaneState::default(),
            activity: Vec::new(),
            expanded_tool_outputs: false,
            menu_stack: MenuStack::new(),
            active_menu: None,
            capabilities: None,
            permission_profiles: Vec::new(),
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
            runtime: if self
                .target
                .as_deref()
                .is_some_and(|target| target.starts_with("ws://") || target.starts_with("wss://"))
            {
                RuntimeMode::Protocol
            } else {
                RuntimeMode::Mock
            },
            connection: if self
                .target
                .as_deref()
                .is_some_and(|target| target.starts_with("ws://") || target.starts_with("wss://"))
            {
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
        self.activity.push(item);
        if self.activity.len() > MAX_ACTIVITY_ITEMS {
            let excess = self.activity.len() - MAX_ACTIVITY_ITEMS;
            self.activity.drain(0..excess);
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
            return;
        };
        self.composer = self
            .composer_drafts
            .iter()
            .find(|draft| draft.session_id == session_id)
            .map(|draft| draft.text.clone())
            .unwrap_or_default();
    }

    pub fn clear_current_composer_draft(&mut self) {
        let session_id = self.active_session().map(|session| session.id.clone());
        self.composer.clear();
        if let Some(session_id) = session_id {
            self.composer_drafts
                .retain(|draft| draft.session_id != session_id);
        }
    }
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

        if let Some(step) = plan_step_from_line(trimmed, in_plan) {
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
}
