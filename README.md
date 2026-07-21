# octos-tui

<div align="center">
<pre>
             .---.
            / o o \
            \  ~  /
        .-._/`---'\_.-.
       ( ( (  ) (  ) ) )
        `-´ (_) (_) `-´

 ██████╗  ██████╗████████╗ ██████╗ ███████╗
██╔═══██╗██╔════╝╚══██╔══╝██╔═══██╗██╔════╝
██║   ██║██║        ██║   ██║   ██║███████╗
██║   ██║██║        ██║   ██║   ██║╚════██║
╚██████╔╝╚██████╗   ██║   ╚██████╔╝███████║
 ╚═════╝  ╚═════╝   ╚═╝    ╚═════╝ ╚══════╝
</pre>
<em>Welcome to Octos — Your Coding Buddy</em>
</div>

`octos-tui` is the terminal app for [Octos](https://github.com/octos-org/octos)
— an AI coding assistant in your terminal, in the spirit of Claude Code and
Codex. The Octos server runs the agent, the models, and the tools; `octos-tui`
is the fast, keyboard-driven way to talk to it: chat, diffs, tool approvals,
background tasks — all without leaving the shell.

## Start here

Two pieces: the Octos **server** (the brain) and this **TUI** (the terminal
client). One command installs both:

```bash
npm install -g @octos-org/octos @octos-org/octos-tui
# or with Homebrew (each repo is its own tap):
#   brew tap octos-org/octos     https://github.com/octos-org/octos
#   brew tap octos-org/octos-tui https://github.com/octos-org/octos-tui
#   brew install octos-org/octos/octos octos-org/octos-tui/octos-tui
```

Then start the TUI with its own private local server:

```bash
octos-tui --mode protocol \
  --stdio-command "octos serve --stdio --solo --data-dir ~/.octos-tui-data"
```

You'll land on the **"Welcome to Octos"** screen. In the next five minutes:
create your local profile (three fields — the email is local metadata only),
pick an AI provider, paste its API key, and open your first coding chat.
The [Quickstart](#quickstart-solo-onboarding) below walks every screen.

> **Heads-up:** plain `octos-tui` with no flags opens a **mock demo** with
> canned replies — nice for a look around, but not connected to anything.
> Use the command above for the real thing.

### If something looks wrong

| Symptom | Fix |
|---|---|
| `command not found: octos` | The server isn't installed — `npm install -g @octos-org/octos` (or the [server install guide](https://github.com/octos-org/octos#start-here)). |
| Replies are instant and feel canned | You're in mock mode (no flags). Start with the `--stdio-command ...` command above. |
| "Test provider" fails during onboarding | Re-check the API key and the provider choice; you can redo it anytime with `/onboard` or `/setup`. |

More in the full [Troubleshooting](#troubleshooting) table below.

---

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

# Homebrew (this repo is its own tap)
brew tap octos-org/octos-tui https://github.com/octos-org/octos-tui
brew install octos-org/octos-tui/octos-tui

# shell installer (macOS / Linux)
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/octos-org/octos-tui/releases/latest/download/octos-tui-installer.sh | sh

# PowerShell installer (Windows)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/octos-org/octos-tui/releases/latest/download/octos-tui-installer.ps1 | iex"
```

Once installed, `octos-tui update` checks for a newer release — and for
shell/PowerShell-installer installs it self-updates in place; npm/brew/cargo
installs are owned by their package manager, so it prints the matching
upgrade command instead. `octos-tui doctor` diagnoses the local environment
and connection prerequisites.

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

### 1. Install the binaries

