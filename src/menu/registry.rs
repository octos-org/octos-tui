use std::collections::BTreeMap;
use std::fmt;

use octos_core::ui_protocol::methods;

use crate::menu::availability::{
    AvailabilityContext, AvailabilityStatus, CommandAvailability, SessionRequirement,
    TaskRequirement, evaluate_command,
};
use crate::menu::types::{
    AppUiActionKind, CommandCategory, CommandEntry, CommandSpec, InlineArgMode, LocalAction,
    MenuBuildResult, MenuContext, MenuId, MenuProvider, MenuStatusSpec,
};

pub const MENU_HELP: &str = "help";
pub const MENU_ONBOARD: &str = "onboard";
pub const MENU_ONBOARD_LANGUAGE: &str = "onboard-language";
pub const MENU_ONBOARD_FAMILY: &str = "onboard-family";
pub const MENU_ONBOARD_MODEL: &str = "onboard-model";
pub const MENU_ONBOARD_ROUTE: &str = "onboard-route";
/// UX2 B.2: workspace staging + validation lives on its OWN onboarding step
/// screen (the "Set Up LLM Provider" menu now configures provider/model only).
/// Retained for older servers without the launch flow; on a launch-flow server
/// the provider step ends at [`MENU_ONBOARD_DONE`] instead.
pub const MENU_ONBOARD_WORKSPACE: &str = "onboard-workspace";
/// Terminal onboarding screen on a launch-flow server (Model A): the profile +
/// provider are set, so onboarding ends here with launch instructions instead
/// of staging a workspace / opening a session — launch-time activation
/// (`launch/resolve`) opens the session on the next start. See
/// `onboarding_done_menu`.
pub const MENU_ONBOARD_DONE: &str = "onboard-done";
/// Phase 3 startup picker: "attach which profile?" shown at launch when more
/// than one local profile exists and no `--profile-id` was pinned.
pub const MENU_PROFILE_PICKER: &str = "profile-picker";
/// Per-profile action drill-in from the profiles surface: use / set-default /
/// delete for the profile the user selected.
pub const MENU_PROFILE_ACTIONS: &str = "profile-actions";
/// Yes/No confirm for the destructive profile delete.
pub const MENU_PROFILE_DELETE_CONFIRM: &str = "profile-delete-confirm";
/// Per-project launch prompt (Model A): the Activate / CrossProfile choice
/// raised from a `launch/resolve` decision. See `launch_prompt_menu`.
pub const MENU_LAUNCH_PROMPT: &str = "launch-prompt";
pub const MENU_LOGIN: &str = "login";
/// Mid-session staged model-config surface: the `/model` → "Add a model" flow
/// and the (menu-hidden) `/add-model` command. Replaced the retired
/// `MENU_PROVIDER` ("provider") dashboard, which flat-enumerated the catalog.
pub const MENU_MODEL_CONFIG: &str = "model-config";
/// `/model` → "Remove a model…" picker (configured models only).
pub const MENU_MODEL_REMOVE: &str = "model-remove";
/// Yes/No confirm for removing the staged model via `profile/llm/delete`.
pub const MENU_MODEL_REMOVE_CONFIRM: &str = "model-remove-confirm";
pub const MENU_COMPACT_CONFIRM: &str = "compact-confirm";
pub const MENU_CONTEXT: &str = "context";
pub const MENU_MODEL: &str = "model";
/// Named provider lanes (`sub_providers`) for the deep_research pipeline lane —
/// the `/research` menu lists them and `/research add|rm` mutates them.
pub const MENU_RESEARCH: &str = "research";
/// Yes/No confirm for removing a staged research lane via
/// `profile/sub_providers/remove`.
pub const MENU_RESEARCH_REMOVE_CONFIRM: &str = "research-remove-confirm";
/// Lane-key picker for the wizard's research-lane Save: deep_research requests
/// lanes by the literal keys `cheap`/`strong` (`contract_for`), so the Save
/// must land on one of those — a family-id key would never be routed to.
pub const MENU_RESEARCH_LANE_KEY: &str = "research-lane-key";
/// `/undo` snapshot picker (#1768).
pub const MENU_UNDO: &str = "undo";
/// #324: Alt+S session switcher popup (open sessions, live/unread badges).
pub const MENU_SESSIONS: &str = "sessions";
/// Yes/No confirm for restoring the staged snapshot via `snapshot/restore`.
pub const MENU_UNDO_CONFIRM: &str = "undo-confirm";
pub const MENU_COST: &str = "cost";
/// `/resume` session picker menu.
pub const MENU_RESUME: &str = "resume";
pub const MENU_AGENTS: &str = "agents";
/// `/rewind` turn picker menu.
pub const MENU_REWIND: &str = "rewind";
pub const MENU_STATUS: &str = "status";
pub const MENU_THEME: &str = "theme";
/// Reasoning/thinking effort selection menu (opened by `/thinking` with no arg).
pub const MENU_THINKING: &str = "thinking";
/// UI language selection menu (opened by `/lang` with no arg).
pub const MENU_LANG: &str = "lang";
pub const MENU_STATUS_LINE: &str = "statusline";
pub const MENU_TITLE: &str = "title";
pub const MENU_KEYMAP: &str = "keymap";
pub const MENU_PERMISSIONS: &str = "permissions";
pub const MENU_MCP: &str = "mcp";
pub const MENU_TOOL_SETTINGS: &str = "tool-settings";
pub const MENU_SKILLS: &str = "skills";
/// `@` composer file picker (#363): searchable path list over the workspace
/// file tree; selecting inserts the relative path at the composer cursor.
/// Opened by typing `@` at a word boundary in the composer — no slash command.
pub const MENU_FILE_PICKER: &str = "file-picker";

pub const APPUI_METHOD_MODEL_LIST: &str = crate::model::APPUI_METHOD_MODEL_LIST;
pub const APPUI_METHOD_MODEL_SELECT: &str = crate::model::APPUI_METHOD_MODEL_SELECT;
pub const APPUI_METHOD_SESSION_STATUS_READ: &str = crate::model::APPUI_METHOD_SESSION_STATUS_READ;
pub const APPUI_METHOD_SESSION_COMPACT: &str = crate::model::APPUI_METHOD_SESSION_COMPACT;
pub const APPUI_METHOD_SESSION_COMPACT_MODE_SET: &str =
    crate::model::APPUI_METHOD_SESSION_COMPACT_MODE_SET;
