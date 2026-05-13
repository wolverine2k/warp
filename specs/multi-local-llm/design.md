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
        # image, pdf, audio (Option<bool>) and reasoning (bool) are already on
        # AgentProviderModel as of Phase 1b-1; Phase 4c wires them into the
        # send-path enforcement and adds the Auto inference (see §14).
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
# [agents.compaction]
# model = "byop:abc-uuid:claude-3-5-haiku-20241022"   # any LLMId, or unset to use the conversation primary; see §15

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
| **4a. /models fetch button** 🧪 code complete | Per-provider one-click model discovery. Adds `build_list_models_request` + `parse_list_models_response` to `ProviderAdapter` (with `UnsupportedApiType` default impls so `OpenAiResp` inherits a graceful "not supported"); a wire-agnostic `fetch_models()` helper with pagination + dedupe + 15s timeout; five new `AISettingsPageAction` variants + three view fields on `AISettingsPageView`; a card-style modal panel rendered above the providers list. See `plan-phase-4a.md`. | new `app/src/ai/agent_providers/fetch_models.rs`, new `app/src/settings_view/fetched_models_modal.rs`, per-adapter `list_models` parsers, edits to `agent_providers_widget.rs` + `ai_page.rs` | Live test against each of the 5 upstreams — pending |
| **4b. models.dev catalog + quick-add chips** | Catalog-driven onboarding (see §13). Inline chips beside the empty "+ Add Model" row plus a "Browse catalog" modal sourced from a cached `https://models.dev/api.json` (7-day TTL, baked-in snapshot fallback). Cross-references 4a's `DiscoveredModel` to pre-fill `context_window`, `max_output_tokens`, and 4c capability flags. | new `crates/ai/src/catalog/{mod,fetch,parse,cache}.rs`, new `app/src/settings_view/catalog_modal.rs`, edits to `agent_providers_widget.rs` + `ai_page.rs` | Live test of fetch + offline fallback + chip auto-fill |
| **4c. Per-model multimodal capabilities** | Wire the existing `image / pdf / audio: Option<bool>` flags (see §14) into the send path. New `crates/ai/src/capabilities.rs` resolves `Option<bool>` → `bool` per modality via Explicit user → 4b catalog → per-api_type heuristic → conservative-false. Send button blocks with inline error when the resolved capability is `false`. Adds three-state toggle chips per model row. | new `crates/ai/src/capabilities.rs`, edits to the existing pre-send gate and `agent_providers_widget.rs::render_model_row` | Live test per modality per adapter |
| **4d. Dedicated compaction model** | Optional global `agents.compaction.model: Option<LLMId>` setting (see §15). When `Some`, every compaction call dispatches to the named model regardless of conversation primary; falls back to primary on resolve failure. Reuses existing `snapshot_for_request` so cloud agent + BYOP compaction (or vice versa) works without new dispatch primitives. New "Summarization model" dropdown in Settings → AI. | edits to the compaction pipeline + `ai_page.rs` + new `SetCompactionModel` action variant | Live test with cloud agent + BYOP compaction |

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

## 13. Phase 4b — models.dev catalog + quick-add chips

**Goal:** Cross-reference the user's BYOP configuration against the open-source [models.dev](https://models.dev) catalog so users don't have to hand-fill model rows. Where Phase 4a surfaces *what's installed at your endpoint right now* via a live `/models` probe, Phase 4b surfaces *what exists in the ecosystem* via a cached catalog — and pre-populates the metadata (display name, context window, multimodal capability flags) that live `/models` responses rarely include in full.

### 13.1 Catalog cache

A new `crates/ai/src/catalog/{mod,fetch,parse,cache}.rs` owns the lifecycle. `fetch::fetch_catalog(http)` does an HTTP GET against `https://models.dev/api.json` (no auth, 10s timeout). `parse::parse_catalog(body)` is tolerant of unknown fields so upstream schema drift doesn't break the cache. `cache::CatalogCache` persists to `<config_dir>/byop_catalog.json` with a 7-day TTL. On settings-page open the cache renders immediately and a background refresh kicks off if stale, so the UI never blocks. A baked-in snapshot ships in the binary as a last-resort fallback when both cache and live fetch fail — the settings page surfaces a dim "using built-in fallback" caption.

### 13.2 UX — inline chips + Browse catalog modal

