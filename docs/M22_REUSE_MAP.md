# M22 TUI Solo Onboarding — Reuse Map

Status: M22-A baseline (issue #51)
Date: 2026-05-21
Contract:
[`octos/docs/M22_TUI_SOLO_ONBOARDING_CONTRACT_2026-05-18.md`](../../octos/docs/M22_TUI_SOLO_ONBOARDING_CONTRACT_2026-05-18.md)

The M22 onboarding cluster (issues #51–#58 in `octos-tui`) MUST extend the
existing first-launch surface instead of introducing a parallel wizard. This
document records the canonical reuse points so every M22 slice can ground its
implementation in what is already there.

## First-launch entry

| Concern | Existing location | Reuse rule |
|---|---|---|
| Auto-open on first launch | `Store::maybe_open_onboarding_on_first_launch` in `src/store.rs` | Triggers only when capabilities advertise a profile-creation method: `profile/local/create` for local solo, or ALL of `auth/send_code` + `auth/verify` + `auth/me` for legacy email-OTP onboarding. Provider/catalog-only capabilities MUST NOT trigger. |
| First-launch layout | `render_onboarding_first_launch_layout` + `onboarding_first_launch_active` in `src/app.rs` | Renders the onboarding menu surface above the composer when there are no sessions and the active menu id is one of `onboard`, `onboard-family`, `onboard-model`, `onboard-route`. Do not invent a separate "splash" renderer. |
| First-launch trigger capability set | `APPUI_FIRST_LAUNCH_LOCAL_SOLO_METHODS` and `APPUI_FIRST_LAUNCH_LEGACY_AUTH_METHODS` in `src/menu/registry.rs` | Extend these constants — do not add ad-hoc capability checks in `Store`. |
| `/onboard` and `/setup` slash entry | `CommandSpec { name: "onboard", aliases: &["setup"], … }` in `src/menu/registry.rs::core_command_specs` | Both names dispatch the same `OnboardingAction::Open` → `MenuId::from(MENU_ONBOARD)`. Tests in `src/store.rs::tests::setup_alias_opens_same_onboarding_surface_as_onboard` pin this. |

## Wizard state

| Concern | Existing location | Reuse rule |
|---|---|---|
| Wizard state | `OnboardingWizardState` in `src/model.rs` | Add fields here; do not introduce a parallel state machine in `Store` or any new module. |
| Onboarding intent dispatch | `Store::dispatch_onboarding_action` + `dispatch_onboarding_inline` in `src/store.rs` | All `/onboard <subcommand>` paths funnel through these two functions and `OnboardingAction`. |
| Profile creation RPC | `Store::onboarding_create_local_profile_command` in `src/store.rs` | Only `profile/local/create` may run in the local solo path. OTP send/verify are explicitly hidden when local profile create is advertised. |
| Provider catalog/save/test | `Store::onboarding_refresh_catalog_command`, `onboarding_save_provider_command`, `onboarding_test_provider_command`, etc. in `src/store.rs` | Reuse these for any provider polish; do not introduce a TUI-side provider store. |
| Session open after onboarding | `Store::onboarding_finish_command` in `src/store.rs` | Resolves profile id, validates a saved primary provider, extracts cwd via `onboarding_workspace_cwd`, and emits `AppUiCommand::OpenSession`. The session-open boundary stays here. |

## Menu surface

| Concern | Existing location | Reuse rule |
|---|---|---|
| Onboarding menus | `onboarding_menu`, `onboarding_local_profile_menu`, `onboarding_provider_setup_menu`, `onboarding_family_menu`, `onboarding_model_menu`, `onboarding_route_menu` in `src/menu/providers.rs` | Add rows to these menus. Disable rows with typed reasons; do not silently hide. |
| Permissions menu | `permissions_menu` in `src/menu/providers.rs` | Labels and modes are the canonical names; the onboarding permission step (issue #53) must reuse them. |
| Capability gating | `CommandAvailability` + `APPUI_*_METHODS_ANY` constants in `src/menu/registry.rs` | Every new step must declare its method set as a constant and gate with `CommandAvailability::with_required_methods_any` or `with_required_methods_when_capabilities`. |

## Tests

| Concern | Existing location | Reuse rule |
|---|---|---|
| First-launch open coverage | `Store::tests::first_launch_opens_onboarding_menu_when_server_advertises_solo_profile_create` and the M22-A additions in `src/store.rs::tests` | Extend; do not duplicate. |
| First-launch render coverage | `render_first_launch_onboarding_is_not_mixed_with_empty_chat`, `render_first_launch_onboarding_child_menu_stays_on_onboarding_surface`, and `render_onboarding_without_capabilities_shows_disabled_reason_not_blank` in `src/app.rs::tests` | Extend with new step-specific render assertions; reuse `rendered_text` / `rendered_buffer` helpers. |
| Onboarding tmux soak | `scripts/run-onboarding-tmux-soak.sh` and `docs/ONBOARDING_TMUX_SOAK.md` | The M22 live UX gate (issue #55, deferred to human-driven) extends this runner. Soak artifacts and validators land alongside existing ones. |

## Runtime truth boundary

The TUI never writes profile/session/permission files directly. Every onboarding
mutation MUST issue an AppUI RPC (`profile/local/create`, `profile/llm/upsert`,
`permission/profile/set`, `session/open`, …) and read back the server-effective
state from the next notification or status read. The `OnboardingWizardState`
fields are *staged intent* only; the server stamps decide truth via
`session/status/read` and `runtime-policy-stamp.json`.
