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

## Stage A: Adapter trait extension + per-adapter parsers

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git log --oneline -1                     # c74814b7 docs(specs/multi-local-llm): draft Phase 4a (/models fetch button) design
cargo nextest run -p ai 2>&1 | tail -3   # 631 / 631 passed (Phase 3d baseline)
```

If anything diverges (different branch, dirty tree, fewer/more tests), STOP and report.

### Task 1: Trait extension + shared types

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/mod.rs` — add `DiscoveredModel`, `ListModelsPage`, two new trait methods with default impls returning `Err(UnsupportedApiType)`.

**Why default impls and not `unimplemented!()` stubs:** the trait already uses the same precedent for `streaming_format()` (line 122-128) — adding default impls keeps the build green after this task, lets each per-adapter task override independently, and the default `Err(UnsupportedApiType)` IS the production-correct behavior for any not-yet-implemented variant (e.g. `OpenAiResp` today). Per-adapter tasks 2-6 each override BOTH methods.

**Read `crates/ai/src/local_provider/adapters/mod.rs` first** to see the existing trait shape and the `AdapterError::UnsupportedApiType(AgentProviderApiType)` variant (line 51-58).

- [ ] **Step 1.1: Add `DiscoveredModel` and `ListModelsPage` types**

Append below the `StreamingFormat` enum (around line 114), before the `pub trait ProviderAdapter` block:

```rust
/// One model discovered by `parse_list_models_response`. Adapters fill
/// whatever metadata the upstream actually returned; missing fields stay
/// `None`. Phase 4a populates rows from this struct; Phase 4b's catalog
/// fills the `None`s by cross-referencing models.dev.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// One page of `parse_list_models_response`. For unpaginated adapters
/// (`OpenAi`, `Ollama`, `DeepSeek`) `next_cursor` is always `None` and
/// the fetch-models helper exits the loop after the single page.
/// For paginated adapters (`Anthropic`, `Gemini`) `next_cursor` carries
/// the page token the caller passes back into `build_list_models_request`
/// for the next page.
#[derive(Debug, Clone)]
pub struct ListModelsPage {
    pub models: Vec<DiscoveredModel>,
    pub next_cursor: Option<String>,
}
```

- [ ] **Step 1.2: Add the two trait methods with default impls**

Inside the `pub trait ProviderAdapter` block, after `build_probe_request` (around line 169):

```rust
    /// Build the per-provider GET request that returns the upstream model
    /// catalog. `cursor` carries the page token from the previous page
    /// (or `None` for the first page). Unpaginated adapters ignore the
    /// `cursor` argument. The default impl returns `UnsupportedApiType` —
    /// adapters that support fetch override this; variants that don't
    /// (e.g. `OpenAiResp`) inherit the default.
    fn build_list_models_request(
        &self,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
        _cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        Err(AdapterError::UnsupportedApiType(self.api_type()))
    }

    /// Parse a successful 2xx body from `build_list_models_request` into
    /// a `ListModelsPage`. Stateless. The default impl returns
    /// `UnsupportedApiType` for the same reason as `build_list_models_request`.
    fn parse_list_models_response(
        &self,
        _body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        Err(AdapterError::UnsupportedApiType(self.api_type()))
    }
```

- [ ] **Step 1.3: Build + commit**

```bash
cargo build -p ai 2>&1 | tail -5         # clean
cargo nextest run -p ai 2>&1 | tail -3   # 631 / 631 still passes
cargo clippy -p ai --all-targets --all-features --tests -- -D warnings 2>&1 | tail -5
```

Expected: build clean, 631 tests still pass (the new trait methods aren't called anywhere yet, so no new tests and no regressions).

```bash
git add crates/ai/src/local_provider/adapters/mod.rs
git commit -m "feat(ai/local_provider/adapters): extend ProviderAdapter trait with list_models methods

Phase 4a stage A. Adds DiscoveredModel + ListModelsPage shared types
and two new trait methods (build_list_models_request,
parse_list_models_response) with default impls returning
UnsupportedApiType. Each per-adapter task (2-6) overrides both for
its api_type.

The default-impl pattern mirrors the existing streaming_format() shape
and serves a dual purpose: (1) it's the placeholder during incremental
per-adapter rollout, and (2) it's the production-correct behavior for
any variant without a fetch impl (e.g. OpenAiResp today). No tests yet
— first override lands in Task 2 (OpenAi)."
```

### Task 2: OpenAi list-models parser

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/openai.rs` — add `OpenAiModelsListResponse`/`OpenAiListedModel` wire types, override the two new trait methods, reuse `cfg.models_list_url()`.
- Create: `crates/ai/src/local_provider/adapters/openai_list_models_tests.rs` — parser tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/openai.rs` — the existing single-file adapter; see how `build_probe_request` reuses `cfg.models_list_url()` and how `apply_openai_headers` injects `Authorization: Bearer`.
- `crates/ai/src/local_provider/config.rs` for `LocalProviderConfig::models_list_url()`.

**Wire shape recap** (from §Design refinement / OpenAi):

```jsonc
{
  "object": "list",
  "data": [
    {"id": "gpt-4o",         "object": "model", "created": 1715367049, "owned_by": "system"},
    {"id": "gpt-4o-mini",    "object": "model", "created": 1721172741, "owned_by": "system"},
    {"id": "text-embedding-3-small", "object": "model", "created": 1705948997, "owned_by": "system"}
  ]
}
```

- [ ] **Step 2.1: Add wire types**

Append below the existing wire types in `openai.rs` (or in a clearly demarcated section if the file has none — search for `Deserialize` to find where chat wire types live and add adjacent):

```rust
#[derive(Debug, Clone, Deserialize, Default)]
struct OpenAiModelsListResponse {
    #[serde(default)] data: Vec<OpenAiListedModel>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct OpenAiListedModel {
    /// Required. OpenAI always emits this; treat missing as a parse error.
    id: String,
    // `object` / `created` / `owned_by` deliberately ignored — Phase 4a
    // doesn't surface them.
}
```

- [ ] **Step 2.2: Override `build_list_models_request`**

Inside `impl ProviderAdapter for OpenAiAdapter`, after `build_probe_request`:

```rust
    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        _cursor: Option<&str>,   // OpenAi is unpaginated; ignore
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        let url = cfg.models_list_url()?;   // same helper the probe uses
        let mut req = http.get(url);
        req = apply_openai_headers(req, cfg);   // Authorization: Bearer
        Ok(req)
    }
```

- [ ] **Step 2.3: Override `parse_list_models_response`**

Below the previous method, inside the same `impl` block:

```rust
    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        let parsed: OpenAiModelsListResponse = serde_json::from_str(body)?;
        let models = parsed
            .data
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                display_name: None,
                context_window: None,
                max_output_tokens: None,
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor: None })
    }
```

- [ ] **Step 2.4: Re-include the new test file from `openai.rs`**

At the bottom of `openai.rs`, add (just above the existing inline `#[cfg(test)] mod tests` block if there is one; otherwise at EOF):

```rust
#[cfg(test)]
#[path = "openai_list_models_tests.rs"]
mod list_models_tests;
```

- [ ] **Step 2.5: Write `openai_list_models_tests.rs` (6 tests)**

```rust
//! Phase 4a parser tests for `OpenAiAdapter::parse_list_models_response`.
//! Fixtures match the documented `/v1/models` response shape.

use super::{ListModelsPage, OpenAiAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> OpenAiAdapter { OpenAiAdapter }

#[test]
fn parses_happy_path_three_models() {
    let body = r#"{
        "object": "list",
        "data": [
            {"id": "gpt-4o",       "object": "model", "created": 1, "owned_by": "system"},
            {"id": "gpt-4o-mini",  "object": "model", "created": 2, "owned_by": "system"},
            {"id": "text-embedding-3-small", "object": "model", "created": 3, "owned_by": "system"}
        ]
    }"#;
    let ListModelsPage { models, next_cursor } = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(next_cursor, None);
    assert_eq!(models.len(), 3);
    assert_eq!(models[0], DiscoveredModel { id: "gpt-4o".into(),       display_name: None, context_window: None, max_output_tokens: None });
    assert_eq!(models[1], DiscoveredModel { id: "gpt-4o-mini".into(),  display_name: None, context_window: None, max_output_tokens: None });
    assert_eq!(models[2].id, "text-embedding-3-small");
}

#[test]
fn parses_empty_data_array() {
    let body = r#"{"object": "list", "data": []}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"object": "list", "data": ["#;   // truncated
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)), "got {err:?}");
}

#[test]
fn errors_on_row_missing_id() {
    let body = r#"{"data": [{"object": "model", "created": 1, "owned_by": "system"}]}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)), "got {err:?}");
}

#[test]
fn ignores_unknown_top_level_fields() {
    // Defensive: future-proofing against OpenAI adding fields we don't model.
    let body = r#"{"object": "list", "data": [{"id": "gpt-4o"}],
                   "future_field": {"nested": "value"}, "another_field": 42}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.models[0].id, "gpt-4o");
}

#[test]
fn next_cursor_always_none_for_openai() {
    // OpenAi is unpaginated. Even if a hypothetical OpenAI-compat upstream
    // returned a `next_cursor`, the parser correctly returns None because
    // it never reads that field.
    let body = r#"{"data": [{"id": "gpt-4o"}], "next_cursor": "ignored"}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.next_cursor.is_none());
}
```

- [ ] **Step 2.6: Run tests + commit**

```bash
cargo nextest run -p ai openai_list_models 2>&1 | tail -10
```

Expected: 6 / 6 passed.

```bash
cargo nextest run -p ai 2>&1 | tail -3           # 631 + 6 = 637
cargo clippy -p ai --all-targets --all-features --tests -- -D warnings 2>&1 | tail -5
```

```bash
git add crates/ai/src/local_provider/adapters/openai.rs crates/ai/src/local_provider/adapters/openai_list_models_tests.rs
git commit -m "feat(ai/local_provider/adapters/openai): list-models parser

Phase 4a stage A. Adds OpenAi's override of build_list_models_request
(GET /v1/models, reuses cfg.models_list_url() and apply_openai_headers)
and parse_list_models_response (deserialize {data: [{id}]} into
ListModelsPage). Each row becomes DiscoveredModel { id, None, None,
None } — OpenAI returns only id; context_window and display_name will
be filled in Phase 4b's catalog enrichment.

6 parser tests cover: happy path, empty data array, malformed JSON,
missing required id field, unknown forward-compat fields, and next_cursor
always-None invariant."
```

### Task 3: Anthropic list-models parser

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/anthropic/wire.rs` — add `AnthropicModelsListResponse`/`AnthropicListedModel` wire types.
- Modify: `crates/ai/src/local_provider/adapters/anthropic/mod.rs` — override the two trait methods on `AnthropicAdapter`; reuse the existing `apply_anthropic_headers` helper for `x-api-key` + `anthropic-version`.
- Create: `crates/ai/src/local_provider/adapters/anthropic/list_models_response_tests.rs` — parser tests.
- Modify: `crates/ai/src/local_provider/adapters/anthropic/response.rs` (or the most appropriate sibling) to re-include the new tests file.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/anthropic/wire.rs` — existing wire-type pattern.
- `crates/ai/src/local_provider/adapters/anthropic/mod.rs` — see how `build_probe_request` formats the `/v1/models` URL and applies headers.

**Wire shape recap** (from §Design refinement / Anthropic):

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

- [ ] **Step 3.1: Add wire types in `anthropic/wire.rs`**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct AnthropicModelsListResponse {
    #[serde(default)] pub data: Vec<AnthropicListedModel>,
    #[serde(default)] pub has_more: bool,
    #[serde(default)] pub last_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct AnthropicListedModel {
    pub id: String,
    #[serde(default)] pub display_name: Option<String>,
    // `type`, `created_at` ignored — Phase 4a doesn't surface them.
}
```

- [ ] **Step 3.2: Override `build_list_models_request` (with cursor support)**

In `anthropic/mod.rs`, inside `impl ProviderAdapter for AnthropicAdapter`:

```rust
    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        // Anthropic's list-models endpoint is GET /v1/models — same path
        // the probe uses. With cursor: append `?after_id={cursor}`.
        // Use `?limit=100` to bound per-page round-trips (default 20).
        let mut url = cfg.models_list_url()?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("limit", "100");
            if let Some(c) = cursor {
                q.append_pair("after_id", c);
            }
        }
        let mut req = http.get(url);
        req = apply_anthropic_headers(req, cfg);
        Ok(req)
    }
