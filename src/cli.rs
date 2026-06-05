use clap::{Parser, ValueEnum};
use eyre::{Result, WrapErr, eyre};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Mock,
    Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    Terminal,
    Slate,
    Codex,
    Claude,
    Solarized,
}

/// UI display language (i18n). English is the source/fallback locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lang {
    En,
    Zh,
}

impl Lang {
    /// The rust-i18n locale code passed to `rust_i18n::set_locale`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }

    /// Best-effort parse of a `LANG`/`OCTOS_LANG`-style value (e.g.
    /// `zh_CN.UTF-8`, `zh`, `en_US`) into a supported UI language; `None` if
    /// unrecognized so the caller can fall through to the default.
    pub fn from_env_value(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_lowercase();
        if v.starts_with("zh") {
            Some(Lang::Zh)
        } else if v.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// JSON config file used as launch defaults.
    pub config: Option<PathBuf>,
    /// Backend mode.
    pub mode: Mode,
    /// UI Protocol v1 WebSocket endpoint.
    pub base_url: Option<String>,
    /// UI Protocol v1 stdio child command.
    pub stdio_command: Option<String>,
    /// Session id to open first.
    pub session: Option<String>,
    /// Profile id to use for the session.
    pub profile_id: Option<String>,
    /// Workspace cwd to request for this AppUi session. Defaults to the launch directory.
    pub cwd: Option<PathBuf>,
    /// Bearer token for UI Protocol authentication. Falls back to OCTOS_AUTH_TOKEN.
    pub auth_token: Option<String>,
    /// Disable turn/start sends and use the client as a read-only viewer.
    pub readonly: bool,
    /// Color palette.
    pub theme: ThemeName,
    /// UI display language (i18n).
    pub lang: Lang,
}

#[derive(Debug, Parser)]
#[command(
    name = "octos-tui",
    version = env!("CARGO_PKG_VERSION"),
    about = "Mock-backed Octos TUI prototype on the AppUi/UI Protocol boundary"
)]
struct CliArgs {
    /// JSON config file used as launch defaults. CLI flags override config values.
    #[arg(long = "config", value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Backend mode.
    #[arg(long, value_enum)]
    pub mode: Option<Mode>,

    /// UI Protocol v1 WebSocket endpoint.
    #[arg(
        long = "endpoint",
        alias = "protocol-endpoint",
        value_name = "WS_URL",
        value_parser = parse_websocket_url,
        conflicts_with = "stdio_command",
    )]
    pub base_url: Option<String>,

    /// UI Protocol v1 stdio child command.
    #[arg(
        long = "stdio-command",
        value_name = "CMD",
        value_parser = parse_stdio_command,
        conflicts_with = "base_url",
    )]
    pub stdio_command: Option<String>,

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
    #[arg(long, conflicts_with = "no_readonly")]
    pub readonly: bool,

    /// Force read-write mode when the config file sets readonly=true.
    #[arg(long = "no-readonly", conflicts_with = "readonly")]
    pub no_readonly: bool,

    /// Color palette.
    #[arg(long, value_enum)]
    pub theme: Option<ThemeName>,

    /// UI display language (e.g. `en`, `zh`). Falls back to OCTOS_LANG/LANG.
    #[arg(long, value_enum)]
    pub lang: Option<Lang>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct CliFileConfig {
    pub mode: Option<Mode>,

    #[serde(
        rename = "endpoint",
        alias = "base-url",
        alias = "base_url",
        alias = "protocol-endpoint",
        alias = "protocol_endpoint"
    )]
    pub base_url: Option<String>,

    #[serde(alias = "stdio_command")]
    pub stdio_command: Option<String>,
    pub session: Option<String>,

    #[serde(alias = "profile_id", alias = "profile")]
    pub profile_id: Option<String>,

    pub cwd: Option<PathBuf>,

    #[serde(alias = "auth_token")]
    pub auth_token: Option<String>,

    pub readonly: Option<bool>,
    pub theme: Option<ThemeName>,
    pub lang: Option<Lang>,
}

impl Cli {
    pub fn parse() -> Result<Self> {
        let args = CliArgs::parse();
        Self::from_args(args)
    }

