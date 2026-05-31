//! UPCR-2026-019 `review/start` TUI contract tests.
//!
//! This pins the final issue #154 surface: the TUI must expose a
//! user-reachable review trigger only when the server advertises the
//! review feature and method, and it must fold the accepted workflow
//! result into visible local state while the existing supervised
//! task/agent notifications render the launched specialists.

use octos_core::SessionKey;
use octos_core::ui_protocol::TurnId;
use octos_tui::client_event::ClientEvent;
use octos_tui::menu::CapabilitySet;
use octos_tui::model::{
    APPUI_FEATURE_REVIEW_START_V1, APPUI_METHOD_REVIEW_START, AppState, AppUiCommand,
    ReviewStartResult, SessionView,
};
use octos_tui::store::Store;

fn store_with_review_capability() -> Store {
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
        [APPUI_METHOD_REVIEW_START],
        [APPUI_FEATURE_REVIEW_START_V1],
    ));
    store
}

#[test]
fn review_start_constants_match_upcr_2026_019() {
    assert_eq!(APPUI_FEATURE_REVIEW_START_V1, "review.start.v1");
    assert_eq!(APPUI_METHOD_REVIEW_START, "review/start");
}

#[test]
fn review_slash_is_feature_gated() {
    let mut store = store_with_review_capability();
    store.state.composer = "/review".into();
    assert!(matches!(
        store.compose_command(),
        Some(AppUiCommand::StartReview(_))
    ));

    store.state.capabilities = Some(CapabilitySet::from_methods([APPUI_METHOD_REVIEW_START]));
    store.state.composer = "/review".into();
    assert!(store.compose_command().is_none());
    assert!(store.state.status.contains(APPUI_FEATURE_REVIEW_START_V1));
}

#[test]
fn review_start_result_is_visible_state() {
    let mut store = store_with_review_capability();
    let turn_id = TurnId::new();
    store.apply_client_event(ClientEvent::ReviewStart(ReviewStartResult {
        accepted: true,
        session_id: SessionKey("coding:local:tui#coding".into()),
        turn_id: turn_id.clone(),
        workflow: Some("code_review".into()),
        backend: Some("native".into()),
        agent_count: Some(3),
    }));

    assert!(store.state.status.contains("3 specialist"));
    assert_eq!(store.state.run_state.label(), "running");
    let activity = store.state.activity.last().expect("review activity");
    assert_eq!(activity.title, "code review");
    assert_eq!(activity.turn_id.as_ref(), Some(&turn_id));
}
