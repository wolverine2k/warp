# Worker-Side BYOP Contract — Remote Orchestration

**Date:** 2026-05-25
**Author:** nmehta
**Branch:** `multi-local-llm`
**Audience:** warp.dev / Namespace server team (worker infrastructure)

---

## Summary

Phase 5d of the multi-local-llm initiative shipped the **client-side** surface for Remote BYOP (Bring Your Own Provider) orchestration. The Warp client now populates five new fields on `AgentConfigSnapshot` and `HarnessAuthSecretsConfig` when a user picks a BYOP-configured model with Remote execution. End-to-end Remote BYOP runs are gated on the **worker side** honoring these fields: resolving the managed secret to an API key, reconstructing the provider config, and routing the child agent's LLM traffic to the user's endpoint instead of the default cloud path. This document specifies the contract the worker must implement so that the client-side bridge produces working end-to-end runs.

---

## Status / Ownership

- **Owner:** Server team (warp.dev / Namespace worker infrastructure).
- **Not blocking on this repo.** The client-side bridge is complete and merged on the `multi-local-llm` branch. All five new wire fields are backward-compatible (`Option<T>`, serde-default, skip-if-None); pre-5d clients and servers interoperate without change.
- **Related client-side documents:**
  - Implementation plan: [`plan-phase-5d.md`](plan-phase-5d.md)
  - Umbrella spec: [`spec-phase-5.md`](spec-phase-5.md)
  - Project index: [`README.md`](README.md)
- **Final client-side commit:** `6d3a3cd1` on `multi-local-llm`.

---

## Inbound Wire-Shape Additions

Phase 5d extends two structs that cross the orchestration RPC boundary via `SpawnAgentRequest.config`. Both structs are JSON-serialized on the wire.

### Wire-shape example: pre-5d payload (unchanged)

```json
{
  "name": "child-1",
  "model_id": "claude-sonnet-4",
  "environment_id": "env-abc",
  "harness": { "type": "claude" },
  "harness_auth_secrets": {
    "claude_auth_secret_name": "my-claude-key"
  }
}
```

### Wire-shape example: fully-populated 5d payload

```json
{
  "name": "child-1",
  "model_id": "byop:a1b2c3d4-e5f6-7890-abcd-ef1234567890:claude-sonnet-4-20250514",
  "environment_id": "env-abc",
  "harness": { "type": "claude" },
  "harness_auth_secrets": {
    "claude_auth_secret_name": "my-claude-key",
    "byop_auth_secret_name": "byop-a1b2c3d4-e5f6-7890-abcd-ef1234567890"
  },
  "byop_base_url": "https://api.anthropic.com/v1",
  "byop_api_type": "anthropic",
  "compaction_model_provider_id": "f9e8d7c6-b5a4-3210-fedc-ba0987654321",
  "compaction_model_id": "claude-haiku-3"
}
```

### Field 1: `byop_base_url`

- **Struct:** `AgentConfigSnapshot`
- **Serde key:** `byop_base_url`
- **Type:** `Option<String>`
- **Backward-compat:** `#[serde(default, skip_serializing_if = "Option::is_none")]`. Pre-5d payloads deserialize to `None`. Absent on non-BYOP launches.
- **When populated:** The client sets this when the run-wide `model_id` starts with `byop:` and the execution mode is Remote. The value is the provider's `base_url` from settings, e.g. `"https://api.anthropic.com/v1"` or `"https://api.deepseek.com"`.
- **When `None`:** Non-BYOP launches (cloud-Warp dispatch). The worker should follow the existing cloud dispatch path unchanged.
- **Source of truth:** `app/src/ai/agent_providers/mod.rs:277` (`resolve_byop_for_remote_child`), populated at `app/src/pane_group/pane/terminal_pane.rs:2166`.

### Field 2: `byop_api_type`

- **Struct:** `AgentConfigSnapshot`
- **Serde key:** `byop_api_type`
- **Type:** `Option<String>`
- **Backward-compat:** Same as `byop_base_url`.
- **When populated:** Always alongside `byop_base_url`. Never set independently.
- **Canonical wire strings:** The client maps the six `AgentProviderApiType` enum variants to these stable strings:

  | Enum variant | Wire string |
  |---|---|
  | `OpenAi` | `"open_ai"` |
  | `OpenAiResp` | `"open_ai_resp"` |
  | `Anthropic` | `"anthropic"` |
  | `Gemini` | `"gemini"` |
  | `Ollama` | `"ollama"` |
  | `DeepSeek` | `"deep_seek"` |

