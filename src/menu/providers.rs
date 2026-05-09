//! Local and capability-backed menu providers for the M9.34 framework.
//!
//! Providers use only the canonical `menu::types` contract. This prevents the
//! menu registry, renderer, and store from drifting into parallel type systems.

use crossterm::event::{KeyCode, KeyModifiers};
use octos_core::{
    app_ui::AppUiCommand,
    ui_protocol::{ApprovalScopesListParams, methods},
};

use crate::menu::{
    AppUiActionKind, AvailabilityStatus, ClientEffect, CommandEntry, CommandRegistry, KeyBinding,
    LocalAction, MenuAction, MenuAppSnapshot, MenuBuildResult, MenuContext, MenuId, MenuItem,
    MenuItemState, MenuMode, MenuPreview, MenuPreviewRow, MenuProvider, MenuRegistry, MenuSpec,
    MenuStatusSpec, MenuTab,
    registry::{
        APPUI_METHOD_APPROVAL_SCOPES_CLEAR, APPUI_METHOD_PERMISSION_PROFILE_LIST,
        APPUI_METHOD_PERMISSION_PROFILE_SET, MENU_HELP, MENU_KEYMAP, MENU_MCP, MENU_MODEL,
        MENU_PERMISSIONS, MENU_STATUS, MENU_STATUS_LINE, MENU_THEME, MENU_TITLE,
    },
};
use crate::permission_profile::{
    PermissionNetworkPolicy, PermissionProfileMode, PermissionProfileSelection,
    PermissionProfileUpdate,
};

