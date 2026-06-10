//! `octos-tui doctor` — flutter-doctor-style diagnostics (design §B).
//!
//! One line per check (`[✓]` pass / `[!]` warn / `[✗]` fail), grouped by
//! category, each non-pass line followed by an indented `→ fix:` action,
//! closing with a one-line summary. `--json` emits the same data structured
//! (support bundle; tokens redacted); `--verbose` adds resolved paths/versions.
//!
//! Exit `0` when all checks pass (warnings are OK but mentioned), `1` on any
//! `[✗]`. `--strict` promotes warnings to failures.
//!
//! Checks implemented here:
//! - **Binary & version**: octos-tui on PATH, install method, newer release,
//!   shadowing installs.
//! - **Terminal**: TERM/terminfo, UTF-8 locale, CJK width, color support.
//! - **Config & data**: config dir + data dir writability.
//! - **Backend**: stdio-command resolves (+ `octos --version`), and a
//!   structural **protocol-skew** comparison of the TUI's compiled-in
//!   `octos-core` schema/feature set against the protocol's known feature
//!   registry. The live WS `config/capabilities/list` probe is a documented
//!   TODO (see [`backend_checks`]).
//! - **Network**: GitHub reachability.

use std::path::{Path, PathBuf};

use eyre::Result;
use octos_core::ui_protocol::{
    UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1, UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1,
    UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1, UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1, UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1,
    UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1, UI_PROTOCOL_FEATURE_SESSION_HYDRATE_V1,
    UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1, UI_PROTOCOL_FEATURE_USER_QUESTION_V1,
    UI_PROTOCOL_KNOWN_FEATURES, UI_PROTOCOL_SCHEMA_VERSION, UI_PROTOCOL_V1, UiProtocolCapabilities,
};

use super::github::{self, Reachability};
use super::install_method::{self, InstallMethod};

/// Features the TUI *requires* of any server it connects to (the set it sends
/// in `X-Octos-Ui-Features`). The skew check fails when the server's schema is
/// incompatible and warns when a required feature is missing.
pub const TUI_REQUIRED_FEATURES: &[&str] = &[
    UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1,
    UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1,
    UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1,
    UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1,
    UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1,
    UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1,
    UI_PROTOCOL_FEATURE_SESSION_HYDRATE_V1,
    UI_PROTOCOL_FEATURE_USER_QUESTION_V1,
];

/// Parsed `octos-tui doctor` flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorArgs {
    /// Emit machine-readable JSON (support bundle).
    pub json: bool,
    /// Add resolved paths / versions to each line.
    pub verbose: bool,
    /// Promote warnings to failures (affects exit code).
    pub strict: bool,
    /// stdio child command, if the TUI is configured for stdio transport.
    pub stdio_command: Option<String>,
    /// WS endpoint, if configured.
    pub endpoint: Option<String>,
    /// Data dir override (defaults to `~/.octos`).
    pub data_dir: Option<PathBuf>,
}

/// Pass / warn / fail per check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn glyph(self) -> &'static str {
        match self {
            CheckStatus::Pass => "[✓]",
            CheckStatus::Warn => "[!]",
            CheckStatus::Fail => "[✗]",
        }
    }

    fn json_str(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }
}

/// A single diagnostic line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub category: &'static str,
    pub name: String,
    pub status: CheckStatus,
    /// One-line detail shown after the name.
    pub detail: String,
    /// Actionable fix, rendered as a `→ fix:` line. `None` for passing checks.
    pub fix: Option<String>,
    /// Optional resolved value (path/version) shown in `--verbose` and JSON.
    pub value: Option<String>,
}

impl Check {
    fn pass(category: &'static str, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            category,
            name: name.into(),
            status: CheckStatus::Pass,
            detail: detail.into(),
            fix: None,
            value: None,
        }
    }

    fn warn(
        category: &'static str,
        name: impl Into<String>,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
            value: None,
        }
    }

    fn fail(
        category: &'static str,
        name: impl Into<String>,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
            value: None,
        }
    }

    fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }
}

/// Aggregated report.
#[derive(Debug, Clone)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(checks: Vec<Check>) -> Self {
        Self { checks }
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut pass = 0;
        let mut warn = 0;
        let mut fail = 0;
        for c in &self.checks {
            match c.status {
                CheckStatus::Pass => pass += 1,
                CheckStatus::Warn => warn += 1,
                CheckStatus::Fail => fail += 1,
            }
        }
        (pass, warn, fail)
    }

    /// Exit code: `1` on any failure, or (with `strict`) any warning.
    pub fn exit_code(&self, strict: bool) -> i32 {
        let (_, warn, fail) = self.counts();
        if fail > 0 || (strict && warn > 0) {
            1
        } else {
            0
        }
    }

    /// Render the flutter-doctor-style human report to a string.
    pub fn render(&self, verbose: bool, strict: bool) -> String {
        let mut out = String::new();
        let mut last_category: Option<&str> = None;
        for check in &self.checks {
            if last_category != Some(check.category) {
                if last_category.is_some() {
                    out.push('\n');
                }
                out.push_str(check.category);
                out.push('\n');
                last_category = Some(check.category);
            }
            out.push_str(check.status.glyph());
            out.push(' ');
            out.push_str(&check.name);
            if !check.detail.is_empty() {
                out.push_str(" — ");
                out.push_str(&check.detail);
            }
            if verbose {
                if let Some(value) = &check.value {
                    out.push_str(" (");
                    out.push_str(value);
                    out.push(')');
                }
            }
            out.push('\n');
            if let Some(fix) = &check.fix {
                out.push_str("    → fix: ");
                out.push_str(fix);
                out.push('\n');
            }
        }

        let (pass, warn, fail) = self.counts();
        out.push('\n');
        if fail == 0 && (warn == 0 || !strict) {
            out.push_str(&format!(
                "• Doctor summary: {pass} passed, {warn} warning(s). No fatal issues found."
            ));
        } else {
            out.push_str(&format!(
                "• Doctor summary: {pass} passed, {warn} warning(s), {fail} failure(s)."
            ));
        }
        out.push('\n');
        out
    }

    /// Render the support-bundle JSON.
    pub fn to_json(&self, strict: bool) -> serde_json::Value {
        let (pass, warn, fail) = self.counts();
        let checks: Vec<_> = self
            .checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "category": c.category,
                    "name": c.name,
                    "status": c.status.json_str(),
                    "detail": c.detail,
                    "fix": c.fix,
                    "value": c.value,
                })
            })
            .collect();
        serde_json::json!({
            "checks": checks,
            "summary": {
                "passed": pass,
                "warnings": warn,
                "failures": fail,
            },
            "exit_code": self.exit_code(strict),
            "octos_tui_version": env!("CARGO_PKG_VERSION"),
            "octos_core_schema_version": UI_PROTOCOL_SCHEMA_VERSION,
            "octos_protocol": UI_PROTOCOL_V1,
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        })
    }
}