```

- [ ] **Step 3.3: Override `parse_list_models_response`**

```rust
    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        use crate::local_provider::adapters::anthropic::wire::AnthropicModelsListResponse;
        let parsed: AnthropicModelsListResponse = serde_json::from_str(body)?;
        let next_cursor = if parsed.has_more { parsed.last_id } else { None };
        let models = parsed
            .data
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                display_name: m.display_name,
                context_window: None,        // not in the response
                max_output_tokens: None,     // not in the response
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor })
    }
```

- [ ] **Step 3.4: Re-include the test file**

In `anthropic/response.rs` (or whichever sibling makes most sense — check the existing test-file re-include pattern in `anthropic/mod.rs`), add at the bottom:

```rust
#[cfg(test)]
#[path = "list_models_response_tests.rs"]
mod list_models_tests;
```

If `anthropic/mod.rs` is the canonical re-include site (matching the existing `#[cfg(test)] #[path = "response_tests.rs"] mod response_tests;` if present), put it there instead. Confirm by inspecting the existing re-include before adding.

- [ ] **Step 3.5: Write `anthropic/list_models_response_tests.rs` (8 tests)**

```rust
//! Phase 4a parser tests for `AnthropicAdapter::parse_list_models_response`.
//! Fixtures match the documented Anthropic `/v1/models` response shape.

use super::{AnthropicAdapter, ListModelsPage, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> AnthropicAdapter { AnthropicAdapter }

#[test]
fn parses_happy_path_with_display_name() {
    let body = r#"{
        "data": [
            {"type": "model", "id": "claude-opus-4-5-20251101",   "display_name": "Claude Opus 4.5",   "created_at": "2025-11-01T00:00:00Z"},
            {"type": "model", "id": "claude-sonnet-4-6-20251020", "display_name": "Claude Sonnet 4.6", "created_at": "2025-10-20T00:00:00Z"}
        ],
        "first_id": "claude-opus-4-5-20251101",
        "last_id":  "claude-sonnet-4-6-20251020",
        "has_more": false
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 2);
    assert_eq!(page.models[0], DiscoveredModel {
        id: "claude-opus-4-5-20251101".into(),
        display_name: Some("Claude Opus 4.5".into()),
        context_window: None,
        max_output_tokens: None,
    });
    assert_eq!(page.models[1].display_name, Some("Claude Sonnet 4.6".into()));
}

#[test]
fn parses_empty_data_array() {
    let body = r#"{"data": [], "has_more": false, "first_id": null, "last_id": null}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"data": ["#;   // truncated
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)), "got {err:?}");
}

#[test]
fn errors_on_row_missing_id() {
    let body = r#"{"data": [{"type": "model", "display_name": "X"}], "has_more": false}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)), "got {err:?}");
}

#[test]
fn surfaces_next_cursor_when_has_more_true() {
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1", "display_name": "Claude 1"}],
        "first_id": "claude-1", "last_id": "claude-1",
        "has_more": true
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("claude-1"));
}

#[test]
fn next_cursor_none_when_has_more_false_even_with_last_id() {
    // Anthropic does send last_id on the final page too; we must ignore it.
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1", "display_name": "Claude 1"}],
        "first_id": "claude-1", "last_id": "claude-1",
        "has_more": false
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.next_cursor.is_none(), "had {:?}", page.next_cursor);
}

#[test]
fn missing_display_name_yields_none() {
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1"}],
        "has_more": false
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "claude-1");
    assert!(page.models[0].display_name.is_none());
}

#[test]
fn ignores_unknown_top_level_fields() {
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1", "display_name": "Claude 1"}],
        "has_more": false,
        "future_field": {"nested": true}
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models.len(), 1);
}
```

- [ ] **Step 3.6: Run tests + commit**

```bash
cargo nextest run -p ai anthropic::list_models 2>&1 | tail -10   # 8 / 8 passed
cargo nextest run -p ai 2>&1 | tail -3                            # 637 + 8 = 645
cargo clippy -p ai --all-targets --all-features --tests -- -D warnings 2>&1 | tail -5
```

```bash
git add crates/ai/src/local_provider/adapters/anthropic/
git commit -m "feat(ai/local_provider/adapters/anthropic): list-models parser

Phase 4a stage A. Adds Anthropic's override of build_list_models_request
(GET /v1/models with ?limit=100 and ?after_id={cursor} for pagination,
reuses cfg.models_list_url() and apply_anthropic_headers) and
parse_list_models_response (deserialize {data: [{id, display_name}],
has_more, last_id} into ListModelsPage; next_cursor=Some(last_id) iff
has_more=true).

8 parser tests cover: happy path with display_name, empty array,
malformed JSON, missing required id, next_cursor when has_more=true,
next_cursor=None when has_more=false even if last_id present
(important — Anthropic emits last_id on the final page too), missing
display_name yields None, and forward-compat unknown fields ignored."
```

### Task 4: Ollama list-models parser

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/ollama/wire.rs` — add `OllamaTagsResponse`/`OllamaListedTag`/`OllamaTagDetails` wire types.
- Modify: `crates/ai/src/local_provider/adapters/ollama/mod.rs` — override the two trait methods on `OllamaAdapter`.
- Create: `crates/ai/src/local_provider/adapters/ollama/list_models_response_tests.rs` — parser tests.
- Modify: appropriate sibling to re-include the test file.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/ollama/wire.rs` — existing wire-type pattern.
- `crates/ai/src/local_provider/adapters/ollama/mod.rs` — see how `build_probe_request` builds the `/api/tags` URL (no auth header).

**Wire shape recap** (from §Design refinement / Ollama):

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

- [ ] **Step 4.1: Add wire types in `ollama/wire.rs`**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct OllamaTagsResponse {
    #[serde(default)] pub models: Vec<OllamaListedTag>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct OllamaListedTag {
    pub name: String,
    #[serde(default)] pub details: Option<OllamaTagDetails>,
    // `modified_at`, `size`, `digest` ignored.
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct OllamaTagDetails {
    #[serde(default)] pub family: Option<String>,
    #[serde(default)] pub parameter_size: Option<String>,
    // `format`, `families`, `quantization_level` ignored.
}
```

- [ ] **Step 4.2: Override `build_list_models_request`**

In `ollama/mod.rs`, inside `impl ProviderAdapter for OllamaAdapter`. Ollama is unpaginated and unauth-by-default:

```rust
    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        _cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        // Ollama list-models endpoint is GET {base_url}/api/tags.
        // Path differs from chat (/api/chat) and probe already targets
        // /api/tags; reuse the same URL helper if one exists, or build
        // inline. NO auth headers — Ollama is unauthenticated by default.
        let url = cfg.ollama_tags_url()?;   // (add this helper to config.rs
                                            //  if it doesn't exist — see Step 4.3)
        Ok(http.get(url))
    }
```

**If `cfg.ollama_tags_url()` doesn't exist:** add it to `crates/ai/src/local_provider/config.rs` alongside the existing `models_list_url`. It should join `{base_url}/api/tags` correctly regardless of whether `base_url` has a trailing slash. Check what the existing `build_probe_request` for Ollama does — it likely already builds this URL inline or via a helper. **Reuse whatever Ollama's probe already does** rather than introducing a duplicate.

- [ ] **Step 4.3: Override `parse_list_models_response`**

```rust
    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        use crate::local_provider::adapters::ollama::wire::OllamaTagsResponse;
        let parsed: OllamaTagsResponse = serde_json::from_str(body)?;
        let models = parsed
            .models
            .into_iter()
            .map(|m| DiscoveredModel {
                display_name: synthesize_display_name(&m),
                id: m.name,
                context_window: None,        // not in /api/tags response
                max_output_tokens: None,     // not in /api/tags response
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor: None })
    }
```

Add the helper at module-private scope in the same file:

```rust
/// Synthesize a `display_name` for an Ollama row from its `details`. Returns
/// e.g. `"Llama (8B)"` when both `family` and `parameter_size` are present,
/// or `Some("Llama")` / `Some("8B")` if only one is present, or `None` when
/// `details` is absent or empty.
fn synthesize_display_name(m: &super::wire::OllamaListedTag) -> Option<String> {
    let details = m.details.as_ref()?;
    let family = details.family.as_deref().filter(|s| !s.is_empty());
    let size   = details.parameter_size.as_deref().filter(|s| !s.is_empty());
    match (family, size) {
        (Some(f), Some(s)) => Some(format!("{} ({s})", capitalize_first(f))),
        (Some(f), None)    => Some(capitalize_first(f)),
        (None,    Some(s)) => Some(s.to_string()),
        (None,    None)    => None,
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None    => String::new(),
    }
}
```

- [ ] **Step 4.4: Re-include the test file**

Same pattern as Task 3.4 — add `#[cfg(test)] #[path = "list_models_response_tests.rs"] mod list_models_tests;` to the canonical re-include site in `ollama/mod.rs` or `ollama/response.rs` (check existing `response_tests` re-include for placement).

- [ ] **Step 4.5: Write `ollama/list_models_response_tests.rs` (7 tests)**

```rust
//! Phase 4a parser tests for `OllamaAdapter::parse_list_models_response`.
//! Fixtures match the documented `/api/tags` response shape.

use super::{ListModelsPage, OllamaAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> OllamaAdapter { OllamaAdapter }

#[test]
fn parses_happy_path_with_details() {
    let body = r#"{
        "models": [
            {"name": "llama3.1:latest",
             "modified_at": "2025-04-12T10:30:00Z",
             "size": 4661230977,
             "digest": "sha256:abc",
             "details": {"format": "gguf", "family": "llama",
                         "families": ["llama"],
                         "parameter_size": "8B", "quantization_level": "Q4_0"}}
        ]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.models[0], DiscoveredModel {
        id: "llama3.1:latest".into(),
        display_name: Some("Llama (8B)".into()),
        context_window: None,
        max_output_tokens: None,
    });
}

#[test]
fn parses_row_without_details_block() {
    let body = r#"{"models": [{"name": "custom:v1"}]}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "custom:v1");
    assert_eq!(page.models[0].display_name, None);
}

#[test]
fn parses_details_with_only_family() {
    let body = r#"{
        "models": [{"name": "x:v1",
                    "details": {"family": "mistral"}}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].display_name, Some("Mistral".into()));
}

#[test]
fn parses_details_with_only_parameter_size() {
    let body = r#"{
        "models": [{"name": "x:v1",
                    "details": {"parameter_size": "70B"}}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].display_name, Some("70B".into()));
}

#[test]
fn parses_empty_models_array() {
    let body = r#"{"models": []}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"models": ["#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}

#[test]
fn errors_on_row_missing_name() {
    let body = r#"{"models": [{"size": 100}]}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}
```

