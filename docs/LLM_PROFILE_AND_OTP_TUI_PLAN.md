# Octos TUI Login And LLM Provider UX PRD

## Purpose

This document is the ground truth for octos-tui onboarding, login, and LLM
provider configuration. Implementation must follow this document before changing
TUI behavior or AppUI request shapes.

The TUI must not own a separate model/provider registry. It renders server
truth from AppUI and writes profile LLM configuration through the same profile
JSON schema used by the dashboard.

## Reference Review

Codex starts unauthenticated users in a dedicated onboarding/auth screen. The
auth widget initially shows only login choices, then transitions into the chosen
flow. API-key entry is a separate masked input state; it is not mixed into the
normal chat screen.

Claude Code uses first-run onboarding steps. Login is a focused auth step with a
method picker and pending/error/success states. Model choice is separate and is
available later through command UI, not blended into initial login.

Octos web currently supports email OTP login through:

- `GET /api/auth/status`
- `POST /api/auth/send-code`
- `POST /api/auth/verify`
- `GET /api/auth/me`
- `POST /api/auth/logout`

After OTP verification, the web client stores the returned token, calls
`/api/auth/me`, and persists the selected profile id from the server response.
For octos-tui, email OTP is the only supported login method for now. Admin token
or API-key login must not appear in the TUI unless a future protocol revision
explicitly advertises and documents it.

The dashboard LLM page is the model configuration source of truth. Its schema is
model family -> model name -> route/API provider. Routes include official
endpoints and alternatives such as AutoDL and WiseModel. Custom providers use a
custom family id, model id, base URL, API key env name, API type, and test
action.

## UX Contract

First launch while unauthenticated shows an empty login surface, not chat and
not model configuration. The only login choice for Octos today is:

- Sign in with Email OTP

The login flow is:

1. Probe `auth/status`.
2. Let the user enter an email.
3. Send code through `auth/send_code`.
4. Let the user enter the OTP code.
5. Verify through `auth/verify`.
6. Store the returned token in the TUI auth state.
7. Call `auth/me`.
8. Resolve the server-selected profile.
9. Enter normal TUI mode.

LLM setup is post-login. Users configure LLM providers with `/provider` and
select among configured models with `/model`. `/onboard` may remain as a guided
convenience command, but it must reuse the same login and provider state as
`/login` and `/provider`; it must not define a second provider workflow.

Shared terminal UX requirements:

- Editable onboarding/provider fields use the same composer contract as the
  coding screen: paste appends literal text, multi-line paste remains visible
  where the field accepts it, and very large pasted values must state clearly
  that the full value is retained.
- If tmux or a terminal sends paste as a rapid key burst instead of bracketed
  paste, embedded Enter keys are still treated as pasted newlines.
- Composer and field-edit surfaces resize with input length, wrapping width, and
  terminal height, while preserving enough surrounding menu context to avoid
  hiding the selected row.
- Provider/onboarding field editors inherit composer shortcuts, including
  `Ctrl+A/E/B/F`, `Ctrl+W`, `Ctrl+K`, `Ctrl+D`, `Alt+B/F/D`, and
  `Alt+Backspace`.
- Over-budget input shows the tail/current edit position rather than the first
  line only.
- Provider test progress is scoped to the selected provider/field. Rows below
  the active test must not display a false "test in progress" state.

## Dashboard LLM Schema

The TUI must preserve this dashboard schema exactly when sending or decoding LLM
profile state:

```ts
interface ModelHints {
  uses_completion_tokens?: boolean
  fixed_temperature?: boolean
  lacks_vision?: boolean
  merge_system_messages?: boolean
}

interface LlmRouteConfig {
  route_id?: string | null
  label?: string | null
  base_url?: string | null
  api_key_env?: string | null
  api_type?: string | null
}

interface LlmModelSelectionConfig {
  family_id?: string | null
  model_id?: string | null
  route?: LlmRouteConfig | null
  model_hints?: ModelHints | null
  cost_per_m?: number | null
  strong?: boolean | null
}

interface LlmProfileConfig {
  primary?: LlmModelSelectionConfig | null
  fallbacks?: LlmModelSelectionConfig[]
}
```

The catalog returned by `profile/llm/catalog` must preserve the dashboard catalog
shape, including endpoint-specific routes:

```json
{
  "moonshot": {
    "env": "MOONSHOT_API_KEY",
    "models": [
      {
        "id": "kimi-k2.5",
        "input": 0.6,
        "output": 2.4,
        "max_output": 65535,
        "endpoints": [
          { "id": "moonshot", "label": "Official API" },
          {
            "id": "autodl",
            "label": "AutoDL",
            "base_url": "https://www.autodl.art/api/v1",
            "api_key_env": "AUTODL_API_KEY"
          }
        ]
      }
    ]
  }
}
```

Required examples:

- Moonshot Kimi through AutoDL:
  `family_id=moonshot`, `model_id=kimi-k2.5`,
  `route.route_id=autodl`,
  `route.base_url=https://www.autodl.art/api/v1`,
  `route.api_key_env=AUTODL_API_KEY`.