    #[cfg(test)]
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let args = CliArgs::try_parse_from(itr)?;
        Self::from_args(args)
    }

    fn from_args(args: CliArgs) -> Result<Self> {
        let file_config = match args.config.as_ref() {
            Some(path) => Some(load_config_file(path)?),
            None => None,
        };
        let file_config = file_config.unwrap_or_default();

        let base_url = args.base_url.or(file_config.base_url);
        let stdio_command = args.stdio_command.or(file_config.stdio_command);
        if base_url.is_some() && stdio_command.is_some() {
            return Err(eyre!(
                "endpoint and stdio-command cannot both be configured; choose one AppUI transport"
            ));
        }

        let base_url = match base_url {
            Some(value) => Some(parse_websocket_url(&value).map_err(|message| eyre!(message))?),
            None => None,
        };
        let stdio_command = match stdio_command {
            Some(value) => Some(parse_stdio_command(&value).map_err(|message| eyre!(message))?),
            None => None,
        };

        let readonly = if args.no_readonly {
            false
        } else if args.readonly {
            true
        } else {
            file_config.readonly.unwrap_or(false)
        };

        Ok(Self {
            config: args.config,
            mode: args.mode.or(file_config.mode).unwrap_or(Mode::Mock),
            base_url,
            stdio_command,
            session: args.session.or(file_config.session),
            profile_id: args.profile_id.or(file_config.profile_id),
            cwd: args.cwd.or(file_config.cwd),
            auth_token: args.auth_token.or(file_config.auth_token),
            readonly,
            theme: args.theme.or(file_config.theme).unwrap_or(ThemeName::Codex),
            lang: args
                .lang
                .or(file_config.lang)
                .or_else(|| {
                    std::env::var("OCTOS_LANG")
                        .ok()
                        .and_then(|v| Lang::from_env_value(&v))
                })
                .or_else(|| {
                    std::env::var("LANG")
                        .ok()
                        .and_then(|v| Lang::from_env_value(&v))
                })
                .unwrap_or(Lang::En),
        })
    }
}

pub fn load_config_file(path: &Path) -> Result<CliFileConfig> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read TUI config {}", path.display()))?;
    serde_json::from_str(&contents)
        .wrap_err_with(|| format!("failed to parse TUI config {}", path.display()))
}

pub fn parse_websocket_url(value: &str) -> std::result::Result<String, String> {
    if is_websocket_url(value) {
        Ok(value.to_string())
    } else {
        Err("endpoint must be a WebSocket URL starting with ws:// or wss://".into())
    }
}

