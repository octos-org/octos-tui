//! `octos-tui update` — install-method-aware updater (design §A).
//!
//! Behavior by detected [`InstallMethod`]:
//! - **cargo-dist installer** (receipt present): self-update in place via
//!   axoupdater (only when the `update` feature is on; otherwise advise the
//!   one-line installer).
//! - **Homebrew / npm / cargo**: print the exact package-manager command and
//!   exit `3` — we never clobber a binary another tool owns.
//!
//! `--check` is install-method-agnostic: it queries the latest GitHub release
//! for `octos-org/octos-tui`, compares to the compiled-in `CARGO_PKG_VERSION`,
//! prints the result, and exits `10` (update available) or `0` (current).
//!
//! Exit codes (design §A.4): `0` success/up-to-date · `10` update available
//! (scriptable, `--check`) · `3` "can't self-update here, run this" · `1` hard
//! error. The caller (`main`) maps the returned [`UpdateOutcome`] to a process
//! exit code.

use eyre::{Result, WrapErr, eyre};
use semver::Version;

use super::github;
#[cfg(feature = "update")]
use super::github::GITHUB_REPO;
use super::install_method::{InstallMethod, detect};

/// Parsed `octos-tui update` flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateArgs {
    /// Only report whether an update is available; never mutate.
    pub check: bool,
    /// Target a specific semantic version.
    pub version: Option<String>,
    /// Target a specific release tag.
    pub tag: Option<String>,
    /// Allow prerelease targets.
    pub prerelease: bool,
    /// Re-install even if already current.
    pub force: bool,
    /// Skip the interactive confirmation.
    pub yes: bool,
    /// Emit machine-readable JSON.
    pub json: bool,
}

/// Outcome of running `update`, mapped to a process exit code by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// Up to date, or a self-update completed successfully.
    Success,
    /// An update is available (`--check` only).
    UpdateAvailable,
    /// Can't self-update here; the correct command was printed.
    DeferredToPackageManager,
}

impl UpdateOutcome {
    /// The process exit code for this outcome (design §A.4).
    pub fn exit_code(self) -> i32 {
        match self {
            UpdateOutcome::Success => 0,
            UpdateOutcome::UpdateAvailable => 10,
            UpdateOutcome::DeferredToPackageManager => 3,
        }
    }
}

/// Entry point for `octos-tui update`.
pub fn run(args: UpdateArgs) -> Result<UpdateOutcome> {
    let method = detect();
    let current = current_version()?;

    if args.check {
        return run_check(&args, &method, &current);
    }

    match method {
        InstallMethod::CargoDistInstaller => self_update(&args, &current),
        _ => {
            defer_to_package_manager(&method, &args);
            Ok(UpdateOutcome::DeferredToPackageManager)
        }
    }
}

