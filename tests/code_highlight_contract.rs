//! Contract tests for fenced-code syntax highlighting
//! (`specs/task-code-syntax-highlight.spec`).
//!
//! syntect colors code tokens by the fence's language tag, taking only
//! FOREGROUND colors so blocks keep blending with the terminal background in
//! the live tail, native scrollback, and the pager. Unknown/missing languages
//! fall back to the previous single-color rendering.

use octos_core::ui_protocol::TurnId;
use octos_core::{Message, SessionKey};
use octos_tui::app::{
    LiveTurnFinalization, finalized_live_turn_lines_between, next_live_turn_finalization,
};
use octos_tui::cli::ThemeName;
use octos_tui::model::{AppState, LiveReply, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use ratatui::style::Color;
use ratatui::text::Line;

const RUST_BLOCK: &str = "intro.\n\n```rust\nfn main() { let answer = 42; }\n```\n\n";

fn store_with_committed_reply(reply: &str) -> Store {
    Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:highlight-test".into()),
                title: "highlight-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("code please"), Message::assistant(reply)],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        ),
    }
}

fn palette() -> Palette {
    Palette::for_theme(ThemeName::default())
}

/// Render a committed reply through the public scrollback-commit path and
/// return the produced lines.
fn committed_lines(reply: &str) -> Vec<Line<'static>> {
    let store = store_with_committed_reply(reply);
    octos_tui::app::finalized_history_lines_range_dedup_live(&store.state, palette(), 100, 1, &[])
}

/// The code-body rows of a rendered block (those carrying the `│ ` frame).
fn code_rows(lines: &[Line<'_>]) -> Vec<Vec<(String, Option<Color>)>> {
    lines
        .iter()
        .filter(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains('│'))
        })
        .map(|line| {
            line.spans
                .iter()
                .map(|span| (span.content.to_string(), span.style.fg))
                .collect()
        })
        .collect()
}

#[test]
fn known_language_block_gets_colored_tokens() {
    let lines = committed_lines(RUST_BLOCK);
    let rows = code_rows(&lines);
    assert!(!rows.is_empty(), "code rows rendered");

    let token_colors: std::collections::HashSet<_> = rows
        .iter()
        .flatten()
        .filter(|(text, _)| !text.contains('│') && !text.trim().is_empty())
        .filter_map(|(_, fg)| *fg)
        .collect();
    assert!(
        token_colors.len() > 1,
        "a rust line must produce multiple distinct token colors, got {token_colors:?}"
    );
    assert!(
        token_colors
            .iter()
            .all(|color| matches!(color, Color::Rgb(..))),
        "highlight colors come from the syntect theme as RGB"
    );
}

#[test]
fn highlighted_code_has_no_background() {
    let lines = committed_lines(RUST_BLOCK);
    let rows = code_rows(&lines);

    for row in &rows {
        for (text, _) in row {
            let _ = text;
        }
    }
    for line in &lines {
        for span in &line.spans {
            if span.content.as_ref().trim().is_empty() || span.content.as_ref().contains('│') {
                continue;
            }
            // Committed scrollback lines may carry the assistant surface bg at
            // this layer (stripped later by the flush path); the HIGHLIGHTED
            // token spans themselves must never introduce their own bg.
            if let Some(Color::Rgb(..)) = span.style.fg {
                assert_eq!(
                    span.style.bg, None,
                    "highlight spans must not set a background"
                );
            }
        }
    }
}

#[test]
fn unknown_language_falls_back_to_muted() {
    let lines = committed_lines("x.\n\n```nosuchlang\nsome opaque text here\n```\n\n");
    let rows = code_rows(&lines);
    let body: Vec<_> = rows
        .iter()
        .flatten()
        .filter(|(text, _)| text.contains("opaque"))
        .collect();
    assert!(!body.is_empty(), "code body rendered");
    assert!(
        body.iter().all(|(_, fg)| *fg == Some(palette().muted)),
        "unknown languages keep the muted single-color rendering"
    );
}

#[test]
fn missing_language_falls_back_to_muted() {
    let lines = committed_lines("x.\n\n```\nplain block body\n```\n\n");
    let rows = code_rows(&lines);
    let body: Vec<_> = rows
        .iter()
        .flatten()
        .filter(|(text, _)| text.contains("plain block body"))
        .collect();
    assert!(!body.is_empty());
    assert!(body.iter().all(|(_, fg)| *fg == Some(palette().muted)));
}

#[test]
fn streaming_flush_and_pager_highlight_consistently() {
    // Streaming path: the closed fence flushes through the live watermark.
    let turn_id = TurnId::new();
    let mut store = store_with_committed_reply("placeholder");
    store.state.sessions[0].messages.pop();
    store.state.sessions[0].live_reply = Some(LiveReply {
        turn_id,
        text: RUST_BLOCK.to_string(),
    });
    store.state.set_run_state_in_progress();

    let next = next_live_turn_finalization(&store.state, None).expect("watermark");
    let empty = LiveTurnFinalization::default();
    let streamed = finalized_live_turn_lines_between(&store.state, palette(), 100, &empty, &next);

    // Committed path: same content as a finished message.
    let committed = committed_lines(RUST_BLOCK);

    let colors = |lines: &[Line<'_>]| -> Vec<Option<Color>> {
        code_rows(lines)
            .iter()
            .flatten()
            .filter(|(text, _)| !text.contains('│') && !text.trim().is_empty())
            .map(|(_, fg)| *fg)
            .collect()
    };
    assert_eq!(
        colors(&streamed),
        colors(&committed),
        "the streaming flush and the committed render share the same highlighter"
    );
}
