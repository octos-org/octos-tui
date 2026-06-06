//! Local and capability-backed menu providers for the M9.34 framework.
//!
//! Providers use only the canonical `menu::types` contract. This prevents the
//! menu registry, renderer, and store from drifting into parallel type systems.

use crossterm::event::{KeyCode, KeyModifiers};
use octos_core::ui_protocol::{
    ApprovalScopesListParams, PermissionNetworkPolicy, PermissionProfileListParams,
    PermissionProfileMode, PermissionProfileSelection, PermissionProfileSetParams,
    PermissionProfileUpdate, methods,
};
use serde_json::Value;

use crate::menu::{
    AppUiActionKind, AvailabilityStatus, ClientEffect, CommandEntry, CommandRegistry, KeyBinding,
    LocalAction, MenuAction, MenuAppSnapshot, MenuBuildResult, MenuContext, MenuId, MenuItem,
    MenuItemState, MenuMode, MenuPreview, MenuPreviewRow, MenuProvider, MenuRegistry, MenuSpec,
    MenuStatusSpec, MenuTab,
    registry::{
        APPUI_MCP_MENU_METHODS_ANY, APPUI_METHOD_APPROVAL_SCOPES_CLEAR, APPUI_METHOD_AUTH_LOGOUT,
        APPUI_METHOD_AUTH_ME, APPUI_METHOD_AUTH_SEND_CODE, APPUI_METHOD_AUTH_STATUS,
        APPUI_METHOD_AUTH_VERIFY, APPUI_METHOD_MCP_CONFIG_DELETE, APPUI_METHOD_MCP_CONFIG_LIST,
        APPUI_METHOD_MCP_CONFIG_SET_ENABLED, APPUI_METHOD_MCP_CONFIG_TEST,
        APPUI_METHOD_MCP_CONFIG_UPSERT, APPUI_METHOD_MCP_STATUS_LIST, APPUI_METHOD_MODEL_LIST,
        APPUI_METHOD_MODEL_SELECT, APPUI_METHOD_PERMISSION_PROFILE_LIST,
        APPUI_METHOD_PERMISSION_PROFILE_SET, APPUI_METHOD_PROFILE_LLM_CATALOG,
        APPUI_METHOD_PROFILE_LLM_DELETE, APPUI_METHOD_PROFILE_LLM_FETCH_MODELS,
        APPUI_METHOD_PROFILE_LLM_TEST, APPUI_METHOD_PROFILE_LLM_UPSERT,
        APPUI_METHOD_PROFILE_LOCAL_CREATE, APPUI_METHOD_PROFILE_SKILLS_INSTALL,
        APPUI_METHOD_PROFILE_SKILLS_LIST, APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
        APPUI_METHOD_PROFILE_SKILLS_REMOVE, APPUI_METHOD_TOOL_CONFIG_DELETE,
        APPUI_METHOD_TOOL_CONFIG_LIST, APPUI_METHOD_TOOL_CONFIG_SET_ENABLED,
        APPUI_METHOD_TOOL_CONFIG_TEST, APPUI_METHOD_TOOL_CONFIG_UPSERT,
        APPUI_METHOD_TOOL_STATUS_LIST, APPUI_ONBOARDING_METHODS_ANY,
        APPUI_PERMISSION_MENU_METHODS_ANY, APPUI_PROVIDER_MENU_METHODS_ANY,
        APPUI_TOOL_SETTINGS_MENU_METHODS_ANY, MENU_COST, MENU_HELP, MENU_KEYMAP, MENU_LOGIN,
        MENU_MCP, MENU_MODEL, MENU_ONBOARD, MENU_PERMISSIONS, MENU_PROVIDER, MENU_SKILLS,
        MENU_STATUS, MENU_STATUS_LINE, MENU_THEME, MENU_TITLE, MENU_TOOL_SETTINGS,
    },
};
use crate::model::{
    AppUiCommand, AuthLogoutParams, AuthMeParams, AuthStatusParams, LlmConfiguredProvider,
    LlmRouteConfig, LlmSelectionConfig, McpConfigDeleteParams, McpConfigEntry, McpConfigListParams,
    McpConfigSetEnabledParams, McpConfigTestParams, McpStatus, McpStatusListParams, ModelStatus,
    OnboardingAction, OnboardingProviderPending, OnboardingProviderSaveTarget,
    OnboardingProviderStatus, OnboardingWizardState, ProfileLlmCatalogParams, ProfileLlmListParams,
    ProfileLlmSelectParams, ProfileLlmTestParams, ProfileSkillsInstallParams,
    ProfileSkillsListParams, ProfileSkillsRemoveParams, RuntimePolicyMcpServer,
    SessionStatusReadParams, ToolConfigDeleteParams, ToolConfigEntry, ToolConfigListParams,
    ToolConfigSetEnabledParams, ToolConfigTestParams, ToolStatus, ToolStatusListParams,
};

pub fn core_menu_registry() -> MenuRegistry {
    let mut registry = MenuRegistry::new();
    for provider in [
        Provider::Help,
        Provider::Onboard,
        Provider::OnboardFamily,
        Provider::OnboardModel,
        Provider::OnboardRoute,
        Provider::Login,
        Provider::Theme,
        Provider::Thinking,
        Provider::Lang,
        Provider::StatusLine,
        Provider::Title,
        Provider::Keymap,
        Provider::Status,
        Provider::Cost,
        Provider::Model,
        Provider::Provider,
        Provider::Permissions,
        Provider::Mcp,
        Provider::ToolSettings,
        Provider::Skills,
    ] {
        registry
            .register_provider(provider)
            .expect("core menu provider ids are unique");
    }
    registry
}

#[derive(Debug, Clone, Copy)]
enum Provider {
    Help,
    Onboard,
    OnboardFamily,
    OnboardModel,
    OnboardRoute,
    Login,
    Theme,
    Thinking,
    Lang,
    StatusLine,
    Title,
    Keymap,
    Status,
    Cost,
    Model,
    Provider,
    Permissions,
    Mcp,
    ToolSettings,
    Skills,
}

impl MenuProvider for Provider {
    fn id(&self) -> MenuId {
        MenuId::from(match self {
            Self::Help => MENU_HELP,
            Self::Onboard => MENU_ONBOARD,
            Self::OnboardFamily => crate::menu::registry::MENU_ONBOARD_FAMILY,
            Self::OnboardModel => crate::menu::registry::MENU_ONBOARD_MODEL,
            Self::OnboardRoute => crate::menu::registry::MENU_ONBOARD_ROUTE,
            Self::Login => MENU_LOGIN,
            Self::Theme => MENU_THEME,
            Self::Thinking => crate::menu::registry::MENU_THINKING,
            Self::Lang => crate::menu::registry::MENU_LANG,
            Self::StatusLine => MENU_STATUS_LINE,
            Self::Title => MENU_TITLE,
            Self::Keymap => MENU_KEYMAP,
            Self::Status => MENU_STATUS,
            Self::Cost => MENU_COST,
            Self::Model => MENU_MODEL,
            Self::Provider => MENU_PROVIDER,
            Self::Permissions => MENU_PERMISSIONS,
            Self::Mcp => MENU_MCP,
            Self::ToolSettings => MENU_TOOL_SETTINGS,
            Self::Skills => MENU_SKILLS,
        })
    }

    fn build(&self, ctx: &MenuContext<'_>) -> MenuBuildResult {
        match self {
            Self::Help => MenuBuildResult::Ready(help_menu(ctx)),
            Self::Onboard => onboarding_menu(ctx),
            Self::OnboardFamily => onboarding_family_menu(ctx),
            Self::OnboardModel => onboarding_model_menu(ctx),
            Self::OnboardRoute => onboarding_route_menu(ctx),
            Self::Login => login_menu(ctx),
            Self::Theme => MenuBuildResult::Ready(theme_menu(ctx)),
            Self::Thinking => MenuBuildResult::Ready(thinking_menu(ctx)),
            Self::Lang => MenuBuildResult::Ready(lang_menu(ctx)),
            Self::StatusLine => MenuBuildResult::Ready(status_line_menu(ctx)),
            Self::Title => MenuBuildResult::Ready(title_menu(ctx)),
            Self::Keymap => MenuBuildResult::Ready(keymap_menu()),
            Self::Status => MenuBuildResult::Ready(status_menu(ctx)),
            Self::Cost => cost_menu(ctx),
            Self::Model => model_menu(ctx),
            Self::Provider => provider_menu(ctx),
            Self::Permissions => permissions_menu(ctx),
            Self::Mcp => mcp_menu(ctx),
            Self::ToolSettings => tool_settings_menu(ctx),
            Self::Skills => skills_menu(ctx),
        }
    }

    fn on_cancel(&self, _ctx: &mut MenuContext<'_>) -> Vec<ClientEffect> {
        Vec::new()
    }
}

fn help_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    let commands = CommandRegistry::with_core_commands();
    let items = commands
        .visible_commands(&ctx.availability)
        .into_iter()
        .enumerate()
        .map(|(idx, visible)| {
            let command = visible.command;
            let mut item = MenuItem::new(
                command.name,
                command.slash_name(),
                action_for_command_entry(&command.entry),
            )
            .with_description(command_description(command.description, command.aliases));
            if let Some(shortcut) = numeric_shortcut(idx) {
                item = item.with_shortcut(shortcut);
            }
            match visible.availability {
                AvailabilityStatus {
                    reason: Some(reason),
                    ..
                } if !visible.availability.is_available() => item.disabled(reason),
                _ => item,
            }
        })
        .collect();

    MenuSpec {
        id: MenuId::from(MENU_HELP),
        title: t!("menu.help.title").into_owned(),
        subtitle: Some(t!("menu.help.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.help.search").into_owned()),
        footer_hint: Some(t!("menu.help.footer").into_owned()),
        // No right-hand preview: the static "Routing" blurb was internal plumbing
        // info (not actionable) and, with the two-pane split, its text collided
        // with the command column. Each command already carries its own inline
        // description, so the list renders full-width instead.
        preview: None,
        mode: MenuMode::SingleSelect,
    }
}

fn lang_menu(_ctx: &MenuContext<'_>) -> MenuSpec {
    use crate::cli::Lang;
    let current = rust_i18n::locale().to_string();
    let items = [
        ("en", "English", Lang::En),
        ("zh", "中文 (Chinese)", Lang::Zh),
    ]
    .into_iter()
    .enumerate()
    .map(|(idx, (id, label, lang))| {
        let mut state = MenuItemState::default();
        state.current = current.as_str() == lang.code();
        let mut item = MenuItem::new(
            id,
            label,
            MenuAction::Local(LocalAction::SetLanguageCode(lang)),
        )
        .with_state(state);
        if let Some(shortcut) = numeric_shortcut(idx) {
            item = item.with_shortcut(shortcut);
        }
        item
    })
    .collect();

    MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_LANG),
        title: t!("menu.lang.title").into_owned(),
        subtitle: Some(t!("menu.lang.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.enter_apply_esc_close").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    }
}

fn thinking_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    use octos_core::ui_protocol::ReasoningEffortLevel as L;
    let current = ctx.app.reasoning_effort;
    let items = [
        (
            "default",
            t!("menu.thinking.item.default.label"),
            t!("menu.thinking.item.default.desc"),
            None,
        ),
        (
            "low",
            t!("menu.thinking.item.low.label"),
            t!("menu.thinking.item.low.desc"),
            Some(L::Low),
        ),
        (
            "medium",
            t!("menu.thinking.item.medium.label"),
            t!("menu.thinking.item.medium.desc"),
            Some(L::Medium),
        ),
        (
            "high",
            t!("menu.thinking.item.high.label"),
            t!("menu.thinking.item.high.desc"),
            Some(L::High),
        ),
        (
            "max",
            t!("menu.thinking.item.max.label"),
            t!("menu.thinking.item.max.desc"),
            Some(L::Max),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(idx, (id, label, description, level))| {
        let mut state = MenuItemState::default();
        state.current = level == current;
        let mut item = MenuItem::new(
            id,
            label,
            MenuAction::Local(LocalAction::SetThinkingLevel(level)),
        )
        .with_description(description)
        .with_state(state);
        if let Some(shortcut) = numeric_shortcut(idx) {
            item = item.with_shortcut(shortcut);
        }
        item
    })
    .collect();

    MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_THINKING),
        title: t!("menu.thinking.title").into_owned(),
        subtitle: Some(t!("menu.thinking.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.enter_apply_esc_close").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    }
}

fn theme_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    let current = ctx.theme_name.unwrap_or("codex");
    let items = [
        ("terminal", "Terminal", t!("menu.theme.item.terminal.desc")),
        ("codex", "Codex", t!("menu.theme.item.codex.desc")),
        ("claude", "Claude", t!("menu.theme.item.claude.desc")),
        ("slate", "Slate", t!("menu.theme.item.slate.desc")),
        (
            "solarized",
            "Solarized",
            t!("menu.theme.item.solarized.desc"),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(idx, (id, label, description))| {
        let mut state = MenuItemState::default();
        state.current = id == current;
        let mut item = MenuItem::new(
            id,
            label,
            MenuAction::Local(LocalAction::SetTheme(id.to_owned())),
        )
        .with_description(description)
        .with_state(state);
        if let Some(shortcut) = numeric_shortcut(idx) {
            item = item.with_shortcut(shortcut);
        }
        item
    })
    .collect();

    MenuSpec {
        id: MenuId::from(MENU_THEME),
        title: t!("menu.theme.title").into_owned(),
        subtitle: Some(t!("menu.theme.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.theme.search").into_owned()),
        footer_hint: Some(t!("menu.footer.enter_apply_esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.theme.preview_title").into_owned()),
            rows: vec![MenuPreviewRow {
                label: "theme".into(),
                value: current.into(),
            }],
        }),
        mode: MenuMode::SingleSelect,
    }
}

fn status_line_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    component_menu(
        MENU_STATUS_LINE,
        t!("menu.statusline.title").into_owned(),
        t!("menu.statusline.subtitle").into_owned(),
        &status_line_items(ctx.app.clone()),
        LocalAction::SaveStatusLine,
    )
}

fn title_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    component_menu(
        MENU_TITLE,
        t!("menu.title.title").into_owned(),
        t!("menu.title.subtitle").into_owned(),
        &title_items(ctx.app.clone()),
        LocalAction::SaveTerminalTitle,
    )
}