pub fn parse_stdio_command(value: &str) -> std::result::Result<String, String> {
    let command = value.trim();
    if command.is_empty() {
        Err("stdio command must not be empty".into())
    } else {
        Ok(command.to_string())
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
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use clap::{Parser, error::ErrorKind};

    use super::{Cli, Mode, ThemeName};

    fn write_config(name: &str, contents: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("octos-tui-{name}-{nonce}.json"));
        fs::write(&path, contents).expect("config writes");
        path
    }

    #[test]
    fn lang_parses_env_values_and_maps_to_locale_code() {
        use super::Lang;
        assert_eq!(Lang::from_env_value("zh"), Some(Lang::Zh));
        assert_eq!(Lang::from_env_value("zh_CN.UTF-8"), Some(Lang::Zh));
        assert_eq!(Lang::from_env_value("  EN_us "), Some(Lang::En));
        assert_eq!(Lang::from_env_value("fr"), None);
        assert_eq!(Lang::from_env_value(""), None);
        assert_eq!(Lang::Zh.code(), "zh");
        assert_eq!(Lang::En.code(), "en");
    }

    #[test]
    fn lang_flag_parses_and_defaults_to_en() {
        let cli = Cli::try_parse_from(["octos-tui", "--lang", "zh"]).expect("parse --lang zh");
        assert_eq!(cli.lang, super::Lang::Zh);
        let cli = Cli::try_parse_from(["octos-tui"]).expect("parse default");
        // No flag/config/env override in this minimal invocation → English.
        assert!(matches!(cli.lang, super::Lang::En | super::Lang::Zh));
    }

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
        assert!(cli.stdio_command.is_none());
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
        assert!(cli.stdio_command.is_none());
        assert_eq!(cli.theme, ThemeName::Codex);
    }

    #[test]
    fn prints_package_version() {
        let err = super::CliArgs::try_parse_from(["octos-tui", "--version"])
            .expect_err("version flag exits early");

        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn parses_stdio_command() {
        let cli = Cli::try_parse_from([
            "octos-tui",
            "--mode",
            "protocol",
            "--stdio-command",
            "octos serve --stdio",
        ])
        .expect("cli parses");

        assert_eq!(cli.mode, Mode::Protocol);
        assert_eq!(cli.stdio_command.as_deref(), Some("octos serve --stdio"));
        assert!(cli.base_url.is_none());
    }

    #[test]
    fn rejects_empty_stdio_command() {
        let err = Cli::try_parse_from(["octos-tui", "--stdio-command", "   "])
            .expect_err("empty stdio command should be rejected");

        assert!(err.to_string().contains("stdio command must not be empty"));
    }

    #[test]
    fn rejects_endpoint_and_stdio_command_together() {
        let err = Cli::try_parse_from([
            "octos-tui",
            "--endpoint",
            "wss://example.test/ui-protocol",
            "--stdio-command",
            "octos serve --stdio",
        ])
        .expect_err("endpoint and stdio command should conflict");

        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn parses_theme_choice() {
        let cli = Cli::try_parse_from(["octos-tui", "--theme", "claude"]).expect("cli parses");

        assert_eq!(cli.theme, ThemeName::Claude);
    }

    #[test]
    fn parses_terminal_theme_choice() {
        let cli = Cli::try_parse_from(["octos-tui", "--theme", "terminal"]).expect("cli parses");

        assert_eq!(cli.theme, ThemeName::Terminal);
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

    #[test]
    fn loads_json_config_file() {
        let path = write_config(
            "launch",
            r#"{
                "mode": "protocol",
                "stdio_command": "octos serve --stdio --data-dir ~/.octos",
                "session": "coding:local:config",
                "profile_id": "coding",
                "cwd": "/tmp/config-project",
                "auth_token": "config-token",
                "readonly": true,
                "theme": "solarized"
            }"#,
        );

        let cli =
            Cli::try_parse_from(["octos-tui", "--config", path.to_str().unwrap()]).expect("parses");

        assert_eq!(cli.config.as_deref(), Some(path.as_path()));
        assert_eq!(cli.mode, Mode::Protocol);
        assert_eq!(
            cli.stdio_command.as_deref(),
            Some("octos serve --stdio --data-dir ~/.octos")
        );
        assert_eq!(cli.session.as_deref(), Some("coding:local:config"));
        assert_eq!(cli.profile_id.as_deref(), Some("coding"));
        assert_eq!(cli.cwd.as_deref(), Some(Path::new("/tmp/config-project")));
        assert_eq!(cli.auth_token.as_deref(), Some("config-token"));
        assert!(cli.readonly);
        assert_eq!(cli.theme, ThemeName::Solarized);
    }

    #[test]
    fn cli_flags_override_json_config_file() {
        let path = write_config(
            "override",
            r#"{
                "mode": "mock",
                "endpoint": "wss://config.example.test/ui-protocol",
                "session": "coding:local:config",
                "profile_id": "coding",
                "readonly": true,
                "theme": "slate"
            }"#,
        );

        let cli = Cli::try_parse_from([
            "octos-tui",
            "--config",
            path.to_str().unwrap(),
            "--mode",
            "protocol",
            "--endpoint",
            "wss://cli.example.test/ui-protocol",
            "--session",
            "coding:local:cli",
            "--profile-id",
            "review",
            "--no-readonly",
            "--theme",
            "codex",
        ])
        .expect("parses");

        assert_eq!(cli.mode, Mode::Protocol);
        assert_eq!(
            cli.base_url.as_deref(),
            Some("wss://cli.example.test/ui-protocol")
        );
        assert_eq!(cli.session.as_deref(), Some("coding:local:cli"));
        assert_eq!(cli.profile_id.as_deref(), Some("review"));
        assert!(!cli.readonly);
        assert_eq!(cli.theme, ThemeName::Codex);
    }

    #[test]
    fn rejects_endpoint_and_stdio_command_from_config() {
        let path = write_config(
            "conflict",
            r#"{
                "mode": "protocol",
                "endpoint": "wss://example.test/ui-protocol",
                "stdio_command": "octos serve --stdio"
            }"#,
        );

        let err = Cli::try_parse_from(["octos-tui", "--config", path.to_str().unwrap()])
            .expect_err("conflicting config should fail");

        assert!(err.to_string().contains("choose one AppUI transport"));
    }

    #[test]
    fn rejects_model_provider_fields_in_tui_config() {
        let path = write_config(
            "provider-owned-by-octos",
            r#"{
                "mode": "protocol",
                "stdio_command": "octos serve --stdio",
                "provider": "deepseek",
                "model": "deepseek-v4-pro"
            }"#,
        );

        let err = Cli::try_parse_from(["octos-tui", "--config", path.to_str().unwrap()])
            .expect_err("model/provider should not be accepted by TUI config");
        let error = format!("{err:?}");

        assert!(error.contains("unknown field"));
        assert!(error.contains("provider"));
    }
}
