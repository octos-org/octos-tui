//! Contract tests for block-aligned streaming markdown flush
//! (`specs/task-markdown-stream-flush.spec`).
//!
//! The scrollback is append-only, so a streaming reply may only be flushed up
//! to a COMPLETED markdown block boundary (closed fence, blank-line paragraph
//! end). Cutting mid-block rendered each batch as an independent half-document
//! and froze the damage forever — the reported "markdown isn't rendered".

use octos_core::ui_protocol::TurnId;
use octos_core::{Message, SessionKey};
use octos_tui::app::{
    finalized_history_lines_range_dedup_live, finalized_live_turn_lines_between,
    next_live_turn_finalization,
};
use octos_tui::cli::ThemeName;
use octos_tui::model::{AppState, LiveReply, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;

fn streaming_store(live_text: &str) -> Store {
    let turn_id = TurnId::new();
    let mut store = Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:md-stream-test".into()),
                title: "md-stream-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show me code")],
                tasks: vec![],
                live_reply: Some(LiveReply {
                    turn_id,
                    text: live_text.to_string(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        ),
    };
    store.state.set_run_state_in_progress();
    store
}

fn palette() -> Palette {
    Palette::for_theme(ThemeName::default())
}

fn watermark(live_text: &str) -> String {
    let store = streaming_store(live_text);
    next_live_turn_finalization(&store.state, None)
        .expect("active turn watermark")
        .reply_flushed_text
        .clone()
}

fn lines_text(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn unclosed_fence_holds_flush_watermark() {
    let text = "intro paragraph.\n\n```rust\nlet x = 1;\nlet y = 2;\n";

    let flushed = watermark(text);

    assert_eq!(
        flushed, "intro paragraph.\n\n",
        "the watermark must stop before the unclosed fence, not at the last newline"
    );
}

#[test]
fn closed_fence_flushes_as_complete_block() {
    let text = "intro.\n\n```rust\nlet x = 1;\n```\nafter the block\n";
    let store = streaming_store(text);
    let next = next_live_turn_finalization(&store.state, None).expect("watermark");

    assert!(
        next.reply_flushed_text.contains("```rust"),
        "a closed fence is flushable"
    );

    let previous_empty = octos_tui::app::LiveTurnFinalization::default();
    let lines =
        finalized_live_turn_lines_between(&store.state, palette(), 80, &previous_empty, &next);
    let rendered = lines_text(&lines);
    let opens = rendered.iter().filter(|l| l.contains("┌─")).count();
    let closes = rendered.iter().filter(|l| l.contains("└─")).count();
    assert_eq!(
        (opens, closes),
        (1, 1),
        "the flushed batch renders one complete framed code block; lines: {rendered:#?}"
    );
    assert!(
        rendered.iter().any(|l| l.contains("rust")),
        "the fence language label survives"
    );
}

#[test]
fn open_paragraph_holds_flush_watermark() {
    let text = "finished paragraph.\n\nstill being streamed, no blank line yet\nmore words\n";

    let flushed = watermark(text);

    assert_eq!(
        flushed, "finished paragraph.\n\n",
        "a paragraph still accumulating lines must stay in the live tail"
    );
}

#[test]
fn only_first_batch_carries_prose_marker() {
    let text = "first block done.\n\nsecond block also done.\n\n";
    let store = streaming_store(text);
    let next = next_live_turn_finalization(&store.state, None).expect("watermark");
    assert_eq!(next.reply_flushed_text, text);

    // Batch 1: from empty watermark — carries the bullet.
    let empty = octos_tui::app::LiveTurnFinalization::default();
    let first_batch = lines_text(&finalized_live_turn_lines_between(
        &store.state,
        palette(),
        80,
        &empty,
        &next,
    ));
    assert!(
        first_batch.iter().any(|l| l.contains("• ")),
        "first batch carries the prose marker; lines: {first_batch:#?}"
    );

    // Batch 2: simulate a later delta on the same turn — no bullet.
    let mut mid = next.clone();
    mid.reply_flushed_text = "first block done.\n\n".to_string();
    let second_batch = lines_text(&finalized_live_turn_lines_between(
        &store.state,
        palette(),
        80,
        &mid,
        &next,
    ));
    assert!(
        !second_batch.iter().any(|l| l.contains("• ")),
        "continuation batches must not re-issue the bullet; lines: {second_batch:#?}"
    );
}

#[test]
fn commit_suffix_joins_flushed_prefix_without_marker() {
    // The turn committed: the message holds the full reply, a coverage
    // watermark says the first part is already in scrollback.
    let full = "first block done.\n\ntail paragraph after watermark.";
    let mut store = streaming_store("ignored");
    store.state.sessions[0].live_reply = None;
    store.state.sessions[0]
        .messages
        .push(Message::assistant(full));

    let coverage = octos_tui::app::LiveTurnFinalization {
        reply_flushed_text: "first block done.\n\n".to_string(),
        ..Default::default()
    };

    let lines = lines_text(&finalized_history_lines_range_dedup_live(
        &store.state,
        palette(),
        80,
        1,
        std::slice::from_ref(&coverage),
    ));

    assert!(
        lines
            .iter()
            .any(|l| l.contains("tail paragraph after watermark")),
        "the suffix beyond the watermark is committed; lines: {lines:#?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("first block done")),
        "the already-flushed prefix is never re-emitted"
    );
    assert!(
        !lines.iter().any(|l| l.contains("• ")),
        "the commit suffix is a continuation — no second bullet"
    );
}

#[test]
fn fully_settled_text_flushes_to_end() {
    let text = "everything here is settled.\n\nincluding this block.\n\n";

    let flushed = watermark(text);

    assert_eq!(
        flushed, text,
        "with no open structures the watermark reaches the end (no regression in flush latency)"
    );
}
