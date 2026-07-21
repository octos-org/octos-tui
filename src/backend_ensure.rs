//! Auto-provision the `octos` server backend so a fresh octos-tui install
//! "just works" without a separate manual `octos` download.
//!
//! octos-tui is a *client*: a local launch spawns `octos serve --stdio` as a
//! child (`--stdio-command`). Before the TUI takes over the terminal, this
//! module makes sure `octos` is available and, if it is missing, installs it —
//! **binary-only**. First choice is a package manager (`brew`/`npm`); on Windows,
//! where `brew` never exists and `npm` is often absent on a fresh box, it falls
//! back to downloading the prebuilt server bundle straight from the GitHub
//! release (checksum-verified) into `~\.octos\bin`. We deliberately do NOT run
//! octos's `install.sh` / `install.ps1`: those are server-deployment tools that
//! register a background `octos-serve` service (a `sudo` daemon on Unix, an
//! `OctosServe` scheduled task on Windows), which a stdio client neither needs
//! nor should trigger.
//!
//! We resolve octos against BOTH `PATH` and the legacy installer dir
//! `~/.octos/bin`. When it's usable only in that dir (not on `PATH`), we
//! **rewrite the stdio command to the full path** — octos-tui forbids `unsafe`,
//! so we can't mutate the process `PATH`. A `brew`/`npm` install lands on
//! `PATH`, so that rewrite is mainly for a pre-existing `install.sh` deployment.
//!
//! Scope — it acts on a `Mode::Protocol` launch whose `--stdio-command`'s
//! **leading program** is a bare `octos` (PATH-resolved). Trailing args may
//! carry shell syntax (`--data-dir ~/x`, a Windows `C:\...` path, a pipe): we
//! still probe/install, since only the *rewrite* to an off-PATH path needs
//! round-trippable syntax — and that rewrite bails to a clear "add octos to
//! PATH" error when it can't. An explicit octos path, a `PATH=` override, or a
//! non-octos program is the user's own setup and is left untouched. An octos
//! older than [`MIN_OCTOS_VERSION`] surfaces a clear "please update" error
//! rather than guessing which package manager owns it. Opt out of install with
//! `OCTOS_TUI_NO_AUTO_INSTALL=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{Cli, Mode};
use eyre::{Result, WrapErr, eyre};

/// The minimum `octos` server version this build is known to speak with.
/// octos-tui pins `octos-core` (the UI-Protocol crate) by git rev; this is the
/// released server version carrying a compatible protocol. Bump it alongside
/// the pinned `octos-core` rev whenever the protocol surface moves.
const MIN_OCTOS_VERSION: &str = "1.1.0";

/// Set to any value to disable auto-install (a missing backend then errors).
const OPT_OUT_ENV: &str = "OCTOS_TUI_NO_AUTO_INSTALL";

/// Default Homebrew formula for the octos server, as `<user>/<tap>/<formula>`.
/// This MUST reference the PUBLIC tap `octos-org/tap` (→ `github.com/octos-org/
/// homebrew-tap`). The shorthand `octos-org/octos` instead makes brew auto-tap
/// the PRIVATE `octos-org/homebrew-octos`, whose non-interactive clone dies with
/// `could not read Username`. Override with [`BREW_FORMULA_ENV`].
const DEFAULT_BREW_FORMULA: &str = "octos-org/tap/octos";
/// Env var overriding the Homebrew formula (to install a fork or a local tap).
const BREW_FORMULA_ENV: &str = "OCTOS_TUI_BREW_FORMULA";

/// Default npm package for the octos server. Override with [`NPM_PACKAGE_ENV`].
const DEFAULT_NPM_PACKAGE: &str = "@octos-org/octos";
/// Env var overriding the npm package (to install a fork or from a private registry).
const NPM_PACKAGE_ENV: &str = "OCTOS_TUI_NPM_PACKAGE";

/// The Homebrew formula to install, from [`BREW_FORMULA_ENV`] or the default.
fn brew_formula() -> String {
    env_or(BREW_FORMULA_ENV, DEFAULT_BREW_FORMULA)
}

/// The npm package to install, from [`NPM_PACKAGE_ENV`] or the default.
fn npm_package() -> String {
    env_or(NPM_PACKAGE_ENV, DEFAULT_NPM_PACKAGE)
}

