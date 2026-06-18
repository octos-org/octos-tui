//! Contract tests for composer multi-line input
//! (`specs/task-composer-multiline.spec`).
//!
//! The composer already stores/renders `\n`, but users had no key to insert a
//! newline (plain Enter submits) and Up/Down only scrolled the transcript. This
//! pins: Alt+Enter / Ctrl+J insert a newline (Enter still submits), and Up/Down
//! move the cursor between logical lines, falling back to transcript scroll at
//! the first/last line.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::SessionKey;
use octos_tui::event_loop::{KeyAction, handle_terminal_event};
use octos_tui::model::{AppState, FocusPane, SessionView};
use octos_tui::store::Store;

fn composer_store(text: &str, cursor: Option<usize>) -> Store {
    let session = SessionView {
        id: SessionKey("local:composer-test".into()),
        title: "composer-test".into(),
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
    store.state.focus = FocusPane::Composer;
    store.state.composer = text.into();
    store.state.composer_cursor = cursor;
    store
}

fn key(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, mods))
}

#[test]
fn alt_enter_inserts_newline_without_submitting() {
    let mut store = composer_store("ab", Some(2));
    let action = handle_terminal_event(&mut store, key(KeyCode::Enter, KeyModifiers::ALT));

    assert_eq!(
        store.state.composer, "ab\n",
        "Alt+Enter must insert a newline"
    );
    assert!(
        matches!(action, KeyAction::Continue),
        "Alt+Enter must not submit the turn"
    );
}

#[test]
fn ctrl_j_inserts_newline() {
    let mut store = composer_store("ab", Some(2));
    let action = handle_terminal_event(&mut store, key(KeyCode::Char('j'), KeyModifiers::CONTROL));

    assert_eq!(store.state.composer, "ab\n", "Ctrl+J must insert a newline");
    assert!(
        matches!(action, KeyAction::Continue),
        "Ctrl+J must not submit"
    );
}

#[test]
fn plain_enter_still_submits() {
    let mut store = composer_store("hello", Some(5));
    let action = handle_terminal_event(&mut store, key(KeyCode::Enter, KeyModifiers::NONE));

    assert!(
        matches!(action, KeyAction::Send(_)),
        "plain Enter must still submit the turn (no regression)"
    );
    assert!(
        !store.state.composer.contains('\n'),
        "plain Enter must never insert a newline"
    );
}

#[test]
fn arrow_down_moves_cursor_to_next_line() {
    // "abc\nde": cursor after `c` (col 3 on line 1). Down → line 2, clamped to
    // its end (col 2 = byte index 6).
    let mut store = composer_store("abc\nde", Some(3));
    let before_scroll = store.state.transcript_scroll;
    handle_terminal_event(&mut store, key(KeyCode::Down, KeyModifiers::NONE));

    assert_eq!(
        store.state.composer_cursor_index(),
        6,
        "Down must move to the next line, column clamped to line end"
    );
    assert_eq!(
        store.state.transcript_scroll, before_scroll,
        "moving within the composer must not scroll the transcript"
    );
}

#[test]
fn arrow_up_at_first_line_scrolls_transcript() {
    // Cursor on the first line → Up cannot move up, so it falls back to the
    // existing transcript scroll instead of moving the cursor.
    let mut store = composer_store("abc\nde", Some(1));
    handle_terminal_event(&mut store, key(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(
        store.state.composer_cursor_index(),
        1,
        "Up on the first line must not move the cursor"
    );
    assert_eq!(
        store.state.transcript_scroll, 1,
        "Up on the first line falls back to scrolling the transcript"
    );
}

#[test]
fn arrow_down_at_last_line_does_not_panic() {
    // Cursor at the very end (last line) → Down cannot move down; it must fall
    // back to scrolling without panicking or corrupting the cursor.
    let mut store = composer_store("abc\nde", Some(6));
    handle_terminal_event(&mut store, key(KeyCode::Down, KeyModifiers::NONE));

    assert_eq!(
        store.state.composer_cursor_index(),
        6,
        "Down on the last line must leave the cursor at a valid position"
    );
    // The fallback is scroll-down, which saturates at the bottom (0) — the
    // point is it takes the scroll path without panicking or moving the cursor.
    assert_eq!(
        store.state.transcript_scroll, 0,
        "Down at the bottom stays pinned to latest (scroll-down saturates)"
    );
}
