//! Contract tests for `/activity` navigator overlay
//! (`specs/task-activity-navigator-overlay.spec`).

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::ui_protocol::TaskRuntimeState;
use octos_core::{Message, SessionKey, TaskId};
use octos_tui::app::{self, ActivityNavigatorStatus};
use octos_tui::cli::ThemeName;
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{
    ActivityItem, ActivityKind, ActivityNavigatorFilter, AppState, SessionView, TaskView,
};
use octos_tui::store::Store;
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

fn task(title: &str, state: TaskRuntimeState, detail: Option<&str>, output: &str) -> TaskView {
    TaskView {
        id: TaskId::new(),
        title: title.into(),
        state,
        runtime_detail: detail.map(str::to_string),
        output_tail: output.into(),
        turn_id: None,
    }
}

fn store_with_tasks(tasks: Vec<TaskView>) -> Store {
    Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:activity-test".into()),
                title: "activity-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("status?")],
                tasks,
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

fn char_key(ch: char) -> Event {
    key(KeyCode::Char(ch))
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

#[test]
fn activity_navigator_model_counts_statuses() {
    let mut store = store_with_tasks(vec![
        task("running task", TaskRuntimeState::Running, None, ""),
        task("completed task", TaskRuntimeState::Completed, None, ""),
    ]);
    store.state.set_run_state_blocked("approval required");
    store.state.push_activity(
        ActivityItem::new(ActivityKind::Error, "failed tool", "failed")
            .with_detail("needle failure"),
    );

    let model = app::activity_navigator_model(&store.state);

    assert_eq!(model.counts.running, 1);
    assert_eq!(model.counts.blocked, 1);
    assert_eq!(model.counts.failed, 1);
    assert_eq!(model.counts.done, 2);
    assert_eq!(model.counts.all, 5);
}

#[test]
fn activity_navigator_search_matches_task_and_activity_detail() {
    let mut store = store_with_tasks(vec![task(
        "protocol task",
        TaskRuntimeState::Running,
        Some("needle-task-detail"),
        "",
    )]);
    store.state.push_activity(
        ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_detail("needle-activity-detail"),
    );

    store.state.activity_navigator.query = "needle-task-detail".into();
    let task_model = app::activity_navigator_model(&store.state);
    assert_eq!(task_model.rows.len(), 1);
    assert_eq!(task_model.rows[0].kind.label(), "task");

    store.state.activity_navigator.query = "needle-activity-detail".into();
    let activity_model = app::activity_navigator_model(&store.state);
    assert_eq!(activity_model.rows.len(), 1);
    assert_eq!(activity_model.rows[0].kind.label(), "activity");
}

#[test]
fn activity_navigator_search_matches_session_messages() {
    let mut store = store_with_tasks(vec![]);
    store.state.sessions[0].messages = vec![
        Message::user("please compare the Rust tui options"),
        Message::assistant("ratatui is one candidate"),
    ];
    store.state.activity_navigator.open();

    for ch in "rust".chars() {
        handle_terminal_event(&mut store, char_key(ch));
    }

    let model = app::activity_navigator_model(&store.state);
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].kind.label(), "message");
    assert!(model.rows[0].title.contains("Rust tui options"));
}

#[test]
fn activity_navigator_search_omits_system_messages() {
    let mut store = store_with_tasks(vec![]);
    store.state.sessions[0].messages = vec![
        Message::system("system secret should stay hidden"),
        Message::user("visible user message"),
    ];
    store.state.activity_navigator.open();
    store.state.activity_navigator.query = "system secret".into();

    let model = app::activity_navigator_model(&store.state);

    assert!(model.rows.is_empty());
    store.state.activity_navigator.clear_query();
    let rows = rendered_rows(&store.state, 100, 28);
    let text = rows.join("\n");
    assert!(!text.contains("system secret"));
}

#[test]
fn activity_navigator_open_resets_query_and_filter_to_all() {
    let mut store = store_with_tasks(vec![task(
        "running task",
        TaskRuntimeState::Running,
        None,
        "",
    )]);
    store.state.activity_navigator.query = "stale-query".into();
    store.state.activity_navigator.filter = ActivityNavigatorFilter::Done;

    store.state.activity_navigator.open();

    assert_eq!(store.state.activity_navigator.query, "");
    assert_eq!(
        store.state.activity_navigator.filter,
        ActivityNavigatorFilter::All
    );
    let model = app::activity_navigator_model(&store.state);
    assert!(!model.rows.is_empty());
}

#[test]
fn activity_navigator_typing_starts_search_and_filters() {
    let mut store = store_with_tasks(vec![
        task(
            "needle-direct task",
            TaskRuntimeState::Running,
            Some("visible after direct search"),
            "",
        ),
        task("other task", TaskRuntimeState::Completed, None, ""),
    ]);
    store.state.activity_navigator.open();

    for ch in "needle-direct".chars() {
        handle_terminal_event(&mut store, char_key(ch));
    }

    assert!(store.state.activity_navigator.search_active);
    assert_eq!(store.state.activity_navigator.query, "needle-direct");
    let model = app::activity_navigator_model(&store.state);
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].title, "needle-direct task");
}

