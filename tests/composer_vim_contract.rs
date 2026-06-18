//! Contract tests for composer Vim modal editing
//! (`specs/task-composer-vim-mode.spec`).
//!
//! Vim is opt-in (`vim_mode`); when off the composer is a plain text field.
//! When on: Esc → Normal, i → Insert; Normal interprets hjkl/w/b/e/0/$/gg/G/
//! x/dd/dw/cc/i/a/A/I/o/O; Enter always submits.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::SessionKey;
use octos_tui::event_loop::{KeyAction, handle_terminal_event};
use octos_tui::model::{AppState, ComposerMode, FocusPane, SessionView};
use octos_tui::store::Store;

fn store_with(text: &str, cursor: Option<usize>, vim: bool, mode: ComposerMode) -> Store {
    let session = SessionView {
        id: SessionKey("local:vim-test".into()),
        title: "vim-test".into(),
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
    store.state.vim_mode = vim;
    store.state.composer_mode = mode;
    store.state.composer = text.into();
    store.state.composer_cursor = cursor;
    store
}

fn normal(text: &str, cursor: usize) -> Store {
    store_with(text, Some(cursor), true, ComposerMode::Normal)
}

fn press(store: &mut Store, c: char) -> KeyAction {
    handle_terminal_event(
        &mut *store,
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
    )
}

fn press_key(store: &mut Store, code: KeyCode) -> KeyAction {
    handle_terminal_event(
        &mut *store,
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
    )
}

// ----- toggle-safety -----

#[test]
fn vim_disabled_by_default_types_normally() {
    let mut store = store_with("", Some(0), false, ComposerMode::Insert);
    press(&mut store, 'h');
    assert_eq!(
        store.state.composer, "h",
        "with Vim off, 'h' is inserted as text, not treated as a motion"
    );
}

#[test]
fn vimmode_slash_toggles_enabled() {
    let mut store = store_with("", None, false, ComposerMode::Insert);
    store.state.set_composer_text("/vimmode");
    let _ = store.compose_command();
    assert!(store.state.vim_mode, "/vimmode enables Vim");
    assert_eq!(
        store.state.composer_mode,
        ComposerMode::Insert,
        "enabling Vim starts in Insert"
    );
}

#[test]
fn vim_mode_parses_from_config_file() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("octos-vim-{nonce}.json"));
    std::fs::write(&path, r#"{ "vim-mode": true }"#).unwrap();
    let cfg = octos_tui::cli::load_config_file(&path).expect("parses");
    assert_eq!(cfg.vim_mode, Some(true));
    let _ = std::fs::remove_file(&path);
}

// ----- mode-switch -----

#[test]
fn esc_enters_normal_mode() {
    let mut store = store_with("ab", Some(2), true, ComposerMode::Insert);
    press_key(&mut store, KeyCode::Esc);
    assert_eq!(store.state.composer_mode, ComposerMode::Normal);
    // A printable key in Normal no longer inserts text.
    press(&mut store, 'z');
    assert_eq!(store.state.composer, "ab", "Normal-mode keys don't insert");
}

#[test]
fn i_enters_insert_mode() {
    let mut store = normal("ab", 0);
    press(&mut store, 'i');
    assert_eq!(store.state.composer_mode, ComposerMode::Insert);
    press(&mut store, 'X');
    assert_eq!(store.state.composer, "Xab", "Insert-mode keys insert again");
}

// ----- normal-motions -----

#[test]
fn normal_hjkl_moves_cursor_without_editing() {
    let mut store = normal("abc\nde", 0);
    press(&mut store, 'l'); // → col 1
    press(&mut store, 'j'); // → line 2
    assert_eq!(store.state.composer, "abc\nde", "motions never edit text");
    assert_eq!(
        store.state.composer_cursor_index(),
        5,
        "l then j lands on line 2"
    );
}

#[test]
fn normal_line_start_and_end() {
    let mut store = normal("hello", 2);
    press(&mut store, '0');
    assert_eq!(store.state.composer_cursor_index(), 0);
    press(&mut store, '$');
    assert_eq!(store.state.composer_cursor_index(), 5);
}

