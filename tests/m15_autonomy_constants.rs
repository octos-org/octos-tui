use octos_tui::model::{
    APPUI_FEATURE_CODING_AGENT_CONTROL_V1, APPUI_FEATURE_CODING_AUTONOMY_V1,
    APPUI_FEATURE_CODING_GOAL_RUNTIME_V1, APPUI_FEATURE_CODING_LOOP_RUNTIME_V1,
    APPUI_METHOD_AGENT_ARTIFACT_LIST, APPUI_METHOD_AGENT_ARTIFACT_READ,
    APPUI_METHOD_AGENT_ARTIFACT_UPDATED, APPUI_METHOD_AGENT_CLOSE, APPUI_METHOD_AGENT_INTERRUPT,
    APPUI_METHOD_AGENT_LIST, APPUI_METHOD_AGENT_OUTPUT_DELTA, APPUI_METHOD_AGENT_OUTPUT_READ,
    APPUI_METHOD_AGENT_STATUS_READ, APPUI_METHOD_AGENT_UPDATED, APPUI_METHOD_LOOP_COMPLETED,
    APPUI_METHOD_LOOP_CREATE, APPUI_METHOD_LOOP_DELETE, APPUI_METHOD_LOOP_FIRE_NOW,
    APPUI_METHOD_LOOP_FIRED, APPUI_METHOD_LOOP_LIST, APPUI_METHOD_LOOP_PAUSE,
    APPUI_METHOD_LOOP_RESUME, APPUI_METHOD_LOOP_UPDATED, APPUI_METHOD_SESSION_GOAL_CLEAR,
    APPUI_METHOD_SESSION_GOAL_CLEARED, APPUI_METHOD_SESSION_GOAL_GET,
    APPUI_METHOD_SESSION_GOAL_SET, APPUI_METHOD_SESSION_GOAL_UPDATED,
};

/// Guard test: the M15-E capability and method constants the TUI uses
/// to gate `/agents`, `/goal`, and `/loop` UX must stay on-spec
/// (UPCR-2026-021).
#[test]
fn m15_autonomy_capabilities_match_upcr_2026_021() {
    assert_eq!(APPUI_FEATURE_CODING_AUTONOMY_V1, "coding.autonomy.v1");
    assert_eq!(
        APPUI_FEATURE_CODING_AGENT_CONTROL_V1,
        "coding.agent_control.v1"
    );
    assert_eq!(
        APPUI_FEATURE_CODING_GOAL_RUNTIME_V1,
        "coding.goal_runtime.v1"
    );
    assert_eq!(
        APPUI_FEATURE_CODING_LOOP_RUNTIME_V1,
        "coding.loop_runtime.v1"
    );
}

/// Guard test: the M15-E request method names. The TUI calls these
/// only when `coding.autonomy.v1` is advertised. Naming drift would
/// either skip gating or call unsupported methods, so each name is
/// pinned.
#[test]
fn m15_agent_inspection_methods_match_spec() {
    assert_eq!(APPUI_METHOD_AGENT_LIST, "agent/list");
    assert_eq!(APPUI_METHOD_AGENT_STATUS_READ, "agent/status/read");
    assert_eq!(APPUI_METHOD_AGENT_OUTPUT_READ, "agent/output/read");
    assert_eq!(APPUI_METHOD_AGENT_ARTIFACT_LIST, "agent/artifact/list");
    assert_eq!(APPUI_METHOD_AGENT_ARTIFACT_READ, "agent/artifact/read");
}

/// Guard test: notification method names the TUI listens for. These
/// are NOT RPC — the TUI may never call them.
#[test]
fn m15_autonomy_notification_methods_match_spec() {
    assert_eq!(APPUI_METHOD_AGENT_UPDATED, "agent/updated");
    assert_eq!(APPUI_METHOD_AGENT_OUTPUT_DELTA, "agent/output/delta");
    assert_eq!(
        APPUI_METHOD_AGENT_ARTIFACT_UPDATED,
        "agent/artifact/updated"
    );
    assert_eq!(APPUI_METHOD_SESSION_GOAL_UPDATED, "session/goal/updated");
    assert_eq!(APPUI_METHOD_SESSION_GOAL_CLEARED, "session/goal/cleared");
    assert_eq!(APPUI_METHOD_LOOP_UPDATED, "loop/updated");
    assert_eq!(APPUI_METHOD_LOOP_FIRED, "loop/fired");
    assert_eq!(APPUI_METHOD_LOOP_COMPLETED, "loop/completed");
}

/// Guard test: M15-E agent control + goal + loop RPC method names.
/// The TUI emits these only when `coding.autonomy.v1` is advertised
/// and the matching method appears in the negotiated capability set
/// (UPCR-2026-021). Naming drift would either skip gating or call
/// unsupported methods.
#[test]
fn m15_autonomy_dispatch_methods_match_spec() {
    assert_eq!(APPUI_METHOD_AGENT_INTERRUPT, "agent/interrupt");
    assert_eq!(APPUI_METHOD_AGENT_CLOSE, "agent/close");
    assert_eq!(APPUI_METHOD_SESSION_GOAL_GET, "session/goal/get");
    assert_eq!(APPUI_METHOD_SESSION_GOAL_SET, "session/goal/set");
    assert_eq!(APPUI_METHOD_SESSION_GOAL_CLEAR, "session/goal/clear");
    assert_eq!(APPUI_METHOD_LOOP_CREATE, "loop/create");
    assert_eq!(APPUI_METHOD_LOOP_LIST, "loop/list");
    assert_eq!(APPUI_METHOD_LOOP_DELETE, "loop/delete");
    assert_eq!(APPUI_METHOD_LOOP_PAUSE, "loop/pause");
    assert_eq!(APPUI_METHOD_LOOP_RESUME, "loop/resume");
    assert_eq!(APPUI_METHOD_LOOP_FIRE_NOW, "loop/fire_now");
}
