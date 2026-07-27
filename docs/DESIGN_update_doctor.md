# Design: `octos-tui update` and `octos-tui doctor`

**Status:** design / RFC.
**Target repos:** `octos-org/octos-tui` (primary), `octos-org/octos` (shared bits + future `octos doctor`/`octos update`).
**Date:** 2026-06-05.

---

## 0. TL;DR

1. **`update`**: build on **`axoupdater`** (the crate behind cargo-dist's updater), driven by the **install receipt** cargo-dist writes. The receipt is the discriminator: **if a receipt exists, the binary came from the cargo-dist shell/PowerShell installer and we self-update in place; if `load_receipt()` returns `NoReceipt`, the binary came from brew / npm / `cargo install` / a distro package, and we print the correct package-manager command instead of clobbering a file we don't own.** This is the rustup model ("self-update is disabled for this build … use your package manager") generalized across all of octos-tui's install methods. Embed axoupdater as a library under an `octos-tui update` subcommand — do **not** ship the standalone `octos-tui-update` binary.
2. **`doctor`**: a **flutter-doctor-style** categorized report (`[✓]/[!]/[✗]` per check, each failing check carries one actionable fix line), `--json` for support bundles, `--verbose` for detail. The headline novel check is **protocol/version skew** between octos-tui's pinned `octos-core` (`UI_PROTOCOL_SCHEMA_VERSION`) and the live server's `config/capabilities/list` payload — the top real-world failure mode for two independently-versioned repos.
3. **Sharing**: put install-method detection + the generic checks (binary/PATH, TERM/locale, network/GitHub, update-check) in a small shared module (`octos-core::diagnostics`, feature-gated) so `octos doctor`/`octos update` on the **server** binary reuse them once octos is itself cargo-dist-packaged.

---

## A. `octos-tui update`

### A.1 Mechanism: axoupdater + cargo-dist receipts

cargo-dist (`dist` 0.32) produces, for the **shell** and **PowerShell** installers, an **install receipt** — JSON at `~/.config/octos-tui/octos-tui-receipt.json` (Linux/macOS) or `%LOCALAPPDATA%\octos-tui\octos-tui-receipt.json` (Windows). Shape (`axoupdater/src/receipt.rs`):

```rust
pub struct InstallReceipt {
    pub install_prefix: Utf8PathBuf,   // where the binary lives
    pub binaries: Vec<String>,         // ["octos-tui"]
    pub cdylibs: Vec<String>,
    pub source: ReleaseSource,         // owner/name/app_name + GitHub vs Axo
    pub version: String,               // "0.1.1"
    pub provider: ReceiptProvider { pub source: String, pub version: String },
    pub modify_path: bool,
}
```

**Only the shell/PowerShell installers write this receipt.** npm, Homebrew, `cargo install`, and distro packages do not. So the presence/absence of a loadable receipt is itself the install-method signal — no path heuristics needed for the happy path.

`dist-workspace.toml` (already partly set in the install PR):

```toml
[dist]
installers = ["shell", "powershell", "npm", "homebrew"]
install-updater = true          # makes cargo-dist emit + maintain the receipt
github-attestations = true      # provenance attestations on release artifacts
```

`octos-tui/Cargo.toml`:

```toml
[dependencies]
axoupdater = { version = "0.6", default-features = false, features = ["github_releases", "blocking"] }
```

### A.2 The subcommand (embedded, not standalone)

`octos-tui update [--check] [--version X.Y.Z | --tag vX.Y.Z] [--prerelease] [--force] [--yes] [--json]`

- detect install method (A.3); if cargo-dist installer → axoupdater self-update; else → print the right package-manager command.
- axoupdater wiring: `AxoUpdater::new_for("octos-tui")` → `load_receipt()` → `set_current_version(CARGO_PKG_VERSION)` → optional `set_github_token(OCTOS_TUI_GITHUB_TOKEN)` → `configure_version_specifier(UpdateRequest::{Latest|LatestMaybePrerelease|SpecificVersion|SpecificTag})` → `always_update(force)` → `--check` uses `query_new_version()`, otherwise `run_sync()`.
- Confirmed axoupdater API: `new_for`, `load_receipt`, `set_current_version`, `set_github_token`, `configure_version_specifier`, `always_update`, `query_new_version`, `is_update_needed_sync`, `run_sync`. `UpdateResult { old_version: Option<Version>, new_version, new_version_tag, install_prefix }`.

### A.3 Install-method detection + per-method behavior

Detection order (first match wins):
1. **cargo-dist installer** → `load_receipt()` succeeds (authoritative; receipt pins `install_prefix`).
2. else classify by `current_exe()` location + corroborating signal:
   - Homebrew prefix (`/opt/homebrew/`, `/usr/local/Cellar/`, `$(brew --prefix)`) or `brew list octos-tui` → **Homebrew**.
   - npm global root (`npm root -g`/`npm prefix -g`, or `node_modules/@octos-org/octos-tui` ancestor) → **npm**.
   - `~/.cargo/bin` + `~/.cargo/.crates2.json` mentions octos-tui → **cargo**; sub-classify `--git` vs registry by the recorded `source`.
   - else → **Unknown / distro**.

| Detected method | `update` does | `--check` does |
|---|---|---|
| cargo-dist installer (receipt) | self-update in place via axoupdater (verify + atomic swap; respects `--version`/`--tag`/`--prerelease`/`--force`) | `query_new_version()`; print + exit 10 if newer |
| Homebrew | print `brew update && brew upgrade octos-org/octos-tui/octos-tui`; exit 3 | best-effort `brew outdated --json`; else print command, exit 0 |
| npm (`-g`) | print `npm update -g @octos-org/octos-tui`; exit 3 | `npm outdated -g @octos-org/octos-tui` |
| cargo install (registry) | print `cargo install octos-tui --force` (+ suggest `cargo install-update`); exit 3 | compare to crates.io / latest tag |
| cargo install --git | print `cargo install --git https://github.com/octos-org/octos-tui octos-tui --force`; exit 3 | compare `CARGO_PKG_VERSION` to repo latest tag |
| Unknown / distro | print manual instructions + suggest the curl\|sh installer to convert to a self-updating install; exit 3 | GitHub-latest compare only |

### A.4 UX, exit codes, security

- Exit codes: `0` success / up-to-date; `10` update available (for `--check`, scriptable); `3` "can't self-update here, here's the command"; `1` hard error.
- Security: rely on cargo-dist artifact integrity (SHA-256 per artifact + GitHub build provenance attestations); axoupdater downloads only from the pinned GitHub Release named in the receipt's `source`; HTTPS only; never cross `owner/name`; never `sudo`-escalate (fail with exit 1 if prefix unwritable).
- **macOS:** re-`codesign --force -s -` the updated binary post-swap — replacing the bit-pattern SIGKILLs on Sequoia even when bit-identical; axoupdater does not do this.
- Pre-flight: print current → target version + source URL before mutating; require confirmation in TTY unless `--yes`.

### A.5 Generalizing to the `octos` server binary

octos already has `crates/octos-cli/src/updater.rs`, but it is **macOS-aarch64-only** (`ASSET_NAME = octos-bundle-aarch64-apple-darwin.tar.gz`), has **no install-method awareness** (clobbers brew/distro binaries), and re-implements backup/rollback/codesign by hand. To reach parity: cargo-dist-package octos (declare the whole bundle in `[dist].binaries` so the receipt covers all of them), set `install-updater = true`, replace updater.rs's bespoke logic with the shared `InstallMethod` + axoupdater wrapper, but **keep** the macOS re-codesign + skills-dir-clean post-steps. Until then, gate updater.rs behind the same install-method check so a brew/distro octos defers instead of clobbering.

---

## B. `octos-tui doctor`

Flutter-doctor output: one line per check, `[✓]` pass / `[!]` warn / `[✗]` fail, grouped by category, each non-pass line followed by an indented `→ fix:` action, ending with a one-line summary. `--json` emits the same data structured (support bundle; redact tokens); `--verbose` adds resolved paths/versions.

### Checks

**Binary & version**
- octos-tui on PATH (resolve `which` vs `current_exe()`).
- newer release available (reuse §A `--check`, method-aware fix command).
- **no shadowing installs** — enumerate every octos-tui on `$PATH` + known prefixes (cargo bin, npm global, brew); warn if >1 with all paths + which wins (the Claude Code #22415 failure mode).

**Terminal environment**
- TERM set + terminfo loadable (pre-empts the README's `can't find terminfo database`); fix `export TERM=xterm-256color`.
- UTF-8 locale (`LANG`/`LC_ALL`/`LC_CTYPE`).
- CJK width (octos-tui uses `unicode-width`; CJK double-width; informational — also depends on terminal font).
- color support (`COLORTERM`/`TERM` truecolor/256).

**Config & data**
- config dir present + writable (`~/.octos` or `--data-dir`).
- auth file valid (parses, token present/not-expired; never print the token).
- data-dir exists + writable.
- profile present (warn on implicit `_main` fallback).

**Backend connectivity** (high-value)
- stdio mode: `--stdio-command` binary resolvable (`octos` on PATH, `octos --version` runs; surface server build hash).
- WS mode: socket reachable (open `…/api/ui-protocol/ws`, auth, `config/capabilities/list`).
- **protocol skew (both modes)** — the load-bearing check. The TUI is built against a **pinned `octos-core`** exposing `UI_PROTOCOL_SCHEMA_VERSION` / capability-schema version / feature consts (`pane.snapshots.v1`, `harness.task_control.v1`, `projection.envelope.v1`, …). Call the server's `config/capabilities/list` → `UiProtocolCapabilities { protocol_version, schema_version, capabilities_schema_version, supported_methods, supported_features, unsupported }` and run octos-core's compatibility check (same `protocol_version`, client `schema_version ≤ server`). `[✗]` on incompatible schema; `[!]` when the server is missing a feature the TUI requires (or vice-versa). Fix line tells the user which side to move.
- capability set — list features the TUI needs vs what the server advertises; warn (not fail) on optional gaps.

**Network**
- GitHub reachable (HEAD `api.github.com` so update checks aren't silently dead behind a proxy).

### `--json` / `--verbose` / auto-fix

- `--json`: array of `{category, name, status, detail, fix, value}` + `{summary, exit_code, octos_tui_version, octos_core_schema_version, platform}`. Support-bundle format (ask users to paste `octos-tui doctor --json`). Redact tokens.
- Auto-fix (`--fix`): only safe, local, reversible actions (create missing data-dir, write default config, *print* the upgrade command). **Never** auto-run brew/npm/binary swaps (that's `update`); never edit shell rc files (print the export line).
- Exit codes: `0` all pass (warnings → 0 but mention), `1` ≥1 `[✗]`; optional `--strict` makes warnings fail.

### Sharing with server `octos doctor`

Make `doctor` a thin renderer over a library. Shared, binary-agnostic checks (install-method detection + update-check, TERM/locale/UTF-8/color, config/data/auth validity, GitHub reachability, the protocol capability compare) live in `octos-core::diagnostics`. TUI-only checks (terminfo specifics, stdio-command parsing) stay in octos-tui; server-only checks (port bind, sandbox backend, provider creds, MCP reachability) live in octos-cli. The renderer + `--json` schema are shared so both binaries look identical.

---

## C. Implementation plan

- **octos-tui crate**: `src/cmd/update.rs` (+ `InstallMethod`), `src/cmd/doctor.rs` (+ renderer). New dep `axoupdater` behind a default-on `update` feature (off for distro packaging — matches rustup's `no-self-update`; with the feature off, `update` still works as a pure advisor and `doctor` still runs). Reuse existing `reqwest`/`serde_json`; `which`/`current_exe` for detection.
- **octos-core**: `pub mod diagnostics` (feature `diagnostics`) with the shared checks + `Check`/`CheckStatus`/`Report` types + a `compare(client_schema, server_caps) -> Vec<Check>` helper (it already has `UiProtocolCapabilities` + version-compat logic).
- **octos-cli** (later): `octos doctor`/`octos update` calling `octos-core::diagnostics` + a refactored install-method-aware `updater.rs`.

### Phasing (ship order)

| Phase | Scope | Effort |
|---|---|---|
| **P0** | `dist-workspace.toml` + first cargo-dist release with receipts. **≈ the install PR — done.** | ~0.5d (CI) |
| **P1** | `update`: detection + axoupdater self-update + package-manager deferral + `--check`/`--version`/`--prerelease`/exit codes. | ~2d |
| **P2** | `doctor` v1: binary/version, TERM/locale/color, config/data (no server needed). `--json`. | ~2d |
| **P3** | `doctor` backend connectivity + **protocol-skew check** (stdio + WS + capability compare). The differentiator. | ~2d |
| **P4** | extract shared checks into `octos-core::diagnostics`; refactor `octos-cli/src/updater.rs`; add `octos doctor`/`octos update`. | ~3d |

### Risks

- axoupdater has **no** built-in "defer to brew/npm" — *we* provide it by treating `NoReceipt` as "not our path". Guard against a future cargo-dist writing receipts for npm/brew by also checking `provider.source`/install prefix before self-updating.
- Independent-versioning skew is **structural** — doctor detects it but can't fix it. Mitigate by having the server surface its octos-core schema at *connect* time (warn early), and document a compatibility window (server supports client schema N and N-1).
- Windows: PS installer writes the receipt under `%LOCALAPPDATA%` (self-update works); macOS codesign step is macOS-only; brew/npm detection must handle Windows paths; verify the self-replace-while-running rename dance on Windows.
- GitHub rate limits on `--check`/doctor probes — honor `OCTOS_TUI_GITHUB_TOKEN`, cache last-check timestamp, make the network check `[!]`-warn (not `[✗]`) on 403/timeout.
- macOS Gatekeeper: self-updated binaries must be re-signed or SIGKILL on Sequoia even when bit-identical.

### Sources

axoupdater (README + `src/lib.rs` + `src/receipt.rs`); cargo-dist updater/receipts/`install-updater` docs; rustup "self-update disabled … use your package manager"; gh/deno package-manager deferral; Claude Code `claude doctor` + release channels + shadowing bug (#22415); flutter doctor UX; octos codebase (`crates/octos-cli/src/updater.rs`, `crates/octos-core/src/ui_protocol.rs`, octos-tui README).
