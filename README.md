# octos-tui

<div align="center">
<pre>
 ██████╗  ██████╗████████╗ ██████╗ ███████╗
██╔═══██╗██╔════╝╚══██╔══╝██╔═══██╗██╔════╝
██║   ██║██║        ██║   ██║   ██║███████╗
██║   ██║██║        ██║   ██║   ██║╚════██║
╚██████╔╝╚██████╗   ██║   ╚██████╔╝███████║
 ╚═════╝  ╚═════╝   ╚═╝    ╚═════╝ ╚══════╝
</pre>
<em>Welcome to Octos — Your Coding Buddy</em>
</div>

`octos-tui` is a standalone terminal UI client for the Octos UI Protocol.
It is a chat-first coding client that you point at an `octos serve` backend; the
server owns the agent, providers, and tools, and the TUI renders the
conversation, command/tool cards, diffs, and approvals.

On a fresh first launch the main window shows the **OCTOS** block-letter
wordmark with the tagline *"Welcome to Octos — Your Coding Buddy"* above a
short onboarding menu — your starting point for the walkthrough below.

`octos-tui` is intentionally separate from `octos-cli`: the `octos` repo owns
the server/runtime and the shared `octos-core` protocol types; this repo owns
the terminal client. Architecture and ownership boundaries live in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Install

Every method installs a single self-contained `octos-tui` binary. Then run
`octos-tui --help`.

### Prebuilt binary — no Rust toolchain needed (recommended)

