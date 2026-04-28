use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use eyre::Result;
use octos_core::app_ui::{AppUiCommand, AppUiEvent};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    app,
    cli::Cli,
    model::{ApprovalModalAction, FocusPane},
    store::Store,
    theme::Palette,
    transport::{AppUiBackend, build_backend},
};

pub fn run(cli: Cli) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let guard = TerminalGuard;

    let palette = Palette::for_theme(cli.theme);
    let mut backend = build_backend(&cli);
    let snapshot = backend.bootstrap()?;
    let mut store = Store::from_snapshot(snapshot);

    loop {
        terminal.draw(|frame| app::render(frame, &store.state, palette))?;

        if event::poll(Duration::from_millis(90))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match handle_key(&mut store, key) {
                KeyAction::Continue => {}
                KeyAction::Quit => break,
                KeyAction::Send(command) => send_command(backend.as_mut(), &mut store, command),
            }
        } else if let Some(event) = backend.next_event()? {
            if let Some(command) = store.apply_client_event(event) {
                send_command(backend.as_mut(), &mut store, command);
            }
        }
    }

    drop(guard);
    Ok(())
}

fn send_command(backend: &mut dyn AppUiBackend, store: &mut Store, command: AppUiCommand) {
    if let Err(err) = backend.send(command) {
        store.apply_event(AppUiEvent::error("send_failed", format!("{err:#}")));
    }
}

pub(crate) enum KeyAction {
    Continue,
    Quit,
    Send(AppUiCommand),
}

pub(crate) fn handle_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    if key.kind != KeyEventKind::Press {
        return KeyAction::Continue;
    }

    if is_control_char(&key, 'q') {
        return KeyAction::Quit;
    }

    if is_control_char(&key, 'c') {
        return store
            .interrupt_command()
            .map_or(KeyAction::Continue, KeyAction::Send);
    }

    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        return handle_plain_key(store, key);
    }

    if is_alt_char(&key, 'j') {
        move_down(&mut store.state);
        return KeyAction::Continue;
    }

    if is_alt_char(&key, 'k') {
        move_up(&mut store.state);
        return KeyAction::Continue;
    }

    KeyAction::Continue
}

