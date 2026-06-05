# octos-tui

Standalone terminal UI client for the Octos AppUi/UI Protocol.

`octos-tui` is intentionally separate from `octos-cli`. The `octos` repo owns
the server/runtime and shared `octos-core` protocol types; this repo owns the
terminal client and connects to `octos serve` over the UI Protocol WebSocket.

Architecture and ownership boundaries are documented in
`docs/ARCHITECTURE.md`.

## Repository Layout

For source builds, keep `octos` and `octos-tui` as sibling directories and
build both from their `main` branches:

```text
octos:     origin/main
octos-tui: origin/main
```

Clone the pair with:

```bash
mkdir octos-workspace
cd octos-workspace

git clone https://github.com/octos-org/octos.git
git clone https://github.com/octos-org/octos-tui.git
```

The sibling layout is required because `octos-tui` depends on the path
dependency declared in `Cargo.toml`:

```text
octos-core = { path = "../octos/crates/octos-core" }
```

If the sibling `../octos` checkout is missing, `cargo build`/`cargo test`
fail early with `failed to load manifest for dependency octos-core`. Verify
the pair from the `octos-tui` checkout with `cargo check` and `cargo test`.

For release packaging, pin `octos-core` to the matching Octos git tag or
published crate version.

## Prerequisites

- Rust 1.85 or newer
- `cargo`
- `tmux` for live visual/E2E tests
- a configured Octos provider key for live coding sessions, for example
  `DEEPSEEK_API_KEY`

Useful terminal defaults:

```bash
export TERM=xterm-256color
export RUST_LOG=off
```

## Build and Install

`octos-tui` is a single binary named `octos-tui`. Build it from the
`octos-tui` checkout with its sibling `../octos` present:

```bash
cd octos-tui

cargo build --release
# produces ./target/release/octos-tui
```

Install it onto `PATH` from the same checkout:

```bash
cargo install --path . --root ~/.local
# installs ~/.local/bin/octos-tui
```

There are no Cargo feature flags to choose: the default build includes both
the WebSocket and stdio AppUI transports. The release binary is what the tmux
harnesses run; point a harness at a specific build with `OCTOS_TUI_BIN`, for
example `OCTOS_TUI_BIN="$PWD/target/release/octos-tui"`.

## Quick Local Smoke Test

Mock mode does not need an Octos server. Use it for render, keyboard, and theme
smoke tests:

```bash
cd octos-tui

CARGO_TARGET_DIR=/tmp/octos-tui-target cargo test
CARGO_TARGET_DIR=/tmp/octos-tui-target cargo run -- --mode mock
CARGO_TARGET_DIR=/tmp/octos-tui-target cargo run -- --mode mock --theme claude
CARGO_TARGET_DIR=/tmp/octos-tui-target cargo run -- --mode mock --theme terminal
```

`CARGO_TARGET_DIR=/tmp/octos-tui-target` is optional. It avoids target directory
lock/permission issues on shared test hosts. On a normal checkout, `cargo test`
and `cargo run -- --mode mock` are enough.

## Run Octos Server

`octos-tui --mode protocol` needs a running `octos serve` process. The TUI does
not create the backend server.

From the sibling `octos` repo:

```bash
cd ../octos

export DEEPSEEK_API_KEY=...
export OCTOS_AUTH_TOKEN=local-dev-token

cargo run -p octos-cli --features api --bin octos -- serve \
  --host 127.0.0.1 \
  --port 50080 \
  --cwd "$PWD/e2e/fixtures/coding-agent-compare-multifile" \
  --data-dir /tmp/octos-tui-dev-data \
  --auth-token "$OCTOS_AUTH_TOKEN"
```

The server prints the dashboard URL:

```text
Dashboard: http://127.0.0.1:50080/admin/
```

Important server settings:

| Setting | Purpose |
|---|---|
| `--cwd` | Server-approved filesystem root for coding tools. TUI sessions can request this directory or an approved subdirectory as their session cwd. |
| `--data-dir` | Runtime state, sessions, auth, logs, and config. |
| `--auth-token` | Bearer token used by the dashboard and UI Protocol WebSocket. |

`octos-tui` does not own provider/model selection. `octos serve` loads the
unified provider, model portfolio, memory, tool policy, and sandbox settings
from the selected `--data-dir`, explicit server `--config`, or workspace
`.octos/config.json`.

## Run octos-tui Against the Server

Open a second terminal:

```bash
cd octos-tui

export OCTOS_AUTH_TOKEN=local-dev-token

CARGO_TARGET_DIR=/tmp/octos-tui-target cargo run -- \
  --mode protocol \
  --endpoint ws://127.0.0.1:50080/api/ui-protocol/ws \
  --session coding:local:readme-demo \
  --profile-id coding \
  --cwd "$PWD/../octos/e2e/fixtures/coding-agent-compare-multifile" \
  --theme codex
```

For a local AppUI process that speaks newline-delimited JSON-RPC over stdio,
use `--stdio-command` instead of `--endpoint`:

```bash
cargo run -- \
  --mode protocol \
  --stdio-command "octos serve --stdio --data-dir /tmp/octos-tui-dev-data --cwd $PWD/../octos/e2e/fixtures/coding-agent-compare-multifile" \
  --session coding:local:readme-demo \
  --profile-id coding
```

The stdio command intentionally omits `--provider` and `--model`. The child
`octos serve --stdio` process uses Octos' unified server config instead.

If `--cwd` is omitted, `octos-tui` requests the directory it was launched from,
matching Codex-style project selection. The server still decides whether that
cwd is approved.

You can also pass the token explicitly:

```bash
cargo run -- \
  --mode protocol \
  --endpoint ws://127.0.0.1:50080/api/ui-protocol/ws \
  --session coding:local:readme-demo \
  --profile-id coding \
  --auth-token local-dev-token
```

Read-only viewer mode opens a protocol session without sending turns:

```bash
cargo run -- \
  --mode protocol \
  --readonly \
  --endpoint ws://127.0.0.1:50080/api/ui-protocol/ws \
  --session coding:local:readme-demo \
  --profile-id coding
```

Available themes:

```text
codex, claude, slate, solarized, terminal
```

The `terminal` theme keeps foreground and background surfaces on the terminal
defaults where ratatui supports it, using only restrained ANSI colors for
borders, accents, and error states.

Diff context controls:

```text
[` / `]` select previous/next inline diff hunk
`c` stages the selected hunk as next-turn context
```

Local composer slash commands:

```text
/ps shows local task/process status and focuses the Tasks pane
/stop interrupts the active turn, or reports locally when nothing is active
/help shows local slash-command help
```

Unknown slash commands are handled locally with a warning and are not sent to
the model.

The current AppUi/UI Protocol v1 bridge stages selected diff context as prompt
text. Structured context attachments are tracked as a formal UPCR in
`docs/M9_31_CONTEXT_ATTACHMENTS_UPCR.md`.

The visual parity harness assertions for Codex-vs-Octos tmux comparison are
tracked in `docs/M9_33_VISUAL_PARITY_HARNESS.md`.

The coding-agent prompt shape that supports clean plan and summary rendering is
documented in `docs/CODING_UX_PROMPT_CONTRACT.md`. The TUI renders defensively,
but this contract belongs in the server profile or harness prompt.

## Onboarding (First Launch)

When you connect to a backend that has no usable profile yet, `octos-tui` opens
the onboarding wizard automatically on first launch. It auto-opens only when
the server advertises a profile-creation surface (local solo, or legacy email
OTP); a provider/model-only catalog does not trigger it. You can also open it
on demand with the `/setup` slash command.

A clean first launch needs a fresh, empty server data directory and no
`--profile-id`. Against a spawned `octos serve --stdio` backend that is the
common solo path:

```bash
cargo run -- \
  --mode protocol \
  --stdio-command "octos serve --stdio --solo --data-dir /tmp/octos-tui-fresh-data --cwd $PWD/../octos/e2e/fixtures/coding-agent-compare-multifile"
