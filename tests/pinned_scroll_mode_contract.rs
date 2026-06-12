//! Contract tests for the `scroll-mode` launch option
//! (`specs/task-pinned-scroll-mode.spec`).
//!
//! `native` (default) keeps the wheel on the terminal: native selection/copy
//! survive and the composer scrolls away with the screen (the pager is the
//! pinned view, entered via Ctrl+T / PageUp). `pinned` is the explicit opt-in
//! that captures the mouse so wheel-up auto-enters the pager (composer pinned)
//! and wheel-down at the pager bottom drops back to the inline tail.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::cli::{ScrollMode, ThemeName, load_config_file};
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{AppState, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

fn chat_store(message_count: usize, pinned: bool) -> Store {
    let messages = (1..=message_count)
        .flat_map(|idx| {
            [
                Message::user(format!("ask number {idx:02}")),
                Message::assistant(format!("history message {idx:02}")),
            ]
        })
        .collect();
    let mut store = Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:scroll-mode-test".into()),
                title: "scroll-mode-test".into(),
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
    };
    store.state.pinned_scroll = pinned;
    store
}

fn wheel(kind: MouseEventKind) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
}

struct BufferFrame {
    area: Rect,
    buffer: Buffer,
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
    let area = Rect::new(0, 0, width, height);
    let mut frame = BufferFrame {
        area,
        buffer: Buffer::empty(area),
    };
    app::render(&mut frame, state, Palette::for_theme(ThemeName::default()));
    (0..height)
        .map(|y| (0..width).map(|x| frame.buffer[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn native_mode_default_keeps_mouse_capture_off() {
    let store = chat_store(6, false);

    assert!(!store.state.pinned_scroll, "native is the default");
    assert!(
        !app::wants_mouse_capture(&store.state),
        "native mode must never request mouse capture in the chat flow"
    );

    // Committed history stays out of the inline viewport — it lives in the
    // terminal's native scrollback, which is what native mode preserves.
    let area = Rect::new(0, 0, 80, 12);
    let mut frame = BufferFrame {
        area,
        buffer: Buffer::empty(area),
    };
    app::render_viewport(
        &mut frame,
        &store.state,
        Palette::for_theme(ThemeName::default()),
    );
    let rows: Vec<String> = (0..12)
        .map(|y| (0..80).map(|x| frame.buffer[(x, y)].symbol()).collect())
        .collect();
    assert!(!rows.iter().any(|row| row.contains("history message")));
}

#[test]
fn native_mode_wheel_does_not_enter_pager() {
    let mut store = chat_store(6, false);

    handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollUp));

    assert!(
        !store.state.transcript_pager_active,
        "native mode wheel events must not hijack the surface into the pager"
    );
    assert_eq!(
        store.state.transcript_scroll, 4,
        "live-tail scrolling keeps its existing behavior"
    );
}

#[test]
fn pinned_mode_requests_mouse_capture_inline() {
    let store = chat_store(6, true);

    assert!(
        app::wants_mouse_capture(&store.state),
        "pinned mode routes the wheel into the app even in the inline chat flow"
    );
}

#[test]
fn pinned_mode_wheel_up_enters_pager() {
    let mut store = chat_store(30, true);

    handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollUp));

    assert!(
        store.state.transcript_pager_active,
        "the first wheel-up opens the pager (committed history is unreachable inline)"
    );
    assert_eq!(
        store.state.transcript_scroll, 0,
        "the pager opens at the bottom with the latest content visible"
    );

    // The composer stays pinned at the bottom while content scrolls above it.
    for _ in 0..10 {
        handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollUp));
    }
    assert!(store.state.transcript_scroll > 0);
    let rows = rendered_rows(&store.state, 60, 20);
    let composer_row = rows
        .iter()
        .position(|row| row.contains("Composer"))
        .expect("composer rendered");
    assert!(
        composer_row >= rows.len() - 6,
        "composer must sit in the bottom rows, found at row {composer_row}"
    );
}

#[test]
fn pinned_mode_wheel_down_at_bottom_exits_pager() {
    // Pinned mode: wheel down at the pager bottom returns to the inline tail.
    let mut store = chat_store(8, true);
    handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollUp));
    assert!(store.state.transcript_pager_active);
    assert_eq!(store.state.transcript_scroll, 0);

    handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollDown));
    assert!(
        !store.state.transcript_pager_active,
        "wheel-down at the pager bottom must drop back to the inline view"
    );

    // Native mode: a manually opened pager holds its position at the bottom.
    let mut store = chat_store(8, false);
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    assert!(store.state.transcript_pager_active);
    handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollDown));
    assert!(
        store.state.transcript_pager_active,
        "native mode must not auto-exit a pager the user opened deliberately"
    );
}

#[test]
fn scroll_mode_parses_from_config_file() {
    let dir = std::env::temp_dir().join(format!("octos-tui-scroll-mode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let pinned_path = dir.join("pinned.json");
    std::fs::write(&pinned_path, r#"{ "scroll-mode": "pinned" }"#).expect("write config");
    let config = load_config_file(&pinned_path).expect("config parses");
    assert_eq!(config.scroll_mode, Some(ScrollMode::Pinned));

    let alias_path = dir.join("alias.json");
    std::fs::write(&alias_path, r#"{ "scroll_mode": "native" }"#).expect("write config");
    let config = load_config_file(&alias_path).expect("config parses");
    assert_eq!(config.scroll_mode, Some(ScrollMode::Native));

    let empty_path = dir.join("empty.json");
    std::fs::write(&empty_path, "{}").expect("write config");
    let config = load_config_file(&empty_path).expect("config parses");
    assert_eq!(
        config.scroll_mode, None,
        "unset key stays None so the launch default resolves to native"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
