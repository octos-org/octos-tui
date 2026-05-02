# AppUI Menu Capability Gaps

This note covers the AppUI-backed slash menus from
`docs/M9_34_MENU_FRAMEWORK_MILESTONE.md`: `/model`, `/status`,
`/permissions`, and `/mcp`.

Scope rule: `octos-tui` must not invent server-owned runtime state. When a
required AppUI method or result field is absent, the menu provider should render
a capability-missing or snapshot-limited state and stop there.

## Existing AppUI Hooks

Current app-facing commands in `../octos/crates/octos-core/src/app_ui.rs`:

- `OpenSession` -> `session/open`
- `SubmitPrompt` -> `turn/start`
- `InterruptTurn` -> `turn/interrupt`
- `RespondApproval` -> `approval/respond`
- `ListApprovalScopes` -> `approval/scopes/list`
- `GetDiffPreview` -> `diff/preview/get`
- `ReadTaskOutput` -> `task/output/read`

Current capability metadata in
`../octos/crates/octos-core/src/ui_protocol.rs`:

- `UiProtocolCapabilities`
- `supported_methods`
- `supported_notifications`
- `supported_features`
- `unsupported`

The TUI transport currently requests feature tokens and maps known command
results, but it does not expose a live capability map to menu providers yet.
There is also no `AppUiCommand` variant for `config/capabilities/list`.

## Menu Gap Matrix

| Menu | Existing hook usable now | Missing AppUI contract | Provider behavior until added |
|---|---|---|---|
| `/model` | None for menu data or mutation. Internal Octos model catalogs/tools exist outside AppUI and must not be used as TUI truth. | `model/list`, `model/select`, and result fields for provider id, display name, supported reasoning efforts, defaults, current selection, and unavailable reason. | Show capability-missing state. Do not build a model list locally. |
| `/status` | `AppUiSnapshot`; `session/open` / `session/opened` expose session id, active profile id, workspace root, cursor, optional panes. TUI state also has readonly, target, run state, task counts, and `APP_UI_API_V1`. `progress/updated` can carry token/cost updates, but current TUI state only turns them into status/activity text. | `session/status/read` for refreshable authoritative status; `config/capabilities/list` for runtime capability display; explicit fields for current model/provider, usage totals, server version/build, connection state, and replay/cursor health. | Render snapshot-limited status only where data already exists; mark server-owned fields unavailable. |
| `/permissions` | `approval/respond`; `approval/scopes/list`; typed approval notifications; `permission_denied` errors. These support active approval decisions and inspection of recorded approval scopes. | `permission/profile/list`, `permission/profile/set`, `approval/scopes/clear`, and read fields for approval policy, permission mode/profile, sandbox mode, supported scopes, persisted scopes, and readonly state. | Render Codex-style permission rows for Default, Read Only, Workspace Write, Full Access, network allow/block, and persisted approval scopes. Enable scope refresh when `approval/scopes/list` is advertised; keep profile/network mutations and scope clearing disabled with explicit missing-method reasons until typed AppUI commands exist. |
| `/mcp` | None through AppUI. MCP clients, server configs, and tool registration exist in `octos-agent`, but are not exposed over `AppUiCommand`. | `mcp/status/list`; optional refresh/reload command if supported; result fields for configured servers, per-server state, tools/resources, and last error/status detail. | Show capability-missing state. Do not inspect agent internals from the TUI. |

## Concrete AppUI Follow-Ups

1. Add capability discovery to AppUI:
   - `config/capabilities/list`
   - Result should carry `UiProtocolCapabilities` or an app-facing equivalent.
   - TUI follow-up: store the capability map in menu context so registry,
     exact slash dispatch, help, and providers share the same gating.

2. Add model menu contract:
   - `model/list`
   - `model/select`
   - Include model id, display name, provider id, supported reasoning efforts,
     default reasoning effort, current model, current reasoning effort, and
     disabled/unavailable reason.
   - Mutating actions must be blocked in readonly mode.

3. Add status menu contract:
   - `session/status/read`
   - Include session id, profile id, cwd/workspace root, server state, current
     model/provider, token/cost totals, task counts, approval state, AppUI/UI
     protocol version, server version/build, and replay/cursor health.
   - Keep current snapshot rendering as a fast first frame, then refresh when
     the method is advertised.

4. Add permission menu contract:
   - `permission/profile/list`
   - `permission/profile/set`
   - `approval/scopes/clear`
   - Include approval policy, permission mode/profile, sandbox mode,
     supported approval scopes, persisted approval scopes, and readonly state.
   - `approval/scopes/list` can be reused for inspect-only persisted scopes.

5. Add MCP menu contract:
   - `mcp/status/list`
   - Optional `mcp/status/refresh` or reload command only if the server can make
     refresh/reload safe and explicit.
   - Include configured MCP servers, transport/status per server, exposed
     tools/resources, and last error/status detail.

## Current TUI State

`src/menu/` now contains the menu framework and concrete providers. For
`/permissions`, the TUI intentionally renders the desired controls before the
server mutation contract exists, but it keeps those controls disabled instead of
inventing local permission state.
