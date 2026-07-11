//! Contract tests for the /scrollmode runtime switch
//! (`specs/task-scrollmode-command.spec`).

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{AppState, SessionView};
use octos_tui::store::Store;

fn chat_store() -> Store {
    Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:scrollmode-test".into()),
                title: "scrollmode-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hello")],
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

fn run_command(store: &mut Store, command: &str) {
    store.state.set_composer_text(command);
    handle_terminal_event(
        store,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
}

#[test]
fn bare_scrollmode_toggles() {
    let mut store = chat_store();
    assert!(!store.state.pinned_scroll);

    run_command(&mut store, "/scrollmode");
    assert!(
        store.state.pinned_scroll,
        "bare command toggles native → pinned"
    );

    run_command(&mut store, "/scrollmode");
    assert!(!store.state.pinned_scroll, "and back to native");
}

#[test]
fn explicit_argument_sets_mode() {
    let mut store = chat_store();

    run_command(&mut store, "/scrollmode pinned");
    assert!(store.state.pinned_scroll);
    assert!(
        app::wants_mouse_capture(&store.state),
        "mouse capture policy follows immediately (draw re-syncs next frame)"
    );

    run_command(&mut store, "/scrollmode native");
    assert!(!store.state.pinned_scroll);
    assert!(!app::wants_mouse_capture(&store.state));
}

#[test]
fn unknown_argument_keeps_mode() {
    let mut store = chat_store();

    run_command(&mut store, "/scrollmode banana");

    assert!(
        !store.state.pinned_scroll,
        "unknown argument must not change the mode"
    );
    assert!(
        store.state.status.contains("banana"),
        "the status row names the rejected value; status: {:?}",
        store.state.status
    );
}

#[test]
fn scrollmode_registered_in_command_registry() {
    let mut store = chat_store();

    // The alias resolves through the same registry path.
    run_command(&mut store, "/scroll-mode pinned");

    assert!(
        store.state.pinned_scroll,
        "the alias resolves to the same SetScrollMode action"
    );
}

#[test]
fn popup_enter_dispatches_optional_arg_command() {
    // Codex Enter semantics (slash-popup Enter dispatches the selection):
    // `/scrollmode` takes an OPTIONAL argument, so Enter on the highlighted
    // entry dispatches it bare in ONE Enter — the no-arg form toggles the
    // mode — instead of the old complete-into-composer + second-Enter round
    // trip. Only REQUIRED-arg commands still complete (as "/name ").
    let mut store = chat_store();

    // Type `/scroll`: the popup opens and filters down to scrollmode
    // (substring search over id/label/description).
    for ch in "/scroll".chars() {
        handle_terminal_event(
            &mut store,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    assert!(store.state.menu_stack.is_active(), "popup open");

    // Enter dispatches the bare command: the toggle runs, the composer is
    // cleared, and the popup closes.
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(
        store.state.composer, "",
        "dispatch clears the composer instead of completing into it"
    );
    assert!(
        store.state.pinned_scroll,
        "bare dispatch executes the toggle (native -> pinned)"
    );
    assert!(
        !store.state.menu_stack.is_active(),
        "executing closes the popup"
    );

    // A typed-out command WITH an argument still executes directly.
    for ch in "/scrollmode native".chars() {
        handle_terminal_event(
            &mut store,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(
        !store.state.pinned_scroll,
        "the typed argument applies (pinned -> native)"
    );
    assert!(
        !store.state.menu_stack.is_active(),
        "executing closes the popup"
    );
}

#[test]
fn popup_entry_shows_current_mode() {
    use octos_tui::menu::MenuBuildResult;
    let mut store = chat_store();

    let entry_desc = |store: &mut Store| -> String {
        for ch in "/scroll".chars() {
            handle_terminal_event(
                store,
                Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
            );
        }
        let Some(MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
            panic!("popup open");
        };
        let desc = spec
            .items
            .iter()
            .find(|i| i.id == "scrollmode")
            .expect("scrollmode entry")
            .description
            .clone()
            .unwrap_or_default();
        // Close the popup and clear the draft for the next round.
        handle_terminal_event(
            store,
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        store.state.set_composer_text("");
        store.close_all_menus();
        desc
    };

    let desc = entry_desc(&mut store);
    assert!(
        desc.contains("native"),
        "entry surfaces the current mode; desc: {desc:?}"
    );

    run_command(&mut store, "/scrollmode pinned");
    let desc = entry_desc(&mut store);
    assert!(
        desc.contains("pinned"),
        "after switching, the entry reflects the new mode; desc: {desc:?}"
    );
}
