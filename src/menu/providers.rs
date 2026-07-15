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
    AppUiActionKind, AvailabilityStatus, ClientEffect, CommandRegistry, KeyBinding, LocalAction,
    MenuAction, MenuAppSnapshot, MenuBuildResult, MenuContext, MenuId, MenuItem, MenuItemState,
    MenuMode, MenuPreview, MenuPreviewRow, MenuProvider, MenuRegistry, MenuSpec, MenuStatusSpec,
    MenuTab,
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
        APPUI_METHOD_PROFILE_SKILLS_REMOVE, APPUI_METHOD_SESSION_COMPACT,
        APPUI_METHOD_TOOL_CONFIG_DELETE, APPUI_METHOD_TOOL_CONFIG_LIST,
        APPUI_METHOD_TOOL_CONFIG_SET_ENABLED, APPUI_METHOD_TOOL_CONFIG_TEST,
        APPUI_METHOD_TOOL_CONFIG_UPSERT, APPUI_METHOD_TOOL_STATUS_LIST,
        APPUI_ONBOARDING_METHODS_ANY, APPUI_PERMISSION_MENU_METHODS_ANY,
        APPUI_PROVIDER_MENU_METHODS_ANY, APPUI_TOOL_SETTINGS_MENU_METHODS_ANY,
        MENU_COMPACT_CONFIRM, MENU_COST, MENU_HELP, MENU_KEYMAP, MENU_LOGIN, MENU_MCP, MENU_MODEL,
        MENU_ONBOARD, MENU_ONBOARD_LANGUAGE, MENU_PERMISSIONS, MENU_PROVIDER, MENU_RESUME,
        MENU_REWIND, MENU_SKILLS, MENU_STATUS, MENU_STATUS_LINE, MENU_THEME, MENU_TITLE,
        MENU_TOOL_SETTINGS,
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
    SessionCompactParams, SessionStatusReadParams, ToolConfigDeleteParams, ToolConfigEntry,
    ToolConfigListParams, ToolConfigSetEnabledParams, ToolConfigTestParams, ToolStatus,
    ToolStatusListParams,
};