/// A trimmed, non-empty value of env var `key`, else `default`. Keeps the
/// install source out of compiled-in string literals (decoupled) while a
/// blank/whitespace override can't silently break the command.
fn env_or(key: &str, default: &'static str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// Ensure the `octos` backend is present for a stdio launch, rewriting
/// `cli.stdio_command` to an explicit path when octos is usable only in its
/// install dir. Call this BEFORE entering raw mode so installer output prints
/// cleanly.
pub fn ensure_octos_backend(cli: &mut Cli) -> Result<()> {
    // Only the protocol backend spawns `octos serve`; `--mode mock` uses the
    // in-process mock and never launches a child (codex).
    if cli.mode != Mode::Protocol {
        return Ok(());
    }
    let Some(command) = cli.stdio_command.clone() else {
        return Ok(()); // WebSocket launch — no local backend to provision.
    };
    let Some(program) = bare_octos_program(&command) else {
        // Explicit path / PATH override / non-octos — the user's own setup, and
        // not something we can safely probe or rewrite.
        return Ok(());
    };

    match resolve_backend(&program)? {
        // Already on PATH — the bare `octos serve` command works as-is.
        Resolved::OnPath => Ok(()),
        // Usable only in the install dir — rewrite the command to launch it
        // directly, since its dir isn't on this process's PATH.
        Resolved::AtPath(octos) => {
            // On Windows, DON'T rewrite the command to an explicit path. The
            // stdio transport spawns via `cmd /C <command>`, and a path embedded
            // in that string — quoted or not — gets mangled by Rust's arg quoting
            // plus cmd's own quirky quote parsing (the child then dies with exit
            // 1). Instead the transport prepends this install dir to the child's
            // PATH (see `install_bin_dir` / `shell_command`), so the bare `octos`
            // in the command resolves to the exe the auto-installer dropped into
            // `~\.octos\bin`. Nothing to rewrite here — `octos` is bound only for
            // the non-Windows path below.
            if cfg!(windows) {
                let _ = &octos;
                return Ok(());
            }
            let rewritten = rewrite_program(&command, &octos).ok_or_else(|| {
                eyre!(
                    "octos is installed at {} but isn't on PATH, and the launch command uses \
                     shell syntax we can't safely rewrite to that path. Add {} to PATH and \
                     relaunch octos-tui.",
                    octos.display(),
                    octos
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| octos.display().to_string()),
                )
            })?;
            cli.stdio_command = Some(rewritten);
            Ok(())
        }
    }
}

/// A usable octos, either already on `PATH` or at an explicit path we must
/// launch directly.
enum Resolved {
    OnPath,
    AtPath(PathBuf),
}

/// Outcome of probing one candidate octos.
enum Probe {
    /// Runs and is at least [`MIN_OCTOS_VERSION`].
    Ready,
    /// Runs but is older (carries the found version).
    Outdated(String),
    /// Not found.
    Missing,
}

fn opted_out() -> bool {
    std::env::var_os(OPT_OUT_ENV).is_some_and(|v| !v.is_empty())
}

/// Find a usable octos. `program` is the bare name the stdio command runs
/// (always `octos` today) — threaded so the probe targets exactly what the
/// command names rather than a hardcoded string. Tries `PATH` first and, only
/// when that isn't `Ready`, the legacy `~/.octos/bin` — so a stale install-dir
/// binary can't block a working launch. An `Outdated`-only situation asks the
/// user to update; a fully-`Missing` one installs (unless opted out) and
/// re-resolves.
fn resolve_backend(program: &str) -> Result<Resolved> {
    let on_path = probe(Path::new(program));
    // Fast path: a Ready octos on PATH needs no rewrite — and we must NOT probe
    // the legacy dir here. Doing so eagerly runs `~/.octos/bin/octos --version`
    // on every otherwise-working launch, so a stale binary (or one whose
    // `--version` hangs) would block or execute for nothing (codex).
    if matches!(on_path, Probe::Ready) {
        return Ok(Resolved::OnPath);
    }

    // PATH octos isn't usable — now it's worth probing the legacy install dir.
    let dir_octos = install_dir_octos();
    let in_dir = dir_octos.as_ref().map(|p| probe(p));
    if let (Some(dir), Some(Probe::Ready)) = (&dir_octos, &in_dir) {
        return Ok(Resolved::AtPath(dir.clone()));
    }

    // No Ready backend. If either candidate exists but is too old, guide an
    // update — we won't guess which package manager owns an unknown octos.
    if let Probe::Outdated(found) = &on_path {
        return Err(outdated_error(found));
    }
    if let Some(Probe::Outdated(found)) = &in_dir {
        return Err(outdated_error(found));
    }

    // Missing everywhere → install (binary-only) unless opted out.
    if opted_out() {
        return Err(eyre!(
            "octos backend not found and auto-install is disabled ({OPT_OUT_ENV} is set). \
             Install the octos server: `brew install {}` or `npm install -g {}`, or point \
             --endpoint at a running server.",
            brew_formula(),
            npm_package()
        ));
    }
    run_installer()?;

    // Re-resolve. A brew/npm install lands on PATH; a pre-existing install.sh
    // deployment lives in ~/.octos/bin. Require `Ready`, not merely present —
    // an OLDER octos still first on PATH must not be accepted.
    if matches!(probe(Path::new(program)), Probe::Ready) {
        return Ok(Resolved::OnPath);
    }
    if let Some(dir) = install_dir_octos() {
        if matches!(probe(&dir), Probe::Ready) {
            return Ok(Resolved::AtPath(dir));
        }
    }
    Err(eyre!(
        "installed octos, but no working octos >= {MIN_OCTOS_VERSION} is on PATH or in {}. \
         Open a new terminal so PATH picks it up, then relaunch octos-tui.",
        install_dir_octos()
            .and_then(|p| p.parent().map(|d| d.display().to_string()))
            .unwrap_or_else(|| "~/.octos/bin".to_owned())
    ))
}

