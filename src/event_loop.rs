use std::io;
use std::time::{Duration, Instant};

#[cfg(not(test))]
use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableFocusChange},
    terminal::disable_raw_mode,
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
        enable_raw_mode,
    },
};
use eyre::Result;
use octos_core::app_ui::AppUiEvent;
use ratatui::backend::{Backend, CrosstermBackend};

use crate::{
    app,
    cli::Cli,
    client_event::ClientEvent,
    insert_history::insert_history_lines_with_size,
    model::{AppState, AppUiCommand, ApprovalModalAction, FocusPane},
    store::Store,
    theme::Palette,
    transport::{AppUiBackend, build_backend},
    tui_terminal::Terminal as InlineTerminal,
    viewport::ScrollbackTracker,
};

/// Poll timeout while idle. Short enough that input stays responsive; we redraw
/// on change (see `draw_dirty`), not on this tick, so this does not cause the
/// 40×/sec repaint that wiped selections in the old alt-screen model.
const UI_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(25);
const INITIAL_CAPABILITIES_HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1500);
const INITIAL_CAPABILITIES_HANDSHAKE_POLL: Duration = Duration::from_millis(10);
/// Redraw cadence while a turn is active, so the spinner/status animates without
/// a fixed-rate repaint when nothing is happening.
const ANIMATION_INTERVAL: Duration = Duration::from_millis(120);
const MAX_BACKEND_EVENTS_PER_TICK: usize = 512;
/// Cap on queued terminal input events handled per frame. High enough that a
/// momentum-scroll burst coalesces into one repaint, low enough that a
/// pathological event stream cannot starve rendering.
const MAX_INPUT_EVENTS_PER_TICK: usize = 64;

/// Which screen model the terminal is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    /// Inline viewport at the bottom + normal scrollback above (the chat flow:
    /// native select / scroll / copy work here with no mode key).
    Inline,
    /// Full-screen alternate buffer for a transient overlay (inspector,
    /// onboarding wizard, detail modals) — matches codex's alt-screen overlays.
    AltScreen,
}

pub fn run(cli: Cli) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Inline-viewport model (codex-style): we do NOT enter the alternate screen
    // for the main chat. The terminal keeps its normal scrollback, so finalized
    // output written there (see `insert_history`) is natively mouse-selectable,
    // wheel/scrollbar-scrollable, and copyable (incl. via tmux copy-mode) with
    // NO app mode key. We also deliberately do NOT `EnableMouseCapture`, which
    // would route click-drag to the app and defeat native selection.
    execute!(stdout, EnableBracketedPaste, EnableFocusChange)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = InlineTerminal::new(backend)?;
    let mut guard = TerminalGuard {
        mode: RenderMode::Inline,
        saved_inline_viewport: None,
        saved_visible_history_extent: None,
        saved_inline_screen_size: None,
        mouse_captured: false,
    };

    // i18n: select the UI language before the first render. `t!()` reads this
    // process-global locale, chosen at launch via --lang / OCTOS_LANG / LANG
    // and switchable at runtime by the `/lang <code>` command (which re-sets
    // the locale + rebuilds the open menu; the next frame repaints).
    rust_i18n::set_locale(cli.lang.code());
    let mut backend = build_backend(&cli);
    let snapshot = backend.bootstrap()?;
    let mut store = Store::from_snapshot(snapshot);
    // Seed cross-session command history from disk (best-effort) so Up/Down
    // recall works from the first keystroke; preserved across snapshot replays
    // by `Store::apply_event`.
    store.state.composer_history = crate::history::ComposerHistory::load_from_default_path();
    // Seed the runtime palette from the launch theme (`--theme`/config). The
    // `/theme` menu mutates this field, so the palette below is recomputed each
    // frame from `store.state.theme` rather than captured once at startup.
    store.state.theme = cli.theme;
    // `--scroll-mode pinned` opts into app-side wheel handling (composer stays
    // pinned); the default `native` keeps the wheel on the terminal so native
    // selection/copy survive. Seeded once at launch, read-only afterwards.
    store.state.pinned_scroll = cli.scroll_mode == crate::cli::ScrollMode::Pinned;
    // Retain the launch config path so `/saveconfig` can persist runtime UI
    // settings (theme/lang/scroll-mode/vim-mode) back into it.
    store.state.config_path = cli.config.clone();
    // Seed Vim modal editing from the launch flag/config (default off). Runtime
    // `/vimmode` toggles it afterwards; the composer starts in Insert.
    store.state.vim_mode = cli.vim_mode;
    // Seed the onboarding workspace candidate so the first-launch workspace
    // probe validates a real directory. The explicit `--cwd` wins; when it is
    // absent the store falls back to the process working directory (for
    // transport-local launches), so the documented `octos serve --stdio --solo`
    // launch — which carries no `--cwd` and whose transport label resolves to
    // `"stdio"`/empty — still validates out of the box instead of dead-ending on
    // "no usable workspace cwd". Without this the onboarding probe (which reads
    // the transport label, not the top-level `--cwd` that only reaches
    // `session/open`) leaves `/onboard finish` blocked on an unvalidated
    // workspace and the profile runtime never bootstraps.
    store.seed_onboarding_workspace_cwd(
        cli.cwd
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    );
    // Phase 3 startup picker: remember the pinned `--profile-id` (honored
    // unchanged, never triggers the picker) and, for a locally-spawned solo
    // backend, discover the profiles already on disk. Skipped when a profile is
    // pinned (nothing to pick) or for remote/WebSocket launches (no local
    // profiles dir to read). Best-effort — an empty list just runs onboarding.
    store.state.onboarding.launch_profile_id = cli.profile_id.clone();
    if cli.profile_id.is_none() {
        if let Some(stdio_command) = cli.stdio_command.as_deref() {
            store.state.onboarding.available_profiles =
                crate::profiles::discover_local_profile_ids(Some(stdio_command));
        }
    }
    // In-TUI profiles surface (`/profiles`): resolve the on-disk data dir once so
    // set-default / delete can operate on it, and seed the current default so the
    // list can mark it. Local-solo only (a remote launch has no local data dir).
    if let Some(data_dir) = cli
        .stdio_command
        .as_deref()
        .and_then(|command| crate::profiles::solo_data_dir(Some(command)))
    {
        store.state.onboarding.default_profile = crate::profiles::read_default_profile(&data_dir);
        store.state.onboarding.profiles_data_dir = Some(data_dir.to_string_lossy().into_owned());
    }
    let mut input_state = TerminalInputState::default();
    let mut scrollback = ScrollbackTracker::new();
    // Force a draw on the first iteration.
    let mut dirty = true;
    // Whether the LAST drawn frame reserved a slash/command menu row block. Used
    // to detect the menu open→closed edge so the frame that reclaims those rows
    // repaints the transcript over the vacated band (see `draw`). Sampled only
    // on frames we actually draw, so it tracks what is on screen.
    let mut menu_reserved_last_frame = false;
    if drain_initial_startup_events(backend.as_mut(), &mut store)? {
        dirty = true;
    }
    let mut last_animation = Instant::now();

    loop {
        if drain_backend_events(backend.as_mut(), &mut store)? {
            dirty = true;
        }

        // Redraw on change, not on a fixed tick. While a turn is active we also
        // redraw on the animation cadence so the spinner/status moves; otherwise
        // an idle UI emits no terminal writes and never wipes a live selection.
        let turn_active = store.state.run_state.is_active();
        if turn_active {
            // Watchdog: after a turn has sat parked on an operator decision past
            // the escalation threshold, re-show a hidden prompt (never
            // auto-resolves). The prominent banner is driven purely by elapsed
            // time in the render pass; this handles the modal-visibility side.
            if store.escalate_parked_decision_if_due() {
                dirty = true;
            }
            if last_animation.elapsed() >= ANIMATION_INTERVAL {
                dirty = true;
                last_animation = Instant::now();
            }
        }
        if dirty {
            let menu_reserved_now = app::menu_surface_active(&store.state);
            let menu_just_closed = menu_reserved_last_frame && !menu_reserved_now;
            menu_reserved_last_frame = menu_reserved_now;
            draw(
                &mut terminal,
                &mut guard,
                &mut store,
                &mut scrollback,
                menu_just_closed,
            )?;
            dirty = false;
        }

        let poll = if turn_active {
            ANIMATION_INTERVAL.min(UI_EVENT_POLL_INTERVAL)
        } else {
            UI_EVENT_POLL_INTERVAL
        };
        if event::poll(poll)? {
            // Drain every already-queued input event before the next redraw:
            // momentum scrolling delivers dozens of wheel events per second,
            // and repainting the full alt-screen after EACH one is what makes
            // pager scrolling feel laggy. One frame per batch, with a cap so
            // a pathological event stream can never starve rendering.
            let mut quit = false;
            for _ in 0..MAX_INPUT_EVENTS_PER_TICK {
                let raw_event = event::read()?;
                let next_event_waiting = event::poll(Duration::from_millis(0))?;
                let is_resize = matches!(raw_event, Event::Resize(_, _));
                match handle_terminal_event_with_input_state(
                    &mut store,
                    raw_event,
                    &mut input_state,
                    next_event_waiting,
                    Instant::now(),
                ) {
                    KeyAction::Continue => {}
                    KeyAction::Quit => {
                        quit = true;
                        break;
                    }
                    KeyAction::Send(command) => {
                        send_command(backend.as_mut(), &mut store, *command)
                    }
                }
                // A resize invalidates the inline viewport layout; force a repaint.
                if is_resize {
                    terminal.invalidate_viewport();
                }
                if !next_event_waiting {
                    break;
                }
            }
            dirty = true;
            if quit {
                break;
            }
        }

        if flush_pending_clipboard(&mut store) {
            // The OSC 52 write does not touch the rendered frame, but staging it
            // changed status text; redraw so the status line reflects the copy.
            dirty = true;
        }

        // Tick-driven staged-drain backstop: a prompt re-staged after a
        // transport-death (and any staged prompt whose wake site was missed)
        // flows once its gate TTL clears, without needing a turn event or a
        // session switch. The enqueued submit is sent by
        // `drain_backend_events` below via the follow-up queue.
        if store.drain_staged_backstop() {
            dirty = true;
        }

        // Terminal sub-agent chips age out of the strip on this same tick
        // cadence (the loop already wakes every UI_EVENT_POLL_INTERVAL, so no
        // dedicated timer): finished/failed agents linger long enough to
        // read, then leave. O(agents) when nothing expires.
        if store.sweep_terminal_agents(std::time::Instant::now()) {
            dirty = true;
        }

        // A staged provider Test/Save that never receives its response must
        // not freeze the model-config surface forever — time it out here.
        if store.sweep_provider_pending(std::time::Instant::now()) {
            dirty = true;
        }

        if drain_backend_events(backend.as_mut(), &mut store)? {
            dirty = true;
        }
    }

    drop(guard);
    Ok(())
}

/// Draw one frame. In `Inline` mode this flushes newly-finalized history into
/// scrollback (so it becomes natively selectable) and renders only the live UI
/// into the bottom inline viewport. For full-screen overlays it switches to the
/// alternate screen and renders the legacy full layout (codex does the same for
/// its transient overlays).
fn draw<B>(
    terminal: &mut InlineTerminal<B>,
    guard: &mut TerminalGuard,
    store: &mut Store,
    scrollback: &mut ScrollbackTracker,
    menu_just_closed: bool,
) -> Result<()>
where
    B: Backend + io::Write,
{
    let palette = Palette::for_theme(store.state.theme);

    if app::wants_fullscreen_overlay(&store.state) {
        // Transient overlay → alternate screen, full legacy render.
        guard.enter_alt_screen(terminal)?;
        guard.sync_mouse_capture(terminal, app::wants_mouse_capture(&store.state))?;
        let size = terminal.size()?;
        store.state.last_terminal_width = size.width;
        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        let resized = size != terminal.last_known_screen_size || terminal.viewport_area != area;
        if resized {
            terminal.set_viewport_area(area);
            terminal.clear_visible_screen()?;
            terminal.invalidate_viewport();
            terminal.last_known_screen_size = size;
        }
        terminal.draw(|frame| {
            // `render_inline_overlay` is generic over `FrameLike`, so it renders
            // the legacy full-screen layout straight into the inline `Frame`'s
            // buffer (no `ratatui::Terminal` needed for the overlay path).
            app::render_inline_overlay(frame, &store.state, palette);
        })?;
        return Ok(());
    }

    // Inline chat flow. In native scroll-mode capture is released BEFORE the
    // screen switch so native selection works the instant the user is back on
    // scrollback; pinned scroll-mode keeps capture on so the next wheel-up can
    // re-enter the pager.
    guard.sync_mouse_capture(terminal, app::wants_mouse_capture(&store.state))?;
    guard.leave_alt_screen(terminal)?;

    let size = terminal.size()?;
    let width = size.width;
    // Key handlers gate on the drawn width (side-by-side diff toggle); record
    // it on the frame that renders, so gate and render agree.
    store.state.last_terminal_width = width;

    // A slash/command menu is a RESERVED viewport row block (`menu_height` in
    // `render_viewport_with_finalization`), not a floating overlay. Opening it
    // grows the bottom-pinned inline viewport, and that grow scrolls whatever
    // committed transcript sat above the viewport UP into scrollback
    // (`resize_viewport_to_size` grow path / `insert_history_lines`), updating
    // the visible-history watermark to the scrolled position. When the menu
    // CLOSES the viewport shrinks back, but the incremental shrink path clears
    // only from the OLD (higher) viewport top DOWNWARD — the scrolled-up
    // transcript is left stranded high on the screen with a `menu_height` blank
    // band gaping between it and the composer (a plain committed re-flush alone
    // can't fix it: `insert_history_lines` would append the fresh copy right
    // below the stranded one, duplicating it). So do exactly what a resize does
    // for a clean re-render: wipe the visible screen and re-flush the whole
    // committed transcript flush against the now-shrunk viewport. One frame,
    // and it only fires on the open→closed edge (see the event loop), so
    // repeated menu cycles never accumulate blank bands.
    if menu_just_closed {
        terminal.clear_visible_screen()?;
        scrollback.mark_flushed_stale();
    }

    // This frame will take the FULL viewport reset inside
    // `resize_viewport_to` (width change either direction, or terminal-
    // height shrink — mirror of its exact condition): the reset clears the
    // whole visible screen, erasing the transcript rows already flushed
    // there. Forget the flushed watermark BEFORE the sync below, so this
    // same frame re-inserts the committed history freshly wrapped at the
    // new width — otherwise the chat visually vanishes, leaving a bare
    // composer (the old-width copy survives only in real scrollback).
    if size.width != terminal.last_known_screen_size.width
        || size.height < terminal.last_known_screen_size.height
    {
        scrollback.mark_flushed_stale();
    }

    // A dismissed `/btw` aside shrinks the live region and strands a blank
    // band between the transcript tail and the composer — with the turn
    // settled, nothing ever refills it (user report: "huge blank space").
    // The store requests a one-shot re-flush; staling the tracker here makes
    // THIS frame re-insert the transcript over the vacated rows. Which
    // watermark to stale depends on whether the main turn is still
    // streaming (codex P2 ×2 on #288):
    // - settled: committed-only (`mark_committed_flush_stale`) — the kept
    //   `completed_live` watermarks dedupe content that already streamed;
    // - still streaming: a committed-only re-flush can be SHORTER than the
    //   vacated band (e.g. only the user prompt is committed) and later
    //   live chunks would append under a duplicated committed block — so
    //   re-emit the full coherent committed+live block instead
    //   (`mark_flushed_stale`; the pre-dismissal copy migrates up into
    //   scrollback, the same bounded duplication the resize path accepts).
    match store.state.take_transcript_reflush_request() {
        Some(crate::model::TranscriptReflushScope::WithLive) => scrollback.mark_flushed_stale(),
        Some(crate::model::TranscriptReflushScope::CommittedOnly) => {
            scrollback.mark_committed_flush_stale()
        }
        None => {}
    }

    // The scrollback flush must wrap to the SAME width `insert_history_lines`
    // uses (the full viewport width), so the line accounting stays consistent.
    let wrap_width = usize::from(width).max(1);

    let update = scrollback.sync(&store.state, palette, wrap_width);
    let live_tail_finalization = update.live_tail_finalization.clone();
    let height = app::live_ui_height_with_finalization(
        &store.state,
        width,
        size.height,
        live_tail_finalization.as_ref(),
    );
    let needs_history_insert = !update.lines_to_insert.is_empty();
    let needs_resize = terminal.viewport_resize_needed(height, size);

    // The inline structural order mirrors codex-rs: size/clear the viewport
    // first, insert finalized history using that width/anchor, then draw. If a
    // resize/clear or history insertion is pending, batch those structural
    // writes with the frame inside DEC synchronized update so native selections
    // are disturbed only when the terminal genuinely changes.
    if needs_resize || needs_history_insert {
        synchronized_terminal_update(terminal, |terminal| {
            draw_inline_frame(
                terminal,
                &store.state,
                palette,
                height,
                size,
                update.lines_to_insert,
                live_tail_finalization,
            )
        })
    } else {
        draw_inline_frame(
            terminal,
            &store.state,
            palette,
            height,
            size,
            update.lines_to_insert,
            live_tail_finalization,
        )
    }
}

fn draw_inline_frame<B>(
    terminal: &mut InlineTerminal<B>,
    state: &crate::model::AppState,
    palette: Palette,
    height: u16,
    size: ratatui::layout::Size,
    lines_to_insert: Vec<ratatui::text::Line<'static>>,
    live_tail_finalization: Option<app::LiveTurnFinalization>,
) -> Result<()>
where
    B: Backend + io::Write,
{
    // `size` is the SAME snapshot the caller used for the scrollback
    // stale-mark — one sample drives both the reset decision and the
    // re-flush, so they can never disagree about a mid-frame resize.
    terminal.resize_viewport_to_size(height, size)?;
    if !lines_to_insert.is_empty() {
        insert_history_lines_with_size(terminal, lines_to_insert, size)?;
        terminal.invalidate_viewport();
    }
    terminal.draw(|frame| {
        app::render_viewport_with_finalization(
            frame,
            state,
            palette,
            size.height,
            live_tail_finalization.as_ref(),
        );
    })?;
    Ok(())
}

fn synchronized_terminal_update<B>(
    terminal: &mut InlineTerminal<B>,
    operations: impl FnOnce(&mut InlineTerminal<B>) -> Result<()>,
) -> Result<()>
where
    B: Backend + io::Write,
{
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let operation_result = operations(terminal);
    let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    match (operation_result, end_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), _) => Err(err),
        (Ok(()), Err(err)) => Err(err.into()),
    }
}