pub const APPUI_METHOD_PERMISSION_PROFILE_LIST: &str = "permission/profile/list";
pub const APPUI_METHOD_PERMISSION_PROFILE_SET: &str = "permission/profile/set";
pub const APPUI_METHOD_APPROVAL_SCOPES_CLEAR: &str = "approval/scopes/clear";
pub const APPUI_METHOD_MCP_STATUS_LIST: &str = crate::model::APPUI_METHOD_MCP_STATUS_LIST;
pub const APPUI_METHOD_TOOL_STATUS_LIST: &str = crate::model::APPUI_METHOD_TOOL_STATUS_LIST;
pub const APPUI_METHOD_MCP_CONFIG_LIST: &str = crate::model::APPUI_METHOD_MCP_CONFIG_LIST;
pub const APPUI_METHOD_MCP_CONFIG_UPSERT: &str = crate::model::APPUI_METHOD_MCP_CONFIG_UPSERT;
pub const APPUI_METHOD_MCP_CONFIG_DELETE: &str = crate::model::APPUI_METHOD_MCP_CONFIG_DELETE;
pub const APPUI_METHOD_MCP_CONFIG_SET_ENABLED: &str =
    crate::model::APPUI_METHOD_MCP_CONFIG_SET_ENABLED;
pub const APPUI_METHOD_MCP_CONFIG_TEST: &str = crate::model::APPUI_METHOD_MCP_CONFIG_TEST;
pub const APPUI_METHOD_TOOL_CONFIG_LIST: &str = crate::model::APPUI_METHOD_TOOL_CONFIG_LIST;
pub const APPUI_METHOD_TOOL_CONFIG_SET_ENABLED: &str =
    crate::model::APPUI_METHOD_TOOL_CONFIG_SET_ENABLED;
pub const APPUI_METHOD_TOOL_CONFIG_UPSERT: &str = crate::model::APPUI_METHOD_TOOL_CONFIG_UPSERT;
pub const APPUI_METHOD_TOOL_CONFIG_DELETE: &str = crate::model::APPUI_METHOD_TOOL_CONFIG_DELETE;
pub const APPUI_METHOD_TOOL_CONFIG_TEST: &str = crate::model::APPUI_METHOD_TOOL_CONFIG_TEST;
pub const APPUI_METHOD_AUTH_STATUS: &str = crate::model::APPUI_METHOD_AUTH_STATUS;
pub const APPUI_METHOD_AUTH_SEND_CODE: &str = crate::model::APPUI_METHOD_AUTH_SEND_CODE;
pub const APPUI_METHOD_AUTH_VERIFY: &str = crate::model::APPUI_METHOD_AUTH_VERIFY;
pub const APPUI_METHOD_AUTH_ME: &str = crate::model::APPUI_METHOD_AUTH_ME;
pub const APPUI_METHOD_AUTH_LOGOUT: &str = crate::model::APPUI_METHOD_AUTH_LOGOUT;
pub const APPUI_METHOD_PROFILE_LOCAL_CREATE: &str = crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE;
pub const APPUI_METHOD_PROFILE_LLM_CATALOG: &str = crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG;
pub const APPUI_METHOD_PROFILE_LLM_UPSERT: &str = crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT;
pub const APPUI_METHOD_PROFILE_LLM_DELETE: &str = crate::model::APPUI_METHOD_PROFILE_LLM_DELETE;
pub const APPUI_METHOD_PROFILE_LLM_TEST: &str = crate::model::APPUI_METHOD_PROFILE_LLM_TEST;
pub const APPUI_METHOD_PROFILE_LLM_FETCH_MODELS: &str =
    crate::model::APPUI_METHOD_PROFILE_LLM_FETCH_MODELS;
pub const APPUI_METHOD_PROFILE_SKILLS_LIST: &str = crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST;
pub const APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH: &str =
    crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH;
pub const APPUI_METHOD_PROFILE_SKILLS_INSTALL: &str =
    crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL;
pub const APPUI_METHOD_PROFILE_SKILLS_REMOVE: &str =
    crate::model::APPUI_METHOD_PROFILE_SKILLS_REMOVE;