pub fn core_menu_registry() -> MenuRegistry {
    let mut registry = MenuRegistry::new();
    for provider in [
        Provider::Help,
        Provider::Onboard,
        Provider::OnboardLanguage,
        Provider::OnboardFamily,
        Provider::OnboardModel,
        Provider::OnboardRoute,
        Provider::OnboardWorkspace,
        Provider::OnboardDone,
        Provider::ProfilePicker,
        Provider::ProfileActions,
        Provider::ProfileDeleteConfirm,
        Provider::LaunchPrompt,
        Provider::Login,
        Provider::Theme,
        Provider::Thinking,
        Provider::Lang,
        Provider::StatusLine,
        Provider::Title,
        Provider::Keymap,
        Provider::Status,
        Provider::Cost,
        Provider::CompactConfirm,
        Provider::Resume,
        Provider::Rewind,
        Provider::Model,
        Provider::Llm,
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
    ProfilePicker,
    ProfileActions,
    ProfileDeleteConfirm,
    LaunchPrompt,
    OnboardLanguage,
    OnboardFamily,
    OnboardModel,
    OnboardRoute,
    OnboardWorkspace,
    OnboardDone,
    Login,
    Theme,
    Thinking,
    Lang,
    StatusLine,
    Title,
    Keymap,
    Status,
    Cost,
    CompactConfirm,
    Resume,
    Rewind,
    Model,
    Llm,
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
            Self::ProfilePicker => crate::menu::registry::MENU_PROFILE_PICKER,
            Self::ProfileActions => crate::menu::registry::MENU_PROFILE_ACTIONS,
            Self::ProfileDeleteConfirm => crate::menu::registry::MENU_PROFILE_DELETE_CONFIRM,
            Self::LaunchPrompt => crate::menu::registry::MENU_LAUNCH_PROMPT,
            Self::OnboardLanguage => MENU_ONBOARD_LANGUAGE,
            Self::OnboardFamily => crate::menu::registry::MENU_ONBOARD_FAMILY,
            Self::OnboardModel => crate::menu::registry::MENU_ONBOARD_MODEL,
            Self::OnboardRoute => crate::menu::registry::MENU_ONBOARD_ROUTE,
            Self::OnboardWorkspace => crate::menu::registry::MENU_ONBOARD_WORKSPACE,
            Self::OnboardDone => crate::menu::registry::MENU_ONBOARD_DONE,
            Self::Login => MENU_LOGIN,
            Self::Theme => MENU_THEME,
            Self::Thinking => crate::menu::registry::MENU_THINKING,
            Self::Lang => crate::menu::registry::MENU_LANG,
            Self::StatusLine => MENU_STATUS_LINE,
            Self::Title => MENU_TITLE,
            Self::Keymap => MENU_KEYMAP,
            Self::Status => MENU_STATUS,
            Self::Cost => MENU_COST,
            Self::CompactConfirm => MENU_COMPACT_CONFIRM,
            Self::Resume => MENU_RESUME,
            Self::Rewind => MENU_REWIND,
            Self::Model => MENU_MODEL,
            Self::Llm => MENU_PROVIDER,
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
            Self::ProfilePicker => profile_picker_menu(ctx),
            Self::ProfileActions => profile_actions_menu(ctx),
            Self::ProfileDeleteConfirm => profile_delete_confirm_menu(ctx),
            Self::LaunchPrompt => launch_prompt_menu(ctx),
            Self::OnboardLanguage => MenuBuildResult::Ready(onboarding_language_menu()),
            Self::OnboardFamily => onboarding_family_menu(ctx),
            Self::OnboardModel => onboarding_model_menu(ctx),
            Self::OnboardRoute => onboarding_route_menu(ctx),
            Self::OnboardWorkspace => onboarding_workspace_menu(ctx),
            Self::OnboardDone => onboarding_done_menu(ctx),
            Self::Login => login_menu(ctx),
            Self::Theme => MenuBuildResult::Ready(theme_menu(ctx)),
            Self::Thinking => MenuBuildResult::Ready(thinking_menu(ctx)),
            Self::Lang => MenuBuildResult::Ready(lang_menu(ctx)),
            Self::StatusLine => MenuBuildResult::Ready(status_line_menu(ctx)),
            Self::Title => MenuBuildResult::Ready(title_menu(ctx)),
            Self::Keymap => MenuBuildResult::Ready(keymap_menu()),
            Self::Status => MenuBuildResult::Ready(status_menu(ctx)),
            Self::Cost => cost_menu(ctx),
            Self::CompactConfirm => compact_confirm_menu(ctx),
            Self::Resume => resume_menu(ctx),
            Self::Rewind => rewind_menu(ctx),
            Self::Model => model_menu(ctx),
            Self::Llm => provider_menu(ctx),
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
            // Codex Enter semantics (checked against codex-rs
            // bottom_pane/chat_composer/slash_input.rs): Enter on a highlighted
            // command DISPATCHES it immediately — an argument-less command goes
            // straight to its page/menu/action in one Enter, never the old
            // complete-then-Enter-again round trip. Argful commands complete
            // into the composer WITH a trailing space instead (the user has to
            // type arguments anyway, and the next Enter executes the draft
            // directly via `slash_help_enter_executes`) — codex spends Tab on
            // that affordance; Tab is the inspector toggle here.
            let action = if command.inline_args == crate::menu::types::InlineArgMode::Required {
                // Bare dispatch would only be a usage error — complete with a
                // trailing space so the user types the required argument;
                // the next Enter executes the draft directly.
                MenuAction::Local(LocalAction::EditComposer(format!("/{} ", command.name)))
            } else {
                // None AND Optional: bare dispatch is valid and useful
                // (optional-arg commands open their interactive page, e.g.
                // /lang → language picker) — one Enter, straight there.
                MenuAction::Local(LocalAction::RunSlashCommand(format!("/{}", command.name)))
            };
            let mut description = command_description(command.description, command.aliases);
            if command.name == "scrollmode" {
                // Surface the CURRENT mode so the user knows what a toggle
                // would do before running it.
                let mode = if ctx.app.pinned_scroll {
                    "pinned"
                } else {
                    "native"
                };
                description = format!("{description} {}", t!("scrollmode.current", mode = mode));
            }
            let mut item = MenuItem::new(command.name, command.slash_name(), action)
                .with_description(description);
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

fn available_language_choices() -> Vec<crate::cli::Lang> {
    let mut langs = Vec::new();
    for locale in rust_i18n::available_locales!() {
        if let Some(lang) = crate::cli::Lang::from_env_value(locale) {
            if !langs.contains(&lang) {
                langs.push(lang);
            }
        }
    }
    if langs.is_empty() {
        langs.extend([crate::cli::Lang::En, crate::cli::Lang::Zh]);
    }
    langs.sort_by_key(|lang| match lang {
        crate::cli::Lang::En => 0,
        crate::cli::Lang::Zh => 1,
    });
    langs
}

fn current_language() -> crate::cli::Lang {
    let current = rust_i18n::locale().to_string();
    crate::cli::Lang::from_env_value(&current).unwrap_or(crate::cli::Lang::En)
}

fn language_label(lang: crate::cli::Lang) -> String {
    match lang {
        crate::cli::Lang::En => t!("menu.lang.item.en.label").into_owned(),
        crate::cli::Lang::Zh => t!("menu.lang.item.zh.label").into_owned(),
    }
}

fn language_description(lang: crate::cli::Lang) -> String {
    match lang {
        crate::cli::Lang::En => t!("menu.lang.item.en.desc").into_owned(),
        crate::cli::Lang::Zh => t!("menu.lang.item.zh.desc").into_owned(),
    }
}

fn language_menu_items(id_prefix: &str) -> Vec<MenuItem> {
    let current = current_language();
    available_language_choices()
        .into_iter()
        .enumerate()
        .map(|(idx, lang)| {
            let state = MenuItemState {
                current: lang == current,
                ..MenuItemState::default()
            };
            let mut item = MenuItem::new(
                format!("{id_prefix}.{}", lang.code()),
                language_label(lang),
                MenuAction::Local(LocalAction::SetLanguageCode(lang)),
            )
            .with_description(language_description(lang))
            .with_state(state);
            if let Some(shortcut) = numeric_shortcut(idx) {
                item = item.with_shortcut(shortcut);
            }
            item
        })
        .collect()
}

fn lang_menu(_ctx: &MenuContext<'_>) -> MenuSpec {
    let items = language_menu_items("lang");

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

fn onboarding_language_menu() -> MenuSpec {
    let progress = crate::menu::wizard::WizardProgress {
        current: crate::menu::wizard::WizardStep::Language,
        done: [false; crate::menu::wizard::WizardStep::ALL.len()],
    };
    MenuSpec {
        id: MenuId::from(MENU_ONBOARD_LANGUAGE),
        title: t!("onboarding.language.title").into_owned(),
        subtitle: Some(progress.subtitle()),
        items: language_menu_items("onboard.language"),
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("onboarding.language.footer").into_owned()),
        preview: Some(progress.explanation_preview()),
        mode: MenuMode::SingleSelect,
    }
}

fn onboarding_language_row() -> MenuItem {
    MenuItem::new(
        "onboard.language",
        format!(
            "{}: {}",
            t!("onboarding.language.label"),
            language_label(current_language())
        ),
        MenuAction::OpenMenu(MenuId::from(MENU_ONBOARD_LANGUAGE)),
    )
    .with_description(t!("onboarding.language.description"))
    .with_state(MenuItemState::required(true))
}

fn thinking_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    use octos_core::ui_protocol::ReasoningEffortLevel as L;
    let current = ctx.app.reasoning_effort;
    let mut items = [
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
        let state = MenuItemState {
            current: level == current,
            ..MenuItemState::default()
        };
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
    .collect::<Vec<_>>();

    // A non-interactive divider separates the radio effort levels above from
    // the display toggle below — different axes (how hard vs whether shown).
    items.push(
        MenuItem::new("", t!("menu.thinking.divider.display"), MenuAction::Noop).with_state(
            MenuItemState {
                non_selectable: true,
                ..MenuItemState::default()
            },
        ),
    );
    // Display toggle — orthogonal to the effort levels: whether the committed
    // reasoning renders as a transcript block for this session. Rendered as a
    // checkbox (`[x]`/`[ ]`), NOT the radio `*`, so it reads as a toggle rather
    // than a 6th level.
    let display_on = ctx.app.reasoning_display;
    items.push(
        MenuItem::new(
            "reasoning_display",
            t!("menu.thinking.item.display.label"),
            MenuAction::Local(LocalAction::ToggleReasoningDisplay),
        )
        .with_description(t!("menu.thinking.item.display.desc"))
        .with_state(MenuItemState {
            checked: Some(display_on),
            ..MenuItemState::default()
        }),
    );

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
        let state = MenuItemState {
            current: id == current,
            ..MenuItemState::default()
        };
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
        // Honest footer: the checkboxes are a read-only preview of the
        // build-time layout — no Space toggle / reorder handling is wired and
        // Enter reports "not wired" — so promise only navigation (plain
        // English, no i18n key; the localized `menu.component.footer` still
        // advertises the unwired Space toggle).
        footer_hint: Some(
            "Up/Down move | Esc close — read-only preview, save not wired yet".into(),
        ),
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
                    MenuAction::send_appui(AppUiCommand::ReadSessionStatus(
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
            MenuAction::send_appui(AppUiCommand::ReadSessionStatus(SessionStatusReadParams {
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

/// A 2-item Yes/No confirm for `/compact`. "Yes" sends `session/compact`
/// (force-compact the current session); "No" closes the menu. Modeled on
/// [`cost_menu`]; gated on the server advertising `session/compact` and on a
/// session being selected.
fn compact_confirm_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(session_id) = ctx.app.selected_session_id.cloned() else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_COMPACT_CONFIRM),
            title: t!("menu.compact.unavailable_title").into_owned(),
            message: t!("menu.compact.unavailable_no_session").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    };

    if !ctx
        .availability
        .supports_method(APPUI_METHOD_SESSION_COMPACT)
    {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_COMPACT_CONFIRM),
            title: t!("menu.compact.unavailable_title").into_owned(),
            message: method_missing_reason(ctx, APPUI_METHOD_SESSION_COMPACT),
            footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        });
    }

    let items = vec![
        MenuItem::new(
            "compact.confirm",
            t!("menu.compact.item.confirm.label"),
            MenuAction::send_appui(AppUiCommand::CompactContext(SessionCompactParams {
                session_id,
            })),
        )
        .with_description(t!("menu.compact.item.confirm.desc").into_owned()),
        MenuItem::new(
            "compact.cancel",
            t!("menu.compact.item.cancel.label"),
            MenuAction::Close,
        ),
    ];

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_COMPACT_CONFIRM),
        title: t!("menu.compact.title").into_owned(),
        subtitle: Some(t!("menu.compact.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.esc_close").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

/// `/resume` session picker. Renders `Loading` until the `session/list` result
/// lands (see `Store::apply_session_list_result` refreshing the open menu),
/// then one selectable row per prior session — picking a row switches to it and
/// hydrates its transcript. Modeled on `cost_menu`'s fetch-then-refresh async
/// pattern; strings are plain English (no new i18n keys), mirroring `/copy`.
fn resume_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if ctx.app.resume_sessions.is_empty() {
        // Distinguish "the fetch already returned zero sessions" from "the fetch
        // is still in flight". Only the latter renders `Loading`; a completed
        // fetch with no sessions renders a terminal placeholder instead of
        // spinning forever (`resume_list_loaded` flips true when a `session/list`
        // result is applied — see `Store::apply_session_list_result`).
        if ctx.app.resume_list_loaded {
            return MenuBuildResult::Unavailable(MenuStatusSpec {
                id: MenuId::from(MENU_RESUME),
                title: "Resume a session".into(),
                message: "No prior sessions to resume".into(),
                footer_hint: Some("Esc to close".into()),
            });
        }
        return MenuBuildResult::Loading(MenuStatusSpec {
            id: MenuId::from(MENU_RESUME),
            title: "Resume a session".into(),
            message: "Loading sessions…".into(),
            footer_hint: Some("Esc to close".into()),
        });
    }

    let items = ctx
        .app
        .resume_sessions
        .iter()
        .map(|row| {
            // Label: `{short_id}  {prompt}` — the short id doubles as the
            // `/resume <id>` prefix handle; the prompt prefers the last user
            // message, then the title, then a placeholder.
            let short_id = short_session_id(&row.id);
            let prompt = row
                .last_prompt
                .as_deref()
                .filter(|prompt| !prompt.trim().is_empty())
                .or_else(|| {
                    row.title
                        .as_deref()
                        .filter(|title| !title.trim().is_empty())
                })
                .unwrap_or("(no preview)");
            let label = format!("{short_id}  {}", truncate_display_width(prompt, 60));
            // Description: relative datetime (when the server sent one) + count.
            let description = match row.updated_at.as_deref() {
                Some(updated) if !updated.is_empty() => {
                    format!(
                        "{} · {} msgs",
                        crate::store::relative_time(updated),
                        row.message_count
                    )
                }
                _ => format!("{} msgs", row.message_count),
            };
            MenuItem::new(
                row.id.clone(),
                label,
                MenuAction::Local(LocalAction::ResumeSession(row.id.clone())),
            )
            .with_description(description)
        })
        .collect();

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_RESUME),
        title: "Resume a session".into(),
        subtitle: Some("Switch to a prior session and reload its transcript.".into()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Search sessions…".into()),
        footer_hint: Some("Enter resume · /resume <id> · Esc".into()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

/// A short, human-meaningful, usually-unique handle for a session id of the
/// canonical `channel:profile:base#topic` shape. The topic (after `#`) is what
/// users recognize and is unique per base — a far better `/resume <id>` handle
/// than a fixed 6-char prefix, which collides for every id sharing a namespace
/// prefix (`dev:local:tui#a` / `#b` both → `dev:lo`, codex P2). Falls back to
/// the base segment (after the last `:`), then the whole id. `resolve_resume_
/// session` matches this handle via an exact-topic step.
fn short_session_id(id: &str) -> String {
    if let Some((_, topic)) = id.rsplit_once('#')
        && !topic.is_empty()
    {
        return topic.to_string();
    }
    if let Some((_, base)) = id.rsplit_once(':')
        && !base.is_empty()
    {
        return base.to_string();
    }
    id.to_string()
}

/// Truncate `text` to at most `max_cols` display columns (unicode-width aware,
/// so CJK/emoji don't overrun the row), collapsing to the first non-blank line
/// and appending `…` when it overflows.
fn truncate_display_width(text: &str, max_cols: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut out = String::new();
    let mut width = 0usize;
    for ch in line.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_cols {
            out.push('…');
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out
}

/// `/rewind` turn picker. Unlike `/resume` this needs no async fetch — the
/// active session's user turns are already in the local transcript, snapshotted
/// into `rewind_turns` when the picker opens. Empty → `Unavailable` (nothing to
/// rewind to); otherwise one selectable row per user turn (newest-first), and
/// picking a row drops the later turns via `session/rollback` and puts that
/// message back in the composer to edit and resend. Strings are plain English
/// (no new i18n keys), mirroring `/resume`.
fn rewind_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    if ctx.app.rewind_turns.is_empty() {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_REWIND),
            title: "Rewind the conversation".into(),
            message: "Nothing to rewind to in this session".into(),
            footer_hint: Some("Esc to close".into()),
        });
    }

    // Bind every row to the session the rows were built from: the dispatch
    // side refuses a pick whose session no longer matches the active one (the
    // user switched sessions while the picker was open).
    let session_id = ctx
        .app
        .selected_session_id
        .map(|key| key.0.clone())
        .unwrap_or_default();
    let items = ctx
        .app
        .rewind_turns
        .iter()
        .map(|row| {
            // Label: `#{checkpoint}  {preview}`; description: relative datetime
            // (when the source message carried one) + the explicit drop count.
            let label = format!("#{}  {}", row.checkpoint, row.preview);
            let description = match row.timestamp.as_deref() {
                Some(timestamp) if !timestamp.is_empty() => {
                    format!(
                        "{} · drops {} turn(s)",
                        crate::store::relative_time(timestamp),
                        row.num_turns
                    )
                }
                _ => format!("drops {} turn(s)", row.num_turns),
            };
            MenuItem::new(
                format!("rewind:{}", row.num_turns),
                label,
                MenuAction::Local(LocalAction::RewindToTurn {
                    session_id: session_id.clone(),
                    num_turns: row.num_turns,
                    prefill: row.prefill.clone(),
                }),
            )
            .with_description(description)
        })
        .collect();

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_REWIND),
        title: "Rewind the conversation".into(),
        subtitle: Some("Go back to an earlier message to edit and resend it.".into()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Search messages…".into()),
        footer_hint: Some("Enter rewind · /rewind <n> · Esc".into()),
        preview: None,
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
    // "Create a new profile" forces the create step even mid-session, where the
    // active session's profile would otherwise route the wizard to provider
    // setup. (Cleared once the profile is created, so the wizard then advances
    // to setting up the new profile's model.)
    let force_create = state.creating_new_profile && local_profile_create;
    if force_create
        || (local_profile_create && state.effective_profile_id(current_profile).is_none())
    {
        return onboarding_local_profile_menu(
            state,
            local_profile_requested_id_supported(ctx),
            local_profile_make_default_supported(ctx),
        );
    }
    if state.effective_profile_id(current_profile).is_some() {
        return onboarding_provider_setup_menu(ctx, state, current_profile);
    }

    let mut items = if local_profile_create {
        vec![
            onboarding_language_row(),
            MenuItem::new(
                "onboard.local.status",
                onboarding_local_profile_label(state),
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_status.desc")),
            MenuItem::new(
                "onboard.local.name",
                if state.has_name() {
                    format!("{}: {}", t!("onboarding.field.full_name"), state.name)
                } else {
                    format!(
                        "{}: {}",
                        t!("onboarding.field.full_name"),
                        t!("onboarding.value_not_set")
                    )
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_name.desc"))
            .with_state(MenuItemState::required(state.has_name())),
            MenuItem::new(
                "onboard.local.username",
                if state.has_username() {
                    format!("{}: {}", t!("onboarding.field.username"), state.username)
                } else {
                    format!(
                        "{}: {}",
                        t!("onboarding.field.username"),
                        t!("onboarding.value_not_set")
                    )
                },
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.local_username.desc"))
            .with_state(MenuItemState::required(state.has_username())),
            MenuItem::new(
                "onboard.local.email",
                if state.has_email() {
                    format!("{}: {}", t!("onboarding.field.email"), state.email)
                } else {
                    format!(
                        "{}: {}",
                        t!("onboarding.field.email"),
                        t!("onboarding.value_not_set")
                    )
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
            onboarding_language_row(),
            MenuItem::new(
                "onboard.status.auth",
                onboarding_auth_label(state),
                MenuAction::Noop,
            )
            .with_description(t!("menu.onboard.item.auth_status.desc")),
            MenuItem::new(
                "onboard.auth.status",
                t!("menu.onboard.item.auth_refresh.label"),
                MenuAction::send_appui(AppUiCommand::AuthStatus(AuthStatusParams::default())),
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
                MenuAction::send_appui(AppUiCommand::AuthMe(AuthMeParams {
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
            format!(
                "{}: {}",
                t!("menu.onboard.item.provider_current.label"),
                state.provider_label()
            ),
            MenuAction::Noop,
        )
        .with_description(t!("menu.onboard.item.provider_current.desc"))
        .with_state(MenuItemState::required(state.selection_ready())),
        MenuItem::new(
            "onboard.provider.key",
            if state.has_api_key() {
                format!(
                    "{}: {}",
                    t!("menu.onboard.item.api_key.label"),
                    state.api_key_label()
                )
            } else {
                format!(
                    "{}: {}",
                    t!("menu.onboard.item.api_key.label"),
                    t!("onboarding.value_not_set")
                )
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

/// Phase 3 startup picker: "attach which profile?". Lists the local profiles
/// discovered at launch; selecting one attaches it (via `SetProfileId`, the
/// same path `/onboard profile <id>` uses) and the wizard advances straight to
/// provider setup for that profile. A trailing row starts a fresh profile
/// through the normal onboarding create step. Only reached when more than one
/// profile exists and no `--profile-id` was pinned (see
/// `maybe_open_onboarding_on_first_launch`).
fn profile_picker_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let onboarding = ctx.app.onboarding;
    let profiles = onboarding
        .map(|onboarding| onboarding.available_profiles.as_slice())
        .unwrap_or(&[]);
    let default = onboarding.and_then(|onboarding| onboarding.default_profile.as_deref());

    let mut items: Vec<MenuItem> = profiles
        .iter()
        .enumerate()
        .map(|(index, profile)| {
            // Mark the machine default with a trailing `*default`.
            let label = if default == Some(profile.as_str()) {
                format!("{profile}  {}", t!("menu.profiles.default_marker"))
            } else {
                profile.clone()
            };
            // Selecting a profile drills into its per-profile action menu (info
            // in the right pane + set-default / delete); "use it" is a row there.
            let mut item = MenuItem::new(
                format!("profile.pick.{index}"),
                label,
                MenuAction::Local(LocalAction::SelectProfileForActions(profile.clone())),
            )
            .with_description(t!("menu.profiles.item.manage.desc"));
            if let Some(shortcut) = numeric_shortcut(index) {
                item = item.with_shortcut(shortcut);
            }
            item
        })
        .collect();

    items.push(
        MenuItem::new(
            "profile.pick.new",
            t!("menu.profile_picker.item.create.label"),
            // Reset the wizard to a clean slate, then open the create step
            // (Name-this-profile) — so it starts FRESH rather than resuming the
            // active profile's already-configured setup.
            MenuAction::Local(LocalAction::CreateNewProfile),
        )
        .with_description(t!("menu.profile_picker.item.create.desc")),
    );
    items.push(
        MenuItem::new(
            "profile.pick.exit",
            t!("menu.onboard.item.exit.label"),
            MenuAction::Local(LocalAction::Exit),
        )
        .with_description(t!("menu.onboard.item.exit.desc")),
    );

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_PROFILE_PICKER),
        title: t!("menu.profiles.title").into_owned(),
        subtitle: Some(t!("menu.profiles.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: profiles.len() > 8,
        search_placeholder: Some(t!("menu.profile_picker.search").into_owned()),
        footer_hint: Some(t!("menu.profiles.footer").into_owned()),
        preview: Some(MenuPreview::Text {
            title: Some(t!("menu.profiles.preview_title").into_owned()),
            body: t!("menu.profiles.preview_hint").into_owned(),
        }),
        mode: MenuMode::SingleSelect,
    })
}

/// Per-profile action drill-in: shows the selected profile's info in the right
/// pane and offers Use / Set-default / Delete. Reached from the profiles list.
fn profile_actions_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let onboarding = ctx.app.onboarding;
    let Some(profile) = onboarding.and_then(|onboarding| onboarding.selected_profile.clone())
    else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_PROFILE_ACTIONS),
            title: t!("menu.profiles.actions.title").into_owned(),
            message: t!("menu.profiles.actions.none").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    };
    let is_default =
        onboarding.and_then(|o| o.default_profile.as_deref()) == Some(profile.as_str());

    let mut items = vec![
        MenuItem::new(
            "profile.action.use",
            t!("menu.profiles.actions.use"),
            MenuAction::Local(LocalAction::SwitchToProfile(profile.clone())),
        )
        .with_description(t!("menu.profiles.actions.use_desc")),
    ];
    let set_default = MenuItem::new(
        "profile.action.default",
        t!("menu.profiles.actions.set_default"),
        MenuAction::Local(LocalAction::SetProfileDefault(profile.clone())),
    )
    .with_description(t!("menu.profiles.actions.set_default_desc"));
    items.push(if is_default {
        set_default.maybe_disabled(Some(
            t!("menu.profiles.actions.already_default").into_owned(),
        ))
    } else {
        set_default
    });
    items.push(
        MenuItem::new(
            "profile.action.delete",
            t!("menu.profiles.actions.delete"),
            MenuAction::Local(LocalAction::RequestDeleteProfile(profile.clone())),
        )
        .with_description(t!("menu.profiles.actions.delete_desc")),
    );
    items.push(MenuItem::new(
        "profile.action.back",
        t!("menu.profiles.actions.back"),
        MenuAction::Close,
    ));

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_PROFILE_ACTIONS),
        title: t!(
            "menu.profiles.actions.title_named",
            profile = profile.clone()
        )
        .into_owned(),
        subtitle: Some(t!("menu.profiles.actions.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        preview: Some(profile_info_preview(onboarding, &profile, is_default)),
        mode: MenuMode::SingleSelect,
    })
}

/// Right-pane info for a profile: its model summary + default status.
fn profile_info_preview(
    onboarding: Option<&OnboardingWizardState>,
    profile: &str,
    is_default: bool,
) -> MenuPreview {
    let mut body = t!("menu.profiles.info.name", profile = profile.to_string()).into_owned();
    let model = onboarding
        .and_then(|o| o.profiles_data_dir.as_deref())
        .and_then(|dir| {
            crate::profiles::profile_llm_summary(
                &std::path::Path::new(dir).join("profiles"),
                profile,
            )
        });
    body.push('\n');
    match model {
        Some(model) => body.push_str(&t!("menu.profiles.info.model", model = model)),
        None => body.push_str(&t!("menu.profiles.info.no_model")),
    }
    body.push('\n');
    body.push_str(&if is_default {
        t!("menu.profiles.info.is_default")
    } else {
        t!("menu.profiles.info.not_default")
    });
    MenuPreview::Text {
        title: Some(t!("menu.profiles.info.title").into_owned()),
        body,
    }
}

/// Yes/No confirm for deleting the selected profile (destructive).
fn profile_delete_confirm_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(profile) = ctx
        .app
        .onboarding
        .and_then(|onboarding| onboarding.selected_profile.clone())
    else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_PROFILE_DELETE_CONFIRM),
            title: t!("menu.profiles.delete.title").into_owned(),
            message: t!("menu.profiles.actions.none").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        });
    };
    let items = vec![
        MenuItem::new(
            "profile.delete.yes",
            t!("menu.profiles.delete.yes", profile = profile.clone()),
            MenuAction::Local(LocalAction::ConfirmDeleteProfile(profile.clone())),
        )
        .with_description(t!("menu.profiles.delete.yes_desc")),
        MenuItem::new(
            "profile.delete.no",
            t!("menu.profiles.delete.no"),
            MenuAction::Close,
        ),
    ];
    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_PROFILE_DELETE_CONFIRM),
        title: t!(
            "menu.profiles.delete.title_named",
            profile = profile.clone()
        )
        .into_owned(),
        subtitle: Some(t!("menu.profiles.delete.subtitle").into_owned()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

/// Per-project launch prompt (Model A). Renders the Activate / CrossProfile
/// choice raised from a `launch/resolve` decision: Activate confirms opening the
/// resolved brain in an as-yet-unused folder; CrossProfile offers to start the
/// launching brain here or switch to one already used in this folder. Every
/// choice sends `session/open` carrying this folder's cwd so the session lands
/// in the folder's per-project store. Renders Unavailable if no prompt is
/// staged (defensive — the store only opens this menu with one set).
fn launch_prompt_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(prompt) = ctx
        .app
        .onboarding
        .and_then(|onboarding| onboarding.launch_prompt.as_ref())
    else {
        return MenuBuildResult::Unavailable(MenuStatusSpec::new(
            MenuId::from(crate::menu::registry::MENU_LAUNCH_PROMPT),
            t!("menu.launch_prompt.activate.title").into_owned(),
            t!("menu.launch_prompt.unavailable").into_owned(),
        ));
    };

    let open_session = |profile: &str| -> MenuAction {
        let session_id =
            octos_core::SessionKey::with_profile_topic(profile, "local", "tui", "coding");
        MenuAction::send_appui(AppUiCommand::OpenSession(
            octos_core::ui_protocol::SessionOpenParams {
                session_id,
                topic: None,
                profile_id: Some(profile.to_owned()),
                cwd: Some(prompt.cwd.clone()),
                sandbox: None,
                after: None,
            },
        ))
    };

    let (title, subtitle, mut items) = match prompt.decision {
        crate::model::LaunchDecisionKind::Activate => {
            let mut activate = MenuItem::new(
                "launch.activate",
                t!(
                    "menu.launch_prompt.activate.item.activate.label",
                    profile = prompt.resolved_profile.clone()
                ),
                open_session(&prompt.resolved_profile),
            )
            .with_description(t!(
                "menu.launch_prompt.activate.item.activate.desc",
                profile = prompt.resolved_profile.clone()
            ));
            if let Some(shortcut) = numeric_shortcut(0) {
                activate = activate.with_shortcut(shortcut);
            }
            (
                t!("menu.launch_prompt.activate.title").into_owned(),
                t!(
                    "menu.launch_prompt.activate.subtitle",
                    cwd = prompt.cwd.clone()
                )
                .into_owned(),
                vec![activate],
            )
        }
        // CrossProfile (and any non-Activate decision that reached the prompt):
        // "start the launching brain here" first, then one switch row per
        // profile already used in this folder.
        _ => {
            let mut items = vec![
                MenuItem::new(
                    "launch.start",
                    t!(
                        "menu.launch_prompt.cross.item.start.label",
                        profile = prompt.resolved_profile.clone()
                    ),
                    open_session(&prompt.resolved_profile),
                )
                .with_description(t!(
                    "menu.launch_prompt.cross.item.start.desc",
                    profile = prompt.resolved_profile.clone()
                )),
            ];
            for (index, existing) in prompt.existing_profiles.iter().enumerate() {
                let mut item = MenuItem::new(
                    format!("launch.switch.{index}"),
                    t!(
                        "menu.launch_prompt.cross.item.switch.label",
                        profile = existing.clone()
                    ),
                    open_session(existing),
                )
                .with_description(t!(
                    "menu.launch_prompt.cross.item.switch.desc",
                    profile = existing.clone()
                ));
                // Reserve shortcut 1 for "start here"; switch rows follow.
                if let Some(shortcut) = numeric_shortcut(index + 1) {
                    item = item.with_shortcut(shortcut);
                }
                items.push(item);
            }
            (
                t!("menu.launch_prompt.cross.title").into_owned(),
                t!(
                    "menu.launch_prompt.cross.subtitle",
                    cwd = prompt.cwd.clone()
                )
                .into_owned(),
                items,
            )
        }
    };

    items.push(
        MenuItem::new(
            "launch.cancel",
            t!("menu.launch_prompt.item.cancel.label"),
            MenuAction::Local(LocalAction::Exit),
        )
        .with_description(t!("menu.launch_prompt.item.cancel.desc")),
    );

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_LAUNCH_PROMPT),
        title,
        subtitle: Some(subtitle),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.launch_prompt.footer").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

/// Terminal onboarding screen on a launch-flow server (Model A). The profile and
/// its LLM provider are already set up, so onboarding ends here with launch
/// instructions instead of staging a workspace or opening a session —
/// launch-time activation (`launch/resolve`) opens the session on the next
/// start. Renders an Exit row to leave the wizard. Reached only when
/// [`launch_flow_supported`] (older servers keep the workspace/Activate step).
fn onboarding_done_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let profile = ctx
        .app
        .onboarding
        .and_then(|onboarding| onboarding.effective_profile_id(ctx.app.current_profile))
        .unwrap_or_default();
    let subtitle = if profile.is_empty() {
        t!("menu.onboard_done.subtitle_generic").into_owned()
    } else {
        t!("menu.onboard_done.subtitle", profile = profile).into_owned()
    };
    // Name the concrete command to start a session with this profile, rather
    // than a vague "relaunch Octos here" (user feedback).
    let (ready_label, ready_desc) = if profile.is_empty() {
        (
            t!("menu.onboard_done.item.ready.label_generic").into_owned(),
            t!("menu.onboard_done.item.ready.desc_generic").into_owned(),
        )
    } else {
        (
            t!("menu.onboard_done.item.ready.label", profile = &profile).into_owned(),
            t!("menu.onboard_done.item.ready.desc", profile = &profile).into_owned(),
        )
    };
    let items = vec![
        MenuItem::new("onboard.done.status", ready_label, MenuAction::Noop)
            .with_description(ready_desc)
            // A read-only "what's next" instruction, not an action: mark it
            // non-selectable so the cursor skips it (only Close acts here).
            .with_state(MenuItemState {
                non_selectable: true,
                ..MenuItemState::default()
            }),
        MenuItem::new(
            "onboard.done.exit",
            t!("menu.onboard_done.item.exit.label"),
            MenuAction::Local(LocalAction::Exit),
        )
        .with_description(t!("menu.onboard_done.item.exit.desc")),
    ];
    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_ONBOARD_DONE),
        title: t!("menu.onboard_done.title").into_owned(),
        subtitle: Some(subtitle),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(t!("menu.onboard_done.footer").into_owned()),
        preview: None,
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_provider_setup_menu(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> MenuBuildResult {
    // UX2 feedback: read-only status rows (Profile / Selected provider / Saved
    // provider) are `Noop` — the user can't act on them by selecting — so they
    // move to the right info pane (`onboarding_provider_preview`) and the left
    // list holds only the actionable provider-config rows.
    let saved_primary = onboarding_saved_primary(ctx, state, current_profile);

    // Profile↔model decoupling (user feedback: "collapse to one Add model").
    // The detailed model config stays behind a single "Add a model" entry. It
    // expands to the family/model/route/key/save rows ONLY while the user is
    // actively setting up a model — i.e. a staged selection that is NOT yet the
    // saved primary. A profile whose provider is already saved (or freshly
    // saved, or resumed) collapses back to "Add another model" + Finish, rather
    // than dumping the raw form (which reads as "no Add-a-model option").
    let has_staged = !state.provider.family_id.trim().is_empty();
    // The staged selection has already been saved as this profile's primary
    // when EITHER: the session just saved this exact selection (its label
    // matches `saved_primary_provider_label`, set only on a primary save and
    // never reset by re-staging), OR the server reports a matching saved
    // primary (a resumed/hydrated profile). Comparing labels/ids — not just a
    // "was anything ever saved" flag — is what lets staging a DIFFERENT model
    // still expand (add another / fallback).
    let staged_label = state.provider_label();
    let staged_is_saved_primary = has_staged
        && (state.saved_primary_provider_label.as_deref() == Some(staged_label.as_str())
            || saved_primary.is_some_and(|saved| {
                saved.family_id.as_deref() == Some(state.provider.family_id.trim())
                    && saved.model_id.as_deref() == Some(state.provider.model_id.trim())
            }));
    let configuring = has_staged && !staged_is_saved_primary;

    let mut items: Vec<MenuItem> = Vec::new();
    if !configuring {
        // "Add another model" once a primary exists (you can add a fallback or
        // replace it); plain "Add a model" on a fresh profile.
        let (label, desc) = if saved_primary.is_some() {
            (
                t!("onboarding.provider.add_another_model_label"),
                t!("onboarding.provider.add_another_model_desc"),
            )
        } else {
            (
                t!("onboarding.provider.add_model_label"),
                t!("onboarding.provider.add_model_desc"),
            )
        };
        items.push(
            MenuItem::new(
                "onboard.provider.add_model",
                label,
                MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY)),
            )
            .with_description(desc)
            .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_CATALOG)),
        );
    }

    if configuring {
        items.push(
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
        );

        items.extend([
            MenuItem::new(
                "onboard.provider.family",
                format!(
                    "{}: {}",
                    t!("menu.onboard.item.family.label"),
                    onboarding_family_label(state, saved_primary)
                ),
                MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY)),
            )
            .with_description(t!("menu.onboard.item.family.desc"))
            .with_state(MenuItemState::required(
                !state.provider.family_id.trim().is_empty(),
            )),
            MenuItem::new(
                "onboard.provider.model",
                format!(
                    "{}: {}",
                    t!("menu.onboard.item.model.label"),
                    onboarding_model_label(state, saved_primary)
                ),
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
                    .then(|| t!("onboarding.disabled.choose_family_first").into_owned())
            }),
            MenuItem::new(
                "onboard.provider.route",
                format!(
                    "{}: {}",
                    t!("menu.onboard.item.route.label"),
                    onboarding_route_label(state, saved_primary)
                ),
                MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_ROUTE)),
            )
            .with_description(t!("menu.onboard.item.route.desc"))
            .with_state(MenuItemState::required(
                !state.provider.route.route_id.trim().is_empty(),
            ))
            .maybe_disabled(
                (!onboarding_model_selected(state))
                    .then(|| t!("onboarding.disabled.choose_family_model_first").into_owned()),
            ),
        ]);

        // Draft-first, saved-fallback for the API key row: a key already saved in
        // the profile (server-confirmed `has_api_key`) must not read as "not set".
        let api_key_display = if state.has_api_key() {
            Some(state.api_key_label().to_string())
        } else if saved_primary.is_some_and(|provider| provider.has_api_key) {
            Some(t!("onboarding.api_key_saved").into_owned())
        } else {
            None
        };
        let api_key_present = api_key_display.is_some();

        items.extend([
            onboarding_edit_item(
                "onboard.provider.key",
                t!("menu.onboard.item.api_key.label").as_ref(),
                api_key_display.as_deref(),
                "/onboard key ",
            )
            .with_state(MenuItemState::required(api_key_present))
            .maybe_disabled(
                (!state.selection_ready())
                    .then(|| t!("onboarding.disabled.choose_provider_first").into_owned()),
            ),
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
        ]);
        // NOTE: onboarding no longer surfaces "Add as fallback" — first-run is
        // about getting ONE model working, and the fallback concept confused
        // users here. Fallbacks remain available post-onboarding via
        // `/add-model` (the MENU_PROVIDER surface).
    }

    // Terminal step (always shown — collapsed or expanded). On a launch-flow
    // server (Model A) the provider step ends at the launch-instructions screen
    // (`MENU_ONBOARD_DONE`) — the redundant workspace/Activate screen is skipped
    // and launch-time activation opens the session on the next start. Older
    // servers keep the workspace step (`MENU_ONBOARD_WORKSPACE`), which owns the
    // final Activate. Either way it is disabled until a provider is saved so the
    // steps stay ordered.
    items.push({
        let blocked = (!onboarding_has_saved_primary_provider(ctx, state, current_profile))
            .then(|| t!("onboarding.wizard.workspace_locked_reason").into_owned());
        if launch_flow_supported(ctx) {
            MenuItem::new(
                "onboard.done.open",
                t!("onboarding.wizard.finish_label"),
                MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_DONE)),
            )
            .with_description(t!("onboarding.wizard.finish_description"))
            .maybe_disabled(blocked)
        } else {
            MenuItem::new(
                "onboard.workspace.open",
                t!("onboarding.wizard.workspace_open_label"),
                MenuAction::OpenMenu(MenuId::from(crate::menu::registry::MENU_ONBOARD_WORKSPACE)),
            )
            .with_description(t!("onboarding.wizard.workspace_open_description"))
            .with_state(MenuItemState::required(
                state.workspace_validation.is_valid(),
            ))
            .maybe_disabled(blocked)
        }
    });

    // Same escape hatch as the create-profile step: this menu also lives under
    // the root MENU_ONBOARD id, where Esc is swallowed while no session is
    // open — without a visible Exit row the user would be trapped here.
    items.push(
        MenuItem::new(
            "onboard.local.exit",
            t!("menu.onboard.item.exit.label"),
            MenuAction::Local(LocalAction::Exit),
        )
        .with_description(t!("menu.onboard.item.exit.desc")),
    );

    for (idx, item) in items.iter_mut().enumerate() {
        if let Some(shortcut) = numeric_shortcut(idx) {
            item.shortcut = Some(shortcut);
        }
    }

    // Wizard framing: compute the coarse step (Provider → Connect → Save →
    // Workspace → Activate) so the subtitle, footer, and right-side teaching
    // panel all stay in lock-step with the granular rows above.
    let progress = crate::menu::wizard::WizardProgress::from_state(
        state,
        current_profile,
        local_profile_create_supported(ctx),
        onboarding_saved_guidance_ready(ctx, state, current_profile),
    );
    let next_action = onboarding_next_action_hint(ctx, state, current_profile);

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_ONBOARD),
        title: t!("onboarding.wizard.setup_title").into_owned(),
        subtitle: Some(progress.subtitle()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some(t!("menu.onboard.search").into_owned()),
        footer_hint: Some(progress.footer_hint(&next_action)),
        preview: Some(onboarding_provider_preview(
            &progress,
            state,
            current_profile,
        )),
        mode: MenuMode::SingleSelect,
    })
}

