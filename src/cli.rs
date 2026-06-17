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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    Terminal,
    Slate,
    #[default]
    Codex,
    Claude,
    Solarized,
}

impl ThemeName {
    /// Stable kebab id used by the `/theme` menu items and the runtime
    /// theme marker. Must match the ids emitted by `theme_menu` so the
    /// active palette round-trips through `from_id`.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeName::Terminal => "terminal",
            ThemeName::Slate => "slate",
            ThemeName::Codex => "codex",
            ThemeName::Claude => "claude",
            ThemeName::Solarized => "solarized",
        }
    }

    /// Parse a `/theme` menu id back into a `ThemeName`. Returns `None`
    /// for an unknown id so the caller can leave the active theme intact.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "terminal" => Some(ThemeName::Terminal),
            "slate" => Some(ThemeName::Slate),
            "codex" => Some(ThemeName::Codex),
            "claude" => Some(ThemeName::Claude),
            "solarized" => Some(ThemeName::Solarized),
            _ => None,
        }
    }
}

/// How scrolling interacts with the chat composer.
///
/// `Native` (default) keeps the terminal's own scrollback authoritative: the
/// wheel scrolls the terminal, native selection/copy work untouched, and the
/// composer scrolls away with the screen (the transcript pager via Ctrl+T /
/// PageUp is the pinned view). `Pinned` opts into app-side mouse capture so
/// the wheel scrolls the transcript pager instead — the composer stays pinned
/// to the bottom no matter how you scroll, at the cost of native mouse
/// selection (use Shift+drag in most terminals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollMode {
    #[default]
    Native,
    Pinned,
}

impl ScrollMode {
    /// Kebab id symmetric with the `scroll-mode` config key / `--scroll-mode`
    /// flag, so a saved value round-trips back through the loader.
    pub fn as_str(self) -> &'static str {
        match self {
            ScrollMode::Native => "native",
            ScrollMode::Pinned => "pinned",
        }
    }
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
    /// Wheel-scroll behavior for the chat flow.
    pub scroll_mode: ScrollMode,
}

#[derive(Debug, Parser)]
#[command(
    name = "octos-tui",
    version = env!("CARGO_PKG_VERSION"),
    about = "Mock-backed Octos TUI prototype on the Octos UI Protocol boundary"
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

    /// Wheel-scroll behavior: `native` keeps terminal scrollback + native
    /// selection (default); `pinned` captures the mouse so the wheel scrolls
    /// the transcript pager and the composer stays pinned to the bottom
    /// (native selection then needs Shift+drag).
    #[arg(long = "scroll-mode", value_enum)]
    pub scroll_mode: Option<ScrollMode>,
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

    #[serde(alias = "scroll_mode")]
    pub scroll_mode: Option<ScrollMode>,
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
            // No explicit `--config`: fall back to the default location so UI
            // settings saved via `/saveconfig` (which writes there) round-trip
            // on the next plain launch.
            None => load_default_config_file(),
        };
        let file_config = file_config.unwrap_or_default();

        let base_url = args.base_url.or(file_config.base_url);
        let stdio_command = args.stdio_command.or(file_config.stdio_command);
        if base_url.is_some() && stdio_command.is_some() {
            return Err(eyre!(
                "endpoint and stdio-command cannot both be configured; choose one Octos UI transport"
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
            scroll_mode: args
                .scroll_mode
                .or(file_config.scroll_mode)
                .unwrap_or_default(),
        })
    }
}

pub fn load_config_file(path: &Path) -> Result<CliFileConfig> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read TUI config {}", path.display()))?;
    serde_json::from_str(&contents)
        .wrap_err_with(|| format!("failed to parse TUI config {}", path.display()))
}