/// Entry point: gather all checks, render, return the exit code.
pub fn run(args: DoctorArgs) -> Result<i32> {
    let mut checks = Vec::new();
    checks.extend(binary_checks(&args));
    checks.extend(terminal_checks());
    checks.extend(config_checks(&args));
    checks.extend(backend_checks(&args));
    checks.extend(network_checks());

    let report = Report::new(checks);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.to_json(args.strict))?
        );
    } else {
        print!("{}", report.render(args.verbose, args.strict));
    }
    Ok(report.exit_code(args.strict))
}

// ---------------------------------------------------------------------------
// Binary & version
// ---------------------------------------------------------------------------

const CAT_BINARY: &str = "Binary & version";

fn binary_checks(_args: &DoctorArgs) -> Vec<Check> {
    let mut checks = Vec::new();

    // current_exe resolves.
    let current_exe = std::env::current_exe().ok();
    match &current_exe {
        Some(exe) => checks.push(
            Check::pass(
                CAT_BINARY,
                "octos-tui binary",
                format!("v{}", env!("CARGO_PKG_VERSION")),
            )
            .with_value(exe.display().to_string()),
        ),
        None => checks.push(Check::warn(
            CAT_BINARY,
            "octos-tui binary",
            "could not resolve current executable",
            "ensure octos-tui is on a real filesystem path",
        )),
    }

    // Install method.
    let method = install_method::detect();
    checks.push(Check::pass(CAT_BINARY, "install method", method.label()).with_value(method.id()));

    // PATH resolvability + shadowing installs. We track `$PATH` resolutions
    // separately from extra known-install prefixes so "on PATH" reflects what
    // can actually be run *by name*, not merely what exists on disk.
    let located = locate_octos_tui();
    checks.push(on_path_check(&located, current_exe.as_deref(), &method));
    checks.push(shadow_check(&located, &method));

    // Newer release (best-effort; network failure → warn, not fail).
    checks.push(release_check(&method));

    checks
}

/// `octos-tui` binaries discovered on the host, with `$PATH` hits tracked
/// separately from extra known-install prefixes (cargo bin, brew, …) that may
/// not be on `$PATH`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocatedBinaries {
    /// Resolved via `$PATH` (runnable by bare name), in PATH precedence order.
    pub on_path: Vec<PathBuf>,
    /// Found only in extra known-install prefixes that are NOT on `$PATH`.
    pub off_path: Vec<PathBuf>,
}

impl LocatedBinaries {
    /// Every distinct binary location (PATH hits first, then off-PATH extras).
    fn all(&self) -> Vec<PathBuf> {
        let mut v = self.on_path.clone();
        v.extend(self.off_path.iter().cloned());
        v
    }
}

/// Whether `octos-tui` is runnable by bare name (`$PATH`-resolvable). When the
/// running executable's directory is not on `$PATH`, warn that it was launched
/// by path and won't be found by name — folding the cargo-bin/brew prefixes in
/// would mask exactly this case.
fn on_path_check(
    located: &LocatedBinaries,
    current_exe: Option<&Path>,
    method: &InstallMethod,
) -> Check {
    if let Some(first) = located.on_path.first() {
        return Check::pass(CAT_BINARY, "octos-tui on PATH", "resolvable by name")
            .with_value(first.display().to_string());
    }
    // npm global (esp. Windows): the launcher shim (octos-tui.ps1/.cmd) IS on
    // PATH and runnable by name, but `current_exe()` resolves to the real binary
    // deep under `node_modules/.bin_real`, whose dir is NOT on PATH and whose
    // basename isn't `octos-tui[.exe]` — so the PATH scan finds nothing. Don't
    // false-warn, and never suggest adding an internal node_modules dir. (#189)
    if matches!(method, InstallMethod::Npm) {
        return Check::pass(
            CAT_BINARY,
            "octos-tui on PATH",
            "runnable by name via the npm global shim",
        )
        .with_value(
            current_exe
                .map(|e| e.display().to_string())
                .unwrap_or_default(),
        );
    }
    // Not on $PATH at all. If we know where this exe lives, point at its dir.
    match current_exe.and_then(|e| e.parent()) {
        Some(dir) => Check::warn(
            CAT_BINARY,
            "octos-tui on PATH",
            "octos-tui isn't on $PATH — you ran it by path",
            format!("add {} to PATH to run by name", dir.display()),
        )
        .with_value(dir.display().to_string()),
        None => Check::warn(
            CAT_BINARY,
            "octos-tui on PATH",
            "octos-tui not found on $PATH",
            "add the install dir to your PATH",
        ),
    }
}