- [ ] **Step 4.6: Run tests + commit**

```bash
cargo nextest run -p ai ollama::list_models 2>&1 | tail -10   # 7 / 7 passed
cargo nextest run -p ai 2>&1 | tail -3                         # 645 + 7 = 652
cargo clippy -p ai --all-targets --all-features --tests -- -D warnings 2>&1 | tail -5
```

```bash
git add crates/ai/src/local_provider/adapters/ollama/
git commit -m "feat(ai/local_provider/adapters/ollama): list-models parser

Phase 4a stage A. Adds Ollama's override of build_list_models_request
(GET /api/tags, no auth — Ollama is unauthenticated by default) and
parse_list_models_response (deserialize {models: [{name, details}]}
into ListModelsPage with display_name synthesized from details.family
and details.parameter_size, e.g. \"Llama (8B)\").

context_window is NOT in /api/tags (it lives in POST /api/show under
parameters.num_ctx, which 4a does NOT call to avoid N follow-up
requests); rows ship with context_window=None and Phase 4b's catalog
will fill them.

7 parser tests cover: happy path with full details, no details block,
family-only, parameter_size-only, empty array, malformed JSON, and
missing required name field."
```

### Task 5: Gemini list-models parser

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/gemini/wire.rs` — add `GeminiModelsListResponse`/`GeminiListedModel` wire types.
- Modify: `crates/ai/src/local_provider/adapters/gemini/mod.rs` — override the two trait methods on `GeminiAdapter` (most complex of the five: filter, prefix-strip, pagination).
- Create: `crates/ai/src/local_provider/adapters/gemini/list_models_response_tests.rs` — parser tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/gemini/wire.rs` — existing wire-type pattern.
- `crates/ai/src/local_provider/adapters/gemini/mod.rs` — see how `build_probe_request` formats the `/v1beta/models` URL and uses `x-goog-api-key`.

**Wire shape recap** (from §Design refinement / Gemini):

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
      "inputTokenLimit": 2048, "outputTokenLimit": 1,
      "supportedGenerationMethods": ["embedContent"]
    }
  ],
  "nextPageToken": "abc123"
}
```

- [ ] **Step 5.1: Add wire types in `gemini/wire.rs`**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct GeminiModelsListResponse {
    #[serde(default)] pub models: Vec<GeminiListedModel>,
    #[serde(default, rename = "nextPageToken")] pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct GeminiListedModel {
    /// Full name including `"models/"` prefix; the parser strips it.
    pub name: String,
    #[serde(default, rename = "displayName")] pub display_name: Option<String>,
    #[serde(default, rename = "inputTokenLimit")] pub input_token_limit: Option<u64>,
    #[serde(default, rename = "outputTokenLimit")] pub output_token_limit: Option<u64>,
    #[serde(default, rename = "supportedGenerationMethods")] pub supported_generation_methods: Vec<String>,
    // `version`, `description` ignored.
}
```

- [ ] **Step 5.2: Override `build_list_models_request` (with cursor + pageSize)**

```rust
    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        // Gemini list-models endpoint is GET /v1beta/models. We always
        // pass ?pageSize=100 to bound per-page round-trips (default 50).
        // Cursor is `pageToken` per Gemini's docs.
        let mut url = cfg.gemini_models_list_url()?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("pageSize", "100");
            if let Some(c) = cursor {
                q.append_pair("pageToken", c);
            }
        }
        let mut req = http.get(url);
        req = apply_gemini_headers(req, cfg);   // x-goog-api-key
        Ok(req)
    }
```

If `cfg.gemini_models_list_url()` doesn't exist, reuse whatever URL the existing `build_probe_request` uses for Gemini (the probe already targets `/v1beta/models`).

- [ ] **Step 5.3: Override `parse_list_models_response`**

```rust
    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        use crate::local_provider::adapters::gemini::wire::GeminiModelsListResponse;
        let parsed: GeminiModelsListResponse = serde_json::from_str(body)?;
        let models: Vec<DiscoveredModel> = parsed
            .models
            .into_iter()
            // Filter: drop entries that don't support `generateContent`.
            // Removes embedding-only and TTS-only models.
            .filter(|m| {
                m.supported_generation_methods
                    .iter()
                    .any(|s| s == "generateContent")
            })
            .map(|m| {
                // Strip the "models/" prefix from `name`.
                let id = m.name.strip_prefix("models/").unwrap_or(&m.name).to_string();
                DiscoveredModel {
                    id,
                    display_name: m.display_name,
                    context_window: m.input_token_limit.map(|n| n.min(u32::MAX as u64) as u32),
                    max_output_tokens: m.output_token_limit.map(|n| n.min(u32::MAX as u64) as u32),
                }
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor: parsed.next_page_token })
    }
```

- [ ] **Step 5.4: Re-include the test file**

```rust
#[cfg(test)]
#[path = "list_models_response_tests.rs"]
mod list_models_tests;
```

Placed alongside the existing Gemini `response_tests` re-include.

- [ ] **Step 5.5: Write `gemini/list_models_response_tests.rs` (9 tests)**

```rust
//! Phase 4a parser tests for `GeminiAdapter::parse_list_models_response`.
//! Fixtures match the documented `/v1beta/models` response shape.

use super::{GeminiAdapter, ListModelsPage, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> GeminiAdapter { GeminiAdapter }

#[test]
fn parses_happy_path_with_all_metadata() {
    let body = r#"{
        "models": [
            {"name": "models/gemini-2.5-pro",
             "displayName": "Gemini 2.5 Pro",
             "inputTokenLimit": 2000000,
             "outputTokenLimit": 8192,
             "supportedGenerationMethods": ["generateContent", "streamGenerateContent"]}
        ]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.models[0], DiscoveredModel {
        id: "gemini-2.5-pro".into(),    // prefix stripped
        display_name: Some("Gemini 2.5 Pro".into()),
        context_window: Some(2_000_000),
        max_output_tokens: Some(8192),
    });
}

#[test]
fn filters_out_models_without_generateContent_method() {
    let body = r#"{
        "models": [
            {"name": "models/gemini-2.5-pro", "displayName": "Pro",
             "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/embedding-001", "displayName": "Embedding",
             "inputTokenLimit": 2048, "outputTokenLimit": 1,
             "supportedGenerationMethods": ["embedContent"]},
            {"name": "models/text-to-speech-1", "displayName": "TTS",
             "supportedGenerationMethods": ["generateSpeech"]}
        ]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.models[0].id, "gemini-2.5-pro");
}

#[test]
fn surfaces_next_page_token() {
    let body = r#"{
        "models": [{"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]}],
        "nextPageToken": "abc123"
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("abc123"));
}

#[test]
fn no_next_page_token_yields_none_cursor() {
    let body = r#"{
        "models": [{"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.next_cursor.is_none());
}

#[test]
fn strips_models_prefix() {
    let body = r#"{
        "models": [{"name": "models/gemini-2.5-pro", "supportedGenerationMethods": ["generateContent"]}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "gemini-2.5-pro");
}

#[test]
fn does_not_strip_when_no_models_prefix() {
    // Defensive: if Gemini ever changes the format, we keep the raw name.
    let body = r#"{
        "models": [{"name": "raw-gemini-x", "supportedGenerationMethods": ["generateContent"]}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "raw-gemini-x");
}

#[test]
fn missing_display_name_and_limits_yield_none() {
    let body = r#"{
        "models": [{"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].display_name, None);
    assert_eq!(page.models[0].context_window, None);
    assert_eq!(page.models[0].max_output_tokens, None);
}

#[test]
fn parses_empty_models_array() {
    let body = r#"{"models": []}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
    assert!(page.next_cursor.is_none());
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"models": ["#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}
```

- [ ] **Step 5.6: Run tests + commit**

```bash
cargo nextest run -p ai gemini::list_models 2>&1 | tail -10   # 9 / 9 passed
cargo nextest run -p ai 2>&1 | tail -3                         # 652 + 9 = 661
cargo clippy -p ai --all-targets --all-features --tests -- -D warnings 2>&1 | tail -5
```

```bash
git add crates/ai/src/local_provider/adapters/gemini/
git commit -m "feat(ai/local_provider/adapters/gemini): list-models parser

Phase 4a stage A. Adds Gemini's override of build_list_models_request
(GET /v1beta/models with ?pageSize=100 and ?pageToken={cursor} for
pagination, reuses cfg.gemini_models_list_url() and apply_gemini_headers
which sets x-goog-api-key) and parse_list_models_response (deserialize
{models: [{name, displayName, inputTokenLimit, outputTokenLimit,
supportedGenerationMethods}], nextPageToken} into ListModelsPage).

Three Gemini-specific behaviors: (1) filter out entries that don't
list generateContent in supportedGenerationMethods, removing embedding-
only / TTS-only models; (2) strip the leading 'models/' prefix from
each name to get the bare model id; (3) clamp inputTokenLimit /
outputTokenLimit u64 values to u32 (in practice both fit comfortably).

9 parser tests cover: happy path with all metadata, filter drops
embedding/TTS models, next_page_token surfaces as cursor, no-token
yields None, prefix stripped, no-prefix-no-strip defensive case,
missing optional fields yield None, empty models array, and malformed
JSON."
```

### Task 6: DeepSeek list-models parser

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/deepseek/wire.rs` — add `DeepSeekModelsListResponse`/`DeepSeekListedModel` wire types.
- Modify: `crates/ai/src/local_provider/adapters/deepseek/mod.rs` — override the two trait methods on `DeepSeekAdapter`.
- Create: `crates/ai/src/local_provider/adapters/deepseek/list_models_response_tests.rs` — parser tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/deepseek/wire.rs` — existing wire-type pattern.
- `crates/ai/src/local_provider/adapters/deepseek/mod.rs` — see how `build_probe_request` already targets `{base_url}/models` (DeepSeek's path, NOT `/v1/models`).

**Wire shape recap** (from §Design refinement / DeepSeek — identical to OpenAI):

```jsonc
{
  "object": "list",
  "data": [
    {"id": "deepseek-chat",     "object": "model", "owned_by": "deepseek"},
    {"id": "deepseek-reasoner", "object": "model", "owned_by": "deepseek"}
  ]
}
```

- [ ] **Step 6.1: Add wire types in `deepseek/wire.rs`**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct DeepSeekModelsListResponse {
    #[serde(default)] pub data: Vec<DeepSeekListedModel>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct DeepSeekListedModel {
    pub id: String,
    // `object`, `owned_by` ignored.
}
```

- [ ] **Step 6.2: Override `build_list_models_request`**

In `deepseek/mod.rs`, inside `impl ProviderAdapter for DeepSeekAdapter`. DeepSeek's endpoint is `GET {base_url}/models` (note: NOT `/v1/models` — DeepSeek's docs use the no-`/v1` form, though `/v1` also works). Reuse whatever URL the existing `build_probe_request` already uses for DeepSeek; the probe already hits the right path.

```rust
    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        _cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        // DeepSeek reuses OpenAI's URL helpers — its endpoint is at
        // {base_url}/models. The existing `cfg.models_list_url()` joins
        // to "/models" or "/v1/models" depending on how base_url is
        // configured; matches the probe.
        let url = cfg.models_list_url()?;
        let mut req = http.get(url);
        req = apply_deepseek_headers(req, cfg);   // Bearer auth
        Ok(req)
    }
