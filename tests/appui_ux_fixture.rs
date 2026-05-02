use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Map, Value, json};

const FIXTURE_PATH: &str = "fixtures/appui_ux_parity/coding_session_short.json";

#[derive(Debug, Deserialize)]
struct Fixture {
    schema: String,
    mode: String,
    thresholds: Thresholds,
    records: Vec<Record>,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Thresholds {
    long_output_lines: usize,
    long_diff_lines: usize,
    collapsed_preview_lines: usize,
}

#[derive(Debug, Deserialize)]
struct Expected {
    normalized_sequence: Vec<Value>,
    ux_assertions: UxAssertions,
    live_soak: LiveSoak,
}

#[derive(Debug, Deserialize)]
struct UxAssertions {
    ci_offline: bool,
    requires_real_model_output: bool,
    user_bubble_must_precede_assistant: bool,
    approval_must_block_until_decision: bool,
    required_activity_labels: Vec<String>,
    sticky_placement: String,
    permission_focus: String,
}

#[derive(Debug, Deserialize)]
struct LiveSoak {
    manual_only: bool,
    duration_minutes: u64,
    requires_env: Vec<String>,
    artifact_dir: String,
}

#[derive(Debug, Deserialize)]
struct Record {
    direction: String,
    kind: String,
    method: String,
    websocket: TransportFrame,
    stdio: TransportFrame,
    #[serde(default)]
    ui: UiExpectation,
}

#[derive(Debug, Deserialize)]
struct TransportFrame {
    wire: Value,
}

#[derive(Debug, Default, Deserialize)]
struct UiExpectation {
    placement: Option<String>,
    plan_placement: Option<String>,
    focus: Option<String>,
    blocks_turn: Option<bool>,
    collapsed_by_default: Option<bool>,
    activity_label: Option<String>,
}

#[test]
fn websocket_and_stdio_records_normalize_to_same_semantics() {
    let fixture = load_fixture();

    assert_eq!(fixture.schema, "octos-tui.appui-ux-fixture.v1");
    assert_eq!(fixture.mode, "ci-short");
    assert_eq!(fixture.thresholds.collapsed_preview_lines, 1);

    let websocket = normalize_transport(&fixture, "websocket");
    let stdio = normalize_transport(&fixture, "stdio");

    assert_eq!(
        websocket, stdio,
        "transport ids, frame kind, and stdio line numbers must not change AppUI semantics"
    );
    assert_eq!(
        websocket, fixture.expected.normalized_sequence,
        "fixture normalizer drifted from the pinned semantic transcript"
    );
}

#[test]
fn fixture_covers_known_appui_ux_assertions() {
    let fixture = load_fixture();
    let events = normalize_transport(&fixture, "websocket");
    let assertions = &fixture.expected.ux_assertions;

    assert!(assertions.ci_offline);
    assert!(!assertions.requires_real_model_output);

    if assertions.user_bubble_must_precede_assistant {
        assert_before(
            &events,
            |event| event_name(event) == Some("message.user"),
            |event| event_name(event) == Some("message.assistant_delta"),
            "user bubble should be present before the first assistant delta",
        );
    }

    let assistant = find_event(&events, "message.assistant_delta");
    assert_eq!(assistant["contains_markdown_table"], json!(true));
    assert_eq!(assistant["plan_step_count"], json!(2));
    assert_eq!(
        assistant["plan_placement"],
        json!(assertions.sticky_placement)
    );

    let status = find_event(&events, "status.update");
    assert_eq!(status["placement"], json!(assertions.sticky_placement));

    let approval = find_event(&events, "approval.requested");
    assert_eq!(approval["blocks_turn"], json!(true));
    assert_eq!(approval["focus"], json!(assertions.permission_focus));
    assert_eq!(approval["placement"], json!(assertions.sticky_placement));

    if assertions.approval_must_block_until_decision {
        assert_approval_blocks_mutating_tool_until_decided(&events);
    }

    let long_tool = events
        .iter()
        .find(|event| {
            event_name(event) == Some("activity.tool.completed")
                && event["tool_call_id"] == json!("shell-1")
        })
        .expect("shell completion event");
    assert_eq!(long_tool["long_output"], json!(true));
    assert_eq!(long_tool["collapsed_by_default"], json!(true));

    let task_output = find_event(&events, "task.output_delta");
    assert_eq!(task_output["long_output"], json!(true));
    assert_eq!(task_output["collapsed_by_default"], json!(true));

    let diff = find_event(&events, "diff.preview.ready");
    assert_eq!(diff["long_diff"], json!(true));
    assert_eq!(diff["collapsed_by_default"], json!(true));

    let labels = events
        .iter()
        .filter_map(|event| event.get("activity_label").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    for label in &assertions.required_activity_labels {
        assert!(
            labels.contains(label.as_str()),
            "missing expected activity label {label:?}; got {labels:?}"
        );
    }

    assert!(
        events
            .iter()
            .any(|event| event_name(event) == Some("task.cancelled"))
    );
    assert!(
        events
            .iter()
            .any(|event| event_name(event) == Some("replay.lossy"))
    );
    assert!(
        events
            .iter()
            .any(|event| event_name(event) == Some("task.output.read_result"))
    );
}

#[test]
fn live_soak_notes_remain_manual_and_bounded() {
    let fixture = load_fixture();
    let soak = &fixture.expected.live_soak;

    assert!(soak.manual_only);
    assert_eq!(soak.duration_minutes, 60);
    assert!(
        soak.requires_env
            .iter()
            .any(|env| env.starts_with("OCTOS_TUI_UX_LIVE_SOAK=1"))
    );
    assert!(
        soak.requires_env
            .iter()
            .any(|env| env.starts_with("OCTOS_TUI_PROTOCOL_ENDPOINT="))
    );
    assert!(soak.artifact_dir.contains("test-results-tui-coding-ux"));
}

fn load_fixture() -> Fixture {
    let raw = std::fs::read_to_string(FIXTURE_PATH).expect("fixture is readable");
    serde_json::from_str(&raw).expect("fixture schema is valid")
}

fn normalize_transport(fixture: &Fixture, transport: &str) -> Vec<Value> {
    fixture
        .records
        .iter()
        .map(|record| normalize_record(record, transport, &fixture.thresholds))
        .collect()
}

fn normalize_record(record: &Record, transport: &str, thresholds: &Thresholds) -> Value {
    let wire = match transport {
        "websocket" => &record.websocket.wire,
        "stdio" => &record.stdio.wire,
        other => panic!("unknown transport {other}"),
    };

    let payload = payload_for(record, wire);
    match (
        record.direction.as_str(),
        record.kind.as_str(),
        record.method.as_str(),
    ) {
        ("client_to_server", "request", "session/open") => {
            let mut event = event("session.open.request");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "cwd", &payload["cwd"]);
            insert_str(&mut event, "profile_id", &payload["profile_id"]);
            insert_u64(&mut event, "cursor_after_seq", &payload["after"]["seq"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "session/open") => {
            let mut event = event("session.opened");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "cwd", &payload["workspace_root"]);
            insert_str(&mut event, "profile_id", &payload["active_profile_id"]);
            insert_u64(&mut event, "cursor_seq", &payload["cursor"]["seq"]);
            Value::Object(event)
        }
        ("client_to_server", "request", "turn/start") => {
            let mut event = event("message.user");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            event.insert("role".into(), json!("user"));
            event.insert("content".into(), json!(first_text_input(payload)));
            Value::Object(event)
        }
        ("server_to_client", "notification", "turn/started") => {
            let mut event = event("turn.started");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "message/delta") => {
            let text = payload["text"].as_str().expect("message text");
            let mut event = event("message.assistant_delta");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            event.insert("role".into(), json!("assistant"));
            event.insert("content".into(), json!(text));
            event.insert(
                "contains_markdown_table".into(),
                json!(contains_markdown_table(text)),
            );
            event.insert("plan_step_count".into(), json!(count_plan_steps(text)));
            insert_optional_string(
                &mut event,
                "plan_placement",
                record.ui.plan_placement.as_deref(),
            );
            Value::Object(event)
        }
        ("server_to_client", "notification", "progress/updated") => {
            normalize_progress(record, payload)
        }
        ("server_to_client", "notification", "task/updated") => {
            let state = payload["state"].as_str().expect("task state");
            let runtime_detail = payload["runtime_detail"].as_str().unwrap_or_default();
            let semantic_state = if state == "failed" && runtime_detail.contains("cancel") {
                "cancelled"
            } else {
                state
            };
            let mut event = event(&format!("task.{semantic_state}"));
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "task_id", &payload["task_id"]);
            insert_str(&mut event, "title", &payload["title"]);
            insert_str(&mut event, "state", &payload["state"]);
            insert_str(&mut event, "runtime_detail", &payload["runtime_detail"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "tool/started") => {
            let mut event = event("activity.tool.started");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            insert_str(&mut event, "tool_call_id", &payload["tool_call_id"]);
            insert_str(&mut event, "tool_name", &payload["tool_name"]);
            insert_optional_string(
                &mut event,
                "activity_label",
                record.ui.activity_label.as_deref(),
            );
            Value::Object(event)
        }
        ("server_to_client", "notification", "tool/progress") => {
            let mut event = event("activity.tool.progress");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            insert_str(&mut event, "tool_call_id", &payload["tool_call_id"]);
            insert_str(&mut event, "message", &payload["message"]);
            if let Some(pct) = payload["progress_pct"].as_f64() {
                event.insert("status".into(), json!(format!("{pct:.0}%")));
            }
            Value::Object(event)
        }
        ("server_to_client", "notification", "tool/completed") => {
            let output = payload["output_preview"].as_str().unwrap_or_default();
            let line_count = count_meaningful_lines(output);
            let mut event = event("activity.tool.completed");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            insert_str(&mut event, "tool_call_id", &payload["tool_call_id"]);
            insert_str(&mut event, "tool_name", &payload["tool_name"]);
            insert_optional_string(
                &mut event,
                "activity_label",
                record.ui.activity_label.as_deref(),
            );
            insert_bool(&mut event, "success", &payload["success"]);
            event.insert("output_line_count".into(), json!(line_count));
            event.insert(
                "long_output".into(),
                json!(line_count >= thresholds.long_output_lines),
            );
            event.insert(
                "collapsed_by_default".into(),
                json!(record.ui.collapsed_by_default.unwrap_or(false)),
            );
            Value::Object(event)
        }
        ("server_to_client", "notification", "task/output/delta") => {
            let line_count = count_meaningful_lines(payload["text"].as_str().unwrap_or_default());
            let mut event = event("task.output_delta");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "task_id", &payload["task_id"]);
            insert_u64(&mut event, "cursor_offset", &payload["cursor"]["offset"]);
            event.insert("line_count".into(), json!(line_count));
            event.insert(
                "long_output".into(),
                json!(line_count >= thresholds.long_output_lines),
            );
            event.insert(
                "collapsed_by_default".into(),
                json!(record.ui.collapsed_by_default.unwrap_or(false)),
            );
            Value::Object(event)
        }
        ("client_to_server", "request", "task/output/read") => {
            let mut event = event("task.output.read_requested");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "task_id", &payload["task_id"]);
            insert_u64(&mut event, "limit_bytes", &payload["limit_bytes"]);
            Value::Object(event)
        }
        ("server_to_client", "response", "task/output/read") => {
            let mut event = event("task.output.read_result");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "task_id", &payload["task_id"]);
            event.insert(
                "line_count".into(),
                json!(count_meaningful_lines(
                    payload["text"].as_str().unwrap_or_default()
                )),
            );
            insert_bool(&mut event, "complete", &payload["complete"]);
            insert_str(&mut event, "task_status", &payload["task_status"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "approval/requested") => {
            let mut event = event("approval.requested");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            insert_str(&mut event, "approval_id", &payload["approval_id"]);
            insert_str(&mut event, "tool_name", &payload["tool_name"]);
            insert_str(&mut event, "title", &payload["title"]);
            insert_str(&mut event, "approval_kind", &payload["approval_kind"]);
            insert_str(&mut event, "risk", &payload["risk"]);
            insert_str(
                &mut event,
                "preview_id",
                &payload["typed_details"]["diff"]["preview_id"],
            );
            event.insert(
                "blocks_turn".into(),
                json!(record.ui.blocks_turn.unwrap_or(false)),
            );
            insert_optional_string(&mut event, "focus", record.ui.focus.as_deref());
            insert_optional_string(&mut event, "placement", record.ui.placement.as_deref());
            Value::Object(event)
        }
        ("client_to_server", "request", "diff/preview/get") => {
            let mut event = event("diff.preview.requested");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "preview_id", &payload["preview_id"]);
            Value::Object(event)
        }
        ("server_to_client", "response", "diff/preview/get") => {
            let line_count = count_diff_lines(payload);
            let mut event = event("diff.preview.ready");
            insert_str(&mut event, "session_id", &payload["preview"]["session_id"]);
            insert_str(&mut event, "preview_id", &payload["preview"]["preview_id"]);
            insert_str(&mut event, "status", &payload["status"]);
            insert_str(&mut event, "source", &payload["source"]);
            event.insert("line_count".into(), json!(line_count));
            event.insert(
                "long_diff".into(),
                json!(line_count >= thresholds.long_diff_lines),
            );
            event.insert(
                "collapsed_by_default".into(),
                json!(record.ui.collapsed_by_default.unwrap_or(false)),
            );
            Value::Object(event)
        }
        ("client_to_server", "request", "approval/respond") => {
            let mut event = event("approval.responded");
            insert_str(&mut event, "approval_id", &payload["approval_id"]);
            insert_str(&mut event, "decision", &payload["decision"]);
            insert_str(&mut event, "approval_scope", &payload["approval_scope"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "approval/decided") => {
            let mut event = event("approval.decided");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            insert_str(&mut event, "approval_id", &payload["approval_id"]);
            insert_str(&mut event, "decision", &payload["decision"]);
            insert_str(&mut event, "scope", &payload["scope"]);
            insert_str(&mut event, "decided_by", &payload["decided_by"]);
            Value::Object(event)
        }
        ("client_to_server", "request", "turn/interrupt") => {
            let mut event = event("turn.interrupt.request");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "turn/error") => {
            let mut event = event("turn.cancelled");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_str(&mut event, "turn_id", &payload["turn_id"]);
            insert_str(&mut event, "code", &payload["code"]);
            insert_str(&mut event, "message", &payload["message"]);
            Value::Object(event)
        }
        ("server_to_client", "notification", "protocol/replay_lossy") => {
            let mut event = event("replay.lossy");
            insert_str(&mut event, "session_id", &payload["session_id"]);
            insert_u64(&mut event, "dropped_count", &payload["dropped_count"]);
            insert_u64(
                &mut event,
                "last_durable_seq",
                &payload["last_durable_cursor"]["seq"],
            );
            Value::Object(event)
        }
        other => panic!("unhandled fixture record {other:?}"),
    }
}

fn payload_for<'a>(record: &Record, wire: &'a Value) -> &'a Value {
    match record.kind.as_str() {
        "request" | "notification" => &wire["params"],
        "response" => &wire["result"],
        other => panic!("unknown record kind {other}"),
    }
}

fn normalize_progress(record: &Record, payload: &Value) -> Value {
    let metadata = &payload["metadata"];
    let kind = metadata["kind"].as_str().expect("progress kind");
    if kind == "file_mutation" {
        let file_mutation = &metadata["file_mutation"];
        let mut event = event("progress.file_mutation");
        insert_str(&mut event, "session_id", &payload["session_id"]);
        insert_str(&mut event, "turn_id", &payload["turn_id"]);
        insert_str(&mut event, "path", &file_mutation["path"]);
        insert_str(&mut event, "operation", &file_mutation["operation"]);
        insert_str(&mut event, "preview_id", &file_mutation["preview_id"]);
        insert_str(&mut event, "message", &metadata["message"]);
        insert_optional_string(&mut event, "placement", record.ui.placement.as_deref());
        return Value::Object(event);
    }

    let mut event = event("status.update");
    insert_str(&mut event, "session_id", &payload["session_id"]);
    insert_str(&mut event, "turn_id", &payload["turn_id"]);
    insert_str(&mut event, "message", &metadata["message"]);
    insert_optional_string(&mut event, "placement", record.ui.placement.as_deref());
    Value::Object(event)
}

fn event(name: &str) -> Map<String, Value> {
    let mut event = Map::new();
    event.insert("event".into(), json!(name));
    event
}

fn insert_str(event: &mut Map<String, Value>, key: &str, value: &Value) {
    if let Some(value) = value.as_str() {
        event.insert(key.into(), json!(value));
    }
}

fn insert_optional_string(event: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        event.insert(key.into(), json!(value));
    }
}