pub const APPUI_LOGIN_MENU_METHODS_ANY: &[&str] = &[
    APPUI_METHOD_AUTH_STATUS,
    APPUI_METHOD_AUTH_SEND_CODE,
    APPUI_METHOD_AUTH_VERIFY,
    APPUI_METHOD_AUTH_ME,
    APPUI_METHOD_AUTH_LOGOUT,
];
pub const APPUI_PROVIDER_MENU_METHODS_ANY: &[&str] = &[
    APPUI_METHOD_PROFILE_LLM_CATALOG,
    APPUI_METHOD_MODEL_LIST,
    APPUI_METHOD_PROFILE_LLM_UPSERT,
    APPUI_METHOD_PROFILE_LLM_DELETE,
    APPUI_METHOD_MODEL_SELECT,
    APPUI_METHOD_PROFILE_LLM_TEST,
];
pub const APPUI_ONBOARDING_METHODS_ANY: &[&str] = &[
    APPUI_METHOD_PROFILE_LOCAL_CREATE,
    APPUI_METHOD_AUTH_STATUS,
    APPUI_METHOD_AUTH_SEND_CODE,
    APPUI_METHOD_AUTH_VERIFY,
    APPUI_METHOD_AUTH_ME,
    APPUI_METHOD_PROFILE_LLM_CATALOG,
    APPUI_METHOD_MODEL_LIST,
    APPUI_METHOD_PROFILE_LLM_UPSERT,
    APPUI_METHOD_PROFILE_LLM_TEST,
    APPUI_METHOD_PROFILE_LLM_FETCH_MODELS,
];
/// M22-A first-launch trigger: methods that, when advertised, mean the
/// backend can drive a local solo profile creation flow without OTP.
/// Auto-open of onboarding on first launch requires advertising at
/// least one of these (i.e. `profile/local/create`).
pub const APPUI_FIRST_LAUNCH_LOCAL_SOLO_METHODS: &[&str] = &[APPUI_METHOD_PROFILE_LOCAL_CREATE];
/// M22-A first-launch trigger: methods that, when ALL advertised,
/// mean the backend can drive legacy email-OTP onboarding. Provider-
/// only capability (e.g. `profile/llm/catalog`) MUST NOT trigger
/// onboarding on first launch — without a profile-creation method the
/// user has nothing to onboard into.
///
/// `auth/me` is required: after `auth/verify` succeeds, the wizard
/// follow-up unconditionally calls `auth/me` to resolve the profile id
/// (see `Store::apply_client_event` for the `AuthVerify` branch). Without
/// it the user would be stranded post-OTP with no profile binding.
pub const APPUI_FIRST_LAUNCH_LEGACY_AUTH_METHODS: &[&str] = &[
    APPUI_METHOD_AUTH_SEND_CODE,
    APPUI_METHOD_AUTH_VERIFY,
    APPUI_METHOD_AUTH_ME,
];
pub const APPUI_PERMISSION_MENU_METHODS_ANY: &[&str] = &[
    methods::APPROVAL_SCOPES_LIST,
    APPUI_METHOD_PERMISSION_PROFILE_LIST,
    APPUI_METHOD_PERMISSION_PROFILE_SET,
    APPUI_METHOD_APPROVAL_SCOPES_CLEAR,
];
pub const APPUI_MCP_MENU_METHODS_ANY: &[&str] = &[
    APPUI_METHOD_MCP_CONFIG_LIST,
    APPUI_METHOD_MCP_CONFIG_UPSERT,
    APPUI_METHOD_MCP_CONFIG_DELETE,
    APPUI_METHOD_MCP_CONFIG_SET_ENABLED,
    APPUI_METHOD_MCP_CONFIG_TEST,
    APPUI_METHOD_MCP_STATUS_LIST,
];
pub const APPUI_TOOL_SETTINGS_MENU_METHODS_ANY: &[&str] = &[
    APPUI_METHOD_TOOL_CONFIG_LIST,
    APPUI_METHOD_TOOL_CONFIG_SET_ENABLED,
    APPUI_METHOD_TOOL_CONFIG_UPSERT,
    APPUI_METHOD_TOOL_CONFIG_DELETE,
    APPUI_METHOD_TOOL_CONFIG_TEST,
    APPUI_METHOD_TOOL_STATUS_LIST,
];
pub const APPUI_SKILLS_MENU_METHODS_ANY: &[&str] = &[
    APPUI_METHOD_PROFILE_SKILLS_LIST,
    APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
    APPUI_METHOD_PROFILE_SKILLS_INSTALL,
    APPUI_METHOD_PROFILE_SKILLS_REMOVE,
];
/// `/resume` is gated on the server advertising BOTH `session/list` AND
/// `session/hydrate` (ALL, not any-of): the picker fetches prior sessions via
/// `session/list`, and picking a row loads that session via `session/hydrate`.
/// A server that advertised list-but-not-hydrate would let a selection emit an
/// unsupported `session/hydrate` RPC, so the command hides unless both land.
pub const APPUI_RESUME_MENU_METHODS_ALL: &[&str] =
    &[methods::SESSION_LIST, methods::SESSION_HYDRATE];
/// `/btw` is gated on the server advertising `session/btw` — the out-of-band
/// aside answer; older servers hide the command instead of erroring on send.
pub const APPUI_BTW_METHODS_ALL: &[&str] = &[methods::SESSION_BTW];
/// `/rewind` is gated on the server advertising `session/rollback`; without it
/// there is no way to drop the later turns, so the command hides.
pub const APPUI_REWIND_MENU_METHODS_ANY: &[&str] =
    &[octos_core::ui_protocol::methods::SESSION_ROLLBACK];

/// M15-E (UPCR-2026-021) required capability feature for the
/// combined `/agents` `/goal` `/loop` surface. Clients MUST gate
/// every menu entry, slash command, and dispatch on
/// `coding.autonomy.v1`; old servers must hide the controls instead
/// of being probed for unsupported methods.
pub const APPUI_FEATURE_CODING_AUTONOMY_V1: &str = crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1;
pub const APPUI_FEATURE_TASK_ARTIFACTS_V1: &str = crate::model::APPUI_FEATURE_TASK_ARTIFACTS_V1;
pub const APPUI_TASK_ARTIFACT_MENU_METHODS_ANY: &[&str] =
    &[crate::model::APPUI_METHOD_TASK_ARTIFACT_READ];
pub const APPUI_FEATURE_THREAD_GRAPH_V1: &str = crate::model::APPUI_FEATURE_THREAD_GRAPH_V1;
pub const APPUI_THREAD_GRAPH_MENU_METHODS_ANY: &[&str] =
    &[crate::model::APPUI_METHOD_THREAD_GRAPH_GET];
pub const APPUI_FEATURE_TURN_STATE_GET_V1: &str = crate::model::APPUI_FEATURE_TURN_STATE_GET_V1;
pub const APPUI_TURN_STATE_MENU_METHODS_ANY: &[&str] = &[crate::model::APPUI_METHOD_TURN_STATE_GET];
pub const APPUI_FEATURE_REVIEW_START_V1: &str = crate::model::APPUI_FEATURE_REVIEW_START_V1;
pub const APPUI_REVIEW_START_METHODS_ANY: &[&str] = &[crate::model::APPUI_METHOD_REVIEW_START];
pub const APPUI_AGENTS_MENU_METHODS_ANY: &[&str] = &[
    crate::model::APPUI_METHOD_AGENT_LIST,
    crate::model::APPUI_METHOD_AGENT_STATUS_READ,
    crate::model::APPUI_METHOD_AGENT_OUTPUT_READ,
    crate::model::APPUI_METHOD_AGENT_ARTIFACT_LIST,
    crate::model::APPUI_METHOD_AGENT_ARTIFACT_READ,
    crate::model::APPUI_METHOD_AGENT_INTERRUPT,
    crate::model::APPUI_METHOD_AGENT_CLOSE,
];
pub const APPUI_GOAL_MENU_METHODS_ANY: &[&str] = &[
    crate::model::APPUI_METHOD_SESSION_GOAL_GET,
    crate::model::APPUI_METHOD_SESSION_GOAL_SET,
    crate::model::APPUI_METHOD_SESSION_GOAL_CLEAR,
];
pub const APPUI_LOOP_MENU_METHODS_ANY: &[&str] = &[
    crate::model::APPUI_METHOD_LOOP_CREATE,
    crate::model::APPUI_METHOD_LOOP_LIST,
    crate::model::APPUI_METHOD_LOOP_DELETE,
    crate::model::APPUI_METHOD_LOOP_PAUSE,
    crate::model::APPUI_METHOD_LOOP_RESUME,
    crate::model::APPUI_METHOD_LOOP_FIRE_NOW,
];
const AUTONOMY_FEATURES: &[&str] = &[APPUI_FEATURE_CODING_AUTONOMY_V1];
const TASK_ARTIFACT_FEATURES: &[&str] = &[APPUI_FEATURE_TASK_ARTIFACTS_V1];
const THREAD_GRAPH_FEATURES: &[&str] = &[APPUI_FEATURE_THREAD_GRAPH_V1];
const TURN_STATE_FEATURES: &[&str] = &[APPUI_FEATURE_TURN_STATE_GET_V1];
const REVIEW_START_FEATURES: &[&str] = &[APPUI_FEATURE_REVIEW_START_V1];