fn outdated_error(found: &str) -> eyre::Report {
    eyre!(
        "octos {found} is older than the {MIN_OCTOS_VERSION} this octos-tui needs. \
         Update the octos server (`brew upgrade {}` or `npm install -g {}@latest`), \
         then relaunch.",
        brew_formula(),
        npm_package()
    )
}

/// Run `<candidate> --version` and classify it. `octos` (bare) resolves through
/// PATH; a full path probes that file. A present-but-unparseable/erroring
/// binary counts as Ready — don't fight a backend the user clearly has.
///
/// On Windows a bare name may be a `PATHEXT` shim (`.cmd`/`.ps1`, as an npm
/// install ships `octos`) that a direct spawn — which finds only `.exe` —
/// misses, while the stdio transport's `cmd /C` resolves it. We mirror that
/// resolution via `where` so probing classifies the *same* binary the real
/// launch will run, including when an older octos shadows a newer one (codex).
fn probe(candidate: &Path) -> Probe {
    let is_bare = {
        let s = candidate.to_string_lossy();
        !s.contains('/') && !s.contains('\\')
    };
    let output = if cfg!(windows) && is_bare {
        match where_first(candidate) {
            Some(shim) => Command::new("cmd")
                .arg("/C")
                .arg(&shim)
                .arg("--version")
                .output(),
            None => return Probe::Missing,
        }
    } else {
        Command::new(candidate).arg("--version").output()
    };
    match output {
        Ok(output) if output.status.success() => {
            match parse_octos_version(&String::from_utf8_lossy(&output.stdout)) {
                Some(found) if version_lt(&found, MIN_OCTOS_VERSION) => Probe::Outdated(found),
                _ => Probe::Ready,
            }
        }
        Ok(_) => Probe::Ready,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Probe::Missing,
        // Any other spawn error (permissions, etc.): assume present and let the
        // real launch surface a precise error rather than triggering an install.
        Err(_) => Probe::Ready,
    }
}

/// The first path `where <name>` resolves on Windows — the same PATH+PATHEXT
/// order `cmd /C` (the stdio transport) uses — or `None` when it isn't found.
/// Windows-only; on other platforms `probe`/`have` never call it.
fn where_first(name: &Path) -> Option<PathBuf> {
    let out = Command::new("where").arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
}

/// The octos binary the legacy `install.sh` writes: `$OCTOS_PREFIX/octos` or
/// `~/.octos/bin/octos` (`octos.exe` on Windows). `None` if no home dir.
fn install_dir_octos() -> Option<PathBuf> {
    let dir = match std::env::var_os("OCTOS_PREFIX") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => home_dir()?.join(".octos").join("bin"),
    };
    let name = if cfg!(windows) { "octos.exe" } else { "octos" };
    Some(dir.join(name))
}