/// Right-pane preview for the Provider (LLM config) step. Like the Workspace
/// step, it surfaces the read-only status the user can't act on by selecting —
/// the local profile, the currently-selected provider route, and the last
/// saved provider — so the left list holds only the actionable config rows.
fn onboarding_provider_preview(
    progress: &crate::menu::wizard::WizardProgress,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> MenuPreview {
    let mut preview = progress.explanation_preview();
    if let MenuPreview::Text { body, .. } = &mut preview {
        body.push_str("\n\n");
        body.push_str(&t!("onboarding.preview.provider.configured_title"));
        body.push_str(&format!(
            "\n• {}: {}",
            t!("onboarding.preview.provider.profile"),
            state.profile_label(current_profile)
        ));
        body.push_str(&format!(
            "\n• {}: {}",
            t!("onboarding.preview.provider.selected"),
            state.provider_label()
        ));
        // `onboarding_provider_saved_status_label` already carries its own prefix.
        body.push_str(&format!(
            "\n• {}",
            onboarding_provider_saved_status_label(state)
        ));
    }
    preview
}

/// UX2 B.2: the WORKSPACE step screen, split out of the provider-setup menu so
/// LLM provider/model config and workspace staging+validation live on separate
/// screens. This screen owns the workspace candidate display, validation
/// status, the explicit re-validate action, the staged permission profile, and
/// the final ACTIVATE action (open the coding session). Activate gating is
/// unchanged — it still requires a saved provider AND a `Valid` workspace via
/// `onboarding_open_session_disabled_reason`.
fn onboarding_workspace_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let default_state;
    let state = if let Some(state) = ctx.app.onboarding {
        state
    } else {
        default_state = OnboardingWizardState::default();
        &default_state
    };
    let current_profile = ctx.app.current_profile;

    // UX2 feedback: the left list holds ONLY rows the user can act on by
    // selecting them — Validate and Activate. The read-only staged items
    // (workspace path, validation status, permission profile) are `Noop` (set
    // via slash commands, not selectable), so they moved to the right info
    // pane (see `onboarding_workspace_preview`) instead of cluttering the list
    // with un-selectable rows.
    let mut items = vec![
        MenuItem::new(
            "onboard.workspace.validate",
            t!("menu.onboard.item.workspace_validate.label"),
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::ValidateWorkspace)),
        )
        .with_description(t!("menu.onboard.item.workspace_validate.desc")),
        // The final ACTIVATE step: after model config + test + save succeed and
        // the workspace validates, this is the one explicit action that opens
        // the coding session and drops the user into the working surface.
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
    ];

    for (idx, item) in items.iter_mut().enumerate() {
        if let Some(shortcut) = numeric_shortcut(idx) {
            item.shortcut = Some(shortcut);
        }
    }

    let progress = crate::menu::wizard::WizardProgress::from_state(
        state,
        current_profile,
        local_profile_create_supported(ctx),
        onboarding_saved_guidance_ready(ctx, state, current_profile),
    );
    let next_action = onboarding_next_action_hint(ctx, state, current_profile);

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(crate::menu::registry::MENU_ONBOARD_WORKSPACE),
        title: t!("onboarding.wizard.workspace_title").into_owned(),
        subtitle: Some(progress.subtitle()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some(progress.footer_hint(&next_action)),
        preview: Some(onboarding_workspace_preview(
            &progress,
            state,
            ctx.app.cwd.unwrap_or(""),
        )),
        mode: MenuMode::SingleSelect,
    })
}