```

`--solo` and `--data-dir` are arguments to the spawned `octos serve --stdio`
child, not `octos-tui` flags. If you pass `--profile-id`, the TUI uses that
profile and skips the welcome screen, so omit it for a true first-run flow.

The first-launch flow has three steps:

1. **Welcome screen.** The wizard opens on a screen titled "Welcome to Octos"
   with the subtitle "Set up a local solo profile to continue." The OCTOS ASCII
   wordmark renders in the main window above the menu (it is not in a
   right-side preview pane), and collapses to a one-line tagline on short
   terminals so the menu and its Continue action are never clipped. Fill in
   **Full name**, **Username**, and **Email** (email is local metadata only;
   no OTP is sent for a local solo profile), then choose **Continue** to create
   the profile via `profile/local/create`. You can type each field inline with
   `/onboard name <…>`, `/onboard username <…>`, and `/onboard email <…>`.
2. **Set Up LLM Provider.** After the profile is created the same screen (now
   titled "Set Up LLM Provider") lets you load the dashboard provider catalog,
   choose a model family, model, and route, enter the masked API key, optionally
   test the provider, and save it to the profile JSON via `profile/llm/upsert`.
   The catalog and provider schema are owned by `octos`/the dashboard; the TUI
   never hard-codes provider/model truth.
3. **Open coding session.** Once a provider is saved, choose **Open coding
   session** to call `session/open` with the resolved profile and drop into the
   normal coding UI.

The onboarding/auth/provider methods (`auth/*`, `profile/local/create`,
`profile/llm/*`, `config/capabilities/list`) are server-owned AppUI methods
consumed over the WebSocket or stdio transport, governed by `UPCR-2026-016` in
the `octos` repo.

The reference end-to-end flow lives in `scripts/run-onboarding-tmux-soak.sh`
(it starts a server, launches the TUI, and waits for the "Welcome to Octos"
splash); see `docs/ONBOARDING_TMUX_SOAK.md`.

## Dashboard and Server Startup

The dashboard is served by `octos serve`, so it cannot start the parent
`octos serve` process from nothing. Something outside the dashboard must start
the server first:

- a shell command such as `octos serve ...`
- the install script's OS service
- a launchd, systemd, or Windows Scheduled Task configuration

Once `octos serve` is running, the dashboard can configure profiles, provider
settings, API keys, channels, and gateway child processes. It can start,
stop, and restart those gateway profiles through the admin/user APIs, and the
server watches profile changes for gateway restarts.

For `octos-tui` coding sessions, the AppUi backend agent is created when
`octos serve` starts. If you add or change the provider/model in the dashboard
after the server is already running, restart `octos serve` before using
`octos-tui` for a live coding session.

Installed service controls:

```bash
# macOS, installed LaunchDaemon
sudo launchctl unload /Library/LaunchDaemons/io.octos.serve.plist
sudo launchctl load /Library/LaunchDaemons/io.octos.serve.plist

# Linux, installed systemd unit
sudo systemctl restart octos-serve

# Windows PowerShell, installed scheduled task
Stop-ScheduledTask -TaskName OctosServe
Start-ScheduledTask -TaskName OctosServe
```

## Cwd Behavior

`octos-tui` requests a session cwd through `session/open`. By default that cwd
is the terminal launch directory; `--cwd DIR` overrides it. `octos serve`
canonicalizes the requested directory and accepts it only if it is inside the
server-approved filesystem roots.

For local development, start the server at the broad workspace root and launch
the TUI from the project directory:

```bash
cd /path/to/workspace-root
octos serve --cwd "$PWD" ...

cd /path/to/workspace-root/my-project
octos-tui --mode protocol --endpoint ws://127.0.0.1:50080/api/ui-protocol/ws ...
```

For remote servers, pass a cwd that exists on the server host:

```bash
octos-tui --cwd /Users/cloud/work/my-project --mode protocol ...
```

If the requested cwd is outside the approved roots, `session/open` fails with a
typed protocol error instead of silently running tools in the wrong directory.

## Tmux AppUi Smoke Tests

The Octos repo contains the tmux harness because it starts both the server and
the standalone TUI:

```bash
cd ../octos

export OCTOS_TUI_DIR="$PWD/../octos-tui"
bash e2e/tmux/run.sh default
```

The default tmux lane checks help output, mock TUI rendering, bad endpoint
handling, read-only protocol bootstrap, approval cards, and basic protocol UI
states. It stores captures under:

```text
e2e/test-results-tmux/
```

## Live Codex-Parity E2E Test

Use the live parity harness to compare `octos-tui` with Codex on the same
multi-file Rust coding fixture and the same model label:

```bash
cd ../octos

export DEEPSEEK_API_KEY=...
# Optional only for a live-comparison model override; otherwise Octos config wins.
# export DEEPSEEK_MODEL=...
export OCTOS_TUI_DIR="$PWD/../octos-tui"
export OCTOS_TUI_UX_KEEP_SESSIONS=1
export OCTOS_TMUX_KEEP=1
export OCTOS_TUI_UX_EXIT_HOLD_SECS=1800

scripts/compare-tui-coding-ux-tmux.sh
```

The runner launches:

- real `octos serve`
- real `octos-tui --mode protocol`
- Codex in a separate tmux session
- independent fixture workspaces for each lane

It prints session names like:

```text
[tui-ux] octos-tui session: octos-tmux-<run-id>-octos-tui-client
[tui-ux] codex session: octos-tmux-<run-id>-codex-client
```

Watch them live:

```bash
tmux attach -r -t <octos-tui-client-session>
tmux attach -r -t <codex-client-session>
```

For remote hosts:

```bash
ssh -t cloud@<host> 'tmux attach -r -t <octos-tui-client-session>'
ssh -t cloud@<host> 'tmux attach -r -t <codex-client-session>'
```

Analyze a completed run:

```bash
scripts/analyze-coding-ux-transcripts.sh e2e/test-results-tui-coding-ux/<run-id>
```

Expected artifacts:

```text
summary.env
ux-summary.env
prompts.txt
octos-tui-transcript.log
octos_tui-worktree.diff
octos_tui-cargo-test.log
octos_tui-git-status.txt
codex-transcript.log
codex-worktree.diff
codex-cargo-test.log
codex-git-status.txt
```

## What To Check Visually

During live review, `octos-tui` should show:

- chat-first layout with a stable composer
- user and assistant messages visually distinguished
- live progress while the model is working
- exact command/tool cards with command, cwd, status, elapsed time, and output
- inline approval cards for command/sandbox/network/diff requests
- inline colored diffs in the chat flow, not only modal overlays
- final session summary with files changed, validation, and next steps
- no raw tracing logs, protocol frames, timestamps, or API keys in the UI

## Configuration Reference

`octos-tui` flags:

```text
--config <json-file>
--mode mock|protocol
--endpoint ws://127.0.0.1:50080/api/ui-protocol/ws
--stdio-command "octos serve --stdio --data-dir ~/.octos --cwd /path/to/project"
--session <session-id>
--profile-id <profile-id>
--cwd <workspace-dir>
--auth-token <token>
--readonly
--no-readonly
--theme codex|claude|slate|solarized|terminal
```

`--config` reads a JSON launch config. CLI flags override JSON values:

```json
{
  "mode": "protocol",
  "stdio_command": "octos serve --stdio --data-dir ~/.octos --cwd /path/to/project",
  "session": "coding:local:main",
  "profile_id": "coding",
  "cwd": "/path/to/project",
  "readonly": false,
  "theme": "codex"
}
```

Do not put `provider` or `model` in the TUI config. Those are server-owned
Octos runtime settings loaded by `octos serve`.

Environment variables:

| Variable | Purpose |
|---|---|
| `OCTOS_AUTH_TOKEN` | Fallback bearer token for the UI Protocol WebSocket. |
| `RUST_LOG=off` | Keeps terminal output clean for live visual runs. |
| `TERM=xterm-256color` | Avoids missing terminfo/color issues on remote hosts. |
| `OCTOS_TUI_DIR` | Points Octos harness scripts at this standalone TUI repo. |
| `OCTOS_TUI_BIN` | Forces a specific built `octos-tui` binary. |
| `OCTOS_TUI_UX_KEEP_SESSIONS=1` | Keeps live parity tmux sessions open for inspection. |
| `OCTOS_TMUX_KEEP=1` | Keeps tmux sessions/artifacts after harness runs. |

## Troubleshooting

| Symptom | Fix |
|---|---|
| `octos-core` dependency not found | Keep `octos` and `octos-tui` as sibling directories. |
| `target` lock or permission error | Run with `CARGO_TARGET_DIR=/tmp/octos-tui-target`. |
| Endpoint rejected | Use `ws://` or `wss://`; HTTP URLs are rejected by the CLI. |
| Auth failure | Use the same token for `octos serve --auth-token` and `octos-tui --auth-token` or `OCTOS_AUTH_TOKEN`. |
| TUI opens but no live answer | Confirm `octos serve` has a provider/model/key and restart it after dashboard config changes. |
| Wrong workspace | Start `octos serve` with the desired `--cwd`. |
| `can't find terminfo database` | Set `TERM=xterm-256color` or install terminfo on the host. |
| Raw logs or timestamps appear in the UI | Start both server and TUI with `RUST_LOG=off`. |

## Protocol Contract

`octos-tui` consumes AppUi/UI Protocol fields from `octos-core`. It must not
invent local wire extensions. Any protocol behavior change must land through a
formal UI Protocol change request, shared protocol types, server tests, golden
protocol tests, and TUI reducer/rendering tests.

`octos-tui` currently requests `pane.snapshots.v1` in protocol mode and hydrates
optional pane data from `session/open.panes` when the server supports it. When
the field is absent, it falls back to session snapshots, task tails, launch
target, and status.

Auth, onboarding, and profile LLM provider setup are governed by
`UPCR-2026-016` in the `octos` repo. The TUI must consume
`auth/*`, `profile/llm/*`, and `config/capabilities/list` as server-owned
AppUI methods over WebSocket or stdio; it must not hard-code provider/model
truth or persist a parallel LLM registry.
