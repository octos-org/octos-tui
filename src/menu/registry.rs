use std::collections::BTreeMap;
use std::fmt;

use octos_core::ui_protocol::methods;

use crate::menu::availability::{
    AvailabilityContext, AvailabilityStatus, CommandAvailability, SessionRequirement,
    evaluate_command,
};
use crate::menu::types::{
    CommandCategory, CommandEntry, CommandSpec, InlineArgMode, LocalAction, MenuBuildResult,
    MenuContext, MenuId, MenuProvider, MenuStatusSpec,
};

pub const MENU_HELP: &str = "help";
pub const MENU_ONBOARD: &str = "onboard";
pub const MENU_ONBOARD_FAMILY: &str = "onboard-family";
pub const MENU_ONBOARD_MODEL: &str = "onboard-model";
pub const MENU_ONBOARD_ROUTE: &str = "onboard-route";
pub const MENU_LOGIN: &str = "login";
pub const MENU_PROVIDER: &str = "provider";
pub const MENU_MODEL: &str = "model";
pub const MENU_COST: &str = "cost";
pub const MENU_STATUS: &str = "status";
pub const MENU_THEME: &str = "theme";
pub const MENU_STATUS_LINE: &str = "statusline";
pub const MENU_TITLE: &str = "title";
pub const MENU_KEYMAP: &str = "keymap";
pub const MENU_PERMISSIONS: &str = "permissions";
pub const MENU_MCP: &str = "mcp";
pub const MENU_TOOL_SETTINGS: &str = "tool-settings";
pub const MENU_SKILLS: &str = "skills";

pub const APPUI_METHOD_MODEL_LIST: &str = crate::model::APPUI_METHOD_MODEL_LIST;
pub const APPUI_METHOD_MODEL_SELECT: &str = crate::model::APPUI_METHOD_MODEL_SELECT;
pub const APPUI_METHOD_SESSION_STATUS_READ: &str = crate::model::APPUI_METHOD_SESSION_STATUS_READ;
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

/// M15-E (UPCR-2026-021) required capability feature for the
/// combined `/agents` `/goal` `/loop` surface. Clients MUST gate
/// every menu entry, slash command, and dispatch on
/// `coding.autonomy.v1`; old servers must hide the controls instead
/// of being probed for unsupported methods.
pub const APPUI_FEATURE_CODING_AUTONOMY_V1: &str = crate::model::APPUI_FEATURE_CODING_AUTONOMY_V1;
pub const APPUI_FEATURE_TASK_ARTIFACTS_V1: &str = crate::model::APPUI_FEATURE_TASK_ARTIFACTS_V1;
pub const APPUI_TASK_ARTIFACT_MENU_METHODS_ANY: &[&str] =
    &[crate::model::APPUI_METHOD_TASK_ARTIFACT_READ];
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
                    "Menu unavailable",
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
            description: "Show background task and process status.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::ShowProcessStatus),
        },
        CommandSpec {
            name: "stop",
            aliases: &["interrupt"],
            description: "Interrupt the active turn or stop supported background work.",
            category: CommandCategory::Runtime,
            availability: stop_availability,
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::StopActiveTurn),
        },
        CommandSpec {
            name: "help",
            aliases: &["?", "commands"],
            description: "Show available commands.",
            category: CommandCategory::Help,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_HELP)),
        },
        CommandSpec {
            name: "exit",
            aliases: &["quit"],
            description: "Quit the TUI.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::Exit),
        },
        CommandSpec {
            name: "onboard",
            aliases: &["setup"],
            description: "Run the guided login and LLM provider setup wizard.",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_ONBOARDING_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Onboarding(
                crate::model::OnboardingAction::Open,
            )),
        },
        CommandSpec {
            name: "login",
            aliases: &["auth"],
            description: "Sign in with email OTP or inspect current auth state.",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_LOGIN_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Onboarding(
                crate::model::OnboardingAction::OpenLogin,
            )),
        },
        CommandSpec {
            name: "provider",
            aliases: &["providers"],
            description: "Configure profile-owned LLM providers and routes.",
            category: CommandCategory::Settings,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_PROVIDER_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Onboarding(
                crate::model::OnboardingAction::OpenProvider,
            )),
        },
        CommandSpec {
            name: "model",
            aliases: &[],
            description: "Choose a server-returned profile LLM model.",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_when_capabilities(&[APPUI_METHOD_MODEL_LIST]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_MODEL)),
        },
        CommandSpec {
            name: "status",
            aliases: &[],
            description: "Show snapshot-backed session, runtime, and connection status.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_STATUS)),
        },
        CommandSpec {
            name: "cost",
            aliases: &["usage"],
            description: "Show server-reported token and cost usage.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[APPUI_METHOD_SESSION_STATUS_READ]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_COST)),
        },
        CommandSpec {
            name: "theme",
            aliases: &[],
            description: "Choose the local TUI theme.",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_THEME)),
        },
        CommandSpec {
            name: "statusline",
            aliases: &["status-line"],
            description: "Configure bottom status line items.",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_STATUS_LINE)),
        },
        CommandSpec {
            name: "title",
            aliases: &[],
            description: "Configure terminal title items.",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_TITLE)),
        },
        CommandSpec {
            name: "keymap",
            aliases: &["keys"],
            description: "Inspect and edit TUI key bindings.",
            category: CommandCategory::Settings,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_KEYMAP)),
        },
        CommandSpec {
            name: "permissions",
            aliases: &["permission"],
            description: "Review or change approval, filesystem, and network permissions.",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_PERMISSION_MENU_METHODS_ANY),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_PERMISSIONS)),
        },
        CommandSpec {
            name: "mcp",
            aliases: &[],
            description: "List or configure server-owned MCP entries when supported.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_required_methods_any(APPUI_MCP_MENU_METHODS_ANY),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::McpConfig),
        },
        CommandSpec {
            name: "tools",
            aliases: &["tool-settings"],
            description: "List or configure server-owned tool settings when supported.",
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
            description: "List, search, install, or remove profile skills.",
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
            description: "Read backend task artifacts.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[])
                .with_session(SessionRequirement::Any)
                .with_required_methods_any(APPUI_TASK_ARTIFACT_MENU_METHODS_ANY)
                .with_required_features(TASK_ARTIFACT_FEATURES),
            inline_args: InlineArgMode::Optional,
            entry: CommandEntry::LocalAction(LocalAction::Custom("autonomy")),
        },
        // M15-E autonomy entry points. Each command is hidden unless
        // the server advertises `coding.autonomy.v1`. The actual RPC
        // dispatch happens in `Store::dispatch_autonomy_slash`, which
        // parses the full slash syntax via `crate::autonomy::parse_autonomy_slash`.
        CommandSpec {
            name: "agents",
            aliases: &["agent"],
            description: "Inspect or interrupt backend-owned subagents.",
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
            description: "View, set, pause, resume, or clear the persisted session goal.",
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
            description: "Create, list, pause, resume, fire-now, or delete backend loops.",
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
        assert!(visible.contains(&"provider"));
        assert!(visible.contains(&"model"));
        assert!(visible.contains(&"skills"));
        assert!(!visible.contains(&"permissions"));
        assert!(!visible.contains(&"mcp"));
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
