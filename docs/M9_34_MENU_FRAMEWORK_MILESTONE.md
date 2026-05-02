# M9.34 Menu and Submenu Framework Milestone

Status: proposed

## Purpose

Octos TUI currently has local one-off slash handlers for `/ps`, `/stop`, and
`/help`. That is not enough for Codex-style coding UX. Codex has a reusable
command registry, slash autocomplete popup, selection-list framework,
multi-select framework, nested submenu flow, and specialized menus for model
selection, status, theme, status line, title, keymap, permissions, MCP, and
background terminals.

This milestone defines the Octos-native framework needed to reach that UX
without coupling `octos-tui` to Codex internals. The framework must let future
menus be added by registering menu content and actions, not by rewriting the
composer, renderer, event loop, or AppUI transport.

Codex reference source inspected locally:

- `/Users/yuechen/home/codex/codex-rs/tui/src/slash_command.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/bottom_pane/command_popup.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/bottom_pane/list_selection_view.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/bottom_pane/multi_select_picker.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/theme_picker.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/bottom_pane/status_line_setup.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/bottom_pane/title_setup.rs`
- `/Users/yuechen/home/codex/codex-rs/tui/src/keymap_setup/picker.rs`

Codex is Apache-2.0. Octos is Apache-2.0. We may borrow architecture and
implementation ideas, but the Octos implementation should be native to the
AppUI architecture. If code is copied directly, preserve required attribution
and update notices.

## Product Goals

- Typing `/` opens a command popup instead of requiring exact command recall.
- Commands are discoverable, searchable, gated, and described in the UI.
- Commands that open menus use a common bottom-pane menu framework.
- Menus support single select, multi-select, nested submenus, search, disabled
  rows, current/default indicators, shortcuts, previews, and cancel/restore.
- Local TUI settings can be implemented without AppUI changes.
- Server-backed runtime menus must use AppUI APIs, never stale local guesses.
- Future menu content can be added by registering a provider and actions.
- The framework must be testable through unit tests, render snapshots, keyboard
  simulation, mock AppUI fixtures, and live tmux parity tests.

## Non-Goals

- Do not copy Codex UI branding or product names.
- Do not make `octos-tui` own model/provider/runtime state that belongs to
  `octos serve`.
- Do not introduce AppUI wire changes silently. Any server-backed menu that
  needs new runtime data must add an explicit AppUI contract change.
- Do not block the existing `/ps`, `/stop`, and approval UX while the framework
  is in progress.
- Do not make menu correctness depend on LLM prompt behavior.

## User-Facing Feature Set

### Slash Command Popup

Required behavior:

- Typing `/` opens a command popup above the composer.
- Typing filters commands by exact, prefix, and fuzzy match.
- Each row shows command name and short description.
- Hidden commands are omitted when unavailable because of feature flags,
  readonly mode, protocol mode, missing AppUI capability, or task state.
- Exact typed commands that are disallowed should produce a clear local error.
- Local commands must not be submitted as prompts.
- Unknown commands must not be submitted as prompts unless explicitly marked as
  prompt-compatible.
- Queued user input during an active turn must preserve order and must not be
  rendered ahead of already-started assistant output.

Initial command set:

- `/ps`: show background task/process status.
- `/stop`: interrupt the active turn or stop background tasks where supported.
- `/help`: show command help.
- `/model`: choose model and reasoning options through AppUI-backed data.
- `/status`: show session, runtime, model, usage, cwd, AppUI version, and
  connection status.
- `/theme`: choose local TUI theme.
- `/statusline`: configure bottom status line items.
- `/title`: configure terminal title items.
- `/keymap`: inspect and edit TUI key bindings.
- `/permissions`: configure approval/permission profile when AppUI supports it.
- `/mcp`: list MCP servers/tools when AppUI supports it.

### Generic Menu Surface

Required behavior:

- Renders in the bottom pane near the composer.
- Keeps stable dimensions when navigating.
- Supports narrow and wide terminal layouts.
- Provides consistent footer hints.
- Supports `Esc`/`Ctrl+C` cancel.
- Supports `Enter` accept.
- Supports arrow navigation and optional number shortcuts.
- Supports disabled rows with reasons.
- Supports current/default markers.
- Supports inline toggles for checkbox-like rows.
- Supports optional side preview on wide terminals and stacked preview on narrow
  terminals.
- Supports nested menus with parent restoration on child cancel.
- Supports explicit close on successful action.

### Multi-Select Menus

Required behavior:

- Supports checkbox toggling.
- Supports optional reordering.
- Supports search.
- Supports live preview.
- Supports confirm and cancel callbacks.
- Suitable for `/statusline`, `/title`, and future content-selection menus.

### Server-Backed Menus

Required behavior:

- AppUI server remains authoritative.
- Menu providers can request data from the server before rendering.
- Loading, error, and capability-missing states are visible.
- Server-backed actions send typed `AppUiCommand` values.
- Readonly mode blocks mutating actions while allowing inspect-only menus.
- Disconnected mode can show cached/local state only if clearly marked.

## Framework Architecture

Add a new menu subsystem under `src/menu/`.

Suggested files:

```text
src/menu/mod.rs
src/menu/types.rs
src/menu/registry.rs
src/menu/availability.rs
src/menu/command_popup.rs
src/menu/selection_view.rs
src/menu/multi_select_view.rs
src/menu/render.rs
src/menu/providers/mod.rs
src/menu/providers/help.rs
src/menu/providers/status.rs
src/menu/providers/theme.rs
src/menu/providers/statusline.rs
src/menu/providers/title.rs
src/menu/providers/keymap.rs
src/menu/providers/model.rs
src/menu/providers/permissions.rs
src/menu/providers/mcp.rs
```

Keep the content providers small. The renderer must not know about model,
theme, statusline, or AppUI-specific concepts.

### Core Types

The exact Rust names can change during implementation, but the boundaries
should remain stable.

```rust
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub category: CommandCategory,
    pub availability: CommandAvailability,
    pub inline_args: InlineArgMode,
    pub entry: CommandEntry,
}

pub enum CommandEntry {
    OpenMenu(MenuId),
    LocalAction(LocalAction),
    AppUiAction(AppUiActionKind),
    PromptTemplate(&'static str),
}

pub struct MenuSpec {
    pub id: MenuId,
    pub title: String,
    pub subtitle: Option<String>,
    pub items: Vec<MenuItem>,
    pub tabs: Vec<MenuTab>,
    pub searchable: bool,
    pub search_placeholder: Option<String>,
    pub footer_hint: Option<String>,
    pub preview: Option<MenuPreview>,
    pub mode: MenuMode,
}

pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub shortcut: Option<KeyBinding>,
    pub state: MenuItemState,
    pub disabled_reason: Option<String>,
    pub action: MenuAction,
}

pub enum MenuAction {
    OpenMenu(MenuId),
    ReplaceMenu(MenuId),
    Close,
    Local(LocalAction),
    SendAppUi(AppUiCommand),
    SubmitPrompt(String),
    Noop,
}
```

### Provider Interface

Menu content must be provider-driven so new menus can be added later without
touching generic rendering code.

```rust
pub trait MenuProvider {
    fn id(&self) -> MenuId;
    fn build(&self, ctx: &MenuContext) -> MenuBuildResult;
    fn on_cancel(&self, ctx: &mut MenuContext) -> Vec<ClientEffect>;
}
```

`MenuContext` should expose only stable view/runtime facts:

- current app state
- palette/theme
- terminal size
- readonly mode
- protocol capabilities
- connection state
- selected menu path
- AppUI command builder

Providers must not mutate `AppState` directly. They should return actions or
client effects for the store/event loop to apply.

### Menu Stack

The TUI needs a menu stack, not a single modal flag.

Required stack behavior:

- `open(menu_id)` pushes a menu.
- `replace(menu_id)` swaps the active menu.
- `close()` pops the active menu.
- `close_all()` returns to composer.
- Child cancel returns to the parent menu when the child was opened by a parent.
- Successful child accept can optionally close the parent as well.
- Active menus own keyboard focus.
- Active menus never submit composer text unless the selected action explicitly
  creates a prompt.

### Availability and Capability Gating

Availability must be centralized so the slash popup, exact dispatch, and help
text stay consistent.

Inputs:

- task running or idle
- approval modal visible
- readonly mode
- mock vs protocol mode
- AppUI capability map
- server connected/disconnected
- feature flags
- current session opened or not

Rules:

- Hide impossible commands from the popup.
- Exact typed commands should explain why unavailable.
- Mutating commands are blocked in readonly mode.
- AppUI-backed commands are disabled if the server does not advertise the
  needed capability.
- Local-only commands remain available offline when safe.

## Local Menu Providers

### Theme Menu

Local-only.

Required:

- List built-in themes: codex, claude, terminal, and any future theme names.
- Mark current theme.
- Preview theme while navigating if feasible.
- Restore previous theme on cancel.
- Persist selected theme using the existing TUI config path when available.
- Never require AppUI.

### Status Line Menu

Local-only initially.

Required:

- Multi-select visible status line items.
- Reorder selected items.
- Preview final status line.
- Include at minimum: state, model, usage, cwd, profile, AppUI version, session,
  git branch when known, background task count, approval state.
- Persist local layout preference.

### Terminal Title Menu

Local-only initially.

Required:

- Multi-select and reorder title items.
- Preview final terminal title.
- Include at minimum: app name, project/cwd, state, session, model, branch,
  background task count, approval state.
