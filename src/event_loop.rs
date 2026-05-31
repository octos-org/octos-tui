use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use eyre::Result;
use octos_core::app_ui::AppUiEvent;
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    app,
    cli::Cli,
    client_event::ClientEvent,
    model::{AppUiCommand, ApprovalModalAction, FocusPane},
    store::Store,
    theme::Palette,
    transport::{AppUiBackend, build_backend},
};

const UI_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_BACKEND_EVENTS_PER_TICK: usize = 512;

pub fn run(cli: Cli) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let guard = TerminalGuard;

    let palette = Palette::for_theme(cli.theme);
    let mut backend = build_backend(&cli);
    let snapshot = backend.bootstrap()?;
    let mut store = Store::from_snapshot(snapshot);
    let mut input_state = TerminalInputState::default();

    loop {
        drain_backend_events(backend.as_mut(), &mut store)?;

        terminal.draw(|frame| app::render(frame, &store.state, palette))?;

        if event::poll(UI_EVENT_POLL_INTERVAL)? {
            let raw_event = event::read()?;
            let next_event_waiting = event::poll(Duration::from_millis(0))?;
            match handle_terminal_event_with_input_state(
                &mut store,
                raw_event,
                &mut input_state,
                next_event_waiting,
                Instant::now(),
            ) {
                KeyAction::Continue => {}
                KeyAction::Quit => break,
                KeyAction::Send(command) => send_command(backend.as_mut(), &mut store, command),
            }
        }

        drain_backend_events(backend.as_mut(), &mut store)?;
    }

    drop(guard);
    Ok(())
}

fn drain_backend_events(backend: &mut dyn AppUiBackend, store: &mut Store) -> Result<()> {
    for _ in 0..MAX_BACKEND_EVENTS_PER_TICK {
        let Some(event) = backend.next_event()? else {
            drain_pending_autonomy_hydration(backend, store);
            return Ok(());
        };
        apply_client_event_and_send_followup(backend, store, event);
    }

    drain_pending_autonomy_hydration(backend, store);
    Ok(())
}

fn apply_client_event_and_send_followup(
    backend: &mut dyn AppUiBackend,
    store: &mut Store,
    event: ClientEvent,
) {
    if let Some(command) = store.apply_client_event(event) {
        send_command(backend, store, command);
    }
}

/// M15-E: send any queued autonomy hydration commands the store has
/// staged (e.g. on `session/opened` after reconnect). Bounded by the
/// queue cap inside `AppState` itself.
fn drain_pending_autonomy_hydration(backend: &mut dyn AppUiBackend, store: &mut Store) {
    while let Some(command) = store.state.dequeue_autonomy_hydration() {
        send_command(backend, store, command);
    }
}

fn send_command(backend: &mut dyn AppUiBackend, store: &mut Store, command: AppUiCommand) {
    if let Err(err) = backend.send(command) {
        store.apply_event(AppUiEvent::error("send_failed", format!("{err:#}")));
    }
}

#[derive(Default)]
struct TerminalInputState {
    unbracketed_paste_until: Option<Instant>,
}

impl TerminalInputState {
    fn should_insert_unbracketed_paste_newline(
        &mut self,
        now: Instant,
        next_event_waiting: bool,
    ) -> bool {
        if next_event_waiting || self.unbracketed_paste_active(now) {
            self.extend_unbracketed_paste(now);
            true
        } else {
            false
        }
    }

    fn note_text_key(&mut self, now: Instant) {
        if self.unbracketed_paste_active(now) {
            self.extend_unbracketed_paste(now);
        }
    }

    fn unbracketed_paste_active(&self, now: Instant) -> bool {
        self.unbracketed_paste_until
            .is_some_and(|deadline| now <= deadline)
    }

    fn extend_unbracketed_paste(&mut self, now: Instant) {
        self.unbracketed_paste_until = Some(now + Duration::from_millis(80));
    }
}

pub(crate) enum KeyAction {
    Continue,
    Quit,
    Send(AppUiCommand),
}