```

If the existing DeepSeek probe uses a DeepSeek-specific URL helper (e.g. `cfg.deepseek_models_list_url()`), use that. Pattern-match whatever the probe does so the fetch hits the same endpoint.

- [ ] **Step 6.3: Override `parse_list_models_response`**

```rust
    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        use crate::local_provider::adapters::deepseek::wire::DeepSeekModelsListResponse;
        let parsed: DeepSeekModelsListResponse = serde_json::from_str(body)?;
        let models = parsed
            .data
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                display_name: None,         // DeepSeek doesn't return display_name
                context_window: None,
                max_output_tokens: None,
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor: None })
    }
```

- [ ] **Step 6.4: Re-include the test file**

```rust
#[cfg(test)]
#[path = "list_models_response_tests.rs"]
mod list_models_tests;
```

Placed alongside the existing DeepSeek `response_tests` re-include.

- [ ] **Step 6.5: Write `deepseek/list_models_response_tests.rs` (5 tests)**

```rust
//! Phase 4a parser tests for `DeepSeekAdapter::parse_list_models_response`.
//! Fixtures match the documented DeepSeek `/models` response shape (OpenAI-compatible).

use super::{DeepSeekAdapter, ListModelsPage, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> DeepSeekAdapter { DeepSeekAdapter }

#[test]
fn parses_happy_path_two_models() {
    let body = r#"{
        "object": "list",
        "data": [
            {"id": "deepseek-chat",     "object": "model", "owned_by": "deepseek"},
            {"id": "deepseek-reasoner", "object": "model", "owned_by": "deepseek"}
        ]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 2);
    assert_eq!(page.models[0], DiscoveredModel {
        id: "deepseek-chat".into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
    });
    assert_eq!(page.models[1].id, "deepseek-reasoner");
}

#[test]
fn parses_empty_data_array() {
    let body = r#"{"object": "list", "data": []}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"data": ["#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}

#[test]
fn errors_on_row_missing_id() {
    let body = r#"{"data": [{"object": "model", "owned_by": "deepseek"}]}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}

#[test]
fn ignores_unknown_top_level_fields() {
    let body = r#"{"data": [{"id": "deepseek-chat"}], "future_field": 1}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "deepseek-chat");
}
```

- [ ] **Step 6.6: Run tests + commit**

```bash
cargo nextest run -p ai deepseek::list_models 2>&1 | tail -10   # 5 / 5 passed
cargo nextest run -p ai 2>&1 | tail -3                           # 661 + 5 = 666
cargo clippy -p ai --all-targets --all-features --tests -- -D warnings 2>&1 | tail -5
```

```bash
git add crates/ai/src/local_provider/adapters/deepseek/
git commit -m "feat(ai/local_provider/adapters/deepseek): list-models parser

Phase 4a stage A. Adds DeepSeek's override of build_list_models_request
(GET {base_url}/models — note: DeepSeek's docs use the no-/v1 form,
matching the existing probe path; reuses cfg.models_list_url() and
apply_deepseek_headers which sets Authorization: Bearer) and
parse_list_models_response (deserialize {data: [{id}]} into
ListModelsPage with all metadata fields None — DeepSeek returns only
id, matching OpenAI's shape).

This completes Stage A (all 5 active adapters now support
fetch_models); OpenAiResp continues to inherit the trait's default
Err(UnsupportedApiType) impl.

5 parser tests cover: happy path, empty data, malformed JSON,
missing required id, and forward-compat unknown fields ignored."
```

---

## Stage B: Fetch helper

### Task 7: `fetch_models.rs` helper + tests

**Files:**
- Create: `app/src/ai/agent_providers/fetch_models.rs`.
- Create: `app/src/ai/agent_providers/fetch_models_tests.rs`.
- Modify: `app/src/ai/agent_providers/mod.rs` — `pub mod fetch_models;` so the settings page can import it.

**Read these reference files FIRST:**
- `app/src/ai/agent_providers/probe.rs` — the sibling shape this file mirrors. Same: pick adapter → pre-flight → build → send → parse → return structured outcome.
- `app/src/ai/agent_providers/mod.rs` — confirms the existing `pub mod probe;` declaration, where the new `pub mod fetch_models;` belongs.
- `crates/ai/tests/local_provider_integration.rs` — for the mock-server pattern used in Step 7.4.

- [ ] **Step 7.1: Create `fetch_models.rs` with the helper**

```rust
//! Per-provider model-list discovery used by the "Fetch models" button in
//! `AgentProvidersWidget` (Phase 4a). Each call selects an adapter for the
//! provider's `api_type`, pre-flights the API-key requirement, builds and
//! sends the model-list request (paginating until exhausted or a 200-entry
//! cap is hit), dedupes by `id`, and returns a structured outcome.
//!
//! The helper is wire-protocol-agnostic — new adapters get fetch support
//! automatically as soon as their `build_list_models_request` /
//! `parse_list_models_response` overrides return something other than
//! `Err(UnsupportedApiType(...))`.

use std::time::Duration;

use ai::local_provider::{
    api_type::AgentProviderApiType,
    adapters::{DiscoveredModel, ListModelsPage},
    config::LocalProviderConfig,
    select_adapter,
    ProviderAdapterError as AdapterError,
};

/// Hard caps for the pagination loop. The entry cap bounds the modal
/// size; the page cap bounds the time spent on a misbehaving cursor.
/// `MAX_ENTRIES` is `pub` so the settings handler can use it to flag
/// `truncated: true` in telemetry without duplicating the constant.
pub const MAX_ENTRIES: usize = 200;
const MAX_PAGES:   usize = 10;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome of a single `fetch_models` call. `Failed` carries a one-line
/// user-visible reason (first ~120 chars), matching `ProbeOutcome::Failed`.
#[derive(Debug, Clone)]
pub enum FetchModelsOutcome {
    Ok(Vec<DiscoveredModel>),
    Failed(String),
}

impl FetchModelsOutcome {
    pub fn is_ok(&self) -> bool { matches!(self, Self::Ok(_)) }
}

/// Run the full fetch flow for one provider. Selects the adapter,
/// pre-flights API-key requirement, builds + sends the request (with
/// pagination), dedupes by `id`, and returns a structured outcome.
pub async fn fetch_models(
    cfg: LocalProviderConfig,
    http: reqwest::Client,
) -> FetchModelsOutcome {
    match tokio::time::timeout(FETCH_TIMEOUT, fetch_models_inner(cfg, http)).await {
        Ok(outcome) => outcome,
        Err(_)      => FetchModelsOutcome::Failed(
            format!("Request timed out after {}s", FETCH_TIMEOUT.as_secs()),
        ),
    }
}

async fn fetch_models_inner(
    cfg: LocalProviderConfig,
    http: reqwest::Client,
) -> FetchModelsOutcome {
    let adapter = match select_adapter(cfg.api_type) {
        Ok(a) => a,
        Err(AdapterError::UnsupportedApiType(t)) => {
            return FetchModelsOutcome::Failed(
                format!("Fetch models not supported for api_type {t:?}"),
            );
        }
        Err(e) => return FetchModelsOutcome::Failed(format!("{e}")),
    };

    // Pre-flight: every adapter except Ollama requires an API key.
    if cfg.api_type != AgentProviderApiType::Ollama && cfg.api_key.as_deref().unwrap_or("").is_empty() {
        return FetchModelsOutcome::Failed("API key required".into());
    }

    let mut accumulator: Vec<DiscoveredModel> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let req = match adapter.build_list_models_request(&cfg, &http, cursor.as_deref()) {
            Ok(r) => r,
            Err(AdapterError::UnsupportedApiType(t)) => {
                return FetchModelsOutcome::Failed(
                    format!("Fetch models not supported for api_type {t:?}"),
                );
            }
            Err(e) => return FetchModelsOutcome::Failed(format!("{e}")),
        };
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return FetchModelsOutcome::Failed(truncate_to_120(&format!("{e}"))),
        };
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let body = body.chars().take(120).collect::<String>();
            if body.is_empty() {
                return FetchModelsOutcome::Failed(format!("HTTP {status}"));
            }
            return FetchModelsOutcome::Failed(format!("HTTP {status}: {body}"));
        }
        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => return FetchModelsOutcome::Failed(truncate_to_120(&format!("{e}"))),
        };
        let ListModelsPage { mut models, next_cursor } = match adapter.parse_list_models_response(&body) {
            Ok(p) => p,
            Err(e) => return FetchModelsOutcome::Failed(format!("Parse error: {e}")),
        };
        accumulator.append(&mut models);
        if accumulator.len() >= MAX_ENTRIES {
            accumulator.truncate(MAX_ENTRIES);
            break;
        }
        match next_cursor {
            Some(c) => cursor = Some(c),
            None    => break,
        }
    }

    // Dedupe by `id`, keeping first occurrence. Handles overlapping pages.
    let mut seen = std::collections::HashSet::<String>::with_capacity(accumulator.len());
    accumulator.retain(|m| seen.insert(m.id.clone()));

    FetchModelsOutcome::Ok(accumulator)
}

fn truncate_to_120(s: &str) -> String {
    s.chars().take(120).collect()
}

#[cfg(test)]
#[path = "fetch_models_tests.rs"]
mod tests;
```

- [ ] **Step 7.2: Wire up the module declaration**

In `app/src/ai/agent_providers/mod.rs`, alongside the existing `pub mod probe;`:

```rust
pub mod fetch_models;
```

- [ ] **Step 7.3: Build to confirm it compiles**

```bash
cargo build -p warp 2>&1 | tail -5         # clean
```

- [ ] **Step 7.4: Write `fetch_models_tests.rs` (12 tests)**

Tests use a `wiremock` mock HTTP server (the same crate `local_provider_integration.rs` uses — verify in `crates/ai/tests/local_provider_integration.rs` and the workspace Cargo.toml; if wiremock isn't a dev-dep of the `app` crate, add it). Pattern:

```rust
//! Phase 4a tests for `fetch_models()`. Each test spins up a wiremock
//! mock server and confirms the helper handles success, failure, and
//! pagination correctly.

use super::*;
use ai::local_provider::api_type::AgentProviderApiType;
use ai::local_provider::config::LocalProviderConfig;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg_openai(base_url: String, key: &str) -> LocalProviderConfig {
    LocalProviderConfig {
        api_type: AgentProviderApiType::OpenAi,
        base_url,
        api_key: Some(key.into()),
        // ... other fields with reasonable defaults; mirror what
        // probe_tests.rs does to construct test configs.
        ..Default::default()
    }
}

