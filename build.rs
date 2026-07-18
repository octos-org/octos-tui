use std::process::Command;

fn main() {
    // Git short hash — so `octos-tui --version` identifies the exact build
    // (a branch/dev build reports e.g. `0.2.2-rc.7 (94e43fd …)`), mirroring the
    // octos server. Without it, an unreleased branch build is indistinguishable
    // from the published release of the same Cargo version.
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=OCTOS_TUI_GIT_HASH={hash}");

    // Build date (YYYY-MM-DD)
    let date = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=OCTOS_TUI_BUILD_DATE={date}");

    // Re-run if git HEAD changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
}