fn component_menu(
    id: &'static str,
    title: String,
    subtitle: String,
    rows: &[(&'static str, String, bool)],
    save: fn(Vec<String>) -> LocalAction,
) -> MenuSpec {
    let selected = rows
        .iter()
        .filter(|(_, _, checked)| *checked)
        .map(|(id, _, _)| (*id).to_owned())
        .collect::<Vec<_>>();
    let items = rows
        .iter()
        .map(|(id, label, checked)| {
            MenuItem::new(
                *id,
                label.clone(),
                MenuAction::Local(save(selected.clone())),
            )
            .with_state(MenuItemState::checked(*checked))
        })
        .collect();

    MenuSpec {
        id: MenuId::from(id),
        title,
        subtitle: Some(subtitle),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.component.search").into_owned()),
        footer_hint: Some(t!("menu.component.footer").into_owned()),
        preview: Some(MenuPreview::Text {
            title: Some(t!("menu.component.preview_title").into_owned()),
            body: selected.join(" | "),
        }),
        mode: MenuMode::MultiSelect {
            allow_reorder: true,
            min_selected: 1,
            max_selected: None,
        },
    }
}

fn keymap_menu() -> MenuSpec {
    let rows = [
        (
            "global.quit",
            "Ctrl+Q",
            t!("menu.keymap.item.global_quit.desc"),
        ),
        (
            "global.interrupt",
            "Ctrl+C",
            t!("menu.keymap.item.global_interrupt.desc"),
        ),
        (
            "composer.submit",
            "Enter",
            t!("menu.keymap.item.composer_submit.desc"),
        ),
        (
            "composer.move-line",
            "Ctrl+A/E",
            t!("menu.keymap.item.composer_move_line.desc"),
        ),
        (
            "composer.move-char",
            "Ctrl+B/F",
            t!("menu.keymap.item.composer_move_char.desc"),
        ),
        (
            "composer.move-word",
            "Alt+B/F",
            t!("menu.keymap.item.composer_move_word.desc"),
        ),
        (
            "composer.delete-word",
            "Ctrl+W",
            t!("menu.keymap.item.composer_delete_word.desc"),
        ),
        (
            "composer.kill-line",
            "Ctrl+K",
            t!("menu.keymap.item.composer_kill_line.desc"),
        ),
        (
            "menu.accept",
            "Enter",
            t!("menu.keymap.item.menu_accept.desc"),
        ),
        (
            "menu.cancel",
            "Esc",
            t!("menu.keymap.item.menu_cancel.desc"),
        ),
        ("menu.next", "Down/J", t!("menu.keymap.item.menu_next.desc")),
        (
            "menu.previous",
            "Up/K",
            t!("menu.keymap.item.menu_previous.desc"),
        ),
    ];
    let items = rows
        .into_iter()
        .map(|(id, key, description)| {
            MenuItem::new(id, key, MenuAction::Noop).with_description(description)
        })
        .collect();

    MenuSpec {
        id: MenuId::from(MENU_KEYMAP),
        title: t!("menu.keymap.title").into_owned(),
        subtitle: Some(t!("menu.keymap.subtitle").into_owned()),
        items,
        tabs: keymap_tabs(),
        searchable: true,
        search_placeholder: Some(t!("menu.keymap.search").into_owned()),
        footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        preview: Some(MenuPreview::Text {
            title: Some(t!("menu.keymap.preview_title").into_owned()),
            body: t!("menu.keymap.preview_body").into_owned(),
        }),
        mode: MenuMode::SingleSelect,
    }
}

fn status_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    let mut items = vec![
        MenuItem::new(
            "status.snapshot",
            t!("menu.status.item.snapshot.label"),
            MenuAction::Noop,
        )
        .with_description(ctx.app.status.unwrap_or("no status supplied")),
        MenuItem::new(
            "status.connection",
            t!("menu.status.item.connection.label"),
            MenuAction::Noop,
        )
        .with_description(ctx.app.target.unwrap_or("local/offline")),
    ];

    items.extend(status_runtime_items(ctx));

    if let Some(session_id) = ctx.app.selected_session_id.cloned() {
        if ctx
            .availability
            .supports_method(AppUiActionKind::SessionStatusRead.method())
        {
            items.push(
                MenuItem::new(
                    "status.refresh",
                    t!("menu.status.item.refresh.label"),
                    MenuAction::SendAppUi(AppUiCommand::ReadSessionStatus(
                        SessionStatusReadParams { session_id },
                    )),
                )
                .with_description("Uses session/status/read."),
            );
        } else {
            items.push(
                MenuItem::new(
                    "status.refresh",
                    t!("menu.status.item.refresh.label"),
                    MenuAction::Noop,
                )
                .disabled(format!(
                    "Octos UI method `{}` is not advertised",
                    AppUiActionKind::SessionStatusRead.method()
                )),
            );
        }
    } else {
        items.push(
            MenuItem::new(
                "status.refresh",
                t!("menu.status.item.refresh.label"),
                MenuAction::Noop,
            )
            .disabled("server status requires an open Octos UI session"),
        );
    }

    items.push(capability_summary_item(ctx));

    MenuSpec {
        id: MenuId::from(MENU_STATUS),
        title: t!("menu.status.title").into_owned(),
        subtitle: Some(t!("menu.status.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.status.preview_title").into_owned()),
            rows: status_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    }
}

fn cost_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(session_id) = ctx.app.selected_session_id.cloned() else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_COST),
            title: t!("menu.cost.unavailable_title").into_owned(),
            message: t!("menu.cost.unavailable_no_session").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    };

    if !ctx
        .availability
        .supports_method(AppUiActionKind::SessionStatusRead.method())
    {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_COST),
            title: t!("menu.cost.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, AppUiActionKind::SessionStatusRead.method()),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let mut items = vec![
        MenuItem::new(
            "cost.refresh",
            t!("menu.cost.item.refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ReadSessionStatus(SessionStatusReadParams {
                session_id,
            })),
        )
        .with_description("Uses session/status/read."),
    ];

    if let Some(status) = ctx.app.runtime_status {
        if let Some(usage) = &status.usage {
            items.extend([
                usage_item(
                    "cost.input",
                    t!("menu.cost.item.input_tokens.label").into_owned(),
                    usage.input_tokens,
                ),
                usage_item(
                    "cost.output",
                    t!("menu.cost.item.output_tokens.label").into_owned(),
                    usage.output_tokens,
                ),
                usage_item(
                    "cost.cached_input",
                    t!("menu.cost.item.cached_input.label").into_owned(),
                    usage.cached_input_tokens,
                ),
                usage_item(
                    "cost.cached_output",
                    t!("menu.cost.item.cached_output.label").into_owned(),
                    usage.cached_output_tokens,
                ),
                cost_item(usage.estimated_cost_micros_usd),
            ]);
        } else {
            items.push(
                MenuItem::new(
                    "cost.empty",
                    t!("menu.cost.item.empty.label"),
                    MenuAction::Noop,
                )
                .disabled("session/status/read returned no usage totals yet"),
            );
        }
    } else {
        items.push(
            MenuItem::new(
                "cost.cached",
                t!("menu.cost.item.cached.label"),
                MenuAction::Noop,
            )
            .disabled("session/status/read is advertised but no result is cached yet"),
        );
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_COST),
        title: t!("menu.cost.title").into_owned(),
        subtitle: Some(t!("menu.cost.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.cost.footer").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.runtime_preview_title").into_owned()),
            rows: status_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if !supports_any_method(ctx, APPUI_ONBOARDING_METHODS_ANY) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_ONBOARD),
            title: t!("menu.onboard.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_AUTH_STATUS),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };
    let current_profile = ctx.app.current_profile;
    let local_profile_create = local_profile_create_supported(ctx);
    if local_profile_create && state.effective_profile_id(current_profile).is_none() {
        return onboarding_local_profile_menu(state);
    }
    if state.effective_profile_id(current_profile).is_some() {
        return onboarding_provider_setup_menu(ctx, state, current_profile);
    }

    let mut items = if local_profile_create {
        vec![
            MenuItem::new(
                "onboard.local.status",
                onboarding_local_profile_label(state),
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_status.desc")),
            MenuItem::new(
                "onboard.local.name",
                if state.has_name() {
                    format!("Name: {}", state.name)
                } else {
                    "Name: not set".into()
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_name.desc"))
            .with_state(MenuItemState::required(state.has_name())),
            MenuItem::new(
                "onboard.local.username",
                if state.has_username() {
                    format!("Username: {}", state.username)
                } else {
                    "Username: not set".into()
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_username.desc"))
            .with_state(MenuItemState::required(state.has_username())),
            MenuItem::new(
                "onboard.local.email",
                if state.has_email() {
                    format!("Email: {}", state.email)
                } else {
                    "Email: not set".into()
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_email.desc"))
            .with_state(MenuItemState::required(state.has_email())),
            MenuItem::new(
                "onboard.local.create",
                t!("menu.onboard.item.local_create.label"),
                MenuAction::Local(LocalAction::Onboarding(
                    OnboardingAction::CreateLocalProfile,
                )),
            )
            .with_description(t!("menu.onboard.item.local_create.desc"))
            .maybe_disabled(onboarding_local_profile_disabled_reason(state)),
        ]
    } else {
        vec![
            MenuItem::new(
                "onboard.status.auth",
                onboarding_auth_label(state),
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.auth_status.desc")),
            MenuItem::new(
                "onboard.auth.status",
                t!("menu.onboard.item.auth_refresh.label"),
                MenuAction::SendAppUi(AppUiCommand::AuthStatus(AuthStatusParams::default())),
            )
            .with_description("Uses auth/status.")
            .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_STATUS)),
            MenuItem::new(
                "onboard.auth.send",
                t!("menu.onboard.item.auth_send.label"),
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SendCode)),
            )
            .with_description("Uses auth/send_code with the wizard email.")
            .maybe_disabled(onboarding_disabled_reason(
                ctx,
                state,
                APPUI_METHOD_AUTH_SEND_CODE,
                "email is empty",
            )),
            MenuItem::new(
                "onboard.auth.verify",
                t!("menu.onboard.item.auth_verify.label"),
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::VerifyCode)),
            )
            .with_description(t!("menu.onboard.item.auth_verify.desc"))
            .maybe_disabled(onboarding_verify_disabled_reason(ctx, state)),
            MenuItem::new(
                "onboard.auth.me",
                t!("menu.onboard.item.auth_me.label"),
                MenuAction::SendAppUi(AppUiCommand::AuthMe(AuthMeParams {
                    token: state.auth_token.clone(),
                })),
            )
            .with_description("Uses auth/me.")
            .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_ME)),
        ]
    };

    items.push(
        MenuItem::new(
            "onboard.catalog.refresh",
            t!("menu.onboard.item.catalog_refresh.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::RefreshCatalog)),
        )
        .with_description(t!("menu.onboard.item.catalog_refresh.desc"))
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_CATALOG)),
    );

    items.extend(onboarding_catalog_items(ctx, state));

    items.extend([
        MenuItem::new(
            "onboard.provider.current",
            format!("Provider: {}", state.provider_label()),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.provider_current.desc"))
        .with_state(MenuItemState::required(state.selection_ready())),
        MenuItem::new(
            "onboard.provider.key",
            if state.has_api_key() {
                format!("API key: {}", state.api_key_label())
            } else {
                "API key: not set".into()
            },
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.api_key.desc"))
        .with_state(MenuItemState::required(state.has_api_key())),
        MenuItem::new(
            "onboard.provider.fetch",
            t!("menu.onboard.item.fetch_models.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::FetchModels)),
        )
        .with_description(t!("menu.onboard.item.fetch_models.desc"))
        .maybe_disabled(onboarding_fetch_models_disabled_reason(ctx, state)),
        MenuItem::new(
            "onboard.provider.test",
            t!("menu.onboard.item.test_provider.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::TestProvider)),
        )
        .with_description(t!("menu.onboard.item.test_provider.desc"))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_TEST,
        )),
        MenuItem::new(
            "onboard.provider.save",
            t!("menu.onboard.item.save_provider.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SaveProvider)),
        )
        .with_description(t!("menu.onboard.item.save_provider.desc"))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        )),
        MenuItem::new(
            "onboard.providers.refresh",
            t!("menu.onboard.item.providers_refresh.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::RefreshProviders)),
        )
        .with_description("Uses profile/llm/list.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_MODEL_LIST)),
        MenuItem::new(
            "onboard.finish",
            t!("menu.onboard.item.finish.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::Finish)),
        )
        .with_description(t!("menu.onboard.item.finish.desc"))
        .maybe_disabled(onboarding_finish_disabled_reason(
            ctx,
            state,
            current_profile,
        )),
        MenuItem::new(
            "onboard.reset",
            t!("menu.onboard.item.reset.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::Reset)),
        ),
    ]);

    for (idx, item) in items.iter_mut().enumerate() {
        if let Some(shortcut) = numeric_shortcut(idx) {
            item.shortcut = Some(shortcut);
        }
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_ONBOARD),
        title: t!("menu.onboard.title").into_owned(),
        subtitle: Some(t!("menu.onboard.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.onboard.search").into_owned()),
        footer_hint: Some(t!("menu.onboard.footer").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.onboard.preview_title").into_owned()),
            rows: onboarding_preview_rows(state, current_profile),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_provider_setup_menu(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> MenuBuildResult {
    let mut items = vec![
        MenuItem::new(
            "onboard.provider.profile",
            format!("Profile: {}", state.profile_label(current_profile)),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.profile.desc")),
        MenuItem::new(
            "onboard.catalog.refresh",
            if ctx.app.profile_llm_catalog.is_some() {
                t!("menu.onboard.item.catalog_reload.label")
            } else {
                t!("menu.onboard.item.catalog_load.label")
            },
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::RefreshCatalog)),
        )
        .with_description(t!("menu.onboard.item.catalog_load.desc"))
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_CATALOG)),
    ];

    items.extend([
        MenuItem::new(
            "onboard.provider.family",
            format!("Model family: {}", onboarding_family_label(state)),
            MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY)),
        )
        .with_description(t!("menu.onboard.item.family.desc"))
        .with_state(MenuItemState::required(
            !state.provider.family_id.trim().is_empty(),
        )),
        MenuItem::new(
            "onboard.provider.model",
            format!("Model: {}", onboarding_model_label(state)),
            MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL)),
        )
        .with_description(t!("menu.onboard.item.model.desc"))
        .with_state(MenuItemState::required(
            !state.provider.model_id.trim().is_empty(),
        ))
        .maybe_disabled({
            state
                .provider
                .family_id
                .trim()
                .is_empty()
                .then_some("choose family first".into())
        }),
        MenuItem::new(
            "onboard.provider.route",
            format!("Provider route: {}", onboarding_route_label(state)),
            MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE)),
        )
        .with_description(t!("menu.onboard.item.route.desc"))
        .with_state(MenuItemState::required(
            !state.provider.route.route_id.trim().is_empty(),
        ))
        .maybe_disabled(
            (!onboarding_model_selected(state)).then_some("choose family and model first".into()),
        ),
    ]);

    items.extend([
        MenuItem::new(
            "onboard.provider.current",
            format!("Selected provider: {}", state.provider_label()),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.selected_provider.desc"))
        .with_state(MenuItemState::required(state.selection_ready())),
        MenuItem::new(
            "onboard.provider.saved",
            onboarding_provider_saved_status_label(state),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.saved_provider.desc"))
        .with_state(onboarding_provider_saved_status_state(state)),
        onboarding_edit_item(
            "onboard.provider.key",
            t!("menu.onboard.item.api_key.label").into_owned(),
            state.has_api_key().then_some(state.api_key_label()),
            "/onboard key ",
        )
        .with_state(MenuItemState::required(state.has_api_key()))
        .maybe_disabled((!state.selection_ready()).then_some("choose provider first".into())),
        MenuItem::new(
            "onboard.provider.test",
            onboarding_provider_test_label(state),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::TestProvider)),
        )
        .with_description(t!("menu.onboard.item.verify_provider.desc"))
        .with_state(onboarding_provider_test_state(state))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_TEST,
        )),
        MenuItem::new(
            "onboard.provider.save",
            onboarding_provider_save_label(state),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SaveProvider)),
        )
        .with_description(t!("menu.onboard.item.persist_provider.desc"))
        .with_state(onboarding_provider_save_state(state))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        )),
        MenuItem::new(
            "onboard.provider.fallback",
            onboarding_provider_fallback_label(state),
            MenuAction::Local(LocalAction::Onboarding(
                OnboardingAction::SaveProviderFallback,
            )),
        )
        .with_description(t!("menu.onboard.item.fallback_provider.desc"))
        .with_state(onboarding_provider_save_state(state))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        )),
        // M22-C: workspace step. Surfaces the staged candidate (or
        // the active workspace root), its validation status, and
        // gives the user an explicit re-validate row. Finish is
        // disabled until validation reports `Valid`.
        MenuItem::new(
            "onboard.workspace.current",
            format!(
                "Workspace: {}",
                onboarding_workspace_display(state, ctx.app.cwd.unwrap_or(""))
            ),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.workspace.desc"))
        .with_state(MenuItemState::required(
            state.workspace_validation.is_valid(),
        )),
        MenuItem::new(
            "onboard.workspace.status",
            onboarding_workspace_status_label(state),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.workspace_status.desc")),
        MenuItem::new(
            "onboard.workspace.validate",
            t!("menu.onboard.item.workspace_validate.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::ValidateWorkspace)),
        )
        .with_description(t!("menu.onboard.item.workspace_validate.desc")),
        // M22-D: permission profile staging row. The wizard only
        // displays the staged choice — the server confirms it via
        // the runtime policy stamp after `session/open`.
        MenuItem::new(
            "onboard.permissions.staged",
            onboarding_permission_profile_label(state),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.permissions_staged.desc"))
        .with_state(MenuItemState::required(
            state.staged_permission_profile.is_some(),
        )),
        // Issue #2/#4: the final ACTIVATE step. After model config + test +
        // save succeed and the workspace validates, this is the one explicit
        // action that opens the coding session and drops the user into the
        // working surface. The label + description spell out exactly what to do
        // ("press Enter") so the activation step is never a mystery.
        {
            let activate_blocked =
                onboarding_open_session_disabled_reason(ctx, state, current_profile);
            let label = if activate_blocked.is_none() {
                t!("onboarding.wizard.activate_ready_label")
            } else {
                t!("onboarding.wizard.activate_blocked_label")
            };
            let description = match &activate_blocked {
                None => t!("onboarding.wizard.activate_ready_description").into_owned(),
                Some(reason) => t!(
                    "onboarding.wizard.activate_blocked_description",
                    reason = reason
                )
                .into_owned(),
            };
            MenuItem::new(
                "onboard.finish",
                label,
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::Finish)),
            )
            .with_description(description)
            .with_state(MenuItemState::required(activate_blocked.is_none()))
            .maybe_disabled(activate_blocked)
        },
    ]);

    for (idx, item) in items.iter_mut().enumerate() {
        if let Some(shortcut) = numeric_shortcut(idx) {
            item.shortcut = Some(shortcut);
        }
    }

    // Wizard framing: compute the coarse step (Provider → Connect → Save →
    // Workspace → Activate) so the subtitle, footer, and right-side checklist
    // all stay in lock-step with the granular rows above.
    let progress = crate::menu::wizard::WizardProgress::from_state(
        state,
        current_profile,
        local_profile_create_supported(ctx),
    );
    let next_action = onboarding_next_action_hint(ctx, state, current_profile);

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_ONBOARD),
        title: t!("onboarding.wizard.setup_title").into_owned(),
        subtitle: Some(progress.subtitle()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Filter setup actions".into()),
        footer_hint: Some(progress.footer_hint(&next_action)),
        preview: Some(progress.checklist_preview()),
        mode: MenuMode::SingleSelect,
    })
}

/// Compute the single next concrete action for the provider/setup phase of the
/// wizard, in dependency order. This drives the `Next: ...` footer so the user
/// always knows the immediate thing to do.
fn onboarding_next_action_hint(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> String {
    if ctx.app.profile_llm_catalog.is_none() {
        return t!("onboarding.wizard.next.load_catalog").into_owned();
    }
    if state.provider.family_id.trim().is_empty() {
        return t!("onboarding.wizard.next.choose_family").into_owned();
    }
    if state.provider.model_id.trim().is_empty() {
        return t!("onboarding.wizard.next.choose_model").into_owned();
    }
    if !state.selection_ready() {
        return t!("onboarding.wizard.next.choose_route").into_owned();
    }
    if !state.has_api_key() {
        return t!("onboarding.wizard.next.paste_key").into_owned();
    }
    if !state.provider_tested
        && !matches!(
            state.provider_status(),
            OnboardingProviderStatus::SavedPrimary
        )
    {
        return t!("onboarding.wizard.next.test_provider").into_owned();
    }
    if !matches!(
        state.provider_status(),
        OnboardingProviderStatus::SavedPrimary | OnboardingProviderStatus::SavedFallback
    ) {
        return t!("onboarding.wizard.next.save_provider").into_owned();
    }
    if onboarding_workspace_disabled_reason(state).is_some() {
        return t!("onboarding.wizard.next.validate_workspace").into_owned();
    }
    if onboarding_open_session_disabled_reason(ctx, state, current_profile).is_none() {
        return t!("onboarding.wizard.next.activate").into_owned();
    }
    t!("onboarding.wizard.next.finish_remaining").into_owned()
}

fn onboarding_local_profile_menu(state: &OnboardingWizardState) -> MenuBuildResult {
    let items = vec![
        MenuItem::new(
            "onboard.local.status",
            t!("onboarding.local.title"),
            MenuAction::Noop,
        )
        .with_description(t!("onboarding.local.description")),
        onboarding_edit_item(
            "onboard.local.name",
            t!("onboarding.field.full_name"),
            state.has_name().then_some(state.name.as_str()),
            "/onboard name ",
        )
        .with_state(MenuItemState::required(state.has_name())),
        onboarding_edit_item(
            "onboard.local.username",
            t!("onboarding.field.username"),
            state.has_username().then_some(state.username.as_str()),
            "/onboard username ",
        )
        .with_state(MenuItemState::required(state.has_username())),
        onboarding_edit_item(
            "onboard.local.email",
            t!("onboarding.field.email"),
            state.has_email().then_some(state.email.as_str()),
            "/onboard email ",
        )
        .with_state(MenuItemState::required(state.has_email())),
        MenuItem::new(
            "onboard.local.create",
            t!("onboarding.local.continue"),
            MenuAction::Local(LocalAction::Onboarding(
                OnboardingAction::CreateLocalProfile,
            )),
        )
        .with_description(t!("onboarding.local.create_action"))
        .maybe_disabled(onboarding_local_profile_disabled_reason(state)),
    ];

    // Wizard framing: this is Step 1 (Profile). The local-create branch is only
    // reached when `profile/local/create` is supported AND no profile is
    // resolved yet, so progress is computed with `local_create_supported = true`
    // and `current_profile = None`.
    let progress = crate::menu::wizard::WizardProgress::from_state(state, None, true);
    let next_action = if state.local_profile_ready() {
        t!("onboarding.wizard.next.local_continue")
    } else {
        t!("onboarding.wizard.next.local_fill_fields")
    };

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_ONBOARD),
        title: t!("onboarding.welcome_title").into(),
        subtitle: Some(progress.subtitle()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(progress.footer_hint(next_action.as_ref())),
        // The first-run OCTOS splash renders in the MAIN window (see
        // `render_onboarding_first_launch_layout` in app.rs); the right pane now
        // carries the wizard progress checklist so the user always sees where
        // they are and what's left.
        preview: Some(progress.checklist_preview()),
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_family_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(catalog) = ctx.app.profile_llm_catalog else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY),
            title: t!("menu.onboard.family.title").into_owned(),
            message: t!("menu.onboard.unavailable_catalog_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    };
    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };
    let mut items = catalog
        .families
        .iter()
        .map(|(family_id, family)| {
            let label = family
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or(family_id);
            let model_count = family
                .get("models")
                .and_then(Value::as_array)
                .map(|models| models.len())
                .unwrap_or(0);
            let mut item = MenuItem::new(
                format!("onboard.family.{family_id}"),
                label.to_owned(),
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetFamilyId(
                    family_id.clone(),
                ))),
            )
            .with_description(format!("{model_count} model(s)"));
            if state.provider.family_id == *family_id {
                item = item.with_state(MenuItemState::current());
            }
            item
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.label.clone());

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY),
        title: t!("menu.onboard.family.title").into_owned(),
        subtitle: Some(t!("menu.onboard.family.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.onboard.family.search").into_owned()),
        footer_hint: Some(t!("menu.onboard.family.footer").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_model_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };
    let family_id = state.provider.family_id.trim();
    if family_id.is_empty() {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL),
            title: t!("menu.onboard.model.title").into_owned(),
            message: t!("menu.onboard.unavailable_family_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    }
    let Some(catalog) = ctx.app.profile_llm_catalog else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL),
            title: t!("menu.onboard.model.title").into_owned(),
            message: t!("menu.onboard.unavailable_catalog_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    };
    let Some(models) = catalog
        .families
        .get(family_id)
        .and_then(|family| family.get("models"))
        .and_then(Value::as_array)
    else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL),
            title: t!("menu.onboard.model.title").into_owned(),
            message: format!("No models found for family `{family_id}`."),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    };
    let mut items = models
        .iter()
        .filter_map(|model| {
            let model_id = model.get("id").and_then(Value::as_str)?;
            let route_count = model
                .get("endpoints")
                .and_then(Value::as_array)
                .map(|routes| routes.len())
                .unwrap_or(1);
            let mut item = MenuItem::new(
                format!("onboard.model.{family_id}.{model_id}"),
                model_id.to_owned(),
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetModelId(
                    model_id.to_owned(),
                ))),
            )
            .with_description(format!("{route_count} provider route(s)"));
            if state.provider.model_id == model_id {
                item = item.with_state(MenuItemState::current());
            }
            Some(item)
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.label.clone());

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL),
        title: t!("menu.onboard.model.title").into_owned(),
        subtitle: Some(format!("Family: {family_id}")),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.onboard.model.search").into_owned()),
        footer_hint: Some(t!("menu.onboard.model.footer").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_route_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };
    if !onboarding_model_selected(state) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE),
            title: t!("menu.onboard.route.title").into_owned(),
            message: t!("menu.onboard.unavailable_model_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    }
    let Some(catalog) = ctx.app.profile_llm_catalog else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE),
            title: t!("menu.onboard.route.title").into_owned(),
            message: t!("menu.onboard.unavailable_catalog_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    };
    let mut choices = catalog_choices(&catalog.families)
        .into_iter()
        .filter(|choice| {
            choice.selection.family_id == state.provider.family_id
                && choice.selection.model_id == state.provider.model_id
        })
        .collect::<Vec<_>>();
    choices.sort_by_key(catalog_choice_rank);
    let items = choices
        .into_iter()
        .map(|choice| {
            let route = &choice.selection.route;
            let route_label = route.label.as_deref().unwrap_or(route.route_id.as_str());
            let mut item = MenuItem::new(
                choice.id,
                format!("{route_label} ({})", route.route_id),
                MenuAction::Local(LocalAction::Onboarding(
                    OnboardingAction::SetProviderSelection(choice.selection.clone()),
                )),
            )
            .with_description(choice.description);
            if state.provider == choice.selection {
                item = item.with_state(MenuItemState::current());
            }
            item
        })
        .collect::<Vec<_>>();

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE),
        title: t!("menu.onboard.route.title").into_owned(),
        subtitle: Some(format!(
            "{} / {}",
            state.provider.family_id, state.provider.model_id
        )),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.onboard.route.search").into_owned()),
        footer_hint: Some(t!("menu.onboard.route.footer").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_edit_item(
    id: &'static str,
    label: impl AsRef<str>,
    value: Option<&str>,
    draft: &'static str,
) -> MenuItem {
    let not_set = t!("onboarding.value_not_set");
    let rendered_value = value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(not_set.as_ref());
    MenuItem::new(
        id,
        format!("{}: {rendered_value}", label.as_ref()),
        MenuAction::Local(LocalAction::EditComposer(draft.into())),
    )
    .with_description(t!("onboarding.action_edit"))
}

fn onboarding_family_label(state: &OnboardingWizardState) -> &str {
    state
        .provider
        .family_id
        .trim()
        .is_empty()
        .then_some("not selected")
        .unwrap_or(state.provider.family_id.as_str())
}

fn onboarding_model_label(state: &OnboardingWizardState) -> &str {
    state
        .provider
        .model_id
        .trim()
        .is_empty()
        .then_some("not selected")
        .unwrap_or(state.provider.model_id.as_str())
}

fn onboarding_route_label(state: &OnboardingWizardState) -> String {
    if state.provider.route.route_id.trim().is_empty() {
        "not selected".into()
    } else {
        state
            .provider
            .route
            .label
            .as_deref()
            .map(|label| format!("{label} ({})", state.provider.route.route_id))
            .unwrap_or_else(|| state.provider.route.route_id.clone())
    }
}

fn onboarding_model_selected(state: &OnboardingWizardState) -> bool {
    !state.provider.family_id.trim().is_empty() && !state.provider.model_id.trim().is_empty()
}

fn onboarding_auth_label(state: &OnboardingWizardState) -> String {
    if state.auth_verified {
        "Auth: verified".into()
    } else if state.auth_code_sent {
        format!("Auth: code sent to {}", state.email)
    } else if state.has_email() {
        format!("Auth: email {}", state.email)
    } else {
        "Auth: email not set".into()
    }
}

fn onboarding_local_profile_label(state: &OnboardingWizardState) -> String {
    if state.local_profile_created {
        format!("Local profile: {}", state.profile_label(None))
    } else if state.local_profile_ready() {
        format!("Local profile: ready for {}", state.username)
    } else {
        "Local profile: name, username, and email required".into()
    }
}

/// M22-D: human label for the staged permission profile in the
/// onboarding menu. Mirrors `permission_profile_items` mode labels
/// so the onboarding step and the `/permissions` menu use the same
/// vocabulary; when a mismatch has been observed the label calls
/// it out so the user knows the server clamped the choice.
fn onboarding_permission_profile_label(state: &OnboardingWizardState) -> String {
    use octos_core::ui_protocol::PermissionProfileMode;
    let staged = match state.staged_permission_profile.as_ref() {
        Some(update) => update,
        None => return "Permissions: (default — use /onboard permissions <mode>)".into(),
    };
    let mode = staged
        .mode
        .map(|m| match m {
            PermissionProfileMode::ReadOnly => {
                t!("menu.permissions.item.read_only.label").into_owned()
            }
            PermissionProfileMode::WorkspaceWrite => {
                t!("menu.permissions.item.workspace_write.label").into_owned()
            }
            PermissionProfileMode::DangerFullAccess => {
                t!("menu.permissions.item.full_access.label").into_owned()
            }
        })
        .unwrap_or_else(|| "(mode unchanged)".to_owned());
    let approval = staged.approval_policy.as_deref().unwrap_or("(unchanged)");
    let network = staged
        .network
        .map(|n| match n {
            octos_core::ui_protocol::PermissionNetworkPolicy::Allow => "network allowed",
            octos_core::ui_protocol::PermissionNetworkPolicy::Deny => "network blocked",
        })
        .unwrap_or("(network unchanged)");
    if let Some(mismatch) = state.permission_profile_mismatch.as_deref() {
        format!("Permissions: staged {mode} · {approval} · {network} — server CLAMPED: {mismatch}")
    } else {
        format!("Permissions: staged {mode} · {approval} · {network}")
    }
}

fn onboarding_local_profile_disabled_reason(state: &OnboardingWizardState) -> Option<String> {
    // M22-B: email stays required to match the current backend
    // contract for `profile/local/create` (it rejects `""` with
    // `profile_local_invalid_email`). The contract's "optional
    // email metadata" wording is aspirational until the backend
    // accepts empty email; flipping the TUI now would invite the
    // user into a guaranteed-failure submission.
    if !state.has_name() {
        Some("name is empty".into())
    } else if !state.has_username() {
        Some("username is empty".into())
    } else if !state.has_email() {
        Some("email is empty".into())
    } else {
        None
    }
}

fn onboarding_finish_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> Option<String> {
    if state.effective_profile_id(current_profile).is_some() {
        return None;
    }
    if local_profile_create_supported(ctx) {
        return onboarding_local_profile_disabled_reason(state);
    }
    Some("profile is unresolved; use /onboard profile <profile_id>".into())
}

fn onboarding_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    method: &'static str,
    missing_input: &'static str,
) -> Option<String> {
    action_missing_reason(ctx, method)
        .or_else(|| (!state.has_email()).then(|| missing_input.into()))
}