/// Right-pane preview for the Workspace step. Beyond the standard step
/// explanation it lists the read-only "staged" items the user can't select on
/// this screen — the workspace path, its validation status, and the staged
/// permission profile (all set via slash commands, not by selecting a row) —
/// plus how to change each. This keeps the LEFT list to only the actions the
/// user can actually take here (Validate, Activate).
fn onboarding_workspace_preview(
    progress: &crate::menu::wizard::WizardProgress,
    state: &OnboardingWizardState,
    active_workspace: &str,
) -> MenuPreview {
    let mut preview = progress.explanation_preview();
    if let MenuPreview::Text { body, .. } = &mut preview {
        body.push_str("\n\n");
        body.push_str(&t!("onboarding.preview.workspace.staged_title"));
        body.push_str(&format!(
            "\n• {}: {}",
            t!("onboarding.preview.workspace.workspace"),
            onboarding_workspace_display(state, active_workspace)
        ));
        body.push_str(&format!(
            "\n    {}",
            t!("onboarding.preview.workspace.workspace_hint")
        ));
        // `onboarding_workspace_status_label` already carries the `Status:` prefix.
        body.push_str(&format!("\n• {}", onboarding_workspace_status_label(state)));
        // `onboarding_permission_profile_label` already carries the `Permissions:` prefix.
        body.push_str(&format!(
            "\n• {}",
            onboarding_permission_profile_label(state)
        ));
        body.push_str(&format!(
            "\n    {}",
            t!("onboarding.preview.workspace.permissions_hint")
        ));
    }
    preview
}

/// Compute the single next concrete action for the provider/setup phase of the
/// wizard, in dependency order. This drives the `Next: ...` footer so the user
/// always knows the immediate thing to do.
/// #203 guidance short-circuit: the hint and progress must agree with the row
/// labels. While the provider draft is untouched, the rows display the
/// server-hydrated saved primary (the "(saved)" fallback) — if that provider
/// also has a key, the provider/connect/save steps are satisfied by server
/// truth and guidance must move past them. Any staged draft input restores
/// draft-first guidance (the user is re-configuring).
///
/// This MUST key on the same predicate that unlocks the Workspace/Activate
/// rows (`onboarding_has_saved_primary_provider`, which filters on
/// `current_profile`), NOT on `onboarding_saved_primary` (which keys on the
/// staged `effective_profile_id`). Otherwise, with an active session on
/// profile A and a staged `/onboard profile B`, guidance could advance to the
/// Workspace step from B's hydrated state while the Workspace row stays
/// disabled with "save a provider first" against A — contradictory progress
/// (codex P2 on #204). Sharing one predicate keeps them in lock-step.
fn onboarding_saved_guidance_ready(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> bool {
    state.provider_draft_empty()
        && onboarding_has_saved_primary_provider(ctx, state, current_profile)
}

fn onboarding_next_action_hint(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> String {
    // The provider-section checks (catalog through save) judge the draft;
    // skip them entirely when the saved provider already covers the section.
    if !onboarding_saved_guidance_ready(ctx, state, current_profile) {
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
    }
    if onboarding_workspace_disabled_reason(state).is_some() {
        return t!("onboarding.wizard.next.validate_workspace").into_owned();
    }
    if onboarding_open_session_disabled_reason(ctx, state, current_profile).is_none() {
        return t!("onboarding.wizard.next.activate").into_owned();
    }
    t!("onboarding.wizard.next.finish_remaining").into_owned()
}

fn onboarding_local_profile_menu(
    state: &OnboardingWizardState,
    requested_id_supported: bool,
    make_default_supported: bool,
) -> MenuBuildResult {
    // The "Create your local Octos profile / stays on this machine, no OTP"
    // framing is NOT a menu row — it is non-actionable info, so it lives in the
    // right-hand teaching panel (`WizardProgress::explanation_preview`) instead
    // of taking a dead `Noop` slot in the action list.
    let mut items = vec![onboarding_language_row()];

    if requested_id_supported {
        // Phase 2: a solo local tool does not need a full identity. Collapse the
        // step to ONE prompt — "Name this profile".
        //
        // Profile identity is DECOUPLED from the model: naming a profile has
        // nothing to do with which model family it will run, so the profile step
        // no longer offers a family picker (which also derived the name). The
        // model is chosen separately in provider setup after the profile exists
        // (and added/changed anytime via `/add-model`).
        items.push(onboarding_requested_id_row(state));
        // Decision #3: let the user mark this new brain as the machine default —
        // the one a bare launch opens in a folder it hasn't seen before. Only
        // offered when the server can honor it.
        if make_default_supported {
            items.push(onboarding_make_default_row(state));
        }
    } else {
        // Legacy fallback for older servers that do not advertise the nameable
        // feature: keep the full name/username/email create so the TUI still
        // works end-to-end against an older octos.
        items.extend([
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
        ]);
    }

    items.push(
        MenuItem::new(
            "onboard.local.create",
            t!("onboarding.local.continue"),
            MenuAction::Local(LocalAction::Onboarding(
                OnboardingAction::CreateLocalProfile,
            )),
        )
        .with_description(t!("onboarding.local.create_action"))
        .maybe_disabled(if requested_id_supported {
            // The effective id is always non-empty (typed or suggested), so
            // Continue is never blocked in the nameable flow.
            None
        } else {
            onboarding_local_profile_disabled_reason(state)
        }),
    );

    items.push(
        // Escape hatch: the wizard auto-opens and swallows Esc, so it MUST offer
        // a visible way out. (Choosing an existing profile is the startup
        // picker's job — this create step is only ever reached to make a NEW
        // one — so the confusing "use existing profile (ID)" row is gone.)
        MenuItem::new(
            "onboard.local.exit",
            t!("menu.onboard.item.exit.label"),
            MenuAction::Local(LocalAction::Exit),
        )
        .with_description(t!("menu.onboard.item.exit.desc")),
    );

    // Wizard framing: the language step is already satisfied by the default
    // English locale, so this screen is the first required profile input step.
    // The local-create branch is only reached when `profile/local/create` is
    // supported AND no profile is resolved yet, so progress is computed with
    // `local_create_supported = true`, `current_profile = None`, and no saved
    // provider (no profile means nothing can be hydrated).
    let progress = crate::menu::wizard::WizardProgress::from_state(state, None, true, false);
    let next_action = if requested_id_supported || state.local_profile_ready() {
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
        // carries the per-step TEACHING panel (explanatory prose + progress) so
        // the user always sees where they are, what's left, and what to do.
        preview: Some(progress.explanation_preview()),
        mode: MenuMode::SingleSelect,
    })
}

fn onboarding_family_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(catalog) = ctx.app.profile_llm_catalog else {
        // Opening this menu auto-sends `profile/llm/catalog` (see
        // `auto_fetch_for_menu`); render Loading until the result refreshes
        // the menu rather than dead-ending on "load the catalog first". When
        // the server never advertised the catalog method, no fetch is in
        // flight — stay Unavailable instead of loading forever.
        let spec = MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_FAMILY),
            title: t!("menu.onboard.family.title").into_owned(),
            message: t!("menu.onboard.unavailable_catalog_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        };
        return if ctx
            .availability
            .supports_method(APPUI_METHOD_PROFILE_LLM_CATALOG)
        {
            MenuBuildResult::Loading(spec)
        } else {
            MenuBuildResult::Unavailable(spec)
        };
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
        // Same auto-fetch contract as the family step: Loading only while a
        // fetch can actually be in flight; Unavailable on servers that never
        // advertised the catalog method.
        let spec = MenuStatusSpec {
            id: MenuId::from(crate::menu::registry::MENU_ONBOARD_MODEL),
            title: t!("menu.onboard.model.title").into_owned(),
            message: t!("menu.onboard.unavailable_catalog_msg").into_owned(),
            footer_hint: Some(t!("menu.footer.esc_back").into_owned()),
        };
        return if ctx
            .availability
            .supports_method(APPUI_METHOD_PROFILE_LLM_CATALOG)
        {
            MenuBuildResult::Loading(spec)
        } else {
            MenuBuildResult::Unavailable(spec)
        };
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
                    OnboardingAction::SetProviderSelection(Box::new(choice.selection.clone())),
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

/// Phase 2: the single "Name this profile" row. Shows the user's typed id or,
/// while untouched, the provider-derived suggestion tagged `(suggested)`. Enter
/// pre-fills the composer with `/onboard profile-name <id>` seeded with the
/// current value/suggestion so the user can accept it verbatim or edit it. The
/// draft is dynamic (the suggestion varies), so this row is built inline rather
/// than through `onboarding_edit_item`'s `&'static str` draft.
fn onboarding_requested_id_row(state: &OnboardingWizardState) -> MenuItem {
    let label = t!("onboarding.field.profile_name");
    let (rendered_value, seed) = if state.has_requested_id() {
        let typed = state.requested_id.trim().to_owned();
        (typed.clone(), typed)
    } else {
        let suggestion = state.suggested_profile_id();
        (
            t!("onboarding.value_suggested", value = &suggestion).into_owned(),
            suggestion,
        )
    };
    MenuItem::new(
        "onboard.local.requested_id",
        format!("{label}: {rendered_value}"),
        MenuAction::Local(LocalAction::EditComposer(format!(
            "/onboard profile-name {seed}"
        ))),
    )
    .with_description(t!("onboarding.field.profile_name_desc"))
    .with_state(MenuItemState::required(state.has_requested_id()))
}

/// Toggle row (nameable flow) for decision #3: mark this new brain as the
/// machine's global default. Flips [`OnboardingWizardState::make_default`]; the
/// value rides on `profile/local/create` as `make_default`.
fn onboarding_make_default_row(state: &OnboardingWizardState) -> MenuItem {
    let value = if state.make_default {
        t!("onboarding.make_default.enabled")
    } else {
        t!("onboarding.make_default.disabled")
    };
    MenuItem::new(
        "onboard.local.make_default",
        format!("{}: {}", t!("onboarding.make_default.label"), value),
        MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetMakeDefault(
            !state.make_default,
        ))),
    )
    .with_description(t!("onboarding.make_default.desc"))
}