pub fn core_menu_registry() -> MenuRegistry {
    let mut registry = MenuRegistry::new();
    for provider in [
        Provider::Help,
        Provider::Theme,
        Provider::StatusLine,
        Provider::Title,
        Provider::Keymap,
        Provider::Status,
        Provider::Model,
        Provider::Permissions,
        Provider::Mcp,
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
    Theme,
    StatusLine,
    Title,
    Keymap,
    Status,
    Model,
    Permissions,
    Mcp,
}

impl MenuProvider for Provider {
    fn id(&self) -> MenuId {
        MenuId::from(match self {
            Self::Help => MENU_HELP,
            Self::Theme => MENU_THEME,
            Self::StatusLine => MENU_STATUS_LINE,
            Self::Title => MENU_TITLE,
            Self::Keymap => MENU_KEYMAP,
            Self::Status => MENU_STATUS,
            Self::Model => MENU_MODEL,
            Self::Permissions => MENU_PERMISSIONS,
            Self::Mcp => MENU_MCP,
        })
    }

    fn build(&self, ctx: &MenuContext<'_>) -> MenuBuildResult {
        match self {
            Self::Help => MenuBuildResult::Ready(help_menu(ctx)),
            Self::Theme => MenuBuildResult::Ready(theme_menu(ctx)),
            Self::StatusLine => MenuBuildResult::Ready(status_line_menu(ctx)),
            Self::Title => MenuBuildResult::Ready(title_menu(ctx)),
            Self::Keymap => MenuBuildResult::Ready(keymap_menu()),
            Self::Status => MenuBuildResult::Ready(status_menu(ctx)),
            Self::Model => appui_missing_or_advertised_menu(
                ctx,
                MENU_MODEL,
                "Model",
                AppUiActionKind::ModelList,
                "Model list/select typed AppUI commands are not available in this TUI API yet.",
            ),
            Self::Permissions => permissions_menu(ctx),
            Self::Mcp => appui_missing_or_advertised_menu(
                ctx,
                MENU_MCP,
                "MCP",
                AppUiActionKind::McpStatusList,
                "MCP status typed AppUI commands are not available in this TUI API yet.",
            ),
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
        title: "Slash Commands".into(),
        subtitle: Some("Commands are resolved by the shared registry.".into()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Filter commands".into()),
        footer_hint: Some("Enter open/run | Esc close".into()),
        preview: Some(MenuPreview::Text {
            title: Some("Routing".into()),
            body: "Exact slash commands are handled locally before prompt submission. Unknown slash commands are never sent to the model.".into(),
        }),
        mode: MenuMode::SingleSelect,
    }
}

fn theme_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    let current = ctx.theme_name.unwrap_or("codex");
    let items = [
        ("terminal", "Terminal", "Use terminal defaults."),
        ("codex", "Codex", "Neutral dark palette with blue accents."),
        ("claude", "Claude", "Warm dark palette."),
        ("slate", "Slate", "Cool dark palette."),
        ("solarized", "Solarized", "Solarized dark palette."),
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
        title: "Theme".into(),
        subtitle: Some("Local TUI palette. Does not require AppUI.".into()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Filter themes".into()),
        footer_hint: Some("Enter apply | Esc close".into()),
        preview: Some(MenuPreview::KeyValues {
            title: Some("Current".into()),
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
        "Status Line",
        "Choose bottom status line components.",
        &status_line_items(ctx.app.clone()),
        LocalAction::SaveStatusLine,
    )
}

fn title_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    component_menu(
        MENU_TITLE,
        "Terminal Title",
        "Choose terminal title components.",
        &title_items(ctx.app.clone()),
        LocalAction::SaveTerminalTitle,
    )
}

fn component_menu(
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
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
        title: title.into(),
        subtitle: Some(subtitle.into()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Filter components".into()),
        footer_hint: Some("Space toggle | Enter save | Esc close".into()),
        preview: Some(MenuPreview::Text {
            title: Some("Preview".into()),
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
        ("global.quit", "Ctrl+Q", "Quit the TUI."),
        ("global.interrupt", "Ctrl+C", "Interrupt the active turn."),
        (
            "composer.submit",
            "Enter",
            "Submit composer or exact slash command.",
        ),
        ("menu.accept", "Enter", "Accept highlighted menu row."),
        ("menu.cancel", "Esc", "Close the active menu."),
        ("menu.next", "Down/J", "Move to next row."),
        ("menu.previous", "Up/K", "Move to previous row."),
    ];
    let items = rows
        .into_iter()
        .map(|(id, key, description)| {
            MenuItem::new(id, key, MenuAction::Noop).with_description(description)
        })
        .collect();

    MenuSpec {
        id: MenuId::from(MENU_KEYMAP),
        title: "Keymap".into(),
        subtitle: Some("Current TUI key bindings.".into()),
        items,
        tabs: keymap_tabs(),
        searchable: true,
        search_placeholder: Some("Filter key bindings".into()),
        footer_hint: Some("Esc close".into()),
        preview: Some(MenuPreview::Text {
            title: Some("Editing".into()),
            body: "This slice exposes the menu surface. Persisted keymap editing remains a follow-up provider action.".into(),
        }),
        mode: MenuMode::SingleSelect,
    }
}

fn status_menu(ctx: &MenuContext<'_>) -> MenuSpec {
    let mut items = vec![
        MenuItem::new("status.snapshot", "Snapshot status", MenuAction::Noop)
            .with_description(ctx.app.status.unwrap_or("no status supplied")),
        MenuItem::new("status.connection", "Connection", MenuAction::Noop)
            .with_description(ctx.app.target.unwrap_or("local/offline")),
    ];

    if ctx
        .availability
        .supports_method(AppUiActionKind::SessionStatusRead.method())
    {
        items.push(
            MenuItem::new("status.refresh", "Refresh server status", MenuAction::Noop)
                .disabled("typed AppUiCommand for session/status/read is not available yet"),
        );
    } else {
        items.push(
            MenuItem::new("status.refresh", "Refresh server status", MenuAction::Noop).disabled(
                format!(
                    "AppUI method `{}` is not advertised",
                    AppUiActionKind::SessionStatusRead.method()
                ),
            ),
        );
    }

    items.push(capability_summary_item(ctx));

    MenuSpec {
        id: MenuId::from(MENU_STATUS),
        title: "Status".into(),
        subtitle: Some("Snapshot-backed status; server-owned fields are capability gated.".into()),
        items,
        tabs: Vec::new(),
        searchable: false,
        search_placeholder: None,
        footer_hint: Some("Esc close".into()),
        preview: Some(MenuPreview::KeyValues {
            title: Some("Snapshot".into()),
            rows: app_snapshot_rows(ctx.app.clone()),
        }),
        mode: MenuMode::SingleSelect,
    }
}

fn permissions_menu(ctx: &MenuContext<'_>) -> MenuBuildResult {
    let Some(session_id) = ctx.app.selected_session_id.cloned() else {
        return MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(MENU_PERMISSIONS),
            title: "Permissions unavailable".into(),
            message: "Permissions require an open AppUI session.".into(),
            footer_hint: Some("Esc close".into()),
        });
    };

    let mut items = permission_profile_items(ctx, session_id.clone());
    items.extend(permission_network_items(ctx, session_id.clone()));
    items.push(approval_scopes_refresh_item(ctx, session_id.clone()));
    items.push(approval_scopes_clear_item(ctx));

    MenuBuildResult::Ready(MenuSpec {
        id: MenuId::from(MENU_PERMISSIONS),
        title: "Update Model Permissions".into(),
        subtitle: Some("Session permission presets; mutation is capability gated.".into()),
        items,
        tabs: Vec::new(),
        searchable: true,
        search_placeholder: Some("Filter permissions".into()),
        footer_hint: Some("Enter apply or refresh | Esc close".into()),
        preview: Some(MenuPreview::KeyValues {
            title: Some("Capability Status".into()),
            rows: permission_preview_rows(ctx),
        }),
        mode: MenuMode::SingleSelect,
    })
}

fn permission_profile_items(
    ctx: &MenuContext<'_>,
    session_id: octos_core::SessionKey,
) -> Vec<MenuItem> {
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
            "Default",
            "Workspace edits; ask for network/outside.",
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::WorkspaceWrite),
                    network: Some(PermissionNetworkPolicy::Deny),
                },
            ),
            permission_default_state(ctx.app.permission_profile),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.read_only",
            "Read Only",
            "No writes without approval.",
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::ReadOnly),
                    network: None,
                },
            ),
            permission_mode_state(ctx.app.permission_profile, PermissionProfileMode::ReadOnly),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.workspace_write",
            "Workspace Write",
            "Read/write inside workspace.",
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::WorkspaceWrite),
                    network: None,
                },
            ),
            permission_workspace_write_state(ctx.app.permission_profile),
            mutation_reason.clone(),
        ),
        permission_mode_item(
            "permissions.full_access",
            "Full Access",
            "No sandbox or network approvals.",
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: Some(PermissionProfileMode::DangerFullAccess),
                    network: Some(PermissionNetworkPolicy::Allow),
                },
            ),
            permission_mode_state(
                ctx.app.permission_profile,
                PermissionProfileMode::DangerFullAccess,
            )
            .destructive(),
            mutation_reason,
        ),
        MenuItem::new(
            "permissions.profile.refresh",
            "Refresh permission profiles",
            MenuAction::Noop,
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
    label: &'static str,
    description: &'static str,
    action: MenuAction,
    state: MenuItemState,
    disabled_reason: Option<String>,
) -> MenuItem {
    MenuItem::new(id, label, action)
        .with_description(description)
        .with_state(state)
        .maybe_disabled(disabled_reason)
}