/// Drain a pending clipboard request by emitting the OSC 52 escape sequence to
/// the terminal. The store stages the text (via `/copy` or `Ctrl+Y`) because it
/// has no terminal handle; this is the one spot that owns stdout. OSC 52 is an
/// out-of-band terminal command (it sets the clipboard, not screen cells) so it
/// does not disturb the ratatui-rendered frame, and it travels in-band over the
/// PTY/SSH channel — the only clipboard path that reaches the operator's local
/// machine when the TUI runs against a remote fleet mini.
///
/// A failed write (e.g. a closed stdout during teardown) is intentionally
/// swallowed: the copy is best-effort UX, never load-bearing, and the status
/// line already reflected the attempt.
fn flush_pending_clipboard(store: &mut Store) -> bool {
    flush_pending_clipboard_to(store, &mut io::stdout())
}

/// Drain a staged clipboard request into `sink` as an OSC 52 escape sequence.
/// Split from `flush_pending_clipboard` so tests can inject an in-memory buffer
/// instead of writing to the real terminal — a direct `stdout` write bypasses
/// libtest's capture, so a terminal that honors OSC 52 would otherwise clobber
/// the developer's clipboard during `cargo test` (codex P2).
fn flush_pending_clipboard_to<W: io::Write>(store: &mut Store, sink: &mut W) -> bool {
    let Some(text) = store.state.pending_clipboard.take() else {
        return false;
    };
    let sequence = crate::clipboard::osc52_copy_sequence(&text);
    if sink.write_all(sequence.as_bytes()).is_ok() {
        let _ = sink.flush();
    }
    true
}

/// Drain pending backend events into the store. Returns `true` when at least
/// one event was applied, so the event loop knows the UI is dirty and must
/// redraw (the inline-viewport model only repaints on change).
fn drain_backend_events(backend: &mut dyn AppUiBackend, store: &mut Store) -> Result<bool> {
    let mut applied = false;
    for _ in 0..MAX_BACKEND_EVENTS_PER_TICK {
        let Some(event) = backend.next_event()? else {
            drain_pending_autonomy_hydration(backend, store);
            return Ok(applied);
        };
        apply_client_event_and_send_followup(backend, store, event);
        applied = true;
    }

    drain_pending_autonomy_hydration(backend, store);
    Ok(applied)
}

/// Give the initial protocol capabilities probe a bounded chance to land before
/// the first frame. First-launch onboarding is capability-gated, so drawing
/// before this handshake can flash or stick on an empty inline composer.
fn drain_initial_startup_events(backend: &mut dyn AppUiBackend, store: &mut Store) -> Result<bool> {
    let deadline = Instant::now() + INITIAL_CAPABILITIES_HANDSHAKE_TIMEOUT;
    let mut applied = false;
    while should_wait_for_initial_capabilities(store) {
        applied |= drain_backend_events(backend, store)?;
        if !should_wait_for_initial_capabilities(store) || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(INITIAL_CAPABILITIES_HANDSHAKE_POLL);
    }
    Ok(applied)
}

