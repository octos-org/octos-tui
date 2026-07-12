//! `octos-tui` subcommands: `update` and `doctor` (design doc).
//!
//! The TUI's CLI is a hand-rolled `Cli::parse()` with no clap subcommands, and
//! the default no-subcommand invocation launches the TUI. To add `update` and
//! `doctor` without disturbing that, [`dispatch`] inspects `argv` for a leading
//! `update`/`doctor` positional *before* the normal launch path. When it
//! matches, it parses that subcommand's flags with a dedicated clap parser and
//! runs it, returning the desired process exit code; otherwise it returns
//! `None` and the caller proceeds to the normal TUI launch.

pub mod config;
pub mod doctor;
pub mod github;
pub mod install_method;
pub mod update;

use clap::Parser;
use eyre::Result;

use config::{ConfigArgs, ConfigCli};
use doctor::DoctorArgs;
use update::UpdateArgs;

/// Recognized subcommand names. Kept tiny so we never shadow a flag.
const SUBCOMMANDS: &[&str] = &["update", "doctor", "config"];

/// Inspect `argv` (excluding the program name) for a leading subcommand. If the
/// first non-flag positional is `update`/`doctor`, run it and return its exit
/// code; otherwise return `None` so the caller launches the TUI as before.
///
/// Only a *leading* positional is treated as a subcommand: `octos-tui --lang zh`
/// still launches the TUI, and a config value that happens to be "doctor" is
/// never misread as a subcommand because we only look at the first bare token.
pub fn dispatch<I, S>(args: I) -> Result<Option<i32>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let argv: Vec<String> = args.into_iter().map(Into::into).collect();
    match route(&argv) {
        Some(Route::Update(args)) => Ok(Some(update::run(args)?.exit_code())),
        Some(Route::Doctor(args)) => Ok(Some(doctor::run(args)?)),
        Some(Route::Config(args)) => Ok(Some(config::run(args)?)),
        None => Ok(None),
    }
}

/// A routed (parsed) subcommand, ready to run. Splitting routing from
/// execution lets the routing/arg-parsing be unit-tested without performing the
/// network I/O the command bodies do.
#[derive(Debug)]
enum Route {
    Update(UpdateArgs),
    Doctor(DoctorArgs),
    Config(ConfigArgs),
}

/// Parse `argv` into a [`Route`] if it leads with a known subcommand; otherwise
/// `None` (the caller launches the TUI). The synthetic program name keeps
/// clap's usage strings accurate (e.g. `octos-tui doctor`); the subcommand
/// token is dropped (`skip(2)`) so clap does not see it as a stray positional.
fn route(argv: &[String]) -> Option<Route> {
    let first = argv.get(1)?;
    if !SUBCOMMANDS.contains(&first.as_str()) {
        return None;
    }
    let prog = format!("octos-tui {first}");
    let sub_argv: Vec<String> = std::iter::once(prog)
        .chain(argv.iter().skip(2).cloned())
        .collect();
    match first.as_str() {
        "update" => Some(Route::Update(UpdateCli::parse_from(&sub_argv).into_args())),
        "doctor" => Some(Route::Doctor(DoctorCli::parse_from(&sub_argv).into_args())),
        "config" => Some(Route::Config(ConfigCli::parse_from(&sub_argv).into_args())),
        _ => unreachable!("guarded by SUBCOMMANDS"),
    }
}

/// `octos-tui update` flags.
#[derive(Debug, Parser)]
#[command(
    name = "octos-tui update",
    about = "Update octos-tui in place (cargo-dist installs) or print the right upgrade command"
)]
struct UpdateCli {
    /// Only report whether an update is available (exit 10 if newer).
    #[arg(long)]
    check: bool,
    /// Update to a specific version (e.g. 0.1.2).
    #[arg(long, value_name = "X.Y.Z", conflicts_with = "tag")]
    version: Option<String>,
    /// Update to a specific release tag (e.g. v0.1.2).
    #[arg(long, value_name = "TAG", conflicts_with = "version")]
    tag: Option<String>,
    /// Allow prerelease targets.
    #[arg(long)]
    prerelease: bool,
    /// Re-install even if already current.
    #[arg(long)]
    force: bool,
    /// Skip the interactive confirmation.
    #[arg(long, short = 'y')]
    yes: bool,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
}

impl UpdateCli {
    fn into_args(self) -> UpdateArgs {
        UpdateArgs {
            check: self.check,
            version: self.version,
            tag: self.tag,
            prerelease: self.prerelease,
            force: self.force,
            yes: self.yes,
            json: self.json,
        }
    }
}