/// Build the shadowing-install check from the located binaries. Shadowing
/// considers both `$PATH` hits and off-PATH known-install locations (>1 total
/// is the Claude Code #22415 failure mode), labelling which is which.
fn shadow_check(located: &LocatedBinaries, method: &InstallMethod) -> Check {
    let all = located.all();
    match all.len() {
        // npm puts the real binary under node_modules/.bin_real (off PATH, and
        // not in the unix known-dir list), so the locator finds nothing — but
        // that's exactly one healthy install, not a missing one. (#189)
        0 if matches!(method, InstallMethod::Npm) => Check::pass(
            CAT_BINARY,
            "no shadowing installs",
            "exactly one (npm global)",
        ),
        0 => Check::warn(
            CAT_BINARY,
            "no shadowing installs",
            "octos-tui not found on $PATH or known install dirs",
            "install octos-tui or add its dir to your PATH",
        ),
        1 => {
            let only = &all[0];
            let where_ = if located.on_path.is_empty() {
                "off PATH"
            } else {
                "on PATH"
            };
            Check::pass(
                CAT_BINARY,
                "no shadowing installs",
                format!("exactly one ({where_})"),
            )
            .with_value(only.display().to_string())
        }
        n => {
            let label = |p: &PathBuf| -> String {
                let tag = if located.on_path.contains(p) {
                    "PATH"
                } else {
                    "known-dir"
                };
                format!("{} [{tag}]", p.display())
            };
            let labelled: Vec<String> = all.iter().map(label).collect();
            Check::warn(
                CAT_BINARY,
                "no shadowing installs",
                format!("{n} octos-tui binaries found; first wins: {}", labelled[0]),
                format!("remove the extras: {}", labelled[1..].join(", ")),
            )
            .with_value(labelled.join(" | "))
        }
    }
}

fn release_check(method: &InstallMethod) -> Check {
    match github::latest_release(false) {
        Ok(None) => Check::pass(
            CAT_BINARY,
            "up to date",
            format!("v{} (no published releases yet)", env!("CARGO_PKG_VERSION")),
        ),
        Ok(Some(latest)) => {
            let current = env!("CARGO_PKG_VERSION");
            let current_v = super::update::parse_version(current);
            let latest_v = super::update::parse_version(&latest.tag);
            match (current_v, latest_v) {
                (Some(c), Some(l)) if super::update::is_newer(&c, &l) => {
                    let fix = method
                        .upgrade_command()
                        .map(|cmd| cmd.to_string())
                        .unwrap_or_else(|| "run `octos-tui update`".to_string());
                    Check::warn(
                        CAT_BINARY,
                        "up to date",
                        format!("newer release available: {c} -> {l}"),
                        fix,
                    )
                }
                (Some(c), Some(l)) => {
                    Check::pass(CAT_BINARY, "up to date", format!("v{c} is current"))
                        .with_value(l.to_string())
                }
                _ => Check::warn(
                    CAT_BINARY,
                    "up to date",
                    format!("could not parse versions (latest tag {})", latest.tag),
                    "run `octos-tui update --check`",
                ),
            }
        }
        Err(err) => Check::warn(
            CAT_BINARY,
            "up to date",
            format!("could not check GitHub for a newer release: {err}"),
            "run `octos-tui update --check` when online",
        ),
    }
}

/// Enumerate every `octos-tui` on `$PATH` plus known install prefixes,
/// de-duplicated by canonical path, preserving PATH precedence (first wins).
/// `$PATH` resolutions are tracked separately from extra known-install
/// prefixes so the "on PATH" check reflects bare-name runnability, not mere
/// on-disk presence (a cargo-bin install whose dir isn't on `$PATH` would
/// otherwise be mis-reported as runnable by name).
pub fn locate_octos_tui() -> LocatedBinaries {
    let exe_name = if cfg!(windows) {
        "octos-tui.exe"
    } else {
        "octos-tui"
    };
    let mut located = LocatedBinaries::default();
    let mut seen: Vec<PathBuf> = Vec::new();

    let push_if_present = |dir: &Path, dest: &mut Vec<PathBuf>, seen: &mut Vec<PathBuf>| {
        let candidate = dir.join(exe_name);
        if !candidate.is_file() {
            return;
        }
        let canonical = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen.contains(&canonical) {
            return;
        }
        seen.push(canonical);
        dest.push(candidate);
    };

    // Actual `$PATH` resolutions, in precedence order (first wins).
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            push_if_present(&dir, &mut located.on_path, &mut seen);
        }
    }

    // Extra known-install prefixes that may NOT be on `$PATH`. These count for
    // shadow detection but are kept distinct from `$PATH` hits.
    let mut extras: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        extras.push(PathBuf::from(&home).join(".cargo").join("bin"));
        extras.push(PathBuf::from(&home).join(".local").join("bin"));
    }
    for dir in extras {
        push_if_present(&dir, &mut located.off_path, &mut seen);
    }

    located
}

// ---------------------------------------------------------------------------
// Terminal environment
// ---------------------------------------------------------------------------

const CAT_TERM: &str = "Terminal environment";

fn terminal_checks() -> Vec<Check> {
    let term = std::env::var("TERM").ok();
    let lang = std::env::var("LANG").ok();
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_ctype = std::env::var("LC_CTYPE").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    vec![
        term_check(term.as_deref()),
        locale_check(lang.as_deref(), lc_all.as_deref(), lc_ctype.as_deref()),
        cjk_check(),
        color_check(term.as_deref(), colorterm.as_deref()),
    ]
}

/// Result of probing whether a `TERM` value has a loadable terminfo entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminfoProbe {
    /// `infocmp` confirmed the terminfo entry loads.
    Found,
    /// `infocmp` ran but reported the entry is missing (non-zero exit).
    Missing,
    /// `infocmp` itself isn't available — we can't probe, so don't hard-fail.
    ProberAbsent,
}

fn term_check(term: Option<&str>) -> Check {
    term_check_with(term, probe_terminfo)
}

