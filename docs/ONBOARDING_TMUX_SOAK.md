# Onboarding Tmux Soak

This is the live interactive acceptance test for OTP login and dashboard-parity
LLM provider onboarding in octos-tui.

For M12 solo mode, the same runner also has a no-OTP local onboarding lane that
drives AppUI directly through the live tmux environment and captures the runtime
permission evidence required by M12-D/G.

## Runner

```sh
scripts/run-onboarding-tmux-soak.sh start
```

The runner starts two tmux sessions:

- `octos-onboard-server-<run-id>`
- `octos-onboard-tui-<run-id>`

It writes artifacts under:

```text
e2e/test-results-tui-onboarding/<run-id>/
```

The command surface is:

```sh
scripts/run-onboarding-tmux-soak.sh start
scripts/run-onboarding-tmux-soak.sh drive-onboard
scripts/run-onboarding-tmux-soak.sh drive-solo
scripts/run-onboarding-tmux-soak.sh capture
scripts/run-onboarding-tmux-soak.sh send-turn
scripts/run-onboarding-tmux-soak.sh verify
scripts/run-onboarding-tmux-soak.sh verify-solo
scripts/run-onboarding-tmux-soak.sh verify-first-launch
scripts/run-onboarding-tmux-soak.sh verify-provider-missing
scripts/run-onboarding-tmux-soak.sh api-parity
scripts/run-onboarding-tmux-soak.sh self-test
scripts/run-onboarding-tmux-soak.sh solo-self-test
scripts/run-onboarding-tmux-soak.sh stop
```

`self-test` is local and synthetic. It does not start the backend; it creates a
temporary profile JSON, runs `verify`, checks required artifact creation and
redaction, then proves verification fails if `OCTOS_TUI_SOAK_API_KEY` appears
in any artifact.

`solo-self-test` delegates to the backend M12 fixture probe. It validates the
solo artifact schema and proves the retained AppUI transcript contains no OTP
method traffic.

## M12 Solo No-OTP Flow

The solo lane calls `profile/local/create` with display name, username, and
email metadata, then opens a cwd-bound session, probes permission profiles, and
drives the MCP/tool config fixture through AppUI:

- `workspace-write`
- `approval_policy=never` with sandbox still active
- `danger-full-access` plus approval-never
- tenant/cloud dangerous rejection, when the backend exposes the policy method
- `mcp/status/list` and `tool/status/list` for configured state
- `mcp/config/upsert` for stdio and websocket fixture servers
- `mcp/config/set_enabled` for disable/enable semantics
- `tool/config/set_enabled` for per-tool disable/enable semantics
- `mcp/config/test` progress and result for stdio and websocket fixtures
- `mcp/config/delete` followed by status refresh showing server truth

Current backends may not have M12-A/C fully wired. In that case the runner keeps
the evidence files and records blockers in `soak-summary.json`; set
`OCTOS_TUI_SOAK_SOLO_STRICT=1` when the backend is ready to require a pass.
MCP/tool config blockers are recorded the same way until the backend advertises
`mcp/config/*`, `mcp/config/test`, and `tool/config/set_enabled`.
Live transports also require `OCTOS_BIN` to point at an API-enabled `octos`
binary that exposes `serve`.

Provider-free stdio dry-run:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=solo-stdio-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh drive-solo

OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-solo
```

WebSocket tmux run:

```sh
OCTOS_TUI_SOAK_TRANSPORT=ws scripts/run-onboarding-tmux-soak.sh start
OCTOS_TUI_SOAK_TRANSPORT=ws OCTOS_TUI_SOAK_RUN_ID=<run-id> scripts/run-onboarding-tmux-soak.sh drive-solo
OCTOS_TUI_SOAK_TRANSPORT=ws OCTOS_TUI_SOAK_RUN_ID=<run-id> scripts/run-onboarding-tmux-soak.sh verify-solo
```

First-launch splash capture:

```sh
OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE=1 \
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=first-launch-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-first-launch
```

This mode is intentionally opt-in. It starts the TUI without a preselected
profile or session, refuses to reuse an existing profile JSON, waits for the
`Welcome to Octos` onboarding surface, and writes
`tui-capture-first-launch.txt` beside the regular `tui-capture.txt`.
`verify-first-launch` checks the retained capture for the local profile splash,
the `OCTOS` wordmark, and absence of OTP/setup-provider text. Use this lane
when collecting M22/M19 evidence for the first-launch splash; keep the default
launch shape for provider-missing and coding-session lanes.

Missing-provider recovery capture:

```sh
OCTOS_TUI_SOAK_TRANSPORT=stdio \
OCTOS_TUI_SOAK_RUN_ID=provider-missing-$(date -u +%Y%m%dT%H%M%SZ) \
scripts/run-onboarding-tmux-soak.sh start

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh drive-provider-missing

