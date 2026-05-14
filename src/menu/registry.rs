use std::collections::BTreeMap;
use std::fmt;

use octos_core::ui_protocol::UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1;

use crate::menu::availability::{
    AvailabilityContext, AvailabilityStatus, CommandAvailability, evaluate_command,
};
use crate::menu::types::{
    CommandCategory, CommandEntry, CommandSpec, InlineArgMode, LocalAction, MenuBuildResult,
    MenuContext, MenuId, MenuProvider, MenuStatusSpec,
};

pub const MENU_HELP: &str = "help";
pub const MENU_MODEL: &str = "model";
pub const MENU_STATUS: &str = "status";
pub const MENU_THEME: &str = "theme";
pub const MENU_STATUS_LINE: &str = "statusline";
pub const MENU_TITLE: &str = "title";
pub const MENU_KEYMAP: &str = "keymap";
pub const MENU_PERMISSIONS: &str = "permissions";
pub const MENU_MCP: &str = "mcp";

pub const APPUI_METHOD_MODEL_LIST: &str = "model/list";
pub const APPUI_METHOD_SESSION_STATUS_READ: &str = "session/status/read";
pub const APPUI_METHOD_PERMISSION_PROFILE_LIST: &str = "permission/profile/list";
pub const APPUI_METHOD_PERMISSION_PROFILE_SET: &str = "permission/profile/set";
pub const APPUI_METHOD_APPROVAL_SCOPES_CLEAR: &str = "approval/scopes/clear";
pub const APPUI_METHOD_MCP_STATUS_LIST: &str = "mcp/status/list";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
            name: "q",
            aliases: &["exit"],
            description: "Quit octos-tui.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::always(),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::LocalAction(LocalAction::Custom("quit")),
        },
        CommandSpec {
            name: "model",
            aliases: &[],
            description: "Choose model and reasoning options.",
            category: CommandCategory::Session,
            availability: CommandAvailability::app_ui_read(&[APPUI_METHOD_MODEL_LIST]),
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
                .with_required_features(&[UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_PERMISSIONS)),
        },
        CommandSpec {
            name: "mcp",
            aliases: &[],
            description: "List MCP servers, tools, and status when supported.",
            category: CommandCategory::Runtime,
            availability: CommandAvailability::app_ui_read(&[APPUI_METHOD_MCP_STATUS_LIST]),
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from(MENU_MCP)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use octos_core::ui_protocol::{UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1, methods};

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
    fn default_registry_hides_permissions_without_approval_feature() {
        let registry = CommandRegistry::with_core_commands();
        let capabilities = CapabilitySet::from_methods([methods::TURN_INTERRUPT]);
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
        assert!(available.contains(&"q"));
        assert!(available.contains(&"theme"));
        assert!(available.contains(&"status"));
        assert!(!available.contains(&"permissions"));
        assert!(!available.contains(&"model"));
        assert!(!available.contains(&"mcp"));

        let approval_capabilities = CapabilitySet::from_methods_and_features(
            [methods::TURN_INTERRUPT],
            [UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1],
        );
        let approval_ctx = AvailabilityContext {
            capabilities: Some(&approval_capabilities),
            ..ctx
        };
        let available: Vec<_> = registry
            .available_commands(&approval_ctx)
            .into_iter()
            .map(|command| command.name)
            .collect();

        assert!(available.contains(&"permissions"));
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
        assert!(!no_capability.contains(&"model"));
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
        assert!(!partial.contains(&"permissions"));
        assert!(!partial.contains(&"model"));
        assert!(!partial.contains(&"mcp"));

        let full_capabilities = CapabilitySet::from_methods_and_features(
            [
                methods::APPROVAL_SCOPES_LIST,
                APPUI_METHOD_MODEL_LIST,
                APPUI_METHOD_MCP_STATUS_LIST,
            ],
            [UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1],
        );
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
    }

    #[test]
    fn registry_hides_session_bound_commands_without_open_session() {
        let registry = CommandRegistry::with_core_commands();
        let capabilities = CapabilitySet::from_methods([
            methods::APPROVAL_SCOPES_LIST,
            APPUI_METHOD_MODEL_LIST,
            APPUI_METHOD_MCP_STATUS_LIST,
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
        assert!(!visible.contains(&"model"));
        assert!(!visible.contains(&"permissions"));
        assert!(!visible.contains(&"mcp"));
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
