//! Contract tests for the onboarding escape hatches
//! (`specs/task-onboarding-exit-and-existing-profile.spec`).
//!
//! The first-launch wizard auto-opens and deliberately swallows Esc (closing
//! it would strand the user on an empty screen), so it MUST offer visible
//! ways out: an "use existing profile (ID)" edit row that routes the wizard
//! straight to provider setup, and an explicit Exit row. Both reuse existing
//! machinery (`/onboard profile` → `SetProfileId`, `LocalAction::Exit` →
//! `exit_requested`) — these tests pin the discoverability surface.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::ui_protocol::UiProtocolCapabilities;
use octos_tui::client_event::{CapabilitiesClientEvent, ClientEvent};
use octos_tui::event_loop::{KeyAction, handle_terminal_event};
use octos_tui::menu::MenuBuildResult;
use octos_tui::model::{
    APPUI_METHOD_PROFILE_LLM_CATALOG, APPUI_METHOD_PROFILE_LOCAL_CREATE, AppState,
    ConfigCapabilitiesListResult,
};
use octos_tui::store::Store;

/// A first-launch store: no sessions, backend advertising the local-solo
/// profile-create surface. Applying the capabilities event auto-opens the
/// onboarding wizard on the create-profile step.
fn first_launch_store() -> Store {
    let mut store = Store {
        state: AppState::new(
            vec![],
            0,
            "starting".into(),
            Some("stdio:octos serve --stdio --solo".into()),
            false,
        ),
    };
    store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
        result: ConfigCapabilitiesListResult {
            capabilities: UiProtocolCapabilities::new(
                &[
                    APPUI_METHOD_PROFILE_LOCAL_CREATE,
                    APPUI_METHOD_PROFILE_LLM_CATALOG,
                ],
                &[],
            ),
        },
        message: "Octos UI capabilities refreshed".into(),
    }));
    store
}

fn active_menu_item_ids(store: &Store) -> Vec<String> {
    let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
        panic!(
            "expected an open, ready menu; got {:?}",
            store.state.active_menu.is_some()
        );
    };
    spec.items.iter().map(|item| item.id.to_string()).collect()
}

fn select_item(store: &mut Store, item_id: &str) {
    let ids = active_menu_item_ids(store);
    let target = ids
        .iter()
        .position(|id| id == item_id)
        .unwrap_or_else(|| panic!("item {item_id} not in menu; items: {ids:?}"));
    // Walk selection from the top so navigation goes through the same public
    // path the user's Down key uses.
    for _ in 0..ids.len() {
        let Some(MenuBuildResult::Ready(_)) = store.state.active_menu.as_ref() else {
            panic!("menu closed during navigation");
        };
        let selected = store
            .state
            .menu_stack
            .active()
            .expect("active menu frame")
            .selected_index;
        if selected == target {
            return;
        }
        store.select_next_menu_item();
    }
    panic!("could not reach item {item_id}");
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn onboarding_create_menu_offers_existing_profile_row() {
    let store = first_launch_store();

    let ids = active_menu_item_ids(&store);
    assert!(
        ids.iter().any(|id| id == "onboard.local.profile_id"),
        "create form must offer the existing-profile edit row; items: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == "onboard.local.exit"),
        "create form must offer a visible exit row; items: {ids:?}"
    );
}

#[test]
fn existing_profile_id_skips_creation_step() {
    let mut store = first_launch_store();

    // The user fills the existing-profile row (same path as the
    // `/onboard profile <id>` typed command).
    store.state.set_composer_text("/onboard profile alex");
    handle_terminal_event(&mut store, key(KeyCode::Enter));

    assert_eq!(store.state.onboarding.profile_id.as_deref(), Some("alex"));
    let ids = active_menu_item_ids(&store);
    assert!(
        !ids.iter().any(|id| id == "onboard.local.create"),
        "the create-profile step must be skipped once a profile id resolves; items: {ids:?}"
    );
    assert!(
        ids.iter()
            .any(|id| id.starts_with("onboard.provider.") || id.starts_with("onboard.catalog.")),
        "the wizard must land on the provider-setup step; items: {ids:?}"
    );
}

#[test]
fn onboarding_exit_row_requests_app_exit() {
    let mut store = first_launch_store();

    select_item(&mut store, "onboard.local.exit");
    let action = handle_terminal_event(&mut store, key(KeyCode::Enter));

    assert!(
        store.state.exit_requested,
        "the exit row must stage the app-exit request"
    );
    assert!(
        matches!(action, KeyAction::Quit),
        "the event loop must translate the staged exit into Quit"
    );
}

#[test]
fn empty_profile_id_keeps_creation_step() {
    let mut store = first_launch_store();

    store.state.set_composer_text("/onboard profile    ");
    let action = handle_terminal_event(&mut store, key(KeyCode::Enter));

    assert!(
        store.state.onboarding.profile_id.is_none(),
        "whitespace-only profile id must not resolve"
    );
    let ids = active_menu_item_ids(&store);
    assert!(
        ids.iter().any(|id| id == "onboard.local.create"),
        "the wizard must stay on the create-profile step; items: {ids:?}"
    );
    assert!(
        !matches!(action, KeyAction::Send(_)),
        "an empty profile id must not emit any backend request"
    );
}

#[test]
fn escape_still_keeps_onboarding_open() {
    let mut store = first_launch_store();

    handle_terminal_event(&mut store, key(KeyCode::Esc));

    assert!(
        store.state.menu_stack.is_active(),
        "Esc must not close the auto-opened onboarding wizard (it would strand the user)"
    );
    let ids = active_menu_item_ids(&store);
    assert!(ids.iter().any(|id| id == "onboard.local.exit"));
}

#[test]
fn provider_setup_step_offers_exit_row() {
    let mut store = first_launch_store();

    // Resolve an existing profile so the wizard advances to provider setup —
    // the step the user lands on when launching with a known profile but no
    // open session yet.
    store.state.set_composer_text("/onboard profile alex");
    handle_terminal_event(&mut store, key(KeyCode::Enter));

    let ids = active_menu_item_ids(&store);
    assert!(
        !ids.iter().any(|id| id == "onboard.local.create"),
        "precondition: wizard must be on provider setup; items: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == "onboard.local.exit"),
        "provider setup also swallows Esc while no session is open, so it must \
         offer the exit row too; items: {ids:?}"
    );

    select_item(&mut store, "onboard.local.exit");
    let action = handle_terminal_event(&mut store, key(KeyCode::Enter));
    assert!(store.state.exit_requested);
    assert!(matches!(action, KeyAction::Quit));
}
