use octos_core::SessionKey;
use octos_core::ui_protocol::SessionOpenParams;
use octos_tui::model::{
    APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1, effective_workspace_root_for_display,
    scrub_session_open_cwd_for_capabilities, session_open_may_include_cwd,
};

/// Guard test: the capability flag constant the TUI uses to gate
/// session/open's `cwd` field must stay `"session.workspace_cwd.v1"`
/// (UPCR-2026-003). Naming drift would either skip the gate or
/// reject compliant servers.
#[test]
fn m12_workspace_cwd_capability_constant_matches_spec() {
    assert_eq!(
        APPUI_FEATURE_SESSION_WORKSPACE_CWD_V1,
        "session.workspace_cwd.v1"
    );
}

#[test]
fn session_open_may_include_cwd_when_feature_advertised() {
    let features = vec![
        "approval.typed.v1".to_string(),
        "session.workspace_cwd.v1".to_string(),
    ];
    assert!(session_open_may_include_cwd(&features));
}

#[test]
fn session_open_must_not_include_cwd_without_feature() {
    let features = vec!["approval.typed.v1".to_string()];
    assert!(!session_open_may_include_cwd(&features));
    let empty: Vec<String> = Vec::new();
    assert!(!session_open_may_include_cwd(&empty));
}

/// Guard test: when the server has NOT advertised
/// `session.workspace_cwd.v1`, `scrub_session_open_cwd_for_capabilities`
/// erases the requested cwd before serialization. Compatible-but-old
/// servers would otherwise silently ignore it and run against the
/// wrong root.
#[test]
fn scrub_session_open_cwd_when_feature_absent() {
    let params = SessionOpenParams {
        session_id: SessionKey("local:test".into()),
        topic: None,
        profile_id: Some("ada-server".into()),
        cwd: Some("/tmp/solo-project".into()),
        after: None,
    };
    let supported: Vec<String> = Vec::new();
    let scrubbed = scrub_session_open_cwd_for_capabilities(params, &supported);
    assert!(
        scrubbed.cwd.is_none(),
        "cwd must be stripped when the feature is missing"
    );
    assert_eq!(scrubbed.session_id, SessionKey("local:test".into()));
    assert_eq!(scrubbed.profile_id.as_deref(), Some("ada-server"));
}

/// Guard test: when the server HAS advertised the feature, cwd
/// round-trips unchanged.
#[test]
fn scrub_session_open_cwd_passthrough_when_feature_present() {
    let params = SessionOpenParams {
        session_id: SessionKey("local:test".into()),
        topic: None,
        profile_id: Some("ada-server".into()),
        cwd: Some("/tmp/solo-project".into()),
        after: None,
    };
    let supported = vec!["session.workspace_cwd.v1".to_string()];
    let scrubbed = scrub_session_open_cwd_for_capabilities(params, &supported);
    assert_eq!(scrubbed.cwd.as_deref(), Some("/tmp/solo-project"));
}

/// Guard test: `effective_workspace_root_for_display` prefers the
/// server-confirmed `workspace_root` from `session/status/read` over
/// the locally-requested `cwd`. The TUI must NEVER silently
/// substitute the requested cwd for the server truth — render what
/// the server said.
#[test]
fn workspace_root_display_prefers_server_truth() {
    assert_eq!(
        effective_workspace_root_for_display(Some("/server/wins"), Some("/requested/cwd"),),
        Some("/server/wins"),
    );
    assert_eq!(
        effective_workspace_root_for_display(Some("/server/only"), None),
        Some("/server/only"),
    );
    // Only when the server is silent does the requested cwd surface.
    assert_eq!(
        effective_workspace_root_for_display(None, Some("/local/fallback")),
        Some("/local/fallback"),
    );
    assert_eq!(effective_workspace_root_for_display(None, None), None);
}