fn handle_terminal_event_with_input_state(
    store: &mut Store,
    event: Event,
    input_state: &mut TerminalInputState,
    next_event_waiting: bool,
    now: Instant,
) -> KeyAction {
    if let Event::Key(key) = event {
        if is_plain_composer_enter(store, &key)
            && input_state.should_insert_unbracketed_paste_newline(now, next_event_waiting)
        {
            store.state.insert_composer_text("\n");
            store.state.focus = FocusPane::Composer;
            return KeyAction::Continue;
        }

        let text_key = is_plain_text_key(&key);
        let action = handle_key(store, key);
        if text_key && store.state.focus == FocusPane::Composer {
            input_state.note_text_key(now);
        }
        return action;
    }

    handle_terminal_event(store, event)
}

pub(crate) fn handle_terminal_event(store: &mut Store, event: Event) -> KeyAction {
    match event {
        Event::Key(key) => handle_key(store, key),
        Event::Paste(text) => handle_paste(store, &text),
        Event::Mouse(mouse) => handle_mouse(store, mouse),
        Event::FocusGained | Event::FocusLost | Event::Resize(_, _) => KeyAction::Continue,
    }
}

fn handle_mouse(store: &mut Store, mouse: MouseEvent) -> KeyAction {
    const MOUSE_SCROLL_LINES: usize = 4;
    match mouse.kind {
        MouseEventKind::ScrollUp => scroll_current_surface_up(store, MOUSE_SCROLL_LINES),
        MouseEventKind::ScrollDown => scroll_current_surface_down(store, MOUSE_SCROLL_LINES),
        _ => {}
    }
    KeyAction::Continue
}

fn is_plain_composer_enter(store: &Store, key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && key.code == KeyCode::Enter
        && key.modifiers.is_empty()
        && store.state.focus == FocusPane::Composer
}