/// Load the default config file (the path `/saveconfig` writes to when the
/// session was launched without `--config`) so saved UI settings round-trip on
/// the next plain launch. A missing file is the normal first-run case and
/// yields `None`. A present-but-unreadable/corrupt file must NOT block startup
/// — unlike an explicit `--config`, the user didn't ask for this file — so we
/// warn and fall back to built-in defaults rather than propagating the error.
fn load_default_config_file() -> Option<CliFileConfig> {
    let path = default_config_path()?;
    if !path.exists() {
        return None;
    }
    match load_config_file(&path) {
        Ok(config) => Some(config),
        Err(error) => {
            eprintln!(
                "warning: ignoring unreadable default TUI config {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// Default config path used by `/saveconfig` when the session was launched
/// without an explicit `--config`. Follows the XDG/CLI convention the backend
/// adopted (`~/.config/octos-tui/config.json`).
pub fn default_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("octos-tui")
            .join("config.json"),
    )
}

/// Persist the runtime UI settings (theme / lang / scroll-mode) back into the
/// config file, MERGING into whatever is already there: the existing JSON is
/// read as a generic object and only these three keys are patched, so transport
/// keys (stdio-command, profile-id, session, endpoint, …) and any unknown keys
/// survive untouched. A missing or empty file starts from an empty object.
/// Returns the path actually written.
pub fn save_ui_settings(
    path: &Path,
    theme: ThemeName,
    lang: Lang,
    scroll_mode: ScrollMode,
) -> Result<()> {
    let mut root = match fs::read_to_string(path) {
        Ok(contents) if !contents.trim().is_empty() => {
            serde_json::from_str::<serde_json::Value>(&contents)
                .wrap_err_with(|| format!("failed to parse TUI config {}", path.display()))?
        }
        _ => serde_json::Value::Object(serde_json::Map::new()),
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| eyre!("TUI config {} is not a JSON object", path.display()))?;
    object.insert("theme".into(), theme.as_str().into());
    object.insert("lang".into(), lang.code().into());
    object.insert("scroll-mode".into(), scroll_mode.as_str().into());
    // Drop any legacy snake_case alias so the canonical key is authoritative.
    object.remove("scroll_mode");

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create config dir {}", parent.display()))?;
    }
    let mut serialized =
        serde_json::to_string_pretty(&root).wrap_err("failed to serialize TUI config")?;
    serialized.push('\n');
    fs::write(path, serialized)
        .wrap_err_with(|| format!("failed to write TUI config {}", path.display()))
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

    // Without `--config`, startup now reads the default config path that
    // `/saveconfig` writes to, so saved UI settings round-trip on a plain
    // launch. A missing default file falls back to built-in defaults.
    #[test]
    fn loads_default_config_when_no_explicit_config_flag() {
        use std::sync::Mutex;
        // HOME is process-global; serialize HOME-mutating access.
        static HOME_LOCK: Mutex<()> = Mutex::new(());
        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is valid")
            .as_nanos();
        let home = std::env::temp_dir().join(format!("octos-tui-home-{nonce}"));
        let cfg_dir = home.join(".config").join("octos-tui");
        fs::create_dir_all(&cfg_dir).expect("config dir");
        let cfg_path = cfg_dir.join("config.json");

        let prev_home = std::env::var_os("HOME");
        // SAFETY: the workspace denies unsafe and `std::env::set_var` is unsafe
        // under edition 2024. Guarded by HOME_LOCK (no concurrent reader) and
        // restored below before the guard drops.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("HOME", &home);
        }

        // A saved default config (theme=claude) is read on a plain launch.
        fs::write(&cfg_path, r#"{ "theme": "claude" }"#).expect("write default config");
        let cli = Cli::try_parse_from(["octos-tui"]).expect("parse with default config");
        assert_eq!(
            cli.theme,
            ThemeName::Claude,
            "default config saved by /saveconfig must be loaded on a plain launch"
        );

        // No default file present → built-in default theme.
        fs::remove_file(&cfg_path).ok();
        let cli = Cli::try_parse_from(["octos-tui"]).expect("parse without default config");
        assert_eq!(
            cli.theme,
            ThemeName::Codex,
            "absent default config → built-in default theme"
        );

        // Restore HOME so other tests see the original environment.
        #[allow(unsafe_code)]
        unsafe {
            match prev_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
        let _ = fs::remove_dir_all(&home);
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

        assert!(err.to_string().contains("choose one Octos UI transport"));
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