/// `term_check` with an injectable terminfo prober (for tests).
fn term_check_with(term: Option<&str>, probe: impl Fn(&str) -> TerminfoProbe) -> Check {
    match term {
        Some("dumb") => Check::warn(
            CAT_TERM,
            "TERM set",
            "TERM=dumb has no terminfo capabilities",
            "export TERM=xterm-256color",
        ),
        Some(t) if !t.is_empty() => match probe(t) {
            // The entry exists, or we couldn't probe (prober absent) — pass.
            // Don't hard-fail merely because `infocmp` isn't installed.
            TerminfoProbe::Found | TerminfoProbe::ProberAbsent => {
                Check::pass(CAT_TERM, "TERM set", t.to_string()).with_value(t.to_string())
            }
            // TERM is plausible but its terminfo entry doesn't load — this is
            // the documented "can't find terminfo database" failure.
            TerminfoProbe::Missing => Check::warn(
                CAT_TERM,
                "TERM set",
                format!("TERM=`{t}` has no terminfo entry (the TUI will report 'can't find terminfo database')"),
                "set TERM=xterm-256color or install the terminfo package for your terminal",
            )
            .with_value(t.to_string()),
        },
        _ => Check::warn(
            CAT_TERM,
            "TERM set",
            "TERM is unset; the TUI may not render or may report 'can't find terminfo database'",
            "export TERM=xterm-256color",
        ),
    }
}

/// Probe whether `term`'s terminfo entry is loadable by shelling out to
/// `infocmp`. A zero exit means the entry was found; a non-zero exit means it's
/// missing; a spawn failure means `infocmp` isn't installed (can't probe).
fn probe_terminfo(term: &str) -> TerminfoProbe {
    match std::process::Command::new("infocmp")
        .arg("-1")
        .arg(term)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => TerminfoProbe::Found,
        Ok(_) => TerminfoProbe::Missing,
        Err(_) => TerminfoProbe::ProberAbsent,
    }
}

fn locale_check(lang: Option<&str>, lc_all: Option<&str>, lc_ctype: Option<&str>) -> Check {
    let effective = lc_all.or(lc_ctype).or(lang);
    match effective {
        Some(v)
            if v.to_ascii_uppercase().contains("UTF-8")
                || v.to_ascii_uppercase().contains("UTF8") =>
        {
            Check::pass(CAT_TERM, "UTF-8 locale", v.to_string()).with_value(v.to_string())
        }
        Some(v) => Check::warn(
            CAT_TERM,
            "UTF-8 locale",
            format!("locale `{v}` is not UTF-8; box-drawing and CJK may break"),
            "export LANG=en_US.UTF-8 (or your locale with .UTF-8)",
        ),
        None => Check::warn(
            CAT_TERM,
            "UTF-8 locale",
            "no LANG/LC_ALL/LC_CTYPE set",
            "export LANG=en_US.UTF-8",
        ),
    }
}

fn cjk_check() -> Check {
    // Informational: octos-tui uses `unicode-width` for CJK double-width; the
    // visible result also depends on the terminal font, so this never fails.
    Check::pass(
        CAT_TERM,
        "CJK width",
        "uses unicode-width for double-width glyphs (also depends on terminal font)",
    )
}

fn color_check(term: Option<&str>, colorterm: Option<&str>) -> Check {
    let truecolor = colorterm
        .map(|c| c.contains("truecolor") || c.contains("24bit"))
        .unwrap_or(false);
    let has_256 = term.map(|t| t.contains("256color")).unwrap_or(false);
    if truecolor {
        Check::pass(CAT_TERM, "color support", "truecolor (24-bit)")
    } else if has_256 {
        Check::pass(CAT_TERM, "color support", "256-color")
    } else {
        Check::warn(
            CAT_TERM,
            "color support",
            "no truecolor/256-color advertised; themes may look flat",
            "use a 256-color terminal and set TERM=xterm-256color (COLORTERM=truecolor)",
        )
    }
}

// ---------------------------------------------------------------------------
// Config & data
// ---------------------------------------------------------------------------

const CAT_CONFIG: &str = "Config & data";

fn config_checks(args: &DoctorArgs) -> Vec<Check> {
    let data_dir = args
        .data_dir
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".octos")))
        .unwrap_or_else(|| PathBuf::from(".octos"));
    vec![writability_check("octos data dir", &data_dir)]
}

/// Check that a directory exists and is writable (or creatable). A missing dir
/// that can be created is a `[!]` with a `--fix`-able action, not a failure. A
/// path that exists but is **not** a directory (e.g. a stray regular file at
/// `~/.octos`) is a `[✗]` failure: the `mkdir -p` hint would fail, so we tell
/// the user to clear the path instead.
fn writability_check(name: &'static str, dir: &Path) -> Check {
    if dir.is_dir() {
        if is_writable(dir) {
            Check::pass(CAT_CONFIG, name, "present and writable")
                .with_value(dir.display().to_string())
        } else {
            Check::fail(
                CAT_CONFIG,
                name,
                format!("{} is not writable", dir.display()),
                format!("chmod u+w {}", dir.display()),
            )
        }
    } else if dir.exists() {
        // Exists but isn't a directory — `mkdir -p` would fail, so don't offer
        // it. The path is occupied by a file (or other non-dir); clear it.
        Check::fail(
            CAT_CONFIG,
            name,
            format!("{} exists but is not a directory", dir.display()),
            format!(
                "remove the file at {} or point --data-dir elsewhere",
                dir.display()
            ),
        )
        .with_value(dir.display().to_string())
    } else {
        Check::warn(
            CAT_CONFIG,
            name,
            format!("{} does not exist yet", dir.display()),
            format!("mkdir -p {}", dir.display()),
        )
        .with_value(dir.display().to_string())
    }
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".octos-tui-doctor-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Backend connectivity + protocol skew
// ---------------------------------------------------------------------------

const CAT_BACKEND: &str = "Backend";

fn backend_checks(args: &DoctorArgs) -> Vec<Check> {
    let mut checks = Vec::new();

    // Transport resolution.
    if let Some(cmd) = &args.stdio_command {
        checks.push(stdio_command_check(cmd));
    } else if let Some(endpoint) = &args.endpoint {
        // Live WS probe is a documented TODO; we record the configured endpoint
        // and run the structural skew check below regardless.
        checks.push(
            Check::warn(
                CAT_BACKEND,
                "WS endpoint probe",
                format!("endpoint configured ({endpoint}); live config/capabilities/list probe not yet wired"),
                "run `octos-tui --endpoint … ` to exercise the live connection (TODO: doctor live WS probe)",
            )
            .with_value(endpoint.clone()),
        );
    } else {
        checks.push(Check::pass(
            CAT_BACKEND,
            "transport",
            "no backend configured (mock mode); skipping connectivity",
        ));
    }

    // Structural protocol-skew check (always runs; does not need a live
    // server). Compares the TUI's required feature set + compiled-in schema
    // version against the octos-core feature registry the TUI is built with.
    checks.push(protocol_skew_check());

    checks
}

/// Shell operators that mean the stdio command runs as a shell *script*, not a
/// bare exec — pipes, sequencing, redirection, command substitution, etc. When
/// any of these are present we cannot statically resolve "the binary".
const SHELL_OPERATORS: &[&str] = &[
    "&&", "||", ";", "|", "`", "$(", ">", "<", "&", "\n", "(", ")", "{", "}",
];

/// What the leading executable of a stdio command resolves to, after stripping
/// shell prefixes. The stdio child runs via the transport's shell (`sh -c` /
/// `cmd /C`), so env-assignment prefixes and shell operators are legal and must
/// not be reported as a hard `[✗]` failure.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StdioResolution {
    /// A plain `prog arg…` (possibly with `VAR=val` prefixes) whose leading
    /// program is `prog`.
    Program(String),
    /// The command uses shell syntax (operators/substitution); the binary
    /// can't be statically verified — it'll run via the transport shell.
    ShellSyntax,
    /// The command couldn't be parsed (unbalanced quotes, …).
    Unparsable,
}

