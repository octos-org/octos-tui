//! `octos-tui config`: an interactive wizard + inspection for the client's
//! startup config (`~/.config/octos-tui/config.json`).
//!
//! The wizard walks the top-level CLI options — **introspected from the clap
//! `Command`**, so it stays in sync as flags are added rather than hand-mirrored
//! — explains each, shows the value currently saved, and writes the chosen
//! values back by *merging* (keys the user doesn't touch survive). The saved
//! config is the launch default; an explicit CLI flag still overrides it
//! (`Cli::from_args`). "Skip" (empty input) never writes a value, so a built-in
//! default keeps applying.
//!
//! This is the client half of the config-wizard feature; `octos config wizard`
//! is the server half.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Parser, Subcommand};
use eyre::{Result, WrapErr, bail, eyre};

use crate::cli::{self, CliFileConfig, parse_stdio_command, parse_websocket_url};

/// Options that the wizard deliberately does NOT prompt for (the design's
/// "denylist overlay"): `config` selects the target file (chicken/egg),
/// `no-readonly` is just the negation of `readonly`, `auth-token` is a secret
/// that must not land in world-readable JSON, `mode` is inferred from the chosen
/// transport, and `help`/`version` are clap builtins. Transport (`endpoint` /
/// `stdio-command`) is handled by a dedicated mutually-exclusive prompt.
const SKIP: &[&str] = &[
    "config",
    "no-readonly",
    "auth-token",
    "mode",
    "help",
    "version",
    "endpoint",
    "stdio-command",
];

/// The default stdio backend command — matches what a bare install resolves to
/// (and what `backend_ensure` auto-provisions).
const DEFAULT_STDIO_COMMAND: &str = "octos serve --stdio --solo";

/// All config spellings that mean "stdio transport" (`CliFileConfig` accepts the
/// snake alias). Used to READ the current value and to CLEAR every form when the
/// user switches to the endpoint transport, so no stale key survives to trip the
/// launch-time transport-conflict check (codex).
const STDIO_KEYS: &[&str] = &["stdio-command", "stdio_command"];

/// All config spellings that mean "endpoint transport" (`CliFileConfig` renames
/// `base_url` to `endpoint` with these aliases). Same read/clear rationale.
const ENDPOINT_KEYS: &[&str] = &[
    "endpoint",
    "base-url",
    "base_url",
    "protocol-endpoint",
    "protocol_endpoint",
];

/// Parsed `octos-tui config …` invocation (the `config` token already stripped
/// by the dispatcher).
#[derive(Debug)]
pub struct ConfigArgs {
    pub action: ConfigAction,
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum ConfigAction {
    Wizard,
    Show,
    Path,
}

/// `octos-tui config` flags.
#[derive(Debug, Parser)]
#[command(
    name = "octos-tui config",
    about = "Configure octos-tui interactively and inspect the saved config"
)]
pub struct ConfigCli {
    #[command(subcommand)]
    action: Option<ConfigActionCli>,
    /// Config file to read/write (default: ~/.config/octos-tui/config.json).
    #[arg(long = "config", value_name = "FILE", global = true)]
    config: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ConfigActionCli {
    /// Walk every option, explain it, and save your choices (this is the default).
    Wizard,
    /// Print the current saved config.
    Show,
    /// Print the resolved config file path.
    Path,
}

impl ConfigCli {
    pub fn into_args(self) -> ConfigArgs {
        let action = match self.action {
            None | Some(ConfigActionCli::Wizard) => ConfigAction::Wizard,
            Some(ConfigActionCli::Show) => ConfigAction::Show,
            Some(ConfigActionCli::Path) => ConfigAction::Path,
        };
        ConfigArgs {
            action,
            config: self.config,
        }
    }
}

/// Run `octos-tui config`. Returns the process exit code.
pub fn run(args: ConfigArgs) -> Result<i32> {
    let path = match args.config {
        Some(path) => path,
        None => cli::default_config_path()
            .ok_or_else(|| eyre!("no home directory found; pass --config <FILE>"))?,
    };
    match args.action {
        ConfigAction::Path => {
            println!("{}", path.display());
            Ok(0)
        }
        ConfigAction::Show => show(&path),
        ConfigAction::Wizard => wizard(&path),
    }
}

fn show(path: &Path) -> Result<i32> {
    match std::fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => {
            println!(
                "{} is empty. Run `octos-tui config` to set it up.",
                path.display()
            );
            Ok(0)
        }
        Ok(contents) => {
            println!("# {}", path.display());
            println!("{}", contents.trim_end());
            Ok(0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "No config yet at {}.\nRun `octos-tui config` to create one.",
                path.display()
            );
            Ok(0)
        }
        Err(error) => Err(error).wrap_err_with(|| format!("failed to read {}", path.display())),
    }
}

