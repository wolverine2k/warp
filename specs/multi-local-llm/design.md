# Multi-Local-LLM Provider Support — Design

**Status:** ✅ **Phase 1 complete** (tagged `v0.1.0`, 2026-05-08). Phases 1a / 1b-1 / 1b-2 / 1b-3 shipped end-to-end; Phase 1b-4 cleanup is drafted and queued. Phases 2 / 3 / 4 (provider-adapter trait, native non-OpenAI adapters, polish) remain future work — each gets its own design + plan when started.
**Author:** nmehta
**Date:** 2026-05-07 (initial design); 2026-05-08 (Phase 1 shipped)
**Branch:** `multi-local-llm` (forked from `nmehta/local-llm-provider` @ `64d5172a`, tagged `v0.1.0` at the dispatch-scoping fix)
**Related work:** `specs/GH9303/` (single-provider Phase A/B), `openwarp` branch (full BYOP reference design)

## Goal

Allow a Warp user to configure **multiple local-LLM providers** simultaneously (e.g. Ollama on localhost:11434, LM Studio on localhost:1234, a remote OpenAI-compatible box, plus eventually an Anthropic key, a Gemini key, a DeepSeek key) and pick a specific *provider × model* per conversation. The single-provider scaffolding already on `nmehta/local-llm-provider` (config struct, OpenAI-compatible wire code, compaction, multi-turn agent loop) is preserved and extended; existing user configs auto-migrate intact.

## Non-goals

- No cloud-Warp dispatch changes — existing cloud path is untouched.
- No new database schema. Settings + keychain only.
- No model fine-tuning, quantization, or model-management UI.
- No multi-account-per-provider support (one API key per provider entry).

## High-level approach

Three near-term observations drive the design:

1. The `openwarp` branch already implements a clean multi-provider design under the BYOP brand. Its data model (`AgentProvider`, `AgentProviderApiType`, `AgentProviderModel`, `AgentProviderSecrets`, `byop:<provider_id>:<model_id>` LLM IDs) is well-shaped and openwarp-compatible. We **adopt those names verbatim from day one**, even though Phase 1 only implements the `OpenAi` api type.
2. The current branch's OpenAI-compatible wire code (`crates/ai/src/local_provider/{request,response,wire}.rs`) is correct and tested. We keep it. Phase 2 wraps it behind a `ProviderAdapter` trait so we can add native Anthropic / Gemini / Ollama / DeepSeek dispatchers without rewriting the run loop.
3. The biggest single risk is **migration safety**. Existing users on `nmehta/local-llm-provider` already have a populated `agents.local_provider.*` block plus a keychain entry. The migration runs once at first launch, is non-destructive, and is unit-tested with a fixture profile.

## Scope summary

| Aspect | Phase 1 (this design covers in detail) | Later phases |
|---|---|---|
| Multiple providers | Yes — `Vec<AgentProvider>`, add/remove from settings | — |
| API types supported | OpenAI-compatible only | Anthropic, Gemini, Ollama-native, DeepSeek (Phase 3) |
| Per-conversation provider+model selection | Yes | — |
| Models per provider | Multiple, free-text IDs | `/models` fetch (Phase 4a), models.dev catalog (Phase 4b) |
| Capabilities | `tool_call: bool` per model | `image/pdf/audio/reasoning` per model (Phase 4c) |
| Migration of existing single-provider config | Yes, one-time, non-destructive | — |
| Compaction | Existing pipeline, per-conversation | Dedicated `compaction_model` (Phase 4d) |
| Settings UI | Provider list + model rows | Test connection (Phase 2), quick-add chips (Phase 4b) |

---

## 1. Data model

### 1.1 Settings schema (TOML, `settings.toml`)