/// Classify a stdio command into a resolvable leading program, shell syntax, or
/// an unparsable string. Strips leading `VAR=value` env-assignment prefixes.
fn classify_stdio_command(command: &str) -> StdioResolution {
    // Shell operators ⇒ the transport shell runs a script; don't hard-fail.
    if SHELL_OPERATORS.iter().any(|op| command.contains(op)) {
        return StdioResolution::ShellSyntax;
    }
    let Some(tokens) = shlex::split(command) else {
        return StdioResolution::Unparsable;
    };
    // Skip leading `VAR=value` env-assignment prefixes (e.g. `FOO=1 octos …`).
    let mut rest = tokens.into_iter().skip_while(|tok| is_env_assignment(tok));
    let Some(program) = rest.next() else {
        // Only env assignments (or empty) — nothing to exec statically, but the
        // shell would still run; treat as shell syntax, not a hard failure.
        return StdioResolution::ShellSyntax;
    };
    // An explicit shell wrapper (`sh -c '…'`, `bash -lc '…'`) runs an arbitrary
    // script; the real binary is inside the quoted argument and can't be
    // statically resolved.
    if is_shell_wrapper(&program) && rest.any(|a| a.starts_with("-") && a.contains('c')) {
        return StdioResolution::ShellSyntax;
    }
    StdioResolution::Program(program)
}

/// Whether `program` is a POSIX shell that would run its `-c` argument as a
/// script (so the real executable is hidden inside the quoted string).
fn is_shell_wrapper(program: &str) -> bool {
    let base = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    matches!(base, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish")
}

/// Whether `tok` is a leading `VAR=value` shell env-assignment. The name must
/// be a non-empty valid shell identifier (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_env_assignment(tok: &str) -> bool {
    let Some(eq) = tok.find('=') else {
        return false;
    };
    let name = &tok[..eq];
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or('\0');
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Resolve the leading executable of `--stdio-command` on PATH and, if it is
/// the `octos` server, run `<bin> --version` to surface the build it would
/// launch. The child runs via the transport's shell, so shell-syntax commands
/// (env prefixes, `cd … &&`, pipes) are downgraded to `[!]` warns — we can't
/// statically verify them — rather than reported as a missing binary `[✗]`.
fn stdio_command_check(command: &str) -> Check {
    let program = match classify_stdio_command(command) {
        StdioResolution::Program(p) => p,
        StdioResolution::ShellSyntax => {
            return Check::warn(
                CAT_BACKEND,
                "stdio command",
                "stdio command uses shell syntax; can't statically verify the binary — it will run via the transport shell",
                "ensure the command launches an octos server with `--stdio` (e.g. `octos serve --stdio`)",
            )
            .with_value(command.to_string());
        }
        StdioResolution::Unparsable => {
            return Check::fail(
                CAT_BACKEND,
                "stdio command",
                format!("could not parse stdio command `{command}`"),
                "set a valid --stdio-command (e.g. `octos serve --stdio`)",
            );
        }
    };

    let resolved = which(&program);
    match resolved {
        Some(path) => {
            // Surface the server build (best effort).
            let version = std::process::Command::new(&path)
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            let detail = match &version {
                Some(v) if !v.is_empty() => format!("resolves to {} ({v})", path.display()),
                _ => format!("resolves to {}", path.display()),
            };
            Check::pass(CAT_BACKEND, "stdio command", detail).with_value(path.display().to_string())
        }
        None => Check::fail(
            CAT_BACKEND,
            "stdio command",
            format!("`{program}` not found on PATH"),
            format!("install `{program}` or correct --stdio-command"),
        ),
    }
}

/// Structural protocol-skew check (design §B, P3 fallback).
///
/// Compares what the TUI requires against the `octos-core` it was compiled
/// with: confirms every [`TUI_REQUIRED_FEATURES`] entry is a known feature in
/// this protocol build (so the TUI isn't asking for a feature the protocol
/// crate no longer defines), and reports the compiled-in protocol/schema
/// version. A live server `config/capabilities/list` comparison reuses
/// [`compare_against_server`]; wiring the live WS handshake is a TODO.
fn protocol_skew_check() -> Check {
    let unknown: Vec<&str> = TUI_REQUIRED_FEATURES
        .iter()
        .copied()
        .filter(|f| !UI_PROTOCOL_KNOWN_FEATURES.contains(f))
        .collect();
    if unknown.is_empty() {
        Check::pass(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "TUI requires {} features; all known in {UI_PROTOCOL_V1} (schema v{UI_PROTOCOL_SCHEMA_VERSION})",
                TUI_REQUIRED_FEATURES.len()
            ),
        )
        .with_value(format!("{UI_PROTOCOL_V1} schema v{UI_PROTOCOL_SCHEMA_VERSION}"))
    } else {
        Check::fail(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "TUI requires features absent from its octos-core build: {}",
                unknown.join(", ")
            ),
            "re-pin octos-tui's octos-core revision to one that defines these features",
        )
    }
}