Same model as Claude Code and Codex: each [GitHub Release](https://github.com/octos-org/octos-tui/releases)
ships prebuilt binaries for macOS (Apple Silicon), Linux (x86-64 +
arm64), and Windows (x86-64), distributed via:

```bash
# npm
npm install -g @octos-org/octos-tui

# Homebrew
brew install octos-org/tap/octos-tui

# shell installer (macOS / Linux)
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/octos-org/octos-tui/releases/latest/download/octos-tui-installer.sh | sh
```

### From source with Cargo (needs Rust 1.85+)

```bash
# straight from git — no crates.io publish required
cargo install --git https://github.com/octos-org/octos-tui octos-tui

# or, once published to crates.io
cargo install octos-tui
```

> `octos-core` (the shared protocol crate) is pulled automatically as a git
> dependency, so installing needs no sibling `octos` checkout.

---

## Quickstart: solo onboarding

A copy-pasteable, first-time walkthrough. By the end you have a local profile,
an LLM provider, and a live coding session — no dashboard, no email OTP.

### 1. Get the source and build

`octos-tui` is a single binary. `octos-core` (the shared protocol crate) is
pulled automatically as a git dependency, so a plain clone builds with **no
sibling checkout** required (needs Rust 1.85+):

```bash
git clone https://github.com/octos-org/octos-tui.git
cd octos-tui
cargo build --release
# produces ./target/release/octos-tui
```

> **Developing against a local `octos`?** To build against an uncommitted
> sibling `../octos/crates/octos-core` instead of the pinned git revision, run
> `cp .cargo/config.toml.example .cargo/config.toml` (gitignored) — see the
> comment in that file. This restores the live-sibling edit loop of the old
> path dependency.

### 2. First run → the welcome screen

For a true first run, spawn an `octos serve --stdio` backend with a **fresh,
empty** data directory and pass **no** `--profile-id`. The TUI launches the
server as a child process over stdio, so you only run one command:

```bash
./target/release/octos-tui \
  --mode protocol \
  --stdio-command "octos serve --stdio --solo --data-dir ./octos-data"
```

You land on the **"Welcome to Octos"** screen (subtitle *"Set up a local solo
profile to continue."*), with the OCTOS wordmark above the menu.

Notes:

- `--solo` and `--data-dir` are arguments to the spawned `octos serve --stdio`
  child, **not** `octos-tui` flags.
- Use a brand-new `--data-dir`; an existing one may already have a profile and
  skip the welcome screen.
- Do **not** pass `--profile-id` on first run — it selects an existing profile
  and bypasses onboarding.

### 3. Create your local profile

On the welcome screen, fill the three fields (the email is local metadata only —
no OTP is sent):

| Field | How to enter it |
|---|---|
| **Full name** | select the row and type, or `/onboard name <your name>` |
| **Username** | select the row and type, or `/onboard username <handle>` |
| **Email** | select the row and type, or `/onboard email <address>` |

Then choose **"Create your local Octos profile" / Continue**. This calls
`profile/local/create` and advances to provider setup.

### 4. Set up an LLM provider

The screen now reads **"Set Up LLM Provider"** (*"Choose a dashboard model
route, enter its API key, then save."*). Work down the rows:

1. **Load provider catalog** — pulls the dashboard's model families and routes.
2. **Model family → Model → Provider route** — pick one route.
3. **API key** — select the row and type the key (`/onboard key <secret>`); it
   is masked in state, logs, and snapshots.
4. (optional) **Test provider** to verify the route.
5. **Save provider to profile** — persists it via `profile/llm/upsert` (the same
   profile JSON the dashboard writes).

The catalog and provider schema are owned by `octos`/the dashboard; the TUI
never hard-codes provider/model truth.

### 5. Open a coding session and chat

Once a provider is saved, choose **"Open coding session"**. This calls
`session/open` with the resolved profile and drops you into the normal coding
UI. Type a request in the composer and press Enter — you're chatting with Octos.

You can reopen this wizard at any time with the `/setup` slash command.

---

## Other ways to run

### Connect to a running `octos serve` over WebSocket

If a server is already running (locally or remote), connect over its UI Protocol
WebSocket instead of spawning a child. Start the server from the sibling repo:

```bash
cd ../octos
export OCTOS_AUTH_TOKEN=local-dev-token
cargo run -p octos-cli --features api --bin octos -- serve \
  --host 127.0.0.1 --port 50080 \
  --cwd "$PWD" \
  --data-dir /tmp/octos-tui-dev-data \
  --auth-token "$OCTOS_AUTH_TOKEN"
```

Then connect in another terminal:

```bash
octos-tui \
  --mode protocol \
  --endpoint ws://127.0.0.1:50080/api/ui-protocol/ws \
  --auth-token local-dev-token \
  --cwd "$PWD/my-project"
```

Use the **same** token for `--auth-token` on both sides (or set
`OCTOS_AUTH_TOKEN`). Add `--profile-id <id>` to open an existing profile and
skip onboarding; add `--readonly` for a view-only session that never sends
turns.

### Mock mode (no server)

For render/keyboard/theme smoke tests with no backend at all:

```bash
cargo run -- --mode mock
cargo run -- --mode mock --theme claude
```

`--mode mock` is also the default when no `--endpoint`/`--stdio-command` is
given.

---

## Reference

### CLI flags

```text
--config <json-file>     JSON launch config; CLI flags override its values
--mode mock|protocol     mock (no server) or protocol (live). Default: mock
--endpoint <ws-url>      UI Protocol WebSocket (ws:// or wss://)
--stdio-command "<cmd>"  spawn an `octos serve --stdio` child instead of --endpoint
--session <session-id>   session to open first
--profile-id <id>        existing profile to use (skips onboarding)
--cwd <dir>              workspace cwd to request; defaults to the launch dir
--auth-token <token>     bearer token; falls back to OCTOS_AUTH_TOKEN
--readonly / --no-readonly   open as a view-only session, or force read-write
--theme <name>           codex | claude | slate | solarized | terminal
--lang en|zh             UI language; falls back to OCTOS_LANG / LANG. Default: en
--scroll-mode <mode>     native (terminal scrollback, default) | pinned (composer pinned)
```

`--endpoint` and `--stdio-command` are mutually exclusive — pick one transport.
Do **not** put `provider` or `model` anywhere: those are server-owned Octos
settings loaded by `octos serve`, and the TUI config rejects them.

### Config file

`--config FILE` reads JSON launch defaults (CLI flags win on conflict):

```json
{
  "mode": "protocol",
  "stdio_command": "octos serve --stdio --solo --data-dir ./octos-data",
  "session": "coding:local:main",
  "profile_id": "coding",
  "cwd": "/path/to/project",
  "readonly": false,
  "theme": "codex",
  "lang": "en",
  "scroll-mode": "native"
}
```

`/saveconfig` writes the active `theme` / `lang` / `scroll-mode` back into this
file (merging — it never clobbers transport keys like `stdio_command`); without
`--config` it falls back to `~/.config/octos-tui/config.json`.

### Themes

```text
codex, claude, slate, solarized, terminal
```

`terminal` keeps foreground/background on your terminal defaults where ratatui
allows it, using only restrained ANSI colors for borders, accents, and errors.

Set the palette at launch with `--theme <name>`, or switch live with `/theme`
(a `*`-marked menu; the change repaints immediately and survives reconnects).

### In-session keys and slash commands

```text
[ / ]   select previous / next inline diff hunk
c       stage the selected hunk as next-turn context
```

```text
/help       local slash-command help
/ps         show local task/process status and focus the Tasks pane
/stop       interrupt the active turn (or report locally if none is active)
/setup      reopen the onboarding wizard
/model      browse the server-returned profile models / catalog
/theme      switch the TUI palette at runtime (menu, or /theme claude)
/lang       switch the UI language (menu, or /lang zh) — English / 中文
/thinking   set reasoning effort for thinking models, per session (menu, or /thinking high)
/scrollmode switch wheel-scroll behavior (toggle, or /scrollmode native|pinned)
/saveconfig persist the active theme / language / scroll-mode to the config file
/onboard    set onboarding fields inline (name, username, email, key, ...)
```

`/model`, `/theme`, `/lang`, and `/thinking` open a selection menu when run with
no argument (or apply inline with an arg). In every selection menu the **active
choice is marked with a leading `*`** (distinct from the `>` navigation cursor).

**Slash-command completion is two-step**, like Codex: pick an entry from the `/`
popup (or type a prefix and press Enter) and the full `/command` lands in the
composer; press Enter again to run it (or type an argument first). Typing a
command's exact name and pressing Enter runs it directly. This is uniform for
every command.

Unknown slash commands are handled locally with a warning and are not sent to
the model.

### Scrolling and the transcript pager

By default (`native` scroll-mode) the wheel scrolls the terminal's own
scrollback, so native selection/copy stay intact and the composer scrolls away
with the screen. Press **Ctrl+T** (or **PageUp**) to open a full-screen
**transcript pager** where history scrolls in the upper pane while the composer
stays pinned to the bottom; **Esc** (or Ctrl+T again) closes it.

`--scroll-mode pinned` (or `/scrollmode pinned`) opts into app-side wheel
handling: the wheel always scrolls the transcript and the composer never moves,
at the cost of native mouse selection (use Shift+drag). Settled tool-activity
groups collapse to a one-line summary; **Ctrl+O** expands them.

### Markdown rendering

Assistant replies render markdown live as they stream: headings, lists,
checkboxes, blockquotes, tables, fenced code blocks with **syntax highlighting**
(theme-matched, following `/theme`), inline bold/italic/code, `~~strikethrough~~`,
`---` rules, and `[links](url)`. Link urls render in full so the terminal can
make them cmd/ctrl+clickable in the native scroll flow.

### Languages (i18n)

The UI is fully localized in **English** and **Simplified Chinese (中文)** — menus,
the command palette, the onboarding wizard, transcript/status surfaces. Pick the
language at launch with `--lang {en,zh}` (or `OCTOS_LANG` / `LANG`), or switch at
runtime with `/lang` (a `*`-marked menu) — no restart needed. English is the
source/fallback locale, so any untranslated string falls back to English.

### Environment variables

| Variable | Purpose |
|---|---|
| `OCTOS_AUTH_TOKEN` | Fallback bearer token for the UI Protocol WebSocket. |
| `OCTOS_LANG` / `LANG` | UI language fallback when `--lang` is unset. |
| `RUST_LOG=off` | Keeps terminal output clean for live visual runs. |
| `TERM=xterm-256color` | Avoids missing terminfo/color issues on remote hosts. |
| `OCTOS_TUI_BIN` | Forces a specific built `octos-tui` binary for harnesses. |
| `OCTOS_TUI_DIR` | Points Octos harness scripts at this standalone TUI repo. |

### Workspace (cwd) behavior

`octos-tui` requests a session cwd through `session/open`. By default that is the
terminal launch directory; `--cwd DIR` overrides it. `octos serve`
canonicalizes the requested path and accepts it only if it is inside the
server-approved roots — so start the server with a `--cwd` that contains the
project you want to work in. For a remote server, pass a `--cwd` that exists on
the **server** host. An out-of-bounds cwd fails `session/open` with a typed
protocol error instead of silently running tools elsewhere.

### Provider changes after the server is running

The AppUi backend agent is created when `octos serve` starts. If you add or
change the provider/model after the server is already up (via the dashboard or a
hand-edited profile), restart `octos serve` before opening a new coding session.

### Troubleshooting

| Symptom | Fix |
|---|---|
| `octos-core` dependency not found | Keep `octos` and `octos-tui` as sibling directories. |
| Welcome screen never appears | Use a fresh empty `--data-dir` and omit `--profile-id`. |
| Endpoint rejected | Use a `ws://` or `wss://` URL; HTTP URLs are rejected. |
| Auth failure | Use the same token on `octos serve --auth-token` and the TUI (`--auth-token` or `OCTOS_AUTH_TOKEN`). |
| TUI opens but no live answer | Confirm the server has a provider/model/key and restart it after config changes. |
| Wrong workspace | Start `octos serve` with the desired `--cwd`. |
| `can't find terminfo database` | Set `TERM=xterm-256color` or install terminfo on the host. |
| Raw logs/timestamps in the UI | Start both server and TUI with `RUST_LOG=off`. |
| `target` lock or permission error | Run with `CARGO_TARGET_DIR=/tmp/octos-tui-target`. |

---

## Testing and harnesses

Run the unit/integration suite (mock-backed, no server needed):

```bash
cargo test
# CARGO_TARGET_DIR=/tmp/octos-tui-target cargo test   # on shared/locked hosts
```

Heavier live and visual harnesses live alongside the code:

- `scripts/run-onboarding-tmux-soak.sh` — reference end-to-end onboarding flow:
  starts a server, launches the TUI, and waits for the "Welcome to Octos"
  splash. See [`docs/ONBOARDING_TMUX_SOAK.md`](docs/ONBOARDING_TMUX_SOAK.md).
- The tmux AppUi smoke and live Codex-parity harnesses live in the sibling
  `octos` repo (they start both the server and the TUI); point them at this repo
  with `OCTOS_TUI_DIR="$PWD/../octos-tui"`.

For release packaging, pin `octos-core` to the matching Octos git tag or
published crate version instead of the sibling path.

---

## Protocol contract

`octos-tui` consumes Octos UI Protocol fields from `octos-core` and must not
invent local wire extensions. Any protocol change must land through a formal UI
Protocol change request with shared types, server tests, golden protocol tests,
and TUI reducer/rendering tests.

In protocol mode the TUI requests `pane.snapshots.v1` and hydrates optional pane
data from `session/open.panes` when the server supports it, falling back to
session snapshots, task tails, launch target, and status otherwise.

Auth, onboarding, and profile LLM provider setup are governed by `UPCR-2026-016`
in the `octos` repo. The TUI consumes `auth/*`, `profile/local/create`,
`profile/llm/*`, and `config/capabilities/list` as server-owned AppUI methods
over WebSocket or stdio; it never hard-codes provider/model truth or persists a
parallel LLM registry. The current v1 bridge stages selected diff context as
prompt text — structured context attachments are tracked in
[`docs/M9_31_CONTEXT_ATTACHMENTS_UPCR.md`](docs/M9_31_CONTEXT_ATTACHMENTS_UPCR.md).
