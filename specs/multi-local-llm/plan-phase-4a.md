# Multi-Local-LLM — Phase 4a (`/models` Fetch Button) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.
>
> **Status:** Design portion (preamble + Design refinement + File map + Risks + Next plan) is committed. The Stage A/B/C/D task breakdown is appended by `superpowers:writing-plans` after this design is approved.

**Goal:** Add a per-provider-card "Fetch models" button to the BYOP settings UI. On click, the adapter hits the upstream model-list endpoint (`/v1/models` for OpenAi / Anthropic / DeepSeek, `/api/tags` for Ollama, `/v1beta/models` for Gemini), parses the response into a list of `DiscoveredModel` records, and surfaces a modal that lets the user pick which models to add as new `AgentProviderModel` rows. **First non-protocol Phase 4 polish** — no chat-protocol changes, no `run.rs` changes, no compaction-pipeline changes. Pure UX + adapter-trait extension.

**Architecture:** Four logical stages, atomic in one PR (split into 4a-i / 4a-ii / 4a-iii / 4a-iv if review prefers):

- **Stage A (Tasks 1–5)** — Extend `ProviderAdapter` trait with `build_list_models_request` + `parse_list_models_response` returning `ListModelsPage { models: Vec<DiscoveredModel>, next_cursor: Option<String> }`. Implement for all five active adapters (OpenAi, Anthropic, Ollama, Gemini, DeepSeek) + `Err(UnsupportedApiType)` for `OpenAiResp`. Per-adapter wire types (`*ModelsListResponse` and `*ListedModel`) live in `wire.rs` alongside the existing chat-completion wire types.
- **Stage B (Task 6)** — `app/src/ai/agent_providers/fetch_models.rs` helper. Pick adapter → build request → send → parse → paginate (200-entry cap) → dedupe by `id` → return `FetchModelsOutcome::{Ok(Vec<DiscoveredModel>), Failed(String)}`. Sibling to existing `probe.rs`; wire-protocol-agnostic.
- **Stage C (Task 7)** — Six new `AISettingsPageAction` variants + handler arms. Three new fields on `AISettingsPageView` (`fetched_models_modal`, `fetch_models_in_flight`, `last_fetch_failure`). Optional `FetchModelsHook` test seam for hermetic action-handler tests.
- **Stage D (Tasks 8–9)** — Widget rendering: "Fetch models" button in card footer + modal panel. Manual smoke against five real upstreams.