/// The directory the auto-installer drops `octos` into (`$OCTOS_PREFIX` or
/// `~/.octos/bin`). The stdio transport prepends this to the child's PATH so a
/// bare `octos` in the launch command resolves to the auto-installed exe —
/// without embedding a path in the command string, which `cmd /C` mangles on
/// Windows. `None` if no home dir.
pub(crate) fn install_bin_dir() -> Option<PathBuf> {
    install_dir_octos().and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Home directory, treating an empty `HOME` as absent so the Windows
/// `USERPROFILE` fallback still applies (codex).
fn home_dir() -> Option<PathBuf> {
    let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    non_empty("HOME")
        .or_else(|| non_empty("USERPROFILE"))
        .map(PathBuf::from)
}

/// The program a `--stdio-command` runs, IFF it is a **bare** `octos` (no path
/// separator) resolved through `PATH`. Handles a leading `env` + `VAR=value`
/// assignments and an optional `stdio:` transport-label prefix. This decides
/// only whether we may *probe/install* the backend, so it inspects just the
/// leading executable — trailing args carrying shell syntax (a `--data-dir
/// ~/x`, a pipe, or a Windows `C:\...` path) must NOT disqualify provisioning
/// (codex); round-trip safety is enforced separately, at the rewrite step.
/// Returns `None` for an explicit path, a `PATH=` override (the child would
/// resolve `octos` against a different search path than we probe), or a
/// non-octos program.
fn bare_octos_program(command: &str) -> Option<String> {
    let command = command.trim();
    let command = command.strip_prefix("stdio:").unwrap_or(command).trim();
    // Split on whitespace to find the leading executable. We deliberately do
    // NOT shlex-parse here: we need only the program token, and an unquoted
    // Windows path arg (`--data-dir C:\Users\x`) would trip POSIX backslash
    // escaping and drop the token entirely.
    let mut iter = command.split_whitespace();
    let mut program = iter.next()?;
    if program == "env" {
        program = iter.next()?;
    }
    // Skip `KEY=value` assignments before the program. A `PATH=` override means
    // the child resolves `octos` against a different search path than we can
    // probe from this process — treat the whole command as user-managed.
    while is_env_assignment(program) {
        if program.split_once('=').is_some_and(|(k, _)| k == "PATH") {
            return None;
        }
        program = iter.next()?;
    }
    if program.contains('/') || program.contains('\\') {
        return None; // explicit path — user's own setup
    }
    // Only a bare `octos` is our canonical, provisionable form. We deliberately
    // do NOT accept `octos.exe`: it's never the canonical command (bare `octos`
    // is, and Windows `cmd /C` resolves it to whatever `.exe`/`.cmd` exists),
    // and npm — our Windows installer — ships an `octos.cmd` shim, never an
    // `octos.exe`, so an `octos.exe` command isn't reliably provisionable
    // anyway. Such a command is left to the user (codex).
    (program == "octos").then(|| program.to_owned())
}

/// Shell metacharacters whose presence means split+rejoin (and `sh -c`
/// re-parsing) would not faithfully preserve the command.
const SHELL_METACHARS: &[char] = &[
    '$', '`', '|', '&', ';', '<', '>', '(', ')', '*', '?', '[', ']', '{', '}', '~', '!', '\\',
    '\n', '\r',
];

/// `KEY=value` with a non-empty, path-free key (so `/opt/x=y` or a bare program
/// isn't mistaken for an assignment).
fn is_env_assignment(token: &str) -> bool {
    token
        .split_once('=')
        .is_some_and(|(k, _)| !k.is_empty() && !k.contains('/') && !k.contains('\\'))
}

/// Rewrite a bare-`octos` stdio command to launch `octos_path` explicitly,
/// preserving a leading `stdio:` prefix, an `env` prefix, `KEY=value`
/// assignments, and all trailing args. Returns `None` — so the caller surfaces
/// an actionable "add octos to PATH" error instead of a mangled command — when
/// the command carries shell syntax the split+rejoin round-trip can't preserve
/// (`$PWD` would become a literal, a `~` would stop expanding, a pipe would be
/// quoted into an argument). Unix-only in practice: the Windows caller errors
/// before reaching here, since `cmd /C` won't honor this POSIX quoting anyway.
fn rewrite_program(command: &str, octos_path: &Path) -> Option<String> {
    let trimmed = command.trim();
    let (prefix, body) = match trimmed.strip_prefix("stdio:") {
        Some(rest) => ("stdio:", rest.trim()),
        None => ("", trimmed),
    };
    if body.contains(SHELL_METACHARS) {
        return None;
    }
    let mut tokens = shlex::split(body)?;
    let has_env_keyword = tokens.first().is_some_and(|t| t == "env");
    let mut idx = usize::from(has_env_keyword);
    let assignments_start = idx;
    while tokens.get(idx).is_some_and(|t| is_env_assignment(t)) {
        idx += 1;
    }
    *tokens.get_mut(idx)? = octos_path.to_string_lossy().into_owned();
    // A DIRECT `VAR=value` prefix (no leading `env`) is fine as typed, but
    // `try_join` re-quotes it (`'VAR=value'`), and `sh -c` then treats the
    // quoted token as the *command name* rather than an assignment — so the
    // backend never launches. Prepend `env` so the (re-quoted) assignments are
    // parsed as `env`'s own args instead (codex). A leading `env` already does
    // this; no assignments → nothing to protect.
    if !has_env_keyword && idx > assignments_start {
        tokens.insert(0, "env".to_owned());
    }
    let joined = shlex::try_join(tokens.iter().map(String::as_str)).ok()?;
    Some(format!("{prefix}{joined}"))
}

/// Pull the first `X.Y.Z` token out of `octos --version` output, e.g.
/// `octos 1.1.0 (79c19f6d4 2026-07-11)` → `1.1.0`.
fn parse_octos_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|tok| {
        let core = tok.trim_start_matches('v');
        let mut parts = core.split('.');
        let ok = [parts.next(), parts.next(), parts.next()]
            .iter()
            .all(|p| p.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
            && parts.next().is_none();
        ok.then(|| core.to_owned())
    })
}

/// `a < b` for dotted numeric versions (`1.2.0 < 1.10.0`). Unparseable segments
/// compare as 0.
fn version_lt(a: &str, b: &str) -> bool {
    let nums = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (nums(a), nums(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x < y;
        }
    }
    false
}

/// A package-manager install command: `program` + `args`, with `how` naming the
/// manager for user-facing messages.
struct InstallPlan {
    program: &'static str,
    args: Vec<String>,
    how: &'static str,
}