#[derive(Debug, Clone, Default)]
pub struct CommandRegistry {
    commands: Vec<CommandSpec>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_core_commands() -> Self {
        let mut registry = Self::new();
        for command in core_command_specs() {
            registry
                .register(command)
                .expect("core slash command identifiers are unique");
        }
        registry
    }

    pub fn register(&mut self, command: CommandSpec) -> Result<(), CommandRegistryError> {
        validate_command_identifier(command.name)?;
        if self.find(command.name).is_some() {
            return Err(CommandRegistryError::DuplicateIdentifier {
                identifier: command.name.to_owned(),
            });
        }

        for (index, alias) in command.aliases.iter().enumerate() {
            validate_command_identifier(alias)?;
            if command.name == *alias
                || command.aliases[..index].contains(alias)
                || self.find(alias).is_some()
            {
                return Err(CommandRegistryError::DuplicateIdentifier {
                    identifier: (*alias).to_owned(),
                });
            }
        }

        self.commands.push(command);
        Ok(())
    }

    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    pub fn find(&self, name: &str) -> Option<&CommandSpec> {
        let name = name.strip_prefix('/').unwrap_or(name);
        self.commands
            .iter()
            .find(|command| command.matches_name(name))
    }

    pub fn resolve<'registry, 'input>(
        &'registry self,
        input: &'input str,
    ) -> CommandResolution<'registry, 'input> {
        let Some(invocation) = CommandInvocation::parse(input) else {
            return CommandResolution::NotCommand;
        };
        if invocation.name.is_empty() {
            return CommandResolution::EmptyCommand;
        }

        match self.find(invocation.name) {
            Some(command) => CommandResolution::Found {
                command,
                invocation,
            },
            None => CommandResolution::Unknown { invocation },
        }
    }

    pub fn evaluate(
        &self,
        command: &CommandSpec,
        ctx: &AvailabilityContext<'_>,
    ) -> AvailabilityStatus {
        evaluate_command(command, ctx)
    }

    pub fn available_commands<'a>(&'a self, ctx: &AvailabilityContext<'_>) -> Vec<&'a CommandSpec> {
        self.commands
            .iter()
            .filter(|command| self.evaluate(command, ctx).is_available())
            .collect()
    }

