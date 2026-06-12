//! Contract tests for settled activity-group collapse
//! (`specs/task-activity-group-collapse.spec`).

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::ui_protocol::TurnId;
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::app::LiveTurnFinalization;
use octos_tui::cli::ThemeName;
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{ActivityItem, ActivityKind, AppState, LiveReply, SessionView};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

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
    fn set_cursor_position<P: Into<Position>>(&mut self, _p: P) {}
    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }
}

fn rows(state: &AppState) -> Vec<String> {
    let area = Rect::new(0, 0, 100, 30);
    let mut frame = BufferFrame {
        area,
        buffer: Buffer::empty(area),
    };
    app::render(&mut frame, state, Palette::for_theme(ThemeName::default()));
    (0..30)
        .map(|y| (0..100).map(|x| frame.buffer[(x, y)].symbol()).collect())
        .collect()
}

fn store_with_settled_activity() -> Store {
    let mut store = Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:collapse-test".into()),
                title: "collapse-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run checks")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        ),
    };
    store.state.push_activity(
        ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_detail("cargo test --workspace")
            .with_success(true),
    );
    store
}

fn ctrl(ch: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL))
}

#[test]
fn settled_group_collapses_to_header() {
    let mut store = store_with_settled_activity();
    handle_terminal_event(&mut store, ctrl('t'));

    let all = rows(&store.state);
    assert!(
        !all.iter().any(|row| row.contains("cargo test --workspace")),
        "settled child rows must be hidden by default; rows: {all:#?}"
    );
    assert!(
        all.iter().any(|row| row.contains("(1")),
        "the one-line header summary (action count) remains; rows: {all:#?}"
    );
}

#[test]
fn expanded_group_shows_children() {
    let mut store = store_with_settled_activity();
    handle_terminal_event(&mut store, ctrl('t'));
    handle_terminal_event(&mut store, ctrl('o'));
    assert!(store.state.expanded_tool_outputs);

    let all = rows(&store.state);
    assert!(
        all.iter().any(|row| row.contains("cargo test --workspace")),
        "Ctrl+O expands the children; rows: {all:#?}"
    );
}

#[test]
fn active_group_never_collapses() {
    let mut store = store_with_settled_activity();
    let turn_id = TurnId::new();
    store.state.sessions[0].live_reply = Some(LiveReply {
        turn_id: turn_id.clone(),
        text: "working".into(),
    });
    store.state.set_run_state_in_progress();
    store.state.push_activity(
        ActivityItem::new(ActivityKind::Tool, "shell", "running")
            .with_turn(turn_id)
            .with_detail("cargo clippy --all-targets"),
    );

    let all = rows(&store.state);
    assert!(
        all.iter()
            .any(|row| row.contains("cargo clippy --all-targets")),
        "an in-progress group keeps its live children visible; rows: {all:#?}"
    );
}

#[test]
fn scrollback_flush_keeps_children() {
    // A settled turn's activity log flushed through the late-activity path:
    // the archive must contain the full child rows even though the repainting
    // views collapse them.
    let mut store = store_with_settled_activity();
    let turn_id = TurnId::new();
    store
        .state
        .turn_activity_logs
        .push(octos_tui::model::TurnActivityLog {
            session_id: SessionKey("local:collapse-test".into()),
            turn_id: turn_id.clone(),
            request: None,
            anchor_index: None,
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id.clone())
                    .with_detail("cargo test --workspace")
                    .with_success(true),
            ],
        });
    let coverage = LiveTurnFinalization {
        session_id: "local:collapse-test".into(),
        turn_id: turn_id.0.to_string(),
        ..Default::default()
    };
    let lines = octos_tui::app::finalized_late_activity_lines_for_coverages(
        &store.state,
        Palette::for_theme(ThemeName::default()),
        100,
        std::slice::from_ref(&coverage),
    );
    let text: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert!(
        text.iter()
            .any(|row| row.contains("cargo test --workspace")),
        "the scrollback archive always keeps the full child rows; lines: {text:#?}"
    );
}