fn is_plain_text_key(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && matches!(key.code, KeyCode::Char(_))
        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
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

    if is_control_char(&key, 'u') {
        store.clear_composer_or_staged_messages();
        return KeyAction::Continue;
    }

    if is_control_char(&key, 'o') {
        store.state.toggle_tool_output_expansion();
        return KeyAction::Continue;
    }

    if store.state.focus == FocusPane::Composer && handle_composer_modified_key(store, key) {
        return KeyAction::Continue;
    }

    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        return handle_plain_key(store, key);
    }

    if is_alt_char(&key, 'a') {
        store.show_pending_approval();
        return KeyAction::Continue;
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

fn handle_paste(store: &mut Store, text: &str) -> KeyAction {
    if text.is_empty() {
        return KeyAction::Continue;
    }

    let opens_slash_popup = store.state.composer.is_empty() && text.starts_with('/');
    store.state.insert_composer_text(text);
    store.state.focus = FocusPane::Composer;

    if opens_slash_popup {
        store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));
    }
    if store.state.menu_stack.is_active() && slash_help_menu_active(store) {
        sync_slash_help_search_query(store);
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

    if store.state.artifact_detail.active {
        return handle_artifact_detail_key(store, key);
    }

    if store.state.thread_graph_detail.active {
        return handle_thread_graph_detail_key(store, key);
    }

    if store.state.turn_state_detail.active {
        return handle_turn_state_detail_key(store, key);
    }

    if store.state.menu_stack.is_active() {
        return handle_menu_key(store, key);
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
            FocusPane::Composer => store.state.move_composer_cursor_line_end(),
            _ => store.state.scroll_transcript_to_latest(),
        },
        KeyCode::Home if store.state.focus == FocusPane::Composer => {
            store.state.move_composer_cursor_line_start();
        }
        KeyCode::Left if store.state.focus == FocusPane::Composer => {
            store.state.move_composer_cursor_left();
        }
        KeyCode::Right if store.state.focus == FocusPane::Composer => {
            store.state.move_composer_cursor_right();
        }
        KeyCode::Delete if store.state.focus == FocusPane::Composer => {
            store.state.delete_composer_next_char();
        }
        KeyCode::Backspace if store.state.focus == FocusPane::Composer => {
            store.state.delete_composer_prev_char();
        }
        KeyCode::Enter if store.state.focus == FocusPane::Composer => {
            return handle_composer_enter(store);
        }
        KeyCode::Char('o') if store.state.focus == FocusPane::Tasks => {
            if let Some(command) = store.read_task_output_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char('x') if store.state.focus == FocusPane::Tasks => {
            if let Some(command) = store.cancel_task_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char('d') if store.state.focus != FocusPane::Composer => {
            if let Some(command) = store.read_diff_preview_command() {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Char(']') if store.state.focus != FocusPane::Composer => {
            store.select_next_diff_hunk();
        }
        KeyCode::Char('[') if store.state.focus != FocusPane::Composer => {
            store.select_prev_diff_hunk();
        }
        KeyCode::Char('c')
            if store.state.focus != FocusPane::Composer && store.state.diff_preview.active =>
        {
            store.stage_selected_diff_context();
        }
        KeyCode::Char(ch) => {
            let opens_slash_popup = ch == '/' && store.state.composer.is_empty();
            store.state.insert_composer_char(ch);
            store.state.focus = FocusPane::Composer;
            if opens_slash_popup {
                store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));
            }
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_composer_modified_key(store: &mut Store, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Char('b') | KeyCode::Left => {
                store.state.move_composer_cursor_prev_word();
                return true;
            }
            KeyCode::Char('f') | KeyCode::Right => {
                store.state.move_composer_cursor_next_word();
                return true;
            }
            KeyCode::Char('d') => {
                store.state.delete_composer_next_word();
                return true;
            }
            KeyCode::Backspace => {
                store.state.delete_composer_prev_word();
                return true;
            }
            _ => {}
        }
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('a') | KeyCode::Home => {
                store.state.move_composer_cursor_line_start();
                return true;
            }
            KeyCode::Char('e') | KeyCode::End => {
                store.state.move_composer_cursor_line_end();
                return true;
            }
            KeyCode::Char('b') | KeyCode::Left => {
                store.state.move_composer_cursor_left();
                return true;
            }
            KeyCode::Char('f') | KeyCode::Right => {
                store.state.move_composer_cursor_right();
                return true;
            }
            KeyCode::Char('w') => {
                store.state.delete_composer_prev_word();
                return true;
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                store.state.delete_composer_next_char();
                return true;
            }
            KeyCode::Char('h') | KeyCode::Backspace => {
                store.state.delete_composer_prev_char();
                return true;
            }
            KeyCode::Char('k') => {
                store.state.kill_composer_to_line_end();
                return true;
            }
            _ => {}
        }
    }

    false
}

fn handle_composer_enter(store: &mut Store) -> KeyAction {
    let command = store.compose_command();
    if store.state.exit_requested {
        KeyAction::Quit
    } else {
        command.map_or(KeyAction::Continue, KeyAction::Send)
    }
}

fn handle_menu_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    if menu_composer_edit_active(store) {
        if handle_composer_modified_key(store, key) {
            return KeyAction::Continue;
        }
        match key.code {
            KeyCode::Esc => {
                store.state.set_composer_text("");
            }
            KeyCode::Backspace => {
                store.state.delete_composer_prev_char();
            }
            KeyCode::Delete => {
                store.state.delete_composer_next_char();
            }
            KeyCode::Home => {
                store.state.move_composer_cursor_line_start();
            }
            KeyCode::End => {
                store.state.move_composer_cursor_line_end();
            }
            KeyCode::Left => {
                store.state.move_composer_cursor_left();
            }
            KeyCode::Right => {
                store.state.move_composer_cursor_right();
            }
            KeyCode::Enter => {
                return handle_composer_enter(store);
            }
            KeyCode::Char(ch) => {
                store.state.insert_composer_char(ch);
            }
            _ => {}
        }
        return KeyAction::Continue;
    }

    match key.code {
        KeyCode::Esc => {
            store.close_menu();
        }
        KeyCode::Backspace if slash_help_query_active(store) => {
            store.state.delete_composer_prev_char();
            sync_slash_help_search_query(store);
        }
        KeyCode::Backspace if active_menu_search_has_query(store) => {
            delete_active_menu_search_prev_char(store);
        }
        KeyCode::Char(ch) if slash_help_should_capture_char(store, ch) => {
            store.state.insert_composer_char(ch);
            sync_slash_help_search_query(store);
        }
        KeyCode::Char(ch) if active_menu_should_capture_search_char(store, ch) => {
            append_active_menu_search_char(store, ch);
        }
        KeyCode::Enter => {
            if slash_help_query_active(store) {
                return handle_composer_enter(store);
            }
            let command = store.accept_active_menu_item();
            if store.state.exit_requested {
                return KeyAction::Quit;
            }
            if let Some(command) = command {
                return KeyAction::Send(command);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            store.select_next_menu_item();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            store.select_prev_menu_item();
        }
        _ => {}
    }

    KeyAction::Continue
}

fn menu_composer_edit_active(store: &Store) -> bool {
    store.state.focus == FocusPane::Composer
        && !store.state.composer.is_empty()
        && !slash_help_query_active(store)
}

fn slash_help_query_active(store: &Store) -> bool {
    slash_help_menu_active(store)
        && store.state.composer.starts_with('/')
        && store.state.composer.len() > 1
}

fn slash_help_should_capture_char(store: &Store, ch: char) -> bool {
    slash_help_menu_active(store)
        && store.state.composer.starts_with('/')
        && (store.state.composer.len() > 1 || !matches!(ch, 'j' | 'k'))
}

fn slash_help_menu_active(store: &Store) -> bool {
    store
        .state
        .menu_stack
        .active()
        .is_some_and(|frame| frame.id.as_str() == crate::menu::registry::MENU_HELP)
}

fn sync_slash_help_search_query(store: &mut Store) {
    if let Some(frame) = store.state.menu_stack.active_mut() {
        frame.search_query = store
            .state
            .composer
            .strip_prefix('/')
            .unwrap_or(store.state.composer.as_str())
            .to_string();
        frame.selected_index = 0;
    }
    store.refresh_active_menu();
}

fn active_menu_should_capture_search_char(store: &Store, ch: char) -> bool {
    active_menu_searchable(store)
        && (active_menu_search_has_query(store) || !matches!(ch, 'j' | 'k'))
}

fn active_menu_searchable(store: &Store) -> bool {
    matches!(
        store.state.active_menu.as_ref(),
        Some(crate::menu::MenuBuildResult::Ready(spec)) if spec.searchable
    )
}

fn active_menu_search_has_query(store: &Store) -> bool {
    store
        .state
        .menu_stack
        .active()
        .is_some_and(|frame| !frame.search_query.is_empty())
}

fn append_active_menu_search_char(store: &mut Store, ch: char) {
    if let Some(frame) = store.state.menu_stack.active_mut() {
        frame.search_query.push(ch);
        frame.selected_index = 0;
    }
    store.refresh_active_menu();
}

fn delete_active_menu_search_prev_char(store: &mut Store) {
    if let Some(frame) = store.state.menu_stack.active_mut() {
        frame.search_query.pop();
        frame.selected_index = 0;
    }
    store.refresh_active_menu();
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

fn handle_artifact_detail_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            store.close_modal();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            store.state.artifact_detail.scroll_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            store.state.artifact_detail.scroll_up(1);
        }
        KeyCode::PageDown => {
            store.state.artifact_detail.scroll_down(8);
        }
        KeyCode::PageUp => {
            store.state.artifact_detail.scroll_up(8);
        }
        KeyCode::End => {
            store.state.artifact_detail.scroll = 0;
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_thread_graph_detail_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            store.close_modal();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            store.state.thread_graph_detail.scroll_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            store.state.thread_graph_detail.scroll_up(1);
        }
        KeyCode::PageDown => {
            store.state.thread_graph_detail.scroll_down(8);
        }
        KeyCode::PageUp => {
            store.state.thread_graph_detail.scroll_up(8);
        }
        KeyCode::End => {
            store.state.thread_graph_detail.scroll = 0;
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_turn_state_detail_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            store.close_modal();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            store.state.turn_state_detail.scroll_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            store.state.turn_state_detail.scroll_up(1);
        }
        KeyCode::PageDown => {
            store.state.turn_state_detail.scroll_down(8);
        }
        KeyCode::PageUp => {
            store.state.turn_state_detail.scroll_up(8);
        }
        KeyCode::End => {
            store.state.turn_state_detail.scroll = 0;
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

fn scroll_current_surface_down(store: &mut Store, lines: usize) {
    if store.state.task_output.active {
        store.state.task_output.scroll_down(lines);
        return;
    }
    if store.state.artifact_detail.active {
        store.state.artifact_detail.scroll_down(lines);
        return;
    }
    if store.state.thread_graph_detail.active {
        store.state.thread_graph_detail.scroll_down(lines);
        return;
    }
    if store.state.turn_state_detail.active {
        store.state.turn_state_detail.scroll_down(lines);
        return;
    }

    match store.state.focus {
        FocusPane::Workspace => store.state.workspace.scroll_down(lines),
        FocusPane::Git => store.state.git.scroll_down(lines),
        _ => store.state.scroll_transcript_down(lines),
    }
}

fn scroll_current_surface_up(store: &mut Store, lines: usize) {
    if store.state.task_output.active {
        store.state.task_output.scroll_up(lines);
        return;
    }
    if store.state.artifact_detail.active {
        store.state.artifact_detail.scroll_up(lines);
        return;
    }
    if store.state.thread_graph_detail.active {
        store.state.thread_graph_detail.scroll_up(lines);
        return;
    }
    if store.state.turn_state_detail.active {
        store.state.turn_state_detail.scroll_up(lines);
        return;
    }

    match store.state.focus {
        FocusPane::Workspace => store.state.workspace.scroll_up(lines),
        FocusPane::Git => store.state.git.scroll_up(lines),
        _ => store.state.scroll_transcript_up(lines),
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
    use crate::model::{ActivityKind, AppState, LiveReply, SessionView, TaskView};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use octos_core::{
        SessionKey, TaskId,
        ui_protocol::{
            ApprovalDecision, ApprovalId, ApprovalRequestedEvent, PreviewId, TaskRuntimeState,
            TurnId, UiNotification, UiProtocolCapabilities, approval_scopes,
        },
    };
    use std::collections::VecDeque;

    struct FakeBackend {
        events: VecDeque<ClientEvent>,
        sent: Vec<AppUiCommand>,
    }

    impl FakeBackend {
        fn new(events: Vec<ClientEvent>) -> Self {
            Self {
                events: events.into(),
                sent: Vec::new(),
            }
        }
    }

    impl AppUiBackend for FakeBackend {
        fn bootstrap(&mut self) -> Result<octos_core::app_ui::AppUiSnapshot> {
            unreachable!("drain tests do not bootstrap the backend")
        }

        fn send(&mut self, command: AppUiCommand) -> Result<()> {
            self.sent.push(command);
            Ok(())
        }

        fn next_event(&mut self) -> Result<Option<ClientEvent>> {
            Ok(self.events.pop_front())
        }
    }

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
    fn drain_backend_events_applies_bursts_in_one_tick() {
        let mut backend = FakeBackend::new(vec![
            AppUiEvent::status("one").into(),
            AppUiEvent::status("two").into(),
            AppUiEvent::status("three").into(),
        ]);
        let mut store = store_with_sessions(1);

        drain_backend_events(&mut backend, &mut store).expect("drain succeeds");

        assert!(backend.events.is_empty());
        assert_eq!(store.state.status, "three");
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
    fn composer_supports_readline_control_shortcuts() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.set_composer_text("alpha beta gamma");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert_eq!(store.state.composer, "alpha beta ");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('a'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('f'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('X'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.composer, "aXlpha beta ");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('e'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('k'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert_eq!(store.state.composer, "aXlpha beta ");
    }

    #[test]
    fn composer_supports_alt_word_shortcuts() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.set_composer_text("alpha beta gamma");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('b'), KeyModifiers::ALT)
            ),
            KeyAction::Continue
        ));
        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('d'), KeyModifiers::ALT)
            ),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, "alpha beta ");
    }

    #[test]
    fn onboarding_menu_prefilled_composer_accepts_text_and_enter() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "ready".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));
        store.state.composer = "/onboard name ".into();
        store.state.focus = FocusPane::Composer;

        for ch in ['A', 'd', 'a'] {
            assert!(matches!(
                handle_key(&mut store, key(KeyCode::Char(ch))),
                KeyAction::Continue
            ));
        }
        assert_eq!(store.state.composer, "/onboard name Ada");
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Enter)),
            KeyAction::Continue
        ));

        assert_eq!(store.state.onboarding.name, "Ada");
        assert!(store.state.menu_stack.is_active());
        assert!(store.state.composer.is_empty());
    }

    #[test]
    fn terminal_paste_appends_literal_text_without_shortcut_dispatch() {
        let (mut store, _) = store_with_visible_approval();

        assert!(matches!(
            handle_terminal_event(&mut store, Event::Paste("y\n/safe".into())),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, "y\n/safe");
        assert_eq!(store.state.focus, FocusPane::Composer);
        assert!(
            store
                .state
                .approval
                .as_ref()
                .is_some_and(|modal| modal.visible)
        );
    }

    #[test]
    fn terminal_paste_opens_slash_menu_when_composer_starts_with_slash() {
        let mut store = store_with_sessions(1);

        assert!(matches!(
            handle_terminal_event(&mut store, Event::Paste("/permissions".into())),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, "/permissions");
        assert!(store.state.menu_stack.is_active());
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .expect("slash menu")
                .search_query,
            "permissions"
        );
    }

    #[test]
    fn unbracketed_paste_burst_keeps_enter_as_newline() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        let mut input_state = TerminalInputState::default();
        let now = Instant::now();

        for ch in ['a', 'l', 'p', 'h', 'a'] {
            assert!(matches!(
                handle_terminal_event_with_input_state(
                    &mut store,
                    Event::Key(key(KeyCode::Char(ch))),
                    &mut input_state,
                    true,
                    now,
                ),
                KeyAction::Continue
            ));
        }
        assert!(matches!(
            handle_terminal_event_with_input_state(
                &mut store,
                Event::Key(key(KeyCode::Enter)),
                &mut input_state,
                true,
                now,
            ),
            KeyAction::Continue
        ));
        for ch in ['b', 'e', 't', 'a'] {
            assert!(matches!(
                handle_terminal_event_with_input_state(
                    &mut store,
                    Event::Key(key(KeyCode::Char(ch))),
                    &mut input_state,
                    false,
                    now,
                ),
                KeyAction::Continue
            ));
        }

        assert_eq!(store.state.composer, "alpha\nbeta");
    }

    #[test]
    fn normal_enter_still_sends_when_not_in_paste_burst() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "send this".into();
        let mut input_state = TerminalInputState::default();

        assert!(matches!(
            handle_terminal_event_with_input_state(
                &mut store,
                Event::Key(key(KeyCode::Enter)),
                &mut input_state,
                false,
                Instant::now(),
            ),
            KeyAction::Send(_)
        ));
    }

    #[test]
    fn terminal_focus_resize_and_mouse_events_are_stable_noops() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "draft".into();

        for event in [
            Event::FocusGained,
            Event::FocusLost,
            Event::Resize(120, 40),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 5,
                modifiers: KeyModifiers::empty(),
            }),
        ] {
            assert!(matches!(
                handle_terminal_event(&mut store, event),
                KeyAction::Continue
            ));
        }

        assert_eq!(store.state.focus, FocusPane::Composer);
        assert_eq!(store.state.composer, "draft");
    }

    #[test]
    fn mouse_wheel_scrolls_current_surface() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        assert!(matches!(
            handle_terminal_event(
                &mut store,
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 10,
                    row: 5,
                    modifiers: KeyModifiers::empty(),
                }),
            ),
            KeyAction::Continue
        ));
        assert_eq!(store.state.transcript_scroll, 4);

        assert!(matches!(
            handle_terminal_event(
                &mut store,
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: 10,
                    row: 5,
                    modifiers: KeyModifiers::empty(),
                }),
            ),
            KeyAction::Continue
        ));
        assert_eq!(store.state.transcript_scroll, 0);

        store.state.focus = FocusPane::Workspace;
        handle_terminal_event(
            &mut store,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 10,
                row: 5,
                modifiers: KeyModifiers::empty(),
            }),
        );
        assert_eq!(store.state.workspace.scroll, 4);
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
    fn session_switch_preserves_per_session_composer_drafts() {
        let mut store = store_with_sessions(2);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "draft one".into();

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Tab)),
            KeyAction::Continue
        ));
        assert_eq!(store.state.focus, FocusPane::Sessions);
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.selected_session, 1);
        assert!(store.state.composer.is_empty());

        store.state.composer = "draft two".into();
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('k'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.selected_session, 0);
        assert_eq!(store.state.composer, "draft one");
    }

    #[test]
    fn ctrl_u_clears_staged_messages_before_composer_draft() {
        let mut store = store_with_sessions(1);
        store.state.composer = "draft".into();
        store.state.pending_messages.push("next prompt".into());

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(store.state.pending_messages.is_empty());
        assert_eq!(store.state.composer, "draft");
        assert_eq!(store.state.status, "Cleared 1 staged message(s)");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(store.state.composer.is_empty());
        assert_eq!(store.state.status, "Cleared composer draft");
    }

    #[test]
    fn ctrl_o_toggles_tool_output_expansion_without_leaving_composer() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(store.state.expanded_tool_outputs);
        assert_eq!(store.state.focus, FocusPane::Composer);
        assert_eq!(store.state.status, "Expanded tool output cards");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(!store.state.expanded_tool_outputs);
        assert_eq!(store.state.status, "Collapsed tool output cards");
    }

    #[test]
    fn typing_slash_opens_registry_backed_menu() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('/'))),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, "/");
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_HELP)
        );
        let expected = crate::menu::CommandRegistry::with_core_commands()
            .visible_commands(&store.state.availability_context())
            .into_iter()
            .map(|visible| visible.command.slash_name())
            .collect::<Vec<_>>();
        let Some(crate::menu::MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref()
        else {
            panic!("expected registry help menu");
        };
        let actual = spec
            .items
            .iter()
            .map(|item| item.label.clone())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn typing_in_searchable_menu_filters_items() {
        let mut store = store_with_sessions(1);
        store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));

        for ch in "theme".chars() {
            assert!(matches!(
                handle_key(&mut store, key(KeyCode::Char(ch))),
                KeyAction::Continue
            ));
        }

        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.search_query.as_str()),
            Some("theme")
        );
        let Some(crate::menu::MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref()
        else {
            panic!("expected registry help menu");
        };
        let labels = spec
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["/theme"]);
    }

    #[test]
    fn exact_slash_command_typed_through_popup_dispatches_command() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.target = Some("ws://127.0.0.1:50080/api/ui-protocol/ws".into());
        store.state.capabilities = Some(crate::menu::CapabilitySet::from_methods([
            octos_core::ui_protocol::methods::PERMISSION_PROFILE_LIST,
            octos_core::ui_protocol::methods::PERMISSION_PROFILE_SET,
        ]));

        for ch in "/permissions".chars() {
            assert!(matches!(
                handle_key(&mut store, key(KeyCode::Char(ch))),
                KeyAction::Continue
            ));
        }

        assert_eq!(store.state.composer, "/permissions");
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.search_query.as_str()),
            Some("permissions")
        );

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Enter)),
            KeyAction::Continue
        ));

        assert!(store.state.composer.is_empty());
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_PERMISSIONS)
        );
    }

    #[test]
    fn slash_ps_is_handled_locally_without_prompt_submission() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "/ps".into();
        store.state.sessions[0].tasks.push(TaskView {
            id: TaskId::new(),
            title: "cargo test".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: Some("running tests".into()),
            output_tail: "test output".into(),
        });
        store.state.sessions[0].tasks.push(TaskView {
            id: TaskId::new(),
            title: "format".into(),
            state: TaskRuntimeState::Completed,
            runtime_detail: None,
            output_tail: String::new(),
        });

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Enter)),
            KeyAction::Continue
        ));

        assert!(store.state.composer.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(store.state.focus, FocusPane::Tasks);
        assert!(store.state.status.contains("Local /ps: idle"));
        assert!(store.state.status.contains("2 total"));
        assert_eq!(
            store.state.activity.last().map(|item| item.title.as_str()),
            Some("local /ps")
        );
    }

    #[test]
    fn slash_stop_interrupts_active_turn_without_prompt_submission() {
        let turn_id = TurnId::new();
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "/stop".into();
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: turn_id.clone(),
            text: "streaming".into(),
        });

        let action = handle_key(&mut store, key(KeyCode::Enter));

        let KeyAction::Send(AppUiCommand::InterruptTurn(params)) = action else {
            panic!("expected interrupt command");
        };
        assert_eq!(params.turn_id, turn_id);
        assert!(store.state.composer.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(store.state.status, "Interrupt requested for active turn");
    }

    #[test]
    fn slash_stop_without_active_turn_reports_local_message() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "/stop".into();

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Enter)),
            KeyAction::Continue
        ));

        assert!(store.state.composer.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(store.state.status, "No active turn to interrupt");
        let activity = store.state.activity.last().expect("local activity");
        assert_eq!(activity.kind, ActivityKind::Warning);
        assert_eq!(activity.title, "local /stop");
    }

    #[test]
    fn unknown_slash_command_reports_help_without_prompt_submission() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "/wat".into();

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Enter)),
            KeyAction::Continue
        ));

        assert!(store.state.composer.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
        assert_eq!(
            store.state.status,
            "Unknown slash command: /wat. Try /ps, /stop, or /help."
        );
        let activity = store.state.activity.last().expect("local activity");
        assert_eq!(activity.kind, ActivityKind::Warning);
        assert_eq!(activity.title, "local slash command");
    }

    #[test]
    fn slash_exit_quits_without_prompt_submission() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "/exit".into();

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Enter)),
            KeyAction::Quit
        ));

        assert!(store.state.exit_requested);
        assert!(store.state.composer.is_empty());
        assert!(store.state.sessions[0].messages.is_empty());
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
    fn diff_hunk_keys_select_and_stage_context() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Transcript;
        store
            .state
            .diff_preview
            .apply_result(crate::model::DiffPreviewGetResult {
                status: "ready".into(),
                source: "cache".into(),
                preview: crate::model::DiffPreview {
                    session_id: store.state.sessions[0].id.clone(),
                    preview_id: PreviewId::new(),
                    title: Some("Patch".into()),
                    files: vec![crate::model::DiffPreviewFile {
                        path: "src/lib.rs".into(),
                        old_path: None,
                        status: "modified".into(),
                        hunks: vec![
                            crate::model::DiffPreviewHunk {
                                header: "@@ -1 +1 @@".into(),
                                lines: vec![crate::model::DiffPreviewLine {
                                    kind: "removed".into(),
                                    content: "old".into(),
                                    old_line: Some(1),
                                    new_line: None,
                                }],
                            },
                            crate::model::DiffPreviewHunk {
                                header: "@@ -9 +9 @@".into(),
                                lines: vec![crate::model::DiffPreviewLine {
                                    kind: "added".into(),
                                    content: "new".into(),
                                    old_line: None,
                                    new_line: Some(9),
                                }],
                            },
                        ],
                    }],
                },
            });

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char(']'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.diff_preview.selected_hunk, 1);

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('c'))),
            KeyAction::Continue
        ));
        assert!(store.state.composer.contains("file: src/lib.rs"));
        assert!(store.state.composer.contains("@@ -9 +9 @@"));
        assert_eq!(store.state.focus, FocusPane::Composer);
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
    fn hidden_approval_can_be_reopened_with_alt_a() {
        let (mut store, _) = store_with_visible_approval();
        store.close_modal();
        assert!(
            !store
                .state
                .approval
                .as_ref()
                .expect("approval pending")
                .visible
        );
        assert!(!store.state.approval_auto_open);

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('a'), KeyModifiers::ALT)
            ),
            KeyAction::Continue
        ));

        assert!(
            store
                .state
                .approval
                .as_ref()
                .expect("approval pending")
                .visible
        );
        assert!(store.state.approval_auto_open);
        assert_eq!(store.state.focus, FocusPane::Composer);
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
        let _ = execute!(
            stdout,
            DisableBracketedPaste,
            DisableFocusChange,
            DisableMouseCapture,
            Show,
            LeaveAlternateScreen
        );
    }
}