fn insert_bool(event: &mut Map<String, Value>, key: &str, value: &Value) {
    if let Some(value) = value.as_bool() {
        event.insert(key.into(), json!(value));
    }
}

fn insert_u64(event: &mut Map<String, Value>, key: &str, value: &Value) {
    if let Some(value) = value.as_u64() {
        event.insert(key.into(), json!(value));
    }
}

fn first_text_input(payload: &Value) -> &str {
    payload["input"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["kind"].as_str() == Some("text"))
        })
        .and_then(|item| item["text"].as_str())
        .expect("turn/start has text input")
}

fn contains_markdown_table(text: &str) -> bool {
    let lines = text.lines().collect::<Vec<_>>();
    lines.windows(2).any(|pair| {
        let header = pair[0].trim();
        let separator = pair[1].trim();
        header.starts_with('|')
            && header.ends_with('|')
            && separator.starts_with('|')
            && separator.ends_with('|')
            && separator
                .trim_matches('|')
                .split('|')
                .all(|cell| cell.trim().chars().all(|ch| matches!(ch, '-' | ':')))
    })
}

fn count_plan_steps(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("- [ ] ") || line.starts_with("- [x] ") || line.starts_with("- [X] ")
        })
        .count()
}

fn count_meaningful_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

fn count_diff_lines(payload: &Value) -> usize {
    payload["preview"]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|file| file["hunks"].as_array().into_iter().flatten())
        .map(|hunk| hunk["lines"].as_array().map_or(0, Vec::len))
        .sum()
}