- MiniMax through WiseModel:
  `family_id=minimax`, `model_id=MiniMax-M2.5-highspeed`,
  `route.route_id=wisemodel`,
  `route.base_url=https://open.ospreyai.cn/v1`,
  `route.api_key_env=WISEMODEL_API_KEY`.
- Custom OpenAI-compatible provider:
  custom `family_id`, custom `model_id`, custom `route.base_url`,
  custom `route.api_key_env`, and `route.api_type=openai`.

## Provider Configuration UX

`/provider` is the dashboard-equivalent TUI workflow:

1. Refresh catalog with `profile/llm/catalog`.
2. Choose model family from the catalog, or choose custom family.
3. Choose model name from the selected family, or enter a custom model name.
4. Choose route/API provider for that model: official API, AutoDL, WiseModel, or
   any endpoint advertised by the catalog.
5. For custom or endpoint-backed routes, show/edit base URL, API key env name,
   API type, and route label.
6. Enter the API key in masked form.
7. Test with `profile/llm/test`.
8. Save with `profile/llm/upsert` as primary or fallback.
9. Refresh configured providers with `profile/llm/list`.

`/model` only selects among configured server-returned providers from
`profile/llm/list` or `profile/llm/select`. If no configured providers exist,
it directs the user to `/provider`. It must not hard-code model names.

The TUI must never render, log, snapshot, or include a raw API key in UX soak
artifacts. Raw keys may only be present in the outbound AppUI request payload
for `profile/llm/test` or `profile/llm/upsert`.

## AppUI Contract

These methods must be available over WebSocket and stdio with identical JSON-RPC
method names, params, results, and errors:

- `auth/status`
- `auth/send_code`
- `auth/verify`
- `auth/me`
- `auth/logout`
- `profile/llm/catalog`
- `profile/llm/list`
- `profile/llm/upsert`
- `profile/llm/delete`
- `profile/llm/select`
- `profile/llm/test`
- `profile/llm/fetch_models`

`profile/llm/upsert` input:

```json
{
  "profile_id": "coding",
  "selection": {
    "family_id": "moonshot",
    "model_id": "kimi-k2.5",
    "route": {
      "route_id": "autodl",
      "label": "AutoDL",
      "base_url": "https://www.autodl.art/api/v1",
      "api_key_env": "AUTODL_API_KEY",
      "api_type": "openai"
    }
  },
  "api_key": "secret-value",
  "set_primary": true
}
```

Sanitized provider state returned to TUI:

```json
{
  "profile_id": "coding",
  "primary": {
    "family_id": "moonshot",
    "model_id": "kimi-k2.5",
    "route": {
      "route_id": "autodl",
      "label": "AutoDL",
      "base_url": "https://www.autodl.art/api/v1",
      "api_key_env": "AUTODL_API_KEY"
    },
    "has_api_key": true
  },
  "fallbacks": [],
  "llm": {
    "primary": {
      "family_id": "moonshot",
      "model_id": "kimi-k2.5",
      "route": {
        "route_id": "autodl",
        "label": "AutoDL",
        "base_url": "https://www.autodl.art/api/v1",
        "api_key_env": "AUTODL_API_KEY"
      },
      "has_api_key": true
    },
    "fallbacks": []
  },
  "runtime_policy_stamp": {
    "profile_id": "coding",
    "provider": "moonshot",
    "model": "kimi-k2.5"
  }
}
```

## Implementation Rules

- `/login` owns email OTP interaction.
- `/provider` owns LLM provider configuration.
- `/onboard` may orchestrate the same actions, but it cannot keep separate
  provider shortcuts or a separate schema.
- No TUI hard-coded provider shortcut such as "Add Moonshot AutoDL" is allowed.
  Those options must come from `profile/llm/catalog`.
- No flattened provider/model request schema is allowed for provider mutation.
  Mutations send dashboard-shaped `selection`.
- TUI config files may store endpoint/profile/token preferences only. They must
  not store LLM provider definitions.
- Capability checks must disable missing server-owned methods with the exact
  missing method name.

## Testing And Soak Requirements

Unit tests must prove:

- `/login email`, `/login send-code`, `/login code`, and `/login verify` build
  the expected AppUI auth commands.
- `/provider select moonshot kimi-k2.5 autodl ...` builds a
  dashboard-shaped `selection`.
- Dashboard `LlmModelSelectionConfig` fields decode when optional or null.
- Provider menu choices are built from `profile/llm/catalog`, not hard-coded
  shortcut helpers.
- Provider test/upsert commands mask secrets in debug output and TUI state.
- `/model` renders only configured server-returned providers.

Interactive tmux soak must capture:

- login screen before auth,
- email entry,
- OTP send result,
- OTP verify result,
- account/profile resolution from `auth/me`,
- provider catalog refresh,
- family/model/route selection,
- masked API key entry,
- provider test result,
- provider save result,
- configured provider list,
- model selection from configured providers,
- a raw-secret leak scan over transcript, logs, and captures.