#[test]
fn activity_navigator_search_is_case_insensitive() {
    let mut store = store_with_tasks(vec![]);
    store.state.push_activity(
        ActivityItem::new(
            ActivityKind::Progress,
            "file_mutation",
            "File mutation: modify src/lib.rs | diff preview ready",
        )
        .with_detail("modify src/lib.rs | diff preview ready"),
    );
    store.state.activity_navigator.open();

    handle_terminal_event(&mut store, char_key('/'));
    for ch in "RUST".chars() {
        handle_terminal_event(&mut store, char_key(ch));
    }

    assert_eq!(store.state.activity_navigator.query, "RUST");
    let model = app::activity_navigator_model(&store.state);
    assert_eq!(model.rows.len(), 1);
    assert_eq!(model.rows[0].kind.label(), "change");
}

#[test]
fn activity_navigator_empty_state_names_query_and_filter() {
    let mut store = store_with_tasks(vec![task(
        "visible task",
        TaskRuntimeState::Running,
        None,
        "",
    )]);
    store.state.activity_navigator.open();
    store.state.activity_navigator.query = "does-not-exist".into();

    let rows = rendered_rows(&store.state, 100, 28);
    let text = rows.join("\n");

    assert!(text.contains("does-not-exist"));
    assert!(text.contains("filter: all"));
}

#[test]
fn activity_navigator_filter_running_only() {
    let mut store = store_with_tasks(vec![
        task("running task", TaskRuntimeState::Running, None, ""),
        task("completed task", TaskRuntimeState::Completed, None, ""),
    ]);
    store.state.activity_navigator.filter = ActivityNavigatorFilter::Running;

    let model = app::activity_navigator_model(&store.state);

    assert!(!model.rows.is_empty());
    assert!(
        model
            .rows
            .iter()
            .all(|row| row.status == ActivityNavigatorStatus::Running)
    );
}

#[test]
fn activity_navigator_file_mutations_render_as_recent_changes() {
    let mut store = store_with_tasks(vec![]);
    store.state.push_activity(
        ActivityItem::new(
            ActivityKind::Progress,
            "file_mutation",
            "File mutation: modify src/lib.rs | diff preview ready",
        )
        .with_detail("modify src/lib.rs | diff preview ready"),
    );

    let model = app::activity_navigator_model(&store.state);
    let row = model
        .rows
        .iter()
        .find(|row| row.kind.label() == "change")
        .expect("file mutation row");

    assert_eq!(model.counts.changes, 1);
    assert_eq!(row.title, "RUST modify src/lib.rs");
    assert!(row.subtitle.contains("RUST"));
    assert!(row.subtitle.contains("modify"));
    assert!(row.subtitle.contains("diff preview ready"));
    assert!(
        row.detail_lines
            .iter()
            .any(|line| line == "file: src/lib.rs")
    );
}

#[test]
fn activity_navigator_toolbar_counts_recent_changes() {
    let mut store = store_with_tasks(vec![]);
    store.state.push_activity(
        ActivityItem::new(
            ActivityKind::Progress,
            "file_mutation",
            "File mutation: modify src/lib.rs | diff preview ready",
        )
        .with_detail("modify src/lib.rs | diff preview ready"),
    );
    store.state.activity_navigator.open();

    let rows = rendered_rows(&store.state, 100, 28);
    let text = rows.join("\n");

    assert!(text.contains("changes 1"));
    assert!(text.contains("change"));
    assert!(text.contains("RUST"));
    assert!(text.contains("src/lib.rs"));
}

#[test]
fn slash_activity_opens_activity_navigator_overlay() {
    let mut store = store_with_tasks(vec![]);
    store.state.composer = "/activity".into();

    handle_terminal_event(&mut store, key(KeyCode::Enter));

    assert!(store.state.activity_navigator.active);
    assert!(app::wants_fullscreen_overlay(&store.state));
}

#[test]
fn activity_navigator_overlay_renders_results_and_detail() {
    let mut store = store_with_tasks(vec![task(
        "rendered running task",
        TaskRuntimeState::Running,
        Some("rendered detail"),
        "tail line from task",
    )]);
    store.state.activity_navigator.open();

    let rows = rendered_rows(&store.state, 100, 28);
    let text = rows.join("\n");

    assert!(text.contains("Activity"));
    assert!(text.contains("rendered running task"));
    assert!(text.contains("rendered detail") || text.contains("tail line from task"));
}

#[test]
fn activity_navigator_escape_closes_without_touching_pager_state() {
    let mut store = store_with_tasks(vec![task("running", TaskRuntimeState::Running, None, "")]);
    store.state.activity_navigator.open();
    store.state.transcript_scroll = 7;

    handle_terminal_event(&mut store, key(KeyCode::Esc));

    assert!(!store.state.activity_navigator.active);
    assert_eq!(store.state.transcript_scroll, 7);
}
