//! Auto-provision the `octos` server backend so a fresh octos-tui install
//! "just works" without a separate manual `octos` download.
//!
//! octos-tui is a *client*: a local launch spawns `octos serve --stdio` as a
//! child (`--stdio-command`). Before the TUI takes over the terminal, this
//! module makes sure `octos` is available and, if it is missing, installs it —
//! **binary-only via a package manager** (`brew`/`npm`). We deliberately do NOT
//! run octos's `install.sh`: that is a server-deployment tool that registers +
//! starts an `octos-serve` system service via `sudo`, which a stdio client
//! neither needs nor should trigger (a password prompt / non-zero exit).
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
use eyre::{Result, eyre};

/// The minimum `octos` server version this build is known to speak with.
/// octos-tui pins `octos-core` (the UI-Protocol crate) by git rev; this is the
/// released server version carrying a compatible protocol. Bump it alongside
/// the pinned `octos-core` rev whenever the protocol surface moves.
const MIN_OCTOS_VERSION: &str = "1.1.0";

/// Set to any value to disable auto-install (a missing backend then errors).
const OPT_OUT_ENV: &str = "OCTOS_TUI_NO_AUTO_INSTALL";

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
            // On Windows the stdio transport runs the command via `cmd /C`, but
            // `rewrite_program` re-serializes with POSIX (shlex) quoting, which
            // cmd mangles (codex). This off-PATH rewrite is a Unix-only legacy
            // (`install.sh`) case; on Windows, ask the user to fix PATH.
            if cfg!(windows) {
                return Err(eyre!(
                    "octos is installed at {} but isn't on PATH. Add its directory to PATH \
                     and relaunch octos-tui.",
                    octos.display()
                ));
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
             Install the octos server: `brew install octos-org/octos/octos` or \
             `npm install -g @octos-org/octos`, or point --endpoint at a running server."
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
         Update the octos server — `brew upgrade octos-org/octos/octos` or \
         `npm install -g @octos-org/octos@latest` — then relaunch."
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

/// Install octos **binary-only** via a package manager (never `install.sh`,
/// which sets up a system service). Prefers `brew` (auto-taps
/// `octos-org/homebrew-tap`), then `npm`. Errors with actionable guidance when
/// neither is available. Inherits stdio so progress prints (called pre-raw-mode).
fn run_installer() -> Result<()> {
    let plan: Option<(&str, &[&str], &str)> = if have("brew") {
        Some(("brew", &["install", "octos-org/octos/octos"], "brew"))
    } else if have("npm") {
        Some(("npm", &["install", "-g", "@octos-org/octos"], "npm"))
    } else {
        None
    };
    let Some((program, args, how)) = plan else {
        return Err(eyre!(
            "octos server not found and no supported installer (brew or npm) is available. \
             Install octos (binary only, no service) with one of:\n  \
             brew install octos-org/octos/octos\n  npm install -g @octos-org/octos\n\
             then relaunch octos-tui (or set --endpoint to a running server)."
        ));
    };
    eprintln!(
        "octos-tui: octos backend not found — installing the octos server via {how} \
         (set {OPT_OUT_ENV}=1 to skip)…"
    );
    // On Windows `brew`/`npm` are `.cmd` shims, which a direct spawn can't
    // execute; run them through `cmd /C` like the stdio transport does (codex).
    let status = if cfg!(windows) {
        Command::new("cmd")
            .arg("/C")
            .arg(program)
            .args(args)
            .status()
    } else {
        Command::new(program).args(args).status()
    }
    .map_err(|err| eyre!("failed to launch {program}: {err}"))?;
    if !status.success() {
        return Err(eyre!(
            "{program} could not install octos ({status}). Install the octos server manually \
             (https://github.com/octos-org/octos) and relaunch."
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