fn should_wait_for_initial_capabilities(store: &Store) -> bool {
    store.state.sessions.is_empty()
        && store.state.capabilities.is_none()
        && !store.state.menu_stack.is_active()
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

/// Send any queued reconnect hydration commands the store has staged
/// (e.g. on `session/opened` after reconnect). Bounded by the queue cap
/// inside `AppState` itself.
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

/// Public so contract tests (`tests/*.rs`) can drive the key/mouse pipeline
/// exactly as the run loop does and assert on the resulting action.
pub enum KeyAction {
    Continue,
    Quit,
    Send(Box<AppUiCommand>),
}

impl KeyAction {
    fn send(command: AppUiCommand) -> Self {
        Self::Send(Box::new(command))
    }
}

fn handle_terminal_event_with_input_state(
    store: &mut Store,
    event: Event,
    input_state: &mut TerminalInputState,
    next_event_waiting: bool,
    now: Instant,
) -> KeyAction {
    if let Event::Key(key) = event {
        // Skip the unbracketed-paste newline heuristic while a modal or a peek
        // owns the keyboard — both force Composer focus, so without this an
        // Enter (or pasted newline) lands in the hidden draft and never reaches
        // the modal's / peek's Enter handler. Those surfaces handle Enter
        // themselves downstream.
        if !modal_owns_keyboard(store)
            && !app::agent_view_active(&store.state)
            && is_plain_composer_enter(store, &key)
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

pub fn handle_terminal_event(store: &mut Store, event: Event) -> KeyAction {
    match event {
        Event::Key(key) => handle_key(store, key),
        Event::Paste(text) => handle_paste(store, &text),
        Event::Mouse(mouse) => handle_mouse(store, mouse),
        Event::FocusGained | Event::FocusLost | Event::Resize(_, _) => KeyAction::Continue,
    }
}

fn handle_mouse(store: &mut Store, mouse: MouseEvent) -> KeyAction {
    const MOUSE_SCROLL_LINES: usize = 4;
    // Inside the pager the wheel steps a single line: macOS trackpads deliver
    // dozens of fine-grained scroll events per second during momentum
    // scrolling, and multiplying each by 4 makes the transcript jump instead
    // of glide. Other surfaces (modals, workspace/git panes) keep the coarser
    // step that suits clicky wheel mice.
    let lines = if store.state.transcript_pager_active {
        1
    } else {
        MOUSE_SCROLL_LINES
    };
    match mouse.kind {
        MouseEventKind::ScrollUp => scroll_current_surface_up(store, lines),
        MouseEventKind::ScrollDown => scroll_current_surface_down(store, lines),
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

/// True while a modal overlay (not a menu) owns the keyboard — the same set,
/// in the same order, that `handle_plain_key` routes to before the menu/global
/// arms: the activity navigator, the approval modal, the AskUserQuestion
/// picker, and the task-output / artifact / thread-graph / turn-state detail
/// viewers. While any of these is up, global composer edits (Ctrl+U, modified
/// cursor/word keys) and pastes must not mutate the composer hidden underneath
/// — `show_pending_approval` force-focuses the composer, so a focus check
/// alone cannot catch this.
fn modal_owns_keyboard(store: &Store) -> bool {
    store.state.activity_navigator.active
        || store
            .state
            .approval
            .as_ref()
            .is_some_and(|approval| approval.visible)
        || store
            .state
            .user_question
            .as_ref()
            .is_some_and(|picker| picker.visible)
        || store.state.task_output.active
        || store.state.artifact_detail.active
        || store.state.thread_graph_detail.active
        || store.state.turn_state_detail.active
}

pub(crate) fn handle_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    if key.kind != KeyEventKind::Press {
        return KeyAction::Continue;
    }

    if is_control_char(&key, 'q') {
        return KeyAction::Quit;
    }

    if is_control_char(&key, 'c') {
        // A turn parked on a decision (approval / question) can be waiting BEFORE
        // any reply streams, so `active_turn()` — hence `interrupt_command()` —
        // is a no-op there. Route through the decision-aware interrupt so Ctrl+C
        // reliably cancels a parked turn (and tears down its server-side waiter).
        let command = if app::active_session_has_pending_decision(&store.state) {
            store.interrupt_active_decision_command()
        } else {
            store.interrupt_command()
        };
        return command.map_or(KeyAction::Continue, KeyAction::send);
    }

    // A live sub-agent peek OWNS the keyboard, exactly like a modal: routed here
    // — after the always-on quit/interrupt keys but BEFORE every composer /
    // Ctrl / paste mutation path — so typing, Ctrl+U, word-deletes, Vim edits
    // and paste can't leak into the composer that is hidden behind the overlay.
    // `agent_view_active` is false while any modal is up (so the modal keeps the
    // keyboard) or when the selected agent has vanished (so the inline composer
    // stays editable).
    if app::agent_view_active(&store.state) {
        return handle_agent_peek_key(store, key);
    }

    if is_control_char(&key, 'u') {
        // Swallowed (not cleared) while a modal owns the keyboard: clearing
        // staged messages / the hidden draft under a dialog is invisible data
        // loss.
        if !modal_owns_keyboard(store) {
            store.clear_composer_or_staged_messages();
        }
        return KeyAction::Continue;
    }

    if is_control_char(&key, 'o') {
        store.state.toggle_tool_output_expansion();
        return KeyAction::Continue;
    }

    if is_control_char(&key, 'y') {
        // Yank: copy the last assistant reply to the clipboard. The store
        // stages the text; `flush_pending_clipboard` (called each tick from the
        // run loop) emits the OSC 52 escape sequence so the copy reaches the
        // operator's local clipboard even over SSH.
        store.copy_last_reply();
        return KeyAction::Continue;
    }

    if is_control_char(&key, 't') {
        toggle_transcript_pager(store);
        return KeyAction::Continue;
    }

    // Ctrl+P folds/unfolds the ◆ Goal banner objective. A huge pasted objective
    // (e.g. shader code) folds to one compact preview row by default; Ctrl+P
    // expands it (and re-folds). Only claimed while the active session has a
    // goal — otherwise the key falls through unswallowed so it stays free.
    // (Ctrl+P — Ctrl+G collides with browser bindings, Ctrl+E is the composer's
    // cursor-to-end-of-line, and Alt/Option+E is a dead accent key on macOS
    // unless "Option as Meta" is enabled. Ctrl+P is free and works everywhere.)
    if is_control_char(&key, 'p') && app::active_session_has_goal(&store.state) {
        store.state.toggle_goal_objective_fold();
        return KeyAction::Continue;
    }

    // Modified-key composer edits are gated on no modal owning the keyboard:
    // the approval/question modals force-focus the composer, so without the
    // gate Ctrl+W / Alt+b / Shift+Enter kept mutating the hidden draft while a
    // dialog was up. Menus are NOT gated here — `handle_menu_key` routes to
    // `handle_composer_modified_key` itself where composer editing is intended
    // — EXCEPT the `@` file picker: its Esc-restore contract ("delete the
    // trigger `@`, composer back to its pre-`@` text") relies on the composer
    // being frozen while the picker is up, so a Ctrl+W leaking through here
    // would silently mutate the hidden draft and break the restore (#363).
    if store.state.focus == FocusPane::Composer
        && !modal_owns_keyboard(store)
        && store.state.file_picker.is_none()
        && handle_composer_modified_key(store, key)
    {
        return KeyAction::Continue;
    }

    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        return handle_plain_key(store, key);
    }

    if is_alt_char(&key, 'a') {
        // Recovery key: re-open a hidden pending modal so an accidental Esc never
        // wedges the turn (DO-NOT-SHIP #1). Precedence is deterministic — a hidden
        // question is re-shown FIRST, since a pending AskUserQuestion blocks the
        // turn at the same boundary as an approval but is answered through a
        // distinct picker; an approval is only re-shown if there is no hidden
        // question to recover.
        if !store.show_pending_user_question() {
            store.show_pending_approval();
        }
        return KeyAction::Continue;
    }

    if is_alt_char(&key, 'j') {
        move_down(store);
        return KeyAction::Continue;
    }

    if is_alt_char(&key, 'k') {
        move_up(store);
        return KeyAction::Continue;
    }

    // Agent Dock (#323): Alt+G toggles the sub-agent strip between the
    // one-line summary pill and the per-agent rows. NOT Alt+D — the composer
    // claims that as readline delete-word-forward (handle_composer_modified_key
    // runs first while the composer has focus, mini4 soak catch). Only claimed
    // while a roster exists — with no agents the strip is height-0 and the key
    // stays free.
    if is_alt_char(&key, 'g') && !store.state.active_session_agents().is_empty() {
        store.state.agent_dock_collapsed = !store.state.agent_dock_collapsed;
        return KeyAction::Continue;
    }

    KeyAction::Continue
}

fn handle_paste(store: &mut Store, text: &str) -> KeyAction {
    // A peek owns the keyboard and hides the composer; drop pastes so they can't
    // silently accumulate in the hidden draft.
    if text.is_empty() || app::agent_view_active(&store.state) {
        return KeyAction::Continue;
    }

    // The composer is a PLAIN-TEXT field. Strip styling/control noise from a paste
    // (web copies carry ANSI/CSI/OSC escapes, zero-width + format chars, CR, tabs).
    // Inserted raw, those have zero/odd display width but still occupy buffer bytes,
    // so the byte-cursor and the width-based render desync: the text fails to render
    // and backspace leaves residue (it deletes the invisible bytes, not the glyphs).
    let text = sanitize_pasted_text(text);
    if text.is_empty() {
        return KeyAction::Continue;
    }

    // Route the paste to whoever owns the keyboard, mirroring the plain-key
    // dispatch order — a paste used to bypass every modal and land invisibly
    // in the force-focused composer underneath.
    //
    // (a) The AskUserQuestion free-text "Other" box is actively capturing:
    // append there, exactly like the per-char capture arm (newlines flattened —
    // the box is single-line and Enter means "advance/submit" in the picker).
    if store
        .state
        .user_question
        .as_ref()
        .is_some_and(|picker| picker.visible)
        && store.user_question_editing_free_text()
    {
        for ch in text.chars() {
            store.user_question_push_free_text(if ch == '\n' { ' ' } else { ch });
        }
        return KeyAction::Continue;
    }

    // (b) Any other keyboard-owning modal (approval, question without its
    // free-text box active, detail viewers, activity navigator): dropping the
    // paste with a visible status beats silently editing the hidden composer.
    if modal_owns_keyboard(store) {
        store.state.status = "Paste ignored while a dialog is open".to_string();
        return KeyAction::Continue;
    }

    // (c) A searchable menu is filtering (and the composer is not the menu's
    // text target — slash-popup drafts and menu composer-edit fields still
    // paste into the composer below): extend the search query, the same target
    // the plain-char capture arm feeds.
    if store.state.menu_stack.is_active()
        && !slash_help_capture_active(store)
        && !menu_composer_edit_active(store)
        && active_menu_searchable(store)
    {
        append_active_menu_search_text(store, &text);
        return KeyAction::Continue;
    }

    // (d) Default: the composer. A paste is literal text and must NEVER trigger
    // a slash command — a pasted file path ("/Users/...") or multi-line snippet
    // beginning with '/' is not a command. Unlike a typed leading '/', we do
    // not open the slash-command menu here. (Regression: pasting a path
    // opened/ran the slash menu.)
    store.state.insert_pasted_text(&text);
    store.state.focus = FocusPane::Composer;

    // Only keep an ALREADY-open slash search in sync (e.g. the user typed '/' to
    // open the menu, then pasted an argument); never open it from a paste.
    if store.state.menu_stack.is_active() && slash_help_menu_active(store) {
        sync_slash_help_search_query(store);
    }

    KeyAction::Continue
}

/// Reduce pasted text to the plain text the composer can render. Strips ANSI/CSI/OSC
/// escape sequences and control + zero-width/format chars (which web copies carry and
/// which desync the byte-cursor from the width-based render), normalizes CR/CRLF to
/// LF, and turns tabs into a space. Newlines are kept (the composer is multi-line).
fn sanitize_pasted_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ ... <final byte 0x40..=0x7e>
                Some('[') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ESC \
                Some(']') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Other two-char ESC sequence (e.g. ESC + a letter): drop both.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // 8-bit C1 CSI (U+009B) / OSC (U+009D): the single-char forms of ESC[
            // and ESC]. Drop the whole sequence, not just the introducer (else the
            // params/text like "31m…" leak into the composer).
            '\u{9b}' => {
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            '\u{9d}' => {
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n == '\u{7}' || n == '\u{9c}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            '\n' => out.push('\n'),
            // CR / CRLF -> single LF (web copies often use \r\n).
            '\r' => {
                out.push('\n');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\t' => out.push(' '),
            // Unicode line / paragraph separators -> newline (composer is multi-line).
            '\u{2028}' | '\u{2029}' => out.push('\n'),
            // Invisible / non-rendering format codepoints (zero-width, bidi controls,
            // variation selectors, BOM, ...). Inserted raw they have zero/odd display
            // width but still occupy buffer bytes, so the byte-cursor desyncs from the
            // width-based render: text fails to render and backspace leaves residue.
            c if is_invisible_format_char(c) => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Invisible / non-rendering format codepoints that rich (HTML) clipboard copies
/// carry: zero-width spaces & joiners, bidi (Trojan-Source) controls, variation
/// selectors, BOM, interlinear-annotation marks, and tag chars. The plain-text
/// composer can't render these — raw, they desync the byte-cursor from the
/// width-based render. Mirrors codex's curated strip set
/// (codex-rs/tui/src/terminal_title.rs::is_disallowed_terminal_title_char), widened
/// to the full tags / variation-selectors-supplement plane.
fn is_invisible_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'                    // soft hyphen
            | '\u{034f}'              // combining grapheme joiner
            | '\u{061c}'              // arabic letter mark
            | '\u{115f}'..='\u{1160}' // hangul choseong/jungseong fillers
            | '\u{17b4}'..='\u{17b5}' // khmer inherent vowels (invisible)
            | '\u{180b}'..='\u{180e}' // mongolian free variation selectors + vowel sep
            | '\u{200b}'..='\u{200f}' // ZWSP, ZWNJ, ZWJ, LRM, RLM
            | '\u{202a}'..='\u{202e}' // bidi embedding / override
            | '\u{2060}'..='\u{206f}' // word joiner, invisibles, deprecated format
            | '\u{3164}'              // hangul filler
            | '\u{fe00}'..='\u{fe0f}' // variation selectors
            | '\u{feff}'              // BOM / zero-width no-break space
            | '\u{ffa0}'              // halfwidth hangul filler
            | '\u{fff9}'..='\u{fffb}' // interlinear annotation
            | '\u{1bca0}'..='\u{1bca3}' // shorthand format controls
            | '\u{1d173}'..='\u{1d17a}' // musical symbol formatting
            | '\u{e0000}'..='\u{e0fff}' // tags + variation selectors supplement
    )
}

/// Keyboard handler for the full-screen sub-agent peek. Reached from
/// `handle_key` only while `agent_view_active` — i.e. the peek is the active
/// surface — so it runs ahead of every composer / Ctrl / paste path and the peek
/// can safely OWN the keyboard. Navigation and scroll act; Ctrl+C / Ctrl+Q are
/// already handled upstream in `handle_key`; every other key is swallowed so it
/// can't reach the composer hidden behind the overlay.
fn handle_agent_peek_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    // Alt+A stays a global recovery valve even while peeking: re-show a hidden
    // pending question/approval. A hidden modal does NOT make the peek yield
    // (nothing is visible to render), so without this the peek would swallow the
    // one key that recovers it. Once re-shown the modal is visible, the peek
    // yields, and the modal owns the keyboard.
    if is_alt_char(&key, 'a') {
        if !store.show_pending_user_question() {
            store.show_pending_approval();
        }
        return KeyAction::Continue;
    }
    match key.code {
        // Cycle to the next / previous target (wrapping through `main`).
        // #334 (Phase 2): after landing on a sub-agent, pull its full output so
        // the detail view renders `final_output`, not just the streamed tail.
        KeyCode::Tab => {
            store.state.select_next_chat_view();
            if let Some(command) = store.agent_view_output_fetch_command() {
                return KeyAction::send(command);
            }
        }
        KeyCode::BackTab => {
            store.state.select_prev_chat_view();
            if let Some(command) = store.agent_view_output_fetch_command() {
                return KeyAction::send(command);
            }
        }
        // Leave the peek back to the inline chat.
        KeyCode::Esc => {
            store
                .state
                .set_chat_view(crate::model::ChatViewTarget::Main);
        }
        // Scroll the agent output via its own from-bottom offset.
        KeyCode::Up => store.state.scroll_agent_view_up(1),
        KeyCode::Down => store.state.scroll_agent_view_down(1),
        KeyCode::PageUp => store.state.scroll_agent_view_up(8),
        KeyCode::PageDown => store.state.scroll_agent_view_down(8),
        KeyCode::Home => store.state.scroll_agent_view_up(usize::MAX),
        KeyCode::End => store.state.scroll_agent_view_down(usize::MAX),
        // #335 (Phase 3): cancel the viewed sub-agent (no-op if it is already
        // terminal or the backend can't interrupt).
        KeyCode::Char('x') => {
            if let Some(command) = store.interrupt_viewed_agent_command() {
                return KeyAction::send(command);
            }
        }
        // Everything else (text, Enter, Backspace, `/`, Vim keys, …) is swallowed.
        _ => {}
    }
    KeyAction::Continue
}

fn handle_plain_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    if store.state.activity_navigator.active {
        return handle_activity_navigator_key(store, key);
    }

    if store
        .state
        .approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
    {
        return handle_approval_modal_key(store, key);
    }

    if store
        .state
        .user_question
        .as_ref()
        .is_some_and(|picker| picker.visible)
    {
        return handle_user_question_key(store, key);
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

    // Vim modal editing: only after every overlay/menu has had its say, so Vim
    // never hijacks their keys. No-op (returns None) unless Vim is enabled and
    // the composer is focused.
    if let Some(action) = handle_composer_vim_key(store, &key) {
        return action;
    }

    match key.code {
        // Tab / Shift+Tab ENTER the sub-agent peek from the inline chat: they
        // cycle the main pane across `[main, …running sub-agents]` and select as
        // they move, repurposing Tab away from the now-disabled side-panel focus
        // cycle. Gated to the inline composer (not the pager or an inspector
        // pane) so a peek is only entered from a clean inline state; once a peek
        // is active `handle_agent_peek_key` owns Tab (this arm is unreachable
        // then). No-op when the session has no sub-agents.
        KeyCode::Tab
            if store.state.focus == FocusPane::Composer && !store.state.transcript_pager_active =>
        {
            store.state.select_next_chat_view();
            // #334 (Phase 2): pull the child's full output when the peek opens.
            if let Some(command) = store.agent_view_output_fetch_command() {
                return KeyAction::send(command);
            }
        }
        KeyCode::BackTab
            if store.state.focus == FocusPane::Composer && !store.state.transcript_pager_active =>
        {
            store.state.select_prev_chat_view();
            if let Some(command) = store.agent_view_output_fetch_command() {
                return KeyAction::send(command);
            }
        }
        // Esc clears a stale peek selection (an `Agent(id)` whose agent has
        // vanished, so `agent_view_active` is false and this handler — not
        // `handle_agent_peek_key` — sees the key). A live peek's Esc is handled
        // upstream in `handle_key`.
        KeyCode::Esc if store.state.chat_view != crate::model::ChatViewTarget::Main => {
            store
                .state
                .set_chat_view(crate::model::ChatViewTarget::Main);
        }
        KeyCode::Esc if store.state.transcript_pager_active => {
            store.state.exit_transcript_pager();
        }
        // Side-pane focus (only reachable via `/ps` / `!cmd` now that Tab no
        // longer cycles panes): Esc simply returns focus to the composer.
        // Without this arm the plain-Esc handler below would fire and
        // INTERRUPT a running turn — a destructive exit from a read-only
        // inspection pane (codex final-gate P2).
        KeyCode::Esc if store.state.focus != FocusPane::Composer => {
            store.state.focus = FocusPane::Composer;
        }
        // Shell-escape mode (#364): Esc cancels the `!` draft — never runs it,
        // never interrupts the turn. The NEXT Esc (composer now plain/empty)
        // falls through to the ordinary interrupt semantics below.
        KeyCode::Esc if store.shell_escape_mode_active() => {
            store.cancel_shell_escape_mode();
        }
        KeyCode::Esc => {
            if store.state.active_turn().is_some() {
                let command = if store.state.has_pending_messages() {
                    store.interrupt_staged_command()
                } else {
                    store.interrupt_command()
                };
                if let Some(command) = command {
                    return KeyAction::send(command);
                }
            } else if let Some(command) = store.cancel_running_background_task_command() {
                // No live foreground turn, but a spawn_only background task
                // (deep_research / run_pipeline / sub-agent orchestration) is
                // still running: the foreground turn already finished and
                // handed off, so `active_turn()` is None and the interrupt path
                // above is a no-op. Esc should still stop the work, so cancel
                // the first running background task — independent of the
                // Tasks-pane selection, which may sit on an older completed
                // task. The command builder returns None (falling through to
                // refocus) when nothing is cancellable or the server doesn't
                // advertise task control.
                return KeyAction::send(command);
            }
            store.state.focus = FocusPane::Composer;
        }
        KeyCode::Char('q') if store.state.focus != FocusPane::Composer => {
            return KeyAction::Quit;
        }
        KeyCode::Char('j') if store.state.focus != FocusPane::Composer => {
            move_down(store);
        }
        KeyCode::Char('k') if store.state.focus != FocusPane::Composer => {
            move_up(store);
        }
        KeyCode::Down if store.state.transcript_pager_active => {
            store.state.scroll_transcript_down(1);
        }
        KeyCode::Up if store.state.transcript_pager_active => {
            store.state.scroll_transcript_up(1);
        }
        // In the composer, Up/Down move the cursor between logical lines; at the
        // first/last line they fall back to the existing transcript scroll so
        // that affordance isn't lost.
        KeyCode::Down if store.state.focus == FocusPane::Composer => {
            // Mirror of Up: while browsing, step to a newer entry first (past the
            // newest, recall_next returns an empty draft); recall_next returns
            // None once the entry is edited, then ordinary cursor movement
            // resumes. Otherwise move the cursor down a line and, only at the
            // last line, try newer history then transcript scroll.
            let current = store.state.composer.clone();
            if store.state.composer_history.is_navigating() {
                if let Some(text) = store.state.composer_history.recall_next(&current) {
                    store.state.set_composer_text(text);
                } else if !store.state.move_composer_cursor_down() {
                    store.state.scroll_transcript_down(1);
                }
            } else if !store.state.move_composer_cursor_down() {
                match store.state.composer_history.recall_next(&current) {
                    Some(text) => store.state.set_composer_text(text),
                    None => store.state.scroll_transcript_down(1),
                }
            }
        }
        KeyCode::Up if store.state.focus == FocusPane::Composer => {
            // While browsing history, Up steps to an older entry FIRST, so a
            // recalled multiline entry doesn't trap the cursor inside it;
            // recall_prev returns None once the entry is edited, and ordinary
            // cursor movement resumes. Otherwise move the cursor up a line and,
            // only at the first line, try history (empty composer) then scroll.
            let current = store.state.composer.clone();
            if store.state.composer_history.is_navigating() {
                if let Some(text) = store.state.composer_history.recall_prev(&current) {
                    store.state.set_composer_text(text);
                } else if !store.state.move_composer_cursor_up() {
                    store.state.scroll_transcript_up(1);
                }
            } else if !store.state.move_composer_cursor_up() {
                match store.state.composer_history.recall_prev(&current) {
                    Some(text) => store.state.set_composer_text(text),
                    None => store.state.scroll_transcript_up(1),
                }
            }
        }
        KeyCode::Down => {
            move_down(store);
        }
        KeyCode::Up => {
            move_up(store);
        }
        KeyCode::PageDown if btw_aside_open(&store.state) => {
            scroll_open_btw_aside(store, 8);
        }
        KeyCode::PageUp if btw_aside_open(&store.state) => {
            scroll_open_btw_aside(store, -8);
        }
        KeyCode::PageDown => match store.state.focus {
            FocusPane::Workspace => store.state.workspace.scroll_down(8),
            FocusPane::Git => store.state.git.scroll_down(8),
            _ => store.state.scroll_transcript_down(8),
        },
        KeyCode::PageUp => match store.state.focus {
            FocusPane::Workspace => store.state.workspace.scroll_up(8),
            FocusPane::Git => store.state.git.scroll_up(8),
            // In the inline chat flow PageUp can only reach the live tail —
            // committed history lives in native scrollback — so the first
            // press opens the pager (at the bottom) instead; inside the pager
            // it pages through the full transcript.
            _ => {
                if transcript_pager_available(&store.state) {
                    store.state.enter_transcript_pager();
                } else {
                    store.state.scroll_transcript_up(8);
                }
            }
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
                return KeyAction::send(command);
            }
        }
        KeyCode::Char('x') if store.state.focus == FocusPane::Tasks => {
            if let Some(command) = store.cancel_task_command() {
                return KeyAction::send(command);
            }
        }
        KeyCode::Char('d') if store.state.focus != FocusPane::Composer => {
            if let Some(command) = store.read_diff_preview_command() {
                return KeyAction::send(command);
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
        KeyCode::Char('v')
            if store.state.focus != FocusPane::Composer && store.state.diff_preview.active =>
        {
            store.toggle_diff_view_mode();
        }
        KeyCode::Char(ch) => {
            // Store-level composer input: inserts the char and runs the prefix
            // triggers (`/` slash popup, `!` shell-escape hint #364, `@` file
            // picker #363) so the trigger decisions stay store-testable.
            store.handle_composer_char_input(ch);
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_activity_navigator_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc if store.state.activity_navigator.search_active => {
            store.state.activity_navigator.search_active = false;
        }
        KeyCode::Esc => {
            store.state.activity_navigator.close();
        }
        KeyCode::Down | KeyCode::Char('j') if !store.state.activity_navigator.search_active => {
            let len = app::activity_navigator_model(&store.state).rows.len();
            store.state.activity_navigator.select_next(len);
        }
        KeyCode::Up | KeyCode::Char('k') if !store.state.activity_navigator.search_active => {
            store.state.activity_navigator.select_prev();
        }
        KeyCode::Tab | KeyCode::Char('f') if !store.state.activity_navigator.search_active => {
            store.state.activity_navigator.cycle_filter();
        }
        KeyCode::Char('/') if !store.state.activity_navigator.search_active => {
            store.state.activity_navigator.search_active = true;
        }
        KeyCode::Backspace if store.state.activity_navigator.search_active => {
            store.state.activity_navigator.pop_query_char();
        }
        KeyCode::Delete if store.state.activity_navigator.search_active => {
            store.state.activity_navigator.clear_query();
        }
        KeyCode::Enter => {
            if let Some(session_id) = app::selected_activity_navigator_session(&store.state)
                && let Some(idx) = store
                    .state
                    .sessions
                    .iter()
                    .position(|session| session.id == session_id)
            {
                // Full switch bundle (drafts, staged-message stash, task/scroll
                // resets) — a bare `selected_session` assignment here left the
                // OLD session's staged prompts on the active queue, so a later
                // terminal event would submit them into the newly selected
                // session (codex P1 on the per-session staged-queue fix).
                store.state.switch_selected_session(idx);
                // ...and drain the incoming session's restored staged queue
                // (codex round-2 P2: without this a staged prompt sits stuck
                // until an unrelated turn event).
                store.drain_staged_after_direct_switch();
                store.state.status = t!(
                    "status.activity_navigator_selected_session",
                    title = store.state.sessions[idx].title.clone()
                )
                .into_owned();
            }
        }
        KeyCode::Char(ch) if store.state.activity_navigator.search_active => {
            store.state.activity_navigator.push_query_char(ch);
        }
        KeyCode::Char(ch) if !ch.is_control() => {
            store.state.activity_navigator.start_search_with_char(ch);
        }
        _ => {}
    }

    KeyAction::Continue
}

fn handle_composer_modified_key(store: &mut Store, key: KeyEvent) -> bool {
    // Shift+Enter is the primary, most intuitive newline key. It only reaches
    // the app when the terminal reports the modifier (the Kitty keyboard
    // protocol, or terminals like Warp that map it directly); otherwise plain
    // Enter is indistinguishable and submits, which is why Ctrl+J is the
    // portable fallback. Handled here, before the Enter→submit arm.
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        store.state.insert_composer_text("\n");
        return true;
    }

    if key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            // Alt+Enter also inserts a newline where the terminal reports it as
            // Enter+ALT (e.g. iTerm2). Some terminals (Warp) send Option+Enter
            // as ESC+CR instead, where it can't be caught — use Shift+Enter or
            // Ctrl+J there.
            KeyCode::Enter => {
                store.state.insert_composer_text("\n");
                return true;
            }
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
            // Ctrl+J is a literal line feed and works in every terminal (no
            // Kitty/modifyOtherKeys needed), so it is the portable newline key
            // alongside Alt+Enter. (Terminals that fold Ctrl+J into Enter will
            // submit instead — Alt+Enter is the fallback there.)
            KeyCode::Char('j') => {
                store.state.insert_composer_text("\n");
                return true;
            }
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

/// Vim modal editing for the composer (pragmatic subset). Returns `Some` when
/// the key was consumed, `None` to fall through to the normal composer/global
/// handling. No-op unless `vim_mode` is on and the composer is focused — so
/// non-Vim users (and every overlay, which is checked earlier) are unaffected.
fn handle_composer_vim_key(store: &mut Store, key: &KeyEvent) -> Option<KeyAction> {
    use crate::model::ComposerMode;

    if !store.state.vim_mode || store.state.focus != FocusPane::Composer {
        return None;
    }

    // Insert mode behaves like a plain field; only Esc is special (→ Normal).
    if store.state.composer_mode == ComposerMode::Insert {
        if key.code == KeyCode::Esc {
            store.state.composer_mode = ComposerMode::Normal;
            store.state.composer_vim_pending = None;
            return Some(KeyAction::Continue);
        }
        return None;
    }

    // ----- Normal mode -----
    // Enter still submits: fall through to the Enter→submit arm.
    if key.code == KeyCode::Enter {
        store.state.composer_vim_pending = None;
        return None;
    }
    // Esc with a pending operator clears it and stays in Normal. Without one,
    // fall through (None) to the global Esc arms: Esc in Normal mode is a vim
    // no-op, but globally it interrupts the running turn, cancels a background
    // task, or exits the transcript pager — swallowing it here made vim mode
    // permanently eat all of those.
    if key.code == KeyCode::Esc {
        if store.state.composer_vim_pending.take().is_some() {
            return Some(KeyAction::Continue);
        }
        return None;
    }
    // Non-character keys (arrows, etc.) fall through so they keep moving the
    // cursor via the normal composer arms.
    let KeyCode::Char(c) = key.code else {
        return None;
    };

    // Resolve a pending two-key sequence (gg / dd / dw / cc).
    if let Some(pending) = store.state.composer_vim_pending.take() {
        match (pending, c) {
            ('g', 'g') => store.state.move_composer_cursor_buffer_start(),
            ('d', 'd') => store.state.delete_composer_line(),
            ('d', 'w') => store.state.delete_composer_word_forward(),
            ('c', 'c') => {
                store.state.clear_composer_line();
                store.state.composer_mode = ComposerMode::Insert;
            }
            // Unknown sequence — pending already cleared, swallow the key.
            _ => {}
        }
        return Some(KeyAction::Continue);
    }

    match c {
        // Motions.
        'h' => store.state.move_composer_cursor_left(),
        'l' => store.state.move_composer_cursor_right(),
        'j' => {
            store.state.move_composer_cursor_down();
        }
        'k' => {
            store.state.move_composer_cursor_up();
        }
        '0' => store.state.move_composer_cursor_line_start(),
        '$' => store.state.move_composer_cursor_line_end(),
        'w' => store.state.move_composer_cursor_word_forward(),
        'b' => store.state.move_composer_cursor_prev_word(),
        'e' => store.state.move_composer_cursor_word_end(),
        'G' => store.state.move_composer_cursor_buffer_end(),
        // Edits.
        'x' => store.state.delete_composer_next_char(),
        // Operator/jump prefixes — wait for the second key.
        'g' | 'd' | 'c' => store.state.composer_vim_pending = Some(c),
        // Enter Insert mode (positioning variants).
        'i' => store.state.composer_mode = ComposerMode::Insert,
        'a' => {
            store.state.move_composer_cursor_right();
            store.state.composer_mode = ComposerMode::Insert;
        }
        'A' => {
            store.state.move_composer_cursor_line_end();
            store.state.composer_mode = ComposerMode::Insert;
        }
        'I' => {
            store.state.move_composer_cursor_line_start();
            store.state.composer_mode = ComposerMode::Insert;
        }
        'o' => {
            store.state.open_composer_line_below();
            store.state.composer_mode = ComposerMode::Insert;
        }
        'O' => {
            store.state.open_composer_line_above();
            store.state.composer_mode = ComposerMode::Insert;
        }
        // Any other key in Normal mode is swallowed (never inserts text).
        _ => {}
    }
    Some(KeyAction::Continue)
}

/// Whether the active session has a `/btw` overlay on screen (so PageUp/PageDown
/// scroll it instead of the transcript). The overlay is non-modal — it floats
/// over the live tail — so it takes the paging keys only while present.
fn btw_aside_open(state: &crate::model::AppState) -> bool {
    state
        .active_session()
        .is_some_and(|session| state.btw_aside_for(&session.id).is_some())
}

/// Scroll the active session's `/btw` overlay by `delta` rows (render clamps to
/// the true content max).
fn scroll_open_btw_aside(store: &mut Store, delta: i32) {
    if let Some(session_id) = store
        .state
        .active_session()
        .map(|session| session.id.clone())
    {
        store.state.nudge_btw_scroll(&session_id, delta);
    }
}

fn handle_composer_enter(store: &mut Store) -> KeyAction {
    // Codex-style dialog dismissal: Enter on an EMPTY composer closes the
    // `/btw` aside pane and returns to the live session (submitting a real
    // prompt already dismisses it via `clear_settled_btw_aside`). Codex's
    // bottom-pane views close on plain Enter; the aside is non-modal, so the
    // empty-composer guard keeps Enter-to-send untouched.
    if store.state.composer.is_empty() {
        let dismissed = store
            .state
            .active_session()
            .map(|session| session.id.clone())
            .is_some_and(|session_id| store.state.dismiss_btw_aside(&session_id));
        if dismissed {
            store.state.status = t!("app.btw.closed").into_owned();
            return KeyAction::Continue;
        }
    }
    let command = store.compose_command();
    if store.state.exit_requested {
        KeyAction::Quit
    } else {
        command.map_or(KeyAction::Continue, KeyAction::send)
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

    // Numeric shortcuts: menus RENDER a "1".."9" column but had no dispatch
    // arm, so the advertised digit was dead — and in searchable menus it
    // corrupted the filter instead. Resolve the digit against the ACTIVE
    // (already search-filtered) spec and accept the item exactly like Enter.
    // Deliberately after slash-help capture (digits there are composer
    // filter/argument text) and skipped when no enabled item advertises the
    // digit, so it falls through to the existing search-capture behavior.
    if let KeyCode::Char(ch) = key.code
        // Unmodified digits only: some terminals report Shift+digit as
        // Char(digit)+SHIFT, and handle_key routes SHIFT through the
        // plain-key path — shifted input in a searchable menu must filter,
        // not fire the shortcut (codex P3).
        && key.modifiers.is_empty()
        && !slash_help_capture_active(store)
        && let Some(index) = active_menu_digit_shortcut_index(store, ch)
    {
        if let Some(frame) = store.state.menu_stack.active_mut() {
            frame.selected_index = index;
        }
        let command = store.accept_active_menu_item();
        if store.state.exit_requested {
            return KeyAction::Quit;
        }
        if let Some(command) = command {
            return KeyAction::send(command);
        }
        return KeyAction::Continue;
    }

    match key.code {
        // `@` file picker (#363): Esc closes WITHOUT inserting and removes the
        // auto-typed `@`, restoring the composer to its pre-`@` text.
        KeyCode::Esc if file_picker_menu_active(store) => {
            store.cancel_composer_file_picker();
        }
        KeyCode::Esc => {
            // Esc on the slash popup dismisses its composer draft too (the
            // token that opened/filtered it — codex's dismiss semantics).
            // Leaving "/the" behind made the NEXT `/` append into it instead
            // of reopening the popup, and read as a user draft to the settled
            // interrupt-restore's menu-close retry (which must never clobber
            // real text). Cleared BEFORE the close so that retry sees the
            // empty composer it needs.
            if slash_help_capture_active(store) {
                store.state.set_composer_text("");
            }
            // Esc closes/backs out of menus, EXCEPT the root onboarding wizard
            // step while onboarding is in progress: that menu is only auto-opened
            // on first launch, so closing it would strand the user (issue #5).
            store.handle_menu_escape();
        }
        KeyCode::Backspace if slash_help_capture_active(store) => {
            // Covers the bare `/` too (capture is active from the first char),
            // so Backspace can still delete an accidental slash — the
            // `menu_composer_edit_active` branch no longer handles it.
            store.state.delete_composer_prev_char();
            if store.state.composer.starts_with('/') {
                sync_slash_help_search_query(store);
            } else {
                // Backspaced away the bare `/`: the slash draft is gone, so
                // close the popup instead of leaving it open over an empty
                // composer.
                store.close_menu();
            }
        }
        KeyCode::Backspace if active_menu_search_has_query(store) => {
            delete_active_menu_search_prev_char(store);
        }
        // Backspacing past an EMPTY picker filter deletes the `@` that opened
        // it and closes the picker — the same dismiss the slash popup performs
        // when the bare `/` is backspaced away.
        KeyCode::Backspace if file_picker_menu_active(store) => {
            store.cancel_composer_file_picker();
        }
        KeyCode::Char(ch) if slash_help_should_capture_char(store, ch) => {
            store.state.insert_composer_char(ch);
            sync_slash_help_search_query(store);
        }
        KeyCode::Char(ch) if active_menu_should_capture_search_char(store, ch) => {
            append_active_menu_search_char(store, ch);
        }
        KeyCode::Enter => {
            if slash_help_query_active(store) && slash_help_enter_executes(store) {
                // Executing the slash draft: the popup's job is done — CLOSE
                // it before submitting. Leaving it open buried the command's
                // result surface (e.g. the /btw aside pane) under a stale
                // "No options available" box, which read as the command not
                // running at all (live-terminal bug).
                store.close_menu();
                return handle_composer_enter(store);
            }
            let command = store.accept_active_menu_item();
            if store.state.exit_requested {
                return KeyAction::Quit;
            }
            if let Some(command) = command {
                return KeyAction::send(command);
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
        // While the slash popup is open and the composer holds a slash draft
        // (even the bare `/`), keystrokes must flow into the menu's inline
        // filter — NOT the generic composer-edit branch. Gating on the broader
        // `slash_help_capture_active` (any length) rather than
        // `slash_help_query_active` (len > 1) is what removes the dead first
        // keystroke: the FIRST letter typed after `/` now reaches
        // `slash_help_should_capture_char` and syncs the search query, matching
        // codex's inline `/` behaviour.
        && !slash_help_capture_active(store)
        // The `@` file picker freezes the composer the same way: the draft
        // holds real prompt text (plus the `@` trigger), but every keystroke
        // belongs to the picker's search filter until it closes.
        && !file_picker_menu_active(store)
}

/// True while the `@` composer file picker (#363) is the active menu frame.
fn file_picker_menu_active(store: &Store) -> bool {
    store
        .state
        .menu_stack
        .active()
        .is_some_and(|frame| frame.id.as_str() == crate::menu::registry::MENU_FILE_PICKER)
}

/// True whenever the slash popup is open and the composer is a slash draft
/// (`/`, `/t`, `/theme`, ...). Used to route keystrokes into the inline filter
/// from the very first character — distinct from [`slash_help_query_active`],
/// which gates submit/backspace semantics and only fires once a query exists
/// (composer longer than the bare `/`).
fn slash_help_capture_active(store: &Store) -> bool {
    slash_help_menu_active(store) && store.state.composer.starts_with('/')
}

fn slash_help_query_active(store: &Store) -> bool {
    slash_help_capture_active(store) && store.state.composer.len() > 1
}

/// With the slash popup filtering, Enter EXECUTES the draft only when it
/// already names a command exactly (optionally with arguments) or matches
/// nothing at all; while the name is still a partial prefix, Enter falls
/// through to `accept_active_menu_item` and COMPLETES the highlighted command
/// into the composer (codex's two-step flow: pick, add arguments, run).
fn slash_help_enter_executes(store: &Store) -> bool {
    let draft = store.state.composer.trim();
    let Some(stripped) = draft.strip_prefix('/') else {
        return true;
    };
    if stripped.contains(char::is_whitespace) {
        // Arguments present: the command name part is final — run it.
        return true;
    }
    let registry = crate::menu::CommandRegistry::with_core_commands();
    if let crate::menu::CommandResolution::Found { command, .. } = registry.resolve(draft) {
        // Resolvable name with NO arguments typed yet: dispatch directly
        // unless the command REQUIRES arguments — bare dispatch of those is
        // only a usage error, so they route to the accept path and complete
        // as "/name " for argument typing (codex completes there too, via
        // Tab; Enter doubles as our completion key for required-arg drafts).
        return command.inline_args != crate::menu::types::InlineArgMode::Required;
    }
    // Partial name: execute only if the popup has nothing left to offer.
    store
        .state
        .active_menu
        .as_ref()
        .is_none_or(|menu| match menu {
            crate::menu::MenuBuildResult::Ready(spec) => spec.items.is_empty(),
            _ => true,
        })
}

fn slash_help_should_capture_char(store: &Store, ch: char) -> bool {
    // With a query already typed (`/t...`) every printable char is captured.
    // On the bare `/` we still reserve `j`/`k` for vim-style list navigation,
    // so a user who opened the popup but hasn't started filtering can move the
    // selection. Any other first letter starts the inline filter immediately.
    slash_help_capture_active(store) && (store.state.composer.len() > 1 || !matches!(ch, 'j' | 'k'))
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
        // Filter by the COMMAND TOKEN only (codex command_popup behavior):
        // once the user types arguments ("/btw what are you…"), matching the
        // whole draft against the registry yields "No options available" for
        // a perfectly valid command. The first token keeps the command
        // matched + highlighted while arguments are typed.
        let draft = store
            .state
            .composer
            .strip_prefix('/')
            .unwrap_or(store.state.composer.as_str());
        frame.search_query = draft.split_whitespace().next().unwrap_or("").to_string();
        frame.selected_index = 0;
    }
    store.refresh_active_menu();
}

fn active_menu_should_capture_search_char(store: &Store, ch: char) -> bool {
    active_menu_searchable(store)
        && (active_menu_search_has_query(store)
            // File names legitimately start with j/k (`justfile`,
            // `keymap.rs`): in the `@` picker every printable char filters
            // from the first keystroke; Up/Down still navigate.
            || file_picker_menu_active(store)
            || !matches!(ch, 'j' | 'k'))
}

/// Index of the ENABLED item in the active (search-filtered) menu spec whose
/// advertised shortcut is exactly the plain digit `ch` ('1'..='9'), or `None`
/// (not a digit / no menu / nothing advertises it) so the caller falls through
/// to search capture.
fn active_menu_digit_shortcut_index(store: &Store, ch: char) -> Option<usize> {
    if !ch.is_ascii_digit() || ch == '0' {
        return None;
    }
    let Some(crate::menu::MenuBuildResult::Ready(spec)) = store.state.active_menu.as_ref() else {
        return None;
    };
    spec.items.iter().position(|item| {
        item.is_enabled()
            && item.shortcut.as_ref().is_some_and(|binding| {
                binding.code == KeyCode::Char(ch) && binding.modifiers.is_empty()
            })
    })
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
    let mut buf = [0u8; 4];
    append_active_menu_search_text(store, ch.encode_utf8(&mut buf));
}

/// Append text to the active menu frame's search query (single refresh — a
/// paste must not rebuild the menu once per character). The query is a
/// single-line filter, so newlines flatten to spaces.
fn append_active_menu_search_text(store: &mut Store, text: &str) {
    if let Some(frame) = store.state.menu_stack.active_mut() {
        for ch in text.chars() {
            frame.search_query.push(if ch == '\n' { ' ' } else { ch });
        }
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
            // A parked decision: Esc INTERRUPTS the turn in ONE press (the user
            // traded the peek-behind-the-card affordance for a reliable escape),
            // mirroring Ctrl+C. The server-side interrupt drops the parked
            // approval waiter so no zombie decision is left. Falls back to hiding
            // the card only when there is no parked turn to interrupt.
            if let Some(command) = store.interrupt_active_decision_command() {
                return KeyAction::send(command);
            }
            store.close_modal();
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'y') => {
            if let Some(command) =
                store.respond_approval_command(ApprovalModalAction::ApproveRequest)
            {
                return KeyAction::send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'s') => {
            if let Some(command) =
                store.respond_approval_command(ApprovalModalAction::ApproveSession)
            {
                return KeyAction::send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'n') => {
            if let Some(command) = store.respond_approval_command(ApprovalModalAction::DenyRequest)
            {
                return KeyAction::send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'d') => {
            if let Some(command) = store.read_diff_preview_command() {
                return KeyAction::send(command);
            }
        }
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'v') && store.state.diff_preview.active => {
            store.toggle_diff_view_mode();
        }
        _ => {}
    }

    KeyAction::Continue
}

/// UPCR-2026-023: drive the AskUserQuestion picker. Keyboard model mirrors the
/// multi-select menu (Up/Down move, Space toggle) plus typing-to-Other and
/// Enter to step questions / submit. Esc INTERRUPTS the parked turn in one press
/// (mirroring Ctrl+C), cancelling the question rather than merely hiding it.
fn handle_user_question_key(store: &mut Store, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => {
            // One-press interrupt of the turn parked on this question; the
            // server-side interrupt drops the parked question waiter. Falls back
            // to hiding only when there is no parked turn to interrupt.
            if let Some(command) = store.interrupt_active_decision_command() {
                return KeyAction::send(command);
            }
            store.close_modal();
        }
        KeyCode::Up => store.user_question_cursor_up(),
        KeyCode::Down => store.user_question_cursor_down(),
        KeyCode::Char(' ') if !store.user_question_editing_free_text() => {
            store.user_question_toggle();
        }
        KeyCode::Char(']') => store.user_question_cursor_down(),
        KeyCode::Char('[') => store.user_question_cursor_up(),
        KeyCode::Tab => {
            // Step forward through questions without submitting.
            store.user_question_advance();
        }
        KeyCode::BackTab => store.user_question_back(),
        KeyCode::Backspace => store.user_question_pop_free_text(),
        KeyCode::Enter => {
            // Navigate + Enter must CHOOSE the highlighted option (only Space
            // toggled before, so an arrow-key highlight + Enter submitted an
            // EMPTY answer `[{}]` and the model saw no selection). Refuse to
            // advance/submit a question with no answer so a stray Enter on an
            // empty free-text row can never send an empty answer either — the
            // user picks an option or types, then Enter proceeds.
            if !store.user_question_commit_highlight() {
                store.state.status = t!("status.question_needs_answer").into_owned();
                return KeyAction::Continue;
            }
            // Stepping through questions; submit only on the final one.
            if store.user_question_advance()
                && let Some(command) = store.respond_user_question_command()
            {
                return KeyAction::send(command);
            }
        }
        KeyCode::Char(ch) => {
            // Any other character is captured into the free-text "Other" box.
            store.user_question_push_free_text(ch);
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
                return KeyAction::send(command);
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

fn move_down(store: &mut Store) {
    match store.state.focus {
        FocusPane::Sessions => {
            store.state.select_next_session();
            // A direct switch restores the incoming session's staged queue;
            // drain it (as a follow-up command) or a staged prompt sits stuck
            // until an unrelated turn event (codex round-2 P2).
            store.drain_staged_after_direct_switch();
        }
        FocusPane::Tasks => store.state.select_next_task(),
        FocusPane::Artifacts => store.state.select_next_artifact(),
        FocusPane::Workspace => store.state.select_next_workspace_entry(),
        FocusPane::Git => store.state.select_next_git_entry(),
        FocusPane::Transcript | FocusPane::Composer => store.state.scroll_transcript_down(1),
    }
}

fn move_up(store: &mut Store) {
    match store.state.focus {
        FocusPane::Sessions => {
            store.state.select_prev_session();
            store.drain_staged_after_direct_switch();
        }
        FocusPane::Tasks => store.state.select_prev_task(),
        FocusPane::Artifacts => store.state.select_prev_artifact(),
        FocusPane::Workspace => store.state.select_prev_workspace_entry(),
        FocusPane::Git => store.state.select_prev_git_entry(),
        FocusPane::Transcript | FocusPane::Composer => store.state.scroll_transcript_up(1),
    }
}

/// The transcript pager opens only from the plain chat flow. Any full-screen
/// surface (inspector, onboarding, detail modals — covered by
/// `wants_fullscreen_overlay`), an open menu, or a visible approval/question
/// modal keeps the toggle inert so Ctrl+T can't rip the screen out from under
/// an interaction that owns the keyboard.
pub fn transcript_pager_available(state: &AppState) -> bool {
    !app::wants_fullscreen_overlay(state)
        && !state.menu_stack.is_active()
        && !state
            .approval
            .as_ref()
            .is_some_and(|approval| approval.visible)
        && !state
            .user_question
            .as_ref()
            .is_some_and(|picker| picker.visible)
}

fn toggle_transcript_pager(store: &mut Store) {
    if store.state.transcript_pager_active {
        store.state.exit_transcript_pager();
    } else if transcript_pager_available(&store.state) {
        store.state.enter_transcript_pager();
    }
}

fn scroll_current_surface_down(store: &mut Store, lines: usize) {
    // See `scroll_current_surface_up` for the surface precedence.
    if app::agent_view_active(&store.state) {
        store.state.scroll_agent_view_down(lines);
        return;
    }
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
    // Mirrors `scroll_current_surface_up`: /btw sits under the detail modals but
    // over the focused pane.
    if btw_aside_open(&store.state) {
        scroll_open_btw_aside(store, lines as i32);
        return;
    }

    match store.state.focus {
        FocusPane::Workspace => store.state.workspace.scroll_down(lines),
        FocusPane::Git => store.state.git.scroll_down(lines),
        // Pinned scroll-mode: wheeling down past the pager bottom drops back
        // to the inline tail-following view, completing the "the composer is
        // always pinned, the wheel just moves the content" illusion. A pager
        // opened manually in native mode keeps its position instead.
        _ => {
            if store.state.pinned_scroll
                && store.state.transcript_pager_active
                && store.state.transcript_scroll == 0
            {
                store.state.exit_transcript_pager();
            } else {
                store.state.scroll_transcript_down(lines);
            }
        }
    }
}

fn scroll_current_surface_up(store: &mut Store, lines: usize) {
    // A live peek is a full-screen overlay above everything, so it takes the
    // wheel first. When a real modal is up the peek yields (agent_view_active is
    // false) and the surface precedence below applies: detail modals (rendered on
    // top) beat a /btw aside, which beats the focused pane.
    if app::agent_view_active(&store.state) {
        store.state.scroll_agent_view_up(lines);
        return;
    }
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
    // The /btw aside sits under any detail modal (which renders on top of it) but
    // over the focused pane, so it takes the wheel after the modals, before focus.
    if btw_aside_open(&store.state) {
        scroll_open_btw_aside(store, -(lines as i32));
        return;
    }

    match store.state.focus {
        FocusPane::Workspace => store.state.workspace.scroll_up(lines),
        FocusPane::Git => store.state.git.scroll_up(lines),
        // Pinned scroll-mode: the first wheel-up in the chat flow opens the
        // pager at the bottom (committed history is unreachable inline), the
        // wheel then scrolls inside it. Native mode keeps the wheel on the
        // terminal, so this arm only sees synthetic/test events there.
        _ => {
            if store.state.pinned_scroll && transcript_pager_available(&store.state) {
                store.state.enter_transcript_pager();
            } else {
                store.state.scroll_transcript_up(lines);
            }
        }
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

/// Tracks the current screen model and restores terminal state on drop.
struct TerminalGuard {
    mode: RenderMode,
    saved_inline_viewport: Option<ratatui::layout::Rect>,
    saved_visible_history_extent: Option<(u16, u16)>,
    /// Screen size the INLINE layout was last laid out for, saved on entering
    /// the alt screen. The overlay draw path consumes resizes by updating
    /// `last_known_screen_size` itself, so without restoring this on leave a
    /// resize that happened while the overlay was up would be invisible to
    /// the inline flow — `resize_viewport_to` would take the incremental path
    /// against a stale viewport while the emulator had already rewrapped the
    /// hidden normal screen (reflow ghosts).
    saved_inline_screen_size: Option<ratatui::layout::Size>,
    /// Mouse capture is on ONLY while the transcript pager is up (so the wheel
    /// scrolls the pager). It must never be on in the inline chat flow, where
    /// it would defeat native terminal selection/copy.
    mouse_captured: bool,
}

impl TerminalGuard {
    /// Bring the terminal's mouse-capture state in line with the policy
    /// (`app::wants_mouse_capture`). Idempotent: only writes the escape
    /// sequence on an actual transition.
    fn sync_mouse_capture<B>(&mut self, terminal: &mut InlineTerminal<B>, want: bool) -> Result<()>
    where
        B: Backend + io::Write,
    {
        if want == self.mouse_captured {
            return Ok(());
        }
        if want {
            execute!(terminal.backend_mut(), EnableMouseCapture)?;
        } else {
            execute!(terminal.backend_mut(), DisableMouseCapture)?;
        }
        self.mouse_captured = want;
        Ok(())
    }

    /// Switch into the alternate screen for a full-screen overlay (if not
    /// already there). The viewport is resized to the full screen by the caller.
    fn enter_alt_screen<B>(&mut self, terminal: &mut InlineTerminal<B>) -> Result<()>
    where
        B: Backend + io::Write,
    {
        if self.mode == RenderMode::AltScreen {
            return Ok(());
        }
        self.saved_inline_viewport = Some(terminal.viewport_area);
        self.saved_visible_history_extent = Some((
            terminal.visible_history_rows(),
            terminal.visible_history_bottom(),
        ));
        // Save BEFORE the overwrite below: this is the size the inline layout
        // was laid out for, restored by `leave_alt_screen` so the next inline
        // draw can detect any resize that happened while the overlay was up.
        self.saved_inline_screen_size = Some(terminal.last_known_screen_size);
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
        let size = terminal.size()?;
        terminal.set_viewport_area(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        terminal.clear_visible_screen()?;
        terminal.invalidate_viewport();
        terminal.last_known_screen_size = size;
        self.mode = RenderMode::AltScreen;
        Ok(())
    }

    /// Return to the inline-viewport model (normal scrollback). On the way back
    /// we force the next inline draw to repaint the whole viewport.
    fn leave_alt_screen<B>(&mut self, terminal: &mut InlineTerminal<B>) -> Result<()>
    where
        B: Backend + io::Write,
    {
        if self.mode == RenderMode::Inline {
            return Ok(());
        }
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        let fallback = {
            let size = terminal.size()?;
            ratatui::layout::Rect::new(0, size.height.saturating_sub(1), size.width, 1)
        };
        let saved_visible_history_extent = self.saved_visible_history_extent.take();
        terminal.set_viewport_area(self.saved_inline_viewport.take().unwrap_or(fallback));
        if let Some((rows, bottom)) = saved_visible_history_extent {
            terminal.set_visible_history_extent(rows, bottom);
        }
        // Restore the inline-era screen size so `resize_viewport_to` compares
        // the real size against what the inline layout last saw, not against
        // whatever the overlay draw path recorded while consuming a resize on
        // the alt screen. A size that changed anywhere across the overlay
        // round-trip then takes the full-reset path (the emulator rewrapped
        // the hidden normal screen); an unchanged size stays a cheap
        // invalidate-only repaint exactly as before.
        if let Some(size) = self.saved_inline_screen_size.take() {
            terminal.last_known_screen_size = size;
        }
        terminal.invalidate_viewport();
        self.mode = RenderMode::Inline;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        #[cfg(not(test))]
        {
            let mut stdout = io::stdout();
            if self.mouse_captured {
                let _ = execute!(stdout, DisableMouseCapture);
            }
            if self.mode == RenderMode::AltScreen {
                let _ = execute!(stdout, LeaveAlternateScreen);
            }
            let _ = disable_raw_mode();
            let _ = execute!(stdout, DisableBracketedPaste, DisableFocusChange, Show);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActivityKind, AppState, LiveReply, SessionView, TaskView};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use octos_core::{
        Message, SessionKey, TaskId,
        ui_protocol::{
            ApprovalDecision, ApprovalId, ApprovalRequestedEvent, PreviewId, QuestionId,
            TaskRuntimeState, TurnId, UiNotification, UiProtocolCapabilities, UserQuestion,
            UserQuestionOption, UserQuestionRequestedEvent, approval_scopes,
        },
    };
    use ratatui::{
        backend::{ClearType, WindowSize},
        layout::{Position, Rect, Size},
    };
    use std::collections::VecDeque;
    use std::io::Write;

    fn sent_command(action: KeyAction) -> AppUiCommand {
        let KeyAction::Send(command) = action else {
            panic!("expected command action");
        };
        *command
    }

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

    struct RecordingBackend {
        buf: Vec<u8>,
        size: Size,
        cursor: Position,
        clears: Vec<ClearType>,
    }

    impl RecordingBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                buf: Vec::new(),
                size: Size::new(width, height),
                cursor: Position { x: 0, y: 0 },
                clears: Vec::new(),
            }
        }
    }

    impl Write for RecordingBackend {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for RecordingBackend {
        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.cursor = position.into();
            Ok(())
        }

        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            self.clears.push(clear_type);
            Ok(())
        }

        fn size(&self) -> io::Result<Size> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize {
                columns_rows: self.size,
                pixels: Size::new(0, 0),
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
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

    fn sample_agent_record(
        session_id: &SessionKey,
        id: &str,
    ) -> octos_core::ui_protocol::UiAgentRecord {
        octos_core::ui_protocol::UiAgentRecord {
            agent_id: id.into(),
            parent_agent_id: None,
            session_id: session_id.clone(),
            task_id: None,
            path: "/root".into(),
            role: "worker".into(),
            nickname: id.into(),
            title: None,
            backend_kind: "native".into(),
            status: "running".into(),
            last_task: None,
            summary: None,
            output_tail: None,
            cwd: None,
            profile_id: "coding".into(),
            runtime_policy_stamp: None,
            artifact_count: 0,
            artifacts: vec![],
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    fn sample_goal(objective: &str) -> octos_core::ui_protocol::UiGoalRecord {
        octos_core::ui_protocol::UiGoalRecord {
            profile_id: Some("coding".into()),
            goal_id: "goal_01".into(),
            objective: objective.into(),
            status: "active".into(),
            token_budget: 2_000_000,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn ctrl_p_toggles_goal_objective_fold_when_goal_present() {
        use crate::model::GoalObjectiveFold;
        let mut store = store_with_sessions(1);
        let sid = store.state.active_session().unwrap().id.clone();
        store.state.set_session_goal(
            &sid,
            Some(sample_goal("shader code …")),
            Some("user".into()),
        );

        // Simulate the banner having rendered FOLDED (Auto → long objective).
        store.state.goal_objective_folded_effective.set(true);
        handle_key(
            &mut store,
            modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            store.state.goal_objective_fold,
            GoalObjectiveFold::Unfolded,
            "Ctrl+P on a folded goal expands it",
        );

        // Simulate it now rendered UNFOLDED; Ctrl+P re-folds.
        store.state.goal_objective_folded_effective.set(false);
        handle_key(
            &mut store,
            modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            store.state.goal_objective_fold,
            GoalObjectiveFold::Folded,
            "Ctrl+P on an unfolded goal re-folds it",
        );
    }

    #[test]
    fn ctrl_p_is_a_noop_without_a_goal() {
        use crate::model::GoalObjectiveFold;
        let mut store = store_with_sessions(1);
        // No goal on the active session — Ctrl+P must not claim the key or
        // mutate the fold preference.
        handle_key(
            &mut store,
            modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            store.state.goal_objective_fold,
            GoalObjectiveFold::Auto,
            "Ctrl+P without a goal leaves the fold preference untouched",
        );
    }

    #[test]
    fn tab_cycles_chat_view_between_main_and_agents() {
        use crate::model::ChatViewTarget;
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        let sid = store.state.active_session().unwrap().id.clone();
        store
            .state
            .upsert_session_agent(&sid, sample_agent_record(&sid, "ag-1"));
        store
            .state
            .upsert_session_agent(&sid, sample_agent_record(&sid, "ag-2"));

        assert_eq!(store.state.chat_view, ChatViewTarget::Main);

        // Tab cycles forward through [main, ag-1, ag-2] and wraps.
        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Agent("ag-1".into()));
        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Agent("ag-2".into()));
        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Main);

        // Shift+Tab (BackTab) steps backward.
        handle_key(&mut store, key(KeyCode::BackTab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Agent("ag-2".into()));

        // Tab drives the switcher, not focus: the composer keeps focus and its
        // text is untouched by the view switch.
        assert_eq!(store.state.composer, "");
        assert_eq!(store.state.focus, FocusPane::Composer);
    }

    #[test]
    fn peeking_agent_swallows_composer_text_keys() {
        use crate::model::ChatViewTarget;
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        let sid = store.state.active_session().unwrap().id.clone();
        store
            .state
            .upsert_session_agent(&sid, sample_agent_record(&sid, "ag-1"));

        // Enter the agent peek, then type: the hidden composer must not change.
        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Agent("ag-1".into()));
        handle_key(&mut store, key(KeyCode::Char('x')));
        handle_key(&mut store, key(KeyCode::Char('y')));
        assert_eq!(store.state.composer, "");

        // Esc backs out to the main chat, where typing works again.
        handle_key(&mut store, key(KeyCode::Esc));
        assert_eq!(store.state.chat_view, ChatViewTarget::Main);
        handle_key(&mut store, key(KeyCode::Char('z')));
        assert_eq!(store.state.composer, "z");
    }

    /// Open the `@` file picker over a fixed in-memory listing (no fs scan):
    /// composer holds "see @" (the trigger `@` last), the picker state is set,
    /// and the picker menu is the active frame — the exact state
    /// `handle_composer_char_input('@')` produces.
    fn open_file_picker(store: &mut Store) {
        store.state.set_composer_text("see @");
        store.state.file_picker = Some(crate::file_picker::FilePickerState {
            root: "ws".into(),
            files: vec!["a.rs".into()],
            truncated: false,
        });
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_FILE_PICKER,
        ));
    }

    #[test]
    fn modified_keys_leave_composer_frozen_while_file_picker_open() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        open_file_picker(&mut store);
        assert_eq!(store.state.composer, "see @");

        // #363 review: Ctrl+W must NOT reach the hidden draft while the
        // picker owns the keyboard — it would eat the trigger `@` (and the
        // word before it) and break Esc's exact-restore contract.
        handle_key(
            &mut store,
            modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            store.state.composer, "see @",
            "composer is frozen while the picker owns the keyboard"
        );

        // Sanity: with the picker closed the same key edits the draft again —
        // proving the freeze above came from the picker gate specifically.
        store.cancel_composer_file_picker();
        assert_eq!(store.state.composer, "see ");
        handle_key(
            &mut store,
            modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_ne!(store.state.composer, "see ");
    }

    #[test]
    fn esc_cancels_shell_escape_draft_before_interrupting_turn() {
        // #364 review: arm precedence — Esc on a `!` draft discards the draft
        // and must NEVER fall through to the interrupt arm, even mid-turn.
        let mut store = store_with_live_reply_text("working…");
        store.state.focus = FocusPane::Composer;
        assert!(
            store.state.active_turn().is_some(),
            "precondition: a turn is live, so plain Esc WOULD interrupt"
        );
        store.state.set_composer_text("!ls -la");

        let action = handle_key(&mut store, key(KeyCode::Esc));
        assert!(
            matches!(action, KeyAction::Continue),
            "first Esc cancels the draft without sending anything"
        );
        assert_eq!(store.state.composer, "");
        assert!(!store.shell_escape_mode_active());
    }

    #[test]
    fn esc_closes_file_picker_without_interrupting_turn() {
        // #363 review: arm precedence — Esc with the picker open cancels the
        // picker (restoring the pre-`@` draft) and never reaches the
        // interrupt arm.
        let mut store = store_with_live_reply_text("working…");
        store.state.focus = FocusPane::Composer;
        open_file_picker(&mut store);

        let action = handle_key(&mut store, key(KeyCode::Esc));
        assert!(matches!(action, KeyAction::Continue));
        assert!(store.state.file_picker.is_none(), "picker state cleared");
        assert_eq!(
            store.state.composer, "see ",
            "trigger `@` removed; draft restored to its pre-`@` text"
        );
        assert!(!store.state.menu_stack.is_active(), "picker frame closed");
    }

    #[test]
    fn peek_owns_keyboard_swallows_ctrl_u_and_scrolls_its_own_offset() {
        use crate::model::ChatViewTarget;
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer = "keep me".into();
        let sid = store.state.active_session().unwrap().id.clone();
        store
            .state
            .upsert_session_agent(&sid, sample_agent_record(&sid, "ag-1"));

        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Agent("ag-1".into()));

        // Ctrl+U runs BEFORE the old guard in handle_key; the peek must own it so
        // it can't clear the hidden draft.
        handle_key(
            &mut store,
            modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert_eq!(store.state.composer, "keep me");

        // Up scrolls the peek's OWN offset, leaving the main transcript scroll be.
        handle_key(&mut store, key(KeyCode::Up));
        assert_eq!(store.state.agent_view_scroll, 1);
        assert_eq!(store.state.transcript_scroll, 0);
    }

    #[test]
    fn tab_in_pager_does_not_enter_peek() {
        use crate::model::ChatViewTarget;
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        let sid = store.state.active_session().unwrap().id.clone();
        store
            .state
            .upsert_session_agent(&sid, sample_agent_record(&sid, "ag-1"));
        store.state.transcript_pager_active = true;

        // A peek is only entered from the inline chat; Tab in the pager is a
        // no-op (so leaving states never strands the app in alt-screen).
        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Main);
    }

    #[test]
    fn stale_agent_selection_keeps_composer_editable() {
        use crate::model::ChatViewTarget;
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        // Selection points at an agent absent from the roster: `agent_view_active`
        // is false, so the visible inline composer must remain editable.
        store.state.chat_view = ChatViewTarget::Agent("ghost".into());

        handle_key(&mut store, key(KeyCode::Char('z')));
        assert_eq!(store.state.composer, "z");
    }

    fn store_with_live_reply_text(text: &str) -> Store {
        let session = SessionView {
            id: SessionKey("local:test".into()),
            title: "test".into(),
            profile_id: Some("coding".into()),
            messages: vec![],
            tasks: vec![],
            live_reply: Some(LiveReply {
                turn_id: TurnId::new(),
                text: text.into(),
            }),
        };
        Store {
            state: AppState::new(vec![session], 0, "ready".into(), None, false),
        }
    }

    // --- command-history Up/Down recall wiring (crate::history) ---

    fn composer_store_with_history(entries: &[&str]) -> Store {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.composer_history = crate::history::ComposerHistory::from_entries(
            entries.iter().map(|s| s.to_string()).collect(),
        );
        store
    }

    #[test]
    fn esc_dismisses_slash_popup_draft_and_applies_settled_restore() {
        // Esc on the slash popup dismisses the WHOLE popup including its
        // composer draft (codex's dismiss semantics). Leaving "/" behind made
        // the next `/` append ("//", no popup) and blocked the settled
        // interrupt-restore's menu-close retry (the hook must never clobber a
        // real draft, and "/" read as one).
        let mut store = store_with_live_reply_text("streaming");
        let session_id = store.state.sessions[0].id.clone();
        let turn_id = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("live turn")
            .turn_id
            .clone();
        store.state.record_submitted_user_prompt(
            session_id.clone(),
            turn_id.clone(),
            "the interrupted prompt".into(),
        );
        store.state.focus = FocusPane::Composer;

        // Esc interrupts (arms the deferred restore), `/` opens the popup.
        handle_key(&mut store, key(KeyCode::Esc));
        handle_key(&mut store, key(KeyCode::Char('/')));
        assert!(store.state.menu_stack.is_active());
        assert_eq!(store.state.composer, "/");

        // The turn settles while the popup is open: restore defers again.
        store.apply_event(AppUiEvent::Protocol(
            octos_core::ui_protocol::UiNotification::TurnError(
                octos_core::ui_protocol::TurnErrorEvent {
                    session_id,
                    topic: None,
                    turn_id,
                    code: "interrupted".into(),
                    message: "turn interrupted by client".into(),
                },
            ),
        ));
        assert!(store.state.menu_stack.is_active(), "popup stays up");

        // Esc dismisses the popup AND its slash draft; the settled restore
        // then lands in the now-empty composer.
        handle_key(&mut store, key(KeyCode::Esc));
        assert!(!store.state.menu_stack.is_active());
        assert_eq!(
            store.state.composer, "the interrupted prompt",
            "dismissing the popup hands the interrupted prompt back"
        );
    }

    #[test]
    fn slash_popup_opens_mid_turn_after_esc_interrupt() {
        // The reported flow: a long turn is streaming, the status line coaches
        // "Esc interrupt | /stop to close". The user presses Esc, the turn
        // keeps streaming (interrupts are async — or the turn is wedged), then
        // they type `/`. The popup must open: the interrupt must NOT have
        // filled the composer behind their back while the turn is still live.
        let mut store = store_with_live_reply_text("streaming");
        let session_id = store.state.sessions[0].id.clone();
        let turn_id = store.state.sessions[0]
            .live_reply
            .as_ref()
            .expect("live turn")
            .turn_id
            .clone();
        store.state.record_submitted_user_prompt(
            session_id,
            turn_id,
            "do a full code review pls".into(),
        );
        store.state.focus = FocusPane::Composer;

        let action = handle_key(&mut store, key(KeyCode::Esc));
        assert!(
            matches!(action, KeyAction::Send(_)),
            "Esc interrupts the active turn"
        );
        assert!(
            store.state.composer.is_empty(),
            "the composer stays empty while the interrupted turn still streams"
        );

        handle_key(&mut store, key(KeyCode::Char('/')));
        assert!(
            store.state.menu_stack.is_active(),
            "`/` must open the slash popup mid-turn after an Esc interrupt"
        );
        assert_eq!(store.state.composer, "/");
    }

    #[test]
    fn up_from_empty_composer_recalls_newest_then_older() {
        let mut store = composer_store_with_history(&["older", "newest"]);
        handle_key(&mut store, key(KeyCode::Up));
        assert_eq!(store.state.composer, "newest");
        handle_key(&mut store, key(KeyCode::Up));
        assert_eq!(store.state.composer, "older");
    }

    #[test]
    fn up_from_nonempty_composer_does_not_recall() {
        let mut store = composer_store_with_history(&["entry"]);
        store.state.set_composer_text("typed draft");
        handle_key(&mut store, key(KeyCode::Up));
        // Gate: recall only starts from an empty composer; the draft is intact.
        assert_eq!(store.state.composer, "typed draft");
    }

    #[test]
    fn down_past_newest_history_returns_to_empty_draft() {
        let mut store = composer_store_with_history(&["a", "b"]);
        handle_key(&mut store, key(KeyCode::Up)); // → "b" (newest)
        assert_eq!(store.state.composer, "b");
        handle_key(&mut store, key(KeyCode::Down)); // past newest → empty draft
        assert_eq!(store.state.composer, "");
    }

    #[test]
    fn up_steps_history_through_multiline_recalled_entries() {
        let mut store = composer_store_with_history(&["older", "line1\nline2"]);
        handle_key(&mut store, key(KeyCode::Up)); // recall newest (multiline)
        assert_eq!(store.state.composer, "line1\nline2");
        // Up again steps to the OLDER entry rather than moving the cursor up a
        // line inside the recalled multiline draft.
        handle_key(&mut store, key(KeyCode::Up));
        assert_eq!(store.state.composer, "older");
    }

    #[test]
    fn clearing_composer_resets_history_navigation() {
        let mut store = composer_store_with_history(&["older", "newest"]);
        handle_key(&mut store, key(KeyCode::Up)); // recall "newest"
        assert_eq!(store.state.composer, "newest");
        assert!(store.state.composer_history.is_navigating());
        store.state.clear_current_composer_draft(); // Ctrl+U path
        assert!(!store.state.composer_history.is_navigating());
        // Up after clear recalls the newest again — not a stale 2-press no-op.
        handle_key(&mut store, key(KeyCode::Up));
        assert_eq!(store.state.composer, "newest");
    }

    #[test]
    fn accepted_plain_prompt_is_recorded() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.set_composer_text("hello there");
        let _ = store.compose_command();
        assert_eq!(store.state.composer_history.entries(), &["hello there"]);
    }

    #[test]
    fn readonly_rejected_prompt_is_not_recorded() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.readonly = true;
        store.state.set_composer_text("never sent");
        let _ = store.compose_command();
        // A prompt rejected by the readonly guard must not reach history.
        assert!(store.state.composer_history.entries().is_empty());
    }

    #[test]
    fn accepted_no_arg_slash_is_recorded() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        // `/theme` is `always()`-available and history-safe; it opens the theme
        // menu (a pure-client `Accepted(None)`), so it must be recorded even
        // though it emits no backend command.
        store.state.set_composer_text("/theme");
        let command = store.compose_command();
        assert!(
            command.is_none(),
            "/theme opens a menu and sends no backend command"
        );
        assert_eq!(store.state.composer_history.entries(), &["/theme"]);
    }

    #[test]
    fn bare_loop_in_readonly_is_not_recorded() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.readonly = true;
        // Bare `/loop` is history-safe and registry-available (it is
        // READ-gated, so readonly does not hide it), but the dispatcher rejects
        // it before it runs (here: unavailable in this mock/disconnected store;
        // in a live readonly session it is the `require_mutating_appui_method`
        // block — see `store::tests::bare_loop_in_readonly_mutating_is_not_recorded`).
        // Either way a rejected command must NOT persist (the regression fix).
        store.state.set_composer_text("/loop");
        let _ = store.compose_command();
        assert!(
            store.state.composer_history.entries().is_empty(),
            "a rejected bare /loop must not reach history"
        );
    }

    fn onboarding_capabilities_event() -> ClientEvent {
        ClientEvent::Capabilities(crate::client_event::CapabilitiesClientEvent {
            result: crate::model::ConfigCapabilitiesListResult {
                capabilities: UiProtocolCapabilities::new(
                    &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
                    &[],
                ),
            },
            message: "Octos UI capabilities refreshed: 1 method".into(),
        })
    }

    #[test]
    fn ctrl_y_stages_last_reply_for_clipboard() {
        let mut store = store_with_live_reply_text("answer to copy");

        let action = handle_key(
            &mut store,
            modified_key(KeyCode::Char('y'), KeyModifiers::CONTROL),
        );

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(
            store.state.pending_clipboard.as_deref(),
            Some("answer to copy")
        );
    }

    #[test]
    fn flush_pending_clipboard_writes_osc52_and_clears_the_request() {
        // Drain into an in-memory sink (NOT real stdout) so `cargo test` can't
        // emit OSC 52 to the developer's terminal and overwrite their clipboard
        // (codex P2). This also lets us assert the exact bytes written.
        let mut store = store_with_live_reply_text("answer");
        store.state.pending_clipboard = Some("answer".into());

        let mut sink: Vec<u8> = Vec::new();
        flush_pending_clipboard_to(&mut store, &mut sink);

        // The one-shot field is drained so a copy cannot re-fire on every tick.
        assert!(store.state.pending_clipboard.is_none());
        // And the OSC 52 sequence for "answer" was written to the sink.
        assert_eq!(
            String::from_utf8(sink).unwrap(),
            crate::clipboard::osc52_copy_sequence("answer")
        );
    }

    #[test]
    fn flush_pending_clipboard_writes_nothing_when_unset() {
        let mut store = store_with_live_reply_text("answer");
        store.state.pending_clipboard = None;

        let mut sink: Vec<u8> = Vec::new();
        flush_pending_clipboard_to(&mut store, &mut sink);

        assert!(
            sink.is_empty(),
            "no OSC 52 should be emitted with nothing staged"
        );
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

    fn store_with_visible_user_question() -> (Store, QuestionId) {
        let mut store = store_with_sessions(1);
        let question_id = QuestionId::new();

        store.apply_event(AppUiEvent::Protocol(UiNotification::UserQuestionRequested(
            UserQuestionRequestedEvent::new(
                store.state.sessions[0].id.clone(),
                question_id.clone(),
                TurnId::new(),
                "Pick a framework",
                "The agent needs a framework choice to proceed.",
                vec![UserQuestion {
                    header: "Framework".into(),
                    question: "Which web framework?".into(),
                    options: vec![
                        UserQuestionOption {
                            label: "axum".into(),
                            description: "tokio-native".into(),
                        },
                        UserQuestionOption {
                            label: "actix".into(),
                            description: "actor-based".into(),
                        },
                    ],
                    multi_select: false,
                    allow_free_text: true,
                }],
            ),
        )));

        (store, question_id)
    }

    #[test]
    fn esc_hidden_user_question_can_be_reopened_via_recovery_key() {
        // DO-NOT-SHIP #1: an accidental Esc hides the picker; the recovery key
        // (Alt+a) must re-open it just like a hidden approval, route keys back
        // to the picker, and let the user submit a valid response.
        let (mut store, question_id) = store_with_visible_user_question();

        // Esc hides the picker without answering it.
        store.close_modal();
        assert!(
            !store
                .state
                .user_question
                .as_ref()
                .expect("question still pending")
                .visible
        );
        assert!(!store.state.user_question_auto_open);

        // The recovery key re-opens the hidden question.
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
                .user_question
                .as_ref()
                .expect("question still pending")
                .visible
        );
        assert!(store.state.user_question_auto_open);
        assert_eq!(store.state.focus, FocusPane::Composer);

        // Keys route to the picker again: select an option, then Enter submits a
        // valid response.
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char(' '))),
            KeyAction::Continue
        ));
        let action = handle_key(&mut store, key(KeyCode::Enter));
        let AppUiCommand::RespondUserQuestion(params) = sent_command(action) else {
            panic!("expected RespondUserQuestion command");
        };
        assert_eq!(params.question_id, question_id);
        assert_eq!(params.answers.len(), 1);
        assert_eq!(params.answers[0].selected_labels, vec!["axum".to_string()]);
        assert!(store.state.user_question.is_none());
    }

    #[test]
    fn esc_on_a_parked_approval_interrupts_the_turn_in_one_press() {
        // Fix 2: a turn parked on an approval can be waiting BEFORE any reply
        // streams, so `active_turn()` (which needs a `live_reply`) is None and the
        // plain `interrupt_command()` no-ops. Esc must STILL interrupt the parked
        // turn — not merely hide the card (the old two-step trap) — by building
        // the interrupt from the decision's own turn id.
        let (mut store, _approval_id) = store_with_visible_approval();
        let expected_session = store.state.sessions[0].id.clone();
        let expected_turn = store
            .state
            .approval
            .as_ref()
            .expect("approval pending")
            .turn_id
            .clone();
        assert!(
            store.state.active_turn().is_none(),
            "no live_reply: the interrupt must come from the decision's turn id"
        );

        let action = handle_key(&mut store, key(KeyCode::Esc));

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("Esc on a parked approval must issue an interrupt, not hide the card");
        };
        assert_eq!(params.session_id, expected_session);
        assert_eq!(params.turn_id, expected_turn);
        // The card is NOT silently hidden into the pending-but-invisible limbo: it
        // stays until the server-side cancel lands, exactly like Ctrl+C.
        assert!(
            store.state.approval.as_ref().is_some_and(|a| a.visible),
            "interrupt must not hide the card the way the old Esc did"
        );
    }

    #[test]
    fn esc_on_a_parked_question_interrupts_the_turn_in_one_press() {
        // Same one-press interrupt for a parked AskUserQuestion picker.
        let (mut store, _question_id) = store_with_visible_user_question();
        let expected_turn = store
            .state
            .user_question
            .as_ref()
            .expect("question pending")
            .turn_id
            .clone();

        let action = handle_key(&mut store, key(KeyCode::Esc));

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("Esc on a parked question must issue an interrupt, not hide the picker");
        };
        assert_eq!(params.turn_id, expected_turn);
        assert!(
            store
                .state
                .user_question
                .as_ref()
                .is_some_and(|q| q.visible),
            "interrupt must not hide the picker the way the old Esc did"
        );
    }

    #[test]
    fn ctrl_c_interrupts_a_parked_decision_even_without_a_live_reply() {
        // The old Ctrl+C path called `interrupt_command()`, which no-ops when a
        // decision parks a turn before any reply streams. Route it through the
        // decision-aware interrupt so Ctrl+C reliably cancels a parked turn.
        let (mut store, _approval_id) = store_with_visible_approval();
        assert!(store.state.active_turn().is_none());

        let action = handle_key(
            &mut store,
            modified_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(
            matches!(sent_command(action), AppUiCommand::InterruptTurn(_)),
            "Ctrl+C on a parked decision must interrupt, not no-op"
        );
    }

    #[test]
    fn enter_selects_the_highlighted_option_without_space() {
        // mini5 "llm questions can not get user inputs": navigating to an option
        // and pressing Enter (the natural confirm) previously submitted an EMPTY
        // answer `[{}]` because only Space toggled a selection. A bare Enter must
        // now CHOOSE the highlighted option. Cursor starts on "axum"; move to
        // "actix" and press Enter with no Space.
        let (mut store, question_id) = store_with_visible_user_question();

        handle_key(&mut store, key(KeyCode::Down)); // highlight "actix"
        let action = handle_key(&mut store, key(KeyCode::Enter));
        let AppUiCommand::RespondUserQuestion(params) = sent_command(action) else {
            panic!("expected RespondUserQuestion command");
        };
        assert_eq!(params.question_id, question_id);
        assert_eq!(params.answers.len(), 1);
        assert_eq!(
            params.answers[0].selected_labels,
            vec!["actix".to_string()],
            "bare Enter must select the highlighted option, not send an empty answer"
        );
        assert!(store.state.user_question.is_none());
    }

    #[test]
    fn enter_on_empty_free_text_row_refuses_to_submit_an_empty_answer() {
        // The other half of the fix: a stray Enter on the empty "Other" free-text
        // row must NOT submit an empty answer — the picker stays open with a hint
        // so the model never receives `answers:[{}]`.
        let (mut store, _question_id) = store_with_visible_user_question();

        // Move to the free-text "Other" row (index == options.len()).
        handle_key(&mut store, key(KeyCode::Down)); // actix
        handle_key(&mut store, key(KeyCode::Down)); // Other (free-text row)

        let action = handle_key(&mut store, key(KeyCode::Enter));
        assert!(
            matches!(action, KeyAction::Continue),
            "Enter with no answer must not send a command"
        );
        assert!(
            store.state.user_question.is_some(),
            "the picker must stay open until an answer is given"
        );
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
    fn startup_capabilities_render_onboarding_before_first_frame() {
        let mut backend = FakeBackend::new(vec![onboarding_capabilities_event()]);
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "starting".into(),
                Some("stdio:octos serve --stdio --solo".into()),
                false,
            ),
        };

        let applied =
            drain_initial_startup_events(&mut backend, &mut store).expect("startup drain");

        assert!(applied);
        assert!(app::wants_fullscreen_overlay(&store.state));

        let mut terminal =
            InlineTerminal::new(RecordingBackend::new(80, 24)).expect("recording terminal");
        let mut guard = TerminalGuard {
            mode: RenderMode::Inline,
            saved_inline_viewport: None,
            saved_visible_history_extent: None,
            saved_inline_screen_size: None,
            mouse_captured: false,
        };
        let mut scrollback = ScrollbackTracker::new();
        draw(
            &mut terminal,
            &mut guard,
            &mut store,
            &mut scrollback,
            false,
        )
        .expect("draw onboarding overlay");

        let written = String::from_utf8_lossy(&terminal.backend().buf);
        assert!(
            written.contains("Welcome to Octos"),
            "onboarding should render before the first frame; wrote {written:?}"
        );
    }

    #[test]
    fn active_turn_completed_activity_is_history_only_in_raw_draw_bytes() {
        let turn_id = TurnId::new();
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: SessionKey("local:test".into()),
                    title: "test".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("run the checks")],
                    tasks: vec![],
                    live_reply: Some(LiveReply {
                        turn_id: turn_id.clone(),
                        text: "Still working".into(),
                    }),
                }],
                0,
                "Thinking".into(),
                None,
                false,
            ),
        };
        store.state.set_run_state_in_progress();
        store.state.push_activity(
            crate::model::ActivityItem::new(ActivityKind::Tool, "shell", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-running")
                .with_detail("cargo clippy --all-targets"),
        );
        store.state.push_activity(
            crate::model::ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id)
                .with_tool_call("call-complete")
                .with_detail("cargo test")
                .with_success(true),
        );

        let mut terminal =
            InlineTerminal::new(RecordingBackend::new(100, 30)).expect("recording terminal");
        let mut guard = TerminalGuard {
            mode: RenderMode::Inline,
            saved_inline_viewport: None,
            saved_visible_history_extent: None,
            saved_inline_screen_size: None,
            mouse_captured: false,
        };
        let mut scrollback = ScrollbackTracker::new();

        draw(
            &mut terminal,
            &mut guard,
            &mut store,
            &mut scrollback,
            false,
        )
        .expect("draw active inline frame");

        let written = String::from_utf8_lossy(&terminal.backend().buf);
        assert_eq!(
            written.matches("cargo test").count(),
            1,
            "completed activity should be emitted once via history insertion, not repainted in the live viewport: {written:?}"
        );
        assert!(
            written.contains("cargo clippy --all-targets"),
            "running activity should remain in the live viewport bytes: {written:?}"
        );
    }

    #[test]
    fn menu_close_frame_clears_and_reflushes_committed_transcript() {
        // A slash menu is a reserved viewport row block: opening it grows the
        // inline viewport and scrolls the committed transcript up; closing it
        // shrinks the viewport and, without this fix, strands that transcript
        // high on screen above a `menu_height` blank band. The close frame must
        // therefore behave like a resize — clear the visible screen and re-flush
        // the whole committed transcript flush against the shrunk viewport.
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: SessionKey("local:test".into()),
                    title: "test".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![
                        Message::user("please answer"),
                        Message::assistant("reply zztranscriptmarker done"),
                    ],
                    tasks: vec![],
                    live_reply: None,
                }],
                0,
                "Idle".into(),
                None,
                false,
            ),
        };

        let mut terminal =
            InlineTerminal::new(RecordingBackend::new(100, 30)).expect("recording terminal");
        let mut guard = TerminalGuard {
            mode: RenderMode::Inline,
            saved_inline_viewport: None,
            saved_visible_history_extent: None,
            saved_inline_screen_size: None,
            mouse_captured: false,
        };
        let mut scrollback = ScrollbackTracker::new();

        // First draw flushes the committed transcript into scrollback.
        draw(
            &mut terminal,
            &mut guard,
            &mut store,
            &mut scrollback,
            false,
        )
        .expect("first draw");
        assert!(
            String::from_utf8_lossy(&terminal.backend().buf).contains("zztranscriptmarker"),
            "first draw flushes the committed transcript to scrollback"
        );

        // A steady-state redraw does NOT re-emit already-flushed committed
        // history (it lives in scrollback, not the live viewport).
        let mark = terminal.backend().buf.len();
        draw(
            &mut terminal,
            &mut guard,
            &mut store,
            &mut scrollback,
            false,
        )
        .expect("steady redraw");
        let steady = String::from_utf8_lossy(&terminal.backend().buf[mark..]).into_owned();
        assert!(
            !steady.contains("zztranscriptmarker"),
            "a steady redraw must not re-flush committed history: {steady:?}"
        );

        // The menu-close frame clears the visible screen and re-flushes the
        // whole committed transcript so no stranded copy / blank band survives.
        let mark = terminal.backend().buf.len();
        let clears_before = terminal.backend().clears.len();
        draw(&mut terminal, &mut guard, &mut store, &mut scrollback, true)
            .expect("menu-close draw");
        let close = String::from_utf8_lossy(&terminal.backend().buf[mark..]).into_owned();
        assert!(
            close.contains("zztranscriptmarker"),
            "the menu-close frame must re-flush the committed transcript: {close:?}"
        );
        assert!(
            terminal.backend().clears[clears_before..].contains(&ClearType::All),
            "the menu-close frame must clear the visible screen like a resize"
        );
    }

    #[test]
    fn menu_close_frame_does_not_duplicate_orchestrating_chip_during_live_turn() {
        // Guard for the "two Orchestrating chips on menu-close" report. The
        // hypothesis was that `mark_flushed_stale()` on the menu-close edge wipes
        // the live-turn watermark and makes `finalized_live_turn_lines_between`
        // RE-EMIT the "Orchestrating…" chip into scrollback (a frozen second
        // copy). This test pins that this does NOT happen: the scrollback flush
        // never emits the chip, because
        //   1. `next_live_turn_finalization` only flushes NON-running activity
        //      (app.rs: `!is_running_activity(item)`), so a running spawn tool
        //      call is never in the flushed set, and
        //   2. `push_finalized_activity_items_section` forces `is_active_group =
        //      false` + empty `subagent_titles`, so `agent_task_group_title`
        //      resolves to "completed"/"finished with errors", never
        //      "Orchestrating" (app.rs `push_agent_task_group`).
        // The live viewport renders exactly one chip; the menu-close frame's
        // emitted bytes therefore contain "Orchestrating" exactly once.
        //
        // NOTE: the ACTUAL reported duplicate (two chips one spinner-frame apart,
        // the upper one missing its child row) is a TERMINAL-RETAINED / stranded
        // row, not a re-emitted one — different spinner frames prove the frozen
        // copy was painted on an earlier frame and kept by the emulator, not
        // written twice this frame. The `RecordingBackend` here records only the
        // bytes we EMIT (it models neither a screen grid nor real scrollback —
        // see insert_history.rs "no vt100 dep"), so it cannot reproduce a
        // stranded-row artifact; this test only rules the scrollback-flush path
        // in/out as the cause.
        let turn_id = TurnId::new();
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: SessionKey("local:test".into()),
                    title: "test".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("run a review")],
                    tasks: vec![],
                    live_reply: Some(LiveReply {
                        turn_id: turn_id.clone(),
                        text: String::new(),
                    }),
                }],
                0,
                "Thinking".into(),
                None,
                false,
            ),
        };
        store.state.set_run_state_in_progress();
        // A running spawn tool call under the active turn → the live render emits
        // an "Orchestrating… (1 action(s) · 1 active · turn …)" chip with a
        // running Spawn child.
        store.state.push_activity(
            crate::model::ActivityItem::new(ActivityKind::Tool, "spawn", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-spawn-running")
                .with_detail("Do a SECURITY + CORRECTNESS review of the module"),
        );

        let mut terminal =
            InlineTerminal::new(RecordingBackend::new(100, 30)).expect("recording terminal");
        let mut guard = TerminalGuard {
            mode: RenderMode::Inline,
            saved_inline_viewport: None,
            saved_visible_history_extent: None,
            saved_inline_screen_size: None,
            mouse_captured: false,
        };
        let mut scrollback = ScrollbackTracker::new();

        // Steady draw: establishes the live-turn watermark and paints the single
        // live "Orchestrating…" chip in the viewport.
        draw(
            &mut terminal,
            &mut guard,
            &mut store,
            &mut scrollback,
            false,
        )
        .expect("steady draw");

        // The menu-close frame: clear_visible_screen + mark_flushed_stale +
        // re-flush. Inspect ONLY the bytes this frame wrote.
        let mark = terminal.backend().buf.len();
        draw(&mut terminal, &mut guard, &mut store, &mut scrollback, true)
            .expect("menu-close draw");
        let close = String::from_utf8_lossy(&terminal.backend().buf[mark..]).into_owned();

        assert_eq!(
            close.matches("Orchestrating").count(),
            1,
            "the menu-close frame must render the Orchestrating chip exactly once \
             (a second copy is the frozen/duplicated chip): {close:?}"
        );

        // Directly assert the flush-level invariant the hypothesis was about:
        // after `mark_flushed_stale`, the reflushed scrollback lines must NOT
        // contain the "Orchestrating…" chip (it belongs to the live viewport
        // only). This is independent of the byte-emit quirks above.
        let palette = crate::theme::Palette::for_theme(store.state.theme);
        let mut probe = crate::viewport::ScrollbackTracker::new();
        let _ = probe.sync(&store.state, palette, 100);
        probe.mark_flushed_stale();
        let reflushed = probe.sync(&store.state, palette, 100);
        let flushed_text = reflushed
            .lines_to_insert
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(
            flushed_text.matches("Orchestrating").count(),
            0,
            "the menu-close scrollback flush must not re-emit the Orchestrating \
             chip (running activity is never finalized into scrollback): \
             {flushed_text:?}"
        );
    }

    #[test]
    fn overlay_transition_restores_inline_viewport_for_resize_clear() {
        let mut terminal =
            InlineTerminal::new(RecordingBackend::new(80, 24)).expect("recording terminal");
        terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        terminal.set_visible_history_extent(3, 20);
        terminal.last_known_screen_size = Size::new(80, 24);
        let mut guard = TerminalGuard {
            mode: RenderMode::Inline,
            saved_inline_viewport: None,
            saved_visible_history_extent: None,
            saved_inline_screen_size: None,
            mouse_captured: false,
        };

        guard
            .enter_alt_screen(&mut terminal)
            .expect("enter alt screen");
        assert_eq!(guard.mode, RenderMode::AltScreen);
        assert_eq!(terminal.viewport_area, Rect::new(0, 0, 80, 24));
        assert_eq!(terminal.backend().clears, vec![ClearType::All]);

        terminal.backend_mut().size = Size::new(60, 20);
        guard
            .leave_alt_screen(&mut terminal)
            .expect("leave alt screen");
        assert_eq!(guard.mode, RenderMode::Inline);
        assert_eq!(terminal.viewport_area, Rect::new(0, 20, 80, 4));
        assert_eq!(terminal.visible_history_rows(), 3);
        assert_eq!(terminal.visible_history_bottom(), 20);

        terminal
            .resize_viewport_to(4)
            .expect("resize restored inline viewport");
        assert_eq!(terminal.viewport_area, Rect::new(0, 16, 60, 4));
        // The screen was resized (80x24 -> 60x20) while the overlay was up,
        // so the restore takes the width-change full-reset path: whole-screen
        // clear from the origin and a dropped visible-history extent (the
        // emulator rewrapped the inline screen behind the alt screen).
        assert_eq!(terminal.backend().cursor, Position { x: 0, y: 0 });
        assert_eq!(
            terminal.backend().clears,
            vec![ClearType::All, ClearType::All]
        );
        assert_eq!(terminal.visible_history_rows(), 0);
        assert_eq!(terminal.visible_history_bottom(), 0);

        let written = String::from_utf8_lossy(&terminal.backend().buf);
        assert!(written.contains("\u{1b}[?1049h"));
        assert!(written.contains("\u{1b}[?1049l"));
    }

    #[test]
    fn overlay_resize_consumed_by_alt_screen_still_full_resets_inline() {
        // codex finding on the resize-ghost fix: a resize handled WHILE the
        // overlay was up updates `last_known_screen_size` on the alt screen
        // (the `draw()` overlay path), so the inline restore no longer sees a
        // size delta — yet the emulator rewrapped the hidden NORMAL screen.
        // `leave_alt_screen` must restore the screen size the INLINE layout
        // was last laid out for, so the next inline draw still takes the
        // width-change full-reset path.
        let mut terminal =
            InlineTerminal::new(RecordingBackend::new(80, 24)).expect("recording terminal");
        terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        terminal.set_visible_history_extent(3, 20);
        terminal.last_known_screen_size = Size::new(80, 24);
        let mut guard = TerminalGuard {
            mode: RenderMode::Inline,
            saved_inline_viewport: None,
            saved_visible_history_extent: None,
            saved_inline_screen_size: None,
            mouse_captured: false,
        };

        guard
            .enter_alt_screen(&mut terminal)
            .expect("enter alt screen");
        // Simulate the overlay draw's resize handling (event_loop::draw):
        // the screen narrows while the overlay is up and the overlay path
        // consumes the delta by updating last_known_screen_size itself.
        terminal.backend_mut().size = Size::new(60, 24);
        terminal.set_viewport_area(Rect::new(0, 0, 60, 24));
        terminal
            .clear_visible_screen()
            .expect("overlay resize clear");
        terminal.invalidate_viewport();
        terminal.last_known_screen_size = Size::new(60, 24);

        guard
            .leave_alt_screen(&mut terminal)
            .expect("leave alt screen");
        terminal
            .resize_viewport_to(4)
            .expect("resize restored inline viewport");

        // Width changed 80 -> 60 relative to the inline layout: full reset.
        assert_eq!(terminal.viewport_area, Rect::new(0, 20, 60, 4));
        assert_eq!(
            terminal.backend().clears.last(),
            Some(&ClearType::All),
            "inline restore after an overlay-consumed resize must full-clear; clears: {:?}",
            terminal.backend().clears
        );
        assert_eq!(terminal.backend().cursor, Position { x: 0, y: 0 });
        assert_eq!(terminal.visible_history_rows(), 0);
        assert_eq!(terminal.visible_history_bottom(), 0);
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
    fn terminal_paste_while_approval_modal_visible_is_dropped_with_status() {
        // Fix #2 (c): while the approval modal owns the keyboard, a paste used
        // to land invisibly in the force-focused composer. It must be dropped
        // with a visible status — and must never dispatch approval shortcuts
        // ('y'/'n') either.
        let (mut store, _) = store_with_visible_approval();

        assert!(matches!(
            handle_terminal_event(&mut store, Event::Paste("y\n/safe".into())),
            KeyAction::Continue
        ));

        assert!(
            store.state.composer.is_empty(),
            "paste must not mutate the composer hidden under the modal: {:?}",
            store.state.composer
        );
        assert!(
            store
                .state
                .approval
                .as_ref()
                .is_some_and(|modal| modal.visible),
            "the pasted 'y' must not answer the approval"
        );
        assert!(
            store.state.status.contains("dialog"),
            "dropping a paste needs a visible status, got: {:?}",
            store.state.status
        );
    }

    #[test]
    fn terminal_paste_lands_in_question_free_text_when_editing() {
        // Fix #2 (a): with the AskUserQuestion free-text "Other" box active,
        // a paste belongs there (mirroring the plain-char capture path), not
        // in the composer hidden underneath.
        let (mut store, _) = store_with_visible_user_question();
        handle_key(&mut store, key(KeyCode::Char('m')));
        assert!(store.user_question_editing_free_text());

        handle_terminal_event(&mut store, Event::Paste("y paste\ntail".into()));

        let entry = &store
            .state
            .user_question
            .as_ref()
            .expect("picker")
            .questions[0];
        assert_eq!(
            entry.free_text, "my paste tail",
            "paste appends to the free-text box (newlines flattened)"
        );
        assert!(
            store.state.composer.is_empty(),
            "composer must not receive the paste"
        );
    }

    #[test]
    fn terminal_paste_while_question_modal_visible_without_free_text_is_dropped() {
        let (mut store, _) = store_with_visible_user_question();
        assert!(!store.user_question_editing_free_text());

        handle_terminal_event(&mut store, Event::Paste("stray".into()));

        assert!(store.state.composer.is_empty());
        let entry = &store
            .state
            .user_question
            .as_ref()
            .expect("picker")
            .questions[0];
        assert!(entry.free_text.is_empty());
        assert!(store.state.status.contains("dialog"));
    }

    #[test]
    fn terminal_paste_appends_to_open_searchable_menu_filter() {
        // Fix #2 (b): with a searchable menu open, a paste extends the menu's
        // search query (the same target the plain-char path feeds), not the
        // composer hidden behind the menu.
        let mut store = store_with_sessions(1);
        store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_THEME));

        handle_terminal_event(&mut store, Event::Paste("sol".into()));

        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .expect("menu open")
                .search_query,
            "sol"
        );
        assert!(store.state.composer.is_empty());
    }

    #[test]
    fn terminal_paste_starting_with_slash_does_not_open_slash_menu() {
        // A paste is literal text: pasting "/permissions" inserts it verbatim and
        // must NOT open the slash-command menu (only a TYPED leading '/' does).
        let mut store = store_with_sessions(1);

        assert!(matches!(
            handle_terminal_event(&mut store, Event::Paste("/permissions".into())),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, "/permissions");
        assert!(!store.state.menu_stack.is_active());
    }

    #[test]
    fn terminal_paste_multiline_path_is_literal_not_slash_command() {
        // Regression: pasting a file path / multi-line snippet beginning with '/'
        // (e.g. shell output) must be inserted verbatim and must NOT open or run a
        // slash command.
        let mut store = store_with_sessions(1);
        let pasted = "/Users/cloud/enable_screenshare.sh\nPassword: secret";

        assert!(matches!(
            handle_terminal_event(&mut store, Event::Paste(pasted.into())),
            KeyAction::Continue
        ));

        assert_eq!(store.state.composer, pasted);
        assert!(!store.state.menu_stack.is_active());
    }

    #[test]
    fn terminal_paste_sanitizes_styled_web_text_to_plain() {
        // Web copies carry ANSI escapes, zero-width/format chars, CR, tabs. The
        // composer is plain-text: a paste must be reduced to renderable plain text
        // so the byte-cursor stays aligned with the render (no residue on delete).
        let mut store = store_with_sessions(1);
        let styled = "AAA\u{1b}[31mRED\u{1b}[0m\u{200b}XYZ\r\n\tEND";
        handle_terminal_event(&mut store, Event::Paste(styled.into()));
        assert_eq!(store.state.composer, "AAAREDXYZ\n END");
    }

    #[test]
    fn terminal_paste_strips_html_invisible_format_chars_keeping_cjk() {
        // Real-world repro: copying CJK from a styled web page interleaves the
        // visible text with invisible format codepoints (LRM, ZWSP, soft hyphen,
        // variation selector, bidi embedding, word-joiner, BOM, isolate-pop). The
        // old 5-char strip-set missed most of these, so they stayed in the buffer
        // and desynced the byte-cursor from the width render — the text rendered
        // mostly blank with only the tail surviving. All must reduce to plain CJK.
        let mut store = store_with_sessions(1);
        let styled =
            "\u{200e}五\u{200b}大\u{00ad}拉\u{fe0f}格\u{202a}朗\u{2060}日\u{feff}点\u{2069}";
        handle_terminal_event(&mut store, Event::Paste(styled.into()));
        assert_eq!(store.state.composer, "五大拉格朗日点");
    }

    #[test]
    fn terminal_paste_maps_unicode_line_and_paragraph_separators_to_newline() {
        // U+2028 / U+2029 are not ASCII control chars; left raw they render oddly
        // in the plain-text composer. They mean "line break" — map them to '\n'.
        let mut store = store_with_sessions(1);
        handle_terminal_event(&mut store, Event::Paste("a\u{2028}b\u{2029}c".into()));
        assert_eq!(store.state.composer, "a\nb\nc");
    }

    #[test]
    fn terminal_paste_strips_8bit_c1_csi_and_osc_sequences() {
        // Some sources emit the 8-bit C1 forms of CSI (U+009B) / OSC (U+009D)
        // instead of ESC[ / ESC]. The whole sequence must drop, not just the
        // introducer (else the params/text like "31m…" leak into the composer).
        let mut store = store_with_sessions(1);
        handle_terminal_event(
            &mut store,
            Event::Paste("\u{9b}31mred\u{9d}0;t\u{7}END".into()),
        );
        assert_eq!(store.state.composer, "redEND");
    }

    #[test]
    fn ctrl_u_while_approval_modal_visible_keeps_staged_messages_and_draft() {
        // Fix #3: the global Ctrl+U ran before overlay dispatch, so it cleared
        // staged messages / the composer draft invisibly while a modal owned
        // the keyboard (the approval modal force-focuses the composer).
        let (mut store, _) = store_with_visible_approval();
        store.state.pending_messages.push("staged".into());
        store.state.set_composer_text("draft");

        let action = handle_key(
            &mut store,
            modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(store.state.pending_messages, vec!["staged".to_string()]);
        assert_eq!(store.state.composer, "draft");
    }

    #[test]
    fn composer_modified_keys_do_not_edit_hidden_composer_while_modal_visible() {
        // Fix #3: Ctrl+W (delete word) and friends must not mutate the hidden
        // composer while the approval modal owns the keyboard.
        let (mut store, _) = store_with_visible_approval();
        store.state.set_composer_text("two words");
        store.state.focus = FocusPane::Composer;

        handle_key(
            &mut store,
            modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );

        assert_eq!(store.state.composer, "two words");
    }

    #[test]
    fn ctrl_u_without_modal_still_clears_staged_messages() {
        let mut store = store_with_sessions(1);
        store.state.pending_messages.push("staged".into());

        let action = handle_key(
            &mut store,
            modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );

        assert!(matches!(action, KeyAction::Continue));
        assert!(store.state.pending_messages.is_empty());
    }

    #[test]
    fn menu_digit_shortcut_dispatches_matching_item() {
        // Fix #4: menus render "1".."9" numeric shortcuts but had no dispatch
        // arm. The digit must select + accept the advertised item exactly like
        // Enter ('5' -> the theme menu's 5th item, "solarized").
        let mut store = store_with_sessions(1);
        store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_THEME));

        let action = handle_key(&mut store, key(KeyCode::Char('5')));

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(store.state.theme, crate::cli::ThemeName::Solarized);
    }

    #[test]
    fn menu_digit_without_matching_shortcut_still_types_into_search() {
        // The theme menu advertises only "1".."5"; '9' matches no item, so it
        // must keep the existing searchable-menu capture behavior.
        let mut store = store_with_sessions(1);
        store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_THEME));

        let action = handle_key(&mut store, key(KeyCode::Char('9')));

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .expect("menu open")
                .search_query,
            "9"
        );
        assert_eq!(store.state.theme, crate::cli::ThemeName::default());
    }

    #[test]
    fn slash_popup_digit_stays_in_composer_draft() {
        // In the slash popup the composer is a command line where digits are
        // legitimate filter/argument text; the help menu's advertised numeric
        // shortcuts must NOT hijack them.
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        handle_key(&mut store, key(KeyCode::Char('/')));
        assert!(store.state.menu_stack.is_active());

        handle_key(&mut store, key(KeyCode::Char('1')));

        assert_eq!(store.state.composer, "/1");
    }

    #[test]
    fn typing_slash_then_a_letter_filters_help_menu_without_dead_keystroke() {
        // Regression: opening the slash popup with `/` and typing the FIRST
        // letter must filter the menu immediately (single keystroke), matching
        // codex's inline slash behaviour. Previously the first letter after `/`
        // landed in the composer-edit branch without syncing the search query,
        // so the menu only began filtering on the SECOND keystroke.
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('/'))),
            KeyAction::Continue
        ));
        assert!(store.state.menu_stack.is_active(), "slash popup opened");

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('t'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.composer, "/t");
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .expect("slash menu")
                .search_query,
            "t",
            "first letter after / must filter the menu immediately"
        );
    }

    #[test]
    fn backspace_on_bare_slash_deletes_it_and_closes_popup() {
        // Regression (codex review of the single-keystroke slash fix): with only
        // `/` in the composer and the popup open, Backspace must still delete the
        // accidental slash — and close the popup — rather than no-op. The fix
        // moved the bare `/` out of the composer-edit branch, so the slash
        // Backspace handler has to cover it.
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        handle_key(&mut store, key(KeyCode::Char('/')));
        handle_key(&mut store, key(KeyCode::Char('t')));
        assert_eq!(store.state.composer, "/t");
        assert!(store.state.menu_stack.is_active());

        // `/t` -> `/`: popup stays open over the bare slash.
        handle_key(&mut store, key(KeyCode::Backspace));
        assert_eq!(store.state.composer, "/");
        assert!(
            store.state.menu_stack.is_active(),
            "popup stays open on `/`"
        );

        // `/` -> empty: the slash draft is gone, so the popup closes.
        handle_key(&mut store, key(KeyCode::Backspace));
        assert_eq!(store.state.composer, "");
        assert!(
            !store.state.menu_stack.is_active(),
            "deleting the bare / closes the slash popup"
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
    fn mouse_wheel_over_a_peek_scrolls_the_agent_view_not_the_transcript() {
        use crate::model::ChatViewTarget;
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        let sid = store.state.active_session().unwrap().id.clone();
        store
            .state
            .upsert_session_agent(&sid, sample_agent_record(&sid, "ag-1"));
        handle_key(&mut store, key(KeyCode::Tab));
        assert_eq!(store.state.chat_view, ChatViewTarget::Agent("ag-1".into()));

        // A live peek is the top of the surface precedence, so the wheel drives
        // its own from-bottom offset and leaves the hidden transcript untouched.
        let wheel = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 10,
                row: 5,
                modifiers: KeyModifiers::empty(),
            })
        };
        handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollUp));
        assert_eq!(store.state.agent_view_scroll, 4);
        assert_eq!(store.state.transcript_scroll, 0);
        handle_terminal_event(&mut store, wheel(MouseEventKind::ScrollDown));
        assert_eq!(store.state.agent_view_scroll, 0);
        assert_eq!(store.state.transcript_scroll, 0);
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
    fn jk_navigation_covers_m9_panes_without_stealing_composer_text() {
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

        // The side panel is no longer reachable via Tab (Tab drives the sub-agent
        // switcher now), but its panes still navigate with j/k when focused
        // directly — set focus per pane and exercise the movement.
        store.state.focus = FocusPane::Artifacts;
        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('j'))),
            KeyAction::Continue
        ));
        assert_eq!(store.state.artifacts.selected, 1);

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
        store.state.composer = "draft one".into();

        // Tab now drives the sub-agent switcher rather than side-panel focus;
        // reach the Sessions pane directly to exercise draft preservation.
        store.state.focus = FocusPane::Sessions;
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
        assert_eq!(store.state.status, "Expanded tool output + diff");

        assert!(matches!(
            handle_key(
                &mut store,
                modified_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            ),
            KeyAction::Continue
        ));
        assert!(!store.state.expanded_tool_outputs);
        assert_eq!(store.state.status, "Collapsed tool output + diff");
    }

    /// Codex-style dialog dismissal: Enter on an EMPTY composer closes the
    /// `/btw` aside pane; Enter with text still submits (and dismisses via the
    /// submit path). Mirrors codex's bottom-pane "Enter to close" convention.
    #[test]
    fn empty_composer_enter_dismisses_btw_aside() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        let session_id = store.state.sessions[0].id.clone();
        store.state.set_btw_answering(&session_id, "quick q".into());
        assert!(store.state.btw_asides.contains_key(&session_id));

        assert!(matches!(
            handle_key(&mut store, modified_key(KeyCode::Enter, KeyModifiers::NONE)),
            KeyAction::Continue
        ));
        assert!(
            !store.state.btw_asides.contains_key(&session_id),
            "empty-composer Enter must close the aside pane"
        );

        // Without an aside, empty Enter stays a no-op (no send, no crash).
        assert!(matches!(
            handle_key(&mut store, modified_key(KeyCode::Enter, KeyModifiers::NONE)),
            KeyAction::Continue
        ));
    }

    /// Codex Enter semantics: Enter on a highlighted ARGUMENT-LESS command
    /// dispatches it immediately — one Enter from partial name to the
    /// command's page (here `/them` → the theme menu), never the old
    /// complete-into-composer + second-Enter round trip. Argful commands
    /// complete with a trailing space instead (args must be typed anyway;
    /// the next Enter executes the draft directly).
    #[test]
    fn slash_popup_enter_dispatches_selection_like_codex() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        for ch in "/them".chars() {
            handle_key(&mut store, key(KeyCode::Char(ch)));
        }
        handle_key(&mut store, key(KeyCode::Enter));
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_THEME),
            "one Enter on a partial argless command must open its page"
        );
        assert!(
            store.state.composer.is_empty(),
            "dispatch must not leave the completed name in the composer"
        );

        // Optional-arg command: bare dispatch is valid (opens its page) —
        // one Enter must go straight there too, composer cleared.
        store.close_menu();
        store.state.set_composer_text("");
        for ch in "/lang".chars() {
            handle_key(&mut store, key(KeyCode::Char(ch)));
        }
        handle_key(&mut store, key(KeyCode::Enter));
        assert!(
            store.state.composer.is_empty(),
            "optional-arg command dispatches bare, composer cleared"
        );
        assert_ne!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.id.as_str()),
            Some(crate::menu::registry::MENU_HELP),
            "dispatch must leave the help popup (command page or closed)"
        );
    }

    /// Live-terminal bug: typing `/btw <args>` filtered the registry with the
    /// WHOLE draft (args included) — "No options available" — and Enter
    /// executed the draft but left the stale popup open, burying the aside
    /// pane. The popup must (a) keep filtering by the command token only and
    /// (b) close when Enter executes the draft.
    #[test]
    fn slash_draft_with_args_keeps_match_and_enter_closes_menu() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;

        for ch in "/btw what are you doing".chars() {
            handle_key(&mut store, key(KeyCode::Char(ch)));
        }
        assert_eq!(store.state.composer, "/btw what are you doing");
        // (a) the filter uses the command token, so /btw stays matched.
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.search_query.as_str()),
            Some("btw"),
            "popup must filter by the command token, not the whole draft"
        );

        // (b) Enter executes the draft AND closes the popup.
        let action = handle_key(&mut store, key(KeyCode::Enter));
        assert!(
            store.state.menu_stack.active().is_none(),
            "executing the slash draft must close the popup"
        );
        assert!(
            !matches!(action, KeyAction::Quit),
            "draft execution must not quit"
        );
        assert!(
            store.state.composer.is_empty(),
            "submitting the draft must clear the composer"
        );
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
            turn_id: None,
        });
        store.state.sessions[0].tasks.push(TaskView {
            id: TaskId::new(),
            title: "format".into(),
            state: TaskRuntimeState::Completed,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: None,
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

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("expected InterruptTurn command");
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

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("expected InterruptTurn command");
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

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("expected InterruptTurn command");
        };
        assert_eq!(params.turn_id, turn_id);
        assert_eq!(
            store.state.status,
            "Interrupt requested; staged message will submit when the turn stops"
        );
    }

    #[test]
    fn esc_interrupts_active_turn_without_staged_messages() {
        let turn_id = TurnId::new();
        let mut store = store_with_sessions(1);
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: turn_id.clone(),
            text: "streaming".into(),
        });
        assert!(!store.state.has_pending_messages());

        let action = handle_key(&mut store, key(KeyCode::Esc));

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("expected InterruptTurn command");
        };
        assert_eq!(params.turn_id, turn_id);
        assert_eq!(store.state.status, "Interrupt requested for active turn");
    }

    #[test]
    fn esc_without_active_turn_or_cancellable_task_refocuses_composer() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Tasks;
        assert!(store.state.active_turn().is_none());
        // No tasks at all → nothing cancellable → Esc just refocuses.

        let action = handle_key(&mut store, key(KeyCode::Esc));

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(store.state.focus, FocusPane::Composer);
    }

    #[test]
    fn esc_cancels_running_background_task_even_when_selection_is_stale() {
        // Stale-selection regression (codex P2): the Tasks-pane selection sits
        // on an older COMPLETED task — `selected_task` defaults to 0 and is not
        // moved when newer tasks are appended (`apply_task_update` pushes) — but
        // a later spawn_only task is still running. Esc with no live turn must
        // cancel the RUNNING task, not no-op on the selected completed row
        // (which is the reported bug: Esc looked dead while orchestration ran).
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[octos_core::ui_protocol::methods::TASK_CANCEL],
            &[],
        ));
        // Index 0 (the selected row): already completed.
        store.state.sessions[0].tasks.push(TaskView {
            id: TaskId::new(),
            title: "done".into(),
            state: TaskRuntimeState::Completed,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: None,
        });
        // Index 1: the live background task the user wants to stop.
        let running_id = TaskId::new();
        store.state.sessions[0].tasks.push(TaskView {
            id: running_id.clone(),
            title: "deep_research".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: None,
        });
        assert_eq!(
            store.state.selected_task, 0,
            "selection sits on the completed task"
        );
        assert!(store.state.active_turn().is_none());

        let action = handle_key(&mut store, key(KeyCode::Esc));

        let AppUiCommand::CancelTask(params) = sent_command(action) else {
            panic!("expected CancelTask command for the running task");
        };
        assert_eq!(
            params.task_id, running_id,
            "must cancel the running task, not the selected completed one"
        );
    }

    #[test]
    fn vim_normal_esc_falls_through_to_interrupt_active_turn() {
        // Fix #1: with vim mode on and the composer in Normal mode, Esc used to
        // be swallowed unconditionally, so it could never interrupt a running
        // turn (mirror of `esc_interrupts_active_turn_without_staged_messages`).
        let turn_id = TurnId::new();
        let mut store = store_with_sessions(1);
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: turn_id.clone(),
            text: "streaming".into(),
        });
        store.state.vim_mode = true;
        store.state.composer_mode = crate::model::ComposerMode::Normal;
        store.state.focus = FocusPane::Composer;

        let action = handle_key(&mut store, key(KeyCode::Esc));

        let AppUiCommand::InterruptTurn(params) = sent_command(action) else {
            panic!("expected InterruptTurn command");
        };
        assert_eq!(params.turn_id, turn_id);
    }

    #[test]
    fn vim_normal_esc_with_pending_operator_clears_it_without_interrupting() {
        let mut store = store_with_sessions(1);
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: TurnId::new(),
            text: "streaming".into(),
        });
        store.state.vim_mode = true;
        store.state.composer_mode = crate::model::ComposerMode::Normal;
        store.state.focus = FocusPane::Composer;
        store.state.composer_vim_pending = Some('d');

        let action = handle_key(&mut store, key(KeyCode::Esc));

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(store.state.composer_vim_pending, None);
        assert!(
            store.state.active_turn().is_some(),
            "Esc with a pending operator only clears it; the turn keeps running"
        );
    }

    #[test]
    fn activity_navigator_enter_runs_the_full_session_switch_bundle() {
        // codex P1 (deep-review wave): the navigator Enter path assigned
        // `selected_session` directly, bypassing `switch_selected_session` —
        // the outgoing session's draft was never persisted (and with
        // per-session staged queues, the old session's staged prompts stayed
        // on the active queue, misdelivering into the picked session).
        let mut store = store_with_sessions(2);
        store.state.set_composer_text("draft for zero");
        // A task row is unambiguously linked to its owning session (activity
        // items without a turn also match the ACTIVE session via the
        // belongs-to fallback, which would make row 0 a self-switch).
        store.state.sessions[1].tasks.push(TaskView {
            id: TaskId::new(),
            title: "background probe".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: None,
        });
        store.state.activity_navigator.active = true;
        store.state.activity_navigator.selected = 0;

        handle_activity_navigator_key(&mut store, key(KeyCode::Enter));

        assert_eq!(
            store.state.selected_session, 1,
            "navigator pick lands on the target session"
        );
        assert!(
            store.state.composer.is_empty(),
            "outgoing draft must not bleed into the picked session"
        );
        // Switching back restores the stashed draft — proof the full bundle ran.
        store.state.switch_selected_session(0);
        assert_eq!(store.state.composer, "draft for zero");
    }

    #[test]
    fn shifted_digit_does_not_fire_menu_shortcut() {
        // codex P3: some terminals report Shift+digit as Char(digit)+SHIFT,
        // and handle_key routes SHIFT through the plain-key path — shifted
        // input in a searchable menu must go to the filter, not dispatch the
        // advertised numeric shortcut.
        let mut store = store_with_sessions(1);
        store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));
        assert!(store.state.active_menu.is_some(), "help menu open");
        let before = store
            .state
            .menu_stack
            .active()
            .map(|frame| frame.selected_index);

        let action = handle_key(
            &mut store,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SHIFT),
        );

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(
            store
                .state
                .menu_stack
                .active()
                .map(|frame| frame.selected_index),
            before,
            "shifted digit must not move/dispatch the shortcut selection"
        );
        assert!(
            store.state.active_menu.is_some(),
            "menu stays open (nothing dispatched)"
        );
    }

    #[test]
    fn sessions_pane_switch_back_drains_the_staged_queue() {
        // codex round-2 P2: switch_selected_session restores the incoming
        // session's staged queue, but the Sessions-pane Up/Down path never
        // drained it — the staged prompt sat stuck until an unrelated turn
        // event. The direct-switch paths now enqueue the drained submit on
        // the follow-up queue.
        let mut store = store_with_sessions(2);
        // Switch away FIRST (the outgoing bundle stashes session 0's — empty —
        // active queue), THEN seed session 0's stash: a staged prompt left
        // behind when its terminal fired while session 1 was active.
        store.state.switch_selected_session(1);
        store.state.pending_messages_by_session.insert(
            store.state.sessions[0].id.clone(),
            vec!["staged for zero".into()],
        );
        store.state.focus = FocusPane::Sessions;

        // Down wraps 1 -> 0 (two sessions): the direct switch back must drain.
        handle_key(&mut store, key(KeyCode::Down));

        assert_eq!(store.state.selected_session, 0);
        let follow_up = store
            .state
            .pending_autonomy_hydration
            .iter()
            .find_map(|command| match command {
                AppUiCommand::SubmitPrompt(params) => {
                    params.input.iter().find_map(|item| match item {
                        octos_core::ui_protocol::InputItem::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                }
                _ => None,
            });
        assert_eq!(
            follow_up.as_deref(),
            Some("staged for zero"),
            "the restored staged prompt must be submitted as a follow-up"
        );
        assert!(
            store.state.pending_messages.is_empty(),
            "the staged queue drained"
        );
    }

    #[test]
    fn vim_insert_esc_switches_to_normal_without_interrupting() {
        let mut store = store_with_sessions(1);
        store.state.sessions[0].live_reply = Some(LiveReply {
            turn_id: TurnId::new(),
            text: "streaming".into(),
        });
        store.state.vim_mode = true;
        store.state.composer_mode = crate::model::ComposerMode::Insert;
        store.state.focus = FocusPane::Composer;

        let action = handle_key(&mut store, key(KeyCode::Esc));

        assert!(matches!(action, KeyAction::Continue));
        assert_eq!(
            store.state.composer_mode,
            crate::model::ComposerMode::Normal
        );
        assert!(store.state.active_turn().is_some());
    }

    #[test]
    fn vim_normal_esc_exits_transcript_pager() {
        let mut store = store_with_sessions(1);
        store.state.vim_mode = true;
        store.state.composer_mode = crate::model::ComposerMode::Normal;
        store.state.focus = FocusPane::Composer;
        store.state.transcript_pager_active = true;

        let action = handle_key(&mut store, key(KeyCode::Esc));

        assert!(matches!(action, KeyAction::Continue));
        assert!(
            !store.state.transcript_pager_active,
            "Esc in vim Normal mode must still exit the transcript pager"
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
            turn_id: None,
        });

        let action = handle_key(&mut store, key(KeyCode::Char('d')));

        let AppUiCommand::GetDiffPreview(params) = sent_command(action) else {
            panic!("expected GetDiffPreview command");
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

    fn diff_result_with_two_hunks(session_id: SessionKey) -> crate::model::DiffPreviewGetResult {
        crate::model::DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: crate::model::DiffPreview {
                session_id,
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
        }
    }

    #[test]
    fn v_toggles_diff_view_round_trip_preserving_scroll_position() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Transcript;
        let session_id = store.state.sessions[0].id.clone();
        store
            .state
            .diff_preview
            .apply_result(diff_result_with_two_hunks(session_id));
        store.state.diff_preview.scroll = 7;
        store.state.diff_preview.selected_hunk = 1;

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('v'))),
            KeyAction::Continue
        ));
        assert!(store.state.diff_preview.side_by_side);
        assert_eq!(
            store.state.diff_preview.scroll, 7,
            "toggle must preserve scroll position"
        );
        assert_eq!(
            store.state.diff_preview.selected_hunk, 1,
            "toggle must preserve hunk selection"
        );

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('v'))),
            KeyAction::Continue
        ));
        assert!(
            !store.state.diff_preview.side_by_side,
            "second press round-trips back to unified"
        );
        assert_eq!(store.state.diff_preview.scroll, 7);
        assert_eq!(store.state.diff_preview.selected_hunk, 1);
    }

    #[test]
    fn v_toggle_disabled_when_terminal_too_narrow() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Transcript;
        let session_id = store.state.sessions[0].id.clone();
        store
            .state
            .diff_preview
            .apply_result(diff_result_with_two_hunks(session_id));
        store.state.last_terminal_width = 80;

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('v'))),
            KeyAction::Continue
        ));
        assert!(
            !store.state.diff_preview.side_by_side,
            "toggle is disabled below the side-by-side minimum width"
        );
    }

    #[test]
    fn v_types_into_composer_when_composer_focused() {
        let mut store = store_with_sessions(1);
        store.state.focus = FocusPane::Composer;
        store.state.diff_preview.open_loading(PreviewId::new());

        assert!(matches!(
            handle_key(&mut store, key(KeyCode::Char('v'))),
            KeyAction::Continue
        ));
        assert!(!store.state.diff_preview.side_by_side);
        assert_eq!(store.state.composer, "v");
    }

    #[test]
    fn inline_diff_preview_does_not_steal_pending_approval_keys() {
        let preview_id = PreviewId::new();
        let (mut store, approval_id) = store_with_visible_approval();
        store.state.diff_preview.open_loading(preview_id);

        let action = handle_key(&mut store, key(KeyCode::Char('y')));

        let AppUiCommand::RespondApproval(params) = sent_command(action) else {
            panic!("expected RespondApproval command");
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

        let AppUiCommand::RespondApproval(params) = sent_command(action) else {
            panic!("expected RespondApproval command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Approve);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::REQUEST)
        );

        let (mut store, approval_id) = store_with_visible_approval();
        let action = handle_key(&mut store, key(KeyCode::Char('s')));

        let AppUiCommand::RespondApproval(params) = sent_command(action) else {
            panic!("expected RespondApproval command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Approve);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::SESSION)
        );

        let (mut store, approval_id) = store_with_visible_approval();
        let action = handle_key(&mut store, key(KeyCode::Char('n')));

        let AppUiCommand::RespondApproval(params) = sent_command(action) else {
            panic!("expected RespondApproval command");
        };
        assert_eq!(params.approval_id, approval_id);
        assert_eq!(params.decision, ApprovalDecision::Deny);
        assert_eq!(
            params.approval_scope.as_deref(),
            Some(approval_scopes::REQUEST)
        );
    }
}