fn onboarding_verify_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
) -> Option<String> {
    action_missing_reason(ctx, APPUI_METHOD_AUTH_VERIFY).or_else(|| {
        if !state.has_email() {
            Some("email is empty".into())
        } else if !state.has_otp_code() {
            Some("OTP code is empty".into())
        } else {
            None
        }
    })
}

fn onboarding_provider_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    method: &'static str,
) -> Option<String> {
    action_missing_reason(ctx, method).or_else(|| {
        if !state.selection_ready() {
            Some("provider selection is incomplete".into())
        } else if !state.has_api_key() {
            Some("API key is empty".into())
        } else {
            None
        }
    })
}

fn onboarding_open_session_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> Option<String> {
    onboarding_finish_disabled_reason(ctx, state, current_profile)
        .or_else(|| {
            (!onboarding_has_saved_primary_provider(ctx, state, current_profile))
                .then_some("save provider first".into())
        })
        // M22-C: finish is disabled until workspace validation
        // reports `Valid` so `session/open` never fires against an
        // unverified cwd.
        .or_else(|| onboarding_workspace_disabled_reason(state))
}

fn onboarding_workspace_disabled_reason(state: &OnboardingWizardState) -> Option<String> {
    match &state.workspace_validation {
        crate::model::OnboardingWorkspaceValidation::Valid { .. } => None,
        crate::model::OnboardingWorkspaceValidation::Unvalidated => {
            Some("validate workspace first".into())
        }
        crate::model::OnboardingWorkspaceValidation::Validating => {
            Some("workspace validation in progress".into())
        }
        crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
            Some(format!("workspace invalid: {reason}"))
        }
    }
}

fn onboarding_workspace_display(state: &OnboardingWizardState, active_workspace: &str) -> String {
    match &state.workspace_validation {
        crate::model::OnboardingWorkspaceValidation::Valid { canonical, .. } => canonical.clone(),
        _ => state
            .workspace_candidate
            .clone()
            .or_else(|| {
                let trimmed = active_workspace.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(format!("{trimmed} (active)"))
                }
            })
            .unwrap_or_else(|| "(use /onboard workspace <path>)".into()),
    }
}

fn onboarding_workspace_status_label(state: &OnboardingWizardState) -> String {
    match &state.workspace_validation {
        crate::model::OnboardingWorkspaceValidation::Unvalidated => "Status: not validated".into(),
        crate::model::OnboardingWorkspaceValidation::Validating => "Status: validating...".into(),
        crate::model::OnboardingWorkspaceValidation::Valid {
            writable,
            has_workspace_toml,
            ..
        } => {
            let writable_label = if *writable { "writable" } else { "read-only" };
            let toml_label = if *has_workspace_toml {
                " · .octos-workspace.toml"
            } else {
                ""
            };
            format!("Status: OK ({writable_label}{toml_label})")
        }
        crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
            format!("Status: INVALID — {reason}")
        }
    }
}

fn onboarding_has_saved_primary_provider(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> bool {
    state.provider_saved
        || ctx
            .app
            .profile_llm_state
            .filter(|llm| {
                current_profile.is_none()
                    || llm
                        .profile_id
                        .as_deref()
                        .is_none_or(|profile_id| Some(profile_id) == current_profile)
            })
            .and_then(|llm| llm.primary_provider())
            .is_some_and(|provider| provider.has_api_key)
}

fn onboarding_provider_test_label(state: &OnboardingWizardState) -> String {
    match state.provider_pending {
        Some(OnboardingProviderPending::Test) => "Testing connection...".into(),
        Some(OnboardingProviderPending::Save) => "Test unavailable while saving".into(),
        None if state.provider_tested => "Connection tested".into(),
        None if state.provider_test_failure_reason.is_some() => {
            // M22-E: surface the typed test failure so the user
            // sees what went wrong and knows to edit the key or
            // pick a different route.
            let reason = state
                .provider_test_failure_reason
                .as_deref()
                .unwrap_or_default();
            format!("Test failed — {reason}")
        }
        None => "Test connection".into(),
    }
}

fn onboarding_provider_save_label(state: &OnboardingWizardState) -> &'static str {
    match state.provider_pending {
        Some(OnboardingProviderPending::Save) => "Saving provider...",
        Some(OnboardingProviderPending::Test) => "Save unavailable while testing",
        None if state.provider_saved && state.provider_tested => "Provider saved",
        None => "Save provider",
    }
}

fn onboarding_provider_fallback_label(state: &OnboardingWizardState) -> &'static str {
    match state.provider_pending {
        Some(OnboardingProviderPending::Save) => "Saving provider...",
        Some(OnboardingProviderPending::Test) => "Fallback unavailable while testing",
        None => "Add as fallback",
    }
}

fn onboarding_provider_saved_status_label(state: &OnboardingWizardState) -> String {
    if let (Some(target), Some(label)) = (
        state.last_saved_provider_target,
        state.last_saved_provider_label.as_deref(),
    ) {
        format!("Saved provider: {} {label}", save_target_label(target))
    } else if let Some(label) = state.saved_primary_provider_label.as_deref() {
        format!("Saved provider: primary {label}")
    } else {
        "Saved provider: none".into()
    }
}

fn onboarding_provider_saved_status_state(state: &OnboardingWizardState) -> MenuItemState {
    MenuItemState {
        checked: state.last_saved_provider_label.is_some().then_some(true),
        required_valid: state.last_saved_provider_label.as_ref().map(|_| true),
        ..MenuItemState::default()
    }
}

fn save_target_label(target: OnboardingProviderSaveTarget) -> &'static str {
    match target {
        OnboardingProviderSaveTarget::Primary => "primary",
        OnboardingProviderSaveTarget::Fallback => "fallback",
    }
}

fn onboarding_provider_test_state(state: &OnboardingWizardState) -> MenuItemState {
    MenuItemState {
        checked: state.provider_tested.then_some(true),
        loading: state.provider_pending == Some(OnboardingProviderPending::Test),
        ..MenuItemState::default()
    }
}

fn onboarding_provider_save_state(state: &OnboardingWizardState) -> MenuItemState {
    MenuItemState {
        checked: (state.provider_saved && state.provider_tested).then_some(true),
        loading: state.provider_pending == Some(OnboardingProviderPending::Save),
        ..MenuItemState::default()
    }
}

fn onboarding_fetch_models_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
) -> Option<String> {
    action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_FETCH_MODELS).or_else(|| {
        let route = &state.provider.route;
        (route.route_id.trim().is_empty()
            && route
                .base_url
                .as_deref()
                .is_none_or(|url| url.trim().is_empty()))
        .then(|| "route id or base url is required".into())
    })
}

fn onboarding_preview_rows(
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> Vec<MenuPreviewRow> {
    vec![
        MenuPreviewRow {
            label: "name".into(),
            value: if state.has_name() {
                state.name.clone()
            } else {
                "<unset>".into()
            },
        },
        MenuPreviewRow {
            label: "username".into(),
            value: if state.has_username() {
                state.username.clone()
            } else {
                "<unset>".into()
            },
        },
        MenuPreviewRow {
            label: "profile".into(),
            value: state.profile_label(current_profile),
        },
        MenuPreviewRow {
            label: "email".into(),
            value: if state.has_email() {
                state.email.clone()
            } else {
                "<unset>".into()
            },
        },
        MenuPreviewRow {
            label: "auth".into(),
            value: if state.auth_verified {
                "verified".into()
            } else if state.auth_code_sent {
                "code sent".into()
            } else {
                "not verified".into()
            },
        },
        MenuPreviewRow {
            label: "provider".into(),
            value: state.provider_label(),
        },
        MenuPreviewRow {
            label: "api_key".into(),
            value: if state.has_api_key() {
                state.api_key_label().into()
            } else {
                "<unset>".into()
            },
        },
        MenuPreviewRow {
            label: "saved".into(),
            value: state.provider_saved.to_string(),
        },
        MenuPreviewRow {
            label: "last".into(),
            value: state
                .last_message
                .clone()
                .unwrap_or_else(|| "open /onboard to begin".into()),
        },
    ]
}

#[derive(Debug, Clone)]
struct CatalogChoice {
    id: String,
    label: String,
    description: String,
    selection: LlmSelectionConfig,
}

fn onboarding_catalog_items(ctx: &MenuContext<'_>, state: &OnboardingWizardState) -> Vec<MenuItem> {
    catalog_menu_items(
        ctx,
        state,
        "onboard.catalog",
        "run Refresh dashboard provider catalog first",
    )
}

fn provider_catalog_items(ctx: &MenuContext<'_>, state: &OnboardingWizardState) -> Vec<MenuItem> {
    catalog_menu_items(
        ctx,
        state,
        "provider.catalog.choice",
        "run Refresh provider catalog first",
    )
}

fn catalog_menu_items(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    id_prefix: &str,
    missing_reason: &'static str,
) -> Vec<MenuItem> {
    let Some(catalog) = ctx.app.profile_llm_catalog else {
        return vec![
            MenuItem::new(
                format!("{id_prefix}.empty"),
                "Provider catalog: not loaded",
                MenuAction::Noop,
            )
            .disabled(missing_reason),
        ];
    };

    let mut choices = catalog_choices(&catalog.families);
    choices.sort_by_key(catalog_choice_rank);
    choices
        .into_iter()
        .take(12)
        .map(|choice| {
            let item_id = format!("{id_prefix}.{}", choice.id);
            let mut item = MenuItem::new(
                item_id,
                choice.label,
                MenuAction::Local(LocalAction::Onboarding(
                    OnboardingAction::SetProviderSelection(choice.selection.clone()),
                )),
            )
            .with_description(choice.description);
            if state.provider == choice.selection {
                item = item.with_state(MenuItemState::current());
            }
            item
        })
        .collect()
}

fn catalog_choice_rank(choice: &CatalogChoice) -> (u8, String) {
    let route_id = choice.selection.route.route_id.as_str();
    let family = choice.selection.family_id.as_str();
    let score = match (family, route_id) {
        ("moonshot", "autodl") => 0,
        ("minimax", "wisemodel") => 1,
        ("deepseek", "autodl") => 2,
        ("deepseek", _) => 3,
        ("openai", _) => 4,
        ("anthropic", _) => 5,
        _ => 9,
    };
    (score, choice.label.clone())
}

fn catalog_choices(families: &serde_json::Map<String, Value>) -> Vec<CatalogChoice> {
    let mut choices = Vec::new();
    for (family_id, family) in families {
        let family_env = family
            .get("env")
            .and_then(Value::as_str)
            .filter(|env| !env.is_empty())
            .map(str::to_owned);
        let Some(models) = family.get("models").and_then(Value::as_array) else {
            continue;
        };
        for model in models {
            let Some(model_id) = model.get("id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(endpoints) = model.get("endpoints").and_then(Value::as_array) {
                for endpoint in endpoints {
                    choices.push(catalog_endpoint_choice(
                        family_id,
                        model_id,
                        model,
                        family_env.clone(),
                        endpoint,
                    ));
                }
            } else {
                let route_id = family_id.clone();
                let selection = LlmSelectionConfig {
                    family_id: family_id.clone(),
                    model_id: model_id.to_owned(),
                    route: LlmRouteConfig {
                        route_id: route_id.clone(),
                        label: Some("Official API".into()),
                        base_url: None,
                        api_key_env: family_env.clone(),
                        api_type: Some("openai".into()),
                    },
                    model_hints: model.get("model_hints").cloned(),
                    cost_per_m: model.get("cost_per_m").cloned(),
                    strong: model.get("strong").and_then(Value::as_bool),
                };
                choices.push(CatalogChoice {
                    id: format!("onboard.catalog.{family_id}.{model_id}.{route_id}"),
                    label: format!("{family_id} / {model_id}"),
                    description: format!(
                        "Official route{}",
                        family_env
                            .as_deref()
                            .map(|env| format!("; key env {env}"))
                            .unwrap_or_default()
                    ),
                    selection,
                });
            }
        }
    }
    choices
}

fn catalog_endpoint_choice(
    family_id: &str,
    model_id: &str,
    model: &Value,
    family_env: Option<String>,
    endpoint: &Value,
) -> CatalogChoice {
    let route_id = endpoint
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(family_id)
        .to_owned();
    let label = endpoint
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("Official API")
        .to_owned();
    let base_url = endpoint
        .get("base_url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);
    let api_key_env = endpoint
        .get("api_key_env")
        .and_then(Value::as_str)
        .filter(|env| !env.is_empty())
        .map(str::to_owned)
        .or(family_env);
    let selection = LlmSelectionConfig {
        family_id: family_id.to_owned(),
        model_id: model_id.to_owned(),
        route: LlmRouteConfig {
            route_id: route_id.clone(),
            label: Some(label.clone()),
            base_url: base_url.clone(),
            api_key_env: api_key_env.clone(),
            api_type: Some("openai".into()),
        },
        model_hints: model.get("model_hints").cloned(),
        cost_per_m: model.get("cost_per_m").cloned(),
        strong: model.get("strong").and_then(Value::as_bool),
    };
    CatalogChoice {
        id: format!("onboard.catalog.{family_id}.{model_id}.{route_id}"),
        label: format!("{family_id} / {model_id} via {label}"),
        description: [
            base_url.map(|url| format!("base {url}")),
            api_key_env.map(|env| format!("key env {env}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; "),
        selection,
    }
}

fn login_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if !supports_any_method(ctx, crate::menu::registry::APPUI_LOGIN_MENU_METHODS_ANY) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_LOGIN),
            title: t!("menu.login.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_AUTH_STATUS),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };

    let mut items = vec![
        MenuItem::new(
            "login.status",
            t!("menu.login.item.auth_status.label"),
            MenuAction::SendAppUi(AppUiCommand::AuthStatus(AuthStatusParams::default())),
        )
        .with_description("Uses auth/status.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_STATUS)),
        MenuItem::new(
            "login.me",
            t!("menu.login.item.current_account.label"),
            MenuAction::SendAppUi(AppUiCommand::AuthMe(AuthMeParams {
                token: state.auth_token.clone(),
            })),
        )
        .with_description("Uses auth/me.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_ME)),
        MenuItem::new(
            "login.logout",
            t!("menu.login.item.logout.label"),
            MenuAction::SendAppUi(AppUiCommand::AuthLogout(AuthLogoutParams {
                token: state.auth_token.clone(),
            })),
        )
        .with_description("Uses auth/logout.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_LOGOUT)),
    ];

    if !local_profile_create_supported(ctx)
        && ctx
            .availability
            .supports_method(APPUI_METHOD_AUTH_SEND_CODE)
        && ctx.availability.supports_method(APPUI_METHOD_AUTH_VERIFY)
    {
        items.push(
            MenuItem::new(
                "login.email",
                if state.has_email() {
                    format!("Email: {}", state.email)
                } else {
                    "Email: not set".into()
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.login.item.email.desc")),
        );
        items.push(
            MenuItem::new(
                "login.otp.send",
                t!("menu.login.item.otp_send.label"),
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SendCode)),
            )
            .with_description("Uses auth/send_code.")
            .maybe_disabled(onboarding_disabled_reason(
                ctx,
                state,
                APPUI_METHOD_AUTH_SEND_CODE,
                "email is empty",
            )),
        );
        items.push(
            MenuItem::new(
                "login.code",
                if state.has_otp_code() {
                    t!("menu.login.item.otp_code_set.label").into_owned()
                } else {
                    t!("menu.login.item.otp_code_unset.label").into_owned()
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.login.item.otp_code.desc")),
        );
        items.push(
            MenuItem::new(
                "login.otp.verify",
                t!("menu.login.item.otp_verify.label"),
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::VerifyCode)),
            )
            .with_description("Uses auth/verify.")
            .maybe_disabled(onboarding_verify_disabled_reason(ctx, state)),
        );
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_LOGIN),
        title: t!("menu.login.title").into_owned(),
        subtitle: Some(t!("menu.login.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.enter_run_esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.login.preview_title").into_owned()),
            rows: [
                MenuPreviewRow {
                    label: "email".into(),
                    value: if state.has_email() {
                        state.email.clone()
                    } else {
                        "<unset>".into()
                    },
                },
                MenuPreviewRow {
                    label: "otp".into(),
                    value: if state.has_otp_code() {
                        "set".into()
                    } else {
                        "<unset>".into()
                    },
                },
                MenuPreviewRow {
                    label: "auth".into(),
                    value: if state.auth_verified {
                        "verified".into()
                    } else if state.auth_code_sent {
                        "code sent".into()
                    } else {
                        "not verified".into()
                    },
                },
            ]
            .into_iter()
            .chain(
                [
                    APPUI_METHOD_AUTH_STATUS,
                    APPUI_METHOD_AUTH_SEND_CODE,
                    APPUI_METHOD_AUTH_VERIFY,
                    APPUI_METHOD_AUTH_ME,
                    APPUI_METHOD_AUTH_LOGOUT,
                ]
                .into_iter()
                .map(|method| permission_method_row(ctx, method)),
            )
            .collect(),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn model_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let can_list = ctx.availability.supports_method(APPUI_METHOD_MODEL_LIST);
    let can_select = ctx.availability.supports_method(APPUI_METHOD_MODEL_SELECT);

    let profile_id = ctx.app.current_profile.map(str::to_owned).or_else(|| {
        ctx.app
            .profile_llm_state
            .and_then(|state| state.profile_id.clone())
    });
    let profile_models;
    let models = if let Some(catalog) = ctx.app.model_catalog {
        Some(catalog.models.as_slice())
    } else if let Some(profile_llm_state) = ctx.app.profile_llm_state {
        profile_models = profile_llm_state.models();
        Some(profile_models.as_slice())
    } else {
        None
    };

    let refresh_action = if can_list {
        MenuAction::SendAppUi(AppUiCommand::ProfileLlmList(ProfileLlmListParams {
            profile_id: profile_id.clone(),
        }))
    } else {
        MenuAction::Noop
    };
    let mut refresh = MenuItem::new(
        "model.refresh",
        t!("menu.model.item.refresh.label"),
        refresh_action,
    )
    .with_description("Uses profile/llm/list.");
    if !can_list {
        refresh = refresh.disabled(method_missing_reason(ctx, APPUI_METHOD_MODEL_LIST));
    }
    let mut items = vec![refresh];

    if let Some(models) = models {
        if models.is_empty() {
            items.push(
                MenuItem::new(
                    "model.empty",
                    t!("menu.model.item.empty.label"),
                    MenuAction::Noop,
                )
                .disabled("profile/llm/list returned no models for this profile"),
            );
        } else {
            for (idx, model) in models.iter().enumerate() {
                let id = format!("model.select.{idx}");
                let mut state = MenuItemState::default();
                state.current = model.selected
                    || ctx
                        .app
                        .current_model
                        .is_some_and(|current| current == model.model);
                let action = if can_select {
                    MenuAction::SendAppUi(AppUiCommand::ProfileLlmSelect(ProfileLlmSelectParams {
                        profile_id: profile_id.clone(),
                        family_id: model
                            .family
                            .clone()
                            .unwrap_or_else(|| model.provider.clone()),
                        model_id: model.model.clone(),
                        route_id: model.route.clone().unwrap_or_else(|| "official".into()),
                    }))
                } else {
                    MenuAction::Noop
                };
                let mut item = MenuItem::new(id, model_label(model), action)
                    .with_description(model_description(model))
                    .with_state(state);
                if let Some(shortcut) = numeric_shortcut(idx + 1) {
                    item = item.with_shortcut(shortcut);
                }
                if !can_select {
                    item = item.disabled(method_missing_reason(ctx, APPUI_METHOD_MODEL_SELECT));
                }
                if model.available == Some(false) {
                    item = item.disabled("server reports this model is unavailable");
                }
                items.push(item);
            }
        }
    } else {
        items.push(
            MenuItem::new(
                "model.cached",
                t!("menu.model.item.cached.label"),
                MenuAction::Noop,
            )
            .disabled(if can_list {
                "No cached profile/llm/list result yet; refresh models first".into()
            } else {
                method_missing_reason(ctx, APPUI_METHOD_MODEL_LIST)
            }),
        );
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_MODEL),
        title: t!("menu.model.title").into_owned(),
        subtitle: Some(t!("menu.model.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.model.search").into_owned()),
        footer_hint: Some(t!("menu.model.footer").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.runtime_preview_title").into_owned()),
            rows: model_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn provider_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if !supports_any_method(ctx, APPUI_PROVIDER_MENU_METHODS_ANY) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_PROVIDER),
            title: t!("menu.provider.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_CATALOG),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let profile_id = ctx.app.current_profile.map(str::to_owned);
    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };
    let mut items = vec![
        MenuItem::new(
            "provider.catalog",
            t!("menu.provider.item.catalog_refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ProfileLlmCatalog(
                ProfileLlmCatalogParams::default(),
            )),
        )
        .with_description("Uses profile/llm/catalog.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_CATALOG)),
        MenuItem::new(
            "provider.list",
            t!("menu.provider.item.list_refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ProfileLlmList(ProfileLlmListParams {
                profile_id: profile_id.clone(),
            })),
        )
        .with_description("Uses profile/llm/list.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_MODEL_LIST)),
    ];

    items.extend(provider_saved_items(ctx));
    items.extend(provider_catalog_items(ctx, state));
    items.extend([
        MenuItem::new(
            "provider.current",
            format!("Staged provider: {}", state.provider_label()),
            MenuAction::Noop,
        )
        .with_description(t!("menu.provider.item.staged.desc"))
        .with_state(MenuItemState::required(state.selection_ready())),
        MenuItem::new(
            "provider.saved",
            onboarding_provider_saved_status_label(state),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.saved_provider.desc"))
        .with_state(onboarding_provider_saved_status_state(state)),
        MenuItem::new(
            "provider.key",
            if state.has_api_key() {
                format!("API key: {}", state.api_key_label())
            } else {
                "API key: not set".into()
            },
            MenuAction::Noop,
        )
        .with_description(
            "Use /provider key <secret>. The secret is masked in state, logs, and snapshots.",
        )
        .with_state(MenuItemState::required(state.has_api_key())),
        MenuItem::new(
            "provider.fetch",
            t!("menu.onboard.item.fetch_models.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::FetchModels)),
        )
        .with_description(t!("menu.onboard.item.fetch_models.desc"))
        .maybe_disabled(onboarding_fetch_models_disabled_reason(ctx, state)),
        MenuItem::new(
            "provider.test",
            onboarding_provider_test_label(state),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::TestProvider)),
        )
        .with_description("Uses profile/llm/test with the dashboard-shaped selection.")
        .with_state(onboarding_provider_test_state(state))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_TEST,
        )),
        MenuItem::new(
            "provider.save",
            onboarding_provider_save_label(state),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SaveProvider)),
        )
        .with_description(t!("menu.onboard.item.save_provider.desc"))
        .with_state(onboarding_provider_save_state(state))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        )),
        MenuItem::new(
            "provider.fallback",
            onboarding_provider_fallback_label(state),
            MenuAction::Local(LocalAction::Onboarding(
                OnboardingAction::SaveProviderFallback,
            )),
        )
        .with_description("Append or replace the staged provider under config.llm.fallbacks.")
        .with_state(onboarding_provider_save_state(state))
        .maybe_disabled(onboarding_provider_disabled_reason(
            ctx,
            state,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        )),
    ]);

    if let Some(catalog) = ctx.app.model_catalog {
        for model in &catalog.models {
            let family_id = model
                .family
                .clone()
                .unwrap_or_else(|| model.provider.clone());
            let route_id = model.route.clone().unwrap_or_else(|| "official".into());
            items.push(
                MenuItem::new(
                    format!("provider.test.{family_id}.{}.{}", model.model, route_id),
                    format!("Test {} / {}", model.provider, model.model),
                    MenuAction::SendAppUi(AppUiCommand::ProfileLlmTest(ProfileLlmTestParams {
                        profile_id: profile_id.clone(),
                        selection: LlmSelectionConfig {
                            family_id: family_id.clone(),
                            model_id: model.model.clone(),
                            route: LlmRouteConfig {
                                route_id,
                                label: None,
                                base_url: None,
                                api_key_env: None,
                                api_type: Some("openai".into()),
                            },
                            ..LlmSelectionConfig::default()
                        },
                        api_key: None,
                    })),
                )
                .with_description("Uses profile/llm/test; API key is never rendered.")
                .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_TEST)),
            );
        }
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_PROVIDER),
        title: t!("menu.provider.title").into_owned(),
        subtitle: Some(t!("menu.provider.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.provider.search").into_owned()),
        footer_hint: Some(t!("menu.footer.enter_run_esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.provider.preview_title").into_owned()),
            rows: [
                MenuPreviewRow {
                    label: "profile".into(),
                    value: state.profile_label(ctx.app.current_profile),
                },
                MenuPreviewRow {
                    label: "staged".into(),
                    value: state.provider_label(),
                },
                MenuPreviewRow {
                    label: "api_key".into(),
                    value: if state.has_api_key() {
                        state.api_key_label().into()
                    } else {
                        "<unset>".into()
                    },
                },
            ]
            .into_iter()
            .chain(
                [
                    APPUI_METHOD_PROFILE_LLM_CATALOG,
                    APPUI_METHOD_MODEL_LIST,
                    APPUI_METHOD_PROFILE_LLM_UPSERT,
                    APPUI_METHOD_PROFILE_LLM_DELETE,
                    APPUI_METHOD_MODEL_SELECT,
                    APPUI_METHOD_PROFILE_LLM_TEST,
                ]
                .into_iter()
                .map(|method| permission_method_row(ctx, method)),
            )
            .collect(),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn provider_saved_items(ctx: &MenuContext<'_>) -> Vec<MenuItem> {
    let Some(profile_llm) = ctx.app.profile_llm_state else {
        return vec![
            MenuItem::new(
                "provider.saved.unloaded",
                t!("menu.provider.item.saved_unloaded.label"),
                MenuAction::Noop,
            )
            .with_description(t!("menu.provider.item.saved_unloaded.desc"))
            .disabled("not loaded"),
        ];
    };

    let mut items = Vec::new();
    if let Some(primary) = profile_llm.primary_provider() {
        items.push(configured_provider_item(
            "provider.saved.primary",
            t!("menu.provider.item.saved_primary.prefix").as_ref(),
            primary,
        ));
    } else {
        items.push(
            MenuItem::new(
                "provider.saved.primary.empty",
                t!("menu.provider.item.saved_primary_empty.label"),
                MenuAction::Noop,
            )
            .with_description(t!("menu.provider.item.saved_primary_empty.desc"))
            .with_state(MenuItemState::required(false)),
        );
    }

    let fallbacks = profile_llm.fallback_providers();
    if fallbacks.is_empty() {
        items.push(
            MenuItem::new(
                "provider.saved.fallback.empty",
                t!("menu.provider.item.saved_fallback_empty.label"),
                MenuAction::Noop,
            )
            .with_description(t!("menu.provider.item.saved_fallback_empty.desc"))
            .with_state(MenuItemState::required(false)),
        );
    } else {
        items.extend(fallbacks.iter().enumerate().map(|(idx, provider)| {
            configured_provider_item(
                format!("provider.saved.fallback.{idx}"),
                t!("menu.provider.item.saved_fallback.prefix", n = idx + 1).as_ref(),
                provider,
            )
        }));
    }

    items
}