```toml
[agents.warp_agent]
providers = [
  {
    id = "uuid-v4-string",                 # immutable; also keys the keychain entry
    name = "My Ollama",                    # user-facing display name
    kind = "openai_compatible",            # AgentProviderKind
    api_type = "openai",                   # AgentProviderApiType: openai|openai_resp|anthropic|gemini|ollama|deepseek
    base_url = "http://localhost:11434/v1",
    models = [
      {
        id = "llama3.1",                   # value sent to upstream as `model`
        name = "Llama 3.1",                # picker display
        context_window = 128000,           # 0 = unknown, no token-budget enforcement
        max_output_tokens = 0,             # 0 = unspecified
        tool_call = true,                  # advertise tool schemas?
        # Phase 4c additions: image, pdf, audio (Option<bool>), reasoning (bool)
      },
    ],
  },
]
byop_last_used_model_id = "byop:<uuid>:<model_id>"   # picker default for new conversations

[agents.byop_compaction]
auto = true
prune = true
tail_turns = 2
preserve_recent_tokens = 0                 # 0 = use formula default
reserved = 0                               # 0 = use formula default
# Phase 4d:
# [agents.byop_compaction.model]
# provider_id = "uuid"
# model_id    = "claude-3-5-haiku-20241022"

[agents.warp_agent.migration]
legacy_local_provider_migrated = true      # set after one-time migration runs
```

Phase 1 only implements the `OpenAi` variant of `AgentProviderApiType`; the other variants exist in the enum but `lookup_byop` returns an `UnsupportedApiType` error if invoked. This keeps the schema stable so that adding a new variant in Phase 3 is purely additive.

### 1.2 Secrets (OS keychain)

Single entry, keyed `AgentProviderSecrets`, value is JSON-encoded `HashMap<provider_id_string, api_key_string>`. Replaces the current `LocalProviderApiKey` singleton.

### 1.3 LLM ID format

```
byop:<provider_id_uuid>:<model_id>
```

`crates/ai/src/local_provider/llm_id.rs` (new) provides:

```rust
pub fn encode(provider_id: &str, model_id: &str) -> LLMId;
pub fn decode(llm_id: &LLMId) -> Option<(ProviderId, ModelId)>;
```

A second parser `decode_legacy_local(&LLMId) -> Option<ModelId>` recognizes the existing `local:<model_id>` format and is used **only by the migration helper**.

Picker label format: `"{provider.name} / {model.name}"` (matches openwarp).

### 1.4 No DB schema changes

