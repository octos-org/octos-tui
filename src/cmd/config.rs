//! `octos-tui config`: read-only inspection of the client's startup config
//! (`~/.config/octos-tui/config.json`).
//!
//! There is no interactive wizard. Configuration is covered by the top-level
//! CLI flags, the in-TUI onboarding, and the runtime toggles (`/theme`,
//! `/lang`, `/vimmode`, `/saveconfig`) — so this command only *inspects*:
//! `show` prints the saved config (and points at the file to edit by hand), and
//! `path` prints the resolved config file path. Bare `octos-tui config` defaults
//! to `show`.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use eyre::{Result, WrapErr, eyre};

use crate::cli;

/// Parsed `octos-tui config …` invocation (the `config` token already stripped
/// by the dispatcher).
#[derive(Debug)]
pub struct ConfigArgs {
    pub action: ConfigAction,
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum ConfigAction {
    Show,
    Path,
}

/// `octos-tui config` flags. Read-only: it inspects the saved config but never
/// writes it (edit the JSON directly, or use the runtime `/…` toggles / CLI
/// flags to change settings).
#[derive(Debug, Parser)]
#[command(
    name = "octos-tui config",
    about = "Inspect octos-tui's saved config (edit the file directly to change it)"
)]
pub struct ConfigCli {
    #[command(subcommand)]
    action: Option<ConfigActionCli>,
    /// Config file to read (default: ~/.config/octos-tui/config.json).
    #[arg(long = "config", value_name = "FILE", global = true)]
    config: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ConfigActionCli {
    /// Print the current saved config (this is the default).
    Show,
    /// Print the resolved config file path.
    Path,
}

impl ConfigCli {
    pub fn into_args(self) -> ConfigArgs {
        let action = match self.action {
            None | Some(ConfigActionCli::Show) => ConfigAction::Show,
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
    }
}

/// Print the saved config, then point the user at the file to change it.
fn show(path: &Path) -> Result<i32> {
    match std::fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => {
            println!("{} is empty.", path.display());
            print_edit_hint(path);
            Ok(0)
        }
        Ok(contents) => {
            println!("# {}", path.display());
            println!("{}", contents.trim_end());
            print_edit_hint(path);
            Ok(0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("No config yet at {}.", path.display());
            print_edit_hint(path);
            Ok(0)
        }
        Err(error) => Err(error).wrap_err_with(|| format!("failed to read {}", path.display())),
    }
}

/// The "how to change settings" hint shown by `show`. There is no interactive
/// wizard, so edit the JSON by hand or use the runtime toggles / CLI flags.
fn print_edit_hint(path: &Path) {
    println!(
        "\nEdit {} directly to change settings, or use the runtime commands \
         /theme, /lang, /vimmode, and /saveconfig (and the octos-tui CLI flags).",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_to_show_when_no_subcommand() {
        let args = ConfigCli::parse_from(["octos-tui config"]).into_args();
        assert!(matches!(args.action, ConfigAction::Show));
    }

    #[test]
    fn should_parse_show_and_path_subcommands() {
        let show = ConfigCli::parse_from(["octos-tui config", "show"]).into_args();
        assert!(matches!(show.action, ConfigAction::Show));
        let path = ConfigCli::parse_from(["octos-tui config", "path"]).into_args();
        assert!(matches!(path.action, ConfigAction::Path));
    }

    #[test]
    fn should_carry_the_config_flag_through() {
        let args = ConfigCli::parse_from(["octos-tui config", "show", "--config", "/tmp/c.json"])
            .into_args();
        assert_eq!(
            args.config.as_deref(),
            Some(std::path::Path::new("/tmp/c.json"))
        );
    }

    #[test]
    fn should_reject_the_removed_wizard_subcommand() {
        // The interactive wizard is gone; only show/path remain. A stray
        // `wizard` token must not parse as a valid subcommand.
        assert!(ConfigCli::try_parse_from(["octos-tui config", "wizard"]).is_err());
    }
}
