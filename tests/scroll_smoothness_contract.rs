//! Contract tests for pager scroll smoothness
//! (`specs/task-scroll-smoothness.spec`).
//!
//! Trackpad momentum scrolling delivers dozens of fine-grained wheel events
//! per second; inside the pager each event steps a single line so content
//! glides instead of jumping. Other surfaces keep the coarser 4-line step,
//! and keyboard paging keeps its 8-line stride.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use octos_core::{Message, SessionKey};
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{AppState, FocusPane, SessionView};
use octos_tui::store::Store;

fn chat_store(message_count: usize) -> Store {
    let messages = (1..=message_count)
        .flat_map(|idx| {
            [
                Message::user(format!("ask number {idx:02}")),
                Message::assistant(format!("history message {idx:02}")),
            ]
        })
        .collect();
    Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:smoothness-test".into()),
                title: "smoothness-test".into(),
                profile_id: Some("coding".into()),
                messages,
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

fn wheel_up() -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn pager_wheel_scrolls_one_line_per_event() {
    let mut store = chat_store(20);
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    assert!(store.state.transcript_pager_active);
    assert_eq!(store.state.transcript_scroll, 0);

    handle_terminal_event(&mut store, wheel_up());
    assert_eq!(
        store.state.transcript_scroll, 1,
        "inside the pager one wheel event steps exactly one line"
    );
    handle_terminal_event(&mut store, wheel_up());
    assert_eq!(store.state.transcript_scroll, 2);
}

#[test]
fn non_pager_wheel_keeps_coarse_step() {
    let mut store = chat_store(5);
    store.state.focus = FocusPane::Workspace;

    // Workspace scroll counts from the top, so wheel-down grows the offset.
    handle_terminal_event(
        &mut store,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }),
    );

    assert_eq!(
        store.state.workspace.scroll, 4,
        "non-pager surfaces keep the existing 4-line wheel step"
    );
}

#[test]
fn keyboard_paging_step_unchanged() {
    let mut store = chat_store(20);
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );

    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
    );

    assert_eq!(
        store.state.transcript_scroll, 8,
        "keyboard paging keeps its 8-line stride"
    );
}
