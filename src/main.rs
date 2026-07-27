use eyre::Result;
use octos_tui::{backend_ensure, cli::Cli, cmd, event_loop};

fn main() -> Result<()> {
    color_eyre::install()?;
    install_terminal_restoring_panic_hook();

    // Intercept `update`/`doctor` subcommands before the normal TUI launch.
    // A leading `update`/`doctor` positional dispatches to the command modules
    // and exits with their code; anything else falls through to the TUI.
    if let Some(code) = cmd::dispatch(std::env::args())? {
        std::process::exit(code);
    }

    let mut cli = Cli::parse()?;
    // Provision the `octos` server backend if a local stdio launch needs it and
    // it isn't installed — BEFORE the event loop claims the terminal, so the
    // installer's output prints cleanly. May rewrite `cli.stdio_command` to an
    // explicit `~/.octos/bin/octos` path when a fresh install isn't on PATH.
    // No-op for WebSocket/mock launches, a user-managed octos path/command, or
    // when a compatible backend is already present.
    backend_ensure::ensure_octos_backend(&mut cli)?;
    event_loop::run(cli)
}

/// Chain a panic hook that restores the terminal before the (color_eyre)
/// panic report prints.
///
/// A panic while raw mode and/or the alternate screen are active would emit
/// the report into the raw terminal: staircased line endings, or entirely
/// erased when the alternate screen is dropped, leaving the user's shell
/// looking wedged. Every step is best-effort (errors ignored) and idempotent
/// with the normal `TerminalGuard` restore in the event loop, so running both
/// is harmless.
fn install_terminal_restoring_panic_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        use crossterm::{
            cursor::Show,
            event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture},
            execute,
            terminal::{LeaveAlternateScreen, disable_raw_mode},
        };

        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, DisableMouseCapture);
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = execute!(stdout, DisableBracketedPaste, DisableFocusChange, Show);

        previous_hook(panic_info);
    }));
}
