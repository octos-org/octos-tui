//! Install-method detection for `octos-tui update`/`doctor` (design §A.3).
//!
//! The discriminator is the **cargo-dist install receipt**: only the shell and
//! PowerShell installers write one. If a receipt loads, the binary self-updates
//! in place via axoupdater; otherwise we classify by the binary's path and the
//! corroborating package-manager signal, and print the right upgrade command
//! instead of clobbering a file we do not own.
//!
//! Detection here is intentionally pure/testable: [`classify_path`] takes a
//! synthetic `current_exe` path plus the resolved Homebrew / npm-global /
//! cargo-bin prefixes and returns the [`InstallMethod`], so the path heuristics
//! can be unit-tested without touching the host. [`detect`] wires it to the
//! live environment (and, when the `update` feature is on, the receipt probe).

use std::path::{Path, PathBuf};

/// How this `octos-tui` binary was installed. Drives `update`'s per-method
/// behavior (self-update vs. print-the-command) and `doctor`'s fix lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// Installed by the cargo-dist shell/PowerShell installer — a receipt is
    /// present and we can self-update in place.
    CargoDistInstaller,
    /// Installed via Homebrew (`brew install octos-org/tap/octos-tui`).
    Homebrew,
    /// Installed via npm global (`npm i -g @octos-org/octos-tui`).
    Npm,
    /// `cargo install octos-tui` from the crates.io registry.
    CargoRegistry,
    /// `cargo install --git …` from the GitHub repo.
    CargoGit,
    /// Anything else (distro package, manual copy, dev build, …).
    Unknown,
}

impl InstallMethod {
    /// Short, stable identifier for `--json` output.
    pub fn id(&self) -> &'static str {
        match self {
            InstallMethod::CargoDistInstaller => "cargo-dist-installer",
            InstallMethod::Homebrew => "homebrew",
            InstallMethod::Npm => "npm",
            InstallMethod::CargoRegistry => "cargo-registry",
            InstallMethod::CargoGit => "cargo-git",
            InstallMethod::Unknown => "unknown",
        }
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            InstallMethod::CargoDistInstaller => "cargo-dist installer (self-updating)",
            InstallMethod::Homebrew => "Homebrew",
            InstallMethod::Npm => "npm (global)",
            InstallMethod::CargoRegistry => "cargo install (crates.io)",
            InstallMethod::CargoGit => "cargo install --git",
            InstallMethod::Unknown => "unknown / distro package",
        }
    }

    /// The exact command the user should run to upgrade, for the
    /// package-manager methods. `None` for the self-updating installer (we
    /// upgrade in place) — callers print a self-update message instead.
    pub fn upgrade_command(&self) -> Option<&'static str> {
        match self {
            InstallMethod::CargoDistInstaller => None,
            InstallMethod::Homebrew => Some("brew update && brew upgrade octos-org/tap/octos-tui"),
            InstallMethod::Npm => Some("npm update -g @octos-org/octos-tui"),
            InstallMethod::CargoRegistry => Some("cargo install octos-tui --force"),
            InstallMethod::CargoGit => {
                Some("cargo install --git https://github.com/octos-org/octos-tui octos-tui --force")
            }
            // No package manager owns the binary; suggest converting to a
            // self-updating install via the one-line installer.
            InstallMethod::Unknown => Some(
                "curl --proto '=https' --tlsv1.2 -LsSf \
https://github.com/octos-org/octos-tui/releases/latest/download/octos-tui-installer.sh | sh",
            ),
        }
    }

    /// Whether `update` can mutate the binary in place (only the cargo-dist
    /// installer; everything else defers to the package manager).
    pub fn is_self_updating(&self) -> bool {
        matches!(self, InstallMethod::CargoDistInstaller)
    }
}

/// Inputs to the pure path classifier. All prefixes are optional because they
/// are resolved best-effort from the host (a missing `brew`/`npm`/`cargo`
/// simply means that branch can't match).
#[derive(Debug, Default, Clone)]
pub struct PathClassifierInput {
    /// Resolved `current_exe()` path (canonicalized when possible).
    pub current_exe: PathBuf,
    /// Homebrew prefixes to test as ancestors (e.g. `/opt/homebrew`,
    /// `/usr/local`). Cellar paths are matched by the `/Cellar/` segment.
    pub brew_prefixes: Vec<PathBuf>,
    /// npm global root(s) (`npm root -g` / `npm prefix -g`).
    pub npm_global_roots: Vec<PathBuf>,
    /// `~/.cargo/bin` (cargo install destination).
    pub cargo_bin: Option<PathBuf>,
    /// Whether `~/.cargo/.crates2.json` records this crate as a `--git` source.
    /// `Some(true)` → git, `Some(false)` → registry, `None` → unknown source.
    pub cargo_source_is_git: Option<bool>,
}

