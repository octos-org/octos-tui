//! Contract tests for the onboarding saved-provider hydration
//! (`specs/task-onboarding-saved-provider.spec`).
//!
//! A profile that already has an LLM provider saved (e.g. moonshot/kimi) must
//! never read as "not set" in the provider-setup wizard: the wizard hydrates
//! `profile/llm/list` automatically when a profile is resolved, and the rows
//! fall back to the server-saved values (draft-first, saved-fallback) — the
//! TUI displays server truth, it never fakes it.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::ui_protocol::UiProtocolCapabilities;
use octos_tui::client_event::{CapabilitiesClientEvent, ClientEvent, ProfileLlmListClientEvent};
use octos_tui::event_loop::{KeyAction, handle_terminal_event};
use octos_tui::menu::MenuBuildResult;
use octos_tui::model::{
    APPUI_METHOD_MODEL_LIST, APPUI_METHOD_PROFILE_LLM_CATALOG, APPUI_METHOD_PROFILE_LOCAL_CREATE,
    AppState, AppUiCommand, ConfigCapabilitiesListResult, ProfileLlmListResult,
};
use octos_tui::store::Store;
use serde_json::json;

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
                    APPUI_METHOD_MODEL_LIST,
                ],
                &[],
            ),
        },
        message: "Octos UI capabilities refreshed".into(),
    }));
    store
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn run_command(store: &mut Store, command: &str) -> KeyAction {
    store.state.set_composer_text(command);
    handle_terminal_event(store, key(KeyCode::Enter))
}

fn saved_moonshot_state(profile_id: &str) -> ProfileLlmListResult {
    serde_json::from_value(json!({
        "profile_id": profile_id,
        "primary": {
            "provider": "moonshot",
            "model": "kimi-k2.6",
            "family_id": "moonshot",
            "model_id": "kimi-k2.6",
            "route_id": "moonshot",
            "has_api_key": true
        }
    }))
    .expect("llm state fixture parses")
}

fn apply_llm_state(store: &mut Store, result: ProfileLlmListResult) {
    store.apply_client_event(ClientEvent::ProfileLlmList(ProfileLlmListClientEvent {
        result,
        message: "Configured providers refreshed: 1 provider".into(),
    }));
}

fn menu_label(store: &Store, item_id: &str) -> String {
    let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
        panic!("expected an open, ready menu");
    };
    spec.items
        .iter()
        .find(|item| item.id == item_id)
        .unwrap_or_else(|| {
            let ids: Vec<_> = spec.items.iter().map(|item| item.id.as_str()).collect();
            panic!("item {item_id} not in menu; items: {ids:?}")
        })
        .label
        .clone()
}

#[test]
fn onboard_open_with_profile_hydrates_saved_provider() {
    let mut store = first_launch_store();

    // Resolving a profile advances the wizard to provider setup AND must
    // fetch that profile's saved LLM state.
    let action = run_command(&mut store, "/onboard profile alex");

    let KeyAction::Send(AppUiCommand::ProfileLlmList(params)) = action else {
        panic!("resolving a profile must hydrate profile/llm/list");
    };
    assert_eq!(params.profile_id.as_deref(), Some("alex"));

    // Re-opening the wizard while the state is still missing re-requests it.
    let action = run_command(&mut store, "/onboard");
    assert!(
        matches!(action, KeyAction::Send(AppUiCommand::ProfileLlmList(_))),
        "/onboard with a resolved profile but no llm state must hydrate"
    );
}

#[test]
fn hydrate_is_idempotent_for_current_profile() {
    let mut store = first_launch_store();
    run_command(&mut store, "/onboard profile alex");
    apply_llm_state(&mut store, saved_moonshot_state("alex"));

    let action = run_command(&mut store, "/onboard");

    assert!(
        !matches!(action, KeyAction::Send(AppUiCommand::ProfileLlmList(_))),
        "llm state for the current profile is already hydrated; no re-request"
    );
}

#[test]
fn provider_rows_fall_back_to_saved_values() {
    let mut store = first_launch_store();
    run_command(&mut store, "/onboard profile alex");
    apply_llm_state(&mut store, saved_moonshot_state("alex"));

    let family = menu_label(&store, "onboard.provider.family");
    assert!(
        family.contains("moonshot") && family.contains("saved"),
        "family row must show the saved value with a saved marker; label: {family:?}"
    );
    let model = menu_label(&store, "onboard.provider.model");
    assert!(
        model.contains("kimi-k2.6") && model.contains("saved"),
        "model row must show the saved value with a saved marker; label: {model:?}"
    );
    let api_key = menu_label(&store, "onboard.provider.key");
    assert!(
        api_key.contains("saved in profile"),
        "api key row must show the server-confirmed saved key; label: {api_key:?}"
    );
}

#[test]
fn draft_values_override_saved_display() {
    let mut store = first_launch_store();
    run_command(&mut store, "/onboard profile alex");
    apply_llm_state(&mut store, saved_moonshot_state("alex"));

    // The user starts editing: the local draft wins over the saved value.
    run_command(&mut store, "/onboard family deepseek");

    let family = menu_label(&store, "onboard.provider.family");
    assert!(
        family.contains("deepseek") && !family.contains("saved"),
        "a non-empty draft must override the saved display; label: {family:?}"
    );
}

#[test]
fn rows_show_not_set_without_saved_provider() {
    let mut store = first_launch_store();
    run_command(&mut store, "/onboard profile alex");
    // Server answers with NO saved primary for this profile.
    apply_llm_state(
        &mut store,
        serde_json::from_value(json!({ "profile_id": "alex" })).expect("empty state parses"),
    );

    let family = menu_label(&store, "onboard.provider.family");
    assert!(
        family.contains("not set"),
        "without server-saved state the wizard must not invent values; label: {family:?}"
    );
    let model = menu_label(&store, "onboard.provider.model");
    assert!(model.contains("not set"));
}