fn permission_default_state(current: Option<PermissionProfileSelection>) -> MenuItemState {
    let default = PermissionProfileSelection {
        mode: PermissionProfileMode::WorkspaceWrite,
        network: PermissionNetworkPolicy::Deny,
    };
    MenuItemState {
        current: current.is_some_and(|current| current.normalized() == default),
        ..MenuItemState::default()
    }
}

fn permission_workspace_write_state(current: Option<PermissionProfileSelection>) -> MenuItemState {
    MenuItemState {
        current: current.is_some_and(|current| {
            let current = current.normalized();
            current.mode == PermissionProfileMode::WorkspaceWrite
                && current.network != PermissionNetworkPolicy::Deny
        }),
        ..MenuItemState::default()
    }
}

fn permission_mode_state(
    current: Option<PermissionProfileSelection>,
    mode: PermissionProfileMode,
) -> MenuItemState {
    MenuItemState {
        current: current.is_some_and(|current| current.normalized().mode == mode),
        ..MenuItemState::default()
    }
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
            "Allow Network",
            permission_set_action(
                session_id.clone(),
                PermissionProfileUpdate {
                    mode: None,
                    network: Some(PermissionNetworkPolicy::Allow),
                },
            ),
        )
        .with_description("Enable internet access.")
        .with_state(MenuItemState::checked(
            ctx.app.permission_profile.is_some_and(|current| {
                current.normalized().network == PermissionNetworkPolicy::Allow
            }),
        ))
        .maybe_disabled(mutation_reason.clone()),
        MenuItem::new(
            "permissions.network.block",
            "Block Network",
            permission_set_action(
                session_id,
                PermissionProfileUpdate {
                    mode: None,
                    network: Some(PermissionNetworkPolicy::Deny),
                },
            ),
        )
        .with_description("Deny internet access.")
        .with_state(MenuItemState::checked(
            ctx.app.permission_profile.is_some_and(|current| {
                current.normalized().network == PermissionNetworkPolicy::Deny
            }),
        ))
        .maybe_disabled(mutation_reason),
    ]
}

fn permission_set_action(
    _session_id: octos_core::SessionKey,
    _update: PermissionProfileUpdate,
) -> MenuAction {
    MenuAction::Noop
}