fn wizard(path: &Path) -> Result<i32> {
    if !is_tty() {
        bail!(
            "`octos-tui config` needs an interactive terminal. Run it in a terminal, \
             or edit {} directly.",
            path.display()
        );
    }

    let current = load_current(path)?;
    println!("Configuring octos-tui — saves to {}", path.display());
    println!("Press Enter to skip an option (keep its current or default value).\n");

    let mut answers = serde_json::Map::new();

    // Transport is mutually exclusive (`endpoint` conflicts_with `stdio-command`),
    // so ask once rather than prompting both and risking a conflicting config.
    prompt_transport(&current, &mut answers)?;

    // Everything else, introspected from the clap tree so new flags appear here
    // automatically.
    for arg in cli::cli_command().get_arguments() {
        let Some(key) = arg.get_long() else { continue };
        if SKIP.contains(&key) || arg.is_hide_set() {
            continue;
        }
        let help = arg
            .get_help()
            .map(|help| help.to_string())
            .unwrap_or_default();
        let current_value = current.get(key);
        let answer = if matches!(arg.get_action(), ArgAction::SetTrue) {
            prompt_bool(key, &help, current_value)?
        } else {
            let choices: Vec<String> = arg
                .get_possible_values()
                .iter()
                .map(|value| value.get_name().to_string())
                .collect();
            if choices.is_empty() {
                prompt_string(key, &help, current_value)?
            } else {
                prompt_choice(key, &help, &choices, current_value)?
            }
        };
        if let Some(value) = answer {
            answers.insert(key.to_string(), value);
        }
    }

    if answers.is_empty() {
        println!("\nNothing changed.");
        return Ok(0);
    }
    // Guard against writing a config the launcher would reject: `CliFileConfig`
    // has `deny_unknown_fields`, so this catches a wizard key that drifted from
    // the schema, plus any value with the wrong type, BEFORE we touch the file.
    validate(&answers)?;
    cli::merge_into_config(path, &answers)?;

    let written = answers.iter().filter(|(_, value)| !value.is_null()).count();
    println!("\nSaved {written} setting(s) to {}", path.display());
    println!("Run `octos-tui config show` to review, or just launch `octos-tui`.");
    Ok(0)
}

/// Ask which backend transport to use (mutually exclusive). Sets the chosen key
/// and clears the other (a `null` = remove, per `merge_into_config`).
fn prompt_transport(
    current: &serde_json::Map<String, serde_json::Value>,
    answers: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    // Read the current transport across ALL accepted spellings, so a config
    // using the legacy `stdio_command` / `base-url` forms is shown and defaulted
    // correctly instead of being silently replaced (codex).
    let lookup = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|key| current.get(*key).and_then(|v| v.as_str()))
            .map(str::to_string)
    };
    let current_stdio = lookup(STDIO_KEYS);
    let current_endpoint = lookup(ENDPOINT_KEYS);
    println!("Backend transport — how octos-tui reaches the octos server:");
    println!("  [1] stdio     spawn a local octos server (recommended; auto-installs it)");
    println!("  [2] endpoint  connect to a running server over WebSocket (ws://...)");
    if let Some(stdio) = &current_stdio {
        println!("  current: stdio `{stdio}`");
    } else if let Some(endpoint) = &current_endpoint {
        println!("  current: endpoint {endpoint}");
    }
    // Clear every spelling of the OTHER transport (and any stale `mode`, which
    // `from_args` gives priority over inference — a lingering `mock` would
    // silently ignore the chosen backend). Nulls are removals in
    // `merge_into_config` and are filtered out of schema validation.
    let clear = |answers: &mut serde_json::Map<String, serde_json::Value>, keys: &[&str]| {
        for key in keys {
            answers.insert((*key).into(), serde_json::Value::Null);
        }
        answers.insert("mode".into(), serde_json::Value::Null);
    };
    let choice = read_line("Choose 1/2, or Enter to keep current: ")?;
    match choice.trim() {
        "1" => {
            let default = current_stdio.as_deref().unwrap_or(DEFAULT_STDIO_COMMAND);
            let raw = read_line(&format!("  stdio command [{default}]: "))?;
            let value = if raw.trim().is_empty() {
                default.to_string()
            } else {
                raw.trim().to_string()
            };
            let normalized = parse_stdio_command(&value).map_err(|message| eyre!("{message}"))?;
            answers.insert("stdio-command".into(), normalized.into());
            clear(answers, ENDPOINT_KEYS);
        }
        "2" => {
            let raw = read_line("  WebSocket endpoint (ws://... or wss://...): ")?;
            let value = raw.trim();
            if value.is_empty() {
                println!("  (no endpoint entered — leaving transport unchanged)");
                return Ok(());
            }
            let normalized = parse_websocket_url(value).map_err(|message| eyre!("{message}"))?;
            answers.insert("endpoint".into(), normalized.into());
            clear(answers, STDIO_KEYS);
        }
        _ => {} // keep current
    }
    println!();
    Ok(())
}