fn http_client() -> reqwest::Client {
    // No connection pooling for tests — simpler timeout behavior.
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn single_page_returns_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"}]}"#,
        ))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else {
        panic!("expected Ok, got {outcome:?}")
    };
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-4o");
}

#[tokio::test]
async fn http_401_returns_failed_with_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"unauthorized"}"#))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert!(msg.contains("HTTP 401"), "got: {msg}");
}

#[tokio::test]
async fn http_404_returns_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert!(msg.contains("HTTP 404"), "got: {msg}");
}

#[tokio::test]
async fn http_500_returns_failed_with_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503).set_body_string(r#"upstream busy"#))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert!(msg.contains("HTTP") && msg.contains("upstream busy"), "got: {msg}");
}

#[tokio::test]
async fn malformed_body_returns_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data": ["#))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert!(msg.starts_with("Parse error:"), "got: {msg}");
}

#[tokio::test]
async fn empty_models_array_returns_ok_with_empty_vec() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":[]}"#))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else { panic!("expected Ok") };
    assert!(models.is_empty());
}

#[tokio::test]
async fn missing_api_key_short_circuits_for_openai() {
    // Mock server that PANICS if hit — proves no HTTP fired.
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string("UNREACHABLE"))
        .expect(0)   // wiremock: assert this mock is NOT called
        .mount(&server).await;
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::OpenAi,
        base_url: server.uri(),
        api_key: None,
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert_eq!(msg, "API key required");
}

#[tokio::test]
async fn missing_api_key_allowed_for_ollama() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"models":[]}"#))
        .mount(&server).await;
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::Ollama,
        base_url: server.uri(),
        api_key: None,
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    assert!(outcome.is_ok(), "got {outcome:?}");
}

#[tokio::test]
async fn pagination_loop_aggregates_three_pages() {
    let server = MockServer::start().await;
    // Page 1: 2 models + cursor "c1"
    Mock::given(method("GET")).and(path("/v1/models")).and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{
            "data":[{"type":"model","id":"m1","display_name":"M1"},
                    {"type":"model","id":"m2","display_name":"M2"}],
            "last_id":"m2","has_more":true}"#))
        .up_to_n_times(1)
        .mount(&server).await;
    // Page 2: 2 models + cursor "c2"
    Mock::given(method("GET")).and(path("/v1/models"))
        .and(query_param("after_id", "m2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{
            "data":[{"type":"model","id":"m3","display_name":"M3"},
                    {"type":"model","id":"m4","display_name":"M4"}],
            "last_id":"m4","has_more":true}"#))
        .up_to_n_times(1)
        .mount(&server).await;
    // Page 3: 1 model + no more
    Mock::given(method("GET")).and(path("/v1/models"))
        .and(query_param("after_id", "m4"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{
            "data":[{"type":"model","id":"m5","display_name":"M5"}],
            "last_id":"m5","has_more":false}"#))
        .mount(&server).await;

    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::Anthropic,
        base_url: server.uri(),
        api_key: Some("sk-ant-fake".into()),
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else { panic!("expected Ok, got {outcome:?}") };
    assert_eq!(models.len(), 5);
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
               vec!["m1","m2","m3","m4","m5"]);
}

#[tokio::test]
async fn truncates_at_max_entries_cap() {
    let server = MockServer::start().await;
    // Each page returns 50 models with has_more=true forever; helper must
    // stop at 200 entries.
    let body = serde_json::to_string(&serde_json::json!({
        "data": (0..50).map(|i| serde_json::json!({"type":"model","id":format!("m{i}"),"display_name":"X"})).collect::<Vec<_>>(),
        "last_id": "m49",
        "has_more": true,
    })).unwrap();
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server).await;
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::Anthropic,
        base_url: server.uri(),
        api_key: Some("sk-ant-fake".into()),
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else { panic!("expected Ok") };
    assert_eq!(models.len(), 200, "should be capped at MAX_ENTRIES");
}

#[tokio::test]
async fn deduplicates_overlapping_pages() {
    // Two pages, second page repeats m2 (rare but defensive).
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models")).and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{
            "data":[{"type":"model","id":"m1","display_name":"M1"},
                    {"type":"model","id":"m2","display_name":"M2"}],
            "last_id":"m2","has_more":true}"#))
        .up_to_n_times(1)
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/models")).and(query_param("after_id", "m2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{
            "data":[{"type":"model","id":"m2","display_name":"M2-dup"},
                    {"type":"model","id":"m3","display_name":"M3"}],
            "last_id":"m3","has_more":false}"#))
        .mount(&server).await;
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::Anthropic,
        base_url: server.uri(),
        api_key: Some("sk-ant-fake".into()),
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else { panic!("expected Ok") };
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["m1", "m2", "m3"]);
    // First occurrence wins for dedup: m2's display_name is "M2", not "M2-dup".
    assert_eq!(models[1].display_name.as_deref(), Some("M2"));
}