/// `--check`: query GitHub for the latest release, compare, print, exit 10/0.
fn run_check(
    args: &UpdateArgs,
    method: &InstallMethod,
    current: &Version,
) -> Result<UpdateOutcome> {
    let Some(latest) = github::latest_release(args.prerelease)
        .wrap_err("failed to query the latest octos-tui release from GitHub")?
    else {
        // No releases published yet — nothing to compare against; not an error.
        if args.json {
            let payload = serde_json::json!({
                "current_version": current.to_string(),
                "latest_version": serde_json::Value::Null,
                "latest_tag": serde_json::Value::Null,
                "update_available": false,
                "install_method": method.id(),
                "upgrade_command": method.upgrade_command(),
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!("octos-tui {current} — no published releases found yet.");
        }
        return Ok(UpdateOutcome::Success);
    };
    let latest_version = parse_version(&latest.tag)
        .ok_or_else(|| eyre!("could not parse latest release tag `{}`", latest.tag))?;

    let newer = is_newer(current, &latest_version);
    if args.json {
        let payload = serde_json::json!({
            "current_version": current.to_string(),
            "latest_version": latest_version.to_string(),
            "latest_tag": latest.tag,
            "update_available": newer,
            "install_method": method.id(),
            "upgrade_command": method.upgrade_command(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if newer {
        println!(
            "Update available: {current} -> {latest_version} (tag {})",
            latest.tag
        );
        print_method_hint(method);
    } else {
        println!("octos-tui {current} is up to date (latest is {latest_version}).");
    }

    Ok(if newer {
        UpdateOutcome::UpdateAvailable
    } else {
        UpdateOutcome::Success
    })
}

/// Print the install-method-appropriate upgrade hint (used by `--check`).
fn print_method_hint(method: &InstallMethod) {
    match method.upgrade_command() {
        Some(cmd) => println!("  To upgrade ({}):\n    {cmd}", method.label()),
        None => println!("  Run `octos-tui update` to upgrade in place."),
    }
}

/// Print the package-manager command for a non-self-updating install (exit 3).
fn defer_to_package_manager(method: &InstallMethod, args: &UpdateArgs) {
    let cmd = method.upgrade_command().unwrap_or("");
    if args.json {
        let payload = serde_json::json!({
            "install_method": method.id(),
            "self_update": false,
            "upgrade_command": cmd,
            "message": "octos-tui was not installed by the self-updating installer; \
        run the command above with your package manager",
        });
        if let Ok(text) = serde_json::to_string_pretty(&payload) {
            println!("{text}");
        }
        return;
    }

    println!(
        "octos-tui was installed via {}. Self-update is disabled for this build.",
        method.label()
    );
    if matches!(method, InstallMethod::CargoRegistry) {
        println!("To upgrade, run:\n    {cmd}");
        println!("(tip: `cargo install cargo-update` then `cargo install-update octos-tui`)");
    } else {
        println!("To upgrade, run:\n    {cmd}");
    }
}

/// Self-update path for the cargo-dist installer method.
#[cfg(feature = "update")]
fn self_update(args: &UpdateArgs, current: &Version) -> Result<UpdateOutcome> {
    use axoupdater::{AxoUpdater, UpdateRequest};

    let mut updater = AxoUpdater::new_for("octos-tui");
    updater
        .load_receipt()
        .wrap_err("cargo-dist install receipt not found; cannot self-update")?;

    // Honor OCTOS_TUI_GITHUB_TOKEN so rate-limited / private-repo machines don't
    // fail. axoupdater 0.6.9 exposes the public `set_github_token`, so feed the
    // same token the GitHub client uses (no need to mutate process env).
    if let Some(tok) = super::github::token() {
        updater.set_github_token(&tok);
    }

    // In JSON mode, suppress the underlying installer's stdout/stderr chatter so
    // the emitted JSON object is the only thing on stdout (mirrors how `--check`
    // keeps its JSON clean).
    if args.json {
        updater.disable_installer_output();
    }

    // Pin the running version so axoupdater can decide whether an update is
    // needed (the receipt's recorded version may lag a manual swap).
    if let Ok(v) = axoupdater::Version::parse(&current.to_string()) {
        let _ = updater.set_current_version(v);
    }

    let specifier = match (&args.version, &args.tag, args.prerelease) {
        (Some(v), _, _) => UpdateRequest::SpecificVersion(v.clone()),
        (_, Some(t), _) => UpdateRequest::SpecificTag(t.clone()),
        (_, _, true) => UpdateRequest::LatestMaybePrerelease,
        _ => UpdateRequest::Latest,
    };
    updater.configure_version_specifier(specifier);
    updater.always_update(args.force);

    // Pre-flight confirmation (skipped with --yes, in --json mode, or when not a
    // TTY). JSON callers are non-interactive and combine with --yes anyway.
    if !args.yes && !args.json && is_tty() {
        let needed = updater
            .is_update_needed_sync()
            .wrap_err("failed to check whether an update is available")?;
        if !needed && !args.force {
            println!("octos-tui {current} is already up to date.");
            return Ok(UpdateOutcome::Success);
        }
        println!("About to self-update octos-tui from {current} (source: {GITHUB_REPO}).");
        if !confirm("Proceed? [y/N] ")? {
            println!("Aborted.");
            return Ok(UpdateOutcome::Success);
        }
    }

    match updater
        .run_sync()
        .wrap_err("self-update failed (prefix may be unwritable; never sudo-escalate)")?
    {
        Some(result) => {
            let old_version = result
                .old_version
                .map(|v| v.to_string())
                .unwrap_or_else(|| current.to_string());
            if args.json {
                print_self_update_json(true, &old_version, Some(&result.new_version.to_string()));
            } else {
                println!(
                    "Updated octos-tui {} -> {} (tag {}).",
                    old_version, result.new_version, result.new_version_tag,
                );
            }
            // In --json mode, suppress the codesign *success* notice so stdout
            // stays a single valid JSON document; errors still go to stderr.
            codesign_after_swap(result.install_prefix.as_std_path(), args.json);
            Ok(UpdateOutcome::Success)
        }
        None => {
            if args.json {
                print_self_update_json(false, &current.to_string(), None);
            } else {
                println!("octos-tui {current} is already up to date.");
            }
            Ok(UpdateOutcome::Success)
        }
    }
}

/// Emit the machine-readable result of a self-update attempt. When no update
/// happened, `new_version` is `None` and `old_version` doubles as the current
/// version so consumers always see the running version.
#[cfg(feature = "update")]
fn print_self_update_json(updated: bool, old_version: &str, new_version: Option<&str>) {
    let payload = serde_json::json!({
        "updated": updated,
        "old_version": old_version,
        "new_version": new_version,
        "install_method": InstallMethod::CargoDistInstaller.id(),
    });
    if let Ok(text) = serde_json::to_string_pretty(&payload) {
        println!("{text}");
    }
}

/// Advisor-only self-update when the `update` feature is compiled out: detect
/// + print the one-line installer command (matches rustup's `no-self-update`).
#[cfg(not(feature = "update"))]
fn self_update(_args: &UpdateArgs, current: &Version) -> Result<UpdateOutcome> {
    println!(
        "octos-tui {current} was installed by the cargo-dist installer, but this build was \
compiled without in-place self-update (`update` feature off)."
    );
    if let Some(cmd) = InstallMethod::Unknown.upgrade_command() {
        println!("To upgrade, re-run the installer:\n    {cmd}");
    }
    Ok(UpdateOutcome::DeferredToPackageManager)
}

/// macOS: re-codesign the swapped binary so Gatekeeper does not SIGKILL it on
/// Sequoia (replacing the bit-pattern invalidates the prior signature even when
/// bit-identical). No-op on other platforms / on signing failure (best effort).
///
/// `quiet` suppresses the *success* notice (printed to stdout) so a `--json`
/// self-update keeps stdout a single valid JSON document; failures are always
/// reported on stderr regardless.
#[cfg(feature = "update")]
fn codesign_after_swap(install_prefix: &std::path::Path, quiet: bool) {
    #[cfg(target_os = "macos")]
    {
        let binary = install_prefix.join("bin").join("octos-tui");
        let target = if binary.exists() {
            binary
        } else {
            install_prefix.join("octos-tui")
        };
        if !target.exists() {
            return;
        }
        let status = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&target)
            .status();
        match status {
            Ok(s) if s.success() => {
                if !quiet {
                    println!(
                        "Re-signed {} (ad-hoc) for macOS Gatekeeper.",
                        target.display()
                    );
                }
            }
            _ => eprintln!(
                "warning: could not re-codesign {}; if it is SIGKILLed, run: \
codesign --force --sign - {}",
                target.display(),
                target.display()
            ),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = install_prefix;
        let _ = quiet;
    }
}

/// The compiled-in version of this binary.
fn current_version() -> Result<Version> {
    parse_version(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| eyre!("invalid CARGO_PKG_VERSION `{}`", env!("CARGO_PKG_VERSION")))
}

/// Parse a version string, tolerating a leading `v` (release tags are `vX.Y.Z`).
pub fn parse_version(raw: &str) -> Option<Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    Version::parse(trimmed).ok()
}

/// Whether `candidate` is strictly newer than `current` (semver order).
pub fn is_newer(current: &Version, candidate: &Version) -> bool {
    candidate > current
}

#[cfg(feature = "update")]
fn is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[cfg(feature = "update")]
fn confirm(prompt: &str) -> Result<bool> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .wrap_err("failed to read confirmation")?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_with_and_without_v_prefix() {
        assert_eq!(parse_version("1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_version("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_version("  v0.1.1 ").unwrap(), Version::new(0, 1, 1));
        assert!(parse_version("not-a-version").is_none());
    }

    #[test]
    fn is_newer_follows_semver_ordering() {
        let a = Version::new(0, 1, 1);
        assert!(is_newer(&a, &Version::new(0, 1, 2)));
        assert!(is_newer(&a, &Version::new(0, 2, 0)));
        assert!(is_newer(&a, &Version::new(1, 0, 0)));
        assert!(!is_newer(&a, &Version::new(0, 1, 1)));
        assert!(!is_newer(&a, &Version::new(0, 1, 0)));
        assert!(!is_newer(&a, &Version::new(0, 0, 9)));
    }

    #[test]
    fn prerelease_is_older_than_its_release() {
        // 0.2.0-rc.1 < 0.2.0 by semver precedence.
        let rc = parse_version("0.2.0-rc.1").unwrap();
        let rel = Version::new(0, 2, 0);
        assert!(is_newer(&rc, &rel));
        assert!(!is_newer(&rel, &rc));
    }

    #[test]
    fn outcome_exit_codes_match_design() {
        assert_eq!(UpdateOutcome::Success.exit_code(), 0);
        assert_eq!(UpdateOutcome::UpdateAvailable.exit_code(), 10);
        assert_eq!(UpdateOutcome::DeferredToPackageManager.exit_code(), 3);
    }

    #[test]
    fn per_method_commands_are_stable() {
        // Guards the exact strings the update advisor prints.
        assert_eq!(
            InstallMethod::Homebrew.upgrade_command().unwrap(),
            "brew update && brew upgrade octos-org/octos-tui/octos-tui"
        );
        assert_eq!(
            InstallMethod::Npm.upgrade_command().unwrap(),
            "npm update -g @octos-org/octos-tui"
        );
        assert_eq!(
            InstallMethod::CargoRegistry.upgrade_command().unwrap(),
            "cargo install octos-tui --force"
        );
    }
}