OCTOS_TUI_SOAK_RUN_ID=<same-run-id> \
scripts/run-onboarding-tmux-soak.sh verify-provider-missing
```

`verify-provider-missing` checks `tui-capture-provider-missing.txt` for the
provider setup recovery screen, local profile readiness, provider catalog
action, and API-key row. It fails if the capture is still on the first-launch
splash, has already opened a coding session, or contains OTP/AppUI error text.

The solo lane writes these M12 artifacts into
`e2e/test-results-tui-onboarding/<run-id>/`:

- `tui-capture.txt`
- `tui-capture-first-launch.txt` when
  `OCTOS_TUI_SOAK_FIRST_LAUNCH_CAPTURE=1`
- `tui-capture-provider-missing.txt` when running `drive-provider-missing`
- `server.log`
- `appui-transcript.jsonl`
- `runtime-policy-stamp.json`
- `tool-registry-snapshot.json`
- `mcp-config-before.redacted.json`
- `mcp-config-after.redacted.json`
- `mcp-status-list.json`
- `mcp-connection-test-result.json`
- `approval-events.jsonl`
- `filesystem-probe.json`
- `soak-summary.json`

The transcript artifact is redacted for global capability method lists so the
no-OTP assertion is about actual onboarding traffic. The exact outbound AppUI
method names remain visible; the verifier fails if `auth/send_code` or
`auth/verify` appears.

On 249/mini hosts, pin the run id, port, and artifact root:

```sh
OCTOS_TUI_SOAK_RUN_ID="$(hostname)-solo-$(date -u +%Y%m%dT%H%M%SZ)" \
OCTOS_TUI_SOAK_PORT=50249 \
OCTOS_TUI_SOAK_ARTIFACT_ROOT="$PWD/e2e/test-results-tui-onboarding" \
OCTOS_TUI_SOAK_TRANSPORT=stdio \
  scripts/run-onboarding-tmux-soak.sh drive-solo
```

## Manual Flow

Attach to the TUI:

```sh
tmux attach -t octos-onboard-tui-<run-id>
```

Then complete the guided wizard:

1. `/onboard`
   - `/onboard profile <profile-id>`;
   - `/onboard email <address>` and `/onboard send-code` when OTP is required;
   - `/onboard code <otp>` and `/onboard verify`;
   - `/onboard catalog` to load the dashboard-owned provider schema;
   - choose a catalog route from the menu, or use
     `/onboard select <family> <model> <route> [base-url] [api-key-env]`;
   - `/onboard key <secret>`;
   - `/onboard save`;
   - `/onboard finish`.
2. `/model`
   - verify the menu renders only server-returned provider/model state;
   - verify it does not contain hard-coded TUI model defaults.
3. Prompt:
   - send `Reply with exactly OK.`;
   - verify the response completes and the runtime policy stamp uses the
     configured family/model/route.

For deterministic local smoke without hand typing:

```sh
OCTOS_TUI_SOAK_API_KEY=<secret-value> \
scripts/run-onboarding-tmux-soak.sh drive-onboard
```

`drive-onboard` sends only `/onboard` commands through the live tmux TUI and
then captures the pane. The retained artifacts must show masked secrets only.

## Verify

For Moonshot AutoDL:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=moonshot \
OCTOS_TUI_SOAK_EXPECT_MODEL=kimi-k2.5 \
OCTOS_TUI_SOAK_EXPECT_ROUTE=autodl \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=https://www.autodl.art/api/v1 \
scripts/run-onboarding-tmux-soak.sh verify
```

For MiniMax WiseModel:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=minimax \
OCTOS_TUI_SOAK_EXPECT_MODEL=MiniMax-M2.5-highspeed \
OCTOS_TUI_SOAK_EXPECT_ROUTE=wisemodel \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=https://open.ospreyai.cn/v1 \
scripts/run-onboarding-tmux-soak.sh verify
```

For a custom OpenAI-compatible route:

```sh
OCTOS_TUI_SOAK_RUN_ID=<run-id> \
OCTOS_TUI_SOAK_EXPECT_FAMILY=<custom-family-id> \
OCTOS_TUI_SOAK_EXPECT_MODEL=<custom-model-id> \
OCTOS_TUI_SOAK_EXPECT_ROUTE=<custom-route-id> \
OCTOS_TUI_SOAK_EXPECT_BASE_URL=<custom-base-url> \
scripts/run-onboarding-tmux-soak.sh verify
```

To check secret redaction, pass the test key as an environment variable only:

```sh
OCTOS_TUI_SOAK_API_KEY=<secret-value> scripts/run-onboarding-tmux-soak.sh verify
```

The verifier fails if that value appears in any retained evidence artifact
listed below. `profile-json-after.json` is always redacted before the leak
check. Raw backend state under the run data directory is not treated as retained
evidence because it may legitimately contain the provider secret while the live
server is running. If a run is only validating captures and no profile file is
expected, set
`OCTOS_TUI_SOAK_REQUIRE_PROFILE=0`; provider expectation variables still require
profile JSON.

## Required Artifacts

- `summary.env`
- `server.log`
- `server-pane.txt`
- `tui-capture.txt`
- `profile-json-before.json` when present
- `profile-json-after.json` with `config.env_vars` redacted
- `runtime-policy-stamp.txt`
- `runtime-policy-stamp.json`
- `soak-summary.json`

The runner also writes `api-parity-checklist.json` as lightweight evidence for
the profile API parity contract below.

## API Contract Checks

The live run must be backed by automated API tests in `octos`. This repository
records the expected equivalence checklist via:

```sh
scripts/run-onboarding-tmux-soak.sh api-parity
```

The checklist covers these cases:

- catalog API returns the dashboard provider catalog shape;
- upsert API persists Moonshot AutoDL route exactly;
- upsert API persists MiniMax WiseModel route exactly;
- upsert API persists a custom OpenAI-compatible family/model/route exactly;
- provider test API does not return or log secrets;
- dashboard REST/profile patch and AppUI upsert produce equivalent profile JSON.

For parity comparison, normalize by redacting `config.env_vars` values and
ignoring timestamp/order-only differences. The values that must match are
`config.llm.primary.family_id`, `model_id`, `route.route_id`,
`route.base_url`, `route.api_key_env`, `route.api_type`, and the env var keys.