/// Choose the install command from package-manager availability and the
/// (possibly overridden) identifiers. Pure — takes availability + identifiers as
/// args, reading no env and probing nothing — so tests can assert the exact
/// command without brew/npm installed. `brew` is preferred; `None` means neither
/// manager is available.
fn installer_plan(
    has_brew: bool,
    has_npm: bool,
    brew_formula: &str,
    npm_package: &str,
) -> Option<InstallPlan> {
    if has_brew {
        Some(InstallPlan {
            program: "brew",
            args: vec!["install".to_owned(), brew_formula.to_owned()],
            how: "brew",
        })
    } else if has_npm {
        Some(InstallPlan {
            program: "npm",
            args: vec![
                "install".to_owned(),
                "-g".to_owned(),
                npm_package.to_owned(),
            ],
            how: "npm",
        })
    } else {
        None
    }
}

/// Install octos **binary-only** via a package manager (never `install.sh`,
/// which sets up a system service). Prefers `brew` (the [`DEFAULT_BREW_FORMULA`]
/// tap), then `npm` ([`DEFAULT_NPM_PACKAGE`]) — both env-overridable. Errors with
/// actionable guidance when neither is available. Inherits stdio so progress
/// prints (called pre-raw-mode).
fn run_installer() -> Result<()> {
    let (brew, npm) = (brew_formula(), npm_package());
    let Some(plan) = installer_plan(have("brew"), have("npm"), &brew, &npm) else {
        // No package manager. On Windows — where `brew` never exists and `npm`
        // is often absent on a fresh box — download the prebuilt server bundle
        // directly (binary-only, never a service). Elsewhere, guide the user.
        if cfg!(windows) {
            return install_octos_windows_bundle();
        }
        return Err(eyre!(
            "octos server not found and no supported installer (brew or npm) is available. \
             Install octos (binary only, no service) with one of:\n  \
             brew install {brew}\n  npm install -g {npm}\n\
             then relaunch octos-tui (or set --endpoint to a running server)."
        ));
    };
    eprintln!(
        "octos-tui: octos backend not found; installing the octos server via {} \
         (set {OPT_OUT_ENV}=1 to skip)...",
        plan.how
    );
    // On Windows `brew`/`npm` are `.cmd` shims, which a direct spawn can't
    // execute; run them through `cmd /C` like the stdio transport does (codex).
    let status = if cfg!(windows) {
        Command::new("cmd")
            .arg("/C")
            .arg(plan.program)
            .args(&plan.args)
            .status()
    } else {
        Command::new(plan.program).args(&plan.args).status()
    }
    .map_err(|err| eyre!("failed to launch {}: {err}", plan.program))?;
    if !status.success() {
        return Err(eyre!(
            "{} could not install octos ({status}). Install the octos server manually \
             (https://github.com/octos-org/octos) and relaunch.",
            plan.program
        ));
    }
    Ok(())
}

