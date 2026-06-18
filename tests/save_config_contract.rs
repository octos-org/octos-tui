//! Contract tests for /saveconfig runtime config persistence
//! (`specs/task-config-persistence.spec`).
//!
//! Saving merges runtime UI settings (theme/lang/scroll-mode) into the launch
//! config, preserving transport/unknown keys, and the result round-trips back
//! through the loader. per-session `thinking` is intentionally excluded.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use octos_core::{Message, SessionKey};
use octos_tui::cli::{Lang, ScrollMode, ThemeName, load_config_file, save_ui_settings};
use octos_tui::event_loop::handle_terminal_event;
use octos_tui::model::{AppState, SessionView};
use octos_tui::store::Store;
use std::path::PathBuf;

fn chat_store() -> Store {
    Store {
        state: AppState::new(
            vec![SessionView {
                id: SessionKey("local:saveconfig-test".into()),
                title: "saveconfig-test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hi")],
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

fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("octos-saveconfig-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn run_saveconfig(store: &mut Store) {
    store.state.set_composer_text("/saveconfig");
    handle_terminal_event(
        store,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
}

#[test]
fn saveconfig_writes_runtime_settings() {
    let path = unique_dir("write").join("config.json");
    std::fs::write(&path, r#"{ "theme": "codex", "scroll-mode": "native" }"#).unwrap();
    let mut store = chat_store();
    store.state.config_path = Some(path.clone());
    store.state.theme = ThemeName::Claude;
    store.state.pinned_scroll = true;

    run_saveconfig(&mut store);

    let config = load_config_file(&path).expect("reparse");
    assert_eq!(config.theme, Some(ThemeName::Claude));
    assert_eq!(config.scroll_mode, Some(ScrollMode::Pinned));
}

#[test]
fn saveconfig_preserves_transport_keys() {
    let path = unique_dir("preserve").join("config.json");
    std::fs::write(
        &path,
        r#"{ "stdio-command": "octos serve --stdio", "profile-id": "alex", "theme": "codex" }"#,
    )
    .unwrap();
    let mut store = chat_store();
    store.state.config_path = Some(path.clone());
    store.state.theme = ThemeName::Slate;

    run_saveconfig(&mut store);

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["stdio-command"], "octos serve --stdio");
    assert_eq!(raw["profile-id"], "alex");
    assert_eq!(raw["theme"], "slate");
    // The loader still accepts the merged file (transport keys intact).
    let config = load_config_file(&path).expect("reparse");
    assert_eq!(config.stdio_command.as_deref(), Some("octos serve --stdio"));
    assert_eq!(config.profile_id.as_deref(), Some("alex"));
}

#[test]
fn saveconfig_does_not_clobber_an_unreadable_config() {
    // An existing-but-unreadable config (here: invalid UTF-8) must surface a
    // read error rather than being treated as empty and overwritten with only
    // the UI keys — that would silently drop the transport keys it holds.
    let path = unique_dir("unreadable").join("config.json");
    let original: &[u8] = &[0x66, 0x6f, 0x6f, 0xff, 0xfe]; // "foo" + invalid UTF-8
    std::fs::write(&path, original).unwrap();

    let result = save_ui_settings(
        &path,
        ThemeName::Claude,
        Lang::En,
        ScrollMode::Pinned,
        false,
    );

    assert!(
        result.is_err(),
        "an unreadable existing config must not be silently overwritten"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "the existing config file must be left intact when it can't be read"
    );
}

#[test]
fn saved_config_roundtrips_through_loader() {
    let path = unique_dir("roundtrip").join("config.json");
    std::fs::write(&path, "{}").unwrap();
    let mut store = chat_store();
    store.state.config_path = Some(path.clone());
    store.state.theme = ThemeName::Solarized;
    store.state.pinned_scroll = false;

    run_saveconfig(&mut store);

    let config = load_config_file(&path).expect("reparse");
    assert_eq!(config.theme, Some(ThemeName::Solarized));
    assert_eq!(config.scroll_mode, Some(ScrollMode::Native));
    assert!(config.lang.is_some(), "lang persisted too");
}

#[test]
fn saveconfig_excludes_thinking() {
    let path = unique_dir("nothinking").join("config.json");
    std::fs::write(&path, "{}").unwrap();
    let mut store = chat_store();
    store.state.config_path = Some(path.clone());

    run_saveconfig(&mut store);

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let obj = raw.as_object().unwrap();
    assert!(!obj.contains_key("thinking"));
    assert!(
        obj.contains_key("theme") && obj.contains_key("scroll-mode") && obj.contains_key("lang")
    );
}

#[test]
fn default_config_path_resolves_under_config_dir() {
    // The fallback used when launched without --config: a pure, non-destructive
    // resolution (the dispatch path feeds this into the same merge writer).
    let path = octos_tui::cli::default_config_path().expect("HOME is set in test env");
    let suffix: std::path::PathBuf = [".config", "octos-tui", "config.json"].iter().collect();
    assert!(
        path.ends_with(&suffix),
        "default path {path:?} must end with {suffix:?}"
    );
}

#[test]
fn partial_prefix_completes_into_composer_then_executes() {
    // The user's report: typing a partial name and pressing Enter must COMPLETE
    // the full command into the composer (consistent with argful commands), not
    // execute immediately. A no-arg command like /saveconfig is the case that
    // used to diverge.
    let path = unique_dir("complete").join("config.json");
    std::fs::write(&path, "{}").unwrap();
    let mut store = chat_store();
    store.state.config_path = Some(path.clone());

    // Type a unique partial prefix; the popup filters to /saveconfig.
    for ch in "/savec".chars() {
        handle_terminal_event(
            &mut store,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    // First Enter: completes into the composer, does NOT run yet.
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(
        store.state.composer, "/saveconfig",
        "partial prefix + Enter completes the full command into the composer"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        "{}",
        "completion must not execute the command yet"
    );

    // Second Enter: now it runs.
    handle_terminal_event(
        &mut store,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(raw.get("theme").is_some(), "second Enter executes the save");
}