Conversation rows already carry an `LLMId` field (today's `local:<model>` value lives there). Phase 1 just stores `byop:<uuid>:<model>` instead. Conversations created before migration are rewritten on load via `agent_conversations_model::rewrite_legacy_llm_id_if_needed(...)`.

---

## 2. Provider abstraction

### 2.1 Phase 1: no trait, internal snapshot only

Phase 1 introduces no trait. The existing entry point — `local_provider::run_chat_turn(input, cfg, …)` — already takes a frozen config snapshot. We replace today's `LocalProviderConfig` with a richer `ProviderRuntimeConfig` built from `(AgentProvider, AgentProviderModel, api_key)`:

```rust
pub struct ProviderRuntimeConfig {
    pub provider_id:     ProviderId,
    pub display_name:    String,
    pub base_url:        String,
    pub api_key:         Option<String>,
    pub api_type:        AgentProviderApiType,   // Phase 1: only OpenAi accepted by run_chat_turn
    pub model_id:        String,
    pub context_window:  Option<u32>,
    pub max_output:      Option<u32>,
    pub supports_tools:  bool,
}
```

`run_chat_turn` checks `api_type` and errors `UnsupportedApiType` for non-OpenAI variants.

### 2.2 Phase 2: `ProviderAdapter` trait

Phase 2 hoists the adapter abstraction. Sketch (final shape may change with implementation):

```rust
pub trait ProviderAdapter: Send + Sync {
    fn compose_request(&self, input: &LocalProviderInput, cfg: &ProviderRuntimeConfig)
        -> Result<http::Request<Body>, AdapterError>;

    fn parse_chunk(&self, raw: &[u8], state: &mut StreamState)
        -> Result<Vec<ResponseEvent>, AdapterError>;

    fn extract_usage(&self, state: &StreamState) -> Option<UsageInfo>;
}
```

`OpenAiAdapter` is the existing code, just hoisted. `run.rs` becomes `match cfg.api_type { OpenAi => OpenAiAdapter, … }`.

### 2.3 Phase 3: native adapters

One PR per `api_type` variant. Decision deferred: hand-roll vs. pull in `genai`. Anthropic is the test case; if hand-rolling exceeds 1 week, switch to `genai`.

---

## 3. Dispatch flow (Phase 1)

```
RequestParams.model: LLMId
        │
        ▼
llm_id::decode  ──►  (provider_id, model_id)         (or legacy decode → migrated re-encode)
        │
        ▼
agent_providers::lookup_byop(app, &llm_id)
        │
        ▼
(AgentProvider, AgentProviderModel, api_key)
        │
        ▼
ProviderRuntimeConfig::from_lookup(...)
        │
        ▼
agent/api/impl.rs::route_to_local_provider(params, runtime_cfg)
        │
        ▼
local_provider::run_chat_turn(input, runtime_cfg, ...)
        │
        ▼
LocalResponseStream → ResponseEvent → blocklist controller
```

Cloud path is unchanged; the only change in `agent/api/impl.rs` is that the "is this local?" branch now gates on `LLMId.starts_with("byop:")` (with `local:` as a transitional fallback handled by the migration helper).

### 3.1 Provider unavailable handling

If `lookup_byop` returns `ProviderNotFound`, `ApiKeyMissing`, or `ModelNotFound` (e.g. user removed a provider mid-conversation), dispatch returns a structured error that the conversation surface renders as an inline banner: *"Provider 'My Ollama' is no longer configured. Pick another model to continue."* with a model picker. We do not silently fall back to cloud.

---

## 4. Per-conversation selection

`RequestParams.model` is already persisted per conversation. Phase 1 changes:

1. **New conversation:** picker reads `byop_last_used_model_id` (or first valid `(provider, model)` if unset). User selection at first turn writes through to both the conversation row and `byop_last_used_model_id`.
2. **Existing conversation:** the model field is stable across turns. Switching providers mid-conversation is allowed; the controller re-snapshots `ProviderRuntimeConfig` on each turn from the conversation's current model field.
3. **Compaction state:** keyed per conversation, unchanged. If the conversation switches providers, prior compaction summaries remain valid (they're plain text); the new model's `context_window` may trigger immediate re-compaction.

---

## 5. Settings UI (Phase 1)

New widget at `app/src/settings_view/agent_providers_widget.rs`. Replaces `LocalProviderWidget` once migration verified.

Layout:

```
[ + Add provider ]

╭─ Provider 1  (Ollama)               [×] Remove ──╮
│ Name        [ My Ollama                       ]   │
│ Base URL    [ http://localhost:11434/v1       ]   │
│ API key     [ ••••••••                        ]   │
│ API type    ( OpenAI )  [Anthropic disabled]      │   ← chips; Phase 1 only OpenAI is enabled
│                                                    │
│ Models                                  [ + Add ] │
│  Display name        Model ID       Ctx     Tools │
│  [ Llama 3.1 ]      [ llama3.1 ]   [128000] [☑]  │  [×]
│  [ Mistral 7B ]     [ mistral ]    [ 32000] [☑]  │  [×]
╰────────────────────────────────────────────────────╯

╭─ Provider 2  (LM Studio)            [×] Remove ──╮
…
```

UI actions wire into `AISettingsPageAction::AgentProvider*` variants. Each text-input save uses the existing blur/Enter pattern. Keychain writes are debounced. The legacy `LocalProviderWidget` block is removed once Phase 1 ships green.

Out of Phase 1: Test-connection button (Phase 2), `/models` fetch (Phase 4a), models.dev catalog + quick-add chips (Phase 4b), per-model multimodal/reasoning toggles (Phase 4c).

---

## 6. Migration (one-time, on first launch after upgrade)

Trigger:
- `agents.warp_agent.providers` is empty, AND
- `agents.local_provider.enabled` is true OR `agents.local_provider.base_url` is non-empty, AND
- `agents.warp_agent.migration.legacy_local_provider_migrated != true`.

Steps:

1. Generate UUID v4 → `provider_id`.
2. Synthesize one `AgentProvider`:
   - `name` = `local_provider_display_name` (fallback `"Local"`)
   - `kind = openai_compatible`, `api_type = openai`
   - `base_url` = `local_provider_base_url`
   - `models = [{ id: local_provider_model_id, name: local_provider_model_id,
                  context_window: parse(local_provider_context_window),
                  tool_call: local_provider_supports_tools }]`
3. Read API key from keychain entry `LocalProviderApiKey`. Write into `AgentProviderSecrets[provider_id]`. Leave the legacy keychain entry intact for rollback safety; a follow-up cleanup PR (after Phase 1b is in stable for one full release cycle and telemetry confirms migration adoption) deletes the legacy entry.
4. Move compaction settings: `local_provider_compaction_*` → `agents.byop_compaction.*` (field-by-field; identical types).
5. Walk persisted conversation list. For each row whose `model` LLMId starts with `local:`, rewrite to `byop:<provider_id>:<rest>` and persist. (This is a small SQL update or in-memory transform depending on where the field lives.)
6. Set `agents.warp_agent.migration.legacy_local_provider_migrated = true`.
7. Set `byop_last_used_model_id = byop:<provider_id>:<model_id>`.

The legacy `agents.local_provider.*` keys remain readable for one release as a deprecation window; the LocalProviderWidget UI is removed in Phase 1 to prevent users from editing the dead keys.

Idempotency: re-runs check the marker first and no-op. Even if the marker is missing but `providers` is non-empty, migration skips (we never wipe user-added providers).

---

## 7. Tools, compaction, telemetry

### 7.1 Tools
- Existing curated v1 set (`read_files`, `apply_file_diffs`, `run_shell_command`, `grep`, `file_glob_v2`) is preserved.
- `AgentProviderModel.tool_call: bool` (default `true`) gates whether `tools` array is sent for that model.
- No per-provider tool overrides in Phase 1.

### 7.2 Compaction
- Pipeline (`crates/ai/src/local_provider/compaction/*`) is unchanged in Phase 1.
- Settings move from `agents.local_provider.compaction_*` to `agents.byop_compaction.*` during migration.
- `CompactionConfig::from_settings()` reads the new path.
- Per-conversation compaction state behaves as today.
- Phase 4d adds optional `CompactionModelRef { provider_id, model_id }` for routing summarization to a different (cheaper/faster) model.

### 7.3 Telemetry
- New AI-event field `provider_api_type: String` (the enum string, not the user-given name).
- `provider_id` is hashed (SHA-256, first 8 hex) before logging — useful for correlation, not reversible.
- `base_url` and `api_key` **never** logged.
- `model_id` logged as-is (it's a public model name).

---

## 8. Naming changes

Split into two PRs:

**Phase 1a — symbol-only rename (no behavior change):**

| Old Rust symbol | New Rust symbol |
|---|---|
| `LocalProviderKeyManager` | `AgentProviderSecrets` (struct/file rename, same singleton, same keychain key for now) |
| `LocalProviderWidget` | `AgentProvidersWidget` (file rename, same field bindings for now) |
| `LocalProviderHistory` | (kept; it's per-conversation, not per-provider — name still fits) |
| `LocalTool` | (kept; tools are still local-runtime regardless of provider) |

The Phase 1a PR keeps the existing `local:` LLMId prefix, the `LocalProviderApiKey` keychain key, and the `agents.local_provider.*` TOML schema *intact* so it stays a pure mechanical rename. Review is mechanical.

**Phase 1b — schema + behavior changes (atomic with migration):**

| Old | New |
|---|---|
| `LocalProviderConfig` (in `app/src/ai/local_provider_config.rs`) | `ProviderRuntimeConfig` (in `crates/ai/src/local_provider/`) — built per-request from `(AgentProvider, AgentProviderModel, api_key)` |
| `local:<model>` LLMId prefix | `byop:<provider_id>:<model>` |
| keychain key `LocalProviderApiKey` | keychain key `AgentProviderSecrets` (JSON map keyed by provider id) |
| settings `agents.local_provider.*` | settings `agents.warp_agent.providers` (+ `agents.byop_compaction.*` for compaction subset) |
| `FeatureFlag::LocalLlmProvider` | **unchanged** (no rename) |

These four changes ship together with the migration helper so users never see a half-migrated state.

---

## 9. Phased plan

| Phase | Outcome | Files touched (approx.) | Verification gate |
|---|---|---|---|
| **1a. Rename PR** | Mechanical `LocalProvider*` → `AgentProvider*` / `byop:` rename. No behavior change. | ~30 files | Presubmit clean; existing tests pass; manual smoke test confirms identical UX |
| **1b. Multi-provider data model + migration + UI (OpenAI-compat only)** | User can add N providers, pick provider+model per conversation, legacy config migrates intact. | settings/ai.rs, agent_providers/{mod,secrets,llm_id,migration}.rs, agent_providers_widget.rs, agent/api/impl.rs, conversations_model migration, tests | Presubmit + integration test running 2 mock providers concurrently + manual test against real Ollama+LM Studio + verify legacy migration on a fixture profile |
| **2. ProviderAdapter trait refactor** ✅ shipped | Internal abstraction; no behavior change. "Test connection" button as a free win. | ~5 files in `crates/ai/src/local_provider/` | All existing tests pass; stub adapter exercises dispatch |
| **3a. Anthropic adapter** 🧪 code complete | Native Claude support. Hand-rolled against the Messages API (`/v1/messages`, `x-api-key` + `anthropic-version`, content-block message shape, named SSE events). `StreamDecoder` trait gained `feed_event` to carry the SSE event-name through. | new `local_provider/adapters/anthropic/{mod,wire,request,response}.rs` + sibling tests | Live test against `api.anthropic.com` — pending |
| **3b. Ollama-native adapter** 🧪 code complete | Native Ollama (`/api/chat`, NDJSON streaming, `options.num_ctx`, native tool-call streaming with arguments as JSON object). `ProviderAdapter` trait gained `streaming_format()` so the runner branches between SSE (existing) and NDJSON (new `synthesize_ndjson_stream` in `run.rs`) drive loops. Shared proto-event builders factored into `adapters/proto_helpers.rs`. | new `local_provider/adapters/ollama/{mod,wire,request,response}.rs` + tests + NDJSON drive loop in `run.rs` + `proto_helpers.rs` | Live test against local Ollama — pending |
| **3c. Gemini adapter** 🧪 code complete | Native Gemini (`POST /v1beta/models/{model}:streamGenerateContent?alt=sse`, `x-goog-api-key` auth, content-parts message shape with top-level `systemInstruction`, `user`/`model` role vocabulary, `functionCall`/`functionResponse` parts, `finishReason` as SSE terminator). Inherits the SSE `streaming_format` default — no `run.rs` changes. | new `local_provider/adapters/gemini/{mod,wire,request,response}.rs` + sibling tests | Live test against `generativelanguage.googleapis.com` — pending |
| **3d. DeepSeek adapter** 🧪 code complete | Native DeepSeek (`POST /chat/completions`, OpenAI-compatible wire shape with Bearer auth and `[DONE]` SSE terminator; reuses OpenAI's `chat_completions_url` / `models_list_url` helpers). Phase-3d novelty: `deepseek-reasoner` emits `delta.reasoning_content` alongside `delta.content` — the decoder surfaces it as a distinct `AgentReasoning` proto message. AgentReasoning is dropped from outbound history (API returns HTTP 400 if reasoning_content appears on inbound messages). | new `local_provider/adapters/deepseek/{mod,wire,request,response}.rs` + sibling tests | Live test against `api.deepseek.com` — pending |
| **4a. /models fetch button** | Per-provider one-click model discovery. | UI + small HTTP helper | Manual |
| **4b. models.dev catalog + quick-add chips** | Catalog-driven onboarding. | catalog cache + chips UI | Manual |
| **4c. Multimodal capabilities** | image/pdf/audio per model + attachment routing. | touches blocklist controller | Live test per modality |
| **4d. CompactionModelRef** | Dedicated summarization model. | compaction config + dispatch fork | Test with two providers |

The existing `FeatureFlag::LocalLlmProvider` continues to gate the entire feature through all phases.

---

## 10. Test plan (Phase 1)

### Unit
- `llm_id::encode/decode` round-trip; legacy `local:<model>` parser; malformed inputs.
- Migration: fixture profile with populated legacy fields → expected post-migration provider list, secrets, conversation rewrites; idempotent re-run.
- `lookup_byop`: provider-not-found, model-not-found, api-key-missing.
- `ProviderRuntimeConfig::from_lookup`: maps fields correctly; rejects non-OpenAI api_type with `UnsupportedApiType`.
- Settings UI actions: add/remove provider, add/remove model, name persistence on blur.

### Integration (`crates/ai/tests/`)
- `local_provider_integration.rs` — extend to register two mock OpenAI-compatible servers; run two conversations concurrently against different providers; assert each turn hits the right `base_url` and uses the right `model_id`.
- New `multi_provider_migration.rs` — fixture-driven legacy migration: load profile A (legacy single-provider), call migrator, assert post-state.

### Manual smoke
- Real Ollama + LM Studio side-by-side, two simultaneous conversations.
- Upgrade path: launch on a built copy of `nmehta/local-llm-provider` with populated config, then upgrade to `multi-local-llm` build, verify Provider 1 is the migrated entry and the conversation continues working.

---

## 11. Risks

1. **Picker explosion** with N×M entries. Phase 1 uses flat `"provider / model"` labels (matches openwarp). Tree picker is a Phase 4 polish if needed.
2. **Legacy migration must run exactly once.** Marker flag + idempotent re-runs (no-op if marker set or providers non-empty). Unit-tested.
3. **Keychain downgrade safety.** Migration is non-destructive — old `LocalProviderApiKey` stays until the deprecation window closes.
4. **Compaction overflow on first turn after migration.** Conversation's old context-window assumption may differ from the new model entry's value. Defensive: clamp `tail_turns` * `preserve_recent_tokens` to `min(model.context_window, configured)`.
5. **`genai` decision deferred to Phase 3a.** Documented; revisit if Anthropic hand-roll exceeds 1 week.
6. **Naming churn** is contained in Phase 1a (rename-only PR) so functional review of Phase 1b is clean.
7. **Conversation referencing deleted provider.** Inline banner + picker; no silent cloud fallback.

---

## 12. Out of scope (explicitly)

- Cloud Warp dispatch changes.
- New DB schema.
- Multi-account-per-provider (e.g. two distinct OpenAI keys with different rate limits).
- Provider health probes, latency monitoring, or auto-failover.
- Cost tracking / usage caps.
- Per-workspace provider scoping (single global provider list).

---

## Appendix A — File map (Phase 1b)

New:
- `crates/ai/src/local_provider/llm_id.rs`
- `crates/ai/src/local_provider/migration.rs`
- `app/src/ai/agent_providers/mod.rs`
- `app/src/ai/agent_providers/secrets.rs`
- `app/src/ai/agent_providers/lookup.rs`
- `app/src/settings_view/agent_providers_widget.rs`
- `crates/ai/tests/multi_provider_migration.rs`

Modified:
- `crates/ai/src/local_provider/{mod,run,request,response}.rs` — accept `ProviderRuntimeConfig`
- `crates/ai/src/local_provider/config.rs` — new types or removed (subsumed by settings types)
- `app/src/settings/ai.rs` — `AgentProvider`, `AgentProviderApiType`, `AgentProviderModel`, `AgentProviderKind` types + serde
- `app/src/ai/agent/api/impl.rs` — dispatch on `byop:` prefix, build `ProviderRuntimeConfig` via `lookup_byop`
- `app/src/ai/agent/conversation.rs` — model picker default + last-used persistence
- `app/src/ai/blocklist/controller.rs` — provider-unavailable banner state
- `app/src/ai/agent_conversations_model.rs` — legacy LLMId rewrite at conversation load
- `app/src/settings_view/ai_page.rs` — wire new widget, remove `LocalProviderWidget`
- `crates/ai/tests/local_provider_integration.rs` — multi-provider scenarios

Removed (after Phase 1b ships green):
- `app/src/ai/local_provider_config.rs` (replaced by `agent_providers/mod.rs`)
- `app/src/ai/local_provider_compaction.rs` (replaced by `agents.byop_compaction` settings)
