//! UPCR-2026-009 `session/hydrate` TUI contract tests.
//!
//! This pins the client-side slice for issue #154: the TUI must only
//! request authoritative session hydration when the server advertises
//! the feature and method, and the result must replace durable chat
//! state instead of being treated as a generic status ack.

use chrono::Utc;
use octos_core::SessionKey;
use octos_core::ui_protocol::{HydratedMessage, SessionHydrateResult, TurnId, UiCursor};
use octos_tui::client_event::ClientEvent;
use octos_tui::menu::CapabilitySet;
use octos_tui::model::{
    APPUI_FEATURE_SESSION_HYDRATE_V1, APPUI_METHOD_SESSION_HYDRATE, AppState, AppUiCommand,
    SessionView,
};
use octos_tui::store::Store;

fn store_with_hydrate_capability() -> Store {
    let session = SessionView {
        id: SessionKey("coding:local:tui#coding".into()),
        title: "coding".into(),
        profile_id: Some("coding".into()),
        messages: vec![],
        tasks: vec![],
        live_reply: None,
    };
    let mut store = Store {
        state: AppState::new(
            vec![session],
            0,
            "ready".into(),
            Some("ws://example.test/ui-protocol".into()),
            false,
        ),
    };
    store.state.capabilities = Some(CapabilitySet::from_methods_and_features(
        [APPUI_METHOD_SESSION_HYDRATE],
        [APPUI_FEATURE_SESSION_HYDRATE_V1],
    ));
    store
}

#[test]
fn session_hydrate_constants_match_upcr_2026_009() {
    assert_eq!(APPUI_FEATURE_SESSION_HYDRATE_V1, "state.session_hydrate.v1");
    assert_eq!(APPUI_METHOD_SESSION_HYDRATE, "session/hydrate");
}

#[test]
fn session_hydrate_command_is_feature_gated() {
    let session_id = SessionKey("coding:local:tui#coding".into());
    let mut store = store_with_hydrate_capability();
    assert!(matches!(
        store.hydrate_session_state_command(&session_id),
        Some(AppUiCommand::HydrateSession(_))
    ));

    store.state.capabilities = Some(CapabilitySet::from_methods([APPUI_METHOD_SESSION_HYDRATE]));
    assert!(store.hydrate_session_state_command(&session_id).is_none());
}

#[test]
fn session_hydrate_result_replaces_durable_messages() {
    let mut store = store_with_hydrate_capability();
    let session_id = SessionKey("coding:local:tui#coding".into());
    let turn_id = TurnId::new();
    let result = SessionHydrateResult {
        session_id: session_id.clone(),
        cursor: UiCursor {
            stream: "session".into(),
            seq: 2,
        },
        context: None,
        context_state: None,
        messages: Some(vec![HydratedMessage {
            seq: 1,
            role: "assistant".into(),
            content: "authoritative answer".into(),
            turn_id: Some(turn_id),
            thread_id: Some("thread-1".into()),
            client_message_id: None,
            persisted_at: Utc::now(),
            message_id: Some("msg-1".into()),
            source: Some("assistant".into()),
            media: Vec::new(),
        }]),
        threads: None,
        turns: None,
        pending_approvals: None,
        pending_questions: None,
        replayed_envelopes: None,
    };

    store.apply_client_event(ClientEvent::SessionHydrate(result));

    let session = store.state.active_session().expect("active session");
    assert_eq!(session.messages.len(), 1);
    assert_eq!(session.messages[0].content, "authoritative answer");
    assert!(store.state.status.contains("Session hydrated"));
}