- **Inline quick-add chips.** When the user clicks "+ Add Model" on a provider card, the new empty row renders up to five catalog chips below the input, filtered to the provider's `api_type`. Each chip is a Secondary button labelled `"+ {display_name}"`; clicking auto-fills `name`, `id`, `context_window`, `max_output_tokens`, and the Phase 4c capability flags. Chips disappear the moment any field is edited.
- **Browse catalog modal.** A new "Browse catalog" button in the card footer (between Test connection and Fetch models) opens a card-style modal that mirrors 4a's `FetchedModelsModalState` pattern — rows checkable, commit appends to the provider's models list. Adds a "This provider / All providers" filter chip and a search input. MouseStateHandles live on `AgentProvidersWidget` next to 4a's row pool.

### 13.3 Filtering map

`api_type` → catalog-provider mapping (in `crates/ai/src/catalog/mod.rs`): `OpenAi → openai`, `Anthropic → anthropic`, `Gemini → google`, `DeepSeek → deepseek`, `Ollama → entries marked open_source: true`, `OpenAiResp → none` (no inline chips; falls through to the modal's "All providers" filter).

### 13.4 Cross-phase synergy

4a's `DiscoveredModel` carries only what the upstream returns. 4b cross-references entries by `(api_type, id)` and fills in `context_window`, `max_output_tokens`, and capability flags before the 4a modal rows render — opt-in via a `lookup_catalog_metadata: bool` on the resolve handler so 4a stays usable when the cache is empty.

### 13.5 Risks

1. **Upstream schema drift.** models.dev is mutable; we don't pin a snapshot. Tolerant parsing keeps a partial cache useful; baked-in snapshot covers the worst case.
2. **First-launch staleness.** A user adding their first provider before the background refresh lands sees stale-snapshot chips. Acceptable — chips are still useful and the modal exposes a manual refresh.

### 13.6 Verification gate

Unit tests on `parse_catalog` against a fixture of the live response shape; unit tests on chip-tap auto-fill and the api_type filter map; manual smoke for fresh-install fetch, stale-cache background refresh, and offline fallback.

---

## 14. Phase 4c — Per-model multimodal capabilities

**Goal:** Wire the existing `AgentProviderModel.image / pdf / audio` flags (added in Phase 1b-1) into the attachment send path so a turn carrying an attachment only targets a model that advertises support. Closes the gap where today a user can attach a PDF to a `deepseek-chat` conversation and the upstream returns HTTP 400 with no warning.

### 14.1 Data model — already in place

`AgentProviderModel` already carries `image: Option<bool>`, `pdf: Option<bool>`, `audio: Option<bool>` with three-state semantics: `Some(true)` = forced on, `Some(false)` = forced off, `None` = "Auto, inferred at runtime." Phase 4c implements the inference and the enforcement; the persistence schema doesn't change.

### 14.2 Auto inference

A new `crates/ai/src/capabilities.rs` resolves `Option<bool>` → `bool` per modality, in precedence order:

1. **Explicit user setting** — `Some(true)` / `Some(false)` short-circuits.
2. **Catalog lookup (4b)** — if `(api_type, model_id)` has a catalog entry, use its flag.
3. **Per-api_type heuristic table** — encoded constants: OpenAI's `gpt-4o*` / `gpt-4-turbo*` get image; Claude 3+ gets image+pdf; Gemini gets all three; Ollama matches against a known-multimodal allow-list (`llava-*`, `bakllava-*`, `qwen-vl-*`, …); DeepSeek none.
4. **Conservative fallback** — unresolved defaults to `false` so the user gets a clear "not supported" error rather than a silent 4xx from the upstream.

The resolver lives in `crates/ai/` (not `app/`) so dispatch can call it on the runtime path and so the heuristic table is one location to update when new families ship.

### 14.3 UX — capability toggles in the model row

Each row in `render_model_row` gains three small toggle chips next to the existing `tool_call` toggle: 🖼️ image · 📄 pdf · 🎙️ audio. Each cycles Off / Auto / On on click. The chip label shows the resolved state in dim text when "Auto" — e.g., `"🖼️ Auto (on)"` if the heuristic resolved to `true`. This makes the implicit Auto state inspectable without forcing every user to set every flag.

### 14.4 Enforcement — block + inline error

When the user attaches a file and the selected model's resolved capability for that modality is `false`, the Send button is disabled and an inline error renders next to the input — *"This model doesn't support {modality} attachments. Remove the attachment or pick a different model."* The check runs in the existing pre-send code path (same surface as today's tool-call gating in agent mode); 4c adds a per-modality branch. Dispatch downstream of the gate is unchanged — adapters that already handle multimodal continue to, and adapters that don't never see the attachment because the gate blocks it.

### 14.5 Risks

1. **Heuristic-table drift.** New multimodal model families ship faster than constants update. Mitigation: catalog (4b) takes precedence, so as long as models.dev tracks the new family the heuristic doesn't need to.
2. **False negatives on custom relays.** A user pointing `api_type: OpenAi` at a multimodal-capable relay with a non-OpenAI model id gets the conservative fallback (`false`) and can't attach until they manually flip the toggle. Documented in the chip's "Auto" tooltip.
3. **Per-modality dispatch correctness.** Each adapter's `build_chat_request` already handles content parts differently — 4c relies on per-adapter shape staying correct. Manual smoke per modality per adapter is part of the gate.

### 14.6 Verification gate

Unit tests on `capabilities::resolve` covering all three precedence levels per modality; unit tests on the heuristic table; manual smoke: attach image+pdf+audio to a known-capable model and confirm dispatch; attach to a known-incapable model and confirm the Send block fires.

---

## 15. Phase 4d — Dedicated compaction model

**Goal:** Let the user nominate a separate model for conversation compaction (summarization) so the primary agent model stays focused on agent work while a cheaper/faster model handles summarization. Common case: Claude Sonnet for the agent, Claude Haiku or a local Ollama model for compaction.

### 15.1 Setting

A new global setting at `agents.compaction.model` of type `Option<LLMId>`. `None` (the default) means *use the conversation's primary model* — preserving today's behavior. `Some(llm_id)` routes every compaction call through the named model regardless of which model the conversation itself targets. The setting accepts any `LLMId` — cloud (`claude-haiku-3`), BYOP (`byop:<uuid>:<model_id>`), or legacy `local:` — so a user with a paid cloud agent model can compact against a free local model and vice versa.

```toml
[agents.compaction]
model = "byop:abc-uuid:claude-3-5-haiku-20241022"   # or unset
```

### 15.2 Dispatch

The existing compaction pipeline takes the conversation's primary `LLMId` today. 4d adds a single read of the new setting at compaction-request time:

```rust
let target = AISettings::as_ref(ctx)
    .compaction_model
    .value()
    .clone()
    .unwrap_or_else(|| conversation.primary_llm_id());
```

Everything downstream is unchanged — `snapshot_for_request(target, …)` already routes `byop:` to the local-provider runtime and cloud ids to warp.dev (per `local_provider_config.rs`). 4d adds no new dispatch primitives.

### 15.3 UI

Settings → AI grows a new "Summarization model" dropdown under the existing "Models" group (which today exposes Base model + Coding model). The picker shows the same entries the Base/Coding dropdowns show, prepended with a "Use primary model" item that maps to `None`. The dropdown's blur/select handler dispatches a new `AISettingsPageAction::SetCompactionModel(Option<LLMId>)`.

### 15.4 Fallback

If the chosen `compaction_model` resolves to an unavailable provider at compaction time — the BYOP provider was deleted, its API key was wiped, or the network is offline — the dispatcher logs a single warning per occurrence and falls back to the conversation's primary model. Compaction never blocks the agent on this failure; the alternative (refuse to compact, conversation eventually overflows context) is a worse experience than a transparently-degraded compaction. A future polish step could surface "compaction model unavailable, using primary" inline in the conversation transcript; deferred.

### 15.5 Risks

1. **Context-window mismatch.** A compaction model with a smaller `context_window` than the primary will OOM on long conversations. Mitigation: the compaction prompt already pre-truncates input to a budget; 4d threads the compaction model's `context_window` into that budget computation in place of the primary's.
2. **Silent fallback hides config errors.** A user who set an invalid compaction model gets correct behavior (primary used) but no signal. Acceptable for first ship; future polish adds a settings-page inline warning when the configured compaction model fails to resolve at boot.
3. **Cross-provider auth.** Cloud agent + BYOP compaction (or vice versa) — keys live in separate stores. Dispatch already handles this because each `snapshot_for_request` call is self-contained.

### 15.6 Verification gate

Unit test on the dispatcher's setting read + fallback path; unit test that `snapshot_for_request` still routes `byop:` → local and cloud ids → warp.dev when called from the compaction site; manual smoke: configure two providers (one cloud, one BYOP-Ollama), set compaction to the BYOP one, send a long conversation, confirm compaction requests hit the Ollama endpoint while agent requests still hit the cloud.

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