#[test]
fn normal_word_motions() {
    let mut store = normal("foo bar baz", 0);
    press(&mut store, 'w');
    assert_eq!(
        store.state.composer_cursor_index(),
        4,
        "w → next word start"
    );
    press(&mut store, 'e');
    assert_eq!(store.state.composer_cursor_index(), 6, "e → end of 'bar'");
    press(&mut store, 'b');
    assert_eq!(
        store.state.composer_cursor_index(),
        4,
        "b → back to 'bar' start"
    );
}

#[test]
fn normal_gg_and_g_buffer_bounds() {
    let mut store = normal("one\ntwo\nthree", 5);
    press(&mut store, 'g');
    press(&mut store, 'g');
    assert_eq!(store.state.composer_cursor_index(), 0, "gg → buffer start");
    press(&mut store, 'G');
    assert_eq!(
        store.state.composer_cursor_index(),
        "one\ntwo\nthree".len(),
        "G → buffer end"
    );
}

// ----- normal-edits -----

#[test]
fn normal_x_deletes_char() {
    let mut store = normal("abc", 0);
    press(&mut store, 'x');
    assert_eq!(store.state.composer, "bc");
}

#[test]
fn normal_dd_deletes_line() {
    let mut store = normal("one\ntwo\nthree", 4); // cursor on "two"
    press(&mut store, 'd');
    press(&mut store, 'd');
    assert_eq!(store.state.composer, "one\nthree");
}

#[test]
fn normal_dw_deletes_word() {
    let mut store = normal("foo bar", 0);
    press(&mut store, 'd');
    press(&mut store, 'w');
    assert_eq!(store.state.composer, "bar");
}

#[test]
fn normal_cc_changes_line() {
    let mut store = normal("one\ntwo", 1); // cursor on line 1
    press(&mut store, 'c');
    press(&mut store, 'c');
    assert_eq!(store.state.composer, "\ntwo", "cc clears the line content");
    assert_eq!(
        store.state.composer_mode,
        ComposerMode::Insert,
        "cc enters Insert"
    );
}

// ----- insert-entry -----

#[test]
fn normal_insert_entry_variants_position_cursor() {
    // a: right + Insert
    let mut s = normal("abc", 0);
    press(&mut s, 'a');
    assert_eq!(
        (s.state.composer_cursor_index(), s.state.composer_mode),
        (1, ComposerMode::Insert)
    );
    // A: line end + Insert
    let mut s = normal("abc", 0);
    press(&mut s, 'A');
    assert_eq!(
        (s.state.composer_cursor_index(), s.state.composer_mode),
        (3, ComposerMode::Insert)
    );
    // I: line start + Insert
    let mut s = normal("abc", 2);
    press(&mut s, 'I');
    assert_eq!(
        (s.state.composer_cursor_index(), s.state.composer_mode),
        (0, ComposerMode::Insert)
    );
    // o: open line below + Insert
    let mut s = normal("abc", 1);
    press(&mut s, 'o');
    assert_eq!(s.state.composer, "abc\n");
    assert_eq!(s.state.composer_mode, ComposerMode::Insert);
    // O: open line above + Insert
    let mut s = normal("abc", 1);
    press(&mut s, 'O');
    assert_eq!(s.state.composer, "\nabc");
    assert_eq!(
        (s.state.composer_cursor_index(), s.state.composer_mode),
        (0, ComposerMode::Insert)
    );
}

// ----- submit-no-regress -----

#[test]
fn normal_enter_still_submits() {
    let mut store = normal("hello", 0);
    let action = press_key(&mut store, KeyCode::Enter);
    assert!(
        matches!(action, KeyAction::Send(_)),
        "Enter submits in Normal mode too"
    );
}

#[test]
fn normal_x_on_empty_is_safe() {
    let mut store = normal("", 0);
    press(&mut store, 'x');
    assert_eq!(
        store.state.composer, "",
        "x on empty is a no-op, not a panic"
    );
}