- **Source of truth:** `app/src/ai/agent_providers/mod.rs:288-295` (the `match provider.api_type` block in `resolve_byop_for_remote_child`).

### Field 3: `compaction_model_provider_id`

- **Struct:** `AgentConfigSnapshot`
- **Serde key:** `compaction_model_provider_id`
- **Type:** `Option<String>`
- **Backward-compat:** Same pattern.
- **When populated:** When the user has Phase 4d compaction settings configured (`byop_compaction_model_provider_id` non-empty in `AISettings`) and execution mode is Remote. The value is the UUID of the BYOP provider the user selected for compaction.
- **When `None`:** No dedicated compaction model configured, or local execution. The worker should use the primary model for compaction (fallback).
- **Source of truth:** `app/src/pane_group/pane/terminal_pane.rs:2209-2216`.

### Field 4: `compaction_model_id`

- **Struct:** `AgentConfigSnapshot`
- **Serde key:** `compaction_model_id`
- **Type:** `Option<String>`
- **Backward-compat:** Same pattern.
- **When populated:** Always alongside `compaction_model_provider_id`. The value is the user-side model id for the compaction model (e.g. `"claude-haiku-3"`).
- **When `None`:** Same as `compaction_model_provider_id`.
- **Source of truth:** Same as field 3.

### Field 5: `byop_auth_secret_name`

- **Struct:** `HarnessAuthSecretsConfig` (nested inside `AgentConfigSnapshot.harness_auth_secrets`)
- **Serde key:** `byop_auth_secret_name`
- **Type:** `Option<String>`
- **Backward-compat:** Same pattern. Coexists with the existing `claude_auth_secret_name` and `codex_auth_secret_name` fields; all three can be set simultaneously on the same payload.
- **When populated:** When the run-wide `model_id` is a `byop:` entry and the provider's `remote_secret_name` is non-empty. The value is the name of a managed secret in the user's personal store (e.g. `"byop-a1b2c3d4-..."`).
- **When `None`:** Non-BYOP launches, or BYOP providers that haven't configured a remote secret (Local-only usage).
- **Source of truth:** `app/src/pane_group/pane/terminal_pane.rs:2193-2205`.

---

## Per-api_type Routing Matrix

When the worker receives a `SpawnAgentRequest` where `config.byop_base_url` and `config.byop_api_type` are both `Some(...)`, the worker must route the child agent's LLM traffic to the user's endpoint. The routing depends on the `byop_api_type` value and the harness (from `config.harness.type`).

### Harness compatibility