- Persist local layout preference.

### Keymap Menu

Local-only.

Required:

- Browse current key bindings by category.
- Show global, composer, menu/list, task, approval, diff, and modal actions.
- Search by action name or key.
- Show conflicts before saving.
- Let users reset to defaults.
- Persist overrides only after confirmation.

## Server-Backed Menu Providers

### Model Menu

Requires AppUI support.

Required server data:

- available models
- display name
- model id
- provider id
- supported reasoning efforts
- default reasoning effort
- current model
- current reasoning effort
- disabled/unavailable reason

Required actions:

- select model
- select reasoning effort
- persist selection for session/profile if server supports it
- show capability-missing state otherwise

No local fallback may invent a model list.

### Status Menu

Requires AppUI support for server-owned fields.

Required data:

- session id
- profile id
- cwd
- server state
- current model/provider
- usage/token state when available
- task counts
- approval state
- AppUI protocol version
- server version/build
- replay cursor/lossy state if relevant

This menu can render from current snapshot first and request a refresh when the
server exposes one.

### Permissions Menu

Requires AppUI support.

Required data:

- approval policy
- permission profile
- sandbox mode
- persisted approval scopes
- supported approval scopes
- readonly state

Required actions:

- inspect current permission policy
- choose allowed profile if server supports profile switching
- clear/revoke persisted approval scopes if server supports it
- explain capability-missing cases

### MCP Menu

Requires AppUI support.

Required data:

- configured MCP servers
- status per server
- available tools/resources where exposed
- last error/status detail

Required actions:

- inspect server/tool status
- refresh/reload only if AppUI supports it

## AppUI Contract Work

Do not block the local menu framework on all AppUI changes. Split work:

- Framework and local menus can ship without server API changes.
- Server-backed menus must define AppUI capability requirements before
  implementation.

Candidate AppUI additions, subject to UPCR/review:

```text
config/capabilities/list
session/status/read
model/list
model/select
permission/profile/list
permission/profile/set
approval/scopes/clear
mcp/status/list
```

If equivalent APIs already exist in `octos-core`, use them rather than adding
duplicates.

## Swarm Work Packages

### Agent A: Framework Core

Ownership:

- `src/menu/types.rs`
- `src/menu/registry.rs`
- `src/menu/availability.rs`
- `src/model.rs` menu state additions
- focused unit tests

Deliverables:

- `CommandSpec`, `MenuSpec`, `MenuItem`, `MenuAction`, and `MenuProvider`
  foundation.
- Central command availability and capability gating.
- Menu stack state model.
- No rendering dependency on concrete menu contents.

### Agent B: Composer and Slash Popup

Ownership:

- `src/event_loop.rs`
- composer-related state in `src/store.rs`
- `src/menu/command_popup.rs`
- keyboard tests

Deliverables:

- `/` opens command popup.
- Filtering works.
- Exact command dispatch uses the same registry.
- Unknown and unavailable commands are handled locally.
- Existing `/ps`, `/stop`, `/help` behavior remains intact.

### Agent C: Generic Renderers

Ownership:

- `src/menu/selection_view.rs`
- `src/menu/multi_select_view.rs`
- `src/menu/render.rs`
- integration in `src/app.rs`
- render snapshot tests

Deliverables:

- Single-select menu.
- Multi-select/reorder menu.
- Nested menu stack rendering.
- Wide/narrow preview behavior.
- Stable layout around composer and sticky work panes.

### Agent D: Local Menu Providers

Ownership:

- `src/menu/providers/theme.rs`
- `src/menu/providers/statusline.rs`
- `src/menu/providers/title.rs`
- `src/menu/providers/keymap.rs`
- `src/theme.rs`
- config persistence helpers

Deliverables:

- `/theme`
- `/statusline`
- `/title`
- `/keymap`
- local persistence and cancel-restore behavior

### Agent E: AppUI Server-Backed Menus

Ownership:

- `src/menu/providers/model.rs`
- `src/menu/providers/status.rs`
- `src/menu/providers/permissions.rs`
- `src/menu/providers/mcp.rs`
- `src/transport.rs`
- matching AppUI docs/tests in parent `octos` when needed

Deliverables:

- Capability-aware server-backed menu providers.
- AppUI command/result wiring where APIs already exist.
- Explicit TODO/UPCR notes where APIs are missing.
- No hardcoded model/provider policy in the TUI.

### Agent F: Harness and Product Validation

Ownership:

- docs and harness contracts
- mock fixtures
- tmux live parity scripts in parent `octos`

Deliverables:

- Mock menu fixtures for slash popup, theme, statusline, title, keymap,
  capability-missing model, and server-backed status.
- Live tmux assertions that menus open, filter, close, and do not submit prompts.
- Visual comparisons against Codex where useful.

