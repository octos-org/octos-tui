use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    Mock,
    Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThemeName {
    Terminal,
    Slate,
    Codex,
    Claude,
    Solarized,
}

#[derive(Debug, Parser)]
#[command(
    name = "octos-tui",
    about = "Mock-backed Octos TUI prototype on the AppUi/UI Protocol boundary"
)]
pub struct Cli {
    /// Backend mode.
    #[arg(long, value_enum, default_value_t = Mode::Mock)]
    pub mode: Mode,

    /// UI Protocol v1 WebSocket endpoint.
    #[arg(
        long = "endpoint",
        alias = "protocol-endpoint",
        value_name = "WS_URL",
        value_parser = parse_websocket_url,
    )]
    pub base_url: Option<String>,

    /// Session id to open first.
    #[arg(long)]
    pub session: Option<String>,

    /// Profile id to use for the session.
    #[arg(long = "profile-id", alias = "profile")]
    pub profile_id: Option<String>,

    /// Workspace cwd to request for this AppUi session. Defaults to the launch directory.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Bearer token for UI Protocol authentication. Falls back to OCTOS_AUTH_TOKEN.
    #[arg(long = "auth-token", value_name = "TOKEN")]
    pub auth_token: Option<String>,

    /// Disable turn/start sends and use the client as a read-only viewer.
    #[arg(long)]
    pub readonly: bool,

    /// Color palette.
    #[arg(long, value_enum, default_value_t = ThemeName::Codex)]
    pub theme: ThemeName,
}

pub fn parse_websocket_url(value: &str) -> std::result::Result<String, String> {
    if is_websocket_url(value) {
        Ok(value.to_string())
    } else {
        Err("endpoint must be a WebSocket URL starting with ws:// or wss://".into())
    }
}

fn is_websocket_url(value: &str) -> bool {
    let value = value.trim_start();
    value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ws://"))
        || value
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("wss://"))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Mode};

    #[test]
    fn parses_snapshot_launch_flags() {
        let cli = Cli::try_parse_from([
            "octos-tui",
            "--mode",
            "protocol",
            "--endpoint",
            "wss://example.test/ui-protocol",
            "--session",
            "session-123",
            "--profile-id",
            "coding",
            "--cwd",
            "/tmp/project",
            "--auth-token",
            "secret-token",
            "--readonly",
        ])
        .expect("cli parses");

        assert_eq!(cli.mode, Mode::Protocol);
        assert_eq!(
            cli.base_url.as_deref(),
            Some("wss://example.test/ui-protocol")
        );
        assert_eq!(cli.session.as_deref(), Some("session-123"));
        assert_eq!(cli.profile_id.as_deref(), Some("coding"));
        assert_eq!(
            cli.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        assert_eq!(cli.auth_token.as_deref(), Some("secret-token"));
        assert!(cli.readonly);
    }

    #[test]
    fn defaults_to_mock_mode() {
        let cli = Cli::try_parse_from(["octos-tui"]).expect("cli parses");

        assert_eq!(cli.mode, Mode::Mock);
        assert!(cli.base_url.is_none());
        assert_eq!(cli.theme, super::ThemeName::Codex);
    }

    #[test]
    fn parses_theme_choice() {
        let cli = Cli::try_parse_from(["octos-tui", "--theme", "claude"]).expect("cli parses");

        assert_eq!(cli.theme, super::ThemeName::Claude);
    }

    #[test]
    fn parses_terminal_theme_choice() {
        let cli = Cli::try_parse_from(["octos-tui", "--theme", "terminal"]).expect("cli parses");

        assert_eq!(cli.theme, super::ThemeName::Terminal);
    }

    #[test]
    fn rejects_non_websocket_protocol_endpoint() {
        let err = Cli::try_parse_from([
            "octos-tui",
            "--mode",
            "protocol",
            "--endpoint",
            "https://example.test/ui-protocol",
        ])
        .expect_err("http endpoint should be rejected");

        assert!(err.to_string().contains("endpoint must be a WebSocket URL"));
    }
}
