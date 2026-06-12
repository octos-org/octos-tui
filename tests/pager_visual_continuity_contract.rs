//! Contract tests for pager visual continuity
//! (`specs/task-pager-visual-continuity.spec`).
//!
//! Pinned-mode wheel scrolling enters the pager seamlessly, so the pager must
//! not flip the screen to the theme surface color ("screen went black") and
//! must signal the read position in the status row, since the alt-screen has
//! no native scrollbar.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::cli::ThemeName;
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{AppState, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use ratatui::widgets::Widget;

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
                id: SessionKey("local:pager-visual-test".into()),
                title: "pager-visual-test".into(),
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

fn rendered_frame(state: &AppState, width: u16, height: u16) -> BufferFrame {
    let mut frame = BufferFrame::new(width, height);
    app::render(&mut frame, state, Palette::for_theme(ThemeName::default()));
    frame
}

fn status_row(state: &AppState, width: u16, height: u16) -> String {
    let frame = rendered_frame(state, width, height);
    (0..width)
        .map(|x| frame.buffer[(x, height - 1)].symbol())
        .collect()
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl_t() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL))
}

#[test]
fn pager_transcript_uses_default_background() {
    let mut store = chat_store(10);
    handle_terminal_event(&mut store, ctrl_t());
    assert!(store.state.transcript_pager_active);

    let frame = rendered_frame(&store.state, 60, 20);
    // A cell well inside the transcript pane (above the composer rows).
    let cell = &frame.buffer[(30, 2)];
    assert_eq!(
        cell.bg,
        Color::Reset,
        "the pager transcript must blend with the terminal default background"
    );
}

#[test]
fn non_pager_fullscreen_keeps_surface_background() {
    let store = chat_store(10);
    assert!(!store.state.transcript_pager_active);

    let frame = rendered_frame(&store.state, 60, 20);
    let cell = &frame.buffer[(30, 2)];
    assert_eq!(
        cell.bg,
        Palette::for_theme(ThemeName::default()).surface_alt,
        "non-pager full-screen surfaces keep the existing surface_alt background"
    );
}

#[test]
fn pager_status_shows_reviewing_indicator() {
    let mut store = chat_store(30);
    handle_terminal_event(&mut store, ctrl_t());
    for _ in 0..3 {
        handle_terminal_event(&mut store, key(KeyCode::PageUp));
    }
    assert!(store.state.transcript_scroll > 0);

    let row = status_row(&store.state, 220, 24);
    assert!(
        row.contains("Reviewing"),
        "scrolled pager must surface the reviewing indicator; status row: {row:?}"
    );
}

#[test]
fn pager_status_hides_indicator_at_bottom() {
    let mut store = chat_store(30);
    handle_terminal_event(&mut store, ctrl_t());
    assert_eq!(store.state.transcript_scroll, 0);

    let row = status_row(&store.state, 220, 24);
    assert!(
        !row.contains("Reviewing"),
        "at the bottom the indicator must disappear; status row: {row:?}"
    );
    assert!(
        row.contains("PgUp/PgDn"),
        "the plain pager key hint must remain; status row: {row:?}"
    );
}

#[test]
fn pager_message_blocks_have_no_span_background() {
    let mut store = chat_store(10);
    handle_terminal_event(&mut store, ctrl_t());

    // Every cell in the transcript pane must sit on the terminal default
    // background — message-block "bubble" colors would paint text-shaped
    // stripes over the terminal theme (the reported "black backgrounds").
    let frame = rendered_frame(&store.state, 60, 20);
    let transcript_rows = 20 - 8; // above composer block + status row
    for y in 0..transcript_rows {
        for x in 0..60 {
            let cell = &frame.buffer[(x, y)];
            assert_eq!(
                cell.bg,
                Color::Reset,
                "cell ({x},{y}) must keep the default background, found {:?}",
                cell.bg
            );
        }
    }
}
