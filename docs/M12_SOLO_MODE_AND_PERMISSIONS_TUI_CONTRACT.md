# M12 Solo Mode And Permissions TUI Contract

Status: draft contract for agent swarm execution
Date: 2026-05-13

## Goal

Support Octos solo coding sessions in `octos-tui` without making the TUI a
runtime-policy authority.

The TUI must render server truth from AppUI. It may request a project cwd or a
permission profile, but the backend decides the effective runtime mode,
approval policy, sandbox mode, filesystem scope, network policy, and tool
policy.

Solo onboarding is local and no-OTP. The TUI collects display name, username,
and email, then calls the backend's local profile creation AppUI method. Email
is metadata only in solo mode; the TUI must not require `auth/send_code`,
`auth/verify`, SMTP setup, or dashboard email delivery for the local solo path.

## UX Principles

- Solo mode is for local single-user coding workflows.
- Multi-tenant/dashboard profile behavior stays backend-owned.
- The local solo owner maps to the current OS user conceptually: one local
  user/profile in the local Octos data dir, with project cwd chosen per
  session.
- Email is shown and persisted as metadata only until a future authenticated
  cloud sync mode is introduced.
- `-a never` means no approval prompts. It does not imply full host access.
- Full host access is shown only when the server confirms
  `sandbox_mode=danger-full-access` and `filesystem_scope=host`.
- Dangerous mode must be visually explicit and never silently selected.

## AppUI Dependencies

The TUI depends on these backend AppUI methods:

- `config/capabilities/list`
- `profile/local/create`
- `session/open`
- `session/status/read`
- `permission/profile/list`
- `permission/profile/set`
- `mcp/status/list`
- `mcp/config/upsert`
- `mcp/config/set_enabled`
- `mcp/config/delete`
- `mcp/config/test`
- `tool/status/list`
- `tool/config/set_enabled`

`profile/local/create` request fields:

- `name`
- `username`
- `email`

`profile/local/create` result fields:

- `profile_id`
- `user_id`
- `name`
- `username`
- `email`
- `created`
- `runtime_mode`

The TUI must treat this as the solo login/onboarding primitive. OTP actions are
not part of the solo happy path.

Required status fields:

- `runtime_mode`
- `profile_id`
- `workspace_root`
- `approval_policy`
- `sandbox_mode`
- `permission_profile`
- `filesystem_scope`
- `network`
- `tool_policy_id`
- `mcp_servers`
- `memory_scope`

## MCP And Tool Config Contract

MCP server config and tool enablement are backend-owned profile state. The TUI
must not edit MCP JSON, profile JSON, or tool registry files directly. It must
use AppUI config methods for mutations and status methods for refresh.

Status methods are inspect-only:

- `mcp/status/list`
- `tool/status/list`

Config methods mutate profile-scoped server state:

- `mcp/config/upsert`
- `mcp/config/set_enabled`
- `mcp/config/delete`
- `tool/config/set_enabled`

`mcp/config/test` is a probe with progress and a final result. It may test
an inline candidate config or a persisted `server_id`, but it must not persist a
candidate unless paired with `mcp/config/upsert`.

TUI requirements:

- `/mcp` refresh always renders server truth from `mcp/status/list`.
- Add/upsert, enable/disable, delete, and connection-test actions remain
  disabled with explicit missing-method reasons until AppUI advertises them.
- Disable keeps config but removes the server or tool from active use.
- Delete removes the server from the profile; the next status refresh must not
  show it.
- Profile id is required for config writes and must be sent unchanged from the
  active server-confirmed profile.
- Errors are rendered from structured `data.kind` values such as
  `mcp_server_not_found`, `mcp_invalid_config`, `mcp_connection_failed`,
  `tool_not_found`, `profile_not_found`, and `readonly_profile`.
- Captures and summaries must redact MCP env/header values.

## Workstreams

### M12-E: Solo Launch And Project Cwd UX

Repository: `octos-tui`
Issue: https://github.com/octos-org/octos-tui/issues/29

Deliverables:

- Add a solo/local launch path.
- Add no-OTP local profile onboarding:
  - collect display name, username, email
  - call `profile/local/create`
  - store/use returned `profile_id`
  - do not call `auth/send_code` or `auth/verify`
- Accept project cwd through CLI or an interactive selector.
- Send cwd in `session/open` only when the server advertises
  `session.workspace_cwd.v1`.
- Display the effective `workspace_root` returned by the server.
- Preserve reconnect behavior without replacing the server-returned cwd with a
  local guess.

Acceptance:

- Fixture proves solo onboarding creates a local profile without OTP.
- Fixture proves no `auth/send_code` or `auth/verify` request is emitted in the
  local solo flow.
- Fixture proves TUI rejects cwd workflow when the capability is unavailable.
- Fixture proves displayed cwd equals server status after reconnect.
- Live tmux run opens a solo session against a real project cwd.

### M12-F: Permission Profile UX

Repository: `octos-tui`
Issue: https://github.com/octos-org/octos-tui/issues/30

Deliverables:

- Add a `/permissions` flow or equivalent menu.
- List permission profiles from `permission/profile/list`.
- Apply permission profiles through `permission/profile/set`.
- Render server-confirmed:
  - approval policy
  - sandbox mode
  - filesystem scope
  - network policy
- Keep `-a never` separate from dangerous full access.

Acceptance:

- Fixture proves `approval_policy=never` does not display as full access unless
  the server also reports `danger-full-access`.
- Fixture proves tenant/cloud rejection is rendered as a structured policy
  error.
- Live tmux run captures the permission profile transition.

### M12-G: Interactive Tmux Soak

Repository: `octos-tui`
Issue: https://github.com/octos-org/octos-tui/issues/31

Deliverables:

- Add a live tmux runner for solo permission modes.
- Cover local solo profile creation before opening the coding session.
- Cover stdio and WebSocket transports where available.
- Capture:
  - `tui-capture.txt`
  - `server.log`
  - `appui-transcript.jsonl`
  - `runtime-policy-stamp.json`
  - `approval-events.jsonl`
  - `filesystem-probe.json`

Acceptance:

- Local profile creation completes with `profile/local/create` and no OTP
  traffic.
- Workspace-write solo run completes a simple coding prompt.
- `approval_policy=never` run emits no approval prompts.
- Dangerous run displays server-confirmed full access.
- Negative tenant/cloud run cannot enable dangerous mode.
- Multiline input remains visible in the composer.
