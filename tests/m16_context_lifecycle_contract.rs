use octos_core::SessionKey;
use octos_tui::model::{
    APPUI_FEATURE_CONTEXT_LIFECYCLE_V1, APPUI_METHOD_CONTEXT_COMPACTION_COMPLETED,
    APPUI_METHOD_CONTEXT_NORMALIZATION_REPORTED, ContextCompactionSummary, ContextLifecycleState,
    ContextNormalizationSummary, SessionContextLifecycle,
};
use serde_json::Value;

/// Guard test: the M16-G2 capability and notification method constants
/// the TUI uses to gate compact-context UX must stay on-spec
/// (`context.lifecycle.v1`, `context/compaction_completed`,
/// `context/normalization_reported`).
#[test]
fn m16_capability_and_notification_constants_stay_on_spec() {
    assert_eq!(APPUI_FEATURE_CONTEXT_LIFECYCLE_V1, "context.lifecycle.v1");
    assert_eq!(
        APPUI_METHOD_CONTEXT_COMPACTION_COMPLETED,
        "context/compaction_completed"
    );
    assert_eq!(
        APPUI_METHOD_CONTEXT_NORMALIZATION_REPORTED,
        "context/normalization_reported"
    );
}

/// Guard test: the compaction + normalization wire shapes round-trip
/// into the TUI's lifecycle ledger without losing the fields the M16-G2
/// status surface needs (generation, retained/dropped counts,
/// token estimates, normalization counts).
#[test]
fn compaction_and_normalization_events_round_trip_into_ledger() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/m16_context_lifecycle.json"))
            .expect("fixture parses");
    let events = fixture["events"].as_array().expect("events array");

    let mut ledger = SessionContextLifecycle::default();

    // Empty ledger renders no status (so the TUI hides the surface
    // until the server says something).
    assert!(ledger.summary_line().is_none());

    for event in events {
        match event["method"].as_str().expect("method") {
            "context/compaction_completed" => {
                let state: ContextLifecycleState =
                    serde_json::from_value(event["params"]["context_state"].clone())
                        .expect("context_state shape");
                let compaction: ContextCompactionSummary =
                    serde_json::from_value(event["params"]["compaction"].clone())
                        .expect("compaction shape");
                ledger.apply_compaction(state, compaction);
            }
            "context/normalization_reported" => {
                let state: ContextLifecycleState =
                    serde_json::from_value(event["params"]["context_state"].clone())
                        .expect("context_state shape");
                let normalization: ContextNormalizationSummary =
                    serde_json::from_value(event["params"]["normalization"].clone())
                        .expect("normalization shape");
                ledger.apply_normalization(state, normalization);
            }
            other => panic!("unexpected method in fixture: {other}"),
        }
    }

    let state = ledger.state.as_ref().expect("state populated");
    assert_eq!(state.generation, 4);
    assert_eq!(state.item_count, 42);
    assert_eq!(state.token_estimate, 9100);
    assert_eq!(state.last_compaction_id.as_deref(), Some("comp-001"));

    let compaction = ledger.last_compaction.as_ref().expect("compaction stored");
    assert_eq!(compaction.input_generation, 3);
    assert_eq!(compaction.output_generation, Some(4));
    assert_eq!(compaction.retained_count, 42);
    assert_eq!(compaction.dropped_count, 88);
    assert_eq!(compaction.token_estimate_before, 31200);
    assert_eq!(compaction.token_estimate_after, Some(9100));

    let normalization = ledger
        .last_normalization
        .as_ref()
        .expect("normalization stored");
    assert_eq!(normalization.generation, 4);
    assert_eq!(normalization.repaired_count, 2);
    assert_eq!(normalization.synthetic_count, 1);
    assert_eq!(normalization.dropped_count, 0);
    assert_eq!(normalization.truncated_count, 0);
}

/// Guard test: the bounded `summary_line` matches the documented
/// status surface (the issue's "render active context generation,
/// compacted/rebuilt status, and last compaction summary in a bounded
/// status surface" — without raw transcript hashes or per-item lists).
#[test]
fn summary_line_is_bounded_and_does_not_leak_raw_hashes_or_item_ids() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/m16_context_lifecycle.json"))
            .expect("fixture parses");
    let mut ledger = SessionContextLifecycle::default();
    let comp_event = &fixture["events"][0];
    let state: ContextLifecycleState =
        serde_json::from_value(comp_event["params"]["context_state"].clone())
            .expect("context_state shape");
    let compaction: ContextCompactionSummary =
        serde_json::from_value(comp_event["params"]["compaction"].clone())
            .expect("compaction shape");
    ledger.apply_compaction(state, compaction);

    let summary = ledger.summary_line().expect("summary present");
    assert_eq!(
        summary,
        fixture["expected_summary_after_compaction"]
            .as_str()
            .unwrap()
    );

    // The bounded summary must not leak raw transcript hashes,
    // checkpoint internals, summary item ids, or per-record field
    // names. Those stay inside the ledger struct for diagnostics, but
    // never reach the chat-adjacent status surface.
    for forbidden in fixture["expected_no_raw_record_text"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
    {
        assert!(
            !summary.contains(forbidden),
            "summary leaked raw lifecycle field {forbidden}: {summary}"
        );
    }
}

/// Guard test: applying a normalization event when the state recovery
/// label is not `"healthy"` surfaces that label in the summary, so the
/// status surface can show "(recovering)" / "(degraded)" without the
/// TUI inventing a label of its own.
#[test]
fn recovery_state_label_appears_in_summary_when_not_healthy() {
    let mut ledger = SessionContextLifecycle::default();
    ledger.apply_normalization(
        ContextLifecycleState {
            session_id: SessionKey("local:test".into()),
            thread_id: None,
            generation: 12,
            transcript_hash: "h".into(),
            item_count: 30,
            token_estimate: 4000,
            recovery_state: "recovering".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        },
        ContextNormalizationSummary {
            generation: 12,
            model_capability_id: "anthropic/sonnet-4.7".into(),
            prompt_message_count: 30,
            token_estimate: 4000,
            repaired_count: 0,
            dropped_count: 0,
            synthetic_count: 0,
            truncated_count: 0,
        },
    );
    let summary = ledger.summary_line().expect("summary");
    assert!(summary.contains("(recovering)"), "summary={}", summary);
    // Compaction segment must NOT appear when no compaction has been
    // observed yet.
    assert!(!summary.contains("compacted"), "summary={}", summary);
}
