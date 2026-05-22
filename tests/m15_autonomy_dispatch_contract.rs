//! M15-E dispatch + hydration contract tests (UPCR-2026-021).
//!
//! These tests pin the high-level invariants the rest of the
//! workspace depends on:
//!
//! * `/agents`, `/goal`, `/loop` go through the TUI dispatch surface
//!   and emit AppUI commands, not generic `agent/spawn` calls or
//!   local scheduler activity.
//! * Capability gating (`coding.autonomy.v1`) hides the slash
//!   commands when the server does not advertise the autonomy
//!   surface — old servers must never be probed for unsupported
//!   methods.
//! * Reconnect hydration re-requests agent/goal/loop state from the
//!   backend; local config must never be used to fill it in.

use octos_core::SessionKey;
use octos_core::app_ui::AppUiEvent;
use octos_core::ui_protocol::{SessionOpened, UiNotification};

use octos_tui::menu::CapabilitySet;
use octos_tui::model::{
    APPUI_FEATURE_CODING_AUTONOMY_V1, APPUI_METHOD_AGENT_LIST, APPUI_METHOD_LOOP_LIST,
    APPUI_METHOD_SESSION_GOAL_GET, AgentListParams, AppState, AppUiCommand, LoopListParams,
    SessionGoalGetParams, SessionView,
};
use octos_tui::store::Store;

fn store_with_autonomy_session() -> Store {
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
        [
            APPUI_METHOD_AGENT_LIST,
            APPUI_METHOD_SESSION_GOAL_GET,
            APPUI_METHOD_LOOP_LIST,
        ],
        [APPUI_FEATURE_CODING_AUTONOMY_V1],
    ));
    store
}

/// `/agents` dispatches an `agent/list` RPC. The TUI must NEVER fall
/// through to a generic `agent/spawn` scheduling path.
#[test]
fn agents_slash_dispatches_agent_list_when_advertised() {
    let mut store = store_with_autonomy_session();
    store.state.composer = "/agents".into();
    let command = store.compose_command().expect("dispatched");
    match command {
        AppUiCommand::ListAgents(AgentListParams {
            session_id,
            parent_agent_id,
        }) => {
            assert_eq!(session_id, SessionKey("coding:local:tui#coding".into()));
            assert!(parent_agent_id.is_none());
        }
        other => panic!("expected ListAgents, got {other:?}"),
    }
}

/// `/goal` (bare) dispatches `session/goal/get`. The TUI never emits a
/// `session/goal/set` to fetch state.
#[test]
fn goal_bare_slash_dispatches_session_goal_get_when_advertised() {
    let mut store = store_with_autonomy_session();
    store.state.composer = "/goal".into();
    let command = store.compose_command().expect("dispatched");
    match command {
        AppUiCommand::GetSessionGoal(SessionGoalGetParams {
            session_id,
            profile_id,
        }) => {
            assert_eq!(session_id, SessionKey("coding:local:tui#coding".into()));
            assert_eq!(profile_id.as_deref(), Some("coding"));
        }
        other => panic!("expected GetSessionGoal, got {other:?}"),
    }
}

/// `/loop list` dispatches `loop/list`. Slash command syntax must
/// resolve to a backend RPC; no local scheduler is involved.
#[test]
fn loop_list_slash_dispatches_loop_list_when_advertised() {
    let mut store = store_with_autonomy_session();
    store.state.composer = "/loop list".into();
    let command = store.compose_command().expect("dispatched");
    match command {
        AppUiCommand::ListLoops(LoopListParams { session_id, .. }) => {
            assert_eq!(session_id, SessionKey("coding:local:tui#coding".into()));
        }
        other => panic!("expected ListLoops, got {other:?}"),
    }
}

/// Capability gating: without `coding.autonomy.v1`, `/agents`,
/// `/goal`, and `/loop` are hidden. The TUI must NOT probe unsupported
/// methods.
#[test]
fn autonomy_slashes_hidden_when_feature_absent() {
    let mut store = store_with_autonomy_session();
    // Strip the feature but keep the methods. The registry gate
    // depends on the feature, so the commands must hide.
    store.state.capabilities = Some(CapabilitySet::from_methods([
        APPUI_METHOD_AGENT_LIST,
        APPUI_METHOD_SESSION_GOAL_GET,
        APPUI_METHOD_LOOP_LIST,
    ]));

    for slash in ["/agents", "/goal", "/loop"] {
        store.state.composer = slash.into();
        assert!(
            store.compose_command().is_none(),
            "{slash} must be hidden when coding.autonomy.v1 is missing"
        );
    }
}

/// Capability gating: with the feature but with NO methods advertised,
/// the slash commands also hide. We never probe `agent/list` etc. on a
/// server that says it has none of them.
#[test]
fn autonomy_slashes_hidden_when_no_methods_advertised() {
    let mut store = store_with_autonomy_session();
    // Feature only, zero methods.
    store.state.capabilities = Some(CapabilitySet::from_methods_and_features(
        std::iter::empty::<&str>(),
        [APPUI_FEATURE_CODING_AUTONOMY_V1],
    ));
    for slash in ["/agents", "/goal", "/loop"] {
        store.state.composer = slash.into();
        assert!(
            store.compose_command().is_none(),
            "{slash} must hide when no autonomy methods are advertised"
        );
    }
}

/// Reconnect hydration: SessionOpened with `coding.autonomy.v1`
/// advertised must enqueue agent/list + session/goal/get + loop/list.
/// On a server without the feature, the hydration MUST be empty.
#[test]
fn session_open_enqueues_autonomy_hydration_when_advertised() {
    let mut store = store_with_autonomy_session();
    store.state.sessions.clear();
    let session_id = SessionKey("coding:local:tui#coding".into());
    let opened: SessionOpened = serde_json::from_value(serde_json::json!({
        "session_id": session_id,
        "active_profile_id": "coding",
        "workspace_root": null,
        "cursor": null,
        "panes": null,
    }))
    .expect("session_opened payload");
    store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));
    assert_eq!(store.state.pending_autonomy_hydration.len(), 3);
}

/// Reconnect hydration is no-op when the feature is absent. This is
/// the anti-probe guard.
#[test]
fn session_open_does_not_hydrate_without_autonomy_feature() {
    let mut store = store_with_autonomy_session();
    store.state.sessions.clear();
    // Strip the autonomy feature; we still advertise the methods (the
    // hydration path must depend on the feature flag, not on raw
    // method presence).
    store.state.capabilities = Some(CapabilitySet::from_methods([
        APPUI_METHOD_AGENT_LIST,
        APPUI_METHOD_SESSION_GOAL_GET,
        APPUI_METHOD_LOOP_LIST,
    ]));
    let session_id = SessionKey("coding:local:tui#coding".into());
    let opened: SessionOpened = serde_json::from_value(serde_json::json!({
        "session_id": session_id,
        "active_profile_id": "coding",
        "workspace_root": null,
        "cursor": null,
        "panes": null,
    }))
    .expect("session_opened payload");
    store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));
    assert_eq!(store.state.pending_autonomy_hydration.len(), 0);
}
