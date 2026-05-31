//! UPCR-2026-011 `turn/state/get` TUI contract tests.
//!
//! These tests pin the issue-level surface: `/turn state` must be
//! gated on `state.turn_state_get.v1`, dispatch the `turn/state/get`
//! RPC for the active turn, and fold the response into a rendered
//! detail surface.

use octos_core::SessionKey;
use octos_core::ui_protocol::{TurnId, TurnLifecycleState, TurnStateGetResult};
use octos_tui::client_event::{AutonomyClientEvent, AutonomyResult, ClientEvent};
use octos_tui::menu::CapabilitySet;
use octos_tui::model::{
    APPUI_FEATURE_TURN_STATE_GET_V1, APPUI_METHOD_TURN_STATE_GET, AppState, AppUiCommand,
    LiveReply, SessionView,
};
use octos_tui::store::Store;

fn store_with_live_turn(turn_id: TurnId) -> Store {
    let session = SessionView {
        id: SessionKey("coding:local:tui#coding".into()),
        title: "coding".into(),
        profile_id: Some("coding".into()),
        messages: vec![],
        tasks: vec![],
        live_reply: Some(LiveReply {
            turn_id,
            text: "working".into(),
        }),
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
        [APPUI_METHOD_TURN_STATE_GET],
        [APPUI_FEATURE_TURN_STATE_GET_V1],
    ));
    store
}

#[test]
fn turn_state_constants_match_upcr_2026_011() {
    assert_eq!(APPUI_FEATURE_TURN_STATE_GET_V1, "state.turn_state_get.v1");
    assert_eq!(APPUI_METHOD_TURN_STATE_GET, "turn/state/get");
}

#[test]
fn turn_state_slash_dispatches_active_turn_when_advertised() {
    let turn_id = TurnId::new();
    let mut store = store_with_live_turn(turn_id.clone());
    store.state.composer = "/turn state".into();

    match store.compose_command().expect("dispatch") {
        AppUiCommand::GetTurnState(params) => {
            assert_eq!(
                params.session_id,
                SessionKey("coding:local:tui#coding".into())
            );
            assert_eq!(params.turn_id, turn_id);
        }
        other => panic!("expected GetTurnState, got {other:?}"),
    }
}

#[test]
fn turn_state_slash_is_hidden_without_feature() {
    let turn_id = TurnId::new();
    let mut store = store_with_live_turn(turn_id);
    store.state.capabilities = Some(CapabilitySet::from_methods([APPUI_METHOD_TURN_STATE_GET]));
    store.state.composer = "/turn state".into();

    assert!(store.compose_command().is_none());
    assert!(store.state.status.contains(APPUI_FEATURE_TURN_STATE_GET_V1));
}

#[test]
fn turn_state_result_opens_detail_surface() {
    let turn_id = TurnId::new();
    let mut store = store_with_live_turn(turn_id.clone());

    store.apply_client_event(ClientEvent::Autonomy(AutonomyClientEvent {
        result: AutonomyResult::TurnState(TurnStateGetResult {
            session_id: SessionKey("coding:local:tui#coding".into()),
            turn_id: turn_id.clone(),
            state: TurnLifecycleState::Active,
            context: None,
            context_state: None,
            started_at: None,
            completed_at: None,
            thread_id: Some("thread-1".into()),
            committed_seqs: vec![7, 8],
        }),
    }));

    assert!(store.state.turn_state_detail.active);
    assert!(
        store
            .state
            .turn_state_detail
            .subtitle
            .contains(&turn_id.0.to_string())
    );
    assert!(
        store
            .state
            .turn_state_detail
            .content
            .contains("state: active")
    );
    assert!(
        store
            .state
            .turn_state_detail
            .content
            .contains("thread: thread-1")
    );
    assert!(
        store
            .state
            .turn_state_detail
            .content
            .contains("committed seqs: 7, 8")
    );
}