fn configured_provider_item(
    id: impl Into<String>,
    prefix: &str,
    provider: &LlmConfiguredProvider,
) -> MenuItem {
    MenuItem::new(
        id,
        format!("{prefix}: {}", configured_provider_label(provider)),
        MenuAction::Noop,
    )
    .with_description(configured_provider_description(provider))
    .with_state(MenuItemState {
        current: provider.selected,
        checked: Some(true),
        required_valid: Some(provider.has_api_key),
        ..MenuItemState::default()
    })
}

fn configured_provider_label(provider: &LlmConfiguredProvider) -> String {
    format!(
        "{} / {} via {}",
        configured_provider_family(provider),
        configured_provider_model(provider),
        configured_provider_route_id(provider)
    )
}

fn configured_provider_description(provider: &LlmConfiguredProvider) -> String {
    let mut parts = vec![if provider.has_api_key {
        "api key saved".to_string()
    } else {
        "api key missing".to_string()
    }];
    if let Some(api_key_env) = configured_provider_api_key_env(provider) {
        parts.push(format!("env: {api_key_env}"));
    }
    if let Some(base_url) = configured_provider_base_url(provider) {
        parts.push(format!("base: {base_url}"));
    }
    parts.join(" | ")
}

fn configured_provider_family(provider: &LlmConfiguredProvider) -> String {
    non_empty_str(provider.family_id.as_deref())
        .or_else(|| non_empty_str(Some(provider.provider.as_str())))
        .unwrap_or("unknown")
        .to_owned()
}

fn configured_provider_model(provider: &LlmConfiguredProvider) -> String {
    non_empty_str(provider.model_id.as_deref())
        .or_else(|| non_empty_str(Some(provider.model.as_str())))
        .unwrap_or("unknown")
        .to_owned()
}

fn configured_provider_route_id(provider: &LlmConfiguredProvider) -> String {
    non_empty_str(provider.route_id.as_deref())
        .or_else(|| {
            provider
                .route
                .as_ref()
                .and_then(|route| non_empty_str(Some(route.route_id.as_str())))
        })
        .unwrap_or("official")
        .to_owned()
}

fn configured_provider_base_url(provider: &LlmConfiguredProvider) -> Option<&str> {
    non_empty_str(provider.base_url.as_deref()).or_else(|| {
        provider
            .route
            .as_ref()
            .and_then(|route| non_empty_str(route.base_url.as_deref()))
    })
}

fn configured_provider_api_key_env(provider: &LlmConfiguredProvider) -> Option<&str> {
    non_empty_str(provider.api_key_env.as_deref()).or_else(|| {
        provider
            .route
            .as_ref()
            .and_then(|route| non_empty_str(route.api_key_env.as_deref()))
    })
}

fn non_empty_str(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn mcp_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if !supports_any_method(ctx, APPUI_MCP_MENU_METHODS_ANY) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_MCP),
            title: t!("menu.mcp.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_MCP_CONFIG_LIST),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let profile_id = ctx.app.current_profile.map(ToOwned::to_owned);
    let session_id = ctx.app.selected_session_id.cloned();
    let mut items = Vec::new();

    items.push(
        MenuItem::new(
            "mcp.config.refresh",
            t!("menu.mcp.item.config_refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ListMcpConfig(McpConfigListParams {
                session_id: session_id.clone(),
                profile_id: profile_id.clone(),
                include_disabled: true,
            })),
        )
        .with_description("Uses mcp/config/list.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_MCP_CONFIG_LIST)),
    );

    if let Some(session_id) = session_id.clone() {
        items.push(
            MenuItem::new(
                "mcp.refresh",
                t!("menu.mcp.item.status_refresh.label"),
                MenuAction::SendAppUi(AppUiCommand::ListMcpStatus(McpStatusListParams {
                    session_id,
                    include_disabled: true,
                })),
            )
            .with_description("Uses mcp/status/list.")
            .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_MCP_STATUS_LIST)),
        );
    }

    items.push(
        MenuItem::new(
            "mcp.config.upsert",
            t!("menu.mcp.item.upsert.label"),
            MenuAction::Local(LocalAction::EditComposer("/mcp upsert ".into())),
        )
        .with_description("Edit as: /mcp upsert <server> {json}")
        .maybe_disabled(mutating_action_missing_reason(
            ctx,
            APPUI_METHOD_MCP_CONFIG_UPSERT,
        )),
    );

    if let Some(config) = ctx.app.mcp_config_catalog {
        if config.servers.is_empty() {
            items.push(
                MenuItem::new(
                    "mcp.empty",
                    t!("menu.mcp.item.empty.label"),
                    MenuAction::Noop,
                )
                .disabled("mcp/config/list returned no configured servers"),
            );
        } else {
            for server in &config.servers {
                let server_name = mcp_config_server_name(server);
                let state = MenuItemState {
                    checked: Some(server.enabled),
                    destructive: server.last_error.is_some(),
                    ..MenuItemState::default()
                };
                items.push(
                    MenuItem::new(
                        format!("mcp.server.{server_name}.toggle"),
                        mcp_config_label(server),
                        MenuAction::SendAppUi(AppUiCommand::SetMcpConfigEnabled(
                            McpConfigSetEnabledParams {
                                profile_id: profile_id.clone(),
                                server: server_name.clone(),
                                enabled: !server.enabled,
                            },
                        )),
                    )
                    .with_description(mcp_config_description(server))
                    .with_state(state)
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_MCP_CONFIG_SET_ENABLED,
                    )),
                );
                items.push(
                    MenuItem::new(
                        format!("mcp.server.{server_name}.test"),
                        format!("Test {server_name}"),
                        MenuAction::SendAppUi(AppUiCommand::TestMcpConfig(McpConfigTestParams {
                            session_id: session_id.clone(),
                            profile_id: profile_id.clone(),
                            server: server_name.clone(),
                        })),
                    )
                    .with_description("Uses mcp/config/test.")
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_MCP_CONFIG_TEST,
                    )),
                );
                let mut delete_state = MenuItemState::default();
                delete_state.destructive = true;
                items.push(
                    MenuItem::new(
                        format!("mcp.server.{server_name}.delete"),
                        format!("Delete {server_name}"),
                        MenuAction::SendAppUi(AppUiCommand::DeleteMcpConfig(
                            McpConfigDeleteParams {
                                profile_id: profile_id.clone(),
                                server: server_name,
                            },
                        )),
                    )
                    .with_description("Uses mcp/config/delete.")
                    .with_state(delete_state)
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_MCP_CONFIG_DELETE,
                    )),
                );
            }
        }
    } else if let Some(catalog) = ctx.app.mcp_catalog {
        if catalog.servers.is_empty() {
            items.push(
                MenuItem::new(
                    "mcp.status.empty",
                    t!("menu.mcp.item.empty.label"),
                    MenuAction::Noop,
                )
                .disabled("mcp/status/list returned no servers for this session"),
            );
        } else {
            for server in &catalog.servers {
                let state = MenuItemState {
                    destructive: server.status == "failed",
                    ..MenuItemState::default()
                };
                items.push(
                    MenuItem::new(
                        format!("mcp.status.server.{}", server.server),
                        server.server.clone(),
                        MenuAction::Noop,
                    )
                    .with_description(mcp_server_description(server))
                    .with_state(state),
                );
            }
        }
    } else {
        items.push(
            MenuItem::new(
                "mcp.cached",
                t!("menu.mcp.item.cached.label"),
                MenuAction::Noop,
            )
            .disabled("Run Refresh MCP config first"),
        );
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_MCP),
        title: t!("menu.mcp.title").into_owned(),
        subtitle: Some(t!("menu.mcp.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.mcp.search").into_owned()),
        footer_hint: Some(t!("menu.footer.enter_run_esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.runtime_preview_title").into_owned()),
            rows: mcp_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn tool_settings_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if !supports_any_method(ctx, APPUI_TOOL_SETTINGS_MENU_METHODS_ANY) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_TOOL_SETTINGS),
            title: t!("menu.tools.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_TOOL_CONFIG_LIST),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let profile_id = ctx.app.current_profile.map(ToOwned::to_owned);
    let session_id = ctx.app.selected_session_id.cloned();
    let mut items = Vec::new();

    items.push(
        MenuItem::new(
            "tools.config.refresh",
            t!("menu.tools.item.config_refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ListToolConfig(ToolConfigListParams {
                session_id: session_id.clone(),
                profile_id: profile_id.clone(),
                include_disabled: true,
            })),
        )
        .with_description("Uses tool/config/list.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_TOOL_CONFIG_LIST)),
    );

    if let Some(session_id) = session_id.clone() {
        items.push(
            MenuItem::new(
                "tools.status.refresh",
                t!("menu.tools.item.status_refresh.label"),
                MenuAction::SendAppUi(AppUiCommand::ListToolStatus(ToolStatusListParams {
                    session_id,
                    include_denied: true,
                })),
            )
            .with_description("Uses tool/status/list.")
            .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_TOOL_STATUS_LIST)),
        );
    }

    items.push(
        MenuItem::new(
            "tools.config.upsert",
            t!("menu.tools.item.upsert.label"),
            MenuAction::Local(LocalAction::EditComposer("/tools upsert ".into())),
        )
        .with_description("Edit as: /tools upsert <tool> {json}")
        .maybe_disabled(mutating_action_missing_reason(
            ctx,
            APPUI_METHOD_TOOL_CONFIG_UPSERT,
        )),
    );

    if let Some(contract) = ctx
        .app
        .tool_catalog
        .and_then(|catalog| catalog.coding_tool_contract.as_ref())
    {
        let ready = coding_contract_is_ready(contract);
        items.push(
            MenuItem::new(
                "tools.contract",
                t!("menu.tools.item.contract.label"),
                MenuAction::Noop,
            )
            .with_description(coding_contract_description(contract))
            .with_state(MenuItemState {
                required_valid: Some(ready),
                ..MenuItemState::default()
            }),
        );
        for tool_name in &contract.missing_required_tools {
            let state = MenuItemState {
                required_valid: Some(false),
                destructive: true,
                ..MenuItemState::default()
            };
            items.push(
                MenuItem::new(
                    format!("tools.contract.missing.{tool_name}"),
                    format!(
                        "{}: {tool_name}",
                        t!("menu.tools.item.contract_missing.prefix")
                    ),
                    MenuAction::Noop,
                )
                .with_description(coding_contract_missing_tool_description(
                    contract, tool_name,
                ))
                .with_state(state),
            );
        }
    }

    if let Some(config) = ctx.app.tool_config_catalog {
        if config.tools.is_empty() {
            items.push(
                MenuItem::new(
                    "tools.empty",
                    t!("menu.tools.item.empty.label"),
                    MenuAction::Noop,
                )
                .disabled("tool/config/list returned no configured tools"),
            );
        } else {
            for tool in &config.tools {
                let tool_name = tool_config_name(tool);
                let state = MenuItemState {
                    checked: Some(tool.enabled),
                    destructive: tool.risk.as_deref() == Some("high"),
                    ..MenuItemState::default()
                };
                items.push(
                    MenuItem::new(
                        format!("tools.tool.{tool_name}.toggle"),
                        tool_config_label(tool),
                        MenuAction::SendAppUi(AppUiCommand::SetToolConfigEnabled(
                            ToolConfigSetEnabledParams {
                                profile_id: profile_id.clone(),
                                tool: tool_name.clone(),
                                enabled: !tool.enabled,
                            },
                        )),
                    )
                    .with_description(tool_config_description(tool))
                    .with_state(state)
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_TOOL_CONFIG_SET_ENABLED,
                    )),
                );
                items.push(
                    MenuItem::new(
                        format!("tools.tool.{tool_name}.test"),
                        format!("Test {tool_name}"),
                        MenuAction::SendAppUi(AppUiCommand::TestToolConfig(ToolConfigTestParams {
                            session_id: session_id.clone(),
                            profile_id: profile_id.clone(),
                            tool: tool_name.clone(),
                        })),
                    )
                    .with_description("Uses tool/config/test.")
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_TOOL_CONFIG_TEST,
                    )),
                );
                let mut delete_state = MenuItemState::default();
                delete_state.destructive = true;
                items.push(
                    MenuItem::new(
                        format!("tools.tool.{tool_name}.delete"),
                        format!("Delete {tool_name}"),
                        MenuAction::SendAppUi(AppUiCommand::DeleteToolConfig(
                            ToolConfigDeleteParams {
                                profile_id: profile_id.clone(),
                                tool: tool_name,
                            },
                        )),
                    )
                    .with_description("Uses tool/config/delete.")
                    .with_state(delete_state)
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_TOOL_CONFIG_DELETE,
                    )),
                );
            }
        }
    } else if let Some(catalog) = ctx.app.tool_catalog {
        if catalog.tools.is_empty() {
            items.push(
                MenuItem::new(
                    "tools.status.empty",
                    t!("menu.tools.item.status_empty.label"),
                    MenuAction::Noop,
                )
                .disabled("tool/status/list returned no tools for this session"),
            );
        } else {
            for tool in &catalog.tools {
                let mut state = MenuItemState::default();
                state.checked = Some(tool.enabled);
                state.destructive = tool.denial.is_some();
                items.push(
                    MenuItem::new(
                        format!("tools.status.{}", tool.tool),
                        tool.title.clone().unwrap_or_else(|| tool.tool.clone()),
                        MenuAction::Noop,
                    )
                    .with_description(tool_status_description(tool))
                    .with_state(state),
                );
            }
        }
    } else {
        items.push(
            MenuItem::new(
                "tools.cached",
                t!("menu.tools.item.cached.label"),
                MenuAction::Noop,
            )
            .disabled("Run Refresh tool config first"),
        );
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_TOOL_SETTINGS),
        title: t!("menu.tools.title").into_owned(),
        subtitle: Some(t!("menu.tools.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.tools.search").into_owned()),
        footer_hint: Some(t!("menu.footer.enter_run_esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.runtime_preview_title").into_owned()),
            rows: tool_settings_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn skills_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if !supports_any_method(
        ctx,
        &[
            APPUI_METHOD_PROFILE_SKILLS_LIST,
            APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
            APPUI_METHOD_PROFILE_SKILLS_INSTALL,
            APPUI_METHOD_PROFILE_SKILLS_REMOVE,
        ],
    ) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_SKILLS),
            title: t!("menu.skills.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_PROFILE_SKILLS_LIST),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let profile_id = ctx.app.current_profile.map(ToOwned::to_owned);
    let mut items = vec![
        MenuItem::new(
            "skills.refresh",
            t!("menu.skills.item.refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ProfileSkillsList(ProfileSkillsListParams {
                profile_id: profile_id.clone(),
            })),
        )
        .with_description("Uses profile/skills/list.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_SKILLS_LIST)),
        MenuItem::new(
            "skills.search",
            t!("menu.skills.item.search.label"),
            MenuAction::Local(LocalAction::EditComposer("/skills search ".into())),
        )
        .with_description(t!("menu.skills.item.search.desc"))
        .maybe_disabled(action_missing_reason(
            ctx,
            APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
        )),
        MenuItem::new(
            "skills.install",
            t!("menu.skills.item.install.label"),
            MenuAction::Local(LocalAction::EditComposer("/skills install ".into())),
        )
        .with_description(t!("menu.skills.item.install.desc"))
        .maybe_disabled(mutating_action_missing_reason(
            ctx,
            APPUI_METHOD_PROFILE_SKILLS_INSTALL,
        )),
    ];

    if let Some(skills) = ctx.app.profile_skills {
        if skills.skills.is_empty() {
            items.push(
                MenuItem::new(
                    "skills.none",
                    t!("menu.skills.item.none.label"),
                    MenuAction::Noop,
                )
                .disabled("profile/skills/list returned no installed skills"),
            );
        } else {
            for skill in &skills.skills {
                let mut state = MenuItemState::default();
                state.destructive = true;
                items.push(
                    MenuItem::new(
                        format!("skills.remove.{}", skill.name),
                        format!("{} {}", t!("menu.skills.item.remove.prefix"), skill.name),
                        MenuAction::SendAppUi(AppUiCommand::ProfileSkillsRemove(
                            ProfileSkillsRemoveParams {
                                profile_id: profile_id.clone(),
                                name: skill.name.clone(),
                            },
                        )),
                    )
                    .with_description(installed_skill_description(skill))
                    .with_state(state)
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_PROFILE_SKILLS_REMOVE,
                    )),
                );
            }
        }
    } else {
        items.push(
            MenuItem::new(
                "skills.cache.empty",
                t!("menu.skills.item.cache_empty.label"),
                MenuAction::Noop,
            )
            .disabled("Run Refresh installed skills first"),
        );
    }

    if let Some(registry) = ctx.app.profile_skill_registry {
        for package in &registry.packages {
            let mut state = MenuItemState::default();
            state.checked = package.installed.then_some(true);
            items.push(
                MenuItem::new(
                    format!("skills.registry.{}", package.name),
                    format!("{} {}", t!("menu.skills.item.install.prefix"), package.name),
                    MenuAction::SendAppUi(AppUiCommand::ProfileSkillsInstall(
                        ProfileSkillsInstallParams {
                            profile_id: profile_id.clone(),
                            repo: package.repo.clone(),
                            branch: None,
                            force: false,
                        },
                    )),
                )
                .with_description(registry_package_description(package))
                .with_state(state)
                .maybe_disabled(mutating_action_missing_reason(
                    ctx,
                    APPUI_METHOD_PROFILE_SKILLS_INSTALL,
                )),
            );
        }
    }

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_SKILLS),
        title: t!("menu.skills.title").into_owned(),
        subtitle: Some(t!("menu.skills.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.skills.search").into_owned()),
        footer_hint: Some(t!("menu.footer.enter_run_esc_close").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.skills.preview_title").into_owned()),
            rows: vec![
                MenuPreviewRow {
                    label: "profile".into(),
                    value: profile_id.unwrap_or_else(|| "backend default".into()),
                },
                MenuPreviewRow {
                    label: "installed".into(),
                    value: ctx
                        .app
                        .profile_skills
                        .map(|skills| skills.skills.len().to_string())
                        .unwrap_or_else(|| "not loaded".into()),
                },
                MenuPreviewRow {
                    label: "registry".into(),
                    value: ctx
                        .app
                        .profile_skill_registry
                        .map(|registry| registry.packages.len().to_string())
                        .unwrap_or_else(|| "not searched".into()),
                },
            ],
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn permissions_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(session_id) = ctx.app.selected_session_id.cloned() else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_PERMISSIONS),
            title: t!("menu.permissions.unavailable_title").into_owned(),
            message: t!("menu.permissions.unavailable_no_session").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    };

    if !supports_any_permission_method(ctx) {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_PERMISSIONS),
            title: t!("menu.permissions.unavailable_title").into_owned(),
            message: permission_menu_missing_reason(ctx),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let mut items = permission_profile_items(ctx, session_id.clone());
    items.extend(permission_network_items(ctx, session_id.clone()));
    items.push(approval_scopes_refresh_item(ctx, session_id.clone()));
    items.push(approval_scopes_clear_item(ctx));

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_PERMISSIONS),
        title: t!("menu.permissions.title").into_owned(),
        subtitle: Some(t!("menu.permissions.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.permissions.search").into_owned()),
        footer_hint: Some(t!("menu.permissions.footer").into_owned()),
        preview: Some(MenuPreview::KeyValues {
            title: Some(t!("menu.permissions.preview_title").into_owned()),
            rows: permission_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn supports_any_permission_method(ctx: &MenuContext<'_>) -> bool {
    APPUI_PERMISSION_MENU_METHODS_ANY
        .iter()
        .any(|method| ctx.availability.supports_method(method))
}

fn supports_any_method(ctx: &MenuContext<'_>, methods: &[&str]) -> bool {
    methods
        .iter()
        .any(|method| ctx.availability.supports_method(method))
}

fn local_profile_create_supported(ctx: &MenuContext<'_>) -> bool {
    ctx.availability
        .supports_method(APPUI_METHOD_PROFILE_LOCAL_CREATE)
}

fn action_missing_reason(ctx: &MenuContext<'_>, method: &'static str) -> Option<String> {
    (!ctx.availability.supports_method(method)).then(|| method_missing_reason(ctx, method))
}

fn mutating_action_missing_reason(ctx: &MenuContext<'_>, method: &'static str) -> Option<String> {
    if ctx.availability.readonly {
        Some("Read-only launch: mutating Octos UI commands are disabled".into())
    } else {
        action_missing_reason(ctx, method)
    }
}

fn permission_menu_missing_reason(ctx: &MenuContext<'_>) -> String {
    if ctx.availability.capabilities.is_none() {
        "Octos UI capabilities are not available".into()
    } else if let Some((method, reason)) =
        APPUI_PERMISSION_MENU_METHODS_ANY.iter().find_map(|method| {
            ctx.availability
                .unsupported_method_reason(method)
                .map(|reason| (*method, reason))
        })
    {
        format!("Octos UI method `{method}` is unsupported: {reason}")
    } else {
        format!(
            "Octos UI permission methods are not advertised: {}",
            APPUI_PERMISSION_MENU_METHODS_ANY.join(", ")
        )
    }
}

fn permission_profile_items(
    ctx: &MenuContext<'_>,
    session_id: octos_core::SessionKey,
) -> Vec<MenuItem> {
    let approval_never = permission_approval_policy_is_never(ctx);
    let mutation_reason = permission_action_disabled_reason(
        ctx,
        AppUiActionKind::PermissionProfileSet,
        "typed command missing for profile/set",
    );
    let profile_list_reason = permission_action_disabled_reason(
        ctx,
        AppUiActionKind::PermissionProfileList,
        "typed command missing for profile/list",
    );

    let mut items = vec![
        permission_mode_item(
            "permissions.default",
            t!("menu.permissions.item.default.label"),
            t!("menu.permissions.item.default.desc"),
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::WorkspaceWrite),
                    network: Some(PermissionNetworkPolicy::Deny),
                    approval_policy: Some("on-request".into()),
                },
            ),
            permission_default_state(ctx.app.permission_profile, approval_never),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.read_only",
            t!("menu.permissions.item.read_only.label"),
            t!("menu.permissions.item.read_only.desc"),
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::ReadOnly),
                    network: None,
                    approval_policy: Some("on-request".into()),
                },
            ),
            permission_mode_state(
                ctx.app.permission_profile,
                PermissionProfileMode::ReadOnly,
                approval_never,
            ),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.workspace_write",
            t!("menu.permissions.item.workspace_write.label"),
            t!("menu.permissions.item.workspace_write.desc"),
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::WorkspaceWrite),
                    network: None,
                    approval_policy: Some("on-request".into()),
                },
            ),
            permission_workspace_write_state(ctx.app.permission_profile, approval_never),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.workspace_write_never",
            t!("menu.permissions.item.workspace_write_never.label"),
            t!("menu.permissions.item.workspace_write_never.desc"),
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::WorkspaceWrite),
                    network: Some(PermissionNetworkPolicy::Deny),
                    approval_policy: Some("never".into()),
                },
            ),
            permission_workspace_write_never_state(ctx.app.permission_profile, approval_never),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.full_access",
            t!("menu.permissions.item.full_access.label"),
            t!("menu.permissions.item.full_access.desc"),
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::DangerFullAccess),
                    network: Some(PermissionNetworkPolicy::Allow),
                    approval_policy: Some("never".into()),
                },
            ),
            permission_mode_state(
                ctx.app.permission_profile,
                PermissionProfileMode::DangerFullAccess,
                approval_never,
            )
            .destructive(),
            mutation_reason,
        ),
        MenuItem::new(
            "permissions.profile.refresh",
            t!("menu.permissions.item.profile_refresh.label"),
            MenuAction::SendAppUi(AppUiCommand::ListPermissionProfiles(
                PermissionProfileListParams { session_id },
            )),
        )
        .with_description("Requires profile/list.")
        .maybe_disabled(profile_list_reason),
    ];

    for (idx, item) in items.iter_mut().enumerate() {
        if let Some(shortcut) = numeric_shortcut(idx) {
            item.shortcut = Some(shortcut);
        }
    }
    items
}