    pub fn visible_commands<'a>(
        &'a self,
        ctx: &AvailabilityContext<'_>,
    ) -> Vec<VisibleCommand<'a>> {
        self.commands
            .iter()
            .filter_map(|command| {
                // `menu_hidden` commands stay dispatchable (resolved by name) but
                // are omitted from the `/` menu listing.
                if command.availability.menu_hidden {
                    return None;
                }
                let availability = self.evaluate(command, ctx);
                availability.is_visible().then_some(VisibleCommand {
                    command,
                    availability,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInvocation<'a> {
    pub name: &'a str,
    pub args: &'a str,
}

impl<'a> CommandInvocation<'a> {
    pub fn parse(input: &'a str) -> Option<Self> {
        let trimmed = input.trim_start();
        let command = trimmed.strip_prefix('/')?;
        let Some(split_at) = command.find(char::is_whitespace) else {
            return Some(Self {
                name: command,
                args: "",
            });
        };

        let (name, rest) = command.split_at(split_at);
        Some(Self {
            name,
            args: rest.trim_start(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandResolution<'registry, 'input> {
    NotCommand,
    EmptyCommand,
    Found {
        command: &'registry CommandSpec,
        invocation: CommandInvocation<'input>,
    },
    Unknown {
        invocation: CommandInvocation<'input>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisibleCommand<'a> {
    pub command: &'a CommandSpec,
    pub availability: AvailabilityStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRegistryError {
    InvalidIdentifier { identifier: String, reason: String },
    DuplicateIdentifier { identifier: String },
}

impl fmt::Display for CommandRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { identifier, reason } => {
                write!(f, "invalid command identifier `{identifier}`: {reason}")
            }
            Self::DuplicateIdentifier { identifier } => {
                write!(f, "duplicate command identifier `{identifier}`")
            }
        }
    }
}

impl std::error::Error for CommandRegistryError {}

fn validate_command_identifier(identifier: &str) -> Result<(), CommandRegistryError> {
    if identifier.is_empty() {
        return Err(CommandRegistryError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier cannot be empty".into(),
        });
    }
    if identifier.starts_with('/') {
        return Err(CommandRegistryError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier must not include the leading slash".into(),
        });
    }
    if identifier.chars().any(char::is_whitespace) {
        return Err(CommandRegistryError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier must not contain whitespace".into(),
        });
    }
    Ok(())
}

#[derive(Default)]
pub struct MenuRegistry {
    providers: BTreeMap<MenuId, Box<dyn MenuProvider>>,
}

impl MenuRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider<P>(&mut self, provider: P) -> Result<(), MenuRegistryError>
    where
        P: MenuProvider + 'static,
    {
        let id = provider.id();
        if self.providers.contains_key(&id) {
            return Err(MenuRegistryError::DuplicateProvider { id });
        }
        self.providers.insert(id, Box::new(provider));
        Ok(())
    }

    pub fn provider(&self, id: &MenuId) -> Option<&dyn MenuProvider> {
        self.providers.get(id).map(Box::as_ref)
    }

    pub fn contains(&self, id: &MenuId) -> bool {
        self.providers.contains_key(id)
    }

    pub fn build(&self, id: &MenuId, ctx: &MenuContext<'_>) -> MenuBuildResult {
        self.provider(id)
            .map(|provider| provider.build(ctx))
            .unwrap_or_else(|| {
                MenuBuildResult::Unavailable(MenuStatusSpec::new(
                    id.clone(),
                    t!("menu.unavailable"),
                    format!("No menu provider is registered for `{id}`."),
                ))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuRegistryError {
    DuplicateProvider { id: MenuId },
}

impl fmt::Display for MenuRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider { id } => {
                write!(f, "duplicate menu provider `{id}`")
            }
        }
    }
}

impl std::error::Error for MenuRegistryError {}

pub fn core_command_specs() -> Vec<CommandSpec> {
    let stop_availability = CommandAvailability::local_mutating();

    vec![
        CommandSpec {
            name: "ps",
            aliases: &["tasks"],
            description: "command.ps.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::ShowProcessStatus),
        },
        CommandSpec {
            name: "stop",
            // `esc` is an alias so a user who types `/esc` (a natural guess for
            // "escape/cancel this turn") hits the same interrupt path as the
            // Esc key, Ctrl-C, and `/stop`. Without it `/esc` was an unknown
            // command and the turn kept running.
            aliases: &["interrupt", "esc"],
            description: "command.stop.desc",
            category: CommandCategory::Runtime,
            availability: stop_availability,
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::StopActiveTurn),
        },
        CommandSpec {
            name: "help",
            aliases: &["?", "commands"],
            description: "command.help.desc",
            category: CommandCategory::Help,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_HELP)),
        },
        // Ordered after the `ps`/`stop`/`help` trio so the unknown-command
        // "Try ..." hint (first 3 visible commands) is unchanged.
        CommandSpec {
            name: "activity",
            aliases: &["act"],
            description: "command.activity.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::ActivityNavigator),
        },
        CommandSpec {
            name: "copy",
            aliases: &["yank"],
            description: "command.copy.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::CopyLastReply),
        },
        CommandSpec {
            name: "exit",
            aliases: &["quit"],
            description: "command.exit.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::Exit),
        },
        // The full onboarding wizard stays dispatchable (first-launch drives it,
        // and `/onboard <field>` inline sub-forms still resolve by name) but is
        // hidden from a normal session's `/` menu — model changes go through the
        // focused `/add-model` command below instead.
        CommandSpec {
            name: "onboard",
            aliases: &["setup", "wizard"],
            description: "command.onboard.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_ONBOARDING_METHODS_ANY)
                .hidden_from_menu(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Onboarding(
                crate::model::OnboardingAction::Open,
            )),
        },
        CommandSpec {
            name: "login",
            aliases: &["auth"],
            description: "command.login.desc",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_LOGIN_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Onboarding(
                crate::model::OnboardingAction::OpenLogin,
            )),
        },
        // The focused "add / change the profile's model" flow — the model-adding
        // part of onboarding (provider family -> model -> route -> save), lifted
        // out of the wizard. `provider`/`providers` remain aliases so existing
        // muscle memory and `/provider <sub>` inline forms keep working.
        // Moved into `/model` as its "Add a model" row: hidden from the `/`
        // popup (same treatment as `/onboard`) but still dispatchable by name
        // for muscle memory and the inline verbs (`/add-model key|test|save|
        // fallback|...`). Opens the staged model-config surface.
        CommandSpec {
            name: "add-model",
            aliases: &["provider", "providers", "add_model"],
            description: "command.add_model.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_PROVIDER_MENU_METHODS_ANY)
                .hidden_from_menu(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Onboarding(
                crate::model::OnboardingAction::OpenProvider,
            )),
        },
        CommandSpec {
            name: "model",
            aliases: &[],
            description: "command.model.desc",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_when_capabilities(&[APPUI_METHOD_MODEL_LIST]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_MODEL)),
        },
        // `/research` — manage the named provider lanes (`sub_providers`) that
        // back the isolated deep_research pipeline router. Bare opens the lanes
        // menu; `add`/`rm` mutate a lane inline (Custom, like the autonomy
        // verbs). Gated on the sub_providers list method so old servers hide it.
        // `/undo` (#1768) — the workspace snapshot picker: roll agent file
        // mutations back to a pre-mutation undo point. Gated on the snapshot
        // list method so old servers hide it.
        // #324: the session switcher popup (same surface as Alt+S).
        CommandSpec {
            name: "sessions",
            aliases: &["ss"],
            description: "command.sessions.desc",
            category: CommandCategory::Session,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(crate::menu::registry::MENU_SESSIONS)),
        },
        CommandSpec {
            name: "undo",
            aliases: &["snapshots"],
            description: "command.undo.desc",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_when_capabilities(&[
                    crate::model::APPUI_METHOD_SNAPSHOT_LIST,
                ]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::Custom("undo")),
        },
        CommandSpec {
            name: "research",
            aliases: &["lanes"],
            description: "command.research.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_when_capabilities(&[
                    crate::model::APPUI_METHOD_PROFILE_SUB_PROVIDERS_LIST,
                ]),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("research")),
        },
        CommandSpec {
            name: "status",
            aliases: &[],
            description: "command.status.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_STATUS)),
        },
        CommandSpec {
            name: "cost",
            aliases: &["usage"],
            description: "command.cost.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[APPUI_METHOD_SESSION_STATUS_READ]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_COST)),
        },
        CommandSpec {
            name: "context",
            aliases: &["ctx", "compact", "compress"],
            description: "command.context.desc",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_mutating(&[APPUI_METHOD_SESSION_COMPACT]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_CONTEXT)),
        },
        CommandSpec {
            name: "btw",
            aliases: &["aside"],
            description: "Ask a quick aside question while the current turn keeps working.",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(APPUI_BTW_METHODS_ALL),
            inline_args: InlineArgMode::Required,
            entry: CommandEntry::LocalAction(LocalAction::Btw),
        },
        CommandSpec {
            name: "profiles",
            aliases: &["profile"],
            description: "command.profiles.desc",
            category: CommandCategory::Session,
            // Local-solo only: managing on-disk profiles (list/default/delete)
            // makes sense where the client can see the data dir.
            availability: CommandAvailability::app_ui_read(&[APPUI_METHOD_PROFILE_LOCAL_CREATE]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::OpenProfilesSurface),
        },
        CommandSpec {
            // `/agents` is taken by the M15-E autonomy command below (server
            // `agent/*` RPCs, feature-gated); the DOCK picker is the local
            // roster surface, so it gets its own name.
            name: "dock",
            aliases: &["ag"],
            description: "command.dock.desc",
            category: CommandCategory::Session,
            // Purely local: the picker reads the client-side roster mirror
            // (agent/updated upserts) and switches the main-pane view; no
            // AppUI method is invoked, so it is available everywhere. The
            // menu itself renders Unavailable when the roster is empty.
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_AGENTS)),
        },
        CommandSpec {
            name: "resume",
            // #324: the "sessions" alias moved to the Alt+S open-session
            // switcher popup; /resume keeps its primary name.
            aliases: &[],
            description: "Switch to a prior session and reload its transcript.",
            category: CommandCategory::Session,
            // Gated on ALL of `APPUI_RESUME_MENU_METHODS_ALL` (`session/list` +
            // `session/hydrate`): `app_ui_read` requires every listed method, so
            // a list-but-not-hydrate server hides `/resume` rather than letting a
            // pick emit an unsupported `session/hydrate`.
            availability: CommandAvailability::app_ui_read(APPUI_RESUME_MENU_METHODS_ALL),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::OpenResumePicker),
        },
        CommandSpec {
            name: "rewind",
            aliases: &["backtrack"],
            description: "Go back to an earlier message in this session to edit and resend it.",
            category: CommandCategory::Session,
            // `session/rollback` is a MUTATING method, so this uses
            // `app_ui_mutating` (like `/review`) — it hides in read-only mode —
            // rather than `/resume`'s `app_ui_read`. Gated on the server
            // advertising `session/rollback`.
            availability: CommandAvailability::app_ui_mutating(&[])
                .with_required_methods_any(APPUI_REWIND_MENU_METHODS_ANY),
            // `Optional` so `/rewind <n>` rolls back to checkpoint `n` inline
            // (bare `/rewind` still opens the picker).
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::OpenRewindPicker),
        },
        CommandSpec {
            name: "theme",
            aliases: &[],
            description: "command.theme.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_THEME)),
        },
        CommandSpec {
            name: "lang",
            aliases: &["language"],
            description: "command.lang.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::SetLanguage),
        },
        CommandSpec {
            name: "thinking",
            aliases: &["think"],
            description: "command.thinking.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::SetThinking),
        },
        CommandSpec {
            name: "scrollmode",
            aliases: &["scroll-mode"],
            description: "command.scrollmode.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::SetScrollMode),
        },
        CommandSpec {
            name: "saveconfig",
            aliases: &["save-config"],
            description: "command.saveconfig.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::SaveConfig),
        },
        CommandSpec {
            name: "vimmode",
            aliases: &["vim-mode"],
            description: "command.vimmode.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::ToggleVimMode),
        },
        CommandSpec {
            name: "statusline",
            aliases: &["status-line"],
            description: "command.statusline.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_STATUS_LINE)),
        },
        CommandSpec {
            name: "title",
            aliases: &[],
            description: "command.title.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_TITLE)),
        },
        CommandSpec {
            name: "keymap",
            aliases: &["keys"],
            description: "command.keymap.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_KEYMAP)),
        },
        CommandSpec {
            name: "permissions",
            aliases: &["permission"],
            description: "command.permissions.desc",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_PERMISSION_MENU_METHODS_ANY),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_PERMISSIONS)),
        },
        CommandSpec {
            name: "mcp",
            aliases: &[],
            description: "command.mcp.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_MCP_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::McpConfig),
        },
        CommandSpec {
            name: "tools",
            aliases: &["tool-settings"],
            description: "command.tools.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_TOOL_SETTINGS_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::ToolConfig),
        },
        CommandSpec {
            name: "skills",
            aliases: &["skill"],
            description: "command.skills.desc",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_SKILLS_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Skills),
        },
        CommandSpec {
            name: "task",
            aliases: &[],
            description: "command.task.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_TASK_ARTIFACT_MENU_METHODS_ANY)
                .with_required_features(TASK_ARTIFACT_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
        CommandSpec {
            name: "threads",
            aliases: &["thread"],
            description: "command.threads.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_THREAD_GRAPH_MENU_METHODS_ANY)
                .with_required_features(THREAD_GRAPH_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
        CommandSpec {
            name: "turn",
            aliases: &[],
            description: "command.turn.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_TURN_STATE_MENU_METHODS_ANY)
                .with_required_features(TURN_STATE_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
        CommandSpec {
            name: "review",
            aliases: &["code-review"],
            description: "command.review.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_mutating(&[])
                .with_task(TaskRequirement::Idle)
                .with_required_methods_any(APPUI_REVIEW_START_METHODS_ANY)
                .with_required_features(REVIEW_START_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::AppUiAction(AppUiActionKind::ReviewStart),
        },
        // M15-E autonomy entry points. Each command is hidden unless
        // the server advertises `coding.autonomy.v1`. The actual RPC
        // dispatch happens in `Store::dispatch_autonomy_slash`, which
        // parses the full slash syntax via `crate::autonomy::parse_autonomy_slash`.
        CommandSpec {
            name: "agents",
            aliases: &["agent"],
            description: "command.agents.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_AGENTS_MENU_METHODS_ANY)
                .with_required_features(AUTONOMY_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
        CommandSpec {
            name: "goal",
            aliases: &[],
            description: "command.goal.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_GOAL_MENU_METHODS_ANY)
                .with_required_features(AUTONOMY_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
        CommandSpec {
            name: "loop",
            aliases: &[],
            description: "command.loop.desc",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_LOOP_MENU_METHODS_ANY)
                .with_required_features(AUTONOMY_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
    ]
}

#[cfg(test)]
mod tests {
    use octos_core::ui_protocol::methods;

    use super::*;
    use crate::menu::availability::{
        AvailabilityContext, CapabilitySet, ConnectionState, RuntimeMode, TaskActivity,
    };
    use crate::menu::types::{MenuMode, MenuSpec};

    fn simple_command(name: &'static str, aliases: &'static [&'static str]) -> CommandSpec {
        CommandSpec {
            name,
            aliases,
            description: "test",
            category: CommandCategory::Developer,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from("test")),
        }
    }

    #[test]
    fn command_registry_resolves_names_and_aliases() {
        let registry = CommandRegistry::with_core_commands();

        assert_eq!(
            registry.find("help").map(|command| command.name),
            Some("help")
        );
        assert_eq!(
            registry.find("/?").map(|command| command.name),
            Some("help")
        );
        assert_eq!(
            registry.find("/quit").map(|command| command.name),
            Some("exit")
        );

        let CommandResolution::Found {
            command,
            invocation,
        } = registry.resolve("  /help theme")
        else {
            panic!("expected /help to resolve");
        };
        assert_eq!(command.name, "help");
        assert_eq!(invocation.args, "theme");
    }

    #[test]
    fn resume_command_is_history_safe_and_gated_on_session_list_and_hydrate() {
        let registry = CommandRegistry::with_core_commands();

        // Name + alias resolve, and the verb is history-safe (recorded for
        // Up-recall, checked on the canonical name so `/sessions` is covered).
        let dock = registry.find("dock").expect("/dock is registered");
        assert_eq!(dock.name, "dock");
        assert_eq!(
            registry.find("ag").map(|command| command.name),
            Some("dock"),
            "/ag aliases the dock picker"
        );
        let resume = registry.find("resume").expect("/resume is registered");
        assert_eq!(resume.name, "resume");
        assert!(resume.history_safe(), "/resume must be history-safe");
        // #324: the "sessions" name now belongs to the Alt+S open-session
        // switcher popup, not /resume.
        assert_eq!(
            registry.find("/sessions").map(|command| command.name),
            Some("sessions"),
            "/sessions is the open-session switcher popup"
        );

        // `/resume` fetches the list via `session/list` AND loads the chosen
        // session via `session/hydrate` on selection, so it must be gated on
        // BOTH — advertising list-but-not-hydrate would make a picked row emit
        // an unsupported `session/hydrate` RPC.
        let base_caps = CapabilitySet::from_methods([methods::TURN_INTERRUPT]);
        let list_only_caps = CapabilitySet::from_methods([methods::SESSION_LIST]);
        let both_caps =
            CapabilitySet::from_methods([methods::SESSION_LIST, methods::SESSION_HYDRATE]);
        let without = AvailabilityContext {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: Some(&base_caps),
            feature_flags: &[],
            session_open: true,
        };
        let is_available = |ctx: &AvailabilityContext<'_>| {
            registry
                .available_commands(ctx)
                .into_iter()
                .any(|command| command.name == "resume")
        };

        assert!(
            !is_available(&without),
            "/resume hides without session/list"
        );

        // Advertising `session/list` alone is not enough: selection would emit
        // an unsupported `session/hydrate`, so `/resume` must still hide.
        let list_only = AvailabilityContext {
            capabilities: Some(&list_only_caps),
            ..without
        };
        assert!(
            !is_available(&list_only),
            "/resume must hide when session/hydrate is not advertised (list-only server)"
        );

        // Visible once BOTH `session/list` and `session/hydrate` are advertised.
        let with_both = AvailabilityContext {
            capabilities: Some(&both_caps),
            ..without
        };
        assert!(
            is_available(&with_both),
            "/resume appears once both session/list and session/hydrate are advertised"
        );
    }

    #[test]
    fn rewind_command_is_history_safe_and_gated_on_session_rollback() {
        let registry = CommandRegistry::with_core_commands();

        // Name + alias resolve, and the verb is history-safe (checked on the
        // canonical name so `/backtrack` is covered too).
        let rewind = registry.find("rewind").expect("/rewind is registered");
        assert_eq!(rewind.name, "rewind");
        assert!(rewind.history_safe(), "/rewind must be history-safe");
        assert_eq!(
            registry.find("/backtrack").map(|command| command.name),
            Some("rewind"),
            "/backtrack must alias /rewind"
        );

        // Hidden until the server advertises `session/rollback`; visible once it
        // does.
        let base_caps = CapabilitySet::from_methods([methods::TURN_INTERRUPT]);
        let rollback_caps = CapabilitySet::from_methods([methods::SESSION_ROLLBACK]);
        let without = AvailabilityContext {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: Some(&base_caps),
            feature_flags: &[],
            session_open: true,
        };
        assert!(
            !registry
                .available_commands(&without)
                .into_iter()
                .any(|command| command.name == "rewind"),
            "/rewind hides without session/rollback"
        );

        let with = AvailabilityContext {
            capabilities: Some(&rollback_caps),
            ..without
        };
        assert!(
            registry
                .available_commands(&with)
                .into_iter()
                .any(|command| command.name == "rewind"),
            "/rewind appears once session/rollback is advertised"
        );

        // `session/rollback` is mutating: unlike `/resume`, `/rewind` uses
        // `app_ui_mutating`, so it must hide in read-only mode even when the
        // method is advertised.
        let readonly = AvailabilityContext {
            readonly: true,
            ..with
        };
        assert!(
            !registry
                .available_commands(&readonly)
                .into_iter()
                .any(|command| command.name == "rewind"),
            "/rewind hides in read-only mode (mutating command)"
        );
    }

    #[test]
    fn slash_esc_resolves_to_the_stop_interrupt_command() {
        // The user complaint: typing `/esc` did nothing because `esc` was not
        // a registered command/alias — only the Esc KEY, Ctrl-C, and `/stop`
        // emitted the interrupt. `/esc` now aliases the `stop` command so it
        // routes to the same `StopActiveTurn` → `InterruptTurn` path.
        let registry = CommandRegistry::with_core_commands();
        assert_eq!(
            registry.find("/esc").map(|command| command.name),
            Some("stop"),
            "/esc must alias the stop/interrupt command"
        );
        assert_eq!(
            registry.find("esc").map(|command| command.name),
            Some("stop")
        );
        // The pre-existing names/aliases still resolve to the same command.
        assert_eq!(registry.find("/stop").map(|c| c.name), Some("stop"));
        assert_eq!(registry.find("/interrupt").map(|c| c.name), Some("stop"));
    }

    #[test]
    fn command_registry_rejects_duplicate_aliases() {
        let mut registry = CommandRegistry::new();
        registry
            .register(simple_command("one", &["shared"]))
            .expect("first command registers");

        let err = registry
            .register(simple_command("two", &["shared"]))
            .expect_err("duplicate alias is rejected");

        assert_eq!(
            err,
            CommandRegistryError::DuplicateIdentifier {
                identifier: "shared".into()
            }
        );
    }

    #[test]
    fn command_registry_rejects_duplicate_aliases_on_same_command() {
        let mut registry = CommandRegistry::new();
        let err = registry
            .register(simple_command("one", &["dup", "dup"]))
            .expect_err("duplicate alias is rejected");

        assert_eq!(
            err,
            CommandRegistryError::DuplicateIdentifier {
                identifier: "dup".into()
            }
        );
    }

    #[test]
    fn default_registry_shows_permissions_entry_without_permission_methods() {
        let registry = CommandRegistry::with_core_commands();
        let capabilities =
            CapabilitySet::from_methods([methods::TURN_INTERRUPT, methods::APPROVAL_SCOPES_LIST]);
        let ctx = AvailabilityContext {
            task: TaskActivity::Running,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: Some(&capabilities),
            feature_flags: &[],
            session_open: true,
        };

        let available: Vec<_> = registry
            .available_commands(&ctx)
            .into_iter()
            .map(|command| command.name)
            .collect();

        assert!(available.contains(&"ps"));
        assert!(available.contains(&"stop"));
        assert!(available.contains(&"theme"));
        assert!(available.contains(&"status"));
        assert!(available.contains(&"permissions"));
        assert!(!available.contains(&"model"));
        assert!(!available.contains(&"mcp"));
    }

    #[test]
    fn registry_uses_capability_map_for_full_partial_and_absent_appui_menus() {
        let registry = CommandRegistry::with_core_commands();

        let no_capability_ctx = AvailabilityContext {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: None,
            feature_flags: &[],
            session_open: true,
        };
        let no_capability: Vec<_> = registry
            .visible_commands(&no_capability_ctx)
            .into_iter()
            .map(|visible| visible.command.name)
            .collect();
        assert!(no_capability.contains(&"status"));
        assert!(!no_capability.contains(&"permissions"));
        assert!(no_capability.contains(&"model"));
        assert!(!no_capability.contains(&"mcp"));

        let partial_capabilities = CapabilitySet::from_methods([methods::APPROVAL_SCOPES_LIST]);
        let partial_ctx = AvailabilityContext {
            capabilities: Some(&partial_capabilities),
            ..no_capability_ctx
        };
        let partial: Vec<_> = registry
            .visible_commands(&partial_ctx)
            .into_iter()
            .map(|visible| visible.command.name)
            .collect();
        assert!(partial.contains(&"permissions"));
        assert!(partial.contains(&"model"));
        assert!(!partial.contains(&"mcp"));

        let full_capabilities = CapabilitySet::from_methods([
            methods::APPROVAL_SCOPES_LIST,
            APPUI_METHOD_MODEL_LIST,
            APPUI_METHOD_MODEL_SELECT,
            APPUI_METHOD_MCP_STATUS_LIST,
            APPUI_METHOD_PROFILE_SKILLS_LIST,
        ]);
        let full_ctx = AvailabilityContext {
            capabilities: Some(&full_capabilities),
            ..no_capability_ctx
        };
        let full: Vec<_> = registry
            .visible_commands(&full_ctx)
            .into_iter()
            .map(|visible| visible.command.name)
            .collect();
        assert!(full.contains(&"model"));
        assert!(full.contains(&"permissions"));
        assert!(full.contains(&"mcp"));
        assert!(full.contains(&"skills"));
    }

    #[test]
    fn registry_keeps_onboarding_commands_available_without_open_session() {
        let registry = CommandRegistry::with_core_commands();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_AUTH_STATUS,
            APPUI_METHOD_PROFILE_LLM_CATALOG,
            methods::APPROVAL_SCOPES_LIST,
            APPUI_METHOD_MODEL_LIST,
            APPUI_METHOD_MODEL_SELECT,
            APPUI_METHOD_MCP_STATUS_LIST,
            APPUI_METHOD_PROFILE_SKILLS_LIST,
        ]);
        let ctx = AvailabilityContext {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: Some(&capabilities),
            feature_flags: &[],
            session_open: false,
        };

        let visible: Vec<_> = registry
            .visible_commands(&ctx)
            .into_iter()
            .map(|visible| visible.command.name)
            .collect();

        assert!(visible.contains(&"status"));
        assert!(visible.contains(&"login"));
        assert!(visible.contains(&"model"));
        assert!(visible.contains(&"skills"));
        assert!(!visible.contains(&"permissions"));
        assert!(!visible.contains(&"mcp"));
        // The full onboarding wizard is dispatchable but hidden from the menu,
        // and `/add-model` moved into `/model` as its "Add a model" row — also
        // hidden from the popup while staying dispatchable by name.
        assert!(!visible.contains(&"onboard"));
        assert!(!visible.contains(&"add-model"));
    }

    #[test]
    fn registry_accepts_any_supported_permission_method() {
        let registry = CommandRegistry::with_core_commands();
        let capabilities = CapabilitySet::from_methods([APPUI_METHOD_PERMISSION_PROFILE_SET]);
        let ctx = AvailabilityContext {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: Some(&capabilities),
            feature_flags: &[],
            session_open: true,
        };

        let visible: Vec<_> = registry
            .visible_commands(&ctx)
            .into_iter()
            .map(|visible| visible.command.name)
            .collect();

        assert!(visible.contains(&"permissions"));
    }

    struct TestProvider;

    impl MenuProvider for TestProvider {
        fn id(&self) -> MenuId {
            MenuId::from("test")
        }

        fn build(&self, _ctx: &MenuContext<'_>) -> MenuBuildResult {
            MenuBuildResult::Ready(MenuSpec::new("test", "Test", MenuMode::SingleSelect))
        }
    }

    #[test]
    fn menu_registry_builds_registered_provider() {
        let mut registry = MenuRegistry::new();
        registry
            .register_provider(TestProvider)
            .expect("provider registers");
        let path = Vec::new();
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: Default::default(),
            terminal: Default::default(),
            theme_name: None,
            selected_path: &path,
        };

        let result = registry.build(&MenuId::from("test"), &ctx);

        match result {
            MenuBuildResult::Ready(spec) => assert_eq!(spec.id, MenuId::from("test")),
            _ => panic!("expected ready menu"),
        }
    }
}