/// Compare the TUI's compiled-in protocol against a live server's advertised
/// capabilities. Reusable by a future live WS/stdio probe.
///
/// - `[✗]` when the protocol string differs or the server's schema version is
///   *older* than the TUI's compiled-in schema (incompatible).
/// - `[!]` when the server is missing a feature the TUI requires.
/// - `[✓]` otherwise.
pub fn compare_against_server(server: &UiProtocolCapabilities) -> Check {
    if server.version.protocol != UI_PROTOCOL_V1 {
        return Check::fail(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "server speaks `{}` but the TUI speaks `{UI_PROTOCOL_V1}`",
                server.version.protocol
            ),
            "upgrade whichever side is on the wrong protocol family",
        );
    }
    if server.version.schema_version < UI_PROTOCOL_SCHEMA_VERSION {
        return Check::fail(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "server schema v{} is older than the TUI's v{UI_PROTOCOL_SCHEMA_VERSION}",
                server.version.schema_version
            ),
            "upgrade the octos server (`octos update`) so its schema ≥ the client's",
        );
    }
    let missing: Vec<&str> = TUI_REQUIRED_FEATURES
        .iter()
        .copied()
        .filter(|f| !server.supported_features.iter().any(|s| s == f))
        .collect();
    if missing.is_empty() {
        Check::pass(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "compatible (server schema v{}, all required features present)",
                server.version.schema_version
            ),
        )
    } else {
        Check::warn(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "server is missing TUI-required features: {}",
                missing.join(", ")
            ),
            "upgrade the octos server to advertise these features, or expect degraded behavior",
        )
    }
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

const CAT_NETWORK: &str = "Network";

fn network_checks() -> Vec<Check> {
    let check = match github::reachability() {
        Reachability::Ok => Check::pass(CAT_NETWORK, "GitHub reachable", "api.github.com OK"),
        Reachability::RateLimited => Check::warn(
            CAT_NETWORK,
            "GitHub reachable",
            "api.github.com rate-limited (HTTP 403)",
            "set OCTOS_TUI_GITHUB_TOKEN to raise the rate limit",
        ),
        Reachability::Unreachable(err) => Check::warn(
            CAT_NETWORK,
            "GitHub reachable",
            format!("api.github.com unreachable: {err}"),
            "check your network/proxy; update checks will be unavailable",
        ),
    };
    vec![check]
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Cross-platform `which`: resolve `program` against `$PATH`.
///
/// A match must be an *executable* regular file — a non-executable file on PATH
/// would pass an `is_file()`-only check yet fail to launch, so on Unix we also
/// require an executable bit (`mode & 0o111`). Windows relies on the `.exe`
/// extension (added above) as its executability signal.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe = if cfg!(windows) && !program.ends_with(".exe") {
        format!("{program}.exe")
    } else {
        program.to_string()
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(&exe))
        .find(|candidate| is_executable_file(candidate))
}