The client already filters BYOP entries by harness compatibility at submit time (Phase 5a's `byop_harness_compatible` matrix in `app/src/ai/byop_orchestration_filter.rs:34`). The worker can trust that gating happened upstream. For defense in depth, the worker should reject spawn requests with incompatible combinations rather than silently falling back.

| `byop_api_type` | Compatible harnesses |
|---|---|
| `anthropic` | Native (oz), `claude` |
| `open_ai` | Native (oz), `codex`, `opencode` |
| `open_ai_resp` | Native (oz), `codex` |
| `deep_seek` | Native (oz), `codex`, `opencode` |
| `gemini` | Native (oz), `gemini` |
| `ollama` | Native (oz) only |

### Env vars the worker must set on the child process

These mirror Phase 5c's `byop_env_for_harness` matrix (`app/src/ai/orchestration_byop_env.rs:42`) exactly. The worker must set these env vars on the child process before launch. `{api_key}` is resolved from the managed secret named `byop_auth_secret_name` (see section 5). `{model_id}` is the user-side model id extracted from the `model_id` field: the segment after the second colon in `byop:<provider_id>:<model_id>`.

| Harness | `byop_api_type` | Env vars |
|---|---|---|
| `claude` | `anthropic` | `ANTHROPIC_BASE_URL={byop_base_url}`, `ANTHROPIC_API_KEY={api_key}` |
| `codex` | `open_ai`, `open_ai_resp`, `deep_seek` | `OPENAI_BASE_URL={byop_base_url}`, `OPENAI_API_KEY={api_key}`, `OPENAI_MODEL={model_id}` |
| `opencode` | `open_ai`, `deep_seek` | `OPENAI_BASE_URL={byop_base_url}`, `OPENAI_API_KEY={api_key}` |
| `gemini` | `gemini` | (Gemini CLI is not currently enabled as a child harness; defer implementation) |
| `oz` (Native) | any | No env vars. Native dispatch routes through the in-process BYOP runner using `byop_base_url` + `byop_api_type` + resolved `api_key` directly. |

**Notes:**

- `ANTHROPIC_MODEL` is intentionally NOT set in the BYOP env-var bag. It is already set by the existing `harness_model_env_vars` path for the Claude harness. The BYOP bag is merged after the harness-specific bag, so BYOP entries take precedence on key collision.
- The `model_id` passed to `OPENAI_MODEL` is the user-facing model id (the part after the second colon in `byop:<provider_id>:<model_id>`), not the full `byop:...` LLMId.
- All string values (`byop_base_url`, `api_key`, `model_id`) should be trimmed of leading/trailing whitespace before use (the client trims defensively; the worker should do the same).

---

## Managed-Secret Resolution

When `harness_auth_secrets.byop_auth_secret_name` is `Some(name)`, the worker must resolve that name to an actual API key before spawning the child agent.

### Resolution path

1. Read the managed secret named `name` from the warp.dev managed-secrets store.
2. The secret is owned by `SecretOwner::CurrentUser` (Personal). The worker authenticates as the user who initiated the orchestration run, so the secret should be accessible via the same credential scope.
3. The secret value is the raw API key string (e.g. `"sk-ant-api03-..."` for Anthropic, `"sk-proj-..."` for OpenAI). It was stored by the client-side Auto-create flow via `UpdateManager::create_managed_secret(owner=Personal, name="byop-{provider_id}", type=GenericApiKey, value=api_key)`.
4. Use the resolved API key as `{api_key}` in the env-var matrix above.

### Client-side reference

The Auto-create button on the client calls:
```rust
UpdateManager::create_managed_secret(
    SecretOwner::CurrentUser,
    format!("byop-{provider_id}"),
    ManagedSecretType::GenericApiKey,
    api_key,                            // the raw key from AgentProviderSecrets
    Some("BYOP key for provider {provider_id}"),
)
```

Source: `app/src/settings_view/ai_page.rs` (the `AutoCreateAgentProviderManagedSecret` action handler).

### Error handling

If the managed secret is missing, empty, or the worker cannot resolve it:

- **Fail the spawn with a structured error.** Recommended status: `Failed` with a `TaskStatusMessage` explaining the resolution failure (e.g. `"BYOP managed secret 'byop-abc123' not found or empty. Re-create it in Settings > AI > Custom AI Providers."`).
- **Do NOT fall back to cloud-Warp dispatch.** The user explicitly chose a BYOP model; silent fallback to cloud would route traffic to an unintended endpoint and consume Warp credits unexpectedly.
- **Do NOT log the resolved API key value.** The secret name is safe to log; the secret value is not.

---

## Compaction-Model Dispatch

Phase 4d introduced a dedicated compaction (summarization) model that can differ from the primary conversation model. When the user configures this and runs Remote orchestration, the client forwards the compaction config via two fields on `AgentConfigSnapshot`.

### Resolution

When `compaction_model_provider_id` and `compaction_model_id` are both `Some(...)`:

1. The worker should route the conversation's compaction/summarization pipeline to the specified provider and model pair.
2. The provider config (`base_url`, `api_type`, API key) for the compaction provider must be looked up the same way as the primary BYOP entry: via the managed-secret store, indexed by provider id.
   - The compaction provider's managed secret name follows the same `byop-{provider_id}` convention. The worker should look up `byop-{compaction_model_provider_id}` in the managed-secret store.
   - The compaction provider's `base_url` and `api_type` are NOT forwarded on the wire (only the provider id and model id are). The worker must either look these up from a shared provider registry or require the client to forward them in a future iteration.

### Edge cases

- **Compaction provider differs from primary:** The worker needs both sets of credentials (primary BYOP key via `byop_auth_secret_name`, compaction key via a separate `byop-{compaction_model_provider_id}` lookup). Both secrets are Personal-owner and should be resolvable under the same user scope.
- **Only one of the two fields is set:** Treat as "use the primary model for compaction." This is the conservative fallback the client uses locally.
- **Both fields absent (`None`):** No dedicated compaction model configured. The worker uses whatever its default compaction strategy is (typically the conversation's primary model).
- **Compaction provider deleted client-side after submission:** The managed secret may still exist in the store even if the user deleted the provider from their local settings. If the secret resolves, use it. If it doesn't, fall back to the primary model for compaction and log a warning.

---

## Backward Compatibility & Forward Compatibility

### Backward compatibility (pre-5d clients)

Pre-5d clients send `SpawnAgentRequest` payloads without the five new fields. Because all five use `Option<T>` with `serde(default)`, the worker deserializes them as `None`. The worker must treat `None` values as "cloud-Warp dispatch" (no change from today's behavior). Concretely:

- `byop_base_url: None` + `byop_api_type: None` = use the existing cloud dispatch path.
- `byop_auth_secret_name: None` = no BYOP credential to resolve.
- `compaction_model_provider_id: None` + `compaction_model_id: None` = use default compaction.

### Forward compatibility (future additions)

Future field additions on `AgentConfigSnapshot` should follow the same pattern:

- `Option<T>` type.
- `#[serde(default)]` so old payloads deserialize cleanly.
- `#[serde(skip_serializing_if = "Option::is_none")]` so absent fields don't appear on the wire.
- Workers that don't yet recognize a new field silently ignore it (standard JSON forward-compat convention).

The worker's deserializer should be configured to ignore unknown fields (`#[serde(deny_unknown_fields)]` must NOT be present on the receiving struct).

---

## Security Considerations

### BYOP base_url validation

The `byop_base_url` is user-controlled. The worker must validate it before dispatching requests:

1. **Require HTTPS.** Reject `http://` URLs. Exception: `http://localhost:*` and `http://127.0.0.1:*` may be permitted in development/staging environments if the worker is explicitly configured to allow them, but production workers should reject all non-HTTPS URLs.
2. **Reject loopback and private addresses.** The client already applies `base_url_reachable_from_remote` (`app/src/ai/byop_orchestration_filter.rs:79`) to filter out localhost, 127.0.0.0/8, RFC1918 (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16), `.local`, and `.localhost` URLs before they reach the wire. As defense in depth, the worker should apply the same check server-side. If a private URL somehow made it onto the wire (client bug, API bypass, or future client regression), the worker must reject it rather than attempt to connect.
3. **Do NOT follow redirects blindly.** A BYOP URL that 30x-redirects to an internal service (SSRF) should be blocked. Pin the redirect policy to same-origin or disable redirects entirely for BYOP-routed requests.

### API key handling

- **Do NOT log the API key** resolved from the managed-secret store. The secret name is safe to log for debugging; the value is not.
- **Do NOT include the API key in error messages** returned to the client or stored in task status messages.
- **Memory:** Clear the resolved key from memory after the child process has been spawned and the env vars have been set.

### Managed-secret access control

- The worker authenticates as the user who submitted the orchestration run. It should only resolve secrets owned by that user (`SecretOwner::CurrentUser`). A worker must not resolve secrets owned by other users or teams unless explicitly scoped by a team-level orchestration feature (out of scope for this contract).

---

## Telemetry Hooks

Optional but recommended. These counters and metrics give observability into BYOP Remote adoption and failure modes without exposing PII.

| Metric | Labels | Notes |
|---|---|---|
| `byop_remote_dispatch_total` | `byop_api_type`, `harness`, `status={success,failure}` | Per-dispatch counter. |
| `byop_secret_resolution_total` | `status={success,not_found,empty,error}` | Tracks managed-secret lookup outcomes. |
| `byop_dispatch_latency_seconds` | `byop_api_type`, `harness` | Histogram of time from spawn-request receipt to child process launch. |
| `byop_base_url_rejected_total` | `reason={not_https,private_ip,loopback,redirect}` | Defense-in-depth rejections. |

**PII exclusions:** Do not include `byop_base_url` path segments, API key values, provider id UUIDs, or user identifiers in metric labels. The `byop_api_type` and `harness` labels are sufficient for operational triage.

---

## Verification Gate

A server-side smoke test confirming the contract is honored end-to-end:

1. **Setup:** Spin up a mock BYOP endpoint (e.g. a simple HTTP server that accepts OpenAI-compatible `/chat/completions` requests and returns a canned streaming response).
2. **Client-side config:** Configure a BYOP provider in the Warp client with:
   - `api_type = OpenAi`
   - `base_url = https://{mock-endpoint-host}`
   - A valid API key stored in `AgentProviderSecrets`
   - `available_for_orchestration = true`
   - `remote_secret_name` set via the Auto-create button (or manually entered)
3. **Launch:** Pick the BYOP model in the orchestration picker, select `codex` harness + `Remote` execution, launch with at least 1 child agent.
4. **Verify (a):** Inspect the `SpawnAgentRequest` payload received by the server. Confirm:
   - `config.byop_base_url` = the mock endpoint URL
   - `config.byop_api_type` = `"open_ai"`
   - `config.harness_auth_secrets.byop_auth_secret_name` = the managed secret name
5. **Verify (b):** Confirm the worker resolves the managed secret to the API key value.
6. **Verify (c):** Confirm the spawned child process (Codex CLI) has `OPENAI_BASE_URL`, `OPENAI_API_KEY`, and `OPENAI_MODEL` set in its environment, and that its LLM traffic hits the mock endpoint with the expected `Authorization: Bearer {api_key}` header.
7. **Verify (d):** Repeat with `anthropic` api_type + `claude` harness to confirm `ANTHROPIC_BASE_URL` / `ANTHROPIC_API_KEY` routing.

---

## Out of Scope

The following are explicitly NOT part of this contract:

- **Cloud-Warp dispatch.** Unchanged. When `byop_base_url` is `None`, the worker follows the existing cloud path. No modifications needed.
- **Local execution.** Phase 5b (Native path) and Phase 5c (external-CLI env-var injection) handle BYOP routing entirely on the client side for Local execution. The worker is not involved.
- **Self-hosted worker (`crates/remote_server/`) BYOP support.** If the self-hosted worker path is later confirmed to run orchestration children, a separate follow-up adds the receive-side handling there. This contract covers Warp-managed (Namespace) workers only.
- **Provider-edit-during-orchestration banners.** The client-side spec (Phase 5 risks) calls out that editing a provider mid-run has inconsistent effects (Local children see the change; Remote children use the snapshotted config). A banner warning the user is a follow-up, not part of this contract.
- **Gemini CLI as a local child harness.** Currently disabled in the client (`normalize_local_child_harness` filters it out). If enabled in the future, the env-var matrix above would need a Gemini row (`GOOGLE_API_KEY`, etc.).
- **Per-child model override.** The run-wide `model_id` applies to all children. Per-child BYOP model selection is not supported in Phase 5.

---

## Open Questions for Server Team

These are items that cannot be determined from the Warp client repo alone. The server team should resolve them before or during implementation.

1. **Managed-secret store location and API surface.** Where exactly does the managed-secret store live server-side? Is there an existing resolution API the worker can call (e.g. `GET /secrets/{name}?owner=personal`), or does a new endpoint need to be built? The client-side write path uses `UpdateManager::create_managed_secret`; what is the server-side read equivalent?

2. **Per-user credential broker.** Is there an existing per-user credential broker in the worker infrastructure that we should reuse for BYOP secret resolution, or does this require a new path? The worker authenticates as the user who submitted the run — is that identity available at child-spawn time, or does it need to be threaded through?

3. **Worker pool network policy.** Do Namespace workers allow arbitrary outbound HTTPS? BYOP base URLs point at arbitrary user-controlled endpoints (e.g. `https://api.deepseek.com`, `https://my-company-llm.example.com`). If the worker pool has egress restrictions (allowlists, proxy requirements), BYOP Remote will fail for endpoints not on the allowlist. Does the worker need a policy exception or a configurable egress rule for BYOP traffic?

4. **Compaction provider config resolution.** The wire shape forwards `compaction_model_provider_id` + `compaction_model_id` but NOT the compaction provider's `base_url` and `api_type`. The worker needs these to dispatch compaction requests. Options: (a) the worker looks up the provider config from a shared registry indexed by provider id, (b) the client adds `compaction_base_url` + `compaction_api_type` fields in a follow-up, (c) the worker infers them from the managed-secret name pattern. Which approach does the server team prefer?

5. **Per-request audit logging.** Does compliance require per-request audit logging for BYOP-routed traffic? If so, what fields should be logged (user id, provider id, api_type, harness, timestamp) and what must be excluded (api_key, base_url path)?

6. **Rate limiting / abuse prevention.** Since BYOP routes user traffic through Warp-managed workers to arbitrary endpoints, is there a risk of workers being used as proxies? Should there be per-user rate limits on BYOP Remote dispatches, or does the existing orchestration rate limiting suffice?

7. **Native (oz) harness BYOP dispatch.** When `harness.type` is `oz` (Native) and BYOP fields are present, the worker needs to route the in-process agent loop's LLM calls to the BYOP endpoint. Does the existing Native agent runner support configurable LLM endpoints, or does it always use the Warp cloud? If the latter, this is the most significant implementation gap for Native BYOP Remote.