/// Whether `program` is available (a cheap presence check). On Windows, `brew`
/// and `npm` ship as `PATHEXT` shims (`.cmd`/`.ps1`), so we resolve with `where`
/// — mirroring the `cmd /C` we install through — since a direct `--version`
/// spawn finds only `.exe` and would report a present npm as missing (codex).
fn have(program: &str) -> bool {
    if cfg!(windows) {
        Command::new("where")
            .arg(program)
            .output()
            .is_ok_and(|o| o.status.success())
    } else {
        Command::new(program)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// The prebuilt octos server bundle for 64-bit Windows, served from the latest
/// GitHub release. `7z a <zip> *` archives the build dir's contents, so
/// `octos.exe` sits at the archive root beside its bundled skills.
const WINDOWS_BUNDLE_URL: &str = "https://github.com/octos-org/octos/releases/latest/download/octos-bundle-x86_64-pc-windows-msvc.zip";
/// Published SHA-256 for [`WINDOWS_BUNDLE_URL`] (a `"<hex>  <name>"` line).
const WINDOWS_BUNDLE_SHA256_URL: &str = "https://github.com/octos-org/octos/releases/latest/download/octos-bundle-x86_64-pc-windows-msvc.zip.sha256";

/// Windows fallback when neither `brew` nor `npm` is available (the common case
/// on a fresh Windows box): download the prebuilt octos server bundle and
/// extract it into the install dir, **binary-only — never a service** (unlike
/// `install.ps1`, which registers an `OctosServe` scheduled task). Verifies the
/// published SHA-256 before extracting an executable we are about to run, then
/// places `octos.exe` (and its sibling bundled skills) under `~\.octos\bin` (or
/// `OCTOS_PREFIX`). `resolve_backend` re-probes that dir and the caller rewrites
/// the stdio command to the full path. Called pre-raw-mode so progress prints.
fn install_octos_windows_bundle() -> Result<()> {
    let octos = install_dir_octos().ok_or_else(|| {
        eyre!("cannot determine the octos install directory (no HOME/USERPROFILE set)")
    })?;
    let install_dir = octos
        .parent()
        .ok_or_else(|| eyre!("octos install path {} has no parent", octos.display()))?
        .to_path_buf();

    eprintln!(
        "octos-tui: octos backend not found and no brew/npm available; downloading the \
         octos server bundle (set {OPT_OUT_ENV}=1 to skip)..."
    );

    let bytes = http_get_bytes(WINDOWS_BUNDLE_URL)
        .wrap_err("failed to download the octos server bundle")?;

    // Integrity-check before extracting an executable we're about to run. A
    // missing checksum (older releases) warns but doesn't hard-fail; a mismatch
    // does.
    match http_get_string(WINDOWS_BUNDLE_SHA256_URL) {
        Ok(published) => verify_sha256(&bytes, &published)?,
        Err(err) => eprintln!(
            "octos-tui: could not fetch the bundle checksum ({err}); skipping verification"
        ),
    }

    extract_octos_bundle(&bytes, &install_dir)
        .wrap_err("failed to extract the octos server bundle")?;

    eprintln!(
        "octos-tui: installed the octos server to {}",
        install_dir.display()
    );
    Ok(())
}

/// Blocking GET returning the response body bytes, erroring on non-2xx. Follows
/// GitHub's release-asset redirect (reqwest follows up to 10 by default).
fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    Ok(http_client()?
        .get(url)
        .send()?
        .error_for_status()?
        .bytes()?
        .to_vec())
}

/// Blocking GET returning the response body as text, erroring on non-2xx.
fn http_get_string(url: &str) -> Result<String> {
    Ok(http_client()?.get(url).send()?.error_for_status()?.text()?)
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("octos-tui/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(Into::into)
}

/// Verify `bytes` against a published `.sha256` line (`"<64-hex>  <filename>"`;
/// the leading hex token is all we need). Case-insensitive.
fn verify_sha256(bytes: &[u8], published: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let expected = published
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(eyre!(
            "unexpected octos bundle checksum format: {published:?}"
        ));
    }
    let actual = hex_lower(&Sha256::digest(bytes));
    if actual != expected {
        return Err(eyre!(
            "octos bundle checksum mismatch (expected {expected}, got {actual}); \
             refusing to install"
        ));
    }
    Ok(())
}

/// Lowercase hex encoding (avoids a `hex` crate dep for one call site).
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Extract the octos bundle zip into `install_dir`, binary-only. Stages into a
/// temp dir first (a partial extract never leaves a broken install), finds
/// `octos.exe` anywhere in the tree (mirrors octos's own `deploy.ps1`), then
/// copies that bundle root's files next to it under `install_dir`. `zip`'s
/// `enclosed_name` drops zip-slip (`..`) paths.
fn extract_octos_bundle(zip_bytes: &[u8], install_dir: &Path) -> Result<()> {
    let staging = tempfile::tempdir()?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue; // skip unsafe (`..`/absolute) entries
        };
        let out = staging.path().join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }

    let exe = find_file_named(staging.path(), "octos.exe")
        .ok_or_else(|| eyre!("octos.exe not found in the downloaded bundle"))?;
    let bundle_root = exe.parent().unwrap_or_else(|| staging.path());
    std::fs::create_dir_all(install_dir)?;
    copy_dir_contents(bundle_root, install_dir)?;

    // Sanity: the file the probe will look for must now exist.
    let placed = install_dir.join("octos.exe");
    if !placed.exists() {
        return Err(eyre!(
            "extracted the bundle but {} is missing",
            placed.display()
        ));
    }
    Ok(())
}

/// First file named `name` (case-insensitive) anywhere under `root`, depth-first.
fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            dirs.push(path);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            return Some(path);
        }
    }
    dirs.into_iter().find_map(|d| find_file_named(&d, name))
}