### Review Agent: Critical UX/API Review

Ownership:

- no production files unless explicitly assigned

Deliverables:

- Review the implementation against this document.
- Verify API boundary: local-only vs AppUI-backed menus.
- Verify no menu action bypasses readonly mode, approval state, or capability
  gating.
- Verify code is not a brittle copy of Codex internals.

## Milestone Slices

### M9.34.1: Registry and Menu State

Exit criteria:

- Commands can be registered with names, aliases, descriptions, and availability.
- Store can open/close a menu stack.
- Tests cover capability gating and readonly gating.

### M9.34.2: Slash Popup

Exit criteria:

- Typing `/` opens a filtered popup.
- `/ps`, `/stop`, `/help` route through the registry.
- Unknown commands show local help and never go to the LLM.
- Tests cover active-turn queue behavior.

### M9.34.3: Selection View

Exit criteria:

- Generic menu renders above the composer.
- Keyboard navigation, accept, cancel, disabled rows, and current/default markers
  work.
- Snapshot tests cover narrow and wide terminals.

### M9.34.4: Multi-Select View

Exit criteria:

- Checkbox toggle, reorder, confirm, cancel, and preview work.
- Used by at least one local menu.

### M9.34.5: Local Menus

Exit criteria:

- `/theme`, `/statusline`, `/title`, and `/keymap` work in mock mode.
- Theme cancel restores the previous theme.
- Status line/title settings persist when configured.
- Keymap detects conflicts before saving.

### M9.34.6: Server-Backed Menus

Exit criteria:

- `/status` works from current AppUI snapshot.
- `/model`, `/permissions`, and `/mcp` show capability-aware loading or
  capability-missing states.
- Any implemented server-backed mutations use typed AppUI commands.
- Missing server APIs are documented as AppUI follow-ups, not worked around.

### M9.34.7: Harness and Handoff

Exit criteria:

- Mock harness covers every new menu state.
- Live tmux harness covers slash popup and at least one local menu.
- Codex comparison captures are retained for reference.
- Documentation explains how to add a new menu.

## How To Add A New Menu Later

1. Add a `CommandSpec` entry to the registry.
2. Decide whether the menu is local-only or AppUI-backed.
3. Add a provider under `src/menu/providers/`.
4. Build and return a `MenuSpec`.
5. Use only generic `MenuItem` and `MenuAction` values.
6. Add availability rules.
7. Add unit tests for the provider.
8. Add render snapshots for narrow and wide terminals.
9. Add keyboard-flow tests.
10. Add AppUI contract tests if the menu needs server data.

The generic renderer should not be changed unless the new menu needs a reusable
capability that other menus can also use.

## Test Requirements

Unit tests:

- command registry exact lookup
- alias lookup
- fuzzy/prefix filtering
- availability gating
- readonly gating
- provider build output
- menu stack push/pop/replace behavior

Keyboard tests:

- `/` opens popup
- typing filters popup
- `Enter` opens selected command
- `Esc` closes popup/menu
- queued input order is preserved during an active turn
- local slash commands are not submitted as prompts

Render tests:

- 80x24 narrow popup
- 100x32 standard popup
- 140x40 wide popup with side preview
- long labels truncate or wrap correctly
- disabled row reason displays without overlap
- multi-select preview does not obscure composer

Mock backend tests:

- local menu works offline
- server-backed menu shows capability-missing state
- readonly mode blocks mutating actions
- disconnected mode does not invent server truth

Live tmux tests:

- command popup appears after typing `/`
- `/theme` opens and closes without submitting a prompt
- `/status` displays runtime information
- `/ps` still reports tasks
- `/stop` still interrupts active work
- no menu frame is hidden by the composer
- chat history remains ordered around menu interactions

## Definition Of Done

- Framework and menu content are separated.
- New menus can be added by provider registration.
- Existing TUI UX does not regress.
- Local-only menus do not need AppUI.
- Server-backed menus do not guess server state.
- Tests cover render, keyboard, mock, and at least one live tmux path.
- Docs explain architecture, work packages, acceptance criteria, and extension
  steps.

## Known Risks

- Copying Codex code too literally could couple Octos to assumptions that do
  not fit AppUI. Mitigation: port concepts, not internals, unless a small helper
  is worth attribution.
- Server-backed menus may expose missing AppUI capabilities. Mitigation: render
  capability-missing states and document UPCR work.
- Menu stack can interfere with approval modal or queued input. Mitigation:
  centralize focus ownership and add keyboard tests for active-turn and approval
  states.
- Statusline/title/keymap persistence can become platform-specific. Mitigation:
  keep persistence local and optional for the first slice.
- Large menus can break terminal layout. Mitigation: snapshot narrow, standard,
  and wide sizes before live testing.