fn approval_scopes_refresh_item(
    ctx: &MenuContext<'_>,
    session_id: octos_core::SessionKey,
) -> MenuItem {
    let item = MenuItem::new(
        "permissions.scopes.refresh",
        "Refresh persisted approval scopes",
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
        "Clear persisted approval scopes",
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
        Some("permission/profile commands are not exposed by current octos-core".into())
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

fn appui_missing_or_advertised_menu(
    ctx: &MenuContext<'_>,
    id: &'static str,
    title: &'static str,
    action: AppUiActionKind,
    typed_gap: &'static str,
) -> MenuBuildResult {
    if ctx.availability.supports_method(action.method()) {
        MenuBuildResult::Ready(MenuSpec {
            id: MenuId::from(id),
            title: title.into(),
            subtitle: Some("Server-backed capability is advertised.".into()),
            items: vec![
                MenuItem::new(
                    format!("{id}.typed_gap"),
                    "Server API advertised",
                    MenuAction::Noop,
                )
                .disabled(typed_gap),
            ],
            tabs: Vec::new(),
            searchable: false,
            search_placeholder: None,
            footer_hint: Some("Esc close".into()),
            preview: Some(MenuPreview::KeyValues {
                title: Some("Context".into()),
                rows: app_snapshot_rows(ctx.app.clone()),
            }),
            mode: MenuMode::SingleSelect,
        })
    } else {
        MenuBuildResult::Unavailable(MenuStatusSpec {
            id: MenuId::from(id),
            title: format!("{title} unavailable"),
            message: format!(
                "AppUI method `{}` is not advertised by this backend.",
                action.method()
            ),
            footer_hint: Some("Esc close".into()),
        })
    }
}

fn capability_summary_item(ctx: &MenuContext<'_>) -> MenuItem {
    let description = match ctx.availability.capabilities {
        Some(capabilities) => format!(
            "{} method(s), {} feature(s), {} unsupported report(s)",
            capabilities.methods().len(),
            capabilities.features().len(),
            capabilities.unsupported_methods().len()
        ),
        None => "No AppUI capabilities have been advertised yet".into(),
    };
    MenuItem::new("status.capabilities", "Capabilities", MenuAction::Noop)
        .with_description(description)
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
    if aliases.is_empty() {
        description.to_owned()
    } else {
        format!("{description} Aliases: {}", aliases.join(", "))
    }
}

fn status_line_items(app: MenuAppSnapshot<'_>) -> [(&'static str, String, bool); 9] {
    [
        (
            "state",
            format!("State: {}", app.status.unwrap_or("idle")),
            true,
        ),
        (
            "target",
            format!("Target: {}", app.target.unwrap_or("local")),
            true,
        ),
        (
            "cwd",
            format!("Cwd: {}", app.cwd.unwrap_or("unknown")),
            true,
        ),
        (
            "model",
            format!("Model: {}", app.current_model.unwrap_or("not supplied")),
            true,
        ),
        (
            "profile",
            format!("Profile: {}", app.current_profile.unwrap_or("default")),
            true,
        ),
        (
            "session",
            format!("Session: {}", app.selected_session_title.unwrap_or("none")),
            true,
        ),
        (
            "task",
            format!("Task: {}", app.selected_task_title.unwrap_or("none")),
            false,
        ),
        (
            "background_tasks",
            format!("Background: {}", app.background_task_count),
            true,
        ),
        ("approval", "Approval state".into(), true),
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
    ["Global", "Composer", "Menu", "Task", "Approval"]
        .into_iter()
        .enumerate()
        .map(|(idx, label)| MenuTab {
            id: label.to_ascii_lowercase(),
            label: label.into(),
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
    use octos_core::SessionKey;

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
    fn permissions_menu_disables_profile_commands_even_when_legacy_labels_exist() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            methods::APPROVAL_SCOPES_LIST,
            APPUI_METHOD_PERMISSION_PROFILE_LIST,
            APPUI_METHOD_PERMISSION_PROFILE_SET,
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
        assert!(!full_access.is_enabled());
        assert!(matches!(&full_access.action, MenuAction::Noop));
        assert!(
            full_access
                .disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("not exposed by current octos-core"))
        );

        let refresh = spec
            .items
            .iter()
            .find(|item| item.id == "permissions.profile.refresh")
            .expect("profile refresh row");
        assert!(!refresh.is_enabled());
        assert!(matches!(&refresh.action, MenuAction::Noop));
        assert!(
            refresh
                .disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("not exposed by current octos-core"))
        );
    }

    #[test]
    fn permissions_menu_marks_known_permission_profile_state() {
        let registry = core_menu_registry();
        let capabilities = CapabilitySet::from_methods([
            APPUI_METHOD_PERMISSION_PROFILE_LIST,
            APPUI_METHOD_PERMISSION_PROFILE_SET,
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
}