/// Classify the binary purely from its path + resolved prefixes (no receipt).
/// First match wins, mirroring design §A.3 step 2.
pub fn classify_path(input: &PathClassifierInput) -> InstallMethod {
    let exe = &input.current_exe;

    // npm global: under an npm root, or any ancestor is a node_modules dir
    // containing @octos-org/octos-tui.
    if input
        .npm_global_roots
        .iter()
        .any(|root| is_ancestor(root, exe))
        || path_has_segment(exe, "node_modules")
    {
        return InstallMethod::Npm;
    }

    // Homebrew: under a brew prefix, or anywhere under a `Cellar` dir.
    if input
        .brew_prefixes
        .iter()
        .any(|prefix| is_ancestor(prefix, exe))
        || path_has_segment(exe, "Cellar")
    {
        return InstallMethod::Homebrew;
    }

    // cargo install destination (`~/.cargo/bin/octos-tui`). Sub-classify by the
    // recorded source from `.crates2.json`.
    if input
        .cargo_bin
        .as_ref()
        .is_some_and(|bin| is_ancestor(bin, exe))
        || path_has_segments(exe, &[".cargo", "bin"])
    {
        return match input.cargo_source_is_git {
            Some(true) => InstallMethod::CargoGit,
            // Default cargo installs come from the registry; treat unknown
            // source as registry so the printed command is the common case.
            Some(false) | None => InstallMethod::CargoRegistry,
        };
    }

    InstallMethod::Unknown
}

/// Returns true when `ancestor` is a path prefix of `path` (component-wise).
fn is_ancestor(ancestor: &Path, path: &Path) -> bool {
    if ancestor.as_os_str().is_empty() {
        return false;
    }
    let mut a = ancestor.components();
    let mut p = path.components();
    loop {
        match (a.next(), p.next()) {
            (Some(ac), Some(pc)) if ac == pc => continue,
            (Some(_), _) => return false, // ancestor longer / diverged
            (None, _) => return true,     // ancestor fully consumed → prefix
        }
    }
}

/// Whether any path component equals `segment`.
fn path_has_segment(path: &Path, segment: &str) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_string_lossy() == segment)
}

/// Whether `segments` appear as a contiguous run of path components.
fn path_has_segments(path: &Path, segments: &[&str]) -> bool {
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(segments.len())
        .any(|w| w.iter().zip(segments).all(|(a, b)| a == b))
}

/// Detect the install method against the live host.
///
/// When the `update` feature is on, a successful cargo-dist receipt load is
/// authoritative and short-circuits to [`InstallMethod::CargoDistInstaller`].
/// Otherwise (or when no receipt loads) we fall back to [`classify_path`].
pub fn detect() -> InstallMethod {
    if receipt_present() {
        return InstallMethod::CargoDistInstaller;
    }
    classify_path(&live_classifier_input())
}

/// Probe for a loadable cargo-dist install receipt. Only compiled with the
/// `update` feature; without axoupdater there is no receipt to load, so the
/// path heuristics decide.
#[cfg(feature = "update")]
fn receipt_present() -> bool {
    let mut updater = axoupdater::AxoUpdater::new_for("octos-tui");
    updater.load_receipt().is_ok()
}

#[cfg(not(feature = "update"))]
fn receipt_present() -> bool {
    false
}

/// Assemble the live classifier input from `current_exe()` + best-effort
/// package-manager prefix resolution.
fn live_classifier_input() -> PathClassifierInput {
    let current_exe = std::env::current_exe()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_default();

    PathClassifierInput {
        current_exe,
        brew_prefixes: brew_prefixes(),
        npm_global_roots: npm_global_roots(),
        cargo_bin: cargo_bin(),
        cargo_source_is_git: cargo_source_is_git(),
    }
}

/// Candidate Homebrew prefixes: the standard locations plus `brew --prefix`.
fn brew_prefixes() -> Vec<PathBuf> {
    let mut prefixes = vec![PathBuf::from("/opt/homebrew"), PathBuf::from("/usr/local")];
    if let Ok(out) = std::process::Command::new("brew").arg("--prefix").output()
        && out.status.success()
    {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            prefixes.push(PathBuf::from(p));
        }
    }
    prefixes
}

/// `npm root -g` and `npm prefix -g` (best effort).
fn npm_global_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for arg in [["root", "-g"], ["prefix", "-g"]] {
        if let Ok(out) = std::process::Command::new("npm").args(arg).output()
            && out.status.success()
        {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                roots.push(PathBuf::from(p));
            }
        }
    }
    roots
}

/// `~/.cargo/bin`, honoring `CARGO_HOME`.
fn cargo_bin() -> Option<PathBuf> {
    cargo_home().map(|home| home.join("bin"))
}

fn cargo_home() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CARGO_HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    home_dir().map(|h| h.join(".cargo"))
}