fn permission_mode_item(
    id: &'static str,
    label: impl Into<String>,
    description: impl Into<String>,
    action: MenuAction,
    state: MenuItemState,
    disabled_reason: Option<String>,
) -> MenuItem {
    MenuItem::new(id, label, action)
        .with_description(description)
        .with_state(state)
        .maybe_disabled(disabled_reason)
}

fn permission_default_state(
    current: Option<PermissionProfileSelection>,
    approval_never: bool,
) -> MenuItemState {
    let default = PermissionProfileSelection {
        mode: PermissionProfileMode::WorkspaceWrite,
        network: PermissionNetworkPolicy::Deny,
    };
    MenuItemState {
        current: !approval_never && current.is_some_and(|current| current.normalized() == default),
        ..MenuItemState::default()
    }
}

fn permission_workspace_write_state(
    current: Option<PermissionProfileSelection>,
    approval_never: bool,
) -> MenuItemState {
    MenuItemState {
        current: !approval_never
            && current.is_some_and(|current| {
                let current = current.normalized();
                current.mode == PermissionProfileMode::WorkspaceWrite
                    && current.network != PermissionNetworkPolicy::Deny
            }),
        ..MenuItemState::default()
    }
}

fn permission_workspace_write_never_state(
    current: Option<PermissionProfileSelection>,
    approval_never: bool,
) -> MenuItemState {
    MenuItemState {
        current: approval_never
            && current.is_some_and(|current| {
                let current = current.normalized();
                current.mode == PermissionProfileMode::WorkspaceWrite
                    && current.network == PermissionNetworkPolicy::Deny
            }),
        ..MenuItemState::default()
    }
}

fn permission_mode_state(
    current: Option<PermissionProfileSelection>,
    mode: PermissionProfileMode,
    approval_never: bool,
) -> MenuItemState {
    MenuItemState {
        current: current.is_some_and(|current| {
            current.normalized().mode == mode
                && (!approval_never || mode == PermissionProfileMode::DangerFullAccess)
        }),
        ..MenuItemState::default()
    }
}

fn permission_approval_policy_is_never(ctx: &MenuContext<'_>) -> bool {
    ctx.app
        .runtime_status
        .and_then(status_approval_policy_value)
        .is_some_and(|approval_policy| approval_policy == "never")
}

fn permission_network_items(
    ctx: &MenuContext<'_>,
    session_id: octos_core::SessionKey,
) -> Vec<MenuItem> {
    let mutation_reason = permission_action_disabled_reason(
        ctx,
        AppUiActionKind::PermissionProfileSet,
        "typed command missing for network permissions",
    );

    vec![
        MenuItem::new(
            "permissions.network.allow",
            t!("menu.permissions.item.network_allow.label"),
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: None,
                    network: Some(PermissionNetworkPolicy::Allow),
                    approval_policy: None,
                },
            ),
        )
        .with_description(t!("menu.permissions.item.network_allow.desc"))
        .with_state(MenuItemState::checked(
            ctx.app.permission_profile.is_some_and(|current| {
                current.normalized().network == PermissionNetworkPolicy::Allow
            }),
        ))
        .maybe_disabled(mutation_reason.clone()),
        MenuItem::new(
            "permissions.network.block",
            t!("menu.permissions.item.network_block.label"),
            permission_set_action(
                session_id,
                PermissionProfileUpdate {
                    mode: None,
                    network: Some(PermissionNetworkPolicy::Deny),
                    approval_policy: None,
                },
            ),
        )
        .with_description(t!("menu.permissions.item.network_block.desc"))
        .with_state(MenuItemState::checked(
            ctx.app.permission_profile.is_some_and(|current| {
                current.normalized().network == PermissionNetworkPolicy::Deny
            }),
        ))
        .maybe_disabled(mutation_reason),
    ]
}

fn permission_set_action(
    session_id: octos_core::SessionKey,
    update: PermissionProfileUpdate,
) -> MenuAction {
    MenuAction::SendAppUi(AppUiCommand::SetPermissionProfile(
        PermissionProfileSetParams {
            session_id,
            update,
            runtime_mode: None,
        },
    ))
}

fn approval_scopes_refresh_item(
    ctx: &MenuContext<'_>,
    session_id: octos_core::SessionKey,
) -> MenuItem {
    let item = MenuItem::new(
        "permissions.scopes.refresh",
        t!("menu.permissions.item.scopes_refresh.label"),
        MenuAction::SendAppUi(AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
            session_id,
        })),
    )
    .with_description("Uses approval/scopes/list.");

    if ctx
        .availability
        .supports_method(methods::APPROVAL_SCOPES_LIST)
    {
        item
    } else {
        item.disabled(permission_method_missing_reason(
            ctx,
            methods::APPROVAL_SCOPES_LIST,
        ))
    }
}

fn approval_scopes_clear_item(ctx: &MenuContext<'_>) -> MenuItem {
    MenuItem::new(
        "permissions.scopes.clear",
        t!("menu.permissions.item.scopes_clear.label"),
        MenuAction::Noop,
    )
    .with_description("Requires scopes/clear.")
    .maybe_disabled(permission_action_disabled_reason(
        ctx,
        AppUiActionKind::ApprovalScopesClear,
        "typed command missing for scopes/clear",
    ))
}

fn permission_action_disabled_reason(
    ctx: &MenuContext<'_>,
    action: AppUiActionKind,
    typed_gap: &'static str,
) -> Option<String> {
    let method = action.method();
    if let Some(reason) = ctx.availability.unsupported_method_reason(method) {
        Some(format!("unsupported `{method}`: {reason}"))
    } else if !ctx.availability.supports_method(method) {
        Some(permission_method_missing_reason(ctx, method))
    } else if matches!(
        action,
        AppUiActionKind::PermissionProfileList | AppUiActionKind::PermissionProfileSet
    ) {
        None
    } else {
        Some(typed_gap.into())
    }
}

fn permission_method_missing_reason(ctx: &MenuContext<'_>, method: &str) -> String {
    if ctx.availability.capabilities.is_none() {
        "capabilities unavailable".into()
    } else if method == AppUiActionKind::PermissionProfileSet.method() {
        "missing profile/set".into()
    } else if method == AppUiActionKind::PermissionProfileList.method() {
        "missing profile/list".into()
    } else if method == AppUiActionKind::ApprovalScopesClear.method() {
        "missing scopes/clear".into()
    } else {
        format!("missing `{method}`")
    }
}

fn permission_preview_rows(ctx: &MenuContext<'_>) -> Vec<MenuPreviewRow> {
    let mut rows = app_snapshot_rows(ctx.app.clone());
    if let Some(status) = ctx.app.runtime_status {
        rows.extend(permission_server_policy_rows(status));
    }
    if let Some(current) = ctx.app.permission_profile {
        rows.push(MenuPreviewRow {
            label: "permissions".into(),
            value: current.summary(),
        });
    }
    rows.extend([
        permission_method_row(ctx, APPUI_METHOD_PERMISSION_PROFILE_LIST),
        permission_method_row(ctx, APPUI_METHOD_PERMISSION_PROFILE_SET),
        permission_method_row(ctx, methods::APPROVAL_SCOPES_LIST),
        permission_method_row(ctx, APPUI_METHOD_APPROVAL_SCOPES_CLEAR),
    ]);
    rows
}

fn permission_server_policy_rows(
    status: &crate::model::SessionRuntimeStatus,
) -> Vec<MenuPreviewRow> {
    [
        ("runtime_mode", status_runtime_mode_value(status)),
        ("approval_policy", status_approval_policy_value(status)),
        ("sandbox_mode", status_sandbox_value(status)),
        ("filesystem_scope", status_filesystem_scope_value(status)),
        ("network", status_network_value(status)),
        ("dangerous", status_dangerous_access_value(status)),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        value.map(|value| MenuPreviewRow {
            label: label.into(),
            value,
        })
    })
    .collect()
}

fn permission_method_row(ctx: &MenuContext<'_>, method: &'static str) -> MenuPreviewRow {
    let value = if let Some(reason) = ctx.availability.unsupported_method_reason(method) {
        format!("unsupported: {reason}")
    } else if ctx.availability.supports_method(method) {
        "advertised".into()
    } else if ctx.availability.capabilities.is_none() {
        "capabilities unavailable".into()
    } else {
        "missing".into()
    };

    MenuPreviewRow {
        label: method.into(),
        value,
    }
}

fn method_missing_reason(ctx: &MenuContext<'_>, method: &str) -> String {
    if let Some(reason) = ctx.availability.unsupported_method_reason(method) {
        format!("Octos UI method `{method}` is unsupported: {reason}")
    } else if ctx.availability.capabilities.is_none() {
        "Octos UI capabilities are not available".into()
    } else {
        format!("Octos UI method `{method}` is not advertised by this backend.")
    }
}

fn installed_skill_description(skill: &crate::model::ProfileSkillEntry) -> String {
    let version = skill.version.as_deref().unwrap_or("unversioned");
    let source = skill.source_repo.as_deref().unwrap_or("local");
    format!(
        "{version}; {tool_count} tool(s); source {source}",
        tool_count = skill.tool_count
    )
}

fn registry_package_description(package: &crate::model::ProfileSkillRegistryPackage) -> String {
    let version = package.version.as_deref().unwrap_or("unversioned");
    let installed = if package.installed {
        "installed"
    } else {
        "available"
    };
    let skills = if package.skills.is_empty() {
        package.name.clone()
    } else {
        package.skills.join(", ")
    };
    format!(
        "{installed}; {version}; skills: {skills}; repo {}",
        package.repo
    )
}

fn model_label(model: &ModelStatus) -> String {
    model.title.clone().unwrap_or_else(|| model.model.clone())
}

fn model_description(model: &ModelStatus) -> String {
    let mut parts = vec![format!("{} / {}", model.provider, model.model)];
    if let Some(family) = &model.family {
        parts.push(format!("family {family}"));
    }
    if let Some(route) = &model.route {
        parts.push(format!("route {route}"));
    }
    if let Some(qoe) = &model.qoe_policy {
        parts.push(format!("QoE {qoe}"));
    }
    if let Some(queue) = &model.queue_mode {
        parts.push(format!("queue {queue}"));
    }
    parts.join(" | ")
}

fn model_preview_rows(ctx: &MenuContext<'_>) -> Vec<MenuPreviewRow> {
    let mut rows = app_snapshot_rows(ctx.app.clone());
    rows.push(permission_method_row(ctx, APPUI_METHOD_MODEL_LIST));
    rows.push(permission_method_row(ctx, APPUI_METHOD_MODEL_SELECT));
    if let Some(catalog) = ctx.app.model_catalog {
        rows.push(MenuPreviewRow {
            label: "models".into(),
            value: catalog.models.len().to_string(),
        });
        if let Some(selected) = catalog.models.iter().find(|model| model.selected) {
            rows.push(MenuPreviewRow {
                label: "selected".into(),
                value: format!("{} / {}", selected.provider, selected.model),
            });
        }
    }
    rows
}

fn mcp_server_description(server: &McpStatus) -> String {
    let mut parts = vec![server.status.clone()];
    if let Some(transport) = &server.transport {
        parts.push(format!("transport {transport}"));
    }
    if let Some(endpoint) = &server.endpoint {
        parts.push(endpoint.clone());
    }
    if let Some(tool_count) = server.tool_count {
        parts.push(format!("{tool_count} tools"));
    }
    if let Some(detail) = &server.detail {
        parts.push(detail.clone());
    }
    if let Some(last_error) = &server.last_error {
        parts.push(format!("error: {last_error}"));
    }
    parts.join(" | ")
}

fn mcp_config_server_name(server: &McpConfigEntry) -> String {
    server.name.trim().to_owned()
}

fn mcp_config_label(server: &McpConfigEntry) -> String {
    let name = mcp_config_server_name(server);
    let state = if server.enabled {
        "enabled"
    } else {
        "disabled"
    };
    format!("{name} ({state})")
}

fn mcp_config_description(server: &McpConfigEntry) -> String {
    let mut parts = Vec::new();
    if let Some(status) = &server.status {
        parts.push(status.clone());
    }
    if let Some(transport) = &server.transport {
        parts.push(format!("transport {transport}"));
    }
    if let Some(endpoint) = &server.endpoint {
        parts.push(endpoint.clone());
    }
    if let Some(command) = &server.command {
        let args = if server.args.is_empty() {
            String::new()
        } else {
            format!(" {}", server.args.join(" "))
        };
        parts.push(format!("{command}{args}"));
    }
    if !server.env_keys.is_empty() {
        parts.push(format!("env {}", server.env_keys.join(", ")));
    }
    if let Some(tool_count) = server.tool_count {
        parts.push(format!("{tool_count} tools"));
    }
    if let Some(detail) = &server.detail {
        parts.push(detail.clone());
    }
    if let Some(last_error) = &server.last_error {
        parts.push(format!("error: {last_error}"));
    }
    if parts.is_empty() {
        "Configured by Octos UI.".into()
    } else {
        parts.join(" | ")
    }
}

fn tool_config_name(tool: &ToolConfigEntry) -> String {
    tool.tool.trim().to_owned()
}

fn tool_config_label(tool: &ToolConfigEntry) -> String {
    let name = tool
        .title
        .as_ref()
        .filter(|title| !title.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| tool_config_name(tool));
    let state = if tool.enabled { "enabled" } else { "disabled" };
    format!("{name} ({state})")
}

