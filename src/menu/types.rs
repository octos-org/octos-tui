use std::fmt;

use crate::menu::availability::{AvailabilityContext, CommandAvailability};
use crate::model::{
    AppUiCommand, OnboardingAction, OnboardingWizardState, ProfileLlmCatalogResult,
    ProfileLlmListResult, ProfileSkillsListResult, ProfileSkillsRegistrySearchResult,
    SessionMcpCatalog, SessionModelCatalog, SessionRuntimeStatus,
};
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MenuId(String);

impl MenuId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for MenuId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for MenuId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for MenuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    Session,
    Runtime,
    Settings,
    Help,
    Developer,
}

impl CommandCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Runtime => "Runtime",
            Self::Settings => "Settings",
            Self::Help => "Help",
            Self::Developer => "Developer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineArgMode {
    None,
    Optional,
    Required,
    PromptCompatible,
}

impl InlineArgMode {
    pub fn accepts_args(self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn requires_args(self) -> bool {
        matches!(self, Self::Required)
    }

    pub fn forwards_to_prompt(self) -> bool {
        matches!(self, Self::PromptCompatible)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandEntry {
    OpenMenu(MenuId),
    LocalAction(LocalAction),
    AppUiAction(AppUiActionKind),
    PromptTemplate(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub category: CommandCategory,
    pub availability: CommandAvailability,
    pub inline_args: InlineArgMode,
    pub entry: CommandEntry,
}

impl CommandSpec {
    pub fn matches_name(&self, candidate: &str) -> bool {
        self.name == candidate || self.aliases.contains(&candidate)
    }

    pub fn slash_name(&self) -> String {
        format!("/{}", self.name)
    }

    /// Whether a successful invocation may be persisted to the plaintext
    /// command-history file (and recalled with Up). FAIL-CLOSED: only the
    /// commands listed here are recorded; everything else — auth/credential
    /// families (`onboard`/`login`/`provider`), config-upsert that can carry
    /// tokens (`mcp`/`tools` upsert, `skills install <repo-url>`), and any newly
    /// added command — defaults to NOT recorded, so it can never leak secrets/PII
    /// to history before review. Checked on the canonical `name`, so aliases
    /// (`/auth`=`/login`, `/setup`=`/onboard`) are covered too.
    pub fn history_safe(&self) -> bool {
        matches!(
            self.name,
            "ps" | "stop"
                | "help"
                | "activity"
                | "copy"
                | "exit"
                | "model"
                | "status"
                | "cost"
                | "resume"
                | "rewind"
                | "theme"
                | "lang"
                | "thinking"
                | "scrollmode"
                | "saveconfig"
                | "vimmode"
                | "statusline"
                | "title"
                | "keymap"
                | "permissions"
                | "task"
                | "threads"
                | "turn"
                | "review"
                | "agents"
                | "goal"
                | "loop"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppUiActionKind {
    InterruptTurn,
    ApprovalScopesList,
    ModelList,
    ModelSelect,
    AuthStatus,
    AuthSendCode,
    AuthVerify,
    AuthMe,
    AuthLogout,
    ProfileLocalCreate,
    ProfileLlmCatalog,
    ProfileLlmList,
    ProfileLlmUpsert,
    ProfileLlmDelete,
    ProfileLlmSelect,
    ProfileLlmTest,
    ProfileLlmFetchModels,
    SessionStatusRead,
    ReviewStart,
    PermissionProfileList,
    PermissionProfileSet,
    ApprovalScopesClear,
    McpStatusList,
    McpConfigList,
    McpConfigUpsert,
    McpConfigDelete,
    McpConfigSetEnabled,
    McpConfigTest,
    ToolStatusList,
    ToolConfigList,
    ToolConfigSetEnabled,
    ToolConfigUpsert,
    ToolConfigDelete,
    ToolConfigTest,
    Custom {
        method: &'static str,
        mutating: bool,
    },
}

impl AppUiActionKind {
    pub fn method(self) -> &'static str {
        match self {
            Self::InterruptTurn => octos_core::ui_protocol::methods::TURN_INTERRUPT,
            Self::ApprovalScopesList => octos_core::ui_protocol::methods::APPROVAL_SCOPES_LIST,
            Self::ModelList => crate::model::APPUI_METHOD_MODEL_LIST,
            Self::ModelSelect => crate::model::APPUI_METHOD_MODEL_SELECT,
            Self::AuthStatus => crate::model::APPUI_METHOD_AUTH_STATUS,
            Self::AuthSendCode => crate::model::APPUI_METHOD_AUTH_SEND_CODE,
            Self::AuthVerify => crate::model::APPUI_METHOD_AUTH_VERIFY,
            Self::AuthMe => crate::model::APPUI_METHOD_AUTH_ME,
            Self::AuthLogout => crate::model::APPUI_METHOD_AUTH_LOGOUT,
            Self::ProfileLocalCreate => crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
            Self::ProfileLlmCatalog => crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
            Self::ProfileLlmList => crate::model::APPUI_METHOD_MODEL_LIST,
            Self::ProfileLlmUpsert => crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
            Self::ProfileLlmDelete => crate::model::APPUI_METHOD_PROFILE_LLM_DELETE,
            Self::ProfileLlmSelect => crate::model::APPUI_METHOD_MODEL_SELECT,
            Self::ProfileLlmTest => crate::model::APPUI_METHOD_PROFILE_LLM_TEST,
            Self::ProfileLlmFetchModels => crate::model::APPUI_METHOD_PROFILE_LLM_FETCH_MODELS,
            Self::SessionStatusRead => crate::model::APPUI_METHOD_SESSION_STATUS_READ,
            Self::ReviewStart => crate::model::APPUI_METHOD_REVIEW_START,
            Self::PermissionProfileList => {
                octos_core::ui_protocol::methods::PERMISSION_PROFILE_LIST
            }
            Self::PermissionProfileSet => octos_core::ui_protocol::methods::PERMISSION_PROFILE_SET,
            Self::ApprovalScopesClear => "approval/scopes/clear",
            Self::McpStatusList => crate::model::APPUI_METHOD_MCP_STATUS_LIST,
            Self::McpConfigList => crate::model::APPUI_METHOD_MCP_CONFIG_LIST,
            Self::McpConfigUpsert => crate::model::APPUI_METHOD_MCP_CONFIG_UPSERT,
            Self::McpConfigDelete => crate::model::APPUI_METHOD_MCP_CONFIG_DELETE,
            Self::McpConfigSetEnabled => crate::model::APPUI_METHOD_MCP_CONFIG_SET_ENABLED,
            Self::McpConfigTest => crate::model::APPUI_METHOD_MCP_CONFIG_TEST,
            Self::ToolStatusList => crate::model::APPUI_METHOD_TOOL_STATUS_LIST,
            Self::ToolConfigList => crate::model::APPUI_METHOD_TOOL_CONFIG_LIST,
            Self::ToolConfigSetEnabled => crate::model::APPUI_METHOD_TOOL_CONFIG_SET_ENABLED,
            Self::ToolConfigUpsert => crate::model::APPUI_METHOD_TOOL_CONFIG_UPSERT,
            Self::ToolConfigDelete => crate::model::APPUI_METHOD_TOOL_CONFIG_DELETE,
            Self::ToolConfigTest => crate::model::APPUI_METHOD_TOOL_CONFIG_TEST,
            Self::Custom { method, .. } => method,
        }
    }

    pub fn is_mutating(self) -> bool {
        match self {
            Self::InterruptTurn => true,
            Self::ApprovalScopesList
            | Self::ModelList
            | Self::SessionStatusRead
            | Self::McpStatusList
            | Self::McpConfigList
            | Self::ToolStatusList
            | Self::ToolConfigList => false,
            Self::ModelSelect => true,
            Self::ReviewStart => true,
            Self::PermissionProfileList => false,
            Self::PermissionProfileSet
            | Self::ApprovalScopesClear
            | Self::AuthSendCode
            | Self::AuthVerify
            | Self::AuthLogout
            | Self::ProfileLocalCreate
            | Self::ProfileLlmUpsert
            | Self::ProfileLlmDelete
            | Self::ProfileLlmSelect
            | Self::ProfileLlmTest
            | Self::McpConfigUpsert
            | Self::McpConfigDelete
            | Self::McpConfigSetEnabled
            | Self::McpConfigTest
            | Self::ToolConfigSetEnabled
            | Self::ToolConfigUpsert
            | Self::ToolConfigDelete
            | Self::ToolConfigTest => true,
            Self::AuthStatus
            | Self::AuthMe
            | Self::ProfileLlmCatalog
            | Self::ProfileLlmList
            | Self::ProfileLlmFetchModels => false,
            Self::Custom { mutating, .. } => mutating,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalAction {
    ShowProcessStatus,
    ActivityNavigator,
    StopActiveTurn,
    Exit,
    ShowHelp,
    SetTheme(String),
    SaveStatusLine(Vec<String>),
    SaveTerminalTitle(Vec<String>),
    SaveKeymap,
    RefreshMenu(MenuId),
    EditComposer(String),
    /// Codex Enter semantics for the slash popup: dispatch the highlighted
    /// command IMMEDIATELY (one Enter goes straight to the command's
    /// page/menu/action) instead of completing its name into the composer
    /// and requiring a second Enter. The string is the full slash draft to
    /// run (e.g. "/theme").
    RunSlashCommand(String),
    Onboarding(OnboardingAction),
    Skills,
    McpConfig,
    ToolConfig,
    /// Switch the UI language at runtime (`/lang <code>` inline, or `/lang` with
    /// no arg to open the language selection menu). The locale code is taken from
    /// the command's inline args; the handler calls `rust_i18n::set_locale` and
    /// the next frame repaints in the new language.
    SetLanguage,
    /// Set the UI language to a specific value from the `/lang` selection menu.
    SetLanguageCode(crate::cli::Lang),
    /// Set the per-session reasoning/thinking effort
    /// (`/thinking <low|medium|high|max|default>`). The level is taken from the
    /// command's inline args, stored on `AppState`, and attached to every
    /// `turn/start` for the session; `default` clears the override.
    SetThinking,
    /// Set the per-session reasoning effort to a specific level from the
    /// `/thinking` selection menu. `None` clears the override (server default).
    SetThinkingLevel(Option<octos_core::ui_protocol::ReasoningEffortLevel>),
    /// Toggle whether committed reasoning renders as a transcript block
    /// for the active session (`/thinking` display row).
    ToggleReasoningDisplay,
    /// Persist the runtime UI settings (theme/lang/scroll-mode/vim-mode) back to
    /// the launch config file (`/saveconfig`).
    SaveConfig,
    /// Toggle Vim modal editing in the composer at runtime (`/vimmode`). Flips
    /// the runtime `AppState.vim_mode`; the composer resets to Insert.
    ToggleVimMode,
    /// Switch the wheel-scroll behavior at runtime (`/scrollmode` toggles,
    /// `/scrollmode <native|pinned>` sets). Only flips the runtime
    /// `AppState.pinned_scroll`; the launch config stays the default source.
    SetScrollMode,
    /// Copy the last assistant reply for the active session to the system
    /// clipboard (`/copy`). The store stages the text on
    /// `AppState::pending_clipboard`; the event loop emits the OSC 52 escape
    /// sequence so the copy works over SSH against the fleet minis.
    CopyLastReply,
    /// Open the `/resume` session picker. Fetches `session/list` and opens the
    /// resume selection menu, which renders `Loading` until the `SessionList`
    /// result lands and refreshes it (same async pattern as `/cost`).
    OpenResumePicker,
    /// Resume a specific session chosen from the `/resume` picker: switch the
    /// active session to `session_id` and hydrate its prior transcript through
    /// the existing `session/hydrate` render path.
    ResumeSession(String),
    /// Open the `/rewind` turn picker. Snapshots the ACTIVE session's user
    /// messages (newest-first) into `AppState::rewind_turns` and opens the
    /// rewind selection menu. Unlike `/resume` this needs no fetch — the turns
    /// are already in the local transcript — so the menu renders `Ready`
    /// immediately (or `Unavailable` when there are no user turns to rewind to).
    OpenRewindPicker,
    /// Rewind the active session to an earlier user turn chosen from the
    /// `/rewind` picker. `num_turns` trailing user turns are dropped server-side
    /// via `session/rollback`; `prefill` (that turn's full text) is stashed and,
    /// once the rollback result lands, put back in the composer to edit and
    /// resend (rewind-and-edit). `session_id` is the session the picker rows
    /// were built from: dispatch refuses when it no longer matches the active
    /// session (the user switched sessions while the picker was open), so a
    /// stale pick can never roll back the wrong session's turns.
    RewindToTurn {
        session_id: String,
        num_turns: u32,
        prefill: String,
    },
    /// `/btw <question>` — ask a quick aside answered out-of-band while the
    /// current turn keeps working (no tools, ephemeral). The question is taken
    /// from the command's inline args.
    Btw,
    /// `/profiles` — refresh the on-disk profile list + default pointer into
    /// state and open the profiles surface (the picker, now a manager).
    OpenProfilesSurface,
    /// "Create a new profile" from the profiles surface: reset the create/wizard
    /// state to a clean slate (so it doesn't resume the ACTIVE profile's setup
    /// mid-session) and open onboarding at the "Name this profile" step.
    CreateNewProfile,
    /// Drill into the per-profile action menu for the given profile id.
    SelectProfileForActions(String),
    /// Set the given profile as the machine default (writes `default-profile`).
    SetProfileDefault(String),
    /// "Use this profile" from the profiles surface: switch the active session to
    /// this profile by opening (or resuming) its session in the current folder.
    SwitchToProfile(String),
    /// Open the Yes/No delete confirm for the given profile id.
    RequestDeleteProfile(String),
    /// Confirmed: delete the given profile (descriptor + data dir) from disk.
    ConfirmDeleteProfile(String),
    Custom(&'static str),
}

#[derive(Debug, Clone)]
pub struct MenuSpec {
    pub id: MenuId,
    pub title: String,
    pub subtitle: Option<String>,
    pub items: Vec<MenuItem>,
    pub tabs: Vec<MenuTab>,
    pub searchable: bool,
    pub search_placeholder: Option<String>,
    pub footer_hint: Option<String>,
    pub preview: Option<MenuPreview>,
    pub mode: MenuMode,
}

impl MenuSpec {
    pub fn new(id: impl Into<MenuId>, title: impl Into<String>, mode: MenuMode) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            items: Vec::new(),
            tabs: Vec::new(),
            searchable: false,
            search_placeholder: None,
            footer_hint: None,
            preview: None,
            mode,
        }
    }

    pub fn with_items(mut self, items: Vec<MenuItem>) -> Self {
        self.items = items;
        self
    }

    pub fn searchable(mut self, placeholder: impl Into<String>) -> Self {
        self.searchable = true;
        self.search_placeholder = Some(placeholder.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub shortcut: Option<KeyBinding>,
    pub state: MenuItemState,
    pub disabled_reason: Option<String>,
    pub action: MenuAction,
}

impl MenuItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shortcut: None,
            state: MenuItemState::default(),
            disabled_reason: None,
            action,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_shortcut(mut self, shortcut: KeyBinding) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn with_state(mut self, state: MenuItemState) -> Self {
        self.state = state;
        self
    }

    pub fn disabled(mut self, reason: impl Into<String>) -> Self {
        self.disabled_reason = Some(reason.into());
        self
    }

    pub fn maybe_disabled(self, reason: Option<String>) -> Self {
        match reason {
            Some(reason) => self.disabled(reason),
            None => self,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.disabled_reason.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuItemState {
    pub current: bool,
    pub default: bool,
    pub checked: Option<bool>,
    pub loading: bool,
    pub destructive: bool,
    pub required_valid: Option<bool>,
    /// A non-interactive display row (e.g. a section divider): rendered but
    /// skipped by menu navigation and inert on Enter. Defaults to `false`
    /// (every existing item stays selectable).
    pub non_selectable: bool,
}

impl MenuItemState {
    pub fn current() -> Self {
        Self {
            current: true,
            ..Self::default()
        }
    }

    pub fn checked(checked: bool) -> Self {
        Self {
            checked: Some(checked),
            ..Self::default()
        }
    }

    pub fn required(valid: bool) -> Self {
        Self {
            required_valid: Some(valid),
            ..Self::default()
        }
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuTab {
    pub id: String,
    pub label: String,
    pub active: bool,
    pub count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuPreview {
    Text {
        title: Option<String>,
        body: String,
    },
    KeyValues {
        title: Option<String>,
        rows: Vec<MenuPreviewRow>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuPreviewRow {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuMode {
    SingleSelect,
    MultiSelect {
        allow_reorder: bool,
        min_selected: usize,
        max_selected: Option<usize>,
    },
    Loading,
    Message,
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    OpenMenu(MenuId),
    ReplaceMenu(MenuId),
    Close,
    CloseAll,
    Local(LocalAction),
    SendAppUi(Box<AppUiCommand>),
    SubmitPrompt(String),
    Noop,
}

impl MenuAction {
    pub fn send_appui(command: AppUiCommand) -> Self {
        Self::SendAppUi(Box::new(command))
    }
}

#[derive(Debug, Clone)]
pub enum ClientEffect {
    OpenMenu(MenuId),
    ReplaceMenu(MenuId),
    CloseMenu,
    CloseAllMenus,
    Local(LocalAction),
    SendAppUi(Box<AppUiCommand>),
    SubmitPrompt(String),
    Status(String),
}

#[derive(Debug, Clone)]
pub enum MenuBuildResult {
    Ready(MenuSpec),
    Loading(MenuStatusSpec),
    Unavailable(MenuStatusSpec),
    Error(MenuStatusSpec),
}

impl MenuBuildResult {
    pub fn ready(spec: MenuSpec) -> Self {
        Self::Ready(spec)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuStatusSpec {
    pub id: MenuId,
    pub title: String,
    pub message: String,
    pub footer_hint: Option<String>,
}

impl MenuStatusSpec {
    pub fn new(
        id: impl Into<MenuId>,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            message: message.into(),
            footer_hint: None,
        }
    }
}

pub trait MenuProvider: Send + Sync {
    fn id(&self) -> MenuId;
    fn build(&self, ctx: &MenuContext<'_>) -> MenuBuildResult;

    fn on_cancel(&self, _ctx: &mut MenuContext<'_>) -> Vec<ClientEffect> {
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub struct MenuContext<'a> {
    pub availability: AvailabilityContext<'a>,
    pub app: MenuAppSnapshot<'a>,
    pub terminal: TerminalSize,
    pub theme_name: Option<&'a str>,
    pub selected_path: &'a [MenuId],
}

#[derive(Debug, Clone, Default)]
pub struct MenuAppSnapshot<'a> {
    pub status: Option<&'a str>,
    pub target: Option<&'a str>,
    pub cwd: Option<&'a str>,
    pub current_model: Option<&'a str>,
    pub current_profile: Option<&'a str>,
    /// Active session's reasoning effort, for marking the current `/thinking`
    /// menu item. `None` = server default (no per-session override).
    pub reasoning_effort: Option<octos_core::ui_protocol::ReasoningEffortLevel>,
    /// Whether the active session renders reasoning as a transcript block,
    /// for the `/thinking` display toggle row's on/off state.
    pub reasoning_display: bool,
    pub permission_profile: Option<octos_core::ui_protocol::PermissionProfileSelection>,
    pub runtime_status: Option<&'a SessionRuntimeStatus>,
    pub model_catalog: Option<&'a SessionModelCatalog>,
    pub profile_llm_catalog: Option<&'a ProfileLlmCatalogResult>,
    pub profile_llm_state: Option<&'a ProfileLlmListResult>,
    pub profile_skills: Option<&'a ProfileSkillsListResult>,
    pub profile_skill_registry: Option<&'a ProfileSkillsRegistrySearchResult>,
    pub mcp_catalog: Option<&'a SessionMcpCatalog>,
    pub tool_catalog: Option<&'a crate::model::SessionToolCatalog>,
    pub mcp_config_catalog: Option<&'a crate::model::McpConfigListResult>,
    pub tool_config_catalog: Option<&'a crate::model::ToolConfigListResult>,
    pub onboarding: Option<&'a OnboardingWizardState>,
    pub selected_session_id: Option<&'a octos_core::SessionKey>,
    pub selected_session_title: Option<&'a str>,
    pub selected_task_title: Option<&'a str>,
    pub background_task_count: usize,
    /// Current wheel-scroll mode, so the `/scrollmode` help entry can show
    /// which mode is active before the user toggles blindly.
    pub pinned_scroll: bool,
    /// Prior sessions fetched via `session/list`, mirrored from
    /// `AppState::resume_sessions` so the `/resume` picker (`resume_menu`) can
    /// render one row per session. Empty until the first fetch lands (the menu
    /// renders `Loading` in that window).
    pub resume_sessions: &'a [crate::model::ResumeSessionRow],
    /// Whether a `session/list` result has landed, mirrored from
    /// `AppState::resume_list_loaded`. Lets `resume_menu` tell a genuinely
    /// in-flight fetch (render `Loading`) apart from a completed fetch that
    /// returned zero sessions (render a "No sessions" placeholder), instead of
    /// showing `Loading` forever whenever `resume_sessions` is empty.
    pub resume_list_loaded: bool,
    /// Active-session user turns for the `/rewind` picker, mirrored from
    /// `AppState::rewind_turns` so `rewind_menu` can render one row per turn.
    /// Empty when the active session has no user messages (the menu renders
    /// `Unavailable` in that case).
    pub rewind_turns: &'a [crate::model::RewindTurnRow],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: u16,
    pub height: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuFrame {
    pub id: MenuId,
    pub selected_index: usize,
    pub search_query: String,
}

impl MenuFrame {
    pub fn new(id: impl Into<MenuId>) -> Self {
        Self {
            id: id.into(),
            selected_index: 0,
            search_query: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuStack {
    frames: Vec<MenuFrame>,
}

impl MenuStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, id: impl Into<MenuId>) {
        self.frames.push(MenuFrame::new(id));
    }

    pub fn replace(&mut self, id: impl Into<MenuId>) {
        if let Some(frame) = self.frames.last_mut() {
            *frame = MenuFrame::new(id);
        } else {
            self.open(id);
        }
    }

    pub fn close(&mut self) -> Option<MenuFrame> {
        self.frames.pop()
    }

    pub fn close_all(&mut self) {
        self.frames.clear();
    }

    pub fn active(&self) -> Option<&MenuFrame> {
        self.frames.last()
    }

    pub fn active_mut(&mut self) -> Option<&mut MenuFrame> {
        self.frames.last_mut()
    }

    pub fn is_active(&self) -> bool {
        !self.frames.is_empty()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn path(&self) -> Vec<MenuId> {
        self.frames.iter().map(|frame| frame.id.clone()).collect()
    }

    pub fn frames(&self) -> &[MenuFrame] {
        &self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_stack_restores_parent_after_child_close() {
        let mut stack = MenuStack::new();
        stack.open("root");
        stack.open("child");

        assert_eq!(stack.active().map(|frame| frame.id.as_str()), Some("child"));
        assert_eq!(
            stack.close().map(|frame| frame.id),
            Some(MenuId::from("child"))
        );
        assert_eq!(stack.active().map(|frame| frame.id.as_str()), Some("root"));
    }

    #[test]
    fn replace_opens_when_stack_is_empty() {
        let mut stack = MenuStack::new();
        stack.replace("status");

        assert_eq!(stack.len(), 1);
        assert_eq!(
            stack.active().map(|frame| frame.id.as_str()),
            Some("status")
        );
    }
}