/// Inspect `~/.cargo/.crates2.json` for the recorded source of `octos-tui`.
/// Returns `Some(true)` if the source is a git URL, `Some(false)` for a
/// registry source, `None` if not found / unparseable.
fn cargo_source_is_git() -> Option<bool> {
    let path = cargo_home()?.join(".crates2.json");
    let contents = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let installs = value.get("installs")?.as_object()?;
    // Keys look like `octos-tui 0.1.1 (registry+https://…)` or
    // `octos-tui 0.1.1 (git+https://…)`.
    for key in installs.keys() {
        if key.starts_with("octos-tui ") {
            return Some(key.contains("(git+"));
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(exe: &str) -> PathClassifierInput {
        PathClassifierInput {
            current_exe: PathBuf::from(exe),
            ..Default::default()
        }
    }

    #[test]
    fn should_classify_npm_global_when_under_npm_root() {
        let mut i = input("/usr/local/lib/node_modules/@octos-org/octos-tui/bin/octos-tui");
        i.npm_global_roots = vec![PathBuf::from("/usr/local/lib/node_modules")];
        assert_eq!(classify_path(&i), InstallMethod::Npm);
    }

    #[test]
    fn should_classify_npm_when_node_modules_in_path() {
        // Even without a resolved root, a node_modules ancestor is conclusive.
        let i = input("/home/u/.nvm/versions/node/v20/lib/node_modules/octos-tui/octos-tui");
        assert_eq!(classify_path(&i), InstallMethod::Npm);
    }

    #[test]
    fn should_classify_homebrew_when_under_prefix() {
        let mut i = input("/opt/homebrew/bin/octos-tui");
        i.brew_prefixes = vec![PathBuf::from("/opt/homebrew")];
        assert_eq!(classify_path(&i), InstallMethod::Homebrew);
    }

    #[test]
    fn should_classify_homebrew_when_cellar_segment_present() {
        let i = input("/usr/local/Cellar/octos-tui/0.1.1/bin/octos-tui");
        assert_eq!(classify_path(&i), InstallMethod::Homebrew);
    }

    #[test]
    fn should_classify_cargo_registry_when_in_cargo_bin_without_git_source() {
        let mut i = input("/home/u/.cargo/bin/octos-tui");
        i.cargo_bin = Some(PathBuf::from("/home/u/.cargo/bin"));
        i.cargo_source_is_git = Some(false);
        assert_eq!(classify_path(&i), InstallMethod::CargoRegistry);
    }

    #[test]
    fn should_classify_cargo_git_when_source_is_git() {
        let mut i = input("/home/u/.cargo/bin/octos-tui");
        i.cargo_bin = Some(PathBuf::from("/home/u/.cargo/bin"));
        i.cargo_source_is_git = Some(true);
        assert_eq!(classify_path(&i), InstallMethod::CargoGit);
    }

    #[test]
    fn should_default_cargo_to_registry_when_source_unknown() {
        // `.cargo/bin` segment match with no resolved source → registry.
        let i = input("/home/u/.cargo/bin/octos-tui");
        assert_eq!(classify_path(&i), InstallMethod::CargoRegistry);
    }

    #[test]
    fn should_classify_unknown_for_distro_path() {
        let i = input("/usr/bin/octos-tui");
        assert_eq!(classify_path(&i), InstallMethod::Unknown);
    }

    #[test]
    fn npm_takes_precedence_over_cargo_bin_when_both_match() {
        // node_modules ancestor wins even if a cargo_bin prefix is also set.
        let mut i = input("/x/.cargo/bin/node_modules/octos-tui/octos-tui");
        i.cargo_bin = Some(PathBuf::from("/x/.cargo/bin"));
        assert_eq!(classify_path(&i), InstallMethod::Npm);
    }

    #[test]
    fn upgrade_commands_are_method_specific() {
        assert!(
            InstallMethod::CargoDistInstaller
                .upgrade_command()
                .is_none()
        );
        assert_eq!(
            InstallMethod::Homebrew.upgrade_command(),
            Some("brew update && brew upgrade octos-org/tap/octos-tui")
        );
        assert_eq!(
            InstallMethod::Npm.upgrade_command(),
            Some("npm update -g @octos-org/octos-tui")
        );
        assert_eq!(
            InstallMethod::CargoRegistry.upgrade_command(),
            Some("cargo install octos-tui --force")
        );
        assert_eq!(
            InstallMethod::CargoGit.upgrade_command(),
            Some("cargo install --git https://github.com/octos-org/octos-tui octos-tui --force")
        );
        assert!(
            InstallMethod::Unknown
                .upgrade_command()
                .unwrap()
                .contains("octos-tui-installer.sh")
        );
    }

    #[test]
    fn only_cargo_dist_is_self_updating() {
        assert!(InstallMethod::CargoDistInstaller.is_self_updating());
        for m in [
            InstallMethod::Homebrew,
            InstallMethod::Npm,
            InstallMethod::CargoRegistry,
            InstallMethod::CargoGit,
            InstallMethod::Unknown,
        ] {
            assert!(!m.is_self_updating(), "{} should not self-update", m.id());
        }
    }

    #[test]
    fn is_ancestor_is_component_wise_not_substring() {
        // `/opt/home` must NOT be an ancestor of `/opt/homebrew/bin` (substring
        // trap that a naive `starts_with` on strings would hit).
        assert!(!is_ancestor(
            Path::new("/opt/home"),
            Path::new("/opt/homebrew/bin/octos-tui")
        ));
        assert!(is_ancestor(
            Path::new("/opt/homebrew"),
            Path::new("/opt/homebrew/bin/octos-tui")
        ));
    }
}
