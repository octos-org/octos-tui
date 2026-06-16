//! Contract tests for the full-screen transcript pager
//! (`specs/task-transcript-pager.spec.md`).
//!
//! The inline chat flow writes committed history into the terminal's native
//! scrollback, so the composer scrolls away whenever the user scrolls the
//! terminal itself. The pager is the answer: an alt-screen view where the
//! full transcript scrolls in the upper pane and the composer stays pinned to
//! the bottom row. These tests pin that surface contract: entry/exit keys,
//! pinned-composer rendering, modal mutual exclusion, and the inline-mode
//! invariant that mouse capture stays off so native selection/copy survives.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::ui_protocol::TurnId;
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::cli::ThemeName;
use octos_tui::event_loop::{KeyAction, handle_terminal_event};
use octos_tui::model::{AppState, AppUiCommand, LiveReply, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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
                id: SessionKey("local:pager-test".into()),
                title: "pager-test".into(),
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

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl_t() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL))
}

/// Minimal `FrameLike` over a plain ratatui [`Buffer`], so the public render
/// functions can be exercised without a terminal.
struct BufferFrame {
    area: Rect,
    buffer: Buffer,
}

impl BufferFrame {
    fn new(width: u16, height: u16) -> Self {
        let area = Rect::new(0, 0, width, height);
        Self {
            area,
            buffer: Buffer::empty(area),
        }
    }

    fn rows(&self) -> Vec<String> {
        (0..self.area.height)
            .map(|y| {
                (0..self.area.width)
                    .map(|x| self.buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }
}

impl FrameLike for BufferFrame {
    fn area(&self) -> Rect {
        self.area
    }

    fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, &mut self.buffer);
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, _position: P) {}

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }
}

fn rendered_rows(state: &AppState, width: u16, height: u16) -> Vec<String> {
    let mut frame = BufferFrame::new(width, height);
    app::render(&mut frame, state, Palette::for_theme(ThemeName::default()));
    frame.rows()
}

fn row_index_containing(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("expected a row containing {needle:?}; rows: {rows:#?}"))
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

#[test]
fn ctrl_t_enters_transcript_pager_fullscreen() {
    let mut store = chat_store(4);
    assert!(!store.state.transcript_pager_active);
    assert!(!app::wants_fullscreen_overlay(&store.state));

    let action = handle_terminal_event(&mut store, ctrl_t());

    assert!(matches!(action, KeyAction::Continue));
    assert!(store.state.transcript_pager_active);
    assert!(
        app::wants_fullscreen_overlay(&store.state),
        "pager must route the event loop into the alternate screen"
    );
}

#[test]
fn pager_scroll_keeps_composer_pinned_at_bottom() {
    let mut store = chat_store(30);
    handle_terminal_event(&mut store, ctrl_t());

    // At the bottom the latest committed message is visible.
    let rows = rendered_rows(&store.state, 60, 20);
    assert!(rows.iter().any(|row| row.contains("history message 30")));

    // Page far up (over-scrolling is clamped at render time): the oldest
    // history scrolls into view...
    for _ in 0..40 {
        handle_terminal_event(&mut store, key(KeyCode::PageUp));
    }
    assert!(store.state.transcript_scroll > 0);
    let rows = rendered_rows(&store.state, 60, 20);
    assert!(
        rows.iter().any(|row| row.contains("history message 01")),
        "paging up must reach the oldest committed message; rows: {rows:#?}"
    );

    // ...while the composer stays pinned at the bottom of the screen.
    let composer_row = row_index_containing(&rows, "Composer");
    assert!(
        composer_row >= rows.len() - 6,
        "composer must sit in the bottom rows, found at row {composer_row}"
    );
    let deepest_transcript_row = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("history message"))
        .map(|(idx, _)| idx)
        .max()
        .expect("transcript rows visible");
    assert!(
        deepest_transcript_row < composer_row,
        "transcript content must scroll above the pinned composer \
         (transcript row {deepest_transcript_row}, composer row {composer_row})"
    );
}

#[test]
fn pager_exit_restores_inline_and_resets_scroll() {
    let mut store = chat_store(30);
    handle_terminal_event(&mut store, ctrl_t());
    handle_terminal_event(&mut store, key(KeyCode::PageUp));
    assert!(store.state.transcript_scroll > 0);

    // Esc leaves the pager and resets the read position.
    handle_terminal_event(&mut store, key(KeyCode::Esc));
    assert!(!store.state.transcript_pager_active);
    assert_eq!(store.state.transcript_scroll, 0);
    assert!(!app::wants_fullscreen_overlay(&store.state));
    assert!(
        !app::wants_mouse_capture(&store.state),
        "mouse capture policy must drop with the pager so native selection returns"
    );

    // Ctrl+T is a toggle: in, scroll, out again.
    handle_terminal_event(&mut store, ctrl_t());
    handle_terminal_event(&mut store, key(KeyCode::PageUp));
    handle_terminal_event(&mut store, ctrl_t());
    assert!(!store.state.transcript_pager_active);
    assert_eq!(store.state.transcript_scroll, 0);
    assert!(!app::wants_mouse_capture(&store.state));
}

