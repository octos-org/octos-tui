use octos_tui::model::{
    APPUI_FEATURE_TASK_ARTIFACTS_V1, APPUI_FEATURE_TASK_SUPERVISION_INSPECTION_V1,
    APPUI_METHOD_REVIEW_START, APPUI_METHOD_TASK_ARTIFACT_LIST, APPUI_METHOD_TASK_ARTIFACT_READ,
    SupervisedTaskArtifact, SupervisedTaskEntry,
};
use serde_json::Value;

/// Guard test: capability/method constants the TUI uses to gate M13-D
/// inspection UX must stay on-spec. UPCR-2026-019 §4 defines the wire
/// names — the TUI cannot probe other names or it would either skip
/// gating against compliant servers or call unsupported methods on
/// legacy servers.
#[test]
fn m13_capability_and_method_constants_match_upcr_2026_019() {
    assert_eq!(
        APPUI_FEATURE_TASK_SUPERVISION_INSPECTION_V1,
        "harness.task_supervision_inspection.v1"
    );
    assert_eq!(APPUI_FEATURE_TASK_ARTIFACTS_V1, "harness.task_artifacts.v1");
    assert_eq!(APPUI_METHOD_TASK_ARTIFACT_LIST, "task/artifact/list");
    assert_eq!(APPUI_METHOD_TASK_ARTIFACT_READ, "task/artifact/read");
    assert_eq!(APPUI_METHOD_REVIEW_START, "review/start");
}

/// Guard test: `SupervisedTaskEntry` deserializes the new
/// `source`/`role`/`summary`/`artifact_count`/`runtime_policy_stamp`
/// fields from the wire shape backend sibling shipped on
/// `task/list`/`task/updated` (octos PR #1103). Without this the TUI
/// would either ignore the new metadata or panic on the next live
/// supervised review.
#[test]
fn supervised_task_entries_deserialize_new_supervised_fields() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/m13_supervised_task_list.json"))
            .expect("fixture parses");

    let tasks: Vec<SupervisedTaskEntry> =
        serde_json::from_value(fixture["tasks"].clone()).expect("tasks parse as supervised");
    assert_eq!(tasks.len(), 3, "fixture exposes 3 entries");

    let backend_supervised = tasks.iter().filter(|t| t.is_backend_supervised()).count();
    let user_originated = tasks
        .iter()
        .filter(|t| t.source.as_deref() == Some("user"))
        .count();

    let expected = &fixture["expected"];
    assert_eq!(
        backend_supervised,
        expected["backend_supervised_count"].as_u64().unwrap() as usize,
        "backend-supervised count must match wire shape"
    );
    assert_eq!(
        user_originated,
        expected["user_originated_count"].as_u64().unwrap() as usize,
    );

    let first_model = tasks
        .iter()
        .find(|t| t.source.as_deref() == Some("model"))
        .expect("a model-spawned entry exists");
    assert_eq!(
        first_model.role.as_deref(),
        Some(expected["first_model_role"].as_str().unwrap()),
    );
    assert!(
        first_model.runtime_policy_stamp.is_some(),
        "runtime_policy_stamp must round-trip"
    );

    let first_supervisor = tasks
        .iter()
        .find(|t| t.source.as_deref() == Some("supervisor"))
        .expect("a supervisor-spawned entry exists");
    assert_eq!(
        first_supervisor.summary.as_deref(),
        Some(expected["first_supervisor_summary"].as_str().unwrap()),
    );
    assert_eq!(
        first_supervisor.artifact_count,
        Some(
            expected["first_supervisor_artifact_count"]
                .as_u64()
                .unwrap() as u32
        ),
    );

    // M13 scope explicitly disallows the TUI inventing supervised state
    // from `user`-sourced tasks (which are not M13 supervised children).
    let user_entry = tasks
        .iter()
        .find(|t| t.source.as_deref() == Some("user"))
        .expect("a user-originated entry exists");
    assert!(
        !user_entry.is_backend_supervised(),
        "user-sourced tasks must not be classified as backend-supervised"
    );
}

/// Guard test: `display_label` prefers `role` when present (so the TUI
/// renders "Reviewer running" instead of `spawn_agent running`), then
/// falls back to `tool_name`, then `"task"`. The TUI must never invent
/// labels from runtime detail strings.
#[test]
fn display_label_prefers_role_then_tool_name_then_task() {
    let with_role = SupervisedTaskEntry {
        id: None,
        tool_name: Some("spawn_agent".into()),
        tool_call_id: None,
        status: None,
        lifecycle_state: None,
        runtime_state: None,
        source: Some("model".into()),
        role: Some("reviewer".into()),
        summary: None,
        artifact_count: None,
        runtime_policy_stamp: None,
        parent_session_key: None,
        child_session_key: None,
        child_terminal_state: None,
        child_join_state: None,
    };
    assert_eq!(with_role.display_label(), "reviewer");

    let with_role_blank = SupervisedTaskEntry {
        role: Some("   ".into()),
        ..with_role.clone()
    };
    assert_eq!(
        with_role_blank.display_label(),
        "spawn_agent",
        "blank role should fall back to tool_name"
    );

    let bare = SupervisedTaskEntry {
        id: None,
        tool_name: None,
        tool_call_id: None,
        status: None,
        lifecycle_state: None,
        runtime_state: None,
        source: None,
        role: None,
        summary: None,
        artifact_count: None,
        runtime_policy_stamp: None,
        parent_session_key: None,
        child_session_key: None,
        child_terminal_state: None,
        child_join_state: None,
    };
    assert_eq!(bare.display_label(), "task");
}

/// Guard test: `SupervisedTaskArtifact` round-trips the documented
/// fields. The TUI does not need to consume `extra` columns yet, but
/// it must not reject artifact payloads that include them.
#[test]
fn supervised_task_artifact_accepts_extra_wire_fields() {
    let payload = serde_json::json!({
        "id": "art-1",
        "title": "diff.patch",
        "kind": "diff",
        "status": "ready",
        "path": "/tmp/diff.patch",
        "unknown_future_field": "ignored"
    });
    let artifact: SupervisedTaskArtifact =
        serde_json::from_value(payload).expect("artifact parses");
    assert_eq!(artifact.id.as_deref(), Some("art-1"));
    assert_eq!(artifact.title.as_deref(), Some("diff.patch"));
    assert_eq!(artifact.kind.as_deref(), Some("diff"));
    assert_eq!(artifact.status.as_deref(), Some("ready"));
    assert_eq!(artifact.path.as_deref(), Some("/tmp/diff.patch"));
}
