# AppUI Menu Capability Gaps

This note covers the AppUI-backed slash menus from
`docs/M9_34_MENU_FRAMEWORK_MILESTONE.md`: `/model`, `/status`,
`/permissions`, and `/mcp`.

Scope rule: `octos-tui` must not invent server-owned runtime state. When a
required AppUI method or result field is absent, the menu provider should render
a capability-missing or snapshot-limited state and stop there.

## Existing AppUI Hooks

Current TUI app-facing commands cover the original session/turn/approval/task
surface plus the menu-backed runtime surface:

- `config/capabilities/list`
- `session/status/read`
- `profile/llm/list`, `profile/llm/select`
- `permission/profile/list`, `permission/profile/set`
- `mcp/status/list`
- `mcp/config/list`, `mcp/config/upsert`, `mcp/config/set_enabled`,
  `mcp/config/delete`, `mcp/config/test`
- `tool/status/list`
- `tool/config/list`, `tool/config/upsert`, `tool/config/set_enabled`,
  `tool/config/delete`, `tool/config/test`
- `profile/skills/list`, `profile/skills/install`,
  `profile/skills/remove`, `profile/skills/registry/search`

`config/capabilities/list` hydrates `UiProtocolCapabilities` into TUI state.
The registry, exact slash dispatch, help menu, and providers share that
capability map for gating. `octos-tui doctor --endpoint ...` also probes the
same method live before falling back to the structural local skew check.

## Menu Gap Matrix

| Menu | Existing hook usable now | Missing AppUI contract | Provider behavior until added |
|---|---|---|---|
| `/model` | `profile/llm/list`, `profile/llm/select`, provider catalog and provider mutation methods. | Richer server fields for supported reasoning efforts, unavailable reasons, and current/default reasoning effort per model. | Render server-provided models only; keep selection disabled unless `profile/llm/select` is advertised and readonly permits mutation. |
| `/status` | `AppUiSnapshot`, `config/capabilities/list`, `session/status/read`, progress metadata for token/cost/retry state. | Optional server version/build, richer replay/cursor health, and any runtime fields not yet present in `SessionStatusReadResult`. | Render cached snapshot immediately, refresh authoritative status when advertised, and mark absent server-owned fields unavailable. |
| `/permissions` | `approval/respond`, `approval/scopes/list`, typed approval notifications, `permission/profile/list`, `permission/profile/set`, runtime policy stamps. | `approval/scopes/clear` remains separate from profile selection; richer persisted-scope details may still require server fields. | Render Codex-style permission rows and gate each mutation on advertised methods plus readonly state. |
| `/mcp` | `mcp/status/list`, `mcp/config/list`, `mcp/config/upsert`, `mcp/config/set_enabled`, `mcp/config/delete`, `mcp/config/test`, `tool/status/list`, `tool/config/list`, `tool/config/set_enabled`, `tool/config/upsert`, `tool/config/delete`, `tool/config/test`. | Optional `mcp/status/refresh` or reload command if the server can make refresh/reload safe and explicit. | Render status/config truth from AppUI only. Do not inspect agent internals or edit profile JSON directly. |

## Concrete AppUI Follow-Ups

1. Extend model/status result fields where needed:
   - model unavailable reasons
   - supported/default/current reasoning effort
   - server version/build
   - explicit replay/cursor health

2. Add persisted approval-scope management if the server supports it:
   - `approval/scopes/clear`
   - richer persisted-scope read fields beyond `approval/scopes/list`

3. Consider explicit MCP refresh/reload only if the server can make it safe:
   - optional `mcp/status/refresh`
   - optional reload command with clear side-effect semantics
   - keep config profile-scoped and server-owned

## Current TUI State

`src/menu/` now contains the menu framework and concrete providers. Providers
use only AppUI snapshots, cached AppUI command results, and advertised
capabilities. They must continue to render explicit unavailable states rather
than inventing local truth for server-owned runtime data.