fn tool_config_description(tool: &ToolConfigEntry) -> String {
    let mut parts = Vec::new();
    if let Some(source) = &tool.source {
        parts.push(format!("source {source}"));
    }
    if tool.visible {
        parts.push("visible".into());
    }
    if let Some(risk) = &tool.risk {
        parts.push(format!("risk {risk}"));
    }
    if let Some(status) = &tool.status {
        parts.push(status.clone());
    }
    if let Some(detail) = &tool.detail {
        parts.push(detail.clone());
    }
    if !tool.tags.is_empty() {
        parts.push(format!("tags {}", tool.tags.join(", ")));
    }
    if parts.is_empty() {
        "Configured by Octos UI.".into()
    } else {
        parts.join(" | ")
    }
}

fn tool_status_description(tool: &ToolStatus) -> String {
    let mut parts = Vec::new();
    if let Some(source) = &tool.source {
        parts.push(format!("source {source}"));
    }
    if tool.visible {
        parts.push("visible".into());
    }
    if let Some(risk) = &tool.risk {
        parts.push(format!("risk {risk}"));
    }
    if let Some(policy_id) = &tool.policy_id {
        parts.push(format!("policy {policy_id}"));
    }
    if let Some(denial) = &tool.denial {
        parts.push(format!("denied: {}", denial.reason));
    }
    if !tool.tags.is_empty() {
        parts.push(format!("tags {}", tool.tags.join(", ")));
    }
    if parts.is_empty() {
        "Server-returned tool status.".into()
    } else {
        parts.join(" | ")
    }
}

fn coding_contract_is_ready(contract: &crate::model::CodingToolContract) -> bool {
    contract.status == "ready" && contract.missing_required_tools.is_empty()
}

fn coding_contract_description(contract: &crate::model::CodingToolContract) -> String {
    let mut parts = Vec::new();
    if !contract.id.is_empty() {
        parts.push(match contract.version.as_str() {
            "" => contract.id.clone(),
            version => format!("{} v{version}", contract.id),
        });
    }
    if !contract.feature.is_empty() {
        parts.push(contract.feature.clone());
    }
    if !contract.status.is_empty() {
        parts.push(format!("status {}", contract.status));
    }
    if let Some(policy) = &contract.policy {
        if let Some(policy_id) = &policy.tool_policy_id {
            parts.push(format!("policy {policy_id}"));
        }
        if let Some(sandbox) = &policy.sandbox_mode {
            parts.push(format!("sandbox {sandbox}"));
        }
        if let Some(approval) = &policy.approval_policy {
            parts.push(format!("approval {approval}"));
        }
    }
    if !contract.missing_required_tools.is_empty() {
        parts.push(format!(
            "missing {}",
            contract.missing_required_tools.join(", ")
        ));
    }
    if parts.is_empty() {
        "Server-returned coding tool contract.".into()
    } else {
        parts.join(" | ")
    }
}

fn coding_contract_missing_tool_description(
    contract: &crate::model::CodingToolContract,
    tool_name: &str,
) -> String {
    let Some(tool) = contract
        .required_tools
        .iter()
        .find(|tool| tool.name == tool_name)
    else {
        return "Backend marked this required P0 tool missing.".into();
    };

    let mut parts = Vec::new();
    if !tool.status.is_empty() {
        parts.push(format!("status {}", tool.status));
    }
    if !tool.capability.is_empty() {
        parts.push(format!("capability {}", tool.capability));
    }
    if !tool.policy.is_empty() {
        parts.push(format!("policy {}", tool.policy));
    }
    if let Some(backend_tool) = &tool.backend_tool {
        parts.push(format!("backend {backend_tool}"));
    }
    if let Some(detail) = &tool.detail {
        parts.push(detail.clone());
    }
    if parts.is_empty() {
        "Backend marked this required P0 tool missing.".into()
    } else {
        parts.join(" | ")
    }
}

fn mcp_preview_rows(ctx: &MenuContext<'_>) -> Vec<MenuPreviewRow> {
    let mut rows = app_snapshot_rows(ctx.app.clone());
    rows.push(permission_method_row(ctx, APPUI_METHOD_MCP_CONFIG_LIST));
    rows.push(permission_method_row(
        ctx,
        AppUiActionKind::McpStatusList.method(),
    ));
    if let Some(config) = ctx.app.mcp_config_catalog {
        let enabled = config
            .servers
            .iter()
            .filter(|server| server.enabled)
            .count();
        rows.push(MenuPreviewRow {
            label: "configured".into(),
            value: config.servers.len().to_string(),
        });
        rows.push(MenuPreviewRow {
            label: "enabled".into(),
            value: enabled.to_string(),
        });
    }
    if let Some(catalog) = ctx.app.mcp_catalog {
        let connected = catalog
            .servers
            .iter()
            .filter(|server| server.status == "connected")
            .count();
        let failed = catalog
            .servers
            .iter()
            .filter(|server| server.status == "failed")
            .count();
        rows.push(MenuPreviewRow {
            label: "servers".into(),
            value: catalog.servers.len().to_string(),
        });
        rows.push(MenuPreviewRow {
            label: "connected".into(),
            value: connected.to_string(),
        });
        rows.push(MenuPreviewRow {
            label: "failed".into(),
            value: failed.to_string(),
        });
    }
    rows
}

fn tool_settings_preview_rows(ctx: &MenuContext<'_>) -> Vec<MenuPreviewRow> {
    let mut rows = app_snapshot_rows(ctx.app.clone());
    rows.push(permission_method_row(ctx, APPUI_METHOD_TOOL_CONFIG_LIST));
    rows.push(permission_method_row(ctx, APPUI_METHOD_TOOL_STATUS_LIST));
    if let Some(config) = ctx.app.tool_config_catalog {
        let enabled = config.tools.iter().filter(|tool| tool.enabled).count();
        rows.push(MenuPreviewRow {
            label: "configured".into(),
            value: config.tools.len().to_string(),
        });
        rows.push(MenuPreviewRow {
            label: "enabled".into(),
            value: enabled.to_string(),
        });
    }
    if let Some(catalog) = ctx.app.tool_catalog {
        rows.push(MenuPreviewRow {
            label: "policy".into(),
            value: catalog
                .policy_id
                .clone()
                .unwrap_or_else(|| "server policy".into()),
        });
        if let Some(contract) = &catalog.coding_tool_contract {
            rows.push(MenuPreviewRow {
                label: "coding contract".into(),
                value: if contract.status.is_empty() {
                    contract.id.clone()
                } else if contract.id.is_empty() {
                    contract.status.clone()
                } else {
                    format!("{} ({})", contract.id, contract.status)
                },
            });
            if let Some(policy) = &contract.policy {
                let mut policy_parts = Vec::new();
                if let Some(policy_id) = &policy.tool_policy_id {
                    policy_parts.push(policy_id.clone());
                }
                if let Some(sandbox) = &policy.sandbox_mode {
                    policy_parts.push(format!("sandbox {sandbox}"));
                }
                if let Some(approval) = &policy.approval_policy {
                    policy_parts.push(format!("approval {approval}"));
                }
                if !policy_parts.is_empty() {
                    rows.push(MenuPreviewRow {
                        label: "contract policy".into(),
                        value: policy_parts.join(", "),
                    });
                }
            }
            rows.push(MenuPreviewRow {
                label: "missing P0".into(),
                value: if contract.missing_required_tools.is_empty() {
                    "none".into()
                } else {
                    contract.missing_required_tools.join(", ")
                },
            });
        }
        rows.push(MenuPreviewRow {
            label: "status tools".into(),
            value: catalog.tools.len().to_string(),
        });
    }
    rows
}

fn capability_summary_item(ctx: &MenuContext<'_>) -> MenuItem {
    let description = match ctx.availability.capabilities {
        Some(capabilities) => format!(
            "{} method(s), {} feature(s), {} unsupported report(s)",
            capabilities.methods().len(),
            capabilities.features().len(),
            capabilities.unsupported_methods().len()
        ),
        None => "No Octos UI capabilities have been advertised yet".into(),
    };
    MenuItem::new(
        "status.capabilities",
        t!("menu.status.item.capabilities.label"),
        MenuAction::Noop,
    )
    .with_description(description)
}

fn status_runtime_items(ctx: &MenuContext<'_>) -> Vec<MenuItem> {
    let Some(status) = ctx.app.runtime_status else {
        if ctx
            .availability
            .supports_method(AppUiActionKind::SessionStatusRead.method())
        {
            return vec![
                MenuItem::new(
                    "status.server",
                    t!("menu.status.item.server.label"),
                    MenuAction::Noop,
                )
                .disabled("session/status/read is advertised but no result is cached yet"),
            ];
        }
        return Vec::new();
    };

    let mut items = Vec::new();
    if let Some(health) = status_health_value(status) {
        items.push(
            MenuItem::new(
                "status.health",
                t!("menu.statusline.item.health"),
                MenuAction::Noop,
            )
            .with_description(health),
        );
    }
    if let Some(usage) = status_usage_value(status) {
        items.push(
            MenuItem::new(
                "status.usage",
                t!("menu.statusline.item.usage"),
                MenuAction::Noop,
            )
            .with_description(usage),
        );
    }
    items.extend(runtime_policy_items(status));
    items
}

fn runtime_policy_items(status: &crate::model::SessionRuntimeStatus) -> Vec<MenuItem> {
    let rows: &[(&'static str, &str, Option<String>)] = &[
        (
            "status.runtime_mode",
            "menu.statusline.item.runtime_mode",
            status_runtime_mode_value(status),
        ),
        (
            "status.profile",
            "menu.statusline.item.profile",
            status_profile_value(status),
        ),
        (
            "status.model",
            "menu.statusline.item.model",
            status_model_value(status),
        ),
        (
            "status.provider",
            "menu.statusline.item.provider",
            status_provider_value(status),
        ),
        (
            "status.approval_policy",
            "menu.statusline.item.approval_policy",
            status_approval_policy_value(status),
        ),
        (
            "status.sandbox",
            "menu.statusline.item.sandbox_mode",
            status_sandbox_value(status),
        ),
        (
            "status.filesystem_scope",
            "menu.statusline.item.filesystem_scope",
            status_filesystem_scope_value(status),
        ),
        (
            "status.network",
            "menu.statusline.item.network",
            status_network_value(status),
        ),
        (
            "status.permission_profile",
            "menu.statusline.item.permissions",
            status_permission_value(status),
        ),
        (
            "status.dangerous",
            "menu.statusline.item.dangerous_access",
            status_dangerous_access_value(status),
        ),
        (
            "status.tool_policy",
            "menu.statusline.item.tool_policy",
            status_tool_policy_value(status),
        ),
        (
            "status.tool_contract",
            "menu.statusline.item.tool_contract",
            status_tool_contract_value(status),
        ),
        (
            "status.model_toolset",
            "menu.statusline.item.model_toolset",
            status_model_toolset_value(status),
        ),
        (
            "status.tool_discovery",
            "menu.statusline.item.tool_discovery",
            status_tool_discovery_value(status),
        ),
        (
            "status.mcp",
            "menu.statusline.item.mcp",
            status_mcp_value(status),
        ),
        (
            "status.memory",
            "menu.statusline.item.memory",
            status_memory_value(status),
        ),
        (
            "status.qoe",
            "menu.statusline.item.qoe",
            status_qoe_value(status),
        ),
        (
            "status.queue",
            "menu.statusline.item.queue",
            status_queue_value(status),
        ),
    ];
    rows.iter()
        .filter_map(|(id, key, value)| {
            value
                .as_ref()
                .map(|v| MenuItem::new(*id, t!(*key), MenuAction::Noop).with_description(v))
        })
        .collect()
}

fn status_preview_rows(ctx: &MenuContext<'_>) -> Vec<MenuPreviewRow> {
    let mut rows = app_snapshot_rows(ctx.app.clone());
    if let Some(status) = ctx.app.runtime_status {
        if let Some(health) = status_health_value(status) {
            rows.push(MenuPreviewRow {
                label: "health".into(),
                value: health,
            });
        }
        if let Some(usage) = status_usage_value(status) {
            rows.push(MenuPreviewRow {
                label: "usage".into(),
                value: usage,
            });
        }
        if let Some(cursor) = status_cursor_value(status) {
            rows.push(MenuPreviewRow {
                label: "cursor".into(),
                value: cursor,
            });
        }
        rows.extend(runtime_policy_rows(status));
    }
    rows
}

fn runtime_policy_rows(status: &crate::model::SessionRuntimeStatus) -> Vec<MenuPreviewRow> {
    [
        ("runtime_mode", status_runtime_mode_value(status)),
        ("profile", status_profile_value(status)),
        ("model", status_model_value(status)),
        ("provider", status_provider_value(status)),
        ("approval_policy", status_approval_policy_value(status)),
        ("sandbox_mode", status_sandbox_value(status)),
        ("filesystem_scope", status_filesystem_scope_value(status)),
        ("network", status_network_value(status)),
        ("permissions", status_permission_value(status)),
        ("dangerous", status_dangerous_access_value(status)),
        ("tool_policy", status_tool_policy_value(status)),
        ("tool_contract", status_tool_contract_value(status)),
        ("model_toolset", status_model_toolset_value(status)),
        ("tool_discovery", status_tool_discovery_value(status)),
        ("mcp", status_mcp_value(status)),
        ("memory", status_memory_value(status)),
        ("qoe", status_qoe_value(status)),
        ("queue", status_queue_value(status)),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        value.map(|value| MenuPreviewRow {
            label: label.into(),
            value,
        })
    })
    .collect()
}

fn status_runtime_mode_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.runtime_mode.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.runtime_mode.clone())
    })
}

fn status_profile_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.profile_id.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.profile_id.clone())
    })
}

fn status_model_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .model
        .as_ref()
        .map(|model| model.model.clone())
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.model.clone())
        })
}

fn status_provider_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .model
        .as_ref()
        .map(|model| model.provider.clone())
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.provider.clone())
        })
}

fn status_approval_policy_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.approval_policy.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.approval_policy.clone())
    })
}

fn status_sandbox_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .sandbox_mode
        .clone()
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.sandbox_mode.clone())
        })
        .or_else(|| status.sandbox.clone())
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.sandbox.clone())
        })
}

fn status_filesystem_scope_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.filesystem_scope.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.filesystem_scope.clone())
    })
}

fn status_network_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.network.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.network.clone())
    })
}

fn status_permission_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.permission_profile.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.permission_profile.clone())
    })
}

fn status_dangerous_access_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    let sandbox = status_sandbox_value(status)?;
    let filesystem_scope = status_filesystem_scope_value(status)?;
    (sandbox == "danger-full-access" && filesystem_scope == "host")
        .then(|| "server-confirmed danger-full-access host scope".into())
}

fn status_tool_policy_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .tool_policy_id
        .clone()
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.tool_policy_id.clone())
        })
        .or_else(|| {
            status
                .tool_summary
                .as_ref()
                .and_then(|summary| summary.policy_id.clone())
        })
}

fn status_tool_contract_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    let stamp = status.runtime_policy_stamp.as_ref()?;
    let id = stamp.tool_contract_id.as_ref()?;
    Some(match stamp.tool_contract_version.as_deref() {
        Some(version) if !version.is_empty() => format!("{id} v{version}"),
        _ => id.clone(),
    })
}

fn status_model_toolset_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .runtime_policy_stamp
        .as_ref()
        .and_then(|stamp| stamp.model_toolset.clone())
}

fn status_tool_discovery_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .runtime_policy_stamp
        .as_ref()
        .and_then(|stamp| stamp.dynamic_tool_discovery.clone())
}

fn status_mcp_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    if !status.mcp_servers.is_empty() {
        return Some(status.mcp_servers.join(", "));
    }

    if let Some(stamp) = status.runtime_policy_stamp.as_ref() {
        if !stamp.mcp_servers.is_empty() {
            return Some(
                stamp
                    .mcp_servers
                    .iter()
                    .map(RuntimePolicyMcpServer::label)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }

    status.mcp_summary.as_ref().map(|summary| {
        format!(
            "{} connected, {} connecting, {} failed, {} disabled",
            summary.connected, summary.connecting, summary.failed, summary.disabled
        )
    })
}

fn status_memory_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status.memory_scope.clone().or_else(|| {
        status
            .runtime_policy_stamp
            .as_ref()
            .and_then(|stamp| stamp.memory_scope.clone())
    })
}

fn status_qoe_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .model
        .as_ref()
        .and_then(|model| model.qoe_policy.clone())
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.qoe_policy.clone())
        })
}

fn status_queue_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .model
        .as_ref()
        .and_then(|model| model.queue_mode.clone())
        .or_else(|| {
            status
                .runtime_policy_stamp
                .as_ref()
                .and_then(|stamp| stamp.queue_mode.clone())
        })
}

fn status_health_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    status
        .health
        .as_ref()
        .map(|health| match health.message.as_deref() {
            Some(message) if !message.is_empty() => format!("{} ({message})", health.status),
            _ => health.status.clone(),
        })
}

fn status_usage_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    let usage = status.usage.as_ref()?;
    let mut parts = Vec::new();
    if let Some(tokens) = usage.input_tokens {
        parts.push(format!("in {tokens}"));
    }
    if let Some(tokens) = usage.output_tokens {
        parts.push(format!("out {tokens}"));
    }
    if let Some(tokens) = usage.cached_input_tokens {
        parts.push(format!("cached-in {tokens}"));
    }
    if let Some(tokens) = usage.cached_output_tokens {
        parts.push(format!("cached-out {tokens}"));
    }
    if let Some(cost) = usage.estimated_cost_micros_usd {
        parts.push(format!("cost ${:.4}", cost as f64 / 1_000_000.0));
    }
    (!parts.is_empty()).then(|| parts.join(" | "))
}

fn usage_item(id: &'static str, label: String, value: Option<u64>) -> MenuItem {
    MenuItem::new(id, label, MenuAction::Noop)
        .with_description(value.map_or_else(|| "not reported".into(), |value| value.to_string()))
        .maybe_disabled(value.is_none().then_some("not reported".into()))
}

fn cost_item(value: Option<u64>) -> MenuItem {
    let description = value
        .map(format_micros_usd)
        .unwrap_or_else(|| "not reported".into());
    MenuItem::new(
        "cost.estimated",
        t!("menu.cost.item.estimated.label"),
        MenuAction::Noop,
    )
    .with_description(description)
    .maybe_disabled(value.is_none().then_some("not reported".into()))
}

fn format_micros_usd(value: u64) -> String {
    format!("${:.4}", value as f64 / 1_000_000.0)
}

fn status_cursor_value(status: &crate::model::SessionRuntimeStatus) -> Option<String> {
    let cursor = status.cursor.as_ref()?;
    let mut parts = Vec::new();
    if let Some(cursor) = cursor.cursor.as_ref() {
        parts.push(format!("{}#{}", cursor.stream, cursor.seq));
    }
    parts.push(if cursor.healthy {
        "healthy".into()
    } else {
        "degraded".into()
    });
    if cursor.replay_supported {
        parts.push("replay".into());
    }
    if let Some(detail) = cursor.detail.as_deref().filter(|detail| !detail.is_empty()) {
        parts.push(detail.into());
    }
    Some(parts.join(" | "))
}

fn action_for_command_entry(entry: &CommandEntry) -> MenuAction {
    match entry {
        CommandEntry::OpenMenu(id) => MenuAction::OpenMenu(id.clone()),
        CommandEntry::LocalAction(action) => MenuAction::Local(action.clone()),
        CommandEntry::AppUiAction(_) => MenuAction::Noop,
        CommandEntry::PromptTemplate(template) => MenuAction::SubmitPrompt((*template).into()),
    }
}

fn command_description(description: &str, aliases: &[&str]) -> String {
    let desc = t!(description).into_owned();
    if aliases.is_empty() {
        desc
    } else {
        format!(
            "{desc} {} {}",
            t!("command.aliases_label"),
            aliases.join(", ")
        )
    }
}

fn status_line_items(app: MenuAppSnapshot<'_>) -> [(&'static str, String, bool); 9] {
    [
        (
            "state",
            t!(
                "menu.statusline.item.state_label",
                value = app.status.unwrap_or("idle")
            )
            .into_owned(),
            true,
        ),
        (
            "target",
            t!(
                "menu.statusline.item.target_label",
                value = app.target.unwrap_or("local")
            )
            .into_owned(),
            true,
        ),
        (
            "cwd",
            t!(
                "menu.statusline.item.cwd_label",
                value = app.cwd.unwrap_or("unknown")
            )
            .into_owned(),
            true,
        ),
        (
            "model",
            t!(
                "menu.statusline.item.model_label",
                value = app.current_model.unwrap_or("not supplied")
            )
            .into_owned(),
            true,
        ),
        (
            "profile",
            t!(
                "menu.statusline.item.profile_label",
                value = app.current_profile.unwrap_or("default")
            )
            .into_owned(),
            true,
        ),
        (
            "session",
            t!(
                "menu.statusline.item.session_label",
                value = app.selected_session_title.unwrap_or("none")
            )
            .into_owned(),
            true,
        ),
        (
            "task",
            t!(
                "menu.statusline.item.task_label",
                value = app.selected_task_title.unwrap_or("none")
            )
            .into_owned(),
            false,
        ),
        (
            "background_tasks",
            t!(
                "menu.statusline.item.background_label",
                value = app.background_task_count
            )
            .into_owned(),
            true,
        ),
        (
            "approval",
            t!("menu.statusline.item.approval_label").into_owned(),
            true,
        ),
    ]
}

fn title_items(app: MenuAppSnapshot<'_>) -> [(&'static str, String, bool); 7] {
    [
        ("app", "octos-tui".into(), true),
        (
            "session",
            app.selected_session_title.unwrap_or("no session").into(),
            true,
        ),
        ("state", app.status.unwrap_or("idle").into(), true),
        ("cwd", app.cwd.unwrap_or("unknown").into(), false),
        ("model", app.current_model.unwrap_or("model").into(), false),
        (
            "background_tasks",
            format!("{} tasks", app.background_task_count),
            true,
        ),
        (
            "profile",
            app.current_profile.unwrap_or("default").into(),
            false,
        ),
    ]
}

fn app_snapshot_rows(app: MenuAppSnapshot<'_>) -> Vec<MenuPreviewRow> {
    [
        ("status", app.status),
        ("target", app.target),
        ("cwd", app.cwd),
        ("profile", app.current_profile),
        (
            "session_id",
            app.selected_session_id
                .map(|session_id| session_id.0.as_str()),
        ),
        ("session", app.selected_session_title),
        ("task", app.selected_task_title),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        value.map(|value| MenuPreviewRow {
            label: label.into(),
            value: value.into(),
        })
    })
    .collect()
}