**Branch:** `multi-local-llm`. Forks from `f62f0a91` (the Phase 3d code-complete-status doc commit — current tip on `origin/multi-local-llm` as of design write). Estimated ~1100 lines net code (~250 trait + per-adapter wire types, ~200 per-adapter parsers, ~150 `fetch_models.rs`, ~250 settings action handlers + page-view state, ~150 widget + modal, ~100 tests glue), ~5 hours of subagent-driven work. **Larger than Phase 3d** because it touches all five adapters (vs. 3d's one) and adds new settings UI state + a modal, but each per-adapter change is mechanical.

**Spec references:**

- `specs/multi-local-llm/design.md` §9 row "4a. /models fetch button".
- `specs/multi-local-llm/plan-phase-3d.md` §"Next plan (Phase 4a — `/models` fetch button)" — superseded by this document.
- OpenAI list-models endpoint: <https://platform.openai.com/docs/api-reference/models/list>.
- Anthropic list-models endpoint: <https://docs.anthropic.com/en/api/models-list>.
- Ollama `/api/tags`: <https://github.com/ollama/ollama/blob/main/docs/api.md#list-local-models>.
- Gemini list-models: <https://ai.google.dev/api/models#method:-models.list>.
- DeepSeek list-models (OpenAI-compat): <https://api-docs.deepseek.com/api/list-models>.

**Test gate:** All existing `cargo nextest run -p ai` tests pass (631/631 baseline at HEAD `f62f0a91`); new 4a-specific tests added (~53 = 30 per-adapter parsers + 12 `fetch_models.rs` + 10 action-handler + 1 integration). Manual smoke: per-provider Fetch button surfaces a modal of upstream models, committing ≥1 model from the modal adds the row to `settings.toml` `providers[i].models`, and the new model appears in the picker labelled `"{provider.name} / {display_name}"`.

**Out of Phase 4a (deferred to later phases):**

- **Models.dev catalog enrichment** that fills `context_window` / `max_output_tokens` / `display_name` for adapters that don't return them (OpenAi / Ollama / DeepSeek) — Phase **4b**.
- **Quick-add chips** above the models table for catalog-driven onboarding — Phase **4b**.
- **Per-model `/api/show` enrichment** on Ollama (fills `num_ctx`) — would require N follow-up HTTP requests; deferred. 4b's catalog approach handles this more efficiently.
- **Multimodal capability inference** (`image` / `pdf` / `audio` toggles) — Phase **4c**.
- **`reasoning` flag inference** — only DeepSeek's `deepseek-reasoner` cares today; defaults `false` in 4a, catalog-driven defaulting in 4b/4c.
- **`OpenAiResp` variant** — both new methods return `Err(AdapterError::UnsupportedApiType)`. Lands whenever `OpenAiResp` gets a real adapter (no scheduled phase).
- **In-modal "Refresh" affordance** — user closes and re-clicks Fetch to refresh.
- **Pagination "Load more"** — 4a truncates at 200 with a subtle caption. → 4b polish if real users hit the cap.
- **Multiple simultaneous modals** (one per provider open at once) — single-modal-replace pattern is enough.

---

## Design refinement

### Endpoint + auth (per adapter)

| Adapter | Endpoint | Auth header | Pagination | Notes |
|---|---|---|---|---|
| `OpenAi` | `GET {base_url}/v1/models` | `Authorization: Bearer {api_key}` | none | reuses existing `cfg.models_list_url()` helper |
| `Anthropic` | `GET {base_url}/v1/models` | `x-api-key: {api_key}`, `anthropic-version: 2023-06-01` | `?after_id={last_id}` when `has_more: true` | follows cursor; cap at 200 entries |
| `Ollama` | `GET {base_url}/api/tags` | none (unauth-by-default) | none | endpoint differs from chat path (`/api/chat`) |
| `Gemini` | `GET {base_url}/v1beta/models?pageSize=100` | `x-goog-api-key: {api_key}` | `?pageToken={nextPageToken}` when present | follows cursor; cap at 200 entries; default `pageSize=100` to bound round-trips |
| `DeepSeek` | `GET {base_url}/models` | `Authorization: Bearer {api_key}` | none | same wire shape as OpenAI (path matches DeepSeek's docs, NOT `/v1/models`) |
| `OpenAiResp` | — | — | — | both methods return `Err(UnsupportedApiType)` |

**Cursor injection.** Where pagination is needed (Anthropic, Gemini), `build_list_models_request` accepts an `Option<&str>` cursor parameter (default `None`). The helper loop in `fetch_models.rs` calls it once per page, passing the cursor returned by the previous `parse_list_models_response` call.

**Pre-flight: empty API key.** Adapters that need an API key (every adapter except Ollama) short-circuit in `fetch_models.rs` *before* the HTTP request when `cfg.api_key.is_empty()`. Returns `Failed("API key required")` synchronously. Matches the existing probe pre-flight.

**Timeout.** `fetch_models.rs` wraps the future in `tokio::time::timeout(Duration::from_secs(15), ...)`. Same shape as `probe.rs` but explicit (probe relies on reqwest default ~30s connect+read; we cap tighter here because parse + paginate can compound).

### Adapter trait extension

`crates/ai/src/local_provider/adapters/mod.rs`:

```rust
/// One discovered model from an upstream `/models`-style endpoint.
/// Adapters fill whatever metadata the upstream actually returned;
/// missing fields stay `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// One page of `parse_list_models_response`. For unpaginated adapters
/// (`OpenAi`, `Ollama`, `DeepSeek`) `next_cursor` is always `None`.
#[derive(Debug, Clone)]
pub struct ListModelsPage {
    pub models: Vec<DiscoveredModel>,
    pub next_cursor: Option<String>,
}

pub trait ProviderAdapter: Send + Sync {
    // ... existing methods (build_request, parse_chunk, build_probe_request, etc.) ...

    /// Build a GET request that returns the upstream model catalog.
    /// `cursor` carries the page token from the previous page (or
    /// `None` for the first page). Unpaginated adapters ignore it.
    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    /// Parse the body of a successful 2xx response from
    /// `build_list_models_request` into a `ListModelsPage`. The parser is
    /// expected to be stateless and to surface adapter-specific
    /// metadata (e.g. Gemini's `inputTokenLimit`) in the `DiscoveredModel`
    /// struct's optional fields.
    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError>;
}
```

`select_adapter` is unchanged. The existing 5-variant + 1-error match keeps its exhaustive form (no `_ =>` arm). New trait methods are filled in by each adapter; the `OpenAiResp` variant continues to surface `UnsupportedApiType` at `select_adapter`, which propagates through both old and new code paths uniformly.

### Per-adapter response parsing

#### OpenAi

Endpoint response (the documented minimum we can rely on):

```jsonc
{
  "object": "list",
  "data": [
    {"id": "gpt-4o",        "object": "model", "created": 1715367049, "owned_by": "system"},
    {"id": "gpt-4o-mini",   "object": "model", "created": 1721172741, "owned_by": "system"},
    {"id": "text-embedding-3-small", "object": "model", "created": 1705948997, "owned_by": "system"}
  ]
}
```

Parser maps each entry to `DiscoveredModel { id, display_name: None, context_window: None, max_output_tokens: None }`. **No filtering** in 4a — the picker is the user's call. (Phase 4b's catalog enrichment can add `display_name` and `context_window` after the fact; a future polish item could filter embedding-only models out of the modal.)

#### Anthropic

Endpoint response:

```jsonc
{
  "data": [
    {"type": "model", "id": "claude-opus-4-5-20251101",   "display_name": "Claude Opus 4.5",   "created_at": "2025-11-01T00:00:00Z"},
    {"type": "model", "id": "claude-sonnet-4-6-20251020", "display_name": "Claude Sonnet 4.6", "created_at": "2025-10-20T00:00:00Z"}
  ],
  "first_id": "claude-opus-4-5-20251101",
  "last_id":  "claude-sonnet-4-6-20251020",
  "has_more": false
}
```

Parser extracts `id` and `display_name`. Anthropic does **not** return `context_window` from this endpoint. `next_cursor` is `Some(last_id)` iff `has_more: true`; the `fetch_models.rs` loop appends `?after_id={cursor}` to the next request URL via `build_list_models_request(cfg, http, Some(cursor))`.

#### Ollama

Endpoint response:

```jsonc
{
  "models": [
    {
      "name": "llama3.1:latest",
      "modified_at": "2025-04-12T10:30:00Z",
      "size": 4661230977,
      "digest": "sha256:...",
      "details": {
        "format": "gguf",
        "family": "llama",
        "families": ["llama"],
        "parameter_size": "8B",
        "quantization_level": "Q4_0"
      }
    }
  ]
}
```

Parser maps `name` → `id`. Synthesizes a `display_name` from `details` when present: `"{family_capitalised} ({parameter_size})"` style (e.g. `"Llama (8B)"`). Falls back to the raw `name` if `details` is absent. **`context_window` is NOT in `/api/tags`** — it lives in `POST /api/show {"name": ...}` under `parameters.num_ctx`. 4a does not call `/api/show`; row stays `context_window = 0`. Users can set it manually after add, or wait for 4b's catalog enrichment.

#### Gemini

Endpoint response:

```jsonc
{
  "models": [
    {
      "name": "models/gemini-2.5-pro",
      "version": "2.5",
      "displayName": "Gemini 2.5 Pro",
      "description": "...",
      "inputTokenLimit": 2000000,
      "outputTokenLimit": 8192,
      "supportedGenerationMethods": ["generateContent", "streamGenerateContent"]
    },
    {
      "name": "models/embedding-001",
      "displayName": "Embedding 001",
      "inputTokenLimit": 2048,
      "outputTokenLimit": 1,
      "supportedGenerationMethods": ["embedContent"]
    }
  ],
  "nextPageToken": "abc123"
}
```

Parser:

- `name` → strip the `"models/"` prefix → `id`.
- `displayName` → `display_name`.
- `inputTokenLimit` → `context_window` (clamped to `u32::MAX`; in practice the field is always ≤ 2M).
- `outputTokenLimit` → `max_output_tokens`.
- **Filter:** drop entries whose `supportedGenerationMethods` does NOT include `"generateContent"`. Removes embedding-only / TTS-only models from the modal. Gemini commonly returns 30+ entries; this trims to ~10 useful chat models.
- `next_cursor` is `nextPageToken` when present.

#### DeepSeek

Endpoint response:

```jsonc
{
  "object": "list",
  "data": [
    {"id": "deepseek-chat",     "object": "model", "owned_by": "deepseek"},
    {"id": "deepseek-reasoner", "object": "model", "owned_by": "deepseek"}
  ]
}
```

Same wire shape as OpenAI; the parser is functionally identical but per-adapter for trait-object dispatch. Returns only `id`; `display_name` and `context_window` are `None`.

#### OpenAiResp (deferred)

Both new methods return `Err(AdapterError::UnsupportedApiType(AgentProviderApiType::OpenAiResp))`. `fetch_models.rs` surfaces this as `Failed("Fetch models not supported for this api type")`. UI disables the button when the provider's `api_type` is `OpenAiResp`. Same shape as today's probe behavior for the same variant.

### `fetch_models.rs` design

`app/src/ai/agent_providers/fetch_models.rs`. Mirrors the existing `probe.rs` shape. New file, ~150 lines. Public surface:

```rust
/// Outcome of a single `fetch_models` call. The `String` in `Failed` is
/// user-visible (first ~120 chars of the underlying reason), matching the
/// `ProbeOutcome::Failed` convention.
#[derive(Debug, Clone)]
pub enum FetchModelsOutcome {
    Ok(Vec<DiscoveredModel>),
    Failed(String),
}

/// Run the full fetch flow for one provider. Selects the adapter,
/// pre-flights the API key requirement, builds + sends the request
/// (paginating until exhausted or 200 entries hit), dedupes by `id`,
/// and returns a structured outcome. The body of any HTTP error
/// response is included in the first 120 chars of the `Failed` string.
pub async fn fetch_models(
    cfg: LocalProviderConfig,
    http: reqwest::Client,
) -> FetchModelsOutcome { /* ... */ }
```

Internals:

1. `select_adapter(cfg.api_type)?` — on `UnsupportedApiType`, return `Failed("Fetch models not supported for this api type")`.
2. **API-key pre-flight.** If `cfg.api_type != Ollama` && `cfg.api_key.is_empty()`, return `Failed("API key required")` without firing HTTP.
3. **Pagination loop.** Up to 200 entries cap; up to 10 pages cap (defensive, in case an upstream returns single-entry pages with cursors). For each iteration:
   - `adapter.build_list_models_request(&cfg, &http, cursor)?`.
   - `req.send().await` — on transport error, return `Failed("{err}")` first 120 chars.
   - On non-2xx, return `Failed("HTTP {status}: {body}")` first 120 chars.
   - `resp.text().await` — on read failure, return `Failed("{err}")`.
   - `adapter.parse_list_models_response(&body)?` — on parse failure, return `Failed("Parse error: {err}")`.
   - Append `page.models` to the running accumulator.
   - If accumulator length ≥ 200 OR `page.next_cursor.is_none()`, break (with `truncated: bool` flag if cap hit).
4. **Dedupe by `id`.** Iterate the accumulator; keep first occurrence of each `id`. Removes duplicates that can arise from paginated upstreams whose pages overlap (rare but possible).
5. **Wrap in `tokio::time::timeout(Duration::from_secs(15), ...)`** — on timeout return `Failed("Request timed out after 15s")`.
6. Return `Ok(deduped)`.

The `truncated` flag is logged via telemetry (see §Telemetry) but not surfaced through the `FetchModelsOutcome` enum directly. The modal renders the "Showing first 200" caption based on whether the returned `Vec` length equals 200 (a slight approximation — but if the upstream truly has exactly 200 models, the caption is mildly misleading; acceptable trade-off).

### UI flow

#### Card footer button

`app/src/settings_view/agent_providers_widget.rs`. The existing card footer renders `[ Test connection ]   [ Remove ]` at line ~661–680. 4a inserts a sibling between them:

```text
[ Test connection ]   [ Fetch models ]   [ Remove ]
```

Three visual states for "Fetch models", mirroring the probe button:

| State | Render | Click |
|---|---|---|
| `Idle` | "Fetch models" (default chrome) | dispatches `FetchAgentProviderModels { provider_index }` |
| `Fetching` | "Fetching…" (disabled, optional subtle spinner) | no-op |
| `Failed` | "Failed" (red, tooltip = `last_fetch_failure[index]`) | re-dispatches `FetchAgentProviderModels` (returns to `Fetching`) |

No sticky `Ok` state. The modal is the success surface; on close, the button returns to `Idle`.

**Disabled cases:** the button is rendered disabled with a tooltip when (a) the provider's `api_type` is `OpenAiResp` ("Not supported for this api type"), or (b) the provider requires an API key and the configured `api_key` is empty ("API key required"). The disabled-tooltip is computed at render time from `(api_type, api_key.is_empty())`.

#### Modal panel

A new floating panel rendered when `AISettingsPageView::fetched_models_modal: Some(_)`. Layout sketch:

```text
┌─ Fetch models — Anthropic (My Claude) ─────────────────────┐
│                                                              │
│  Found 7 models. Select which to add:                        │
│                                                              │
│  ☑ claude-opus-4-5-20251101    Claude Opus 4.5              │
│  ☑ claude-sonnet-4-6-20251020  Claude Sonnet 4.6            │
│  ☐ claude-haiku-4-5-20251001   Claude Haiku 4.5             │
│  ⊟ claude-3-5-sonnet-20241022  Claude 3.5 Sonnet (added)    │
│  ...                                                          │
│                                                              │
│  [ Select all ]  [ Select none ]              (3 selected)   │
│                                                              │
│                       [ Cancel ]   [ Add 3 models ]          │
└──────────────────────────────────────────────────────────────┘
```

Row layout: checkbox | model id (monospace) | display name (regular weight) | optional metadata in dim text (e.g. `2M ctx · 8K out` for Gemini rows).

**Already-added rows.** Computed at modal-open time: intersect `fetched[].id` with the provider's existing `provider.models[].id`. Rendered with a disabled `⊟` checkbox and a dim `"(added)"` suffix. Not toggleable; excluded from the default-checked set and from the selection counter. Prevents duplicates at commit time.

**Default selection.** All rows not already added are pre-checked. User can flip with "Select all" / "Select none" buttons (which respect the "already added" exclusion).

**Empty list.** If `fetched.is_empty()`, replace the table with "Upstream returned 0 models." Single `[ Close ]` button.

**Truncation caption.** If `fetched.len() == 200`, render a dim caption above the table: "Showing first 200 models — narrow your provider's catalog or wait for Phase 4b."

**Dismissal.** `Esc` key, the `[ Cancel ]` button, and clicking outside the modal all dispatch `CancelFetchedAgentProviderModelsModal` (close without committing). The `[ Close ]` button on the empty-state view dispatches the same.

#### `AISettingsPageAction` variants

Six new variants on `AISettingsPageAction` (in `app/src/settings_view/ai_page.rs`). Names and shapes:

```rust
/// User clicked the "Fetch models" button on a provider card.
FetchAgentProviderModels { provider_index: usize },

/// Async `fetch_models()` call resolved. Dispatched from the spawned
/// task back onto the page.
ResolveFetchAgentProviderModels {
    provider_index: usize,
    outcome: FetchModelsOutcome,
},

/// User toggled a single row in the open modal.
ToggleFetchedModelInModal { model_id: String, checked: bool },

/// User clicked "Select all" or "Select none" in the modal.
SetAllFetchedModelsChecked { checked: bool },

/// User clicked "Add N models". Reads the checked set, builds the
/// `AgentProviderModel` rows, appends to `provider.models`, closes
/// the modal, triggers settings persistence.
CommitFetchedAgentProviderModels { provider_index: usize },

/// Esc / Cancel / Close. Discards the modal without committing.
CancelFetchedAgentProviderModelsModal,
```

The handler for `CommitFetchedAgentProviderModels` reads the modal's `checked: HashSet<String>` against its `fetched: Vec<DiscoveredModel>`, builds rows via:

```rust
AgentProviderModel {
    id: d.id.clone(),
    name: d.display_name.clone().unwrap_or_else(|| d.id.clone()),
    context_window: d.context_window.unwrap_or(0),
    max_output_tokens: d.max_output_tokens.unwrap_or(0),
    reasoning: false,
    tool_call: true,
    image: None, pdf: None, audio: None,
}
```

…and pushes them onto `provider.models`. The persistence path reuses the same flow as `AddAgentProviderModel`.

#### `AISettingsPageView` new state

```rust
pub fetched_models_modal: Option<FetchedModelsModalState>,
pub fetch_models_in_flight: HashSet<usize>,
pub last_fetch_failure: HashMap<usize, String>,

pub struct FetchedModelsModalState {
    pub provider_index: usize,
    pub fetched: Vec<DiscoveredModel>,
    pub checked: HashSet<String>,        // model_id → checked
    pub already_added: HashSet<String>,  // computed at open time
}
```

Single-modal-replace: opening a modal for provider `B` while provider `A`'s modal is open overwrites `fetched_models_modal`. No "discard unsaved changes" prompt — the user can re-fetch.

**Button-state derivation.** State is computed at render time from the three fields above, not stored separately. This keeps invariants mechanical (no separate state machine to keep in sync):

- `Fetching` ⇔ `fetch_models_in_flight.contains(&provider_index)`
- `Failed` ⇔ `last_fetch_failure.contains_key(&provider_index)` AND modal is not currently open for this provider AND not currently fetching
- `Idle` otherwise

`last_fetch_failure[index]` is cleared when the user clicks the button again (starts a new fetch) or when the modal opens for that provider (successful fetch).

#### `FetchModelsHook` test seam

The action handler dispatches the async fetch via a `Arc<dyn FetchModelsHook>` trait object stored on the page view. Production binds to a `RealFetchModelsHook` that wraps `agent_providers::fetch_models::fetch_models`; tests bind to a `FakeFetchModelsHook` that returns canned outcomes. Same shape as the existing probe handler's seam (added in this phase if not already present).

```rust
#[async_trait]
pub trait FetchModelsHook: Send + Sync {
    async fn fetch(&self, cfg: LocalProviderConfig) -> FetchModelsOutcome;
}
```

### Error handling

| Cause | `FetchModelsOutcome` | UI behavior |
|---|---|---|
| `select_adapter(OpenAiResp)` → `UnsupportedApiType` | `Failed("Fetch models not supported for this api type")` | button stays in `Failed` state; modal does not open |
| Pre-flight: API key required but missing | `Failed("API key required")` — synchronous, no HTTP fired | button stays in `Failed`; modal does not open |
| Network error / DNS / connection refused | `Failed("{transport_err}")` (first 120 chars) | button in `Failed`; modal does not open |
| Request timeout (15s cap) | `Failed("Request timed out after 15s")` | button in `Failed`; modal does not open |
| HTTP 401/403 | `Failed("HTTP {status}: {body}")` (first 120 chars) | button in `Failed`; modal does not open |
| HTTP 404 | `Failed("HTTP 404")` | button in `Failed`; modal does not open |
| HTTP 5xx | `Failed("HTTP {status}: {body}")` (first 120 chars) | button in `Failed`; modal does not open |
| 2xx, body fails JSON parse | `Failed("Parse error: {serde_err}")` (first 120 chars) | button in `Failed`; modal does not open |
| 2xx, parses, **0 models returned** | `Ok(vec![])` | modal opens with empty state, `[ Close ]` only |
| 2xx, parses, ≥1 model | `Ok(Vec<DiscoveredModel>)` | modal opens normally with rows pre-checked |
| Pagination loop hits 200-entry cap | `Ok(first 200)` | modal opens with "Showing first 200" caption |

**Stale-resolve handling.** If the user removes the provider while the async fetch is in flight, the resolve dispatches `ResolveFetchAgentProviderModels { provider_index, outcome }` and the handler validates that `provider_index` still corresponds to the same provider id (stored when fetch was kicked off). If the provider is gone, the resolve is dropped silently (log line at `debug`).

**Concurrency.** Multiple fetches for different providers in flight simultaneously is allowed and tracked by the `fetch_models_in_flight: HashSet<usize>` field. Same handler pattern as today's probe.

### Telemetry

Adds one new AI-settings event, following the schema convention from `design.md §7.3`.

| Field | Type | Value |
|---|---|---|
| `event` | string | `"byop_fetch_models"` |
| `provider_api_type` | string | the enum string (`"openai" \| "anthropic" \| "ollama" \| "gemini" \| "deepseek"`) |
| `provider_id_hash` | string | `sha256(provider_id)[..8]` — same hashing as existing telemetry |
| `outcome` | string | `"ok"` or `"failed"` |
| `count` | u32 | number of models returned (0 on failure or empty success) |
| `truncated` | bool | true iff 200-entry cap hit during pagination |
| `failure_reason_code` | string \| null | one of `"unsupported_api_type"`, `"missing_api_key"`, `"network"`, `"timeout"`, `"http_4xx"`, `"http_5xx"`, `"parse_error"`; null on success |
| `commit_count` | u32 \| null | logged on `CommitFetchedAgentProviderModels`: how many models the user actually added (≤ `count`); null when only the fetch happened |

`base_url`, `api_key`, and individual `model_id` strings are **NOT** logged. `commit_count` is the key 4b-funnel signal: how many fetches actually result in adds.

---

## File map

### New files

| Path | Purpose |
|---|---|
| `app/src/ai/agent_providers/fetch_models.rs` | Wire-protocol-agnostic helper (picks adapter, pre-flights API key, builds + sends request with cursor loop, dedupes, returns `FetchModelsOutcome`). Sibling to existing `probe.rs`. |
| `app/src/ai/agent_providers/fetch_models_tests.rs` | Unit tests for the helper. Re-included via `#[cfg(test)] #[path = "fetch_models_tests.rs"] mod tests;` per repo unit-test convention. |
| `crates/ai/src/local_provider/adapters/openai_list_models_tests.rs` | Parser tests for OpenAi's `/v1/models` response (single-file adapter; sibling-test naming). Re-included from `openai.rs`. |
| `crates/ai/src/local_provider/adapters/anthropic/list_models_response_tests.rs` | Anthropic parser tests. Re-included from `anthropic/response.rs`. |
| `crates/ai/src/local_provider/adapters/ollama/list_models_response_tests.rs` | Ollama parser tests. |
| `crates/ai/src/local_provider/adapters/gemini/list_models_response_tests.rs` | Gemini parser tests. |
| `crates/ai/src/local_provider/adapters/deepseek/list_models_response_tests.rs` | DeepSeek parser tests. |

### Modified files

| Path | Change |
|---|---|
| `crates/ai/src/local_provider/adapters/mod.rs` | Add `DiscoveredModel`, `ListModelsPage`, two new trait methods (`build_list_models_request`, `parse_list_models_response`). Existing methods + `select_adapter` unchanged. |
| `crates/ai/src/local_provider/adapters/openai.rs` | Implement both new methods. New wire types: `OpenAiModelsListResponse`, `OpenAiListedModel`. Reuses `cfg.models_list_url()`. |
| `crates/ai/src/local_provider/adapters/anthropic/{mod,wire,response}.rs` | Implement both methods. New wire types: `AnthropicModelsListResponse`, `AnthropicListedModel`. Cursor handling via `has_more` + `last_id`. Append `?after_id={cursor}` to URL when paginating. |
| `crates/ai/src/local_provider/adapters/ollama/{mod,wire,response}.rs` | Implement both methods. New wire types: `OllamaTagsResponse`, `OllamaListedTag`, `OllamaTagDetails`. `display_name` synthesis from `details.family` + `details.parameter_size`. |
| `crates/ai/src/local_provider/adapters/gemini/{mod,wire,response}.rs` | Implement both methods. New wire types: `GeminiModelsListResponse`, `GeminiListedModel`. `supportedGenerationMethods` filter + `models/`-prefix strip + cursor via `nextPageToken`. |
| `crates/ai/src/local_provider/adapters/deepseek/{mod,wire,response}.rs` | Implement both methods. New wire types: `DeepSeekModelsListResponse`, `DeepSeekListedModel`. Functionally same shape as OpenAi but per-adapter for trait-object dispatch. |
| `app/src/ai/agent_providers/mod.rs` | Re-export `fetch_models` module (`pub mod fetch_models;`) so the settings page can import it. |
| `app/src/settings_view/ai_page.rs` | Add 6 new `AISettingsPageAction` variants + their handler arms. Add `fetched_models_modal`, `fetch_models_in_flight`, `last_fetch_failure` fields on `AISettingsPageView`. Add `FetchModelsHook` test seam. |
| `app/src/settings_view/agent_providers_widget.rs` | Add "Fetch models" button next to "Test connection" (around line 661). Add modal rendering block when `fetched_models_modal: Some(_)`. Wire all 6 new actions. |

### Removed files

None.

---

<!-- task-breakdown-start -->
<!-- The Stage A/B/C/D task breakdown is appended below by `superpowers:writing-plans`. -->
<!-- task-breakdown-end -->

---

## Final verification

(Filled in by `writing-plans` alongside the task breakdown. Skeleton):

- [ ] **Verification 1: Sweeps** — confirm trait methods are implemented on all 5 adapters, `select_adapter` unchanged, no churn in `run.rs` or `config.rs` beyond what 4a needs, no new feature flags introduced.
- [ ] **Verification 2: Build + tests + clippy** — `cargo build -p ai && cargo build -p warp` clean; `cargo nextest run -p ai` ~684/684 (631 baseline + 53 new); `cargo nextest run -p warp --lib` passes; `cargo clippy -p ai --all-targets --all-features -- -D warnings` clean.
- [ ] **Verification 3: Manual smoke** — 5/5 per-adapter smokes pass (per §Manual smoke gate below).
- [ ] **Verification 4: Final reviewer + push** — dispatch `oh-my-claudecode:code-reviewer` for the full Phase 4a diff. Stop before push; user reviews, then pushes manually.

### Manual smoke gate

| Provider | Endpoint | Pass criterion |
|---|---|---|
| OpenAI (real `sk-...`) | `api.openai.com/v1/models` | modal opens with ≥10 models; check 2 chat models, commit, verify they appear in the picker labelled `"{provider.name} / {id}"` |
| Anthropic (real `sk-ant-...`) | `api.anthropic.com/v1/models` | modal opens with all Anthropic models incl. `display_name`; pagination not triggered (~10 models); commit one new model |
| Ollama (local `ollama serve`) | `http://localhost:11434/api/tags` | modal opens with locally-installed models; `display_name` synthesized as `"{family} ({size})"`; commit |
| Gemini (real `AIza...`) | `generativelanguage.googleapis.com/v1beta/models` | modal opens with ≥10 `generateContent`-supporting models incl. `context_window` and `max_output_tokens` pre-filled; pagination loop fires (≥2 pages); embedding-only models filtered out; commit |
| DeepSeek (real `sk-...`) | `api.deepseek.com/models` | modal opens with `deepseek-chat` and `deepseek-reasoner`; commit one |

Pass criterion for the phase: 5/5 smokes pass; in each case the picker shows the newly-committed model and a turn can be sent against it. Same pattern as Phase 3 — when smokes land, the `🧪 code complete` row in the README's status table flips to `✅`.

---

## Risks & open questions

1. **Ollama `/api/show` deferral leaves `context_window = 0`.** Users who fetch Ollama models in 4a get rows with `context_window = 0`, which means dispatch falls back to not enforcing a local token budget (per the existing `is_zero_u32` deserialize behavior in `AgentProviderModel`). **Mitigation:** Document in the modal that "Context window not returned by Ollama; will be filled by Phase 4b's catalog." User can manually edit the row to set it.

2. **Anthropic / Gemini pagination loops forever if upstream returns a broken cursor.** **Mitigation:** Hard cap at 200 entries AND 10 pages in `fetch_models.rs`. Either limit terminates the loop with `truncated: true`. If a real user hits the cap, telemetry will surface it for 4b's "Load more" affordance.

3. **Body-size attack on `parse_list_models_response`.** A malicious or misconfigured upstream could return a multi-GB JSON response. **Mitigation:** reqwest's default response-body reads have no explicit size cap, but `resp.text().await` is awaited synchronously in the loop and will OOM cleanly under load. **Recommendation:** add an explicit `Content-Length` ceiling (e.g. 10MB per response) to the helper as a defensive guard. Document in the open-questions list; defer implementation to writing-plans if reviewer agrees.

4. **`OpenAiResp` button disabled-state messaging.** The disabled-button tooltip ("Not supported for this api type") is the *same* messaging the probe gives today. Confirm with reviewer that the consistency is desired — alternative is to hide the button entirely for `OpenAiResp` providers (cleaner UI but inconsistent with probe behavior).

5. **Default-checked all-not-already-added.** Common-case ergonomics; alternative is default-unchecked. Pre-launch UX testing on a real Anthropic / Gemini fetch should validate that "Select all" + manual unchecking is faster than "Select none" + manual checking. If reviewer prefers default-unchecked, the change is local to one line in `ResolveFetchAgentProviderModels`.

6. **Single-modal-replace pattern.** If the user fires fetch on provider 2 while provider 1's modal is open, provider 1's unsaved checks discard. **Alternative:** stack modals (one per provider open). Out of scope for 4a per §Out-of-Phase-4a. If reviewer disagrees, the change touches only the `ResolveFetchAgentProviderModels` handler (use `HashMap<usize, FetchedModelsModalState>` instead of `Option<...>`).

7. **No live test in CI.** Same gate as Phases 3a/3b/3c/3d — manual smoke against the real APIs. A future integration test using a mock HTTP server (extending `local_provider_integration.rs`) would unblock CI coverage; partially addressed by the one integration test in Stage B (§Test plan).

8. **Pagination + truncation caption approximation.** The modal shows "Showing first 200 models" when `fetched.len() == 200`. If the upstream truly has exactly 200 models, the caption is misleading. **Risk:** Very low — no real upstream returns exactly 200 chat models. Acceptable trade-off vs. piping a `truncated` bool through `FetchModelsOutcome`.

9. **`AgentProviderModel::context_window` is u32 but Gemini returns up to 2_000_000.** Fits comfortably; no clamping concern. Note for future: if a hypothetical adapter returns u64-range values, the field type needs widening or clamping logic. Not 4a's problem.

---

## Next plan (Phase 4b — models.dev catalog + quick-add chips)

After Phase 4a ships green, Phase 4b layers a models.dev catalog cache on top of the fetch flow:

- **Cached catalog** — bundle a snapshot of `models.dev`'s capability database into the repo (or fetch on first launch); cross-reference fetched `DiscoveredModel.id` strings to fill `context_window`, `max_output_tokens`, `display_name`, `reasoning`, and the multimodal capability flags that 4a left blank for OpenAi / Ollama / DeepSeek providers.
- **Quick-add chips** — render a row of chips above the models table (e.g. "GPT-4o", "Claude 3.5 Sonnet", "Llama 3.3 70B") sourced from the catalog. One click adds the chip's model with full pre-fill. Complements 4a's fetch button: catalog chips are catalog-curated; fetch results are upstream-authoritative.
- **Optional in-modal enrichment** — when 4a's modal opens, cross-reference each `DiscoveredModel.id` with the catalog and pre-fill `context_window` / `max_output_tokens` from the catalog if the upstream didn't return them. Falls back to the 4a "Showing 0" placeholders if the catalog has no entry.
- **"Load more" pagination affordance** in the modal — replaces 4a's hard 200-entry truncate with explicit user-driven cursor advance.

Plan written after Phase 4a is approved + executed.