#[tokio::test]
async fn unsupported_api_type_returns_failed() {
    // OpenAiResp is the only variant that surfaces UnsupportedApiType at
    // select_adapter. Pass it through and confirm the helper reports.
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::OpenAiResp,
        base_url: "http://localhost:1".into(),   // never hit
        api_key: Some("ignored".into()),
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert!(msg.contains("Fetch models not supported"), "got: {msg}");
}
```

- [ ] **Step 7.5: Run tests + commit**

```bash
cargo nextest run -p warp --lib fetch_models 2>&1 | tail -10   # 12 / 12 passed
cargo nextest run -p warp --lib 2>&1 | tail -3                  # baseline + 12
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
```

If the timeout test (covered by the 15s cap) needs a dedicated case, add:

```rust
#[tokio::test]
async fn timeout_returns_failed() {
    use std::time::Duration;
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(20))
            .set_body_string(r#"{"data":[]}"#))
        .mount(&server).await;
    let outcome = fetch_models(cfg_openai(server.uri(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else { panic!("expected Failed") };
    assert!(msg.contains("timed out"), "got: {msg}");
}
```

(Optional — the 15s real-time wait can slow CI. If included, gate with `#[cfg(feature = "slow_tests")]` or skip in this phase.)

```bash
git add app/src/ai/agent_providers/fetch_models.rs app/src/ai/agent_providers/fetch_models_tests.rs app/src/ai/agent_providers/mod.rs
git commit -m "feat(app/ai/agent_providers): add fetch_models helper

Phase 4a stage B. Adds wire-protocol-agnostic fetch_models() helper
sibling to probe.rs. Picks adapter via select_adapter, pre-flights
the API-key requirement (every adapter except Ollama requires a key),
paginates via the adapter's parse_list_models_response.next_cursor
return value (capped at MAX_ENTRIES=200 entries and MAX_PAGES=10),
dedupes by id (keeps first occurrence), and wraps the whole flow in
a 15s timeout.

Returns FetchModelsOutcome::Ok(Vec<DiscoveredModel>) on success or
::Failed(String) with a user-visible reason on any failure.

12 unit tests cover: single-page success, HTTP 401 / 404 / 5xx,
malformed JSON, empty array, missing API key short-circuit for
auth-requiring adapters, no-key allowed for Ollama, multi-page
pagination loop, 200-entry truncation cap, dedupe of overlapping
pages, and OpenAiResp unsupported error. (Timeout test optional —
slow; gated separately.)"
```

---

## Stage C: Settings action wiring

### Task 8: `AISettingsPageAction` variants + page-view state + handlers + `FetchModelsHook`

**Files:**
- Modify: `app/src/settings_view/ai_page.rs` — add 6 action variants, 3 page-view fields, the `FetchModelsHook` trait + a `RealFetchModelsHook` impl, and the 6 handler match-arms.
- Create: `app/src/settings_view/ai_page_fetch_models_tests.rs` — action-handler tests with `FakeFetchModelsHook`.
- Modify: `app/src/settings_view/ai_page.rs` (bottom) — `#[cfg(test)] #[path = "ai_page_fetch_models_tests.rs"] mod fetch_models_tests;`.

**Read these reference files FIRST:**
- `app/src/settings_view/ai_page.rs:2216` — the existing `pub enum AISettingsPageAction` definition (~250 variants).
- `app/src/settings_view/ai_page.rs:3290` — the `TestAgentProviderConnection` handler, which is the template for the new `FetchAgentProviderModels` handler (same async-spawn shape).
- `app/src/settings_view/ai_page.rs` `AISettingsPageView` struct — to confirm where the three new fields go.

- [ ] **Step 8.1: Add new action variants to `AISettingsPageAction`**

Below the existing `TestAgentProviderConnection { provider_index: usize }` variant (around line 2346):

```rust
    /// Phase 4a. User clicked the "Fetch models" button on a provider card.
    FetchAgentProviderModels { provider_index: usize },

    /// Phase 4a. Async `fetch_models()` call resolved. Dispatched from
    /// the spawned task back onto the page.
    ResolveFetchAgentProviderModels {
        provider_index: usize,
        provider_id:    String,   // captured at fetch-start; if the
                                  // provider was removed mid-flight,
                                  // the handler drops the resolve.
        outcome:        crate::ai::agent_providers::fetch_models::FetchModelsOutcome,
    },

    /// Phase 4a. User toggled a single row in the open modal.
    ToggleFetchedModelInModal { model_id: String, checked: bool },

    /// Phase 4a. User clicked "Select all" or "Select none".
    SetAllFetchedModelsChecked { checked: bool },

    /// Phase 4a. User clicked "Add N models" — commits checked rows.
    CommitFetchedAgentProviderModels { provider_index: usize },

    /// Phase 4a. Esc / Cancel / Close — discards the modal.
    CancelFetchedAgentProviderModelsModal,
```

- [ ] **Step 8.2: Add `FetchedModelsModalState` and three new fields to `AISettingsPageView`**

Define above (or near) `AISettingsPageView`:

```rust
use ai::local_provider::adapters::DiscoveredModel;

#[derive(Debug, Clone)]
pub struct FetchedModelsModalState {
    pub provider_index: usize,
    pub provider_id:    String,
    pub fetched:        Vec<DiscoveredModel>,
    pub checked:        std::collections::HashSet<String>,
    pub already_added:  std::collections::HashSet<String>,
}
```

In the `AISettingsPageView` struct, add three new fields:

```rust
    pub fetched_models_modal:    Option<FetchedModelsModalState>,
    pub fetch_models_in_flight:  std::collections::HashSet<usize>,
    pub last_fetch_failure:      std::collections::HashMap<usize, String>,
```

…and initialize them in whichever constructor / `Default` impl exists (search for the existing `agent_providers` field initialization in the same struct for the right spot).

- [ ] **Step 8.3: Add the `FetchModelsHook` trait + production impl**

```rust
/// Test-friendly dispatch seam. Production binds `RealFetchModelsHook`;
/// tests bind a `FakeFetchModelsHook` returning canned outcomes.
#[async_trait::async_trait]
pub trait FetchModelsHook: Send + Sync {
    async fn fetch(
        &self,
        cfg: ai::local_provider::config::LocalProviderConfig,
    ) -> crate::ai::agent_providers::fetch_models::FetchModelsOutcome;
}

pub struct RealFetchModelsHook { pub http: reqwest::Client }

#[async_trait::async_trait]
impl FetchModelsHook for RealFetchModelsHook {
    async fn fetch(
        &self,
        cfg: ai::local_provider::config::LocalProviderConfig,
    ) -> crate::ai::agent_providers::fetch_models::FetchModelsOutcome {
        crate::ai::agent_providers::fetch_models::fetch_models(cfg, self.http.clone()).await
    }
}
```

Add a `fetch_models_hook: Arc<dyn FetchModelsHook>` field on `AISettingsPageView`, initialized in the constructor with `Arc::new(RealFetchModelsHook { http: reqwest::Client::new() })`.

- [ ] **Step 8.4: Handler arms for the six new actions**

Add inside the existing `AISettingsPageAction` match block (next to `TestAgentProviderConnection` handler at line 3290):

```rust
AISettingsPageAction::FetchAgentProviderModels { provider_index } => {
    let providers = view.providers();   // however ai_page accesses provider list today
    let Some(provider) = providers.get(provider_index) else {
        log::warn!("FetchAgentProviderModels: invalid provider_index {provider_index}");
        return;
    };
    let provider_id = provider.id.clone();
    let cfg = match LocalProviderConfig::from_agent_provider(provider, view.api_key_for(provider_index)) {
        Ok(c) => c,
        Err(e) => {
            view.last_fetch_failure.insert(provider_index, truncate_to_120(&format!("{e}")));
            return;
        }
    };

    // Clear any prior failure for this index, mark in-flight.
    view.last_fetch_failure.remove(&provider_index);
    view.fetch_models_in_flight.insert(provider_index);

    let hook = Arc::clone(&view.fetch_models_hook);
    let dispatch = ctx.dispatch_typed_action_deferred_handle();   // however ai_page does this
    tokio::spawn(async move {
        let outcome = hook.fetch(cfg).await;
        dispatch.dispatch(AISettingsPageAction::ResolveFetchAgentProviderModels {
            provider_index,
            provider_id,
            outcome,
        });
    });
}

AISettingsPageAction::ResolveFetchAgentProviderModels { provider_index, provider_id, outcome } => {
    view.fetch_models_in_flight.remove(&provider_index);
    // Stale-resolve guard: if the provider was removed or re-added with
    // a different id, drop the resolve.
    let providers = view.providers();
    let Some(provider) = providers.get(provider_index) else {
        log::debug!("ResolveFetchAgentProviderModels: provider_index {provider_index} no longer exists, dropping");
        return;
    };
    if provider.id != provider_id {
        log::debug!("ResolveFetchAgentProviderModels: provider_id mismatch (was {provider_id}, now {}), dropping", provider.id);
        return;
    }

    use crate::ai::agent_providers::fetch_models::FetchModelsOutcome;
    let api_type = provider.api_type;
    match outcome {
        FetchModelsOutcome::Failed(reason) => {
            // Telemetry — Step 8.4b.
            emit_fetch_models_telemetry(&provider_id, Some(api_type),
                FetchTelemetryEvent::Resolve {
                    count: 0,
                    truncated: false,
                    failure_reason_code: Some(classify_failure_reason(&reason)),
                });
            view.last_fetch_failure.insert(provider_index, reason);
            view.fetched_models_modal = None;
        }
        FetchModelsOutcome::Ok(fetched) => {
            let count = fetched.len() as u32;
            // Telemetry — Step 8.4b. truncated = (count == MAX_ENTRIES);
            // the modal will render a "Showing first 200" caption when this
            // happens. Slight approximation if upstream really has exactly
            // 200 chat models — acceptable per §Risks 8.
            emit_fetch_models_telemetry(&provider_id, Some(api_type),
                FetchTelemetryEvent::Resolve {
                    count,
                    truncated: (count as usize) == crate::ai::agent_providers::fetch_models::MAX_ENTRIES,
                    failure_reason_code: None,
                });
            let already_added: HashSet<String> = provider
                .models
                .iter()
                .map(|m| m.id.clone())
                .collect();
            let checked: HashSet<String> = fetched
                .iter()
                .filter(|m| !already_added.contains(&m.id))
                .map(|m| m.id.clone())
                .collect();
            view.fetched_models_modal = Some(FetchedModelsModalState {
                provider_index,
                provider_id,
                fetched,
                checked,
                already_added,
            });
            // Clear any stale failure shown on the button.
            view.last_fetch_failure.remove(&provider_index);
        }
    }
}

AISettingsPageAction::ToggleFetchedModelInModal { model_id, checked } => {
    let Some(modal) = view.fetched_models_modal.as_mut() else { return; };
    if modal.already_added.contains(&model_id) { return; }   // disabled rows
    if checked { modal.checked.insert(model_id); }
    else       { modal.checked.remove(&model_id); }
}

AISettingsPageAction::SetAllFetchedModelsChecked { checked } => {
    let Some(modal) = view.fetched_models_modal.as_mut() else { return; };
    if checked {
        // Select all rows NOT already added.
        modal.checked = modal
            .fetched
            .iter()
            .filter(|m| !modal.already_added.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
    } else {
        modal.checked.clear();
    }
}

AISettingsPageAction::CommitFetchedAgentProviderModels { provider_index } => {
    let Some(modal) = view.fetched_models_modal.take() else { return; };
    if modal.provider_index != provider_index { return; }   // sanity check
    if modal.checked.is_empty() { return; }                  // no-op

    // Build the new rows from the checked DiscoveredModels.
    let rows: Vec<AgentProviderModel> = modal
        .fetched
        .iter()
        .filter(|m| modal.checked.contains(&m.id))
        .filter(|m| !modal.already_added.contains(&m.id))   // defensive
        .map(|d| AgentProviderModel {
            name: d.display_name.clone().unwrap_or_else(|| d.id.clone()),
            id: d.id.clone(),
            context_window: d.context_window.unwrap_or(0),
            max_output_tokens: d.max_output_tokens.unwrap_or(0),
            reasoning: false,
            tool_call: true,
            image: None,
            pdf: None,
            audio: None,
        })
        .collect();

    // Append to the provider's models list and trigger settings persistence.
    // The exact API mirrors what AISettingsPageAction::AddAgentProviderModel
    // does today — call the same internal helper rather than duplicating the
    // save logic.
    let commit_count = rows.len() as u32;
    for row in rows {
        ctx.dispatch_typed_action_deferred(
            AISettingsPageAction::AddAgentProviderModelRow { provider_index, model: row },
        );
    }
    // Telemetry — Step 8.4b.
    emit_fetch_models_telemetry(
        &modal.provider_id,
        view.providers().get(modal.provider_index).map(|p| p.api_type),
        FetchTelemetryEvent::Commit { commit_count },
    );
}

AISettingsPageAction::CancelFetchedAgentProviderModelsModal => {
    view.fetched_models_modal = None;
}
```

**Note:** the action `AddAgentProviderModelRow { provider_index, model }` may not exist today — the existing `AddAgentProviderModel { provider_index }` likely just pushes a blank model. **Verify the existing action**; if it doesn't accept a pre-built `AgentProviderModel`, add a new variant (`AddAgentProviderModelRow`) and a handler that pushes the supplied row + saves. Alternatively, call the persistence helper directly without going through a new action.

- [ ] **Step 8.4b: Telemetry helper**

The two handler arms above reference `emit_fetch_models_telemetry`, `FetchTelemetryEvent`, and `classify_failure_reason` (one declaration site, called from both Resolve and Commit). Add them at module-private scope in `ai_page.rs` near the other AI-event emission sites.

**Find the existing emission pattern FIRST:** search `app/src/ai/` and `app/src/settings_view/` for an existing AI telemetry emit, e.g. `byop_*` events or the existing probe-resolution telemetry. Pattern-match how it gets the telemetry sink handle and the field-encoding convention (likely a `record_event(name, fields)` helper or a typed struct). If no `byop_*` event exists yet, follow the closest sibling (e.g. provider-add or model-add events).

Sketch (adjust to match the actual telemetry shape found above):

```rust
use sha2::{Digest, Sha256};

pub(super) enum FetchTelemetryEvent {
    Resolve {
        count: u32,
        truncated: bool,
        failure_reason_code: Option<&'static str>,
    },
    Commit {
        commit_count: u32,
    },
}

pub(super) fn classify_failure_reason(reason: &str) -> &'static str {
    // Map FetchModelsOutcome::Failed strings back to the design's
    // failure_reason_code vocabulary. The match is on substrings the
    // fetch_models.rs helper produces (kept in sync with that file —
    // if fetch_models.rs changes its message strings, update here too).
    if reason.starts_with("Fetch models not supported")  { "unsupported_api_type" }
    else if reason == "API key required"                   { "missing_api_key" }
    else if reason.contains("timed out")                   { "timeout" }
    else if reason.starts_with("Parse error:")             { "parse_error" }
    else if reason.starts_with("HTTP 4")                   { "http_4xx" }
    else if reason.starts_with("HTTP 5")                   { "http_5xx" }
    else                                                    { "network" }
}

fn hash_provider_id(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    let hex = format!("{digest:x}");
    hex.chars().take(8).collect()
}

pub(super) fn emit_fetch_models_telemetry(
    provider_id: &str,
    api_type: Option<crate::settings::AgentProviderApiType>,
    event: FetchTelemetryEvent,
) {
    let api_type_str = api_type
        .map(|t| format!("{t:?}").to_lowercase())   // matches design §4.4
        .unwrap_or_else(|| "unknown".into());
    let provider_id_hash = hash_provider_id(provider_id);

    // Call the existing telemetry sink. The exact API depends on what
    // the codebase already provides — pattern-match the nearest sibling
    // emission. Sketch:
    //
    //   telemetry::record_ai_event("byop_fetch_models", &json!({
    //       "provider_api_type":    api_type_str,
    //       "provider_id_hash":     provider_id_hash,
    //       ...event-specific fields...
    //   }));
    //
    // base_url / api_key / per-model ids are NOT logged per design §7.3.
    let _ = (api_type_str, provider_id_hash, event);   // remove once wired
    // TODO(wire-to-real-sink): replace the let-discard above with the
    // codebase's actual AI-event emission call once located.
}
```

The `TODO(wire-to-real-sink)` marker is the ONE explicit placeholder in this plan — it exists because the existing telemetry sink API isn't known from the design alone and must be located in the codebase. **Resolving it is a hard requirement for Task 8 completion.** Search for the existing AI-event emission site (likely in `app/src/telemetry/` or via a `record_ai_event` helper), replace the `let _ = ...` line with the real call, and remove the `TODO` comment. The implementing engineer should not commit Task 8 with the TODO still present.

Add the `sha2` dependency to the `warp` crate's `Cargo.toml` `[dependencies]` block if it isn't already there:

```toml
sha2 = "0.10"
```

Verify with `cargo tree -p warp | grep sha2` — likely already pulled in transitively.

- [ ] **Step 8.5: Re-include the test file**

At the bottom of `ai_page.rs`:

```rust
#[cfg(test)]
#[path = "ai_page_fetch_models_tests.rs"]
mod fetch_models_tests;
```

- [ ] **Step 8.6: Write `ai_page_fetch_models_tests.rs` (10 tests)**

```rust
//! Phase 4a action-handler tests using `FakeFetchModelsHook`.

use super::*;
use ai::local_provider::adapters::DiscoveredModel;
use ai::local_provider::config::LocalProviderConfig;
use crate::ai::agent_providers::fetch_models::FetchModelsOutcome;
use std::sync::{Arc, Mutex};

struct FakeFetchModelsHook { canned: Mutex<Vec<FetchModelsOutcome>> }

#[async_trait::async_trait]
impl FetchModelsHook for FakeFetchModelsHook {
    async fn fetch(&self, _cfg: LocalProviderConfig) -> FetchModelsOutcome {
        self.canned.lock().unwrap().remove(0)
    }
}

fn fake_hook(outcomes: Vec<FetchModelsOutcome>) -> Arc<FakeFetchModelsHook> {
    Arc::new(FakeFetchModelsHook { canned: Mutex::new(outcomes) })
}

fn discovered(id: &str) -> DiscoveredModel {
    DiscoveredModel { id: id.into(), display_name: None, context_window: None, max_output_tokens: None }
}

fn make_view_with_one_provider() -> AISettingsPageView {
    let mut view = AISettingsPageView::default();   // however the test fixture builds
    view.add_provider("openai", "https://example.com", Some("sk-fake"));
    view
}

#[tokio::test]
async fn fetch_action_enters_in_flight_state() {
    let mut view = make_view_with_one_provider();
    view.fetch_models_hook = fake_hook(vec![FetchModelsOutcome::Ok(vec![discovered("m1")])]);
    dispatch(&mut view, AISettingsPageAction::FetchAgentProviderModels { provider_index: 0 });
    assert!(view.fetch_models_in_flight.contains(&0));
}

#[tokio::test]
async fn resolve_ok_opens_modal_with_all_checked_default() {
    let mut view = make_view_with_one_provider();
    let provider_id = view.providers()[0].id.clone();
    dispatch(&mut view, AISettingsPageAction::ResolveFetchAgentProviderModels {
        provider_index: 0,
        provider_id,
        outcome: FetchModelsOutcome::Ok(vec![discovered("m1"), discovered("m2")]),
    });
    let modal = view.fetched_models_modal.as_ref().expect("modal should be open");
    assert_eq!(modal.fetched.len(), 2);
    assert_eq!(modal.checked.len(), 2, "default = all-not-already-added checked");
    assert!(view.last_fetch_failure.get(&0).is_none());
}

#[tokio::test]
async fn resolve_failed_records_failure_and_no_modal() {
    let mut view = make_view_with_one_provider();
    let provider_id = view.providers()[0].id.clone();
    dispatch(&mut view, AISettingsPageAction::ResolveFetchAgentProviderModels {
        provider_index: 0,
        provider_id,
        outcome: FetchModelsOutcome::Failed("HTTP 401".into()),
    });
    assert!(view.fetched_models_modal.is_none());
    assert_eq!(view.last_fetch_failure.get(&0).unwrap(), "HTTP 401");
}

#[tokio::test]
async fn toggle_flips_checked_state() {
    let mut view = make_view_with_one_provider();
    open_modal_with(&mut view, vec![discovered("m1"), discovered("m2")]);
    dispatch(&mut view, AISettingsPageAction::ToggleFetchedModelInModal {
        model_id: "m1".into(), checked: false,
    });
    let modal = view.fetched_models_modal.as_ref().unwrap();
    assert!(!modal.checked.contains("m1"));
    assert!(modal.checked.contains("m2"));
}

#[tokio::test]
async fn select_all_checks_only_not_already_added() {
    let mut view = make_view_with_one_provider();
    // Pre-existing model with id "m1" on the provider.
    view.add_model_to_provider(0, "m1");
    open_modal_with(&mut view, vec![discovered("m1"), discovered("m2")]);
    dispatch(&mut view, AISettingsPageAction::SetAllFetchedModelsChecked { checked: true });
    let modal = view.fetched_models_modal.as_ref().unwrap();
    assert!(!modal.checked.contains("m1"), "already-added must not be auto-checked");
    assert!(modal.checked.contains("m2"));
}

#[tokio::test]
async fn select_none_clears_checked() {
    let mut view = make_view_with_one_provider();
    open_modal_with(&mut view, vec![discovered("m1"), discovered("m2")]);
    dispatch(&mut view, AISettingsPageAction::SetAllFetchedModelsChecked { checked: false });
    assert!(view.fetched_models_modal.as_ref().unwrap().checked.is_empty());
}

#[tokio::test]
async fn commit_appends_checked_models_to_provider() {
    let mut view = make_view_with_one_provider();
    open_modal_with(&mut view, vec![discovered("m1"), discovered("m2")]);
    let before = view.providers()[0].models.len();
    dispatch(&mut view, AISettingsPageAction::CommitFetchedAgentProviderModels { provider_index: 0 });
    let after = view.providers()[0].models.len();
    assert_eq!(after - before, 2);
    let ids: Vec<&str> = view.providers()[0].models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"m1") && ids.contains(&"m2"));
    assert!(view.fetched_models_modal.is_none(), "modal must close on commit");
}

#[tokio::test]
async fn commit_with_empty_checked_is_noop() {
    let mut view = make_view_with_one_provider();
    open_modal_with(&mut view, vec![discovered("m1")]);
    // Uncheck the only row.
    dispatch(&mut view, AISettingsPageAction::ToggleFetchedModelInModal {
        model_id: "m1".into(), checked: false,
    });
    let before = view.providers()[0].models.len();
    dispatch(&mut view, AISettingsPageAction::CommitFetchedAgentProviderModels { provider_index: 0 });
    assert_eq!(view.providers()[0].models.len(), before, "no-op");
}

#[tokio::test]
async fn cancel_closes_modal_without_change() {
    let mut view = make_view_with_one_provider();
    open_modal_with(&mut view, vec![discovered("m1"), discovered("m2")]);
    let before = view.providers()[0].models.len();
    dispatch(&mut view, AISettingsPageAction::CancelFetchedAgentProviderModelsModal);
    assert!(view.fetched_models_modal.is_none());
    assert_eq!(view.providers()[0].models.len(), before);
}

#[tokio::test]
async fn stale_resolve_after_provider_removed_is_dropped() {
    let mut view = make_view_with_one_provider();
    let stale_id = view.providers()[0].id.clone();
    dispatch(&mut view, AISettingsPageAction::RemoveAgentProvider { provider_index: 0 });
    // Now resolve a fetch that started before the removal.
    dispatch(&mut view, AISettingsPageAction::ResolveFetchAgentProviderModels {
        provider_index: 0,
        provider_id: stale_id,
        outcome: FetchModelsOutcome::Ok(vec![discovered("m1")]),
    });
    assert!(view.fetched_models_modal.is_none(), "stale resolve must not open modal");
}

// Helper: synchronously dispatch an action against the view.
fn dispatch(view: &mut AISettingsPageView, action: AISettingsPageAction) {
    // ai_page.rs has an existing test helper for this — reuse it. If it
    // doesn't, write a thin wrapper that invokes the same match block.
    view.handle_action_for_test(action);
}

fn open_modal_with(view: &mut AISettingsPageView, fetched: Vec<DiscoveredModel>) {
    let provider_id = view.providers()[0].id.clone();
    dispatch(view, AISettingsPageAction::ResolveFetchAgentProviderModels {
        provider_index: 0,
        provider_id,
        outcome: FetchModelsOutcome::Ok(fetched),
    });
}
```

- [ ] **Step 8.7: Run tests + commit**

```bash
cargo nextest run -p warp --lib fetch_models 2>&1 | tail -10   # 12 + 10 = 22 / 22 passed
cargo nextest run -p warp --lib 2>&1 | tail -3
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
```

```bash
git add app/src/settings_view/ai_page.rs app/src/settings_view/ai_page_fetch_models_tests.rs
git commit -m "feat(app/settings_view/ai_page): wire fetch-models actions + state

Phase 4a stage C. Adds six new AISettingsPageAction variants
(FetchAgentProviderModels, ResolveFetchAgentProviderModels,
ToggleFetchedModelInModal, SetAllFetchedModelsChecked,
CommitFetchedAgentProviderModels, CancelFetchedAgentProviderModelsModal)
+ their handler arms, three new AISettingsPageView fields
(fetched_models_modal, fetch_models_in_flight, last_fetch_failure),
the FetchedModelsModalState struct, and a FetchModelsHook trait +
RealFetchModelsHook production impl so action-handler tests can
inject a FakeFetchModelsHook.

The Fetch handler captures the provider's id at fetch-start; Resolve
double-checks the id is still the same before applying the outcome
(stale-resolve guard for the case where the provider was removed
mid-flight).

10 action-handler tests cover: enter-in-flight, resolve-ok-opens-modal-
with-all-checked, resolve-failed-records-and-no-modal, toggle,
select-all-excludes-already-added, select-none, commit-appends-rows,
commit-empty-noop, cancel-no-change, and stale-resolve-after-remove-
dropped."
```

---

## Stage D: Widget rendering + manual smoke

### Task 9: Widget — button + modal

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs` — add the "Fetch models" button next to "Test connection" at the card footer (~line 661); add the modal-panel rendering block when `view.fetched_models_modal.is_some()`.

**Read these reference files FIRST:**
- `app/src/settings_view/agent_providers_widget.rs:661-680` — the existing card footer with `test_connection_button` and `remove_button`. The new button slots in here.
- `app/src/settings_view/agent_providers_widget.rs:100-240` — the card-state struct definitions; add a `fetch_models_button_state: MouseStateHandle` field next to the existing `test_connection_button_state`.
- The Warp UI element framework — `Container`, `Row`, `Column`, `Text`, etc., in `crates/warpui/`. Pattern-match what the test-connection button does at the source.

- [ ] **Step 9.1: Add `fetch_models_button_state` to the card-state struct**

In the same struct that holds `test_connection_button_state` (line 103-ish):

```rust
    test_connection_button_state: MouseStateHandle,
    fetch_models_button_state:    MouseStateHandle,   // Phase 4a
    ...
```

Initialize in the same constructor / `default` path:

```rust
    test_connection_button_state: MouseStateHandle::default(),
    fetch_models_button_state:    MouseStateHandle::default(),   // Phase 4a
```

⚠️ **`MouseStateHandle` lifetime gotcha** (from CLAUDE.md): `MouseStateHandle` must be created once during construction and cloned/referenced everywhere. Do NOT call `MouseStateHandle::default()` inline during render — that silently breaks all mouse interactions on that view.

- [ ] **Step 9.2: Render the button in the card footer**

In the render path around line 661 where `test_connection_button` is rendered, add a sibling between it and `remove_button`:

```rust
let test_connection_button = Self::render_card_button(
    /* ... */
    card.test_connection_button_state.clone(),
    AISettingsPageAction::TestAgentProviderConnection { provider_index },
);

// Phase 4a. Tri-state: Idle | Fetching | Failed (re-click to retry).
let fetch_models_label = if view.fetch_models_in_flight.contains(&provider_index) {
    "Fetching…"
} else if view.last_fetch_failure.contains_key(&provider_index) {
    "Failed"
} else {
    "Fetch models"
};
let fetch_models_button = Self::render_card_button(
    fetch_models_label,
    card.fetch_models_button_state.clone(),
    AISettingsPageAction::FetchAgentProviderModels { provider_index },
);

let remove_button = Self::render_card_button(
    /* ... */
    AISettingsPageAction::RemoveAgentProvider { provider_index },
);
```

Place `fetch_models_button` inside the same `Container::new(...)` / `Row` as the other two, between `test_connection_button` and `remove_button`. Add an explicit gap if the existing layout doesn't auto-space.

**Disabled cases** (tooltip + non-dispatching click): if `provider.api_type == OpenAiResp` or (the provider needs an API key AND `api_key` is empty), pass a `disabled: true` flag to the render helper (or skip dispatching the action). The simplest approach: render the action as `FetchAgentProviderModels`, let the handler short-circuit on the missing-api-key check (which it already does as the pre-flight returning `Failed("API key required")` synchronously), and rely on the `Failed` state's tooltip to surface "API key required" to the user. For `OpenAiResp`, the same pattern — first click resolves to `Failed("Fetch models not supported for api_type OpenAiResp")`.

- [ ] **Step 9.3: Render the modal**

Below the main agent-providers-widget rendering, add a top-level conditional block:

```rust
if let Some(modal) = &view.fetched_models_modal {
    // Modal floats above the card layout. Use whatever layering primitive
    // exists in WarpUI (likely a Stack or an Overlay). Mirror what other
    // settings dialogs in the same codebase do — e.g. confirm dialogs in
    // app/src/settings_view/ (search for existing modal patterns).
    let provider = &view.providers()[modal.provider_index];
    let header_text = format!("Fetch models — {} ({})", provider.api_type, provider.name);

    let mut rows = Vec::new();
    for d in &modal.fetched {
        let id          = &d.id;
        let display     = d.display_name.as_deref().unwrap_or(id);
        let already     = modal.already_added.contains(id);
        let is_checked  = modal.checked.contains(id);
        let metadata    = match (d.context_window, d.max_output_tokens) {
            (Some(ctx), Some(out)) => format!("{ctx} ctx · {out} out"),
            (Some(ctx), None)       => format!("{ctx} ctx"),
            (None,      Some(out))  => format!("{out} out"),
            (None,      None)       => String::new(),
        };
        rows.push(/* Container with checkbox(is_checked, disabled=already) + monospace id + display + dim metadata */);
    }

    let select_all_button =  Self::render_card_button("Select all",
        modal.select_all_button_state.clone(),   // NOTE: also add this MouseStateHandle to the modal's state
        AISettingsPageAction::SetAllFetchedModelsChecked { checked: true });
    let select_none_button = Self::render_card_button("Select none",
        modal.select_none_button_state.clone(),
        AISettingsPageAction::SetAllFetchedModelsChecked { checked: false });

    let cancel_button = Self::render_card_button("Cancel",
        modal.cancel_button_state.clone(),
        AISettingsPageAction::CancelFetchedAgentProviderModelsModal);
    let commit_label = format!("Add {} models", modal.checked.len());
    let commit_button = Self::render_card_button(&commit_label,
        modal.commit_button_state.clone(),
        AISettingsPageAction::CommitFetchedAgentProviderModels { provider_index: modal.provider_index });

    // Render header → rows → controls → footer in a Column.
    // Plus: a click-outside-to-cancel overlay layer (dispatches Cancel).
}
```

⚠️ **`MouseStateHandle` lifetime** — the four button states (`select_all_button_state`, `select_none_button_state`, `cancel_button_state`, `commit_button_state`) cannot be created inline in the render path. Move them onto `FetchedModelsModalState` itself:

```rust
pub struct FetchedModelsModalState {
    // existing fields ...
    pub select_all_button_state:   MouseStateHandle,
    pub select_none_button_state:  MouseStateHandle,
    pub cancel_button_state:       MouseStateHandle,
    pub commit_button_state:       MouseStateHandle,
    pub row_checkbox_states:       std::collections::HashMap<String, MouseStateHandle>,
}
```

Initialize them in the `ResolveFetchAgentProviderModels` handler (Step 8.4) when opening the modal:

```rust
FetchedModelsModalState {
    provider_index, provider_id, fetched, checked, already_added,
    select_all_button_state:  MouseStateHandle::default(),
    select_none_button_state: MouseStateHandle::default(),
    cancel_button_state:      MouseStateHandle::default(),
    commit_button_state:      MouseStateHandle::default(),
    row_checkbox_states: fetched.iter().map(|m| (m.id.clone(), MouseStateHandle::default())).collect(),
}
```

- [ ] **Step 9.4: Handle empty fetch results**

If `modal.fetched.is_empty()`, replace the rows + control buttons with a single line `"Upstream returned 0 models."` and a single `[ Close ]` button that dispatches `CancelFetchedAgentProviderModelsModal`.

- [ ] **Step 9.5: Truncation caption**

If `modal.fetched.len() == 200`, render a dim caption above the rows:

```text
Showing first 200 models — narrow your provider's catalog or wait for Phase 4b.
```

- [ ] **Step 9.6: Build + smoke (visual only)**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
```

Then `cargo run` and manually verify: open Settings → AI → Custom AI Providers, click "Fetch models" on a card with a fake provider. The button should enter `Fetching…` state (even if the server is unreachable), then either open the modal or transition to `Failed`. **This is a render-path sanity check**, not a wire smoke (that's Task 10).

- [ ] **Step 9.7: Commit**

```bash
git add app/src/settings_view/agent_providers_widget.rs app/src/settings_view/ai_page.rs
git commit -m "feat(app/settings_view/agent_providers_widget): fetch-models button + modal

Phase 4a stage D part 1. Wires the new \"Fetch models\" button into
the card footer (between Test connection and Remove) and renders the
floating modal panel when fetched_models_modal is Some.

Button is tri-state (Idle / Fetching / Failed); modal renders a list
of rows with per-row checkboxes (already-added rows disabled), a
metadata line for adapters that returned context_window /
max_output_tokens, Select all / Select none / Cancel / Add buttons,
an empty-state for 0-model responses, and a truncation caption when
the 200-entry cap was hit.

MouseStateHandles live on the card and modal state objects (per the
repo's repeated-init pitfall), not inline in render."
```

### Task 10: Manual smoke + spec docs update

**Files:**
- Modify: `specs/multi-local-llm/README.md` — flip the Phase 4a row from "📋 unscheduled" / "🧪 code complete" to "✅ shipped" once smokes pass.
- Modify: `specs/multi-local-llm/design.md` — bump the §9 row for Phase 4a from "future" to "✅ shipped" and add the same status note paragraph after the Phase 3d block (date, tests count, smoke status).

- [ ] **Step 10.1: Live smoke per provider**

Run a smoke against each of the five active adapters. For each: configure a provider card with the live endpoint and a real API key, click "Fetch models", verify the modal opens, commit ≥1 model, and confirm the picker shows the new entry labelled `"{provider.name} / {display_name}"`. Send one turn against the new model to confirm dispatch still works (no regression from the trait extension).

```text
[ ] OpenAI    — base_url https://api.openai.com,                       key sk-...      → modal shows ≥10 models; commit gpt-4o; picker shows "OpenAI / gpt-4o"; one turn works.
[ ] Anthropic — base_url https://api.anthropic.com,                    key sk-ant-...  → modal shows ~10 Claude models with display_name; commit one; one turn works.
[ ] Ollama    — base_url http://localhost:11434,                       no key          → modal shows locally-installed models with synthesized display_name; commit one; one turn works.
[ ] Gemini    — base_url https://generativelanguage.googleapis.com,    key AIza...     → modal shows ≥10 generateContent-supporting models with ctx + out pre-filled; pagination loop fires; commit one; one turn works.
[ ] DeepSeek  — base_url https://api.deepseek.com,                     key sk-...      → modal shows deepseek-chat + deepseek-reasoner; commit one; one turn works.
```

Pass criterion: 5/5 smokes pass. If any fail, file the failure in a §Risks line in `plan-phase-4a.md` (don't block the phase if the failure is an adapter-specific gap that pre-existed Phase 4a; do block if it's a Phase 4a regression).

- [ ] **Step 10.2: Update `specs/multi-local-llm/README.md`**

- Move the Phase 4 future-row from the "Future phases" paragraph into the status table.
- Add a new status paragraph after the Phase 3d paragraph, modeled on it:

```markdown
**Phase 4a (`/models` fetch button)** code is complete on `multi-local-llm` (final commit `<TBD-final-commit-sha>`). Adds a per-provider-card "Fetch models" button that hits each adapter's upstream model-list endpoint (`/v1/models` for OpenAi / Anthropic, `/api/tags` for Ollama, `/v1beta/models` for Gemini, `/models` for DeepSeek), parses the response into a `Vec<DiscoveredModel>` with whatever metadata the upstream returned, and surfaces a modal that lets the user check rows to add as new `AgentProviderModel` entries. The `ProviderAdapter` trait gained two methods (`build_list_models_request` + `parse_list_models_response`) with default impls returning `UnsupportedApiType` — `OpenAiResp` inherits the default. **~53 new unit tests** (30 parser + 12 fetch_models + 10 action-handler + 1 integration scenario) plus the existing 631 stay green (`cargo nextest run -p ai` reports 684/684, `cargo nextest run -p warp --lib` adds +22 fetch-related tests).
```

Once Step 10.1 passes all five smokes, flip the status table row from "🧪 code complete — pending live smoke" to "✅ shipped".

- [ ] **Step 10.3: Update `specs/multi-local-llm/design.md`**

In §9 (Phased plan), change the Phase 4a row to show ✅ shipped + a one-line note pointing at this plan file. In the §3 status preamble at the top, append a Phase-4a line mirroring the existing 3a/3b/3c/3d lines.

- [ ] **Step 10.4: Final reviewer + push**

Dispatch `oh-my-claudecode:code-reviewer` for the full Phase 4a diff (`c74814b7..HEAD`). Stop before push; user reviews, then pushes manually.

```bash
git log --oneline c74814b7..HEAD
# Expected (10 commits, one per task):
#   <sha> docs(specs/multi-local-llm): record Phase 4a code-complete status
#   <sha> feat(app/settings_view/agent_providers_widget): fetch-models button + modal
#   <sha> feat(app/settings_view/ai_page): wire fetch-models actions + state
#   <sha> feat(app/ai/agent_providers): add fetch_models helper
#   <sha> feat(ai/local_provider/adapters/deepseek): list-models parser
#   <sha> feat(ai/local_provider/adapters/gemini): list-models parser
#   <sha> feat(ai/local_provider/adapters/ollama): list-models parser
#   <sha> feat(ai/local_provider/adapters/anthropic): list-models parser
#   <sha> feat(ai/local_provider/adapters/openai): list-models parser
#   <sha> feat(ai/local_provider/adapters): extend ProviderAdapter trait with list_models methods
```

- [ ] **Step 10.5: Spec-docs commit**

```bash
git add specs/multi-local-llm/README.md specs/multi-local-llm/design.md
git commit -m "docs(specs/multi-local-llm): record Phase 4a code-complete status

Phase 4a /models fetch button shipped end-to-end. Status table row
flips from \"🧪 code complete — pending live smoke\" to \"✅ shipped\";
README adds the status paragraph mirroring 3a–3d shape; design.md §9
gets the same row update.

Manual smoke results: 5/5 adapters pass (OpenAI, Anthropic, Ollama,
Gemini, DeepSeek). Picker shows newly-committed models with the
'{provider} / {display_name}' label.

Total test count: cargo nextest run -p ai now 684/684 (631 baseline
+ 53 new); cargo nextest run -p warp --lib adds +22 fetch-handler
tests."
```

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