fn handle_plain_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    if store
        .state
        .approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
    {
        return handle_approval_modal_key(store, key);
    }

    if store.state.task_output.active {
        return handle_task_output_key(store, key);
    }

    match key.code {
        KeyCode::Tab => {
            store.state.focus = store.state.focus.next();
        }
        KeyCode::Esc => {
            if store.state.active_turn().is_some() && store.state.has_pending_messages() {
                if let Some(command) = store.interrupt_staged_command() {
                    return KeyAction::Send(command);
                }
            }
            store.state.focus = FocusPane::Composer;
        }
        KeyCode::Char('q') if store.state.focus != FocusPane::Composer => {
            return KeyAction::Quit;
        }
        KeyCode::Char('j') if store.state.focus != FocusPane::Composer => {
            move_down(&mut store.state);
        }
        KeyCode::Char('k') if store.state.focus != FocusPane::Composer => {
            move_up(&mut store.state);
        }
        KeyCode::Down => {
            move_down(&mut store.state);
        }
        KeyCode::Up => {
            move_up(&mut store.state);
        }
        KeyCode::PageDown => match store.state.focus {
            FocusPane::Workspace => store.state.workspace.scroll_down(8),
            FocusPane::Git => store.state.git.scroll_down(8),
            _ => store.state.scroll_transcript_down(8),
        },
        KeyCode::PageUp => match store.state.focus {
            FocusPane::Workspace => store.state.workspace.scroll_up(8),
            FocusPane::Git => store.state.git.scroll_up(8),
            _ => store.state.scroll_transcript_up(8),
        },
        KeyCode::End => match store.state.focus {
            FocusPane::Workspace => store.state.workspace.scroll = 0,
            FocusPane::Git => store.state.git.scroll = 0,
            _ => store.state.scroll_transcript_to_latest(),
        },
        KeyCode::Backspace if store.state.focus == FocusPane::Composer => {
            store.state.composer.pop();
        }
        KeyCode::Enter if store.state.focus == FocusPane::Composer => {
            if let Some(command) = store.compose_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char('o') if store.state.focus == FocusPane::Tasks => {
            if let Some(command) = store.read_task_output_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char('d') if store.state.focus != FocusPane::Composer => {
            if let Some(command) = store.read_diff_preview_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char(ch) => {
            store.state.composer.push(ch);
            store.state.focus = FocusPane::Composer;
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_approval_modal_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            store.close_modal();
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'y') => {
            if let Some(command) =
                store.respond_approval_command(ApprovalModalAction::ApproveRequest)
            {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'s') => {
            if let Some(command) =
                store.respond_approval_command(ApprovalModalAction::ApproveSession)
            {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'n') => {
            if let Some(command) = store.respond_approval_command(ApprovalModalAction::DenyRequest)
            {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'d') => {
            if let Some(command) = store.read_diff_preview_command() {
                return KeyAction::Send(command);
            }
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_task_output_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            store.close_modal();
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'o') => {
            if let Some(command) = store.read_task_output_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            store.state.task_output.scroll_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            store.state.task_output.scroll_up(1);
        }
        KeyCode::PageDown => {
            store.state.task_output.scroll_down(8);
        }
        KeyCode::PageUp => {
            store.state.task_output.scroll_up(8);
        }
        KeyCode::End => {
            store.state.task_output.scroll = 0;
        }
        _ => {}
    }

    KeyAction::Continue
}

fn move_down(state: &mut crate::model::AppState) {
    match state.focus {
        FocusPane::Sessions => state.select_next_session(),
        FocusPane::Tasks => state.select_next_task(),
        FocusPane::Artifacts => state.select_next_artifact(),
        FocusPane::Workspace => state.select_next_workspace_entry(),
        FocusPane::Git => state.select_next_git_entry(),
        FocusPane::Transcript | FocusPane::Composer => state.scroll_transcript_down(1),
    }
}

fn move_up(state: &mut crate::model::AppState) {
    match state.focus {
        FocusPane::Sessions => state.select_prev_session(),
        FocusPane::Tasks => state.select_prev_task(),
        FocusPane::Artifacts => state.select_prev_artifact(),
        FocusPane::Workspace => state.select_prev_workspace_entry(),
        FocusPane::Git => state.select_prev_git_entry(),
        FocusPane::Transcript | FocusPane::Composer => state.scroll_transcript_up(1),
    }
}

fn is_control_char(key: &KeyEvent, expected: char) -> bool {
    matches!(
        key.code,
        KeyCode::Char(ch)
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && ch.eq_ignore_ascii_case(&expected)
    )
}

fn is_alt_char(key: &KeyEvent, expected: char) -> bool {
    matches!(
        key.code,
        KeyCode::Char(ch)
            if key.modifiers.contains(KeyModifiers::ALT)
                && ch.eq_ignore_ascii_case(&expected)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppState, LiveReply, SessionView, TaskView};
    use octos_core::{
        SessionKey, TaskId,
        ui_protocol::{
            ApprovalDecision, ApprovalId, ApprovalRequestedEvent, PreviewId, TaskRuntimeState,
            TurnId, UiNotification, approval_scopes,
        },
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn store_with_sessions(count: usize) -> Store {
        let sessions = (0..count)
            .map(|idx| SessionView {
                id: SessionKey(format!("local:test-{idx}")),
                title: format!("test {idx}"),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            })
            .collect();

        Store {
            state: AppState::new(sessions, 0, "ready".into(), None, false),
        }
    }

    fn store_with_visible_approval() -> (Store, ApprovalId) {
        let mut store = store_with_sessions(1);
        let approval_id = ApprovalId::new();

        store.apply_event(AppUiEvent::Protocol(UiNotification::ApprovalRequested(
            ApprovalRequestedEvent::generic(
                store.state.sessions[0].id.clone(),
                approval_id.clone(),
                TurnId::new(),
                "shell",
                "Run command",
                "cargo test",
            ),
        )));

        (store, approval_id)
    }

    #[test]
    fn composer_accepts_reserved_text_keys() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('q'))),
            KeyAction::Continue
        ));
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('k'))),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, "qjk");
    }

    #[test]
    fn reserved_text_keys_remain_navigation_outside_composer() {
        let mut store = store_with_sessions(2);
        store.state.focus = FocusPane::Sessions;

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.selected_session, 1);

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('k'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.selected_session, 0);

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('q'))),
            KeyAction::Quit
        ));
    }

    #[test]
    fn modified_jk_scroll_transcript_while_composing() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('k'), KeyModifiers::ALT)
            ),
            KeyAction::Continue
        ));
        assert_eq!(store.state.transcript_scroll, 1);

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('j'), KeyModifiers::ALT)
            ),
            KeyAction::Continue
        ));
        assert_eq!(store.state.transcript_scroll, 0);
        assert!(store.state.composer.is_empty());
    }

    #[test]
    fn tab_and_jk_navigation_cover_m9_panes_without_stealing_composer_text() {
        let mut store = store_with_sessions(1);
        store
            .state
            .artifacts
            .items
            .push(crate::model::ArtifactItem {
                title: "extra artifact".into(),
                kind: "test".into(),
                source: "test".into(),
                status: "ready".into(),
            });

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Tab)),
            KeyAction::Continue
        ));
        assert_eq!(store.state.focus, FocusPane::Sessions);
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Tab)),
            KeyAction::Continue
        ));
        assert_eq!(store.state.focus, FocusPane::Tasks);
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Tab)),
            KeyAction::Continue
        ));
        assert_eq!(store.state.focus, FocusPane::Artifacts);
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.artifacts.selected, 1);

        for expected in [
            FocusPane::Transcript,
            FocusPane::Workspace,
            FocusPane::Git,
            FocusPane::Composer,
        ] {
            assert!(matches!(
                handle_key(&mut store, key(KeyCode::Tab)),
                KeyAction::Continue
            ));
            assert_eq!(store.state.focus, expected);
        }

        store.state.focus = FocusPane::Workspace;
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.workspace.selected, 1);

        store.state.focus = FocusPane::Git;
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.git.selected, 1);

        store.state.focus = FocusPane::Composer;
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('g'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.composer, "g");
    }

    #[test]
    fn ctrl_c_emits_interrupt_for_active_turn() {
        let turn_id = TurnId::new();
        let mut store = store_with_sessions(1);
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: turn_id.clone(),
            text: "streaming".into(),
        });

        let action = handle_key(
            &mut store,
            modified_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        let KeyAction::Send(AppUiCommand::InterruptTurn(params)) = action else {
            panic!("expected interrupt command");
        };
        assert_eq!(params.turn_id, turn_id);
        assert_eq!(store.state.status, "Interrupt requested for active turn");
    }

    #[test]
    fn esc_interrupts_when_staged_messages_are_waiting() {
        let turn_id = TurnId::new();
        let mut store = store_with_sessions(1);
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: turn_id.clone(),
            text: "streaming".into(),
        });
        store.state.pending_messages.push("send this next".into());

        let action = handle_key(&mut store, key(KeyCode::Esc));

        let KeyAction::Send(AppUiCommand::InterruptTurn(params)) = action else {
            panic!("expected interrupt command");
        };
        assert_eq!(params.turn_id, turn_id);
        assert_eq!(
            store.state.status,
            "Interrupt requested; staged message will submit when the turn stops"
        );
    }

    #[test]
    fn d_requests_diff_preview_when_selected_task_exposes_preview_id() {
        let preview_id = PreviewId::new();
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Tasks;
        store.state.sessions[0].tasks.push(TaskView {
            id: TaskId::new(),
            title: "diff".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: Some(format!("preview_id={}", preview_id.0)),
            output_tail: String::new(),
        });

        let action = handle_key(&mut store, key(KeyCode::Char('d')));

        let KeyAction::Send(AppUiCommand::GetDiffPreview(params)) = action else {
            panic!("expected diff preview command");
        };
        assert_eq!(params.preview_id, preview_id);
        assert!(store.state.diff_preview.active);
        assert_eq!(store.state.status, "Requested diff preview");
    }

    #[test]
    fn inline_diff_preview_does_not_steal_pending_approval_keys() {
        let preview_id = PreviewId::new();
        let (mut store, approval_id) = store_with_visible_approval();
        store.state.diff_preview.open_loading(preview_id);

        let action = handle_key(&mut store, key(KeyCode::Char('y')));

        let KeyAction::Send(AppUiCommand::RespondApproval(params)) = action else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert!(store.state.diff_preview.active);
    }

    #[test]
    fn approval_modal_keys_emit_request_session_and_denial_scopes() {
        let (mut store, approval_id) = store_with_visible_approval();

        let action = handle_key(&mut store, key(KeyCode::Char('y')));

        let KeyAction::Send(AppUiCommand::RespondApproval(params)) = action else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Approve);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::REQUEST)
        );

        let (mut store, approval_id) = store_with_visible_approval();
        let action = handle_key(&mut store, key(KeyCode::Char('s')));

        let KeyAction::Send(AppUiCommand::RespondApproval(params)) = action else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Approve);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::SESSION)
        );

        let (mut store, approval_id) = store_with_visible_approval();
        let action = handle_key(&mut store, key(KeyCode::Char('n')));

        let KeyAction::Send(AppUiCommand::RespondApproval(params)) = action else {
            panic!("expected approval response command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Deny);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::REQUEST)
        );
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout: Stdout = io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}