fn keymap_tabs() -> Vec<MenuTab> {
    [
        ("global", t!("menu.keymap.tab.global")),
        ("composer", t!("menu.keymap.tab.composer")),
        ("menu", t!("menu.keymap.tab.menu")),
        ("task", t!("menu.keymap.tab.task")),
        ("approval", t!("menu.keymap.tab.approval")),
    ]
    .into_iter()
    .enumerate()
    .map(|(idx, (id, label))| MenuTab {
        id: id.to_owned(),
        label: label.into_owned(),
        active: idx == 0,
        count: None,
    })
    .collect()
}

fn numeric_shortcut(index: usize) -> Option<KeyBinding> {
    let digit = char::from_digit((index + 1) as u32, 10)?;
    Some(KeyBinding::new(KeyCode::Char(digit), KeyModifiers::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{AvailabilityContext, CapabilitySet, TerminalSize};
    use crate::model::{
        CodingToolContract, CodingToolContractPolicy, CodingToolContractTool, McpConfigEntry,
        McpConfigListResult, McpStatus, McpStatusSummary, ModelStatus, RuntimeHealthStatus,
        RuntimePolicyStamp, SessionCursorStatus, SessionMcpCatalog, SessionModelCatalog,
        SessionRuntimeStatus, SessionToolCatalog, SessionUsageStatus, ToolConfigEntry,
        ToolConfigListResult, ToolStatus,
    };
    use octos_core::SessionKey;
    use octos_core::ui_protocol::{TurnId, UiCursor};

    fn runtime_status(session_id: &SessionKey) -> SessionRuntimeStatus {
        SessionRuntimeStatus {
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
        }
    }

    fn rendered_menu_text(spec: &MenuSpec) -> String {
        let mut text = format!("{} {}", spec.title, spec.subtitle.as_deref().unwrap_or(""));
        for item in &spec.items {
            text.push(' ');
            text.push_str(&item.label);
            if let Some(description) = &item.description {
                text.push(' ');
                text.push_str(description);
            }
            if let Some(reason) = &item.disabled_reason {
                text.push(' ');
                text.push_str(reason);
            }
        }
        if let Some(preview) = &spec.preview {
            match preview {
                MenuPreview::Text { title, body } => {
                    text.push_str(title.as_deref().unwrap_or(""));
                    text.push_str(body);
                }
                MenuPreview::KeyValues { title, rows } => {
                    text.push_str(title.as_deref().unwrap_or(""));
                    for row in rows {
                        text.push_str(&row.label);
                        text.push_str(&row.value);
                    }
                }
            }
        }
        text
    }

    #[test]
    fn core_provider_registry_builds_local_menus() {
        let registry = core_menu_registry();
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot::default(),
            terminal: TerminalSize::default(),
            theme_name: Some("terminal"),
            selected_path: &[],
        };

        for id in [
            MENU_HELP,
            MENU_THEME,
            MENU_STATUS_LINE,
            MENU_TITLE,
            MENU_KEYMAP,
        ] {
            let result = registry.build(&MenuId::from(id), &ctx);
            let MenuBuildResult::Ready(spec) = result else {
                panic!("expected ready menu {id}");
            };
            assert_eq!(spec.id, MenuId::from(id));
            assert!(!spec.title.is_empty());
        }
    }

    #[test]
    fn slash_command_menu_has_no_routing_preview_pane() {
        // Regression: the slash-command menu must NOT carry the static "Routing"
        // preview — it was non-actionable internal info, and the two-pane split
        // collided its text with the command column. Full-width list instead.
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot::default(),
            terminal: TerminalSize::default(),
            theme_name: Some("terminal"),
            selected_path: &[],
        };
        let spec = help_menu(&ctx);
        assert!(
            spec.preview.is_none(),
            "slash-command menu should render full-width (no Routing preview pane)"
        );
    }

    #[test]
    fn onboarding_menu_uses_dashboard_catalog_for_provider_choices() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_AUTH_STATUS,
            APPUI_METHOD_PROFILE_LLM_CATALOG,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        let onboarding = OnboardingWizardState::default();
        let mut families = serde_json::Map::new();
        families.insert(
            "moonshot".into(),
            serde_json::json!({
                "env": "MOONSHOT_API_KEY",
                "models": [{
                    "id": "kimi-k2.5",
                    "endpoints": [
                        { "id": "moonshot", "label": "Official API" },
                        {
                            "id": "autodl",
                            "label": "AutoDL",
                            "base_url": "https://www.autodl.art/api/v1",
                            "api_key_env": "AUTODL_API_KEY"
                        }
                    ]
                }]
            }),
        );
        let catalog = crate::model::ProfileLlmCatalogResult { families };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                profile_llm_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_ONBOARD), &ctx) else {
            panic!("expected onboarding menu");
        };
        // Assert via the i18n key (NOT a hardcoded English literal) so the test
        // tracks the source string and stays correct across locales.
        assert_eq!(spec.title, t!("onboarding.wizard.setup_title"));
        // The provider/setup phase now carries the wizard progress checklist as
        // its right-side preview pane.
        assert!(
            matches!(
                spec.preview,
                Some(crate::menu::types::MenuPreview::KeyValues { .. })
            ),
            "provider setup menu should show the wizard progress checklist"
        );
        assert!(
            spec.items
                .iter()
                .any(|item| item.id == "onboard.provider.key")
        );

        let MenuBuildResult::Ready(family_spec) = registry.build(
            &MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY),
            &ctx,
        ) else {
            panic!("expected family menu");
        };
        assert!(
            family_spec
                .items
                .iter()
                .any(|item| item.label == "moonshot")
        );

        let onboarding = OnboardingWizardState {
            provider: LlmSelectionConfig {
                family_id: "moonshot".into(),
                ..LlmSelectionConfig::default()
            },
            ..OnboardingWizardState::default()
        };
        let model_ctx = MenuContext {
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                profile_llm_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            ..ctx
        };
        let MenuBuildResult::Ready(model_spec) = registry.build(
            &MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL),
            &model_ctx,
        ) else {
            panic!("expected model menu");
        };
        assert!(
            model_spec
                .items
                .iter()
                .any(|item| item.label == "kimi-k2.5")
        );

        let onboarding = OnboardingWizardState {
            provider: LlmSelectionConfig {
                family_id: "moonshot".into(),
                model_id: "kimi-k2.5".into(),
                ..LlmSelectionConfig::default()
            },
            ..OnboardingWizardState::default()
        };
        let route_ctx = MenuContext {
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                profile_llm_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            ..model_ctx
        };
        let MenuBuildResult::Ready(route_spec) = registry.build(
            &MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE),
            &route_ctx,
        ) else {
            panic!("expected route menu");
        };
        let autodl = route_spec
            .items
            .iter()
            .find(|item| item.label.contains("AutoDL"))
            .expect("AutoDL endpoint from catalog is rendered");
        let MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetProviderSelection(
            selection,
        ))) = &autodl.action
        else {
            panic!("expected catalog selection action");
        };
        assert_eq!(selection.family_id, "moonshot");
        assert_eq!(selection.model_id, "kimi-k2.5");
        assert_eq!(selection.route.route_id, "autodl");
        assert_eq!(
            selection.route.base_url.as_deref(),
            Some("https://www.autodl.art/api/v1")
        );
        assert_eq!(
            selection.route.api_key_env.as_deref(),
            Some("AUTODL_API_KEY")
        );
    }

    #[test]
    fn onboarding_provider_menu_shows_api_test_progress() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_PROFILE_LLM_TEST,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        let onboarding = OnboardingWizardState {
            provider: LlmSelectionConfig {
                family_id: "deepseek".into(),
                model_id: "deepseek-reasoner".into(),
                route: LlmRouteConfig {
                    route_id: "deepseek".into(),
                    label: Some("Official API".into()),
                    api_type: Some("openai".into()),
                    ..LlmRouteConfig::default()
                },
                ..LlmSelectionConfig::default()
            },
            api_key: Some(crate::model::SecretString::new("sk-test-secret")),
            provider_pending: Some(OnboardingProviderPending::Test),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_ONBOARD), &ctx) else {
            panic!("expected onboarding provider menu");
        };
        let test_item = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.provider.test")
            .expect("test provider row");
        assert_eq!(test_item.label, "Testing connection...");
        assert!(test_item.state.loading);
        assert_eq!(test_item.disabled_reason, None);
        let save_item = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.provider.save")
            .expect("save provider row");
        assert_eq!(save_item.label, "Save unavailable while testing");
        assert_eq!(save_item.disabled_reason, None);
        let family_item = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.provider.family")
            .expect("family row");
        assert_eq!(family_item.disabled_reason, None);
        assert_eq!(family_item.state.required_valid, Some(true));
        let key_item = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.provider.key")
            .expect("api key row");
        assert_eq!(key_item.disabled_reason, None);
        assert_eq!(key_item.state.required_valid, Some(true));
    }

    #[test]
    fn provider_menu_shows_api_test_progress() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_PROFILE_LLM_TEST,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        let onboarding = OnboardingWizardState {
            provider: LlmSelectionConfig {
                family_id: "deepseek".into(),
                model_id: "deepseek-reasoner".into(),
                route: LlmRouteConfig {
                    route_id: "deepseek".into(),
                    label: Some("Official API".into()),
                    api_type: Some("openai".into()),
                    ..LlmRouteConfig::default()
                },
                ..LlmSelectionConfig::default()
            },
            api_key: Some(crate::model::SecretString::new("sk-test-secret")),
            provider_pending: Some(OnboardingProviderPending::Test),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PROVIDER), &ctx)
        else {
            panic!("expected provider menu");
        };
        let test_item = spec
            .items
            .iter()
            .find(|item| item.id == "provider.test")
            .expect("provider test row");
        assert_eq!(test_item.label, "Testing connection...");
        assert!(test_item.state.loading);
        assert_eq!(test_item.disabled_reason, None);
        let fallback_item = spec
            .items
            .iter()
            .find(|item| item.id == "provider.fallback")
            .expect("provider fallback row");
        assert_eq!(fallback_item.label, "Fallback unavailable while testing");
        assert_eq!(fallback_item.disabled_reason, None);
    }

    #[test]
    fn provider_menu_shows_last_saved_provider_status() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([APPUI_METHOD_PROFILE_LLM_CATALOG]);
        let onboarding = OnboardingWizardState {
            last_saved_provider_label: Some(
                "minimax / MiniMax-M2.5-highspeed via wisemodel".into(),
            ),
            last_saved_provider_target: Some(OnboardingProviderSaveTarget::Fallback),
            saved_primary_provider_label: Some("moonshot / kimi-k2.5 via autodl".into()),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PROVIDER), &ctx)
        else {
            panic!("expected provider menu");
        };
        let saved_item = spec
            .items
            .iter()
            .find(|item| item.id == "provider.saved")
            .expect("saved provider row");

        assert_eq!(
            saved_item.label,
            "Saved provider: fallback minimax / MiniMax-M2.5-highspeed via wisemodel"
        );
        assert_eq!(saved_item.state.checked, Some(true));
        assert_eq!(saved_item.state.required_valid, Some(true));
    }

    #[test]
    fn provider_menu_shows_configured_provider_list_from_appui() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST]);
        let llm_state = crate::model::ProfileLlmListResult {
            profile_id: Some("coding".into()),
            primary: Some(LlmConfiguredProvider {
                family_id: Some("moonshot".into()),
                model_id: Some("kimi-k2.5".into()),
                route_id: Some("autodl".into()),
                base_url: Some("https://www.autodl.art/api/v1".into()),
                api_key_env: Some("AUTODL_API_KEY".into()),
                has_api_key: true,
                selected: true,
                ..configured_provider_for_test()
            }),
            fallbacks: vec![LlmConfiguredProvider {
                family_id: Some("minimax".into()),
                model_id: Some("MiniMax-M2.5-highspeed".into()),
                route: Some(LlmRouteConfig {
                    route_id: "wisemodel".into(),
                    base_url: Some("https://open.ospreyai.cn/v1".into()),
                    api_key_env: Some("WISEMODEL_API_KEY".into()),
                    ..LlmRouteConfig::default()
                }),
                has_api_key: true,
                ..configured_provider_for_test()
            }],
            llm: None,
            runtime_policy_stamp: None,
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                profile_llm_state: Some(&llm_state),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PROVIDER), &ctx)
        else {
            panic!("expected provider menu");
        };
        let rendered = rendered_menu_text(&spec);

        assert!(rendered.contains("Saved primary: moonshot / kimi-k2.5 via autodl"));
        assert!(
            rendered.contains("Saved fallback 1: minimax / MiniMax-M2.5-highspeed via wisemodel")
        );
        assert!(rendered.contains("env: AUTODL_API_KEY"));
        assert!(rendered.contains("env: WISEMODEL_API_KEY"));
        let primary = spec
            .items
            .iter()
            .find(|item| item.id == "provider.saved.primary")
            .expect("saved primary row");
        assert!(primary.state.current);
        assert_eq!(primary.state.required_valid, Some(true));
    }

    fn configured_provider_for_test() -> LlmConfiguredProvider {
        LlmConfiguredProvider {
            provider: String::new(),
            model: String::new(),
            family_id: None,
            model_id: None,
            route: None,
            route_id: None,
            base_url: None,
            api_key_env: None,
            has_api_key: false,
            selected: false,
            available: None,
            model_hints: None,
            cost_per_m: None,
            strong: None,
        }
    }

    #[test]
    fn onboarding_menu_uses_local_profile_create_when_advertised() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_PROFILE_LOCAL_CREATE,
            APPUI_METHOD_AUTH_SEND_CODE,
            APPUI_METHOD_AUTH_VERIFY,
            APPUI_METHOD_PROFILE_LLM_CATALOG,
        ]);
        let session_id = SessionKey("local:test".into());
        let onboarding = OnboardingWizardState {
            name: "Ada Lovelace".into(),
            username: "ada".into(),
            email: "ada@example.com".into(),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_ONBOARD), &ctx) else {
            panic!("expected onboarding menu");
        };

        assert!(
            spec.items
                .iter()
                .any(|item| item.id == "onboard.local.create")
        );
        assert!(!spec.items.iter().any(|item| item.id == "onboard.auth.send"));
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id == "onboard.auth.verify")
        );
        assert_eq!(spec.title, "Welcome to Octos");
        // The first-run splash renders in the MAIN window (app.rs
        // render_onboarding_first_launch_layout); the welcome menu now also
        // carries the wizard progress checklist as its preview so the user sees
        // the full Step-N-of-M path from the first screen.
        assert!(
            matches!(
                spec.preview,
                Some(crate::menu::types::MenuPreview::KeyValues { .. })
            ),
            "welcome menu should show the wizard progress checklist"
        );
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id == "onboard.catalog.refresh")
        );
        let name = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.local.name")
            .expect("name row");
        let MenuAction::Local(LocalAction::EditComposer(draft)) = &name.action else {
            panic!("name row should start inline editing");
        };
        assert_eq!(draft, "/onboard name ");
        assert_eq!(name.state.required_valid, Some(true));
    }

    #[test]
    fn onboarding_required_local_profile_rows_mark_missing_values() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([APPUI_METHOD_PROFILE_LOCAL_CREATE]);
        let onboarding = OnboardingWizardState::default();
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_ONBOARD), &ctx) else {
            panic!("expected onboarding menu");
        };

        for id in [
            "onboard.local.name",
            "onboard.local.username",
            "onboard.local.email",
        ] {
            let item = spec
                .items
                .iter()
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("{id} row"));
            assert_eq!(item.state.required_valid, Some(false));
        }
    }

    #[test]
    fn status_menu_renders_snapshot_without_server_status_method() {
        let registry = core_menu_registry();
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot {
                status: Some("ready"),
                target: Some("local mock"),
                selected_session_title: Some("test session"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let result = registry.build(&MenuId::from(MENU_STATUS), &ctx);

        let MenuBuildResult::Ready(spec) = result else {
            panic!("expected snapshot-backed status menu");
        };
        assert_eq!(spec.id, MenuId::from(MENU_STATUS));
        assert!(
            spec.items
                .iter()
                .any(|item| item.label == "Refresh server status" && !item.is_enabled())
        );
    }

    #[test]
    fn status_menu_renders_cached_runtime_policy_when_present() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([AppUiActionKind::SessionStatusRead.method()]);
        let session_id = SessionKey("local:test".into());
        let status = runtime_status(&session_id);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                status: Some("ready"),
                target: Some("ws://example.test/ui"),
                cwd: Some("/workspace/octos"),
                current_model: Some("deepseek-v4-pro"),
                current_profile: Some("coding"),
                runtime_status: Some(&status),
                selected_session_id: Some(&session_id),
                selected_session_title: Some("test session"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_STATUS), &ctx) else {
            panic!("expected status menu");
        };

        let model = spec
            .items
            .iter()
            .find(|item| item.label == "Model")
            .expect("model row");
        assert_eq!(model.description.as_deref(), Some("deepseek-v4-pro"));

        let tool_policy = spec
            .items
            .iter()
            .find(|item| item.label == "Tool Policy")
            .expect("tool policy row");
        assert_eq!(tool_policy.description.as_deref(), Some("coding-v3"));

        let tool_contract = spec
            .items
            .iter()
            .find(|item| item.label == "Tool Contract")
            .expect("tool contract row");
        assert_eq!(
            tool_contract.description.as_deref(),
            Some("codex-compatible-coding-v1 v1")
        );

        let preview = spec.preview.expect("status preview");
        let MenuPreview::KeyValues { rows, .. } = preview else {
            panic!("expected key/value preview");
        };
        assert!(
            rows.iter()
                .any(|row| row.label == "memory" && row.value == "profile-session")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "queue" && row.value == "collect")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "mcp" && row.value == "github, filesystem")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "model_toolset" && row.value == "coding")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "tool_discovery" && row.value == "enabled")
        );
    }

    #[test]
    fn permissions_preview_uses_server_policy_fields_without_inferring_dangerous() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            AppUiActionKind::SessionStatusRead.method(),
            AppUiActionKind::PermissionProfileList.method(),
        ]);
        let session_id = SessionKey("local:test".into());
        let status = runtime_status(&session_id);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                runtime_status: Some(&status),
                selected_session_id: Some(&session_id),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx)
        else {
            panic!("expected permissions menu");
        };
        let MenuPreview::KeyValues { rows, .. } = spec.preview.expect("permissions preview") else {
            panic!("expected key/value preview");
        };

        assert!(
            rows.iter()
                .any(|row| row.label == "approval_policy" && row.value == "never")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "sandbox_mode" && row.value == "workspace-write")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "filesystem_scope" && row.value == "workspace")
        );
        assert!(
            rows.iter()
                .any(|row| row.label == "network" && row.value == "blocked")
        );
        assert!(!rows.iter().any(|row| row.label == "dangerous"));
    }

    #[test]
    fn dangerous_access_renders_only_after_server_confirmation() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([AppUiActionKind::SessionStatusRead.method()]);
        let session_id = SessionKey("local:test".into());
        let mut status = runtime_status(&session_id);
        status.sandbox_mode = Some("danger-full-access".into());
        status.filesystem_scope = Some("host".into());
        status.network = Some("allowed".into());
        status.runtime_policy_stamp = None;
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                runtime_status: Some(&status),
                selected_session_id: Some(&session_id),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_STATUS), &ctx) else {
            panic!("expected status menu");
        };
        let MenuPreview::KeyValues { rows, .. } = spec.preview.expect("status preview") else {
            panic!("expected key/value preview");
        };

        assert!(rows.iter().any(|row| {
            row.label == "dangerous"
                && row.value == "server-confirmed danger-full-access host scope"
        }));
    }

    #[test]
    fn status_menu_shows_cached_status_placeholder_when_capability_exists_without_data() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([AppUiActionKind::SessionStatusRead.method()]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                selected_session_title: Some("test"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_STATUS), &ctx) else {
            panic!("expected status menu");
        };

        let placeholder = spec
            .items
            .iter()
            .find(|item| item.label == "Server runtime status")
            .expect("placeholder row");
        assert!(!placeholder.is_enabled());
        assert_eq!(
            placeholder.disabled_reason.as_deref(),
            Some("session/status/read is advertised but no result is cached yet")
        );
    }

    #[test]
    fn status_menu_refresh_uses_session_status_read_when_capability_exists() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([AppUiActionKind::SessionStatusRead.method()]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                selected_session_title: Some("test"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_STATUS), &ctx) else {
            panic!("expected status menu");
        };

        let refresh = spec
            .items
            .iter()
            .find(|item| item.id == "status.refresh")
            .expect("refresh item");
        let MenuAction::SendAppUi(AppUiCommand::ReadSessionStatus(params)) = &refresh.action else {
            panic!("expected session/status/read action");
        };
        assert_eq!(params.session_id, session_id);
        assert!(refresh.is_enabled());
    }

    #[test]
    fn cost_menu_renders_usage_totals_from_cached_session_status() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([AppUiActionKind::SessionStatusRead.method()]);
        let session_id = SessionKey("local:test".into());
        let status = runtime_status(&session_id);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                runtime_status: Some(&status),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_COST), &ctx) else {
            panic!("expected cost menu");
        };
        assert!(matches!(
            &spec
                .items
                .iter()
                .find(|item| item.id == "cost.refresh")
                .expect("refresh item")
                .action,
            MenuAction::SendAppUi(AppUiCommand::ReadSessionStatus(_))
        ));
        let cost = spec
            .items
            .iter()
            .find(|item| item.id == "cost.estimated")
            .expect("cost item");
        assert_eq!(cost.description.as_deref(), Some("$0.0025"));
    }

    #[test]
    fn model_menu_requires_list_and_select_and_renders_cached_models() {
        let registry = core_menu_registry();
        let only_list = CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST]);
        let session_id = SessionKey("local:test".into());
        let missing_select_ctx = MenuContext {
            availability: AvailabilityContext::protocol(&only_list),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                profile_llm_state: Some(&crate::model::ProfileLlmListResult {
                    profile_id: Some("coding".into()),
                    primary: Some(LlmConfiguredProvider {
                        family_id: Some("deepseek".into()),
                        model_id: Some("deepseek-reasoner".into()),
                        route_id: Some("official".into()),
                        has_api_key: true,
                        ..configured_provider_for_test()
                    }),
                    fallbacks: Vec::new(),
                    llm: None,
                    runtime_policy_stamp: None,
                }),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let MenuBuildResult::Ready(spec) =
            registry.build(&MenuId::from(MENU_MODEL), &missing_select_ctx)
        else {
            panic!("expected model menu to stay visible without model/select");
        };
        let configured = spec
            .items
            .iter()
            .find(|item| item.label.contains("deepseek-reasoner"))
            .expect("configured model row stays visible");
        assert!(
            configured
                .disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains(APPUI_METHOD_MODEL_SELECT))
        );

        let capabilities =
            CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST, APPUI_METHOD_MODEL_SELECT]);
        let catalog = SessionModelCatalog {
            session_id: session_id.clone(),
            models: vec![ModelStatus {
                model: "deepseek-v4-pro".into(),
                provider: "deepseek".into(),
                title: Some("DeepSeek V4 Pro".into()),
                family: Some("deepseek".into()),
                route: Some("coding".into()),
                selected: true,
                available: Some(true),
                queue_mode: Some("interactive".into()),
                qoe_policy: Some("adaptive".into()),
            }],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                model_catalog: Some(&catalog),
                current_model: Some("deepseek-v4-pro"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MODEL), &ctx) else {
            panic!("expected model menu");
        };
        let refresh = spec
            .items
            .iter()
            .find(|item| item.id == "model.refresh")
            .expect("refresh item");
        assert!(matches!(
            &refresh.action,
            MenuAction::SendAppUi(AppUiCommand::ProfileLlmList(_))
        ));
        let select = spec
            .items
            .iter()
            .find(|item| item.label == "DeepSeek V4 Pro")
            .expect("model selection");
        let MenuAction::SendAppUi(AppUiCommand::ProfileLlmSelect(params)) = &select.action else {
            panic!("expected profile/llm/select action");
        };
        assert_eq!(params.model_id, "deepseek-v4-pro");
        assert_eq!(params.family_id, "deepseek");
        assert_eq!(params.route_id, "coding");
        assert!(select.state.current);
    }

    #[test]
    fn model_menu_can_refresh_profile_models_before_session_open() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST, APPUI_METHOD_MODEL_SELECT]);
        let ctx = MenuContext {
            availability: AvailabilityContext {
                session_open: false,
                ..AvailabilityContext::protocol(&capabilities)
            },
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MODEL), &ctx) else {
            panic!("expected model menu before session/open");
        };
        let refresh = spec
            .items
            .iter()
            .find(|item| item.id == "model.refresh")
            .expect("refresh item");
        let MenuAction::SendAppUi(AppUiCommand::ProfileLlmList(params)) = &refresh.action else {
            panic!("expected profile/llm/list action");
        };
        assert_eq!(params.profile_id.as_deref(), Some("coding"));
    }

    #[test]
    fn model_menu_renders_profile_llm_state_without_open_session() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST, APPUI_METHOD_MODEL_SELECT]);
        let profile_llm_state = crate::model::ProfileLlmListResult {
            profile_id: Some("dspfac".into()),
            primary: Some(LlmConfiguredProvider {
                family_id: Some("moonshot".into()),
                model_id: Some("kimi-k2.6".into()),
                route_id: Some("moonshot".into()),
                has_api_key: true,
                selected: true,
                ..configured_provider_for_test()
            }),
            fallbacks: vec![LlmConfiguredProvider {
                family_id: Some("deepseek".into()),
                model_id: Some("deepseek-reasoner".into()),
                route_id: Some("deepseek".into()),
                has_api_key: true,
                ..configured_provider_for_test()
            }],
            llm: None,
            runtime_policy_stamp: None,
        };
        let ctx = MenuContext {
            availability: AvailabilityContext {
                session_open: false,
                ..AvailabilityContext::protocol(&capabilities)
            },
            app: MenuAppSnapshot {
                profile_llm_state: Some(&profile_llm_state),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MODEL), &ctx) else {
            panic!("expected model menu before session/open");
        };
        let rendered = rendered_menu_text(&spec);
        assert!(rendered.contains("moonshot / kimi-k2.6"));
        assert!(rendered.contains("deepseek / deepseek-reasoner"));
        let select = spec
            .items
            .iter()
            .find(|item| item.label.contains("kimi-k2.6"))
            .expect("primary model row");
        let MenuAction::SendAppUi(AppUiCommand::ProfileLlmSelect(params)) = &select.action else {
            panic!("expected profile/llm/select");
        };
        assert_eq!(params.profile_id.as_deref(), Some("dspfac"));
        assert_eq!(params.family_id, "moonshot");
        assert_eq!(params.model_id, "kimi-k2.6");
        assert_eq!(params.route_id, "moonshot");
    }

    #[test]
    fn login_menu_shows_otp_only_when_advertised() {
        let registry = core_menu_registry();
        let session_id = SessionKey("local:test".into());
        let status_only = CapabilitySet::from_methods([APPUI_METHOD_AUTH_STATUS]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&status_only),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_LOGIN), &ctx) else {
            panic!("expected login menu");
        };
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id.starts_with("login.otp"))
        );

        let otp = CapabilitySet::from_methods([
            APPUI_METHOD_AUTH_STATUS,
            APPUI_METHOD_AUTH_SEND_CODE,
            APPUI_METHOD_AUTH_VERIFY,
        ]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&otp),
            ..ctx
        };
        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_LOGIN), &ctx) else {
            panic!("expected login menu");
        };
        assert!(spec.items.iter().any(|item| item.id == "login.otp.send"));
        assert!(spec.items.iter().any(|item| item.id == "login.otp.verify"));

        let solo = CapabilitySet::from_methods([
            APPUI_METHOD_PROFILE_LOCAL_CREATE,
            APPUI_METHOD_AUTH_STATUS,
            APPUI_METHOD_AUTH_SEND_CODE,
            APPUI_METHOD_AUTH_VERIFY,
        ]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&solo),
            ..ctx
        };
        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_LOGIN), &ctx) else {
            panic!("expected login menu");
        };
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id.starts_with("login.otp"))
        );
    }

    #[test]
    fn provider_menu_uses_dashboard_catalog_and_has_no_hardcoded_shortcuts() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_PROFILE_LLM_CATALOG,
            APPUI_METHOD_PROFILE_LLM_TEST,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        let session_id = SessionKey("local:test".into());
        let onboarding = OnboardingWizardState {
            api_key: Some(crate::model::SecretString::new("top-secret")),
            ..OnboardingWizardState::default()
        };
        let mut families = serde_json::Map::new();
        families.insert(
            "minimax".into(),
            serde_json::json!({
                "env": "MINIMAX_API_KEY",
                "models": [{
                    "id": "MiniMax-M2.5-highspeed",
                    "endpoints": [{
                        "id": "wisemodel",
                        "label": "WiseModel",
                        "base_url": "https://open.ospreyai.cn/v1",
                        "api_key_env": "WISEMODEL_API_KEY"
                    }]
                }]
            }),
        );
        let catalog = crate::model::ProfileLlmCatalogResult { families };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                current_profile: Some("coding"),
                onboarding: Some(&onboarding),
                profile_llm_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PROVIDER), &ctx)
        else {
            panic!("expected provider menu");
        };
        let rendered = rendered_menu_text(&spec);
        assert!(!rendered.contains("top-secret"));
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id == "provider.add.moonshot.autodl")
        );
        let wisemodel = spec
            .items
            .iter()
            .find(|item| item.label.contains("WiseModel"))
            .expect("WiseModel endpoint from catalog");
        let MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetProviderSelection(
            selection,
        ))) = &wisemodel.action
        else {
            panic!("expected catalog selection action");
        };
        assert_eq!(selection.family_id, "minimax");
        assert_eq!(selection.model_id, "MiniMax-M2.5-highspeed");
        assert_eq!(selection.route.route_id, "wisemodel");
        assert_eq!(
            selection.route.base_url.as_deref(),
            Some("https://open.ospreyai.cn/v1")
        );
        assert_eq!(
            selection.route.api_key_env.as_deref(),
            Some("WISEMODEL_API_KEY")
        );
    }

    #[test]
    fn model_menu_displays_only_mocked_server_returned_models() {
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST, APPUI_METHOD_MODEL_SELECT]);
        let session_id = SessionKey("local:test".into());
        let catalog = SessionModelCatalog {
            session_id: session_id.clone(),
            models: vec![ModelStatus {
                model: "server-only-model".into(),
                provider: "server-provider".into(),
                title: Some("Server Only".into()),
                family: Some("server-family".into()),
                route: Some("server-route".into()),
                selected: false,
                available: Some(true),
                queue_mode: None,
                qoe_policy: None,
            }],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                model_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MODEL), &ctx) else {
            panic!("expected model menu");
        };
        let labels = spec
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Server Only"));
        assert!(!labels.contains(&"DeepSeek V4 Pro"));
        assert!(!labels.contains(&"Mock Coding"));
    }

    #[test]
    fn mcp_menu_renders_cached_server_statuses_from_appui() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([AppUiActionKind::McpStatusList.method()]);
        let session_id = SessionKey("local:test".into());
        let catalog = SessionMcpCatalog {
            session_id: session_id.clone(),
            servers: vec![
                McpStatus {
                    server: "github".into(),
                    status: "connected".into(),
                    transport: Some("stdio".into()),
                    endpoint: None,
                    tool_count: Some(8),
                    detail: Some("ready".into()),
                    last_error: None,
                },
                McpStatus {
                    server: "playwright".into(),
                    status: "failed".into(),
                    transport: Some("stdio".into()),
                    endpoint: None,
                    tool_count: Some(0),
                    detail: None,
                    last_error: Some("not installed".into()),
                },
            ],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                mcp_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MCP), &ctx) else {
            panic!("expected MCP menu");
        };
        assert!(matches!(
            &spec
                .items
                .iter()
                .find(|item| item.id == "mcp.refresh")
                .expect("refresh item")
                .action,
            MenuAction::SendAppUi(AppUiCommand::ListMcpStatus(_))
        ));
        let failed = spec
            .items
            .iter()
            .find(|item| item.label == "playwright")
            .expect("failed server");
        assert!(failed.state.destructive);
        assert!(
            failed
                .description
                .as_deref()
                .is_some_and(|description| description.contains("not installed"))
        );
    }

    #[test]
    fn mcp_menu_prefers_config_truth_and_gates_mutations() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_MCP_CONFIG_LIST,
            APPUI_METHOD_MCP_CONFIG_SET_ENABLED,
        ]);
        let session_id = SessionKey("local:test".into());
        let config = McpConfigListResult {
            session_id: Some(session_id.clone()),
            profile_id: Some("coding".into()),
            servers: vec![McpConfigEntry {
                name: "github".into(),
                enabled: true,
                transport: Some("stdio".into()),
                command: Some("mcp-github".into()),
                tool_count: Some(8),
                ..McpConfigEntry::default()
            }],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                current_profile: Some("coding"),
                mcp_config_catalog: Some(&config),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MCP), &ctx) else {
            panic!("expected MCP menu");
        };
        let toggle = spec
            .items
            .iter()
            .find(|item| item.id == "mcp.server.github.toggle")
            .expect("toggle item");
        let MenuAction::SendAppUi(AppUiCommand::SetMcpConfigEnabled(params)) = &toggle.action
        else {
            panic!("toggle should call Octos UI set_enabled");
        };
        assert_eq!(params.server, "github");
        assert!(!params.enabled);
        assert!(toggle.disabled_reason.is_none());

        let test = spec
            .items
            .iter()
            .find(|item| item.id == "mcp.server.github.test")
            .expect("test item");
        assert!(
            test.disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("mcp/config/test"))
        );
    }

    #[test]
    fn tool_settings_menu_renders_configured_tools_and_gates_actions() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_TOOL_CONFIG_LIST,
            APPUI_METHOD_TOOL_CONFIG_SET_ENABLED,
            APPUI_METHOD_TOOL_CONFIG_TEST,
        ]);
        let session_id = SessionKey("local:test".into());
        let config = ToolConfigListResult {
            session_id: Some(session_id.clone()),
            profile_id: Some("coding".into()),
            policy_id: Some("coding-tools".into()),
            tools: vec![ToolConfigEntry {
                tool: "web_fetch".into(),
                title: Some("Web Fetch".into()),
                enabled: false,
                visible: true,
                source: Some("platform".into()),
                risk: Some("medium".into()),
                ..ToolConfigEntry::default()
            }],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                current_profile: Some("coding"),
                tool_config_catalog: Some(&config),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_TOOL_SETTINGS), &ctx)
        else {
            panic!("expected tool settings menu");
        };
        let toggle = spec
            .items
            .iter()
            .find(|item| item.id == "tools.tool.web_fetch.toggle")
            .expect("tool toggle");
        let MenuAction::SendAppUi(AppUiCommand::SetToolConfigEnabled(params)) = &toggle.action
        else {
            panic!("toggle should call Octos UI set_enabled");
        };
        assert_eq!(params.tool, "web_fetch");
        assert!(params.enabled);
        assert!(toggle.disabled_reason.is_none());

        let delete = spec
            .items
            .iter()
            .find(|item| item.id == "tools.tool.web_fetch.delete")
            .expect("delete item");
        assert!(
            delete
                .disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("tool/config/delete"))
        );
    }

    #[test]
    fn tool_settings_menu_surfaces_coding_tool_contract_gaps() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([APPUI_METHOD_TOOL_STATUS_LIST]);
        let session_id = SessionKey("local:test".into());
        let catalog = SessionToolCatalog {
            session_id: session_id.clone(),
            policy_id: Some("coding-v3".into()),
            coding_tool_contract: Some(CodingToolContract {
                id: "codex-compatible-coding-v1".into(),
                version: "1".into(),
                feature: "coding.tool_contract.v1".into(),
                status: "incomplete".into(),
                required_tool_names: vec!["apply_patch".into(), "exec_command".into()],
                required_tools: vec![CodingToolContractTool {
                    name: "exec_command".into(),
                    category: "runtime".into(),
                    aliases: vec!["shell".into()],
                    capability: "coding.exec_session.v1".into(),
                    policy: "approval_gated".into(),
                    status: "missing".into(),
                    backend_tool: None,
                    detail: Some("exec sessions are backend blocked".into()),
                }],
                missing_required_tools: vec!["exec_command".into()],
                policy: Some(CodingToolContractPolicy {
                    tool_policy_id: Some("coding-v3".into()),
                    sandbox_mode: Some("workspace-write".into()),
                    approval_policy: Some("on-request".into()),
                }),
            }),
            tools: vec![ToolStatus {
                tool: "apply_patch".into(),
                title: Some("Apply Patch".into()),
                source: Some("platform".into()),
                enabled: true,
                visible: true,
                tags: vec!["edit".into()],
                risk: Some("medium".into()),
                policy_id: Some("coding-v3".into()),
                denial: None,
            }],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                tool_catalog: Some(&catalog),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_TOOL_SETTINGS), &ctx)
        else {
            panic!("expected tool settings menu");
        };

        let contract = spec
            .items
            .iter()
            .find(|item| item.id == "tools.contract")
            .expect("contract item");
        assert_eq!(contract.state.required_valid, Some(false));
        assert!(
            contract
                .description
                .as_deref()
                .is_some_and(|description| description.contains("coding.tool_contract.v1"))
        );

        let missing = spec
            .items
            .iter()
            .find(|item| item.id == "tools.contract.missing.exec_command")
            .expect("missing P0 item");
        assert!(missing.state.destructive);
        assert!(
            missing.description.as_deref().is_some_and(
                |description| description.contains("exec sessions are backend blocked")
            )
        );

        let MenuPreview::KeyValues { rows, .. } = spec.preview.expect("tool preview") else {
            panic!("expected key/value preview");
        };
        assert!(
            rows.iter()
                .any(|row| row.label == "missing P0" && row.value == "exec_command")
        );
        assert!(rows.iter().any(|row| {
            row.label == "contract policy" && row.value.contains("sandbox workspace-write")
        }));
    }

    #[test]
    fn skills_menu_renders_cached_installed_and_registry_actions() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_PROFILE_SKILLS_LIST,
            APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
            APPUI_METHOD_PROFILE_SKILLS_INSTALL,
            APPUI_METHOD_PROFILE_SKILLS_REMOVE,
        ]);
        let installed = crate::model::ProfileSkillsListResult {
            profile_id: Some("coding".into()),
            count: 1,
            skills: vec![crate::model::ProfileSkillEntry {
                name: "deep-search".into(),
                version: Some("0.1.0".into()),
                tool_count: 1,
                source_repo: Some("octos-org/octos-hub/skills/deep-search".into()),
                installed: true,
                status: Some("installed".into()),
            }],
        };
        let registry_result = crate::model::ProfileSkillsRegistrySearchResult {
            profile_id: Some("coding".into()),
            packages: vec![crate::model::ProfileSkillRegistryPackage {
                name: "news".into(),
                description: "News skill".into(),
                repo: "octos-org/octos-hub/skills/news".into(),
                version: Some("0.2.0".into()),
                author: None,
                license: None,
                skills: vec!["news".into()],
                requires: Vec::new(),
                provides_tools: false,
                tags: vec!["news".into()],
                installed: false,
                installed_skills: Vec::new(),
            }],
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("coding"),
                profile_skills: Some(&installed),
                profile_skill_registry: Some(&registry_result),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_SKILLS), &ctx) else {
            panic!("expected skills menu");
        };

        let remove = spec
            .items
            .iter()
            .find(|item| item.id == "skills.remove.deep-search")
            .expect("remove item");
        assert!(matches!(
            &remove.action,
            MenuAction::SendAppUi(AppUiCommand::ProfileSkillsRemove(_))
        ));
        assert!(remove.state.destructive);

        let install = spec
            .items
            .iter()
            .find(|item| item.id == "skills.registry.news")
            .expect("registry install item");
        let MenuAction::SendAppUi(AppUiCommand::ProfileSkillsInstall(params)) = &install.action
        else {
            panic!("expected profile skills install action");
        };
        assert_eq!(params.profile_id.as_deref(), Some("coding"));
        assert_eq!(params.repo, "octos-org/octos-hub/skills/news");
    }

    #[test]
    fn permissions_menu_uses_existing_approval_scopes_command() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([methods::APPROVAL_SCOPES_LIST]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                selected_session_title: Some("test"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let result = registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx);

        let MenuBuildResult::Ready(spec) = result else {
            panic!("expected permissions menu");
        };
        let Some(item) = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.scopes.refresh")
        else {
            panic!("expected refresh item");
        };
        assert!(item.is_enabled());
        assert!(matches!(
            &item.action,
            MenuAction::SendAppUi(AppUiCommand::ListApprovalScopes(_))
        ));
    }

    #[test]
    fn permissions_menu_is_unavailable_without_related_capabilities() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([methods::TURN_INTERRUPT]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Unavailable(spec) =
            registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx)
        else {
            panic!("expected permissions unavailable");
        };
        assert!(
            spec.message
                .contains("Octos UI permission methods are not advertised")
        );
    }

    #[test]
    fn permissions_menu_shows_codex_style_permission_modes_when_mutation_is_missing() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([methods::APPROVAL_SCOPES_LIST]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                selected_session_title: Some("test"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let result = registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx);

        let MenuBuildResult::Ready(spec) = result else {
            panic!("expected permissions menu");
        };
        let labels = spec
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(spec.title, "Update Model Permissions");
        assert!(labels.contains(&"Default"));
        assert!(labels.contains(&"Read Only"));
        assert!(labels.contains(&"Workspace Write"));
        assert!(labels.contains(&"Full Access"));
        assert!(labels.contains(&"Allow Network"));
        assert!(labels.contains(&"Block Network"));

        let full_access = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.full_access")
            .expect("full access row");
        assert!(!full_access.is_enabled());
        assert!(
            full_access
                .disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("profile/set"))
        );
    }

    #[test]
    fn permissions_menu_sends_profile_commands_when_capability_exists() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            methods::APPROVAL_SCOPES_LIST,
            methods::PERMISSION_PROFILE_LIST,
            methods::PERMISSION_PROFILE_SET,
        ]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                selected_session_title: Some("test"),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx)
        else {
            panic!("expected permissions menu");
        };

        let full_access = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.full_access")
            .expect("full access row");
        assert!(full_access.is_enabled());
        assert!(matches!(
            &full_access.action,
            MenuAction::SendAppUi(AppUiCommand::SetPermissionProfile(_))
        ));

        let refresh = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.profile.refresh")
            .expect("profile refresh row");
        assert!(refresh.is_enabled());
        assert!(matches!(
            &refresh.action,
            MenuAction::SendAppUi(AppUiCommand::ListPermissionProfiles(_))
        ));
    }

    #[test]
    fn permissions_menu_marks_known_permission_profile_state() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            methods::PERMISSION_PROFILE_LIST,
            methods::PERMISSION_PROFILE_SET,
        ]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                permission_profile: Some(PermissionProfileSelection {
                    mode: PermissionProfileMode::DangerFullAccess,
                    network: PermissionNetworkPolicy::Allow,
                }),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx)
        else {
            panic!("expected permissions menu");
        };

        let full_access = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.full_access")
            .expect("full access row");
        assert!(full_access.state.current);

        let allow_network = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.network.allow")
            .expect("allow network row");
        assert_eq!(allow_network.state.checked, Some(true));

        let block_network = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.network.block")
            .expect("block network row");
        assert_eq!(block_network.state.checked, Some(false));
    }

    #[test]
    fn permissions_menu_uses_server_approval_stamp_for_never_state() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            methods::PERMISSION_PROFILE_LIST,
            methods::PERMISSION_PROFILE_SET,
        ]);
        let session_id = SessionKey("local:test".into());
        let status = runtime_status(&session_id);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                selected_session_id: Some(&session_id),
                permission_profile: Some(PermissionProfileSelection {
                    mode: PermissionProfileMode::WorkspaceWrite,
                    network: PermissionNetworkPolicy::Deny,
                }),
                runtime_status: Some(&status),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_PERMISSIONS), &ctx)
        else {
            panic!("expected permissions menu");
        };

        let default = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.default")
            .expect("default row");
        assert!(!default.state.current);
        let MenuAction::SendAppUi(AppUiCommand::SetPermissionProfile(params)) = &default.action
        else {
            panic!("expected permission profile update");
        };
        assert_eq!(params.update.approval_policy.as_deref(), Some("on-request"));

        let workspace_never = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.workspace_write_never")
            .expect("workspace never row");
        assert!(workspace_never.state.current);
    }
}
