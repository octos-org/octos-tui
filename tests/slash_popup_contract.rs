//! Contract tests for slash-popup visibility
//! (regression lock for `specs/task-activity-group-collapse.spec`).
//!
//! The activity collapse made inline viewports SHORT, which exposed a latent
//! height bug: the render pass re-applied `menu_height_hint`'s terminal-height
//! `-15` heuristic to the viewport's own height, sizing the slash popup to
//! zero rows — open state, reserved space, blank pixels. The popup must render
//! at any viewport height.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::ui_protocol::UiProtocolCapabilities;
use octos_core::{Message, SessionKey};
use octos_tui::client_event::{CapabilitiesClientEvent, ClientEvent};
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::menu::MenuBuildResult;
use octos_tui::model::SessionView;
use octos_tui::model::{
    APPUI_METHOD_MODEL_LIST, APPUI_METHOD_PROFILE_LLM_CATALOG, APPUI_METHOD_PROFILE_LOCAL_CREATE,
    AppState, ConfigCapabilitiesListResult,
};
use octos_tui::store::Store;

#[test]
fn typing_slash_scrollmode_filters_help_popup() {
    // Session open (normal chat), capabilities advertised.
    let mut store = Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:slash-test".into()),
                title: "slash-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            Some("stdio:octos serve --stdio --solo".into()),
            false,
        ),
    };
    store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
        result: ConfigCapabilitiesListResult {
            capabilities: UiProtocolCapabilities::new(
                &[
                    APPUI_METHOD_PROFILE_LOCAL_CREATE,
                    APPUI_METHOD_PROFILE_LLM_CATALOG,
                    APPUI_METHOD_MODEL_LIST,
                ],
                &[],
            ),
        },
        message: "caps".into(),
    }));

    for ch in "/scro".chars() {
        handle_terminal_event(
            &mut store,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }

    assert!(
        store.state.menu_stack.is_active(),
        "slash help popup should be open"
    );
    let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
        panic!("expected ready menu, composer={:?}", store.state.composer);
    };
    let ids: Vec<_> = spec.items.iter().map(|i| i.label.as_str()).collect();
    eprintln!("MENU ID: {}", spec.id);
    eprintln!("COMPOSER: {:?}", store.state.composer);
    eprintln!("ITEMS: {ids:#?}");
    assert!(
        spec.items.iter().any(|i| i.label.contains("scrollmode")),
        "scrollmode should appear in the filtered popup"
    );
}

#[test]
fn typing_slash_onboard_filters_help_popup() {
    // Session open (normal chat), capabilities advertised.
    let mut store = Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:slash-test".into()),
                title: "slash-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            Some("stdio:octos serve --stdio --solo".into()),
            false,
        ),
    };
    store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
        result: ConfigCapabilitiesListResult {
            capabilities: UiProtocolCapabilities::new(
                &[
                    APPUI_METHOD_PROFILE_LOCAL_CREATE,
                    APPUI_METHOD_PROFILE_LLM_CATALOG,
                    APPUI_METHOD_MODEL_LIST,
                ],
                &[],
            ),
        },
        message: "caps".into(),
    }));

    for ch in "/onbo".chars() {
        handle_terminal_event(
            &mut store,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }

    assert!(
        store.state.menu_stack.is_active(),
        "slash help popup should be open"
    );
    let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
        panic!("expected ready menu, composer={:?}", store.state.composer);
    };
    let ids: Vec<_> = spec.items.iter().map(|i| i.label.as_str()).collect();
    eprintln!("MENU ID: {}", spec.id);
    eprintln!("COMPOSER: {:?}", store.state.composer);
    eprintln!("ITEMS: {ids:#?}");
    assert!(
        spec.items.iter().any(|i| i.label.contains("onboard")),
        "onboard should appear in the filtered popup"
    );
}

struct BufferFrame {
    area: ratatui::layout::Rect,
    buffer: ratatui::buffer::Buffer,
}
impl octos_tui::tui_terminal::FrameLike for BufferFrame {
    fn area(&self) -> ratatui::layout::Rect {
        self.area
    }
    fn render_widget<W: ratatui::widgets::Widget>(
        &mut self,
        widget: W,
        area: ratatui::layout::Rect,
    ) {
        widget.render(area, &mut self.buffer);
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(&mut self, _p: P) {}
    fn buffer_mut(&mut self) -> &mut ratatui::buffer::Buffer {
        &mut self.buffer
    }
}

#[test]
fn slash_popup_renders_in_short_viewport() {
    let mut store = Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:slash-render".into()),
                title: "slash-render".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            Some("stdio:octos serve --stdio --solo".into()),
            false,
        ),
    };
    store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
        result: ConfigCapabilitiesListResult {
            capabilities: UiProtocolCapabilities::new(
                &[
                    APPUI_METHOD_PROFILE_LOCAL_CREATE,
                    APPUI_METHOD_PROFILE_LLM_CATALOG,
                    APPUI_METHOD_MODEL_LIST,
                ],
                &[],
            ),
        },
        message: "caps".into(),
    }));
    for ch in "/onbo".chars() {
        handle_terminal_event(
            &mut store,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }

    // The inline viewport sizes itself; render at the computed height.
    let height = octos_tui::app::live_ui_height(&store.state, 100, 40);
    eprintln!("VIEWPORT HEIGHT: {height}");
    let area = ratatui::layout::Rect::new(0, 0, 100, height);
    let mut frame = BufferFrame {
        area,
        buffer: ratatui::buffer::Buffer::empty(area),
    };
    octos_tui::app::render_viewport(
        &mut frame,
        &store.state,
        octos_tui::theme::Palette::for_theme(octos_tui::cli::ThemeName::default()),
    );
    let rows: Vec<String> = (0..height)
        .map(|y| (0..100).map(|x| frame.buffer[(x, y)].symbol()).collect())
        .collect();
    eprintln!("ROWS: {rows:#?}");
    assert!(
        rows.iter().any(|r| r.contains("/onboard")),
        "the popup must be visible in the inline viewport"
    );
}