fn event_name(event: &Value) -> Option<&str> {
    event.get("event").and_then(Value::as_str)
}

fn find_event<'a>(events: &'a [Value], name: &str) -> &'a Value {
    events
        .iter()
        .find(|event| event_name(event) == Some(name))
        .unwrap_or_else(|| panic!("missing event {name}"))
}

fn assert_before<P, Q>(events: &[Value], first: P, second: Q, message: &str)
where
    P: Fn(&Value) -> bool,
    Q: Fn(&Value) -> bool,
{
    let first_index = events.iter().position(first).expect("first event");
    let second_index = events.iter().position(second).expect("second event");
    assert!(first_index < second_index, "{message}");
}

fn assert_approval_blocks_mutating_tool_until_decided(events: &[Value]) {
    let approval_index = events
        .iter()
        .position(|event| event_name(event) == Some("approval.requested"))
        .expect("approval requested");
    let decision_index = events
        .iter()
        .position(|event| event_name(event) == Some("approval.decided"))
        .expect("approval decided");
    assert!(
        approval_index < decision_index,
        "approval decision must follow request"
    );

    let mutating_tool_started_before_decision = events
        .iter()
        .enumerate()
        .filter(|(index, _)| *index > approval_index && *index < decision_index)
        .any(|(_, event)| {
            event_name(event) == Some("activity.tool.started")
                && matches!(
                    event["tool_name"].as_str(),
                    Some("diff_edit" | "edit_file" | "write_file")
                )
        });

    assert!(
        !mutating_tool_started_before_decision,
        "mutating tools must not start while a permission prompt is blocking the turn"
    );
}