/// Whether `path` is a regular file that can actually be executed. On Unix the
/// file must carry an executable bit; on other platforms `is_file()` (with the
/// `.exe` extension applied by [`which`]) is the best available signal.
fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octos_core::ui_protocol::UiProtocolCapabilities;

    fn server_caps() -> UiProtocolCapabilities {
        UiProtocolCapabilities::full_protocol()
    }

    #[test]
    fn renderer_groups_by_category_and_shows_fix_lines() {
        let checks = vec![
            Check::pass("Cat A", "ok thing", "all good"),
            Check::warn("Cat A", "warny thing", "soft problem", "do the fix"),
            Check::fail("Cat B", "broken thing", "hard problem", "fix me"),
        ];
        let report = Report::new(checks);
        let text = report.render(false, false);
        assert!(text.contains("Cat A\n"));
        assert!(text.contains("Cat B\n"));
        assert!(text.contains("[✓] ok thing"));
        assert!(text.contains("[!] warny thing"));
        assert!(text.contains("[✗] broken thing"));
        assert!(text.contains("    → fix: do the fix"));
        assert!(text.contains("    → fix: fix me"));
        // No fix line for the passing check.
        assert!(!text.contains("→ fix: \n"));
        assert!(text.contains("1 passed, 1 warning(s), 1 failure(s)"));
    }

    #[test]
    fn exit_code_is_one_on_failure_zero_on_warnings() {
        let warn_only = Report::new(vec![Check::warn("c", "n", "d", "f")]);
        assert_eq!(warn_only.exit_code(false), 0);
        assert_eq!(warn_only.exit_code(true), 1); // strict promotes warnings

        let with_fail = Report::new(vec![Check::fail("c", "n", "d", "f")]);
        assert_eq!(with_fail.exit_code(false), 1);
    }

    #[test]
    fn json_redacts_nothing_sensitive_and_carries_summary() {
        let report = Report::new(vec![Check::pass("c", "n", "d")]);
        let json = report.to_json(false);
        assert_eq!(json["summary"]["passed"], 1);
        assert_eq!(json["octos_tui_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            json["octos_core_schema_version"],
            UI_PROTOCOL_SCHEMA_VERSION
        );
        assert!(json["checks"].is_array());
    }

    #[test]
    fn shadow_check_passes_for_single_and_warns_for_multiple() {
        let one = shadow_check(
            &LocatedBinaries {
                on_path: vec![PathBuf::from("/usr/local/bin/octos-tui")],
                off_path: vec![],
            },
            &InstallMethod::Homebrew,
        );
        assert_eq!(one.status, CheckStatus::Pass);
        assert!(one.detail.contains("on PATH"));

        let two = shadow_check(
            &LocatedBinaries {
                on_path: vec![PathBuf::from("/opt/homebrew/bin/octos-tui")],
                off_path: vec![PathBuf::from("/home/u/.cargo/bin/octos-tui")],
            },
            &InstallMethod::Homebrew,
        );
        assert_eq!(two.status, CheckStatus::Warn);
        assert!(two.detail.contains("2 octos-tui binaries"));
        let fix = two.fix.unwrap();
        assert!(fix.contains(".cargo/bin/octos-tui"));
        // The two locations are labelled by where they were found.
        assert!(fix.contains("[known-dir]") || two.detail.contains("[PATH]"));
    }

    #[test]
    fn shadow_check_warns_when_nothing_found() {
        let none = shadow_check(&LocatedBinaries::default(), &InstallMethod::Homebrew);
        assert_eq!(none.status, CheckStatus::Warn);
    }

    #[test]
    fn npm_install_does_not_false_warn_on_path_or_shadow() {
        // #189: npm-global (esp. Windows) — the locator finds no `octos-tui`
        // on PATH (the shim is .ps1/.cmd; the real .exe is under
        // node_modules/.bin_real). Both checks must PASS, not warn.
        let located = LocatedBinaries::default();
        let exe = PathBuf::from(
            "C:/Users/u/AppData/Roaming/npm/node_modules/@octos-org/octos-tui/node_modules/.bin_real/octos-tui.exe",
        );
        let on_path = on_path_check(&located, Some(exe.as_path()), &InstallMethod::Npm);
        assert_eq!(on_path.status, CheckStatus::Pass);
        assert!(
            on_path.fix.is_none(),
            "npm on-PATH check must not suggest a fix"
        );

        let shadow = shadow_check(&located, &InstallMethod::Npm);
        assert_eq!(shadow.status, CheckStatus::Pass);
        assert!(shadow.detail.contains("npm"));
    }

    #[test]
    fn on_path_check_passes_when_resolvable_by_name() {
        let located = LocatedBinaries {
            on_path: vec![PathBuf::from("/usr/local/bin/octos-tui")],
            off_path: vec![],
        };
        let check = on_path_check(
            &located,
            Some(Path::new("/usr/local/bin/octos-tui")),
            &InstallMethod::Homebrew,
        );
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn on_path_check_warns_when_ran_by_abs_path_and_dir_not_on_path() {
        // Finding #1: running `~/.cargo/bin/octos-tui doctor` while
        // `~/.cargo/bin` is NOT on $PATH must WARN that it isn't runnable by
        // name — not pass because the binary merely exists in a known dir.
        let located = LocatedBinaries {
            on_path: vec![],
            off_path: vec![PathBuf::from("/home/u/.cargo/bin/octos-tui")],
        };
        let exe = PathBuf::from("/home/u/.cargo/bin/octos-tui");
        let check = on_path_check(&located, Some(&exe), &InstallMethod::CargoGit);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("isn't on $PATH"));
        // The fix points at the running exe's directory.
        assert!(check.fix.unwrap().contains("/home/u/.cargo/bin"));
    }

    #[test]
    fn term_check_warns_when_unset_or_dumb() {
        // Force the prober to say "Found" so the only warns come from the
        // TERM value itself, not a missing terminfo entry.
        let found = |_: &str| TerminfoProbe::Found;
        assert_eq!(term_check_with(None, found).status, CheckStatus::Warn);
        assert_eq!(
            term_check_with(Some("dumb"), found).status,
            CheckStatus::Warn
        );
        assert_eq!(
            term_check_with(Some("xterm-256color"), found).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn term_check_warns_when_terminfo_entry_missing() {
        // Finding #3: a plausible TERM whose terminfo entry doesn't load must
        // WARN (the documented "can't find terminfo database" case), not pass.
        let missing = |_: &str| TerminfoProbe::Missing;
        let check = term_check_with(Some("xterm-256color"), missing);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("terminfo"));
        assert!(check.fix.unwrap().contains("xterm-256color"));
    }

    #[test]
    fn term_check_passes_when_prober_absent() {
        // If `infocmp` isn't installed we can't probe; pass-with-caveat rather
        // than hard-fail on the prober being absent.
        let absent = |_: &str| TerminfoProbe::ProberAbsent;
        assert_eq!(
            term_check_with(Some("xterm-256color"), absent).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn locale_check_requires_utf8() {
        assert_eq!(
            locale_check(Some("en_US.UTF-8"), None, None).status,
            CheckStatus::Pass
        );
        assert_eq!(
            locale_check(Some("C"), None, None).status,
            CheckStatus::Warn
        );
        assert_eq!(locale_check(None, None, None).status, CheckStatus::Warn);
        // LC_ALL overrides LANG.
        assert_eq!(
            locale_check(Some("C"), Some("en_US.UTF-8"), None).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn color_check_recognizes_truecolor_and_256() {
        assert_eq!(
            color_check(Some("xterm"), Some("truecolor")).status,
            CheckStatus::Pass
        );
        assert_eq!(
            color_check(Some("xterm-256color"), None).status,
            CheckStatus::Pass
        );
        assert_eq!(color_check(Some("xterm"), None).status, CheckStatus::Warn);
    }

    #[test]
    fn structural_skew_check_passes_against_own_core_build() {
        // Every TUI-required feature must be a known feature in the octos-core
        // this crate compiles against — otherwise the TUI ships broken.
        assert_eq!(protocol_skew_check().status, CheckStatus::Pass);
    }

    #[test]
    fn compare_against_server_passes_for_full_protocol() {
        let check = compare_against_server(&server_caps());
        assert_eq!(check.status, CheckStatus::Pass, "{:?}", check);
    }

    #[test]
    fn compare_against_server_warns_when_feature_missing() {
        let mut caps = server_caps();
        caps.supported_features
            .retain(|f| f != UI_PROTOCOL_FEATURE_USER_QUESTION_V1);
        let check = compare_against_server(&caps);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains(UI_PROTOCOL_FEATURE_USER_QUESTION_V1));
    }

    #[test]
    fn compare_against_server_fails_on_older_schema() {
        let mut caps = server_caps();
        // Force an incompatible (older) server schema.
        if UI_PROTOCOL_SCHEMA_VERSION > 0 {
            caps.version.schema_version = UI_PROTOCOL_SCHEMA_VERSION - 1;
            let check = compare_against_server(&caps);
            assert_eq!(check.status, CheckStatus::Fail);
            assert!(check.detail.contains("older"));
        }
    }

    #[test]
    fn compare_against_server_fails_on_wrong_protocol_family() {
        let mut caps = server_caps();
        caps.version.protocol = "octos-ui/v2alpha".into();
        let check = compare_against_server(&caps);
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn writability_check_passes_for_writable_tempdir() {
        let dir = std::env::temp_dir();
        let check = writability_check("tmp", &dir);
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn writability_check_warns_for_missing_dir() {
        let missing = std::env::temp_dir().join("octos-tui-doctor-nope-xyz-12345");
        let _ = std::fs::remove_dir_all(&missing);
        let check = writability_check("missing", &missing);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.fix.unwrap().contains("mkdir -p"));
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_file_rejects_non_executable_and_accepts_executable() {
        use std::os::unix::fs::PermissionsExt;
        // Finding #4: a non-executable file on PATH must not count as a match,
        // since launching it would fail with EACCES.
        let base = std::env::temp_dir().join("octos-tui-doctor-exec-probe-13579");
        let _ = std::fs::remove_file(&base);
        std::fs::write(&base, b"#!/bin/sh\n").expect("create probe");

        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o644))
            .expect("chmod non-exec");
        assert!(
            !is_executable_file(&base),
            "0o644 file must not be executable"
        );

        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755))
            .expect("chmod exec");
        assert!(is_executable_file(&base), "0o755 file must be executable");

        let _ = std::fs::remove_file(&base);
    }

    #[test]
    fn is_executable_file_rejects_directory_and_missing() {
        let missing = std::env::temp_dir().join("octos-tui-doctor-exec-missing-24680");
        let _ = std::fs::remove_file(&missing);
        assert!(!is_executable_file(&missing));
        // A directory is not a runnable file even though it "exists".
        assert!(!is_executable_file(&std::env::temp_dir()));
    }

    #[test]
    fn stdio_classify_plain_command_resolves_leading_program() {
        match classify_stdio_command("octos serve --stdio") {
            StdioResolution::Program(p) => assert_eq!(p, "octos"),
            other => panic!("expected Program, got {other:?}"),
        }
    }

    #[test]
    fn stdio_classify_strips_env_assignment_prefix() {
        // Finding #2: `FOO=1 octos serve --stdio` resolves to `octos`, not the
        // env assignment, and must not be a hard `[✗]`.
        match classify_stdio_command("FOO=1 BAR=2 octos serve --stdio") {
            StdioResolution::Program(p) => assert_eq!(p, "octos"),
            other => panic!("expected Program, got {other:?}"),
        }
    }

    #[test]
    fn stdio_check_env_prefixed_command_is_not_hard_fail() {
        // The env-prefixed plain command resolves to `octos`; whether `octos`
        // is installed in the test env or not, the result must never be a hard
        // `[✗]` caused by mis-resolving the `FOO=1` token as the program.
        let check = stdio_command_check("FOO=1 octos serve --stdio");
        // Either it resolves (Pass) or `octos` is absent (Fail referencing
        // `octos`, never `FOO=1`).
        if check.status == CheckStatus::Fail {
            assert!(
                check.detail.contains("`octos`"),
                "fail must reference octos, not the env prefix: {}",
                check.detail
            );
        }
        assert!(!check.detail.contains("FOO=1"));
    }

    #[test]
    fn stdio_check_shell_operator_command_warns_not_fails() {
        // Finding #2: `cd repo && ./octos serve --stdio` uses shell syntax and
        // must downgrade to `[!]` warn, never a hard `[✗]` "binary not found".
        let check = stdio_command_check("cd repo && ./octos serve --stdio");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("shell syntax"));
    }

    #[test]
    fn stdio_classify_recognizes_pipes_and_substitution_as_shell() {
        assert_eq!(
            classify_stdio_command("sh -c 'octos serve --stdio'"),
            StdioResolution::ShellSyntax
        );
        assert_eq!(
            classify_stdio_command("octos serve --stdio | tee log"),
            StdioResolution::ShellSyntax
        );
        assert_eq!(
            classify_stdio_command("$(which octos) serve --stdio"),
            StdioResolution::ShellSyntax
        );
    }

    #[test]
    fn is_env_assignment_matches_only_valid_shell_assignments() {
        assert!(is_env_assignment("FOO=1"));
        assert!(is_env_assignment("_FOO_BAR=baz"));
        assert!(!is_env_assignment("octos"));
        assert!(!is_env_assignment("./octos"));
        assert!(!is_env_assignment("=value"));
        assert!(!is_env_assignment("1FOO=bad"));
    }

    #[test]
    fn writability_check_fails_when_path_is_a_file() {
        // A path that exists as a regular file must NOT report "does not exist
        // yet (mkdir -p)" — `mkdir -p` would fail. It is a [✗] failure with a
        // remove/relocate fix (finding #3).
        let file = std::env::temp_dir().join("octos-tui-doctor-datadir-as-file-98765");
        let _ = std::fs::remove_file(&file);
        std::fs::write(&file, b"not a dir").expect("create probe file");
        let check = writability_check("data dir", &file);
        let _ = std::fs::remove_file(&file);
        assert_eq!(check.status, CheckStatus::Fail);
        let fix = check.fix.unwrap();
        assert!(fix.contains("remove the file"));
        assert!(!fix.contains("mkdir -p"));
    }
}
