//! Contract tests for transcript role contrast
//! (`specs/task-transcript-role-contrast.spec`).
//!
//! Three visual tiers: user input is the transcript's anchor (accent `▌`
//! gutter + bold body, no background), agent reply prose is the untouched
//! baseline (`• ` prefix, text color), and runtime/tool activity is uniformly
//! muted so the log never outweighs the conversation.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::cli::ThemeName;
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{ActivityItem, ActivityKind, AppState, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;

fn chat_store() -> Store {
    Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:role-contrast-test".into()),
                title: "role-contrast-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("first line of my ask\nsecond line of my ask"),
                    Message::assistant("the reply prose body"),
                ],
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

/// Render the full-screen chat layout (pager active so backgrounds are the
/// terminal default — the strictest surface for the no-background contract).
fn rendered(state: &AppState, width: u16, height: u16) -> BufferFrame {
    let area = Rect::new(0, 0, width, height);
    let mut frame = BufferFrame {
        area,
        buffer: Buffer::empty(area),
    };
    app::render(&mut frame, state, Palette::for_theme(ThemeName::default()));
    frame
}

fn rows(frame: &BufferFrame) -> Vec<String> {
    (0..frame.area.height)
        .map(|y| {
            (0..frame.area.width)
                .map(|x| frame.buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

fn cell_at_text<'a>(frame: &'a BufferFrame, needle: &str) -> Option<&'a ratatui::buffer::Cell> {
    let all = rows(frame);
    for (y, row) in all.iter().enumerate() {
        if let Some(col) = row.find(needle) {
            // `find` returns a byte offset; rows here are ASCII for needles we
            // search, but guard with char-boundary-safe scan anyway.
            let x = row[..col].chars().count();
            return Some(&frame.buffer[(x as u16, y as u16)]);
        }
    }
    None
}

#[test]
fn user_message_renders_accent_gutter_bold() {
    let mut store = chat_store();
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    let frame = rendered(&store.state, 70, 24);
    let all = rows(&frame);

    let palette = Palette::for_theme(ThemeName::default());
    for needle in ["first line of my ask", "second line of my ask"] {
        let row = all
            .iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row with {needle:?} missing; rows: {all:#?}"));
        assert!(
            row.trim_start().starts_with('▌'),
            "every logical user line carries the gutter; row: {row:?}"
        );
        let body_cell = cell_at_text(&frame, needle).expect("body cell");
        assert!(
            body_cell.modifier.contains(Modifier::BOLD),
            "user body must be bold"
        );
    }
    let gutter_cell = cell_at_text(&frame, "▌").expect("gutter cell");
    assert_eq!(
        gutter_cell.fg, palette.accent,
        "the gutter bar uses the accent color"
    );
}

#[test]
fn user_message_has_no_background() {
    let mut store = chat_store();
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    let frame = rendered(&store.state, 70, 24);

    let body_cell = cell_at_text(&frame, "first line of my ask").expect("body cell");
    assert_eq!(
        body_cell.bg,
        Color::Reset,
        "user messages carry no bubble background in the pager"
    );
}

#[test]
fn activity_rows_are_muted_without_bold() {
    let mut store = chat_store();
    store.state.push_activity(
        ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_tool_call("call-1")
            .with_detail("cargo test --workspace")
            .with_success(true),
    );
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    // Settled groups collapse by default now; expand to inspect child styling.
    store.state.expanded_tool_outputs = true;
    let frame = rendered(&store.state, 90, 24);

    let palette = Palette::for_theme(ThemeName::default());
    // Tool activity renders as a Claude-Code-style card (`⏺ Bash($ cmd)`);
    // the muted-label contract now holds on the card's tool name.
    let label_cell = cell_at_text(&frame, "Bash(").expect("tool label cell");
    assert_eq!(
        label_cell.fg, palette.muted,
        "activity tool labels render muted"
    );
    assert!(
        !label_cell.modifier.contains(Modifier::BOLD),
        "activity rows must not be bold — the log never outweighs the prose"
    );
}

#[test]
fn assistant_body_style_unchanged() {
    let mut store = chat_store();
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    let frame = rendered(&store.state, 70, 24);
    let all = rows(&frame);

    let palette = Palette::for_theme(ThemeName::default());
    let row = all
        .iter()
        .find(|row| row.contains("the reply prose body"))
        .expect("assistant row");
    assert!(
        row.contains("• "),
        "assistant prose keeps its bullet prefix; row: {row:?}"
    );
    assert!(
        !row.trim_start().starts_with('▌'),
        "the gutter is exclusive to user input"
    );
    let body_cell = cell_at_text(&frame, "the reply prose body").expect("assistant cell");
    assert_eq!(body_cell.fg, palette.text);
    assert!(!body_cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn pager_and_inline_share_role_styling() {
    // Pager (full transcript) — gutter present.
    let mut store = chat_store();
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    let pager_frame = rendered(&store.state, 70, 24);
    assert!(
        rows(&pager_frame)
            .iter()
            .any(|row| row.contains("▌ first line of my ask")),
        "pager renders the user gutter"
    );

    // Inline live tail (recent-user context pinned for an active turn) —
    // same gutter language, no background.
    let mut store = chat_store();
    store.state.set_run_state_in_progress();
    let area = Rect::new(0, 0, 70, 16);
    let mut frame = BufferFrame {
        area,
        buffer: Buffer::empty(area),
    };
    app::render_viewport(
        &mut frame,
        &store.state,
        Palette::for_theme(ThemeName::default()),
    );
    let inline_rows = rows(&frame);
    let user_row = inline_rows
        .iter()
        .find(|row| row.contains("second line of my ask"));
    if let Some(row) = user_row {
        assert!(
            row.trim_start().starts_with('▌'),
            "inline live tail uses the same gutter language; row: {row:?}"
        );
    }
    assert!(
        !inline_rows.iter().any(|row| row.contains("› first line")),
        "the old plain prefix must not reappear anywhere"
    );
}
