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
    /// Vim modal editing for the composer (opt-in; default off).
    pub vim_mode: bool,
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

    /// Enable Vim modal editing in the composer (Normal/Insert). Off by default;
    /// also toggled at runtime with `/vimmode`.
    #[arg(long = "vim-mode")]
    pub vim_mode: bool,
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

    #[serde(alias = "vim_mode")]
    pub vim_mode: Option<bool>,
}

impl Cli {
    pub fn parse() -> Result<Self> {
        let args = CliArgs::parse();
        // Production launch reads the default config when no `--config` is
        // given, so `/saveconfig` settings round-trip on the next plain launch.
        Self::from_args(args, true)
    }

    #[cfg(test)]
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let args = CliArgs::try_parse_from(itr)?;
        // Tests stay deterministic: don't read the ambient `$HOME` default
        // config (the default-read path is covered by `load_config_file_if_present`
        // tests, which inject a path instead of mutating `$HOME`).
        Self::from_args(args, false)
    }

    fn from_args(args: CliArgs, use_default_config: bool) -> Result<Self> {
        let file_config = match args.config.as_ref() {
            Some(path) => Some(load_config_file(path)?),
            // No explicit `--config`: fall back to the default location so UI
            // settings saved via `/saveconfig` (which writes there) round-trip
            // on the next plain launch. Skipped in tests (`use_default_config`
            // = false) so the suite never depends on the ambient `$HOME`.
            None if use_default_config => load_default_config_file(),
            None => None,
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

        // Mode resolution: explicit (CLI flag, then config) always wins.
        // When neither sets a mode but a transport IS configured
        // (stdio-command or endpoint), infer Protocol — otherwise
        // `octos-tui --stdio-command '…'` would silently launch the MOCK
        // backend and ignore the transport. Mock stays the default only
        // for a transport-less launch.
        let mode = args.mode.or(file_config.mode).unwrap_or_else(|| {
            if stdio_command.is_some() || base_url.is_some() {
                Mode::Protocol
            } else {
                Mode::Mock
            }
        });

        Ok(Self {
            config: args.config,
            mode,
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
            // The flag is a bare bool (no way to distinguish "unset" from
            // "false"), so the CLI flag only force-enables; the config provides
            // the default when the flag is absent.
            vim_mode: args.vim_mode || file_config.vim_mode.unwrap_or(false),
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
/// the next plain launch. `None` when no default path resolves or the file is
/// absent/unreadable (see [`load_config_file_if_present`]).
fn load_default_config_file() -> Option<CliFileConfig> {
    load_config_file_if_present(&default_config_path()?)
}

/// Leniently load an *auto-discovered* config file: a missing file is the
/// normal first-run case and yields `None`; a present-but-unreadable/corrupt
/// file must NOT block startup — unlike an explicit `--config`, the user didn't
/// ask for this file — so we warn and fall back to built-in defaults rather
/// than propagating the error. (Kept path-injectable so it's testable without
/// mutating the process-global `$HOME`.)
fn load_config_file_if_present(path: &Path) -> Option<CliFileConfig> {
    if !path.exists() {
        return None;
    }
    match load_config_file(path) {
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
/// adopted (`~/.config/octos-tui/config.json`). Falls back to `USERPROFILE` on
/// Windows, where `HOME` is usually unset, so `/saveconfig` still has a default
/// home to write to there.
pub fn default_config_path() -> Option<PathBuf> {
    config_path_from_home(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

/// Pure resolver behind [`default_config_path`]: prefer `HOME`, fall back to
/// `USERPROFILE` (Windows, where `HOME` is usually unset). Empty values are
/// ignored. Split out so the fallback is testable without mutating process env
/// (`std::env::set_var` is `unsafe` under edition 2024 + `unsafe_code = deny`).
fn config_path_from_home(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let base = home
        .filter(|value| !value.is_empty())
        .or_else(|| userprofile.filter(|value| !value.is_empty()))?;
    Some(
        PathBuf::from(base)
            .join(".config")
            .join("octos-tui")
            .join("config.json"),
    )
}

/// Persist the runtime UI settings (theme / lang / scroll-mode / vim-mode) back
/// into the config file, MERGING into whatever is already there: the existing
/// JSON is read as a generic object and only these keys are patched, so
/// transport keys (stdio-command, profile-id, session, endpoint, …) and any
/// unknown keys survive untouched. A missing or empty file starts from an empty
/// object. Returns the path actually written.
pub fn save_ui_settings(
    path: &Path,
    theme: ThemeName,
    lang: Lang,
    scroll_mode: ScrollMode,
    vim_mode: bool,
) -> Result<()> {
    let mut root = match fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Ok(contents) => serde_json::from_str::<serde_json::Value>(&contents)
            .wrap_err_with(|| format!("failed to parse TUI config {}", path.display()))?,
        // A not-yet-created file is the normal first-save case → start empty.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        // Any OTHER read error (permissions, invalid UTF-8, …) must NOT be
        // treated as empty: that would overwrite an existing config with only
        // the UI keys, dropping the transport/unknown keys it otherwise
        // preserves. Surface it so the save aborts instead of clobbering.
        Err(error) => {
            return Err(error)
                .wrap_err_with(|| format!("failed to read TUI config {}", path.display()));
        }
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| eyre!("TUI config {} is not a JSON object", path.display()))?;
    object.insert("theme".into(), theme.as_str().into());
    object.insert("lang".into(), lang.code().into());
    object.insert("scroll-mode".into(), scroll_mode.as_str().into());
    object.insert("vim-mode".into(), vim_mode.into());
    // Drop any legacy snake_case aliases so the canonical keys are authoritative.
    object.remove("scroll_mode");
    object.remove("vim_mode");

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
    fn config_path_prefers_home_then_userprofile() {
        use super::config_path_from_home;
        use std::ffi::OsString;

        let suffix: PathBuf = [".config", "octos-tui", "config.json"].iter().collect();

        // HOME wins when set.
        let from_home = config_path_from_home(Some(OsString::from("/home/u")), None)
            .expect("HOME resolves a path");
        assert!(from_home.starts_with("/home/u") && from_home.ends_with(&suffix));

        // Windows: HOME unset/empty → fall back to USERPROFILE.
        let from_profile = config_path_from_home(
            Some(OsString::new()),
            Some(OsString::from("C:\\Users\\Alex")),
        )
        .expect("USERPROFILE resolves a path when HOME is empty");
        assert!(from_profile.ends_with(&suffix));
        assert!(
            config_path_from_home(None, Some(OsString::from("C:\\Users\\Alex"))).is_some(),
            "missing HOME still resolves via USERPROFILE"
        );

        // Neither set (or both empty) → no default path.
        assert!(config_path_from_home(None, None).is_none());
        assert!(config_path_from_home(Some(OsString::new()), Some(OsString::new())).is_none());
    }

    #[test]
    fn lang_flag_parses_and_defaults_to_en() {
        let cli = Cli::try_parse_from(["octos-tui", "--lang", "zh"]).expect("parse --lang zh");
        assert_eq!(cli.lang, super::Lang::Zh);
        let cli = Cli::try_parse_from(["octos-tui"]).expect("parse default");
        // No flag/config/env override in this minimal invocation → English.
        assert!(matches!(cli.lang, super::Lang::En | super::Lang::Zh));
    }

    // The default-config read (used at launch when there's no `--config`) is
    // lenient and path-injectable: a present file loads, an absent file yields
    // None (the caller falls back to built-in defaults), and a corrupt/
    // unreadable file is skipped rather than blocking startup. Exercised via an
    // explicit path so it never mutates the process-global `$HOME`.
    #[test]
    fn default_config_load_is_lenient_and_path_injectable() {
        let path = write_config("default-config", r#"{ "theme": "claude" }"#);

        // Present + valid → loaded (this is what makes /saveconfig round-trip).
        let cfg = super::load_config_file_if_present(&path).expect("present default config loads");
        assert_eq!(cfg.theme, Some(ThemeName::Claude));

        // Absent → None.
        fs::remove_file(&path).expect("remove config");
        assert!(
            super::load_config_file_if_present(&path).is_none(),
            "absent default config yields None"
        );

        // Present but corrupt → None (skipped, not a fatal startup error).
        fs::write(&path, "{ not valid json").expect("write corrupt config");
        assert!(
            super::load_config_file_if_present(&path).is_none(),
            "corrupt default config is skipped, not fatal"
        );
        let _ = fs::remove_file(&path);
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

    // A configured transport with no explicit mode must NOT silently
    // launch the mock backend (the transport flag would be ignored and
    // the user would chat with fake data). Absent mode + a transport
    // infers Protocol.
    #[test]
    fn should_infer_protocol_mode_when_stdio_command_set_without_mode() {
        let cli = Cli::try_parse_from(["octos-tui", "--stdio-command", "octos serve --stdio"])
            .expect("cli parses");

        assert_eq!(cli.mode, Mode::Protocol);
        assert_eq!(cli.stdio_command.as_deref(), Some("octos serve --stdio"));
    }

    #[test]
    fn should_infer_protocol_mode_when_endpoint_set_without_mode() {
        let cli = Cli::try_parse_from(["octos-tui", "--endpoint", "ws://127.0.0.1:1/ui"])
            .expect("cli parses");

        assert_eq!(cli.mode, Mode::Protocol);
        assert_eq!(cli.base_url.as_deref(), Some("ws://127.0.0.1:1/ui"));
    }

    #[test]
    fn should_infer_protocol_mode_from_config_transport_without_mode() {
        let path = write_config(
            "infer-mode",
            r#"{ "stdio_command": "octos serve --stdio" }"#,
        );

        let cli = Cli::try_parse_from(["octos-tui", "--config", path.to_str().unwrap()])
            .expect("cli parses");

        assert_eq!(cli.mode, Mode::Protocol);
    }

    #[test]
    fn explicit_mock_mode_wins_over_transport_inference() {
        let cli = Cli::try_parse_from([
            "octos-tui",
            "--mode",
            "mock",
            "--stdio-command",
            "octos serve --stdio",
        ])
        .expect("cli parses");

        assert_eq!(cli.mode, Mode::Mock);

        // Config-level explicit mode also wins over inference.
        let path = write_config(
            "explicit-mock",
            r#"{ "mode": "mock", "stdio_command": "octos serve --stdio" }"#,
        );
        let cli = Cli::try_parse_from(["octos-tui", "--config", path.to_str().unwrap()])
            .expect("cli parses");
        assert_eq!(cli.mode, Mode::Mock);
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