Install the TUI and the server as shown in [Start here](#start-here) — the
npm and brew routes install **both** pieces. (The shell/PowerShell installers
in [Install](#install) ship the TUI **only**; pair them with a server install
from the [octos repo](https://github.com/octos-org/octos#start-here).)

Building from source works too — `octos-core` (the shared protocol crate) is
pulled automatically as a git dependency, so a plain clone builds with **no
sibling checkout** required (needs Rust 1.85+):

```bash
git clone https://github.com/octos-org/octos-tui.git
cd octos-tui
cargo build --release
# produces ./target/release/octos-tui — substitute it for `octos-tui` below
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
octos-tui \
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

### Agent permissions & code review

A coding session drives an agent that reads and (optionally) edits code in your
workspace. How much it may do is a **per-session** setting you change live with
`/permissions` — no restart, no launch flag:

| Mode | The agent can… | Use it for |
| --- | --- | --- |
| **Read-only** | read files, run read-only commands (`git diff`, `grep`); writes fail | code review — it can't change your repo |
| **Workspace-write** | read + write inside the workspace | hands-on edits, scoped to your project |
| **Full Access** ("yolo") | host filesystem + network, approvals never | trusted local automation — **risk of data loss** |

So for a review, run `/permissions` → **Read-only** and ask the agent to review
the diff; for hands-on changes, switch to **Workspace-write** (or **Full
Access**). The TUI only *requests* the mode — the backend applies it, and **Full
Access is offered only on solo/local backends**, never on a shared `octos serve`.

For **headless / scripted** code review and for running **many review or edit
agents in parallel**, use the `octos chat` CLI in the main
[octos](https://github.com/octos-org/octos) repo (`--sandbox`, `--yolo`,
`--profile`, `--no-session-persistence`) — see its README's *Headless agent mode
& code review* section.

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
--vim-mode               enable Vim modal editing in the composer (default off)
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
  "scroll-mode": "native",
  "vim-mode": false
}
```

`/saveconfig` writes the active `theme` / `lang` / `scroll-mode` / `vim-mode`
back into this file (merging — it never clobbers transport keys like
`stdio_command`); without `--config` it falls back to
`~/.config/octos-tui/config.json`.

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
Tab        peek a running sub-agent's output; Tab/Shift+Tab cycle main↔agents, Esc returns to chat
PgUp/PgDn  scroll the transcript (PgUp also opens the pager)
y / s / n  approve once / approve for session / deny a pending tool approval
Alt+A      re-show the pending approval prompt
[ / ]      select previous / next inline diff hunk
c          stage the selected hunk as next-turn context
Ctrl+U     clear the composer
Ctrl+C     interrupt the active turn
Esc        with no active turn: cancel the first running background task
q          quit
```

```text
/help       local slash-command help
/ps         show local task/process status and focus the Tasks pane (Esc returns to the composer)
/stop       interrupt the active turn (or report locally if none is active)
/setup      reopen the onboarding wizard
/model      browse the server-returned profile models / catalog
/permissions  set the session's sandbox + approval mode (menu): Read-only,
              Workspace-write, or Full Access (the "yolo" mode — host access,
              network, approvals never). Solo/local backends only.
/theme      switch the TUI palette at runtime (menu, or /theme claude)
/lang       switch the UI language (menu, or /lang zh) — English / 中文
/thinking   set reasoning effort for thinking models, per session (menu, or /thinking high)
/scrollmode switch wheel-scroll behavior (toggle, or /scrollmode native|pinned)
/vimmode    toggle Vim modal editing in the composer (Normal/Insert)
/saveconfig persist the active theme / language / scroll-mode / vim-mode to the config file
/onboard    set onboarding fields inline (name, username, email, key, ...)
/copy       copy the last assistant reply to the clipboard (works over SSH)
/status     snapshot-backed session, runtime, and connection status
/cost       server-reported token and cost usage
/title      configure terminal-title items
/keymap     inspect and edit TUI key bindings
/login      sign in with email OTP, or inspect current auth state
/exit       quit the TUI
```

Sessions and autonomy (shown when the server advertises the capability):

```text
/resume     switch to a prior session and reload its transcript (alias: /sessions)
/rewind     go back to an earlier checkpoint in this session to edit & resend (alias: /backtrack)
/loop       create, list, pause, resume, fire-now, or delete backend loops
/goal       view, set, pause, resume, or clear the persisted session goal
```

`/resume` lists sessions newest-first; `/rewind` shows codex-style checkpoint
rows (`#n  message preview`) and rolls the session back to the one you pick, so
you can edit and resend from there. When a session has loops, the status bar
shows a loop chip (active/paused), and the context gauge reflects the **real
per-model context window** reported by the server, not a fixed default.

`/activity` (search sessions/tasks/activity) and `/statusline` (status-bar
items) are always available. Further capability-gated commands
(`/provider`, `/permissions`, `/mcp`, `/tools`, `/skills`, `/task`,
`/threads`, `/turn`, `/agents`, `/review`) appear in the `/` popup only when
the connected server supports them — `/help` always lists what is live.

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

### Composer editing

The composer is multi-line: **Enter** sends, **Shift+Enter** (or **Ctrl+J** as a
portable fallback) inserts a newline, and the box grows as you add lines.

Arrow **Up** from an **empty** composer recalls your command history —
newest first, persisted across sessions, shell-style; once browsing, **Down**
steps back toward newer entries. With text present the arrows move the cursor
between lines (and fall back to scrolling the transcript at the first/last
line). Emacs-style keys also work (Ctrl+A/E, Alt+B/F, Ctrl+W, Ctrl+K, …).

**Vim mode** is opt-in — `--vim-mode`, config `"vim-mode": true`, or `/vimmode`
at runtime; the composer title then shows `NORMAL` / `INSERT`. It implements a
pragmatic subset:

```text
motions   h l j k   0 $   w b e   gg G
edits     x   dd   dw   cc
insert    i a A I o O      (Esc returns to Normal)
```

Enter still sends in both modes. Visual mode, registers/yank-paste, and numeric
counts (`3dd`) are out of scope.

### Scrolling and the transcript pager

By default (`native` scroll-mode) the wheel scrolls the terminal's own
scrollback, so native selection/copy stay intact and the composer scrolls away
with the screen. Press **Ctrl+T** (or **PageUp**) to open a full-screen
**transcript pager** where history scrolls in the upper pane while the composer
stays pinned to the bottom; **Esc** (or Ctrl+T again) closes it.

`--scroll-mode pinned` (or `/scrollmode pinned`) opts into app-side wheel
handling: the wheel always scrolls the transcript and the composer never moves,
at the cost of native mouse selection (use Shift+drag). Settled tool-activity
groups collapse to a one-line summary; **Ctrl+O** expands them — the same
toggle also expands the diff preview's selected hunk in full.

### Markdown rendering

Assistant replies render markdown live as they stream: headings, lists,
checkboxes, blockquotes, tables, fenced code blocks with **syntax highlighting**
(theme-matched, following `/theme`), inline bold/italic/code, `~~strikethrough~~`,
`---` rules, and `[links](url)`. Link urls render in full so the terminal can
make them cmd/ctrl+clickable in the native scroll flow.

While a thinking model reasons, the transcript shows a terse codex-style
`· thinking…` indicator instead of the verbose reasoning stream; the reply
replaces it when the answer starts. Control the effort with `/thinking`.

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

### Hooks

Hooks run a command at agent lifecycle events. They are **server-side** profile
config — edit them on the host that runs `octos serve`; the TUI just shows their
effects. The command is an **argv array** (no shell interpretation, so no pipes
or globs), the environment is sanitized, and a leading `~` in `command[0]` is
expanded. A hook can observe an event, deny it, inject context, or rewrite a
pending tool call.

#### Placement and inheritance

Hooks (and `env_vars`, `sandbox`, `plugins`, `memory`, and the skills layer
below) can be declared in two places:

- **Per-profile** — under `config.hooks` in `~/.octos/profiles/<id>.json`. Fires
  only for that profile.
- **Globally** — as a top-level `hooks` array in
  `<registry-root>/profile-defaults.json` (typically
  `~/.octos/profile-defaults.json`). Every profile inherits it.

The two **stack**: the global hooks run first (in file order), then the
profile's own — both fire. This defaults-under-profile inheritance is the same
mechanism used for `env_vars`, `sandbox`, `plugins`, `memory`, and the skill
layering described in [Custom skills](#custom-skills).

#### Config shape

```json
{
  "hooks": [
    {
      "event": "after_tool_call",
      "command": ["ruff", "check", "--quiet"],
      "timeout_ms": 8000,
      "tool_filter": ["write_file", "edit_file"],
      "path_filter": ["**/*.py"],
      "requires_bin": "ruff"
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `event` | Which lifecycle event triggers the hook (table below). Required. |
| `command` | Argv array — `command[0]` is the program, the rest are arguments. |
| `timeout_ms` | Kill the hook after this many ms (default `5000`). |
| `tool_filter` | Tool events only: fire only for these tool names. Empty = all tools. |
| `path_filter` | Tool events only: fire only when the tool's `args.path` matches one of these glob patterns. Tools with no `path` argument are skipped. |
| `requires_bin` | Skip the hook unless this binary is on `PATH` (ship optional linters without forcing every host to install them). |

#### Events

| Event | When it fires | Can deny? |
|---|---|---|
| `user_prompt_submit` | Once, when a real user prompt enters a turn, **before** the first LLM call. | **Yes** |
| `before_tool_call` | Before each tool executes. | **Yes** |
| `after_tool_call` | After each tool returns. | No |
| `before_llm_call` | Before each LLM iteration within a turn. | **Yes** |
| `after_llm_call` | After each LLM response (carries token / cost / provider stats). | No |
| `on_turn_end` | When a turn settles. | No |
| `on_resume` | When a session resumes. | No |
| `before_spawn_verify` / `on_spawn_verify` / `on_spawn_complete` / `on_spawn_failure` | Background sub-agent (spawn) lifecycle. | `before_spawn_verify` only |

`user_prompt_submit` is distinct from `before_llm_call`: it fires **once per
user turn**, while `before_llm_call` fires on **every** LLM iteration inside that
turn.

#### Protocol

The hook receives a JSON payload on **stdin** and signals its verdict via **exit
code**. The payload always carries `event`, plus `session_id`, `profile_id`, and
`model` / `cwd` where relevant:

- Tool events add `tool_name` and `arguments`, and (after) `result`, `success`,
  `duration_ms`. Arguments/results for `shell`, `read_file`, and `write_file`
  are **redacted**; other tools are truncated to 1 KB.
- `user_prompt_submit` adds the `prompt` text and the turn's `cwd`.

| Exit code | Meaning |
|---|---|
| `0` | Allow. For `user_prompt_submit`, anything printed to **stdout** is injected as extra per-turn context for the model. |
| `1` | Deny — for the before-events above only (blocks the operation; the stdout message is surfaced). On after-events, exit 1 is treated as an error. |
| `2` | For `before_tool_call` / `before_spawn_verify`: replace the pending arguments with the JSON printed on stdout. |
| other | Error (logged, does not block). |

A hook that fails (unexpected non-zero, timeout, or spawn error) **3 consecutive
times** is disabled by a circuit breaker until the server restarts.

#### Examples

Inject live git state into every turn — `user_prompt_submit`, exit 0, stdout
becomes per-turn model context:

```bash
#!/usr/bin/env bash
# ~/.octos/hooks/git-context.sh
set -euo pipefail
payload="$(cat)"
cwd="$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("cwd") or ".")')"
cd "$cwd" 2>/dev/null || exit 0
echo "git branch: $(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '(not a git repo)')"
git status --short 2>/dev/null | head -20
exit 0   # allow the turn; stdout is added to the model's context
```

```json
{ "event": "user_prompt_submit", "command": ["~/.octos/hooks/git-context.sh"], "timeout_ms": 4000 }
```

Deny a prompt that leaks a secret — `user_prompt_submit`, exit 1, the turn never
reaches the LLM:

```bash
#!/usr/bin/env bash
# ~/.octos/hooks/no-secrets.sh
set -euo pipefail
prompt="$(cat | python3 -c 'import json,sys; print(json.load(sys.stdin).get("prompt",""))')"
if printf '%s' "$prompt" | grep -Eq 'AKIA[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY-----'; then
  echo "Blocked: the prompt appears to contain a credential."
  exit 1
fi
exit 0
```

```json
{ "event": "user_prompt_submit", "command": ["~/.octos/hooks/no-secrets.sh"], "timeout_ms": 3000 }
```

Lint Rust files after they are written — `after_tool_call` scoped by
`tool_filter` + `path_filter`, gated on `cargo` being installed:

```json
{
  "event": "after_tool_call",
  "command": ["cargo", "clippy", "--quiet"],
  "timeout_ms": 20000,
  "tool_filter": ["write_file", "edit_file"],
  "path_filter": ["**/*.rs"],
  "requires_bin": "cargo"
}
```

Stacking global + per-profile: put the secret-guard in the global defaults so it
protects every profile, and add the Rust linter to just your coding profile.

```jsonc
// ~/.octos/profile-defaults.json  — top-level "hooks", inherited by all profiles
{ "hooks": [ { "event": "user_prompt_submit", "command": ["~/.octos/hooks/no-secrets.sh"] } ] }
```

```jsonc
// ~/.octos/profiles/coding.json  — "config.hooks", only this profile
{ "config": { "hooks": [
  { "event": "after_tool_call", "command": ["cargo", "clippy", "--quiet"],
    "tool_filter": ["write_file", "edit_file"], "path_filter": ["**/*.rs"],
    "requires_bin": "cargo" }
] } }
```

At runtime the secret-guard (from defaults) runs first, then the profile's
linter — both fire.

### Custom skills

Skills are the agent's plug-in tools. They are configured **server-side** (loaded
by `octos serve` from the profile and its skill directories); the TUI surfaces
them through `/skills` when the server advertises the capability. A skill is a
directory containing a `manifest.json` and an executable binary.

#### Anatomy

```text
greeter/
├── manifest.json     # declares the skill id, its tools, and load gating
└── main              # the executable (chmod +x); override the name with "binary"
```

`manifest.json` declares the skill and each tool it exposes:

```json
{
  "name": "greeter",
  "version": "1.0.0",
  "author": "you",
  "description": "Friendly greetings for any name",
  "binary": "main",
  "timeout_secs": 10,
  "tools": [
    {
      "name": "greet",
      "description": "Return a greeting for a person by name.",
      "input_schema": {
        "type": "object",
        "properties": {
          "name": { "type": "string", "description": "Who to greet" }
        },
        "required": ["name"]
      }
    }
  ],
  "requires": { "bins": [], "env": [], "os": [] }
}
```

| Field | Meaning |
|---|---|
| `name` / `id` | Skill identifier (kebab-case). Equals the directory name and the id used by the layering rules below. |
| `version` | Semver string. |
| `binary` | Executable filename relative to the skill dir (default `main`). |
| `timeout_secs` | Per-tool-call timeout. |
| `tools[]` | One entry per tool: `name` (snake_case, unique), `description`, `input_schema` (JSON Schema). Add `"concurrency_class": "exclusive"` to a tool that writes files or mutates shared state so the scheduler never races it against a sibling. |
| `requires` | Load gating: `bins` (must be on `PATH`), `env` (must be set), `os` (allowed values; empty = any). |

#### Binary protocol

The runtime invokes `./<binary> <tool_name>`, writes the tool arguments as JSON
to **stdin**, and reads one JSON object from **stdout**:

```bash
#!/usr/bin/env bash
# greeter/main — implements the `greet` tool
set -euo pipefail
tool="$1"                              # tool name = argv[1]
args="$(cat)"                          # JSON arguments on stdin
name="$(printf '%s' "$args" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("name","world"))')"

case "$tool" in
  greet) printf '{"success": true, "output": "Hello, %s!", "files_to_send": []}\n' "$name" ;;
  *)     printf '{"success": false, "output": "unknown tool: %s"}\n' "$tool" ;;
esac
```

The response object is `{ "output": string, "success": bool, "files_to_send":
[paths] }`. `output` is what the model sees; any paths in `files_to_send` are
auto-delivered to the chat.

#### Where skills are discovered

`octos serve` scans, in order: the project-local `plugins/` and `skills/`
directories, the bundled system skills, per-profile installs under
`<data-dir>/skills/`, and any colon-separated paths in `OCTOS_SKILLS_PATH`. Drop
the `greeter/` directory into your project's `skills/` (or the profile's
`<data-dir>/skills/`) and restart the server. The legacy global
`~/.octos/skills` and `~/.octos/plugins` directories are **deprecated** and no
longer scanned.

#### Built-in "super power" system skills

Every deployment ships a set of bundled system skills — the "super power" skills.
The core app skills:

| Skill (`id`) | What it does |
|---|---|
| `weather` | Current weather + multi-day forecast for any city via Open-Meteo (no API key). |
| `clock` | Current date/time in any timezone (directory `time/`). |
| `news` | Raw headlines and article text from Google News, Hacker News, Yahoo News, Substack, and Medium. |
| `deep-search` | Iterative multi-round web research: parallel crawling, reference chasing, structured report. |
| `deep-crawl` | Recursive same-origin website crawl via headless Chrome (requires `google-chrome`). |
| `send-email` | Send email via SMTP or Feishu/Lark Mail. |
| `account-manager` | Manage sub-accounts under the current profile. |

The `voice` platform skill (OminiX ASR + preset-voice TTS on Apple Silicon) is
admin-only and loaded explicitly by `octos serve`.

#### Per-profile skill layering

A profile can choose which discovered skills load, via a `skills` block. In a
per-profile file it lives under `config.skills`; in the global defaults file it
is top-level (see the [Hooks](#hooks) section for the two placements and how they
merge). Omitting the block loads every discovered skill — the default,
backward-compatible behavior.

```json
{
  "skills": {
    "mode": "all_discovered",
    "rules": [
      { "id": "deep-crawl", "enabled": false }
    ]
  }
}
```

- `mode: "all_discovered"` (the default) loads **every** discovered skill except
  those with an `enabled: false` rule. The example above ships everything but
  `deep-crawl`.
- `mode: "all_list"` loads **only** skills with an explicit `enabled: true`
  rule — **everything else is disabled, including the bundled system skills**.
  Use it to pin a profile to a fixed toolset:

```json
{
  "skills": {
    "mode": "all_list",
    "rules": [
      { "id": "weather", "enabled": true },
      { "id": "clock", "enabled": true }
    ]
  }
}
```

Rules are keyed by the manifest `id` and are last-wins per id. When the `skills`
block is inherited from `profile-defaults.json`, the two rule sets are unioned
(defaults first) and the profile's rule for a given id replaces the inherited
one — so a profile can re-enable a skill the global defaults disabled:

```jsonc
// ~/.octos/profile-defaults.json  — top-level "skills", inherited by all profiles
{ "skills": { "mode": "all_discovered", "rules": [ { "id": "deep-crawl", "enabled": false } ] } }
```

```jsonc
// ~/.octos/profiles/research.json  — re-enables deep-crawl for just this profile
{ "config": { "skills": { "rules": [ { "id": "deep-crawl", "enabled": true } ] } } }
```

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