/// The server-saved primary provider for the wizard's effective profile, read
/// from `profile_llm_state` (server truth via `profile/llm/list`). `None` when
/// no state was hydrated yet or it belongs to a different profile.
fn onboarding_saved_primary<'a>(
    ctx: &MenuContext<'a>,
    state: &OnboardingWizardState,
    current_profile: Option<&str>,
) -> Option<&'a LlmConfiguredProvider> {
    let effective = state.effective_profile_id(current_profile);
    ctx.app
        .profile_llm_state
        .filter(
            |llm| match (llm.profile_id.as_deref(), effective.as_deref()) {
                (Some(saved), Some(wanted)) => saved == wanted,
                _ => true,
            },
        )
        .and_then(|llm| llm.primary_provider())
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn saved_family_id(provider: &LlmConfiguredProvider) -> Option<&str> {
    provider
        .family_id
        .as_deref()
        .and_then(non_empty)
        .or_else(|| non_empty(&provider.provider))
}

fn saved_model_id(provider: &LlmConfiguredProvider) -> Option<&str> {
    provider
        .model_id
        .as_deref()
        .and_then(non_empty)
        .or_else(|| non_empty(&provider.model))
}

fn saved_route_id(provider: &LlmConfiguredProvider) -> Option<&str> {
    provider
        .route_id
        .as_deref()
        .and_then(non_empty)
        .or_else(|| provider.route.as_ref().and_then(|r| non_empty(&r.route_id)))
}

/// Draft-first, saved-fallback display: the wizard's local draft always wins;
/// when it is empty the server-saved value shows with a "(saved)" marker so a
/// configured profile never reads as "not set".
fn onboarding_family_label(
    state: &OnboardingWizardState,
    saved: Option<&LlmConfiguredProvider>,
) -> String {
    if let Some(family) = non_empty(&state.provider.family_id) {
        return family.to_owned();
    }
    if let Some(family) = saved.and_then(saved_family_id) {
        return t!("onboarding.value_saved", value = family).into_owned();
    }
    t!("onboarding.value_not_set").into_owned()
}

fn onboarding_model_label(
    state: &OnboardingWizardState,
    saved: Option<&LlmConfiguredProvider>,
) -> String {
    if let Some(model) = non_empty(&state.provider.model_id) {
        return model.to_owned();
    }
    if let Some(model) = saved.and_then(saved_model_id) {
        return t!("onboarding.value_saved", value = model).into_owned();
    }
    t!("onboarding.value_not_set").into_owned()
}

fn onboarding_route_label(
    state: &OnboardingWizardState,
    saved: Option<&LlmConfiguredProvider>,
) -> String {
    if non_empty(&state.provider.route.route_id).is_some() {
        return state
            .provider
            .route
            .label
            .as_deref()
            .map(|label| format!("{label} ({})", state.provider.route.route_id))
            .unwrap_or_else(|| state.provider.route.route_id.clone());
    }
    if let Some(route) = saved.and_then(saved_route_id) {
        return t!("onboarding.value_saved", value = route).into_owned();
    }
    t!("onboarding.value_not_set").into_owned()
}

fn onboarding_model_selected(state: &OnboardingWizardState) -> bool {
    !state.provider.family_id.trim().is_empty() && !state.provider.model_id.trim().is_empty()
}

fn onboarding_auth_label(state: &OnboardingWizardState) -> String {
    if state.auth_verified {
        t!("onboarding.auth.verified").into_owned()
    } else if state.auth_code_sent {
        t!("onboarding.auth.code_sent", email = state.email.clone()).into_owned()
    } else if state.has_email() {
        t!("onboarding.auth.email", email = state.email.clone()).into_owned()
    } else {
        t!("onboarding.auth.email_not_set").into_owned()
    }
}

fn onboarding_local_profile_label(state: &OnboardingWizardState) -> String {
    if state.local_profile_created {
        t!(
            "onboarding.profile.created",
            profile = state.profile_label(None)
        )
        .into_owned()
    } else if state.local_profile_ready() {
        t!(
            "onboarding.profile.ready",
            username = state.username.clone()
        )
        .into_owned()
    } else {
        t!("onboarding.profile.required").into_owned()
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
        None => return t!("onboarding.permissions.default_hint").into_owned(),
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
        .unwrap_or_else(|| t!("onboarding.permissions.mode_unchanged").into_owned());
    let approval = staged
        .approval_policy
        .clone()
        .unwrap_or_else(|| t!("onboarding.permissions.unchanged").into_owned());
    let network = staged
        .network
        .map(|n| match n {
            octos_core::ui_protocol::PermissionNetworkPolicy::Allow => {
                t!("onboarding.permissions.network_allowed").into_owned()
            }
            octos_core::ui_protocol::PermissionNetworkPolicy::Deny => {
                t!("onboarding.permissions.network_blocked").into_owned()
            }
        })
        .unwrap_or_else(|| t!("onboarding.permissions.network_unchanged").into_owned());
    if let Some(mismatch) = state.permission_profile_mismatch.as_deref() {
        t!(
            "onboarding.permissions.staged_clamped",
            mode = mode,
            approval = approval,
            network = network,
            mismatch = mismatch,
        )
        .into_owned()
    } else {
        t!(
            "onboarding.permissions.staged",
            mode = mode,
            approval = approval,
            network = network,
        )
        .into_owned()
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
        Some(t!("onboarding.disabled.name_empty").into_owned())
    } else if !state.has_username() {
        Some(t!("onboarding.disabled.username_empty").into_owned())
    } else if !state.has_email() {
        Some(t!("onboarding.disabled.email_empty").into_owned())
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
    Some(t!("onboarding.disabled.profile_unresolved").into_owned())
}

fn onboarding_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
    method: &'static str,
) -> Option<String> {
    action_missing_reason(ctx, method).or_else(|| {
        (!state.has_email()).then(|| t!("onboarding.disabled.email_empty").into_owned())
    })
}

fn onboarding_verify_disabled_reason(
    ctx: &MenuContext<'_>,
    state: &OnboardingWizardState,
) -> Option<String> {
    action_missing_reason(ctx, APPUI_METHOD_AUTH_VERIFY).or_else(|| {
        if !state.has_email() {
            Some(t!("onboarding.disabled.email_empty").into_owned())
        } else if !state.has_otp_code() {
            Some(t!("onboarding.disabled.otp_empty").into_owned())
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
            Some(t!("onboarding.disabled.provider_incomplete").into_owned())
        } else if !state.has_api_key() {
            Some(t!("onboarding.disabled.api_key_empty").into_owned())
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
                .then(|| t!("onboarding.disabled.save_provider_first").into_owned())
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
            Some(t!("onboarding.disabled.validate_workspace_first").into_owned())
        }
        crate::model::OnboardingWorkspaceValidation::Validating => {
            Some(t!("onboarding.disabled.workspace_validating").into_owned())
        }
        crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
            Some(t!("onboarding.disabled.workspace_invalid", reason = reason).into_owned())
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
                    Some(t!("onboarding.workspace.active", path = trimmed).into_owned())
                }
            })
            .unwrap_or_else(|| t!("onboarding.workspace.unset").into_owned()),
    }
}