/// `octos-tui doctor` flags.
#[derive(Debug, Parser)]
#[command(
    name = "octos-tui doctor",
    about = "Diagnose octos-tui's environment, install, and protocol compatibility"
)]
struct DoctorCli {
    /// Emit machine-readable JSON (support bundle).
    #[arg(long)]
    json: bool,
    /// Add resolved paths/versions to each line.
    #[arg(long, short = 'v')]
    verbose: bool,
    /// Treat warnings as failures (affects the exit code).
    #[arg(long)]
    strict: bool,
    /// stdio child command to probe (e.g. `octos serve --stdio`).
    #[arg(long = "stdio-command", value_name = "CMD")]
    stdio_command: Option<String>,
    /// WS endpoint to record for the connectivity check.
    #[arg(long = "endpoint", value_name = "WS_URL")]
    endpoint: Option<String>,
    /// Bearer token for UI Protocol authentication. Falls back to OCTOS_AUTH_TOKEN.
    #[arg(long = "auth-token", value_name = "TOKEN")]
    auth_token: Option<String>,
    /// Data dir to check (defaults to ~/.octos).
    #[arg(long = "data-dir", value_name = "DIR")]
    data_dir: Option<std::path::PathBuf>,
}

impl DoctorCli {
    fn into_args(self) -> DoctorArgs {
        DoctorArgs {
            json: self.json,
            verbose: self.verbose,
            strict: self.strict,
            stdio_command: self.stdio_command,
            endpoint: self.endpoint,
            auth_token: self.auth_token,
            data_dir: self.data_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn route_returns_none_for_no_subcommand() {
        // Plain launch — no subcommand → TUI path.
        assert!(route(&argv(&["octos-tui"])).is_none());
        // A flag-only invocation is not a subcommand.
        assert!(route(&argv(&["octos-tui", "--lang", "zh"])).is_none());
        // A non-subcommand positional also falls through.
        assert!(route(&argv(&["octos-tui", "chat"])).is_none());
    }

    #[test]
    fn route_recognizes_update_with_flags() {
        // Routing + arg-parsing happens here; no network is performed.
        let routed = route(&argv(&["octos-tui", "update", "--check", "--json"]));
        match routed {
            Some(Route::Update(args)) => {
                assert!(args.check);
                assert!(args.json);
            }
            other => panic!("expected Route::Update, got {other:?}"),
        }
    }

    #[test]
    fn route_recognizes_doctor_with_flags() {
        let routed = route(&argv(&["octos-tui", "doctor", "--strict", "--verbose"]));
        match routed {
            Some(Route::Doctor(args)) => {
                assert!(args.strict);
                assert!(args.verbose);
            }
            other => panic!("expected Route::Doctor, got {other:?}"),
        }
    }

    #[test]
    fn doctor_data_dir_and_stdio_flags_parse() {
        // The dedicated parser receives flags *without* the subcommand token
        // (dispatch strips it). Exercises a couple of flags route() doesn't.
        let args = DoctorCli::parse_from([
            "octos-tui doctor",
            "--stdio-command",
            "octos serve --stdio",
            "--data-dir",
            "/tmp/x",
        ])
        .into_args();
        assert_eq!(args.stdio_command.as_deref(), Some("octos serve --stdio"));
        assert_eq!(
            args.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
    }

    #[test]
    fn route_recognizes_config_and_defaults_to_wizard() {
        // Bare `config` → wizard action.
        match route(&argv(&["octos-tui", "config"])) {
            Some(Route::Config(args)) => {
                assert!(matches!(args.action, config::ConfigAction::Wizard));
            }
            other => panic!("expected Route::Config, got {other:?}"),
        }
        // Explicit actions parse.
        match route(&argv(&["octos-tui", "config", "path"])) {
            Some(Route::Config(args)) => assert!(matches!(args.action, config::ConfigAction::Path)),
            other => panic!("expected Route::Config(Path), got {other:?}"),
        }
        match route(&argv(&["octos-tui", "config", "show", "--config", "/tmp/c.json"])) {
            Some(Route::Config(args)) => {
                assert!(matches!(args.action, config::ConfigAction::Show));
                assert_eq!(args.config.as_deref(), Some(std::path::Path::new("/tmp/c.json")));
            }
            other => panic!("expected Route::Config(Show), got {other:?}"),
        }
    }

    #[test]
    fn update_version_and_tag_conflict() {
        // Note: no subcommand token — dispatch strips it before this parser.
        let err = UpdateCli::try_parse_from([
            "octos-tui update",
            "--version",
            "0.1.2",
            "--tag",
            "v0.1.2",
        ]);
        assert!(err.is_err(), "version and tag must conflict");
    }
}