/// Recursively copy the files/subdirs directly inside `src` into `dst`.
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_contents(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_bin_dir_is_the_parent_of_the_probed_exe() {
        // `install_bin_dir` (used by the transport to augment the child PATH)
        // must be exactly the directory the exe is probed/installed in.
        let exe = install_dir_octos();
        let dir = install_bin_dir();
        match (exe, dir) {
            (Some(exe), Some(dir)) => assert_eq!(exe.parent(), Some(dir.as_path())),
            (None, None) => {} // no HOME/USERPROFILE in this env — both absent
            other => panic!("exe/dir presence mismatch: {other:?}"),
        }
    }

    #[test]
    fn verify_sha256_matches_and_rejects() {
        let data = b"octos-bundle-bytes";
        let good = {
            use sha2::{Digest, Sha256};
            hex_lower(&Sha256::digest(data))
        };
        // Real `.sha256` files are `"<hex>  <filename>"` — the trailing name must
        // not matter.
        verify_sha256(
            data,
            &format!("{good}  octos-bundle-x86_64-pc-windows-msvc.zip"),
        )
        .expect("matching checksum should pass");
        assert!(
            verify_sha256(data, &"0".repeat(64)).is_err(),
            "mismatch must fail"
        );
        assert!(
            verify_sha256(data, "not-a-checksum").is_err(),
            "bad format must fail"
        );
    }

    #[test]
    fn extract_octos_bundle_places_exe_and_siblings() {
        // Build an in-memory zip laid out like the real bundle (flat, `octos.exe`
        // at the root beside a bundled-skill file) and extract it.
        use std::io::Write as _;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            w.start_file("octos.exe", opts).unwrap();
            w.write_all(b"MZ fake octos").unwrap();
            w.start_file("skills/weather/main", opts).unwrap();
            w.write_all(b"#!skill").unwrap();
            w.finish().unwrap();
        }
        let dst = tempfile::tempdir().unwrap();
        extract_octos_bundle(&buf, dst.path()).expect("extraction should succeed");
        assert!(dst.path().join("octos.exe").exists(), "octos.exe placed");
        assert!(
            dst.path().join("skills/weather/main").exists(),
            "bundled skill placed beside it"
        );
    }

    #[test]
    fn extract_octos_bundle_finds_exe_under_a_top_level_dir() {
        // Robust to a future layout that nests everything under a top dir.
        use std::io::Write as _;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            w.start_file("octos-bundle/octos.exe", opts).unwrap();
            w.write_all(b"MZ").unwrap();
            w.start_file("octos-bundle/skills/x", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let dst = tempfile::tempdir().unwrap();
        extract_octos_bundle(&buf, dst.path()).expect("extraction should succeed");
        assert!(dst.path().join("octos.exe").exists());
        assert!(dst.path().join("skills/x").exists());
    }

    #[test]
    fn extract_octos_bundle_errors_without_an_exe() {
        use std::io::Write as _;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            w.start_file("readme.txt", opts).unwrap();
            w.write_all(b"no exe here").unwrap();
            w.finish().unwrap();
        }
        let dst = tempfile::tempdir().unwrap();
        assert!(extract_octos_bundle(&buf, dst.path()).is_err());
    }

    #[test]
    fn bare_octos_program_matches_the_standard_shapes() {
        for cmd in [
            "octos serve --stdio --solo",
            "  octos serve --stdio  ",
            "stdio:octos serve --stdio",
            "env OCTOS_FOO=1 DEEPSEEK_API_KEY=sk octos serve --stdio",
            "FOO=1 octos serve",
            // Shell syntax in *arguments* must NOT disqualify provisioning — we
            // only need the leading program to probe/install (codex).
            "octos serve --stdio --solo --data-dir ~/.octos-tui-data",
            "octos serve --stdio --data-dir C:\\Users\\admin\\data",
            "octos serve --stdio | tee log",
            "octos serve && echo done",
            "OCTOS_HOME=\"$PWD/.octos\" octos serve",
        ] {
            assert_eq!(
                bare_octos_program(cmd).as_deref(),
                Some("octos"),
                "should extract bare octos from: {cmd}"
            );
        }
    }

    #[test]
    fn bare_octos_program_skips_explicit_paths_shell_syntax_and_others() {
        for cmd in [
            "/usr/local/bin/octos serve --stdio", // explicit path — user-managed
            "$HOME/.local/bin/octos serve --stdio", // path (leading program) — user-managed
            "./octos serve",                      // explicit path
            "my-custom-backend --stdio",          // not octos
            "env A=1 my-backend serve",           // not octos
            "octos.exe serve --stdio",            // not canonical; npm can't provision .exe
            "env PATH=/custom/bin:$PATH octos serve", // PATH override — can't probe same octos
            "PATH=/opt/octos/bin octos serve",    // leading PATH override
        ] {
            assert_eq!(
                bare_octos_program(cmd),
                None,
                "should NOT auto-manage: {cmd}"
            );
        }
    }

    #[test]
    fn rewrite_program_swaps_the_octos_token_only() {
        let p = Path::new("/home/u/.octos/bin/octos");
        assert_eq!(
            rewrite_program("octos serve --stdio --solo", p).as_deref(),
            Some("/home/u/.octos/bin/octos serve --stdio --solo")
        );
        // env prefix + assignment preserved (shlex may re-quote `A=1`, which is
        // shell-equivalent since the command is re-parsed by `sh -c`).
        let rewritten = rewrite_program("env A=1 octos serve --stdio", p).unwrap();
        assert_eq!(
            shlex::split(&rewritten).unwrap(),
            ["env", "A=1", "/home/u/.octos/bin/octos", "serve", "--stdio"]
        );
        assert_eq!(
            rewrite_program("stdio:octos serve", p).as_deref(),
            Some("stdio:/home/u/.octos/bin/octos serve")
        );
        // A path containing a space is re-quoted so it stays one arg.
        let spaced = Path::new("/home/a b/.octos/bin/octos");
        assert_eq!(
            rewrite_program("octos serve", spaced).as_deref(),
            Some("'/home/a b/.octos/bin/octos' serve")
        );
        // A DIRECT assignment prefix (no `env` keyword) gains one, so `sh -c`
        // keeps it an assignment instead of reading the re-quoted token as a
        // command name (codex).
        let rewritten = rewrite_program("OCTOS_HOME=/data octos serve", p).unwrap();
        assert_eq!(
            shlex::split(&rewritten).unwrap(),
            [
                "env",
                "OCTOS_HOME=/data",
                "/home/u/.octos/bin/octos",
                "serve"
            ]
        );
        // Shell syntax we can't round-trip → None, so the caller errors with an
        // "add octos to PATH" message rather than emitting a mangled command.
        for cmd in [
            "octos serve --data-dir ~/data",          // ~ would stop expanding
            "OCTOS_HOME=\"$PWD/.octos\" octos serve", // $PWD would become literal
            "octos serve | tee log",                  // pipe quoted into an argument
            "octos serve && echo done",               // control operator
        ] {
            assert_eq!(
                rewrite_program(cmd, p),
                None,
                "should refuse to rewrite: {cmd}"
            );
        }
    }

    #[test]
    fn parse_octos_version_extracts_semver() {
        assert_eq!(
            parse_octos_version("octos 1.1.0 (79c19f6d4 2026-07-11)").as_deref(),
            Some("1.1.0")
        );
        assert_eq!(
            parse_octos_version("octos v2.10.3\n").as_deref(),
            Some("2.10.3")
        );
        assert_eq!(parse_octos_version("no version here"), None);
        assert_eq!(parse_octos_version("octos 1.2.3.4"), None); // 4-part isn't X.Y.Z
    }

    #[test]
    fn version_lt_is_numeric_not_lexical() {
        assert!(version_lt("1.1.0", "1.2.0"));
        assert!(version_lt("1.2.0", "1.10.0")); // NOT lexical ("2" < "10")
        assert!(version_lt("0.9.9", "1.0.0"));
        assert!(!version_lt("1.1.0", "1.1.0"));
        assert!(!version_lt("2.0.0", "1.9.9"));
        assert!(!version_lt("1.1.0", "1.1")); // 1.1.0 == 1.1(.0)
    }

    #[test]
    fn installer_plan_brew_uses_the_public_tap_not_the_private_repo() {
        // The default brew formula MUST be the PUBLIC octos-org/tap: the
        // shorthand octos-org/octos auto-taps the PRIVATE homebrew-octos, whose
        // non-interactive clone fails with `could not read Username`.
        let plan = installer_plan(true, false, DEFAULT_BREW_FORMULA, DEFAULT_NPM_PACKAGE)
            .expect("brew available → a plan");
        assert_eq!(plan.program, "brew");
        assert_eq!(plan.args, ["install", "octos-org/tap/octos"]);
        assert_eq!(plan.how, "brew");
    }

    #[test]
    fn installer_plan_prefers_brew_then_npm_then_none() {
        // npm fallback when brew is absent.
        let plan = installer_plan(false, true, DEFAULT_BREW_FORMULA, DEFAULT_NPM_PACKAGE)
            .expect("npm available → a plan");
        assert_eq!(plan.program, "npm");
        assert_eq!(plan.args, ["install", "-g", "@octos-org/octos"]);
        assert_eq!(plan.how, "npm");
        // brew wins when both are present.
        let both = installer_plan(true, true, DEFAULT_BREW_FORMULA, DEFAULT_NPM_PACKAGE).unwrap();
        assert_eq!(both.program, "brew");
        // Neither manager → no plan (caller prints manual-install guidance).
        assert!(installer_plan(false, false, DEFAULT_BREW_FORMULA, DEFAULT_NPM_PACKAGE).is_none());
    }

    #[test]
    fn installer_plan_threads_overridden_identifiers_into_the_command() {
        // Decoupled identifiers flow straight into the command, so an operator
        // can retarget a fork/local tap or registry without a rebuild.
        let brew = installer_plan(true, false, "acme/tap/octos", "@acme/octos").unwrap();
        assert_eq!(brew.args, ["install", "acme/tap/octos"]);
        let npm = installer_plan(false, true, "acme/tap/octos", "@acme/octos").unwrap();
        assert_eq!(npm.args, ["install", "-g", "@acme/octos"]);
    }

    #[test]
    fn env_or_falls_back_to_default_when_unset() {
        // An env var we never set → the baked-in default. (Read-only: octos-tui
        // forbids `unsafe`, so tests can't set_var to exercise the override; the
        // override path is covered via installer_plan's identifier params above.)
        assert_eq!(
            env_or("OCTOS_TUI_UNSET_ENV_XYZZY_12345", "the-default"),
            "the-default"
        );
    }
}