fn onboarding_workspace_status_label(state: &OnboardingWizardState) -> String {
    match &state.workspace_validation {
        crate::model::OnboardingWorkspaceValidation::Unvalidated => {
            t!("onboarding.workspace.status_unvalidated").into_owned()
        }
        crate::model::OnboardingWorkspaceValidation::Validating => {
            t!("onboarding.workspace.status_validating").into_owned()
        }
        crate::model::OnboardingWorkspaceValidation::Valid {
            writable,
            has_workspace_toml,
            ..
        } => {
            let writable_label = if *writable {
                t!("onboarding.workspace.writable").into_owned()
            } else {
                t!("onboarding.workspace.read_only").into_owned()
            };
            let toml_label = if *has_workspace_toml {
                t!("onboarding.workspace.toml_present").into_owned()
            } else {
                String::new()
            };
            t!(
                "onboarding.workspace.status_ok",
                writable = writable_label,
                toml = toml_label,
            )
            .into_owned()
        }
        crate::model::OnboardingWorkspaceValidation::Invalid { reason } => {
            t!("onboarding.workspace.status_invalid", reason = reason).into_owned()
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
        Some(OnboardingProviderPending::Test) => {
            t!("onboarding.provider.test_testing").into_owned()
        }
        Some(OnboardingProviderPending::Save) => {
            t!("onboarding.provider.test_unavailable_saving").into_owned()
        }
        None if state.provider_tested => t!("onboarding.provider.tested").into_owned(),
        None if state.provider_test_failure_reason.is_some() => {
            // M22-E: surface the typed test failure so the user
            // sees what went wrong and knows to edit the key or
            // pick a different route.
            let reason = state
                .provider_test_failure_reason
                .as_deref()
                .unwrap_or_default();
            t!("onboarding.provider.test_failed", reason = reason).into_owned()
        }
        None => t!("onboarding.provider.test").into_owned(),
    }
}

fn onboarding_provider_save_label(state: &OnboardingWizardState) -> String {
    match state.provider_pending {
        Some(OnboardingProviderPending::Save) => t!("onboarding.provider.saving").into_owned(),
        Some(OnboardingProviderPending::Test) => {
            t!("onboarding.provider.save_unavailable_testing").into_owned()
        }
        None if state.provider_saved && state.provider_tested => {
            t!("onboarding.provider.saved").into_owned()
        }
        None => t!("onboarding.provider.save").into_owned(),
    }
}

fn onboarding_provider_fallback_label(state: &OnboardingWizardState) -> String {
    match state.provider_pending {
        Some(OnboardingProviderPending::Save) => t!("onboarding.provider.saving").into_owned(),
        Some(OnboardingProviderPending::Test) => {
            t!("onboarding.provider.fallback_unavailable_testing").into_owned()
        }
        None => t!("onboarding.provider.add_fallback").into_owned(),
    }
}

fn onboarding_provider_saved_status_label(state: &OnboardingWizardState) -> String {
    if let (Some(target), Some(label)) = (
        state.last_saved_provider_target,
        state.last_saved_provider_label.as_deref(),
    ) {
        t!(
            "onboarding.provider.saved_status_target",
            target = save_target_label(target),
            label = label,
        )
        .into_owned()
    } else if let Some(label) = state.saved_primary_provider_label.as_deref() {
        t!("onboarding.provider.saved_status_primary", label = label,).into_owned()
    } else {
        t!("onboarding.provider.saved_status_none").into_owned()
    }
}

fn onboarding_provider_saved_status_state(state: &OnboardingWizardState) -> MenuItemState {
    MenuItemState {
        checked: state.last_saved_provider_label.is_some().then_some(true),
        required_valid: state.last_saved_provider_label.as_ref().map(|_| true),
        ..MenuItemState::default()
    }
}

fn save_target_label(target: OnboardingProviderSaveTarget) -> String {
    match target {
        OnboardingProviderSaveTarget::Primary => t!("onboarding.provider.primary").into_owned(),
        OnboardingProviderSaveTarget::Fallback => t!("onboarding.provider.fallback").into_owned(),
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
                    OnboardingAction::SetProviderSelection(Box::new(choice.selection.clone())),
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
            MenuAction::send_appui(AppUiCommand::AuthStatus(AuthStatusParams::default())),
        )
        .with_description("Uses auth/status.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_STATUS)),
        MenuItem::new(
            "login.me",
            t!("menu.login.item.current_account.label"),
            MenuAction::send_appui(AppUiCommand::AuthMe(AuthMeParams {
                token: state.auth_token.clone(),
            })),
        )
        .with_description("Uses auth/me.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_AUTH_ME)),
        MenuItem::new(
            "login.logout",
            t!("menu.login.item.logout.label"),
            MenuAction::send_appui(AppUiCommand::AuthLogout(AuthLogoutParams {
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
        MenuAction::send_appui(AppUiCommand::ProfileLlmList(ProfileLlmListParams {
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
            // Exactly ONE row is the active model. The catalog's `selected`
            // primary wins (the server marks precisely one, by
            // family+model+route); only if none is selected do we fall back to
            // the row matching the live runtime model id. Resolving a single
            // index — rather than an OR per row — guarantees at most one `*`
            // even when a backend erroneously marks several rows `selected` (a
            // mock/misbehaving server — the "everything shows *" symptom) or two
            // configured entries share a model id (same model via two providers
            // or routes), where the old id-only OR lit up every match.
            let current_idx = models.iter().position(|model| model.selected).or_else(|| {
                ctx.app
                    .current_model
                    .and_then(|current| models.iter().position(|model| model.model == current))
            });
            for (idx, model) in models.iter().enumerate() {
                let id = format!("model.select.{idx}");
                let state = MenuItemState {
                    current: current_idx == Some(idx),
                    ..MenuItemState::default()
                };
                let action = if can_select {
                    MenuAction::send_appui(AppUiCommand::ProfileLlmSelect(ProfileLlmSelectParams {
                        profile_id: profile_id.clone(),
                        session_id: ctx.app.selected_session_id.cloned(),
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
            MenuAction::send_appui(AppUiCommand::ProfileLlmCatalog(
                ProfileLlmCatalogParams::default(),
            )),
        )
        .with_description("Uses profile/llm/catalog.")
        .maybe_disabled(action_missing_reason(ctx, APPUI_METHOD_PROFILE_LLM_CATALOG)),
        MenuItem::new(
            "provider.list",
            t!("menu.provider.item.list_refresh.label"),
            MenuAction::send_appui(AppUiCommand::ProfileLlmList(ProfileLlmListParams {
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
                    MenuAction::send_appui(AppUiCommand::ProfileLlmTest(ProfileLlmTestParams {
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
            MenuAction::send_appui(AppUiCommand::ListMcpConfig(McpConfigListParams {
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
                MenuAction::send_appui(AppUiCommand::ListMcpStatus(McpStatusListParams {
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
                        MenuAction::send_appui(AppUiCommand::SetMcpConfigEnabled(
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
                        MenuAction::send_appui(AppUiCommand::TestMcpConfig(McpConfigTestParams {
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
                let delete_state = MenuItemState {
                    destructive: true,
                    ..MenuItemState::default()
                };
                items.push(
                    MenuItem::new(
                        format!("mcp.server.{server_name}.delete"),
                        format!("Delete {server_name}"),
                        MenuAction::send_appui(AppUiCommand::DeleteMcpConfig(
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
            MenuAction::send_appui(AppUiCommand::ListToolConfig(ToolConfigListParams {
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
                MenuAction::send_appui(AppUiCommand::ListToolStatus(ToolStatusListParams {
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
                        MenuAction::send_appui(AppUiCommand::SetToolConfigEnabled(
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
                        MenuAction::send_appui(AppUiCommand::TestToolConfig(
                            ToolConfigTestParams {
                                session_id: session_id.clone(),
                                profile_id: profile_id.clone(),
                                tool: tool_name.clone(),
                            },
                        )),
                    )
                    .with_description("Uses tool/config/test.")
                    .maybe_disabled(mutating_action_missing_reason(
                        ctx,
                        APPUI_METHOD_TOOL_CONFIG_TEST,
                    )),
                );
                let delete_state = MenuItemState {
                    destructive: true,
                    ..MenuItemState::default()
                };
                items.push(
                    MenuItem::new(
                        format!("tools.tool.{tool_name}.delete"),
                        format!("Delete {tool_name}"),
                        MenuAction::send_appui(AppUiCommand::DeleteToolConfig(
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
                let state = MenuItemState {
                    checked: Some(tool.enabled),
                    destructive: tool.denial.is_some(),
                    ..MenuItemState::default()
                };
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
            MenuAction::send_appui(AppUiCommand::ProfileSkillsList(ProfileSkillsListParams {
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
                let state = MenuItemState {
                    destructive: true,
                    ..MenuItemState::default()
                };
                items.push(
                    MenuItem::new(
                        format!("skills.remove.{}", skill.name),
                        format!("{} {}", t!("menu.skills.item.remove.prefix"), skill.name),
                        MenuAction::send_appui(AppUiCommand::ProfileSkillsRemove(
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
            let state = MenuItemState {
                checked: package.installed.then_some(true),
                ..MenuItemState::default()
            };
            items.push(
                MenuItem::new(
                    format!("skills.registry.{}", package.name),
                    format!("{} {}", t!("menu.skills.item.install.prefix"), package.name),
                    MenuAction::send_appui(AppUiCommand::ProfileSkillsInstall(
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

/// Phase 2: `true` when the server advertises nameable solo profiles. Selects
/// the slimmed single-prompt Profile step over the legacy name/username/email
/// screen. Backward compatible: `false` against older servers.
fn local_profile_requested_id_supported(ctx: &MenuContext<'_>) -> bool {
    ctx.availability
        .supports_feature(crate::model::APPUI_FEATURE_PROFILE_LOCAL_CREATE_REQUESTED_ID_V1)
}

/// True when the backend advertises the optional `make_default` create field, so
/// onboarding may offer the "Make this your default brain?" toggle. Backward
/// compatible: `false` against older servers, which hides the row.
fn local_profile_make_default_supported(ctx: &MenuContext<'_>) -> bool {
    ctx.availability
        .supports_feature(crate::model::APPUI_FEATURE_PROFILE_LOCAL_CREATE_DEFAULT_V1)
}

/// True when the backend advertises the per-project launch flow
/// (`session.workspace_cwd.v1` + `launch/resolve`), so onboarding can end at the
/// launch-instructions screen and defer session activation to launch time.
/// Backward compatible: `false` against older servers, which keep the in-wizard
/// workspace/Activate step.
fn launch_flow_supported(ctx: &MenuContext<'_>) -> bool {
    ctx.availability
        .supports_feature(crate::model::APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1)
        && ctx
            .availability
            .supports_method(crate::model::APPUI_METHOD_LAUNCH_RESOLVE)
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
        t!("menu.availability.ui_unavailable").into_owned()
    } else if let Some((method, reason)) =
        APPUI_PERMISSION_MENU_METHODS_ANY.iter().find_map(|method| {
            ctx.availability
                .unsupported_method_reason(method)
                .map(|reason| (*method, reason))
        })
    {
        t!(
            "menu.availability.method_unsupported",
            method = method,
            reason = reason
        )
        .into_owned()
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
            MenuAction::send_appui(AppUiCommand::ListPermissionProfiles(
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
    MenuAction::send_appui(AppUiCommand::SetPermissionProfile(
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
        MenuAction::send_appui(AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
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
        t!(
            "menu.availability.method_unsupported",
            method = method,
            reason = reason
        )
        .into_owned()
    } else if ctx.availability.capabilities.is_none() {
        t!("menu.availability.ui_unavailable").into_owned()
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

    fn appui_command(action: &MenuAction) -> &AppUiCommand {
        let MenuAction::SendAppUi(command) = action else {
            panic!("expected AppUI action");
        };
        command.as_ref()
    }

    fn ready_spec(result: MenuBuildResult) -> MenuSpec {
        match result {
            MenuBuildResult::Ready(spec) => spec,
            other => panic!("expected a ready menu, got {other:?}"),
        }
    }

    fn has_row(spec: &MenuSpec, id: &str) -> bool {
        spec.items.iter().any(|item| item.id == id)
    }

    #[test]
    fn profile_step_shows_single_name_prompt_when_requested_id_supported() {
        let state = OnboardingWizardState::default();
        let spec = ready_spec(onboarding_local_profile_menu(&state, true, false));

        assert!(
            has_row(&spec, "onboard.local.requested_id"),
            "nameable flow shows the single Name-this-profile row"
        );
        // Profile identity is decoupled from the model — no family picker here.
        assert!(
            !has_row(&spec, "onboard.local.family"),
            "the profile step never offers a model-family choice"
        );
        // And the non-actionable "stays local" blurb is not a dead menu row.
        assert!(
            !has_row(&spec, "onboard.local.status"),
            "the info blurb lives in the right panel, not the action list"
        );
        assert!(
            !has_row(&spec, "onboard.local.name")
                && !has_row(&spec, "onboard.local.username")
                && !has_row(&spec, "onboard.local.email"),
            "nameable flow drops the name/username/email rows"
        );
        // Continue is never blocked — a suggestion is always available.
        let create = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.local.create")
            .expect("create row present");
        assert!(
            create.disabled_reason.is_none(),
            "Continue is enabled (accepts the suggested id)"
        );
    }

    #[test]
    fn profile_step_shows_make_default_toggle_only_when_supported() {
        // Off by default: the row flips the toggle ON when activated.
        let state = OnboardingWizardState::default();
        let spec = ready_spec(onboarding_local_profile_menu(&state, true, true));
        let row = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.local.make_default")
            .expect("make-default toggle present when supported");
        assert!(
            matches!(
                &row.action,
                MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetMakeDefault(
                    true
                )))
            ),
            "an off toggle flips ON when activated"
        );

        // Already on: the row flips it OFF.
        let on = OnboardingWizardState {
            make_default: true,
            ..OnboardingWizardState::default()
        };
        let spec_on = ready_spec(onboarding_local_profile_menu(&on, true, true));
        let row_on = spec_on
            .items
            .iter()
            .find(|item| item.id == "onboard.local.make_default")
            .unwrap();
        assert!(matches!(
            &row_on.action,
            MenuAction::Local(LocalAction::Onboarding(OnboardingAction::SetMakeDefault(
                false
            )))
        ));

        // Unsupported server → the row is hidden entirely.
        let unsupported = ready_spec(onboarding_local_profile_menu(&state, true, false));
        assert!(!has_row(&unsupported, "onboard.local.make_default"));
    }

    #[test]
    fn provider_step_ends_at_done_screen_on_launch_flow_server() {
        let onboarding = OnboardingWizardState {
            profile_id: Some("glm".into()),
            local_profile_created: true,
            ..OnboardingWizardState::default()
        };

        // Launch-flow server (feature + method): the terminal row is the done
        // screen, and the redundant workspace step is skipped.
        let launch_caps = CapabilitySet::from_methods_and_features(
            [
                crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
                crate::model::APPUI_METHOD_LAUNCH_RESOLVE,
            ],
            [crate::model::APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1],
        );
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&launch_caps),
            app: MenuAppSnapshot {
                current_profile: Some("glm"),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let spec = ready_spec(onboarding_provider_setup_menu(
            &ctx,
            &onboarding,
            Some("glm"),
        ));
        assert!(
            has_row(&spec, "onboard.done.open"),
            "launch-flow onboarding ends at the done screen"
        );
        assert!(
            !has_row(&spec, "onboard.workspace.open"),
            "the redundant workspace step is skipped"
        );

        // Older server (no launch flow): keep the workspace/Activate step so the
        // user is never stranded without a way to start a session.
        let legacy_caps =
            CapabilitySet::from_methods([crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT]);
        let legacy_ctx = MenuContext {
            availability: AvailabilityContext::protocol(&legacy_caps),
            app: MenuAppSnapshot {
                current_profile: Some("glm"),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let legacy = ready_spec(onboarding_provider_setup_menu(
            &legacy_ctx,
            &onboarding,
            Some("glm"),
        ));
        assert!(has_row(&legacy, "onboard.workspace.open"));
        assert!(!has_row(&legacy, "onboard.done.open"));
    }

    #[test]
    fn onboard_done_menu_shows_launch_instructions_and_exit() {
        let onboarding = OnboardingWizardState {
            profile_id: Some("glm".into()),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot {
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let spec = ready_spec(onboarding_done_menu(&ctx));
        assert!(
            has_row(&spec, "onboard.done.status"),
            "shows the ready line"
        );
        assert!(has_row(&spec, "onboard.done.exit"), "offers a way out");
        assert!(
            spec.subtitle.as_deref().unwrap_or_default().contains("glm"),
            "names the created brain in the subtitle"
        );
    }

    #[test]
    fn profile_step_falls_back_to_full_fields_without_feature() {
        let state = OnboardingWizardState::default();
        let spec = ready_spec(onboarding_local_profile_menu(&state, false, false));

        assert!(
            has_row(&spec, "onboard.local.name")
                && has_row(&spec, "onboard.local.username")
                && has_row(&spec, "onboard.local.email"),
            "legacy flow keeps name/username/email for older servers"
        );
        assert!(
            !has_row(&spec, "onboard.local.requested_id"),
            "legacy flow has no requested_id prompt"
        );
        // The family choice is nameable-flow only.
        assert!(!has_row(&spec, "onboard.local.family"));
    }

    #[test]
    fn onboarding_menu_routes_to_single_prompt_when_feature_negotiated() {
        let capabilities = CapabilitySet::from_methods_and_features(
            [APPUI_METHOD_PROFILE_LOCAL_CREATE],
            [crate::model::APPUI_FEATURE_PROFILE_LOCAL_CREATE_REQUESTED_ID_V1],
        );
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
        let spec = ready_spec(onboarding_menu(&ctx));
        assert!(has_row(&spec, "onboard.local.requested_id"));
        assert!(!has_row(&spec, "onboard.local.email"));
    }

    #[test]
    fn profile_picker_lists_profiles_marks_default_and_offers_create() {
        let onboarding = OnboardingWizardState {
            available_profiles: vec!["glm".into(), "deepseek".into()],
            default_profile: Some("glm".into()),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot {
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let spec = ready_spec(profile_picker_menu(&ctx));

        // Each profile row now drills into its action menu (info + set-default /
        // delete), rather than attaching directly.
        let glm = spec
            .items
            .iter()
            .find(|item| item.id == "profile.pick.0")
            .expect("glm row present");
        assert!(
            glm.label.contains("glm") && glm.label.contains("default"),
            "the default profile is marked: {:?}",
            glm.label
        );
        assert!(matches!(
            &glm.action,
            MenuAction::Local(LocalAction::SelectProfileForActions(id)) if id == "glm"
        ));
        // The non-default profile is listed without the marker.
        let deepseek = spec
            .items
            .iter()
            .find(|item| item.id == "profile.pick.1")
            .expect("deepseek row present");
        assert_eq!(deepseek.label, "deepseek");
        assert!(has_row(&spec, "profile.pick.new"), "offers a create row");
    }

    #[test]
    fn launch_prompt_cross_profile_offers_start_here_and_switch_rows() {
        let onboarding = OnboardingWizardState {
            launch_prompt: Some(crate::model::LaunchPromptState {
                decision: crate::model::LaunchDecisionKind::CrossProfile,
                resolved_profile: "glm".into(),
                existing_profiles: vec!["deepseek".into()],
                cwd: "/tmp/proj".into(),
            }),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot {
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let spec = ready_spec(launch_prompt_menu(&ctx));

        // "Start <launching> here" opens the launching brain in this folder.
        let start = spec
            .items
            .iter()
            .find(|item| item.id == "launch.start")
            .expect("start-here row present");
        let AppUiCommand::OpenSession(params) = appui_command(&start.action) else {
            panic!("start row must open a session");
        };
        assert_eq!(params.profile_id.as_deref(), Some("glm"));
        assert_eq!(params.cwd.as_deref(), Some("/tmp/proj"));

        // One switch row per profile already used in this folder.
        let switch = spec
            .items
            .iter()
            .find(|item| item.id == "launch.switch.0")
            .expect("switch row present");
        let AppUiCommand::OpenSession(switch_params) = appui_command(&switch.action) else {
            panic!("switch row must open a session");
        };
        assert_eq!(switch_params.profile_id.as_deref(), Some("deepseek"));
        assert!(has_row(&spec, "launch.cancel"), "offers a cancel escape");
    }

    #[test]
    fn launch_prompt_activate_confirms_single_profile() {
        let onboarding = OnboardingWizardState {
            launch_prompt: Some(crate::model::LaunchPromptState {
                decision: crate::model::LaunchDecisionKind::Activate,
                resolved_profile: "glm".into(),
                existing_profiles: Vec::new(),
                cwd: "/tmp/proj".into(),
            }),
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::local(),
            app: MenuAppSnapshot {
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let spec = ready_spec(launch_prompt_menu(&ctx));

        let activate = spec
            .items
            .iter()
            .find(|item| item.id == "launch.activate")
            .expect("activate row present");
        let AppUiCommand::OpenSession(params) = appui_command(&activate.action) else {
            panic!("activate row must open a session");
        };
        assert_eq!(params.profile_id.as_deref(), Some("glm"));
        // Activate is a single-profile confirm — no switch rows.
        assert!(!has_row(&spec, "launch.switch.0"));
        // Both the label AND description must interpolate the profile, not leave
        // a literal "%{profile}" placeholder (regression: the desc dropped the arg).
        assert!(
            activate.label.contains("glm") && !activate.label.contains("%{profile}"),
            "activate label: {:?}",
            activate.label
        );
        assert!(
            activate
                .description
                .as_deref()
                .is_some_and(|d| d.contains("glm") && !d.contains("%{profile}")),
            "activate desc must name the profile: {:?}",
            activate.description
        );
    }

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
        // Stage a model family so the provider step renders its expanded config
        // rows (family/model/route/key). Without a staged model the step is
        // collapsed to a single "Add a model" entry, which hides those rows.
        let onboarding = OnboardingWizardState {
            provider: LlmSelectionConfig {
                family_id: "moonshot".into(),
                ..LlmSelectionConfig::default()
            },
            ..OnboardingWizardState::default()
        };
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
        // UX2 A.3: the provider/setup phase now carries the per-step TEACHING
        // panel (explanatory prose + progress) as its right-side preview pane.
        assert!(
            matches!(
                spec.preview,
                Some(crate::menu::types::MenuPreview::Text { .. })
            ),
            "provider setup menu should show the per-step explanation panel"
        );
        assert!(
            spec.items
                .iter()
                .any(|item| item.id == "onboard.provider.key")
        );
        // UX2 B.2: workspace staging lives on its OWN step screen now, reached
        // via the "continue to workspace" row — it is NOT in the provider menu.
        assert!(
            spec.items
                .iter()
                .any(|item| item.id == "onboard.workspace.open"),
            "provider menu routes to the workspace step"
        );
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id == "onboard.workspace.validate"),
            "workspace validation row moved off the provider menu"
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

    /// Profile↔model decoupling: after naming a profile, the provider step is
    /// collapsed to a single "Add a model" entry — the family/model/route/key
    /// config rows are hidden until a model is actually being set up. Once a
    /// family is staged the step expands to those rows. Either way the terminal
    /// (finish/workspace) row stays, so onboarding can always progress.
    #[test]
    fn provider_step_collapses_model_config_behind_add_a_model() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_AUTH_STATUS,
            APPUI_METHOD_PROFILE_LLM_CATALOG,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        let build = |onboarding: &OnboardingWizardState| {
            let ctx = MenuContext {
                availability: AvailabilityContext::protocol(&capabilities),
                app: MenuAppSnapshot {
                    current_profile: Some("coding"),
                    onboarding: Some(onboarding),
                    ..MenuAppSnapshot::default()
                },
                terminal: TerminalSize::default(),
                theme_name: None,
                selected_path: &[],
            };
            let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_ONBOARD), &ctx)
            else {
                panic!("expected provider setup menu");
            };
            spec
        };

        // Nothing configured yet → collapsed.
        let collapsed = build(&OnboardingWizardState::default());
        assert!(
            has_row(&collapsed, "onboard.provider.add_model"),
            "collapsed step offers a single Add-a-model entry"
        );
        assert!(
            !has_row(&collapsed, "onboard.provider.family")
                && !has_row(&collapsed, "onboard.provider.key"),
            "the inline model config rows are hidden until setup begins"
        );
        assert!(
            has_row(&collapsed, "onboard.workspace.open")
                || has_row(&collapsed, "onboard.done.open"),
            "the terminal (finish/workspace) row still lets onboarding progress"
        );

        // A staged, UNSAVED selection means the user is actively setting up a
        // model → expanded config rows.
        let configuring = OnboardingWizardState {
            provider: LlmSelectionConfig {
                family_id: "glm-4.6".into(),
                model_id: "glm-5.2".into(),
                route: LlmRouteConfig {
                    route_id: "zai".into(),
                    ..LlmRouteConfig::default()
                },
                ..LlmSelectionConfig::default()
            },
            ..OnboardingWizardState::default()
        };
        let expanded = build(&configuring);
        assert!(
            has_row(&expanded, "onboard.provider.family")
                && has_row(&expanded, "onboard.provider.key"),
            "an unsaved staged selection expands to the detailed model rows"
        );
        assert!(
            !has_row(&expanded, "onboard.provider.add_model"),
            "the collapsed entry is replaced while actively configuring"
        );
        // Onboarding never surfaces "Add as fallback" — first-run is about one
        // model; fallbacks live behind `/add-model` (MENU_PROVIDER).
        assert!(
            !has_row(&expanded, "onboard.provider.fallback"),
            "onboarding does not offer the fallback save"
        );

        // Once the staged selection has been SAVED as the primary (session
        // label matches), the step collapses back to "Add another model" — it
        // must NOT dump the raw form (the user's "still no add_model option"
        // report). The saved model shows via the right-pane summary.
        let mut saved = configuring.clone();
        saved.provider_saved = true;
        saved.saved_primary_provider_label = Some(saved.provider_label());
        let after_save = build(&saved);
        assert!(
            has_row(&after_save, "onboard.provider.add_model"),
            "a saved primary collapses back to the Add-another-model entry"
        );
        assert!(
            !has_row(&after_save, "onboard.provider.family")
                && !has_row(&after_save, "onboard.provider.key"),
            "the raw config rows are hidden once the primary is saved"
        );

        // Staging a DIFFERENT model than the saved primary re-expands (to
        // replace it) — but still without a fallback row (that's `/add-model`).
        let mut adding_second = saved.clone();
        adding_second.provider.model_id = "glm-4.7".into();
        let second = build(&adding_second);
        assert!(
            has_row(&second, "onboard.provider.family")
                && !has_row(&second, "onboard.provider.fallback"),
            "staging a different model re-expands, still with no fallback row"
        );
    }

    /// UX2 B.2: the workspace step is its OWN menu (`MENU_ONBOARD_WORKSPACE`),
    /// not part of the provider-setup screen. It owns the workspace candidate,
    /// validation status, the re-validate action, the staged permission row,
    /// and the final ACTIVATE action — and carries the per-step teaching panel.
    #[test]
    fn onboarding_workspace_menu_owns_workspace_validation_and_activate() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_AUTH_STATUS,
            APPUI_METHOD_PROFILE_LLM_UPSERT,
        ]);
        // Profile resolved, provider saved, workspace validated → Activate is
        // unblocked on the workspace screen.
        let onboarding = OnboardingWizardState {
            profile_id: Some("ada".into()),
            provider: LlmSelectionConfig {
                family_id: "moonshot".into(),
                model_id: "kimi-k2.5".into(),
                route: LlmRouteConfig {
                    route_id: "moonshot".into(),
                    ..LlmRouteConfig::default()
                },
                ..LlmSelectionConfig::default()
            },
            api_key: Some(crate::model::SecretString::new("sk-test")),
            provider_tested: true,
            provider_saved: true,
            workspace_validation: crate::model::OnboardingWorkspaceValidation::Valid {
                canonical: "/tmp/ws".into(),
                writable: true,
                has_workspace_toml: false,
            },
            ..OnboardingWizardState::default()
        };
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                current_profile: Some("ada"),
                onboarding: Some(&onboarding),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };

        let MenuBuildResult::Ready(spec) = registry.build(
            &MenuId::from(crate::menu::registry::MENU_ONBOARD_WORKSPACE),
            &ctx,
        ) else {
            panic!("expected workspace step menu");
        };
        assert_eq!(spec.title, t!("onboarding.wizard.workspace_title"));
        // UX2 feedback: the left list holds ONLY actionable rows — Validate +
        // Activate. The read-only staged rows moved to the info pane.
        for id in ["onboard.workspace.validate", "onboard.finish"] {
            assert!(
                spec.items.iter().any(|item| item.id == id),
                "workspace menu must contain actionable `{id}`"
            );
        }
        // The non-actionable (`Noop`) staged rows are NO LONGER in the left list
        // — they're read-only info, surfaced in the right pane instead.
        for id in [
            "onboard.workspace.current",
            "onboard.workspace.status",
            "onboard.permissions.staged",
        ] {
            assert!(
                !spec.items.iter().any(|item| item.id == id),
                "read-only `{id}` must move to the info pane, not the left list"
            );
        }
        // Provider config rows do NOT bleed into the workspace screen.
        assert!(
            !spec
                .items
                .iter()
                .any(|item| item.id == "onboard.provider.key"),
            "provider rows stay on the provider screen"
        );
        // Activate is unblocked given saved provider + valid workspace.
        let activate = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.finish")
            .expect("activate row");
        assert!(
            activate.is_enabled(),
            "activate is unblocked with provider saved + workspace valid"
        );
        // The staged workspace path, validation status, and permission profile
        // now ride in the right-side info pane (read-only text), not the list.
        let Some(crate::menu::types::MenuPreview::Text { body, .. }) = &spec.preview else {
            panic!("workspace step must keep a Text teaching pane");
        };
        assert!(
            body.contains("/tmp/ws"),
            "info pane should show the staged workspace path: {body:?}"
        );
        assert!(
            body.contains(&onboarding_workspace_status_label(&onboarding)),
            "info pane should show the validation status: {body:?}"
        );
        assert!(
            body.contains(&onboarding_permission_profile_label(&onboarding)),
            "info pane should show the staged permission profile: {body:?}"
        );
        assert!(
            body.contains("/onboard workspace"),
            "info pane should show how to change the workspace: {body:?}"
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
        assert_eq!(
            test_item.label,
            t!("onboarding.provider.test_testing").into_owned()
        );
        assert!(test_item.state.loading);
        assert_eq!(test_item.disabled_reason, None);
        let save_item = spec
            .items
            .iter()
            .find(|item| item.id == "onboard.provider.save")
            .expect("save provider row");
        assert_eq!(
            save_item.label,
            t!("onboarding.provider.save_unavailable_testing").into_owned()
        );
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

        // UX2 feedback: the read-only `Noop` status rows are NOT in the left
        // list — they moved to the right info pane.
        for id in [
            "onboard.provider.profile",
            "onboard.provider.current",
            "onboard.provider.saved",
        ] {
            assert!(
                !spec.items.iter().any(|item| item.id == id),
                "read-only `{id}` must move to the info pane, not the left list"
            );
        }
        // ...and that status now rides in the right-side teaching pane.
        let Some(crate::menu::types::MenuPreview::Text { body, .. }) = &spec.preview else {
            panic!("provider step must keep a Text teaching pane");
        };
        assert!(
            body.contains(t!("onboarding.preview.provider.selected").as_ref())
                && body.contains(t!("onboarding.preview.provider.profile").as_ref()),
            "info pane should surface the read-only provider/profile status: {body:?}"
        );
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
        assert_eq!(
            test_item.label,
            t!("onboarding.provider.test_testing").into_owned()
        );
        assert!(test_item.state.loading);
        assert_eq!(test_item.disabled_reason, None);
        let fallback_item = spec
            .items
            .iter()
            .find(|item| item.id == "provider.fallback")
            .expect("provider fallback row");
        assert_eq!(
            fallback_item.label,
            t!("onboarding.provider.fallback_unavailable_testing").into_owned()
        );
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
            onboarding_provider_saved_status_label(&onboarding)
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
        // carries the per-step TEACHING panel (explanatory prose + progress) as
        // its preview so the user sees the full Step-N-of-M path plus what to do
        // from the first screen.
        assert!(
            matches!(
                spec.preview,
                Some(crate::menu::types::MenuPreview::Text { .. })
            ),
            "welcome menu should show the per-step explanation panel"
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
        let AppUiCommand::ReadSessionStatus(params) = appui_command(&refresh.action) else {
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
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::ReadSessionStatus(_))
        ));
        let cost = spec
            .items
            .iter()
            .find(|item| item.id == "cost.estimated")
            .expect("cost item");
        assert_eq!(cost.description.as_deref(), Some("$0.0025"));
    }

    #[test]
    fn resume_menu_is_loading_until_session_list_lands() {
        let capabilities = CapabilitySet::from_methods([methods::SESSION_LIST]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot::default(),
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let result = core_menu_registry().build(&MenuId::from(MENU_RESUME), &ctx);
        assert!(
            matches!(
                result,
                MenuBuildResult::Loading(status) if status.message.contains("Loading")
            ),
            "empty resume_sessions must render a Loading placeholder"
        );
    }

    #[test]
    fn resume_menu_shows_no_sessions_when_loaded_but_empty() {
        // A `session/list` result landed but returned zero prior sessions
        // (`resume_list_loaded == true`, `resume_sessions` empty). The picker
        // must render a terminal "No sessions" placeholder, NOT `Loading`
        // forever (which is indistinguishable from a fetch still in flight).
        let capabilities = CapabilitySet::from_methods([methods::SESSION_LIST]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                resume_list_loaded: true,
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let result = core_menu_registry().build(&MenuId::from(MENU_RESUME), &ctx);
        assert!(
            matches!(
                &result,
                MenuBuildResult::Unavailable(status) if status.message.contains("No prior sessions")
            ),
            "a loaded-but-empty session list must render a No-sessions placeholder, got: {result:?}"
        );
    }

    #[test]
    fn resume_menu_renders_a_row_per_prior_session() {
        let rows = vec![
            crate::model::ResumeSessionRow {
                id: "local:alpha".into(),
                title: Some("Alpha".into()),
                message_count: 5,
                // Ancient timestamp → the relative-time helper's deterministic
                // date fallback, so this row assertion never drifts with wall
                // clock. Recent-bucket rendering is covered separately below.
                updated_at: Some("2020-05-01T00:00:00Z".into()),
                last_prompt: Some("Draft the Q3 deck".into()),
            },
            crate::model::ResumeSessionRow {
                id: "local:bravo".into(),
                title: None,
                message_count: 2,
                updated_at: None,
                last_prompt: None,
            },
        ];
        let capabilities = CapabilitySet::from_methods([methods::SESSION_LIST]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                resume_sessions: &rows,
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let MenuBuildResult::Ready(spec) =
            core_menu_registry().build(&MenuId::from(MENU_RESUME), &ctx)
        else {
            panic!("expected a ready resume menu");
        };
        assert!(spec.searchable, "the picker is searchable");
        assert_eq!(spec.items.len(), 2);
        assert_eq!(
            spec.footer_hint.as_deref(),
            Some("Enter resume · /resume <id> · Esc")
        );

        // Row 0: `{short_id}  {prompt}` label (last_prompt wins over title),
        // "<relative> · N msgs" description, ResumeSession action with the id.
        let alpha = &spec.items[0];
        assert_eq!(alpha.id, "local:alpha");
        // Handle = the id's last segment (no `#topic` here → after the last
        // `:`), so "alpha" — unique, unlike the old shared 6-char prefix.
        assert_eq!(alpha.label, "alpha  Draft the Q3 deck");
        assert_eq!(alpha.description.as_deref(), Some("2020-05-01 · 5 msgs"));
        assert!(matches!(
            &alpha.action,
            MenuAction::Local(LocalAction::ResumeSession(id)) if id == "local:alpha"
        ));

        // Row 1: no prompt/title → "(no preview)"; no updated_at → bare count.
        let bravo = &spec.items[1];
        assert_eq!(bravo.label, "bravo  (no preview)");
        assert_eq!(bravo.description.as_deref(), Some("2 msgs"));
    }

    /// The resume row's description renders a recent `updated_at` through the
    /// relative-time helper (not the raw timestamp), and the label leads with
    /// the short id. Uses an offset from `now` well inside the "hours" bucket so
    /// it stays deterministic across the test's runtime.
    #[test]
    fn resume_menu_row_shows_short_id_and_relative_time() {
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let rows = vec![crate::model::ResumeSessionRow {
            id: "dev:local:tui#alpha".into(),
            title: None,
            message_count: 3,
            updated_at: Some(two_hours_ago),
            last_prompt: Some("Investigate the flaky test".into()),
        }];
        let capabilities = CapabilitySet::from_methods([methods::SESSION_LIST]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                resume_sessions: &rows,
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let MenuBuildResult::Ready(spec) =
            core_menu_registry().build(&MenuId::from(MENU_RESUME), &ctx)
        else {
            panic!("expected a ready resume menu");
        };
        let row = &spec.items[0];
        // Handle is the TOPIC (`#alpha`), not a 6-char id prefix — unique and
        // human-meaningful for canonical `channel:profile:base#topic` ids.
        assert_eq!(row.label, "alpha  Investigate the flaky test");
        assert_eq!(row.description.as_deref(), Some("2h ago · 3 msgs"));
    }

    #[test]
    fn rewind_menu_is_unavailable_without_user_messages() {
        let capabilities = CapabilitySet::from_methods([methods::SESSION_ROLLBACK]);
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot::default(),
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let result = core_menu_registry().build(&MenuId::from(MENU_REWIND), &ctx);
        assert!(
            matches!(
                result,
                MenuBuildResult::Unavailable(status) if status.message.contains("Nothing to rewind")
            ),
            "an empty rewind_turns must render an Unavailable placeholder"
        );
    }

    #[test]
    fn rewind_menu_renders_a_row_per_user_turn_newest_first() {
        // Rows are already newest-first (row 0 = newest user message → the
        // store builds them that way); num_turns is index + 1.
        let rows = vec![
            crate::model::RewindTurnRow {
                preview: "third question".into(),
                num_turns: 1,
                prefill: "third question in full".into(),
                checkpoint: 1,
                // Ancient timestamp → deterministic date fallback for the row
                // assertion below (recent buckets are covered by the pure
                // relative_time unit tests in store.rs).
                timestamp: Some("2020-05-01T00:00:00Z".into()),
            },
            crate::model::RewindTurnRow {
                preview: "second question".into(),
                num_turns: 2,
                prefill: "second question in full".into(),
                checkpoint: 2,
                // No timestamp → the description omits the datetime.
                timestamp: None,
            },
            crate::model::RewindTurnRow {
                preview: "first question".into(),
                num_turns: 3,
                prefill: "first question in full".into(),
                checkpoint: 3,
                timestamp: Some("2020-05-01T00:00:00Z".into()),
            },
        ];
        let capabilities = CapabilitySet::from_methods([methods::SESSION_ROLLBACK]);
        let session_id = SessionKey("local:test".into());
        let ctx = MenuContext {
            availability: AvailabilityContext::protocol(&capabilities),
            app: MenuAppSnapshot {
                rewind_turns: &rows,
                selected_session_id: Some(&session_id),
                ..MenuAppSnapshot::default()
            },
            terminal: TerminalSize::default(),
            theme_name: None,
            selected_path: &[],
        };
        let MenuBuildResult::Ready(spec) =
            core_menu_registry().build(&MenuId::from(MENU_REWIND), &ctx)
        else {
            panic!("expected a ready rewind menu");
        };
        assert!(spec.searchable, "the picker is searchable");
        assert_eq!(spec.mode, MenuMode::SingleSelect);
        assert_eq!(spec.items.len(), 3);
        assert_eq!(
            spec.footer_hint.as_deref(),
            Some("Enter rewind · /rewind <n> · Esc")
        );

        // Row 0 is the newest user message → checkpoint #1 / num_turns 1 (drop
        // the last exchange): `#N  preview` label, "<relative> · drops N turn(s)"
        // description, and the action carries the source session + num_turns +
        // the full prefill (dispatch refuses a pick whose session no longer
        // matches).
        let newest = &spec.items[0];
        assert_eq!(newest.label, "#1  third question");
        assert_eq!(
            newest.description.as_deref(),
            Some("2020-05-01 · drops 1 turn(s)")
        );
        assert!(matches!(
            &newest.action,
            MenuAction::Local(LocalAction::RewindToTurn { session_id, num_turns, prefill })
                if *num_turns == 1
                    && prefill == "third question in full"
                    && session_id == "local:test"
        ));

        // Row 1 has no timestamp → the description omits the datetime.
        let middle = &spec.items[1];
        assert_eq!(middle.label, "#2  second question");
        assert_eq!(middle.description.as_deref(), Some("drops 2 turn(s)"));

        // The oldest user message is last → checkpoint #3 / num_turns 3.
        let oldest = &spec.items[2];
        assert_eq!(oldest.label, "#3  first question");
        assert!(matches!(
            &oldest.action,
            MenuAction::Local(LocalAction::RewindToTurn { num_turns, .. }) if *num_turns == 3
        ));
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
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::ProfileLlmList(_))
        ));
        let select = spec
            .items
            .iter()
            .find(|item| item.label == "DeepSeek V4 Pro")
            .expect("model selection");
        let AppUiCommand::ProfileLlmSelect(params) = appui_command(&select.action) else {
            panic!("expected profile/llm/select action");
        };
        assert_eq!(params.model_id, "deepseek-v4-pro");
        assert_eq!(params.family_id, "deepseek");
        assert_eq!(params.route_id, "coding");
        assert!(select.state.current);
    }

    #[test]
    fn model_menu_marks_exactly_one_active_row() {
        // Bug 3 hardening: the `*` marker must land on exactly one row. Two
        // failure inputs the old id-only `current_model == model.model` OR
        // painted wrong: (1) two entries sharing a model id, and (2) a
        // misbehaving/mock backend that marks every row `selected` (the
        // reported "everything shows *"). A real SSOT backend marks exactly one
        // (verified live), so this is defensive robustness against bad inputs.
        let registry = core_menu_registry();
        let capabilities =
            CapabilitySet::from_methods([APPUI_METHOD_MODEL_LIST, APPUI_METHOD_MODEL_SELECT]);
        let session_id = SessionKey("local:test".into());
        let model = |name: &str, provider: &str, selected: bool| ModelStatus {
            model: name.into(),
            provider: provider.into(),
            title: Some(format!("{provider} / {name}")),
            family: Some(provider.into()),
            route: Some("official".into()),
            selected,
            available: Some(true),
            queue_mode: None,
            qoe_policy: None,
        };
        let marked_ids = |catalog: &SessionModelCatalog, current: Option<&'static str>| {
            let ctx = MenuContext {
                availability: AvailabilityContext::protocol(&capabilities),
                app: MenuAppSnapshot {
                    selected_session_id: Some(&session_id),
                    model_catalog: Some(catalog),
                    current_model: current,
                    ..MenuAppSnapshot::default()
                },
                terminal: TerminalSize::default(),
                theme_name: None,
                selected_path: &[],
            };
            let MenuBuildResult::Ready(spec) = registry.build(&MenuId::from(MENU_MODEL), &ctx)
            else {
                panic!("expected model menu");
            };
            spec.items
                .iter()
                .filter(|item| item.id.starts_with("model.select.") && item.state.current)
                .map(|item| item.id.clone())
                .collect::<Vec<_>>()
        };

        // Same model id via two providers (a real failover config). Only the
        // primary is `selected`; the live model id matches BOTH rows.
        let dup = SessionModelCatalog {
            session_id: session_id.clone(),
            models: vec![
                model("shared-model", "openai", true),
                model("shared-model", "openrouter", false),
            ],
        };
        assert_eq!(
            marked_ids(&dup, Some("shared-model")),
            vec!["model.select.0".to_string()],
            "duplicate model ids must mark only the selected (primary) row",
        );

        // A backend that marks EVERY row selected → client still shows one.
        let all_selected = SessionModelCatalog {
            session_id: session_id.clone(),
            models: vec![
                model("m-a", "openai", true),
                model("m-b", "zai", true),
                model("m-c", "deepseek", true),
            ],
        };
        assert_eq!(
            marked_ids(&all_selected, None).len(),
            1,
            "a multi-selected backend must not paint every row active",
        );

        // No row selected → fall back to the live runtime model (first match).
        let none_selected = SessionModelCatalog {
            session_id: session_id.clone(),
            models: vec![model("m-a", "openai", false), model("m-b", "zai", false)],
        };
        assert_eq!(
            marked_ids(&none_selected, Some("m-b")),
            vec!["model.select.1".to_string()],
            "with nothing selected, the live runtime model marks its row",
        );
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
        let AppUiCommand::ProfileLlmList(params) = appui_command(&refresh.action) else {
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
        let AppUiCommand::ProfileLlmSelect(params) = appui_command(&select.action) else {
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
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::ListMcpStatus(_))
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
        let AppUiCommand::SetMcpConfigEnabled(params) = appui_command(&toggle.action) else {
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
        let AppUiCommand::SetToolConfigEnabled(params) = appui_command(&toggle.action) else {
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
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::ProfileSkillsRemove(_))
        ));
        assert!(remove.state.destructive);

        let install = spec
            .items
            .iter()
            .find(|item| item.id == "skills.registry.news")
            .expect("registry install item");
        let AppUiCommand::ProfileSkillsInstall(params) = appui_command(&install.action) else {
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
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::ListApprovalScopes(_))
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
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::SetPermissionProfile(_))
        ));

        let refresh = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.profile.refresh")
            .expect("profile refresh row");
        assert!(refresh.is_enabled());
        assert!(matches!(
            &refresh.action,
            MenuAction::SendAppUi(command) if matches!(command.as_ref(), AppUiCommand::ListPermissionProfiles(_))
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
        let AppUiCommand::SetPermissionProfile(params) = appui_command(&default.action) else {
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