#[test]
fn pageup_in_chat_auto_enters_pager() {
    let mut store = chat_store(6);

    handle_terminal_event(&mut store, key(KeyCode::PageUp));

    assert!(
        store.state.transcript_pager_active,
        "PageUp in the inline chat flow cannot reach committed history, so it opens the pager"
    );
    assert_eq!(
        store.state.transcript_scroll, 0,
        "the pager opens at the bottom with the latest content visible"
    );
}

#[test]
fn ctrl_t_ignored_when_modal_overlay_active() {
    let mut store = chat_store(4);
    store.state.task_output.active = true;

    handle_terminal_event(&mut store, ctrl_t());

    assert!(
        !store.state.transcript_pager_active,
        "Ctrl+T must stay inert while a modal owns the screen"
    );
    assert!(
        store.state.task_output.active,
        "the modal must keep its state untouched"
    );
}

#[test]
fn inline_mode_never_enables_mouse_capture() {
    let mut store = chat_store(8);

    // The inline chat flow must never ask for mouse capture...
    assert!(!app::wants_mouse_capture(&store.state));

    // ...and committed history must stay out of the inline viewport (it lives
    // in native scrollback, which is what keeps selection/copy native).
    let mut frame = BufferFrame::new(80, 12);
    app::render_viewport(
        &mut frame,
        &store.state,
        Palette::for_theme(ThemeName::default()),
    );
    let rows = frame.rows();
    assert!(
        !rows.iter().any(|row| row.contains("history message")),
        "committed history must not be repainted in the inline viewport; rows: {rows:#?}"
    );

    // Plain navigation keys must not flip the capture policy either.
    handle_terminal_event(&mut store, key(KeyCode::Down));
    handle_terminal_event(&mut store, key(KeyCode::End));
    assert!(!app::wants_mouse_capture(&store.state));
    assert!(!app::wants_fullscreen_overlay(&store.state));
}

#[test]
fn pager_during_active_turn_streams_and_returns_to_tail() {
    let mut store = chat_store(3);
    let turn_id = TurnId::new();
    store.state.sessions[0].live_reply = Some(LiveReply {
        turn_id: turn_id.clone(),
        text: "LIVE STREAM MARKER first chunk".into(),
    });
    store.state.set_run_state_in_progress();

    handle_terminal_event(&mut store, ctrl_t());
    let rows = rendered_rows(&store.state, 80, 24);
    assert!(
        rows.iter().any(|row| row.contains("LIVE STREAM MARKER")),
        "the streaming reply must be visible inside the pager; rows: {rows:#?}"
    );

    // More streamed content arrives while the pager is open.
    store.state.sessions[0].live_reply = Some(LiveReply {
        turn_id,
        text: "LIVE STREAM MARKER first chunk SECOND CHUNK".into(),
    });
    let rows = rendered_rows(&store.state, 80, 24);
    assert!(rows.iter().any(|row| row.contains("SECOND CHUNK")));

    // Leaving the pager returns to the inline tail-following view: scroll is
    // reset and the live tail shows the newest streamed content.
    handle_terminal_event(&mut store, key(KeyCode::Esc));
    assert!(!store.state.transcript_pager_active);
    assert_eq!(store.state.transcript_scroll, 0);
    let mut frame = BufferFrame::new(80, 16);
    app::render_viewport(
        &mut frame,
        &store.state,
        Palette::for_theme(ThemeName::default()),
    );
    let rows = frame.rows();
    assert!(
        rows.iter().any(|row| row.contains("SECOND CHUNK")),
        "the inline live tail must follow the newest output after pager exit; rows: {rows:#?}"
    );
}

#[test]
fn pager_with_empty_transcript_renders_safely() {
    // No session at all: the pager must still open and render its empty state.
    let mut store = Store {
        state: AppState::new(vec![], 0, "ready".into(), None, false),
    };
    handle_terminal_event(&mut store, ctrl_t());
    assert!(store.state.transcript_pager_active);
    let rows = rendered_rows(&store.state, 60, 16);
    assert!(rows.iter().any(|row| row.contains("Composer")));

    // The composer keeps accepting input inside the pager...
    handle_terminal_event(&mut store, key(KeyCode::Char('h')));
    handle_terminal_event(&mut store, key(KeyCode::Char('i')));
    assert_eq!(store.state.composer, "hi");

    // ...and with a (still empty) session, Enter submits the prompt.
    let mut store = chat_store(0);
    handle_terminal_event(&mut store, ctrl_t());
    assert!(store.state.transcript_pager_active);
    handle_terminal_event(&mut store, key(KeyCode::Char('h')));
    handle_terminal_event(&mut store, key(KeyCode::Char('i')));
    let action = handle_terminal_event(&mut store, key(KeyCode::Enter));
    assert!(
        matches!(
            action,
            KeyAction::Send(command) if matches!(*command, AppUiCommand::SubmitPrompt(_))
        ),
        "the pinned composer must still submit prompts from inside the pager"
    );
}