fn prompt_bool(
    key: &str,
    help: &str,
    current: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let current_note = match current.and_then(|v| v.as_bool()) {
        Some(value) => format!("  [current: {value}]"),
        None => String::new(),
    };
    println!("{key} — {help}{current_note}");
    let raw = read_line("  yes/no, or Enter to skip: ")?;
    let answer = raw.trim().to_ascii_lowercase();
    Ok(match answer.as_str() {
        "y" | "yes" | "true" | "on" => Some(true.into()),
        "n" | "no" | "false" | "off" => Some(false.into()),
        _ => None,
    })
}

fn prompt_choice(
    key: &str,
    help: &str,
    choices: &[String],
    current: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let current_note = match current.and_then(|v| v.as_str()) {
        Some(value) => format!("  [current: {value}]"),
        None => String::new(),
    };
    println!("{key} — {help}{current_note}");
    for (index, choice) in choices.iter().enumerate() {
        println!("  [{}] {choice}", index + 1);
    }
    let raw = read_line("  choose a number, or Enter to skip: ")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let index: usize = trimmed
        .parse()
        .ok()
        .filter(|n| (1..=choices.len()).contains(n))
        .ok_or_else(|| eyre!("`{trimmed}` is not one of 1..={}", choices.len()))?;
    Ok(Some(choices[index - 1].clone().into()))
}

fn prompt_string(
    key: &str,
    help: &str,
    current: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let current_note = match current.and_then(|v| v.as_str()) {
        Some(value) => format!("  [current: {value}]"),
        None => String::new(),
    };
    println!("{key} — {help}{current_note}");
    let raw = read_line("  value, or Enter to skip: ")?;
    let trimmed = raw.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string().into()))
}

/// Validate the collected answers against the launch schema so we never write a
/// config the launcher would reject. Null entries are removals (not values), and
/// several transport aliases map to one serde field, so they're filtered out
/// first — otherwise deserialization would reject them as duplicate fields.
fn validate(answers: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    let values: serde_json::Map<String, serde_json::Value> = answers
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::from_value::<CliFileConfig>(serde_json::Value::Object(values))
        .map(|_| ())
        .wrap_err("the collected settings don't match the config schema")
}

/// Load the existing config as a raw object (empty if absent/empty), for showing
/// current values. A corrupt file is surfaced rather than silently overwritten.
fn load_current(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    match std::fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => Ok(serde_json::Map::new()),
        Ok(contents) => serde_json::from_str::<serde_json::Value>(&contents)
            .wrap_err_with(|| format!("failed to parse {}", path.display()))?
            .as_object()
            .cloned()
            .ok_or_else(|| eyre!("{} is not a JSON object", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(error).wrap_err_with(|| format!("failed to read {}", path.display())),
    }
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .wrap_err("failed to read input")?;
    Ok(line)
}

fn is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_keys_are_never_prompted() {
        // Every SKIP entry that is a real arg must exist on the command (guards
        // against a renamed flag silently un-skipping a secret/dangerous option).
        let cmd = cli::cli_command();
        let longs: Vec<String> = cmd
            .get_arguments()
            .filter_map(|a| a.get_long().map(String::from))
            .collect();
        for key in [
            "config",
            "no-readonly",
            "auth-token",
            "mode",
            "endpoint",
            "stdio-command",
        ] {
            assert!(
                longs.contains(&key.to_string()),
                "expected arg --{key} to exist"
            );
        }
    }

    #[test]
    fn validate_accepts_known_keys_and_rejects_unknown() {
        let mut ok = serde_json::Map::new();
        ok.insert("theme".into(), "slate".into());
        ok.insert("vim-mode".into(), true.into());
        ok.insert("endpoint".into(), serde_json::Value::Null);
        assert!(validate(&ok).is_ok());

        let mut bad = serde_json::Map::new();
        bad.insert("not-a-key".into(), "x".into());
        assert!(validate(&bad).is_err(), "unknown key must be rejected");

        let mut wrong_type = serde_json::Map::new();
        wrong_type.insert("vim-mode".into(), "definitely-not-a-bool".into());
        assert!(
            validate(&wrong_type).is_err(),
            "wrong value type must be rejected"
        );
    }

    #[test]
    fn wizard_covers_the_expected_options() {
        // The generic loop should reach these curated keys (not in SKIP, visible).
        let cmd = cli::cli_command();
        let prompted: Vec<String> = cmd
            .get_arguments()
            .filter_map(|a| a.get_long().map(String::from))
            .filter(|k| !SKIP.contains(&k.as_str()))
            .collect();
        for key in [
            "session",
            "profile-id",
            "cwd",
            "readonly",
            "theme",
            "lang",
            "scroll-mode",
            "vim-mode",
        ] {
            assert!(
                prompted.contains(&key.to_string()),
                "wizard should prompt --{key}"
            );
        }
    }
}
