# Multi-Local-LLM — Phase 2 (ProviderAdapter Trait) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Hoist a `ProviderAdapter` trait abstraction over the existing OpenAI-compatible streaming wire code so Phase 3 can plug in native Anthropic / Gemini / Ollama / DeepSeek adapters without touching `run.rs`. Adds a "Test connection" probe button (free win — depends only on the trait surface) to the settings widget. **No user-visible behavior change** for OpenAI-compatible endpoints; the existing 320+ tests stay green and exercise the same paths through the trait.

**Architecture:** Four logical stages, atomic in one PR (or split into 2a/2b/2c/2d if review prefers):

- **Stage A (Tasks 1-2)** — Move `AgentProviderApiType` from the app crate down to `crates/ai` (its natural layer; it's a wire-protocol decision) and re-export from the settings module for transparent compatibility. Add an `api_type: AgentProviderApiType` field to `LocalProviderConfig` (default `OpenAi` so existing literal constructors keep working with `..Default::default()`).
- **Stage B (Tasks 3-4)** — Define `ProviderAdapter` and `StreamDecoder` traits. Implement `OpenAiAdapter` lifting the bodies of `compose_chat_completion_request`, the `OpenAiSseAdapter` drive loop, and `run_summarizer_turn`'s body composer/parser. Add `select_adapter(api_type)` with stub branches for the five non-OpenAI variants returning `UnsupportedApiType`.
- **Stage C (Task 5)** — Refactor `run_chat_turn` and `run_summarizer_turn` to delegate to the selected adapter. The SSE drive loop in `synthesize_stream` switches from a concrete `OpenAiSseAdapter` to a `Box<dyn StreamDecoder>` — body change is a sed-grade rename.
- **Stage D (Task 6)** — Wire the "Test connection" button into `AgentProvidersWidget`. Per-card button shows `Idle | Probing | Ok | Failed("…")` state; on click runs the adapter's `build_probe_request` (typically `GET {base_url}/models`).

**Branch:** `multi-local-llm`. Forks from latest `master` of Phase 1 (tag `v0.1.0`). 31 commits ahead of `nmehta/local-llm-provider`. Estimated ~400 lines net code, ~3 hours of subagent-driven work.

**Spec references:**
- `specs/multi-local-llm/design.md` §2.2 (ProviderAdapter trait sketch), §9 (phased plan, "Verification gate" column for Phase 2: "All existing tests pass; stub adapter exercises dispatch").
- `specs/multi-local-llm/README.md` (Phase status table — gets a row for Phase 2 once this plan is accepted).

**Test gate:** All existing `cargo nextest run -p ai` tests pass; new trait-level tests added (~10 new tests). Manual smoke: a single Ollama provider continues to dispatch identically; "Test connection" button works against a real endpoint.

**Out of Phase 2 (deferred):**
- Native Anthropic / Gemini / Ollama / DeepSeek adapter bodies — Phase 3a–d (one PR per variant).
- `/models` fetch button — Phase 4a (the probe button is a connectivity check, not a model-list importer).
- models.dev catalog — Phase 4b.
- Multimodal capability fields wiring — Phase 4c.
- Dedicated compaction model — Phase 4d.

---

## Design refinement

`design.md` §2.2 sketches the trait but defers final shape to implementation. This section locks the shape down.

### Two traits, not one

The current code has two distinct concerns:

1. Composing an HTTP request body (synchronous, pure).
2. Decoding a streaming response (stateful, mid-stream).

Forcing both into one trait method (`compose_request -> http::Request<Body>`) loses the stateful decoder — each chunk feeds into accumulated state across `feed` calls. Splitting cleanly:

- `ProviderAdapter` is `Send + Sync` and stateless. One zero-sized struct per registered api_type is fine — it has no per-turn state. Selection-time cost is a single `Box::new`.
- `StreamDecoder` is `Send` (not `Sync`) and stateful. The adapter constructs a fresh decoder per turn. The decoder is owned by the SSE drive loop and never shared across threads.

### Trait shapes

```rust
// crates/ai/src/local_provider/adapters/mod.rs

pub trait ProviderAdapter: Send + Sync {
    fn api_type(&self) -> AgentProviderApiType;

    /// Build the per-turn streaming chat request. The returned `RequestBuilder`
    /// has the body, headers, and auth already applied; the caller does the POST.
    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    /// Construct the stream decoder used for the lifetime of this turn. `ids`
    /// carries the controller-supplied conversation/task identifiers — when
    /// `None`, the decoder synthesizes fresh ones (test paths only).
    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder>;

    /// Build the non-streaming summarizer request used by the compaction
    /// pipeline. Returned `RequestBuilder` includes body + headers + auth.
    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    /// Decode the upstream summarizer body (already a successful HTTP 200) into
    /// the assistant's summary text. Returns `SummarizerError::DecodeResponse`
    /// or `NoContent` per the existing contract.
    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError>;

    /// Build a lightweight GET probe for the "Test connection" button. The
    /// adapter chooses the most compatible endpoint (e.g. `GET /v1/models`
    /// for OpenAi). Caller fires it; success is HTTP 2xx, body content is
    /// not parsed.
    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;
}

pub trait StreamDecoder: Send {
    fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent>;
    fn finish(&mut self) -> Vec<api::ResponseEvent>;
    fn is_terminal(&self) -> bool;
    fn record_upstream_error(&mut self, msg: String);
}

#[derive(Debug, Clone)]
pub struct StreamIds {
    pub conversation_id: String,
    pub request_id: String,
    pub run_id: String,
    pub task_id: String,
}
```

### Adapter selection

```rust
pub fn select_adapter(api_type: AgentProviderApiType)
    -> Result<Box<dyn ProviderAdapter>, AdapterError>
{
    use AgentProviderApiType::*;
    match api_type {
        OpenAi => Ok(Box::new(OpenAiAdapter)),
        OpenAiResp | Gemini | Anthropic | Ollama | DeepSeek => {
            Err(AdapterError::UnsupportedApiType(api_type))
        }
    }
}
```

The five stub branches return a structured error; Phase 3 swaps each into a real impl. Returning `Err` keeps the dispatcher's failure path tested today (and shows up in the probe button as `Failed: api_type Anthropic is not implemented yet` — a useful signal during Phase 3 rollout).

The match is intentionally exhaustive — no `_ =>` arm — per repo convention so Phase 3 PRs trigger a compile error at this site when adding/removing variants.

### `OpenAiAdapter` is a re-skin of existing code

`OpenAiAdapter` is a unit struct (`pub struct OpenAiAdapter;`) — zero-sized, no state. Its impls just delegate to the existing functions:

- `build_chat_request` calls `compose_chat_completion_request(input, cfg)` and applies `Authorization: Bearer` + body/content-type headers (the body of today's `run_chat_turn` lines 50–110, hoisted).
- `create_stream_decoder` returns `Box::new(OpenAiSseAdapter::with_ids(...))` (or `new()` when `ids = None`).
- `OpenAiSseAdapter` gains `impl StreamDecoder for OpenAiSseAdapter { ... }` whose methods just forward to the existing inherent methods.
- `build_summarizer_request` lifts the body composition + header apply from today's `run_summarizer_turn` lines 433–453.
- `parse_summarizer_response` lifts the response-decoding logic from `run_summarizer_turn` lines 465–494.
- `build_probe_request` is new: `GET {base_url}/models` with the bearer token. Most OpenAI-compatible servers (Ollama, LM Studio, vLLM, llama.cpp, OpenRouter) implement this.

Net new logic: ~30 lines (probe + glue). Net moved logic: ~80 lines (URL/header wiring out of `run.rs`).

### `LocalProviderConfig` gets `api_type`

```rust
pub struct LocalProviderConfig {
    pub display_name: String,
    pub base_url: String,
    pub model_id: String,
    pub api_key: Option<String>,
    pub supports_tools: bool,
    pub context_window: Option<u32>,
    pub api_type: AgentProviderApiType,    // NEW (Phase 2)
}
```

Default for `api_type` is `OpenAi` (matches today's behavior). The dispatch site `app/src/ai/local_provider_config.rs::snapshot_for_request` reads `provider.api_type` from the looked-up `AgentProvider` (already available — it's just been silently dropped since 1b-2). Three lines change there.

The legacy `snapshot_from_app` path (used during the pre-migration window) sets `api_type: AgentProviderApiType::OpenAi` explicitly — the legacy single-provider config has always been OpenAI-compatible.

### Why move `AgentProviderApiType` to `crates/ai`

`AgentProviderApiType` is a wire-protocol enum, not a settings-only concern. Today it lives in `app/src/settings/ai.rs` because settings consume it for serde. After Phase 2 the dispatch crate also consumes it for adapter selection. Moving it down to `crates/ai/src/local_provider/api_type.rs` puts it in its natural layer; the settings module re-exports via `pub use ::ai::local_provider::AgentProviderApiType;` so no settings call site changes.

This also avoids the `app` crate becoming a dependency of `crates/ai` (which would create a circular reference — the `app` crate already depends on `crates/ai`).

### Test-connection probe (free win)

The `build_probe_request` adapter method exists so the settings UI can ping a provider without rebuilding the body composer. For OpenAI the probe is `GET {base_url}/models` with the bearer token. Success criterion: HTTP 2xx. We do not parse the body — some servers return non-JSON or vendor-specific shapes; we just want to know "did the URL + auth + base path work."

The widget button has four visual states:
- **Idle** — default; clicking runs probe.
- **Probing** — spinner; waiting on the response.
- **Ok** — green check + "Connected"; resets to Idle on next config change.
- **Failed(reason)** — red text with the first ~80 chars of the error.

State lives on the widget, not in settings (probe results are ephemeral; surviving a settings reload would lie if the URL was edited and the app restarted before re-probing).

---

## File map

**Files modified:**
- `crates/ai/src/local_provider/mod.rs` — register `pub mod adapters; pub mod api_type;` + re-exports.
- `crates/ai/src/local_provider/config.rs` — add `api_type` field, add `Default` impl, add `models_list_url()` helper.
- `crates/ai/src/local_provider/run.rs` — `run_chat_turn` + `run_summarizer_turn` delegate to selected adapter; `synthesize_stream` parameterized over `Box<dyn StreamDecoder>`.
- `crates/ai/src/local_provider/response.rs` — add `impl StreamDecoder for OpenAiSseAdapter` block (4 forwarding methods).
- `crates/ai/src/local_provider/request.rs` — fixture `cfg()` in `tests` module gains `api_type: AgentProviderApiType::OpenAi,`.
- `app/src/settings/ai.rs` — replace `AgentProviderApiType` definition with `pub use ::ai::local_provider::AgentProviderApiType;` (one line + deletion of ~35 lines).
- `app/src/ai/local_provider_config.rs` — `snapshot_for_request` populates `api_type` from looked-up `AgentProvider`; `snapshot_from_app` sets `OpenAi` explicitly.
- `app/src/settings_view/agent_providers_widget.rs` — add `Test connection` button + `ProbeUiState` per-card.
- `app/src/ai/agent_providers/mod.rs` — register `pub mod probe;`.

**Files created:**
- `crates/ai/src/local_provider/api_type.rs` — moved `AgentProviderApiType` enum body.
- `crates/ai/src/local_provider/adapters/mod.rs` — `ProviderAdapter` + `StreamDecoder` traits, `StreamIds`, `select_adapter`, `AdapterError`.
- `crates/ai/src/local_provider/adapters/openai.rs` — `OpenAiAdapter` impl.
- `crates/ai/src/local_provider/adapters/adapters_tests.rs` — sibling tests for `select_adapter` + stub error path + `OpenAiAdapter` glue.
- `crates/ai/src/local_provider/adapters/probe_tests.rs` — sibling tests for the probe builder.
- `app/src/ai/agent_providers/probe.rs` — `probe(cfg, http) -> ProbeOutcome` async helper.

**Cargo deps:** none added.

---

## Stage A: Plumb `api_type` into `LocalProviderConfig`

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git status --short                       # only .claude/, .omc/ untracked
git describe --tags --abbrev=0           # v0.1.0 (or later)
cargo nextest run -p ai 2>&1 | tail -3   # 320+/320+ passed
```

If anything diverges, STOP and report.

### Task 1: Move `AgentProviderApiType` to `crates/ai`

**Files:**
- Create: `crates/ai/src/local_provider/api_type.rs`
- Modify: `crates/ai/src/local_provider/mod.rs`
- Modify: `app/src/settings/ai.rs`

- [ ] **Step 1.1: Create `api_type.rs`**

Move the enum definition from `app/src/settings/ai.rs:759-793` verbatim. Preserve every derive (`Serialize`, `Deserialize`, `schemars::JsonSchema`, `strum_macros::EnumIter`) and the `#[serde(rename_all = "snake_case")]`. Doc comments on each variant ride along.

```rust
//! Wire-protocol variant of an Agent provider. The dispatch layer uses this
//! to select a `ProviderAdapter` impl. Lives in the ai crate (not the
//! settings module) because adapter selection is a wire-protocol decision
//! — the settings module just re-exports it for serde compatibility.

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash,
    Serialize, Deserialize,
    strum_macros::EnumIter,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderApiType {
    #[default]
    OpenAi,
    OpenAiResp,
    Gemini,
    Anthropic,
    Ollama,
    DeepSeek,
}
```

Verify both crates have the deps already (`serde`, `schemars`, `strum_macros`) — they do; no Cargo.toml change.

- [ ] **Step 1.2: Re-export from `crates/ai/src/local_provider/mod.rs`**

Add alphabetically (between `agent_provider_secrets` and `compaction`):

```rust
pub mod api_type;
pub use api_type::AgentProviderApiType;
```

- [ ] **Step 1.3: Replace `AgentProviderApiType` definition in `app/src/settings/ai.rs`**

Delete lines ~755–793 (the enum body + its doc comment) and replace with one line at an appropriate location near the top of the imports / type re-export section:

```rust
pub use ::ai::local_provider::AgentProviderApiType;
```

Existing call sites (`AgentProvider.api_type` field, settings UI dropdown if any, schemars schema gen) keep working unchanged — same type, same serde shape, same path string at usage sites.

- [ ] **Step 1.4: Build + tests + commit**

```bash
cargo build -p ai 2>&1 | tail -5
cargo build -p warp 2>&1 | tail -5
cargo nextest run -p ai 2>&1 | tail -3       # unchanged count
cargo nextest run -p warp --lib 2>&1 | tail -3
```

Commit:

```
refactor(ai/local_provider): move AgentProviderApiType into the ai crate

Phase 2 stage A. Wire-protocol enum lives in
crates/ai/src/local_provider/api_type.rs (its natural layer); the
settings module re-exports for transparent compatibility. Sets up
adapter selection in stage B. No serde or behavior change.
```

### Task 2: Add `api_type` to `LocalProviderConfig`

**Files:**
- Modify: `crates/ai/src/local_provider/config.rs`
- Modify: `crates/ai/src/local_provider/request.rs` (fixture)
- Modify: `app/src/ai/local_provider_config.rs`

- [ ] **Step 2.1: Add the field + `Default` impl**

In `crates/ai/src/local_provider/config.rs`, extend the struct and add `Default`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalProviderConfig {
    pub display_name: String,
    pub base_url: String,
    pub model_id: String,
    pub api_key: Option<String>,
    pub supports_tools: bool,
    pub context_window: Option<u32>,
    /// Phase 2: wire-protocol selector for adapter dispatch. Defaults to
    /// `OpenAi` so existing literal constructors stay compatible via
    /// `..Default::default()`.
    pub api_type: super::AgentProviderApiType,
}

impl Default for LocalProviderConfig {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            base_url: String::new(),
            model_id: String::new(),
            api_key: None,
            supports_tools: true,
            context_window: None,
            api_type: super::AgentProviderApiType::OpenAi,
        }
    }
}
```

- [ ] **Step 2.2: Update test fixtures**

Existing test fixtures in `request.rs::tests::cfg()` and `config.rs::tests::cfg()` use literal struct expressions. Add `api_type: super::AgentProviderApiType::OpenAi,` (or convert to `..Default::default()`). Two helpers, ~6 keystrokes each.

- [ ] **Step 2.3: Add `models_list_url` helper**

In `config.rs::LocalProviderConfig`, mirror the existing `chat_completions_url`:

```rust
/// `{base_url}/models` for the OpenAI-compatible model-list endpoint.
/// Used by the test-connection probe.
pub fn models_list_url(&self) -> Result<Url, LocalProviderConfigError> {
    let mut base = Url::parse(&self.base_url)
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))?;
    if !base.path().ends_with('/') {
        let new_path = format!("{}/", base.path());
        base.set_path(&new_path);
    }
    base.join("models")
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))
}
```

Add three unit tests in `config.rs::tests` mirroring the existing `chat_completions_url_*` tests (joins path; trailing slash; no path):

```rust
#[test]
fn models_list_url_joins_path() {
    let url = cfg("http://localhost:11434/v1", "llama3.1")
        .models_list_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/v1/models");
}

#[test]
fn models_list_url_with_trailing_slash() {
    let url = cfg("http://localhost:11434/v1/", "llama3.1")
        .models_list_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/v1/models");
}

#[test]
fn models_list_url_no_path() {
    let url = cfg("http://localhost:11434", "llama3.1")
        .models_list_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/models");
}
```

- [ ] **Step 2.4: Populate `api_type` at the dispatch site**

In `app/src/ai/local_provider_config.rs::snapshot_for_request` (around the `LocalProviderConfig { ... }` literal at lines ~110–122):

```rust
let cfg = LocalProviderConfig {
    display_name: ...,
    base_url: ...,
    model_id: ...,
    api_key: ...,
    supports_tools: ...,
    context_window,
    api_type: provider.api_type,         // NEW (read from AgentProvider)
};
```

In `snapshot_from_app` (around lines 53–60), set explicit `OpenAi`:

```rust
let cfg = LocalProviderConfig {
    display_name,
    base_url,
    model_id,
    api_key,
    supports_tools,
    context_window,
    api_type: ::ai::local_provider::AgentProviderApiType::OpenAi,  // legacy path
};
```

- [ ] **Step 2.5: Build + tests + commit**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -3       # +3 tests (models_list_url)
```

Commit:

```
feat(ai/local_provider): thread api_type into LocalProviderConfig

Phase 2 stage A. Adds api_type: AgentProviderApiType to the runtime
config snapshot, populated from the looked-up AgentProvider at
dispatch time. Defaults to OpenAi so literal-struct callers stay
compatible. Adds a Default impl and a models_list_url helper used
by the test-connection probe in stage D.
```

---

## Stage B: Define `ProviderAdapter` trait + `OpenAiAdapter`

### Task 3: Define traits, error type, and selection helper

**Files:**
- Create: `crates/ai/src/local_provider/adapters/mod.rs`
- Create: `crates/ai/src/local_provider/adapters/openai.rs` (stub for now)
- Modify: `crates/ai/src/local_provider/mod.rs`

- [ ] **Step 3.1: Trait + error definitions**

`crates/ai/src/local_provider/adapters/mod.rs`:

```rust
//! Provider adapter trait — abstracts request composition and stream decoding
//! over wire-protocol variants. Phase 2 implements only `OpenAi`; Phase 3
//! adds Anthropic / Gemini / Ollama-native / DeepSeek as additional impls.

use thiserror::Error;
use warp_multi_agent_api as api;

use crate::local_provider::{
    api_type::AgentProviderApiType,
    config::{LocalProviderConfig, LocalProviderConfigError},
    request::LocalProviderInput,
    run::{SummarizerError, SummarizerInput},
};

pub mod openai;
pub use openai::OpenAiAdapter;

#[cfg(test)]
#[path = "adapters_tests.rs"]
mod adapters_tests;
#[cfg(test)]
#[path = "probe_tests.rs"]
mod probe_tests;

/// Trait-level errors. Distinct from `response::AdapterError` (parser-level).
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("invalid local provider config: {0}")]
    InvalidConfig(#[from] LocalProviderConfigError),
    #[error("failed to encode request body: {0}")]
    EncodeRequest(#[from] serde_json::Error),
    #[error("provider api_type {0:?} is not implemented yet")]
    UnsupportedApiType(AgentProviderApiType),
}

#[derive(Debug, Clone)]
pub struct StreamIds {
    pub conversation_id: String,
    pub request_id: String,
    pub run_id: String,
    pub task_id: String,
}

pub trait StreamDecoder: Send {
    fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent>;
    fn finish(&mut self) -> Vec<api::ResponseEvent>;
    fn is_terminal(&self) -> bool;
    fn record_upstream_error(&mut self, msg: String);
}

pub trait ProviderAdapter: Send + Sync {
    fn api_type(&self) -> AgentProviderApiType;

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder>;

    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError>;

    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;
}

pub fn select_adapter(
    api_type: AgentProviderApiType,
) -> Result<Box<dyn ProviderAdapter>, AdapterError> {
    use AgentProviderApiType::*;
    match api_type {
        OpenAi => Ok(Box::new(OpenAiAdapter)),
        OpenAiResp | Gemini | Anthropic | Ollama | DeepSeek => {
            Err(AdapterError::UnsupportedApiType(api_type))
        }
    }
}
```

`SummarizerError` lives in `run.rs` today — it stays there. `SummarizerInput` likewise. Both are referenced from the trait via `crate::local_provider::run::{SummarizerError, SummarizerInput}`.

- [ ] **Step 3.2: Stub `OpenAiAdapter`**

Create `crates/ai/src/local_provider/adapters/openai.rs` with a unit struct + `unimplemented!()` bodies — Task 4 fills them in. This keeps the module compiling so trait imports resolve in `select_adapter`.

```rust
use super::{
    AdapterError, AgentProviderApiType, LocalProviderConfig, LocalProviderInput,
    ProviderAdapter, StreamDecoder, StreamIds, SummarizerError, SummarizerInput,
};

pub struct OpenAiAdapter;

impl ProviderAdapter for OpenAiAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::OpenAi
    }
    fn build_chat_request(
        &self,
        _input: &LocalProviderInput,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        unimplemented!("Task 4: hoist run_chat_turn body composition")
    }
    fn create_stream_decoder(
        &self,
        _ids: Option<StreamIds>,
        _skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        unimplemented!("Task 4: lift OpenAiSseAdapter construction")
    }
    fn build_summarizer_request(
        &self,
        _input: &SummarizerInput,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        unimplemented!("Task 4: hoist run_summarizer_turn body composition")
    }
    fn parse_summarizer_response(&self, _body: &str) -> Result<String, SummarizerError> {
        unimplemented!("Task 4: lift run_summarizer_turn parse logic")
    }
    fn build_probe_request(
        &self,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        unimplemented!("Task 4: implement probe (GET /models)")
    }
}
```

- [ ] **Step 3.3: Register module + re-export**

In `crates/ai/src/local_provider/mod.rs`, add (alphabetically, between `agent_provider_secrets` and `api_type`):

```rust
pub mod adapters;
pub use adapters::{
    select_adapter, AdapterError as ProviderAdapterError, OpenAiAdapter, ProviderAdapter,
    StreamDecoder, StreamIds,
};
```

The `AdapterError as ProviderAdapterError` rename avoids collision with the existing `response::AdapterError` re-export in this same file (they live in the same module and would otherwise conflict).

- [ ] **Step 3.4: Build + commit**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai 2>&1 | tail -3       # unchanged count
```

Commit:

```
feat(ai/local_provider/adapters): introduce ProviderAdapter + StreamDecoder traits

Phase 2 stage B. Defines the ProviderAdapter trait, the StreamDecoder
trait (split out from a single-method shape so per-turn state stays
addressable), the AdapterError variants for unsupported variants and
config issues, and select_adapter that dispatches on
AgentProviderApiType. OpenAiAdapter exists as a unit struct with
unimplemented!() bodies — Task 4 fills it in.
```

### Task 4: Implement `OpenAiAdapter`

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/openai.rs`
- Modify: `crates/ai/src/local_provider/response.rs`
- Modify: `crates/ai/src/local_provider/run.rs` (extract `first_chars` helper if needed)
- Create: `crates/ai/src/local_provider/adapters/adapters_tests.rs`
- Create: `crates/ai/src/local_provider/adapters/probe_tests.rs`

- [ ] **Step 4.1: `impl StreamDecoder for OpenAiSseAdapter`**

In `crates/ai/src/local_provider/response.rs`, append after the existing `impl OpenAiSseAdapter`:

```rust
impl crate::local_provider::adapters::StreamDecoder for OpenAiSseAdapter {
    fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent> {
        Self::feed(self, data)
    }
    fn finish(&mut self) -> Vec<api::ResponseEvent> {
        Self::finish(self)
    }
    fn is_terminal(&self) -> bool {
        Self::is_terminal(self)
    }
    fn record_upstream_error(&mut self, msg: String) {
        Self::record_upstream_error(self, msg)
    }
}
```

- [ ] **Step 4.2: Replace `OpenAiAdapter` stub bodies**

`crates/ai/src/local_provider/adapters/openai.rs`:

```rust
use crate::local_provider::{
    request::compose_chat_completion_request,
    response::OpenAiSseAdapter,
    run::first_chars,
    wire::{ChatCompletionRequest, ChatCompletionResponse},
};

use super::{
    AdapterError, AgentProviderApiType, LocalProviderConfig, LocalProviderInput,
    ProviderAdapter, StreamDecoder, StreamIds, SummarizerError, SummarizerInput,
};

pub struct OpenAiAdapter;

impl ProviderAdapter for OpenAiAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::OpenAi
    }

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.chat_completions_url()?;
        let body = compose_chat_completion_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        let mut req = http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .body(body_json);
        if let Some(key) = &cfg.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        Ok(req)
    }

    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        let mut adapter = match ids {
            Some(ids) => OpenAiSseAdapter::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => OpenAiSseAdapter::new(),
        };
        if skip_create_task {
            adapter.skip_create_task();
        }
        Box::new(adapter)
    }

    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.chat_completions_url()?;
        let body = ChatCompletionRequest {
            model: cfg.model_id.clone(),
            messages: input.messages.clone(),
            tools: None,
            tool_choice: None,
            stream: false,
            stream_options: None,
        };
        let body_json = serde_json::to_string(&body)?;
        let mut req = http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body_json);
        if let Some(key) = &cfg.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        Ok(req)
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: ChatCompletionResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!("{e}: {}", first_chars(body, 200)))
        })?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(err.message));
        }
        parsed
            .choices
            .into_iter()
            .find_map(|choice| {
                let m = choice.message?;
                let candidate = m
                    .content
                    .filter(|s| !s.trim().is_empty())
                    .or(m.reasoning_content)
                    .or(m.reasoning)?;
                let trimmed = candidate.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .ok_or(SummarizerError::NoContent)
    }

    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.models_list_url()?;
        let mut req = http.get(url);
        if let Some(key) = &cfg.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        Ok(req)
    }
}
```

The two private helpers — `compose_chat_completion_request` and `first_chars` — need to be `pub(crate)` so the adapter can reach them. `first_chars` lives in `run.rs:497`; promote it from `fn` to `pub(crate) fn`. `compose_chat_completion_request` is already `pub` in `request.rs`.

- [ ] **Step 4.3: Tests for `select_adapter` + `OpenAiAdapter` glue**

`crates/ai/src/local_provider/adapters/adapters_tests.rs`:

```rust
use super::*;
use crate::local_provider::{config::LocalProviderConfig, request::LocalProviderInput};

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Local".into(),
        base_url: "http://localhost:11434/v1".into(),
        model_id: "llama3.1".into(),
        api_key: Some("k".into()),
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::OpenAi,
    }
}

#[test]
fn select_adapter_returns_openai_for_openai_api_type() {
    let a = select_adapter(AgentProviderApiType::OpenAi).expect("ok");
    assert_eq!(a.api_type(), AgentProviderApiType::OpenAi);
}

#[test]
fn select_adapter_errors_for_each_unimplemented_variant() {
    for ty in [
        AgentProviderApiType::OpenAiResp,
        AgentProviderApiType::Gemini,
        AgentProviderApiType::Anthropic,
        AgentProviderApiType::Ollama,
        AgentProviderApiType::DeepSeek,
    ] {
        let err = select_adapter(ty).expect_err("expected UnsupportedApiType");
        match err {
            AdapterError::UnsupportedApiType(got) => assert_eq!(got, ty),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}

#[test]
fn openai_adapter_builds_chat_request_with_bearer_auth() {
    let http = reqwest::Client::new();
    let req = OpenAiAdapter
        .build_chat_request(&LocalProviderInput::default(), &cfg(), &http)
        .expect("ok")
        .build()
        .expect("buildable");
    assert_eq!(req.method().as_str(), "POST");
    assert_eq!(
        req.url().as_str(),
        "http://localhost:11434/v1/chat/completions"
    );
    assert_eq!(
        req.headers().get("authorization").map(|v| v.to_str().unwrap()),
        Some("Bearer k"),
    );
}

#[test]
fn openai_adapter_omits_bearer_when_key_absent() {
    let http = reqwest::Client::new();
    let mut c = cfg();
    c.api_key = None;
    let req = OpenAiAdapter
        .build_chat_request(&LocalProviderInput::default(), &c, &http)
        .unwrap()
        .build()
        .unwrap();
    assert!(req.headers().get("authorization").is_none());
}

#[test]
fn openai_adapter_decoder_returns_box_dyn_stream_decoder() {
    let dec = OpenAiAdapter.create_stream_decoder(None, false);
    assert!(!dec.is_terminal());
}

#[test]
fn openai_adapter_decoder_with_explicit_ids_round_trips_terminal_state() {
    let ids = StreamIds {
        conversation_id: "c".into(),
        request_id: "r".into(),
        run_id: "u".into(),
        task_id: "t".into(),
    };
    let mut dec = OpenAiAdapter.create_stream_decoder(Some(ids), true);
    assert!(!dec.is_terminal());
    dec.feed("[DONE]");
    assert!(dec.is_terminal());
}

#[test]
fn openai_adapter_parse_summarizer_response_extracts_content() {
    let body = r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#;
    let s = OpenAiAdapter
        .parse_summarizer_response(body)
        .expect("ok");
    assert_eq!(s, "hi");
}

#[test]
fn openai_adapter_parse_summarizer_response_no_content_errors() {
    let body = r#"{"choices":[]}"#;
    let err = OpenAiAdapter
        .parse_summarizer_response(body)
        .expect_err("no content");
    assert!(matches!(err, SummarizerError::NoContent));
}
```

- [ ] **Step 4.4: Tests for the probe builder**

`crates/ai/src/local_provider/adapters/probe_tests.rs`:

```rust
use super::*;
use crate::local_provider::config::LocalProviderConfig;

fn cfg(base: &str, api_key: Option<&str>) -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Local".into(),
        base_url: base.into(),
        model_id: "llama3.1".into(),
        api_key: api_key.map(str::to_string),
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::OpenAi,
    }
}

#[test]
fn probe_url_targets_models_list() {
    let req = OpenAiAdapter
        .build_probe_request(
            &cfg("http://localhost:11434/v1", Some("k")),
            &reqwest::Client::new(),
        )
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(req.method().as_str(), "GET");
    assert_eq!(req.url().as_str(), "http://localhost:11434/v1/models");
}

#[test]
fn probe_request_includes_bearer_when_key_set() {
    let req = OpenAiAdapter
        .build_probe_request(
            &cfg("http://localhost:11434/v1", Some("k")),
            &reqwest::Client::new(),
        )
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        req.headers().get("authorization").map(|v| v.to_str().unwrap()),
        Some("Bearer k"),
    );
}

#[test]
fn probe_request_omits_bearer_when_key_absent() {
    let req = OpenAiAdapter
        .build_probe_request(
            &cfg("http://localhost:11434/v1", None),
            &reqwest::Client::new(),
        )
        .unwrap()
        .build()
        .unwrap();
    assert!(req.headers().get("authorization").is_none());
}
```

- [ ] **Step 4.5: Build + tests + commit**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -3       # ~+11 tests vs Task 3
cargo clippy -p ai --all-targets --all-features -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters): implement OpenAiAdapter

Phase 2 stage B. The OpenAi adapter wraps existing
compose_chat_completion_request, OpenAiSseAdapter, and the
summarizer body composer/parser. No behavior change on the wire —
the adapter is just an internal indirection. The probe builder
issues GET {base_url}/models for the test-connection button.
StreamDecoder impl on OpenAiSseAdapter forwards to existing
inherent methods.
```

---

## Stage C: Refactor `run_chat_turn` and `run_summarizer_turn`

### Task 5: Delegate to the selected adapter

**Files:**
- Modify: `crates/ai/src/local_provider/run.rs`

- [ ] **Step 5.1: Add `LocalRunError::Adapter` variant**

```rust
#[derive(Debug, thiserror::Error)]
pub enum LocalRunError {
    #[error("invalid local provider config: {0}")]
    InvalidConfig(#[from] crate::local_provider::config::LocalProviderConfigError),
    #[error("adapter error: {0}")]
    Adapter(#[from] crate::local_provider::adapters::AdapterError),
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("failed to encode request body: {0}")]
    EncodeRequest(#[from] serde_json::Error),
}
```

- [ ] **Step 5.2: Rewrite `run_chat_turn`**

Replace the body of `run_chat_turn` (lines 49–129) with:

```rust
pub async fn run_chat_turn(
    input: LocalProviderInput,
    cfg: LocalProviderConfig,
    cancel_rx: oneshot::Receiver<()>,
    http: reqwest::Client,
) -> Result<LocalResponseStream, LocalRunError> {
    let adapter = crate::local_provider::adapters::select_adapter(cfg.api_type)?;

    let request_builder = adapter.build_chat_request(&input, &cfg, &http)?;
    let body_json = capture_body_for_debug_dump(&request_builder)?;
    debug_dump_request(&body_json);

    let stream_ids = input.task_id.as_deref().map(|task_id| {
        let conversation_id = input
            .conversation_id
            .clone()
            .unwrap_or_else(|| format!("local:{}", uuid::Uuid::new_v4()));
        crate::local_provider::adapters::StreamIds {
            conversation_id,
            request_id: uuid::Uuid::new_v4().to_string(),
            run_id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
        }
    });
    let decoder = adapter.create_stream_decoder(stream_ids, !input.needs_create_task);

    let mut event_source = request_builder
        .eventsource()
        .expect("eventsource() on a fresh, single-use RequestBuilder cannot fail");
    event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

    let synthesized = synthesize_stream(decoder, event_source, cancel_rx).boxed();
    Ok(synthesized)
}
```

`capture_body_for_debug_dump` extracts the body string for the env-gated debug dump. Implementation: clone the `RequestBuilder` (`.try_clone().expect(...)`) and call `.build()`, then read `.body()`. If cloning fails (the builder owns a stream body), fall back to a placeholder. Defer the helper if the dump is disabled — it's only used for dev diagnostics and can be omitted entirely with a feature gate. Simplest path: drop the dump for now, add a TODO, restore in a follow-up. Document in the commit message.

Actually, since keeping the debug dump is valuable for diagnostics, prefer this:

```rust
fn capture_body_for_debug_dump(rb: &reqwest::RequestBuilder) -> Result<String, LocalRunError> {
    if !debug_dump_enabled() {
        return Ok(String::new());
    }
    // try_clone returns None for streamed bodies; we know our adapter sets a
    // String body, so unwrap is safe in practice. Fall back to "<unavailable>"
    // defensively.
    let cloned = rb.try_clone().ok_or_else(|| LocalRunError::EncodeRequest(
        serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "request builder not cloneable for debug dump",
        ))
    ))?;
    let req = cloned.build()?;
    let bytes = req.body().and_then(|b| b.as_bytes()).unwrap_or(b"");
    Ok(String::from_utf8_lossy(&bytes).to_string())
}
```

The exact mechanics of body extraction here are slightly fiddly — confirm against the `reqwest` docs at implementation time. If it's awkward, use the simpler path: have the adapter return `(RequestBuilder, body_json: String)` from `build_chat_request`. That's a one-line tuple change in the trait method. Pick whichever feels less hacky during implementation.

- [ ] **Step 5.3: Generalize `synthesize_stream`**

Change the function signature from:

```rust
fn synthesize_stream(
    mut adapter: OpenAiSseAdapter,
    mut event_source: reqwest_eventsource::EventSource,
    mut cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send {
```

to:

```rust
fn synthesize_stream(
    mut decoder: Box<dyn crate::local_provider::adapters::StreamDecoder>,
    mut event_source: reqwest_eventsource::EventSource,
    mut cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send {
```

Inside, every `adapter.feed(...)`, `adapter.finish()`, `adapter.is_terminal()`, `adapter.record_upstream_error(...)` becomes `decoder.feed(...)` etc. — local var rename, no semantic change. The trait methods have identical signatures to the inherent methods.

- [ ] **Step 5.4: Rewrite `run_summarizer_turn`**

Replace the body (lines 425–495) with:

```rust
pub async fn run_summarizer_turn(
    input: SummarizerInput,
    cfg: &LocalProviderConfig,
    http: &reqwest::Client,
) -> Result<String, SummarizerError> {
    let adapter = crate::local_provider::adapters::select_adapter(cfg.api_type)
        .map_err(|e| SummarizerError::Adapter(e.to_string()))?;
    let request_builder = adapter
        .build_summarizer_request(&input, cfg, http)
        .map_err(|e| SummarizerError::Adapter(e.to_string()))?;

    let resp = request_builder.send().await?;
    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(SummarizerError::UpstreamHttp {
            status: status.as_u16(),
            body: text.chars().take(500).collect(),
        });
    }
    adapter.parse_summarizer_response(&text)
}
```

`SummarizerError` gains:

```rust
#[error("adapter error: {0}")]
Adapter(String),
```

(String, not the structured `AdapterError`, because `AdapterError::InvalidConfig` already wraps `LocalProviderConfigError` which `SummarizerError` separately wraps via `InvalidConfig` — wrapping it twice in the same enum is a duplicate-source-error nuisance. The string variant's content is user-facing and downstream callers don't discriminate.)

- [ ] **Step 5.5: Build + tests + commit**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -3       # all green
cargo clippy -p ai --all-targets --all-features -- -D warnings
```

Commit:

```
refactor(ai/local_provider/run): delegate to selected ProviderAdapter

Phase 2 stage C. run_chat_turn and run_summarizer_turn now select an
adapter via select_adapter(cfg.api_type), call build_chat_request /
build_summarizer_request, and drive the SSE/HTTP loop through the
trait. Behavior unchanged for OpenAi (the only adapter shipping in
Phase 2). UnsupportedApiType errors surface for the five reserved
variants until Phase 3 implements them.

synthesize_stream takes Box<dyn StreamDecoder> instead of a concrete
OpenAiSseAdapter — local var rename, no semantic change.
```

---

## Stage D: Test-connection probe (free win)

### Task 6: Probe entry point + UI button

**Files:**
- Create: `app/src/ai/agent_providers/probe.rs`
- Modify: `app/src/ai/agent_providers/mod.rs`
- Modify: `app/src/settings_view/agent_providers_widget.rs`

- [ ] **Step 6.1: Public probe helper**

`app/src/ai/agent_providers/probe.rs`:

```rust
//! Per-provider connection probe used by the "Test connection" button in
//! AgentProvidersWidget. Each call selects an adapter for the provider's
//! api_type, builds a probe request (typically GET /v1/models), fires it,
//! and returns a one-line user-visible status.

use ai::local_provider::{
    config::LocalProviderConfig, select_adapter, ProviderAdapterError,
};

#[derive(Debug, Clone)]
pub enum ProbeOutcome {
    Ok,
    Failed(String),
}

pub async fn probe(cfg: LocalProviderConfig, http: reqwest::Client) -> ProbeOutcome {
    let adapter = match select_adapter(cfg.api_type) {
        Ok(a) => a,
        Err(ProviderAdapterError::UnsupportedApiType(t)) => {
            return ProbeOutcome::Failed(format!("api_type {t:?} is not implemented yet"));
        }
        Err(e) => return ProbeOutcome::Failed(format!("{e}")),
    };
    let req = match adapter.build_probe_request(&cfg, &http) {
        Ok(r) => r,
        Err(e) => return ProbeOutcome::Failed(format!("{e}")),
    };
    match req.send().await {
        Ok(resp) if resp.status().is_success() => ProbeOutcome::Ok,
        Ok(resp) => {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect::<String>();
            ProbeOutcome::Failed(format!("HTTP {status}: {body}"))
        }
        Err(e) => ProbeOutcome::Failed(format!("{e}")),
    }
}
```

In `app/src/ai/agent_providers/mod.rs`, add `pub mod probe;` (alphabetically — between `migration` and any other module).

- [ ] **Step 6.2: Wire button into the widget**

In `app/src/settings_view/agent_providers_widget.rs`, add a per-card `Test connection` button right after the API key field. Per-provider state lives on the widget:

```rust
#[derive(Debug, Clone, Default)]
enum ProbeUiState {
    #[default]
    Idle,
    Probing,
    Ok,
    Failed(String),
}
```

Widget gains `probe_states: HashMap<String, ProbeUiState>` keyed by `provider.id`. Button click handler:

1. Build a `LocalProviderConfig` snapshot for the provider (use the provider's first model id as a placeholder for `model_id` — `validate()` doesn't care about model existence, only non-empty).
2. Set `probe_states[provider.id] = ProbeUiState::Probing`, re-render.
3. Spawn `probe(cfg, http_client).await` via the existing background-task mechanism (whichever the widget uses today for async work — match the existing pattern).
4. On completion, set state to `Ok` or `Failed(msg)` and re-render.

Reset state to `Idle` on any provider field edit (so a stale Ok doesn't lie about a just-changed URL).

Visual:
- **Idle**: button labelled "Test connection".
- **Probing**: "Testing…" with a small spinner.
- **Ok**: green check icon + "Connected" — fades back to Idle on next field edit.
- **Failed(msg)**: red text "Failed: <first 80 chars of msg>". Tooltip shows the full message.

Match the design tokens (colors, typography) used elsewhere in `AgentProvidersWidget` — don't introduce new ones.

- [ ] **Step 6.3: Manual smoke test (documented, not automated)**

Documented in the commit body — requires a running provider:

1. Run a local Ollama on `http://localhost:11434/v1`.
2. Configure a provider entry pointing at it.
3. Click `Test connection` — expect green check + "Connected" inside ~1s.
4. Edit base URL to a wrong port, click — expect red "Failed: HTTP error: connection refused…" inside the configured timeout (~5s).
5. Set api_type to anything other than OpenAi (edit `settings.toml` directly until 1b-3's chip UI is in place), click — expect "Failed: api_type Anthropic is not implemented yet".

- [ ] **Step 6.4: Build + commit**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings
```

Commit:

```
feat(ai/agent_providers): add Test connection probe button

Phase 2 stage D. Per-provider button in AgentProvidersWidget runs the
selected adapter's build_probe_request, surfaces success/failure
inline. Free win on top of the trait abstraction. Stub adapters
return "api_type Foo is not implemented yet" — useful signal for
users while Phase 3 lands.

End of Phase 2 — adapter trait is in place. Phase 3a will implement
the Anthropic adapter as the test case for native non-OpenAI bodies.
```

---

## Final verification

- [ ] **Verification 1: Sweeps**

```bash
echo "=== trait + select_adapter present ==="
grep -rn "trait ProviderAdapter\|trait StreamDecoder\|fn select_adapter" --include="*.rs" .

echo "=== AgentProviderApiType moved ==="
grep -n "pub enum AgentProviderApiType" crates/ai/src/local_provider/api_type.rs
grep -n "pub use ::ai::local_provider::AgentProviderApiType" app/src/settings/ai.rs

echo "=== api_type plumbed into LocalProviderConfig ==="
grep -n "pub api_type:" crates/ai/src/local_provider/config.rs

echo "=== run_chat_turn delegates ==="
grep -n "select_adapter\|build_chat_request\|create_stream_decoder" crates/ai/src/local_provider/run.rs

echo "=== probe wired ==="
grep -rn "build_probe_request\|ProbeOutcome" --include="*.rs" .

echo "=== no stray OpenAiSseAdapter direct references in run.rs ==="
grep -n "OpenAiSseAdapter" crates/ai/src/local_provider/run.rs   # expect: 0 matches
```

- [ ] **Verification 2: Build + tests + clippy**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -5       # 320+ + ~14 new tests
cargo nextest run -p warp --lib 2>&1 | tail -5
cargo clippy -p ai --all-targets --all-features -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

(Workspace clippy with bin targets has the stale-build-hash issue noted in Phase 1b-1; CI presubmit uses a clean build and avoids it.)

- [ ] **Verification 3: Manual smoke**

Real Ollama or LM Studio: open the app, confirm the existing single-provider conversation still works end-to-end (a local model picker entry, send a turn, receive streamed output, run a tool, see the result). No behavior should differ from pre-Phase-2.

Then click "Test connection" on the provider card, expect green check. Edit base URL to a wrong port, click again, expect red failure with HTTP-level reason.

- [ ] **Verification 4: Final reviewer + push**

Dispatch `oh-my-claudecode:code-reviewer` for the full Phase 2 diff (4 logical stages, 6–8 commits). Stop before push; user reviews, then pushes manually.

---

## Risks & open questions

1. **`AgentProviderApiType` move could ripple.** Risk: settings UI dropdown chips, schemars generated schemas, or an EnumIter use elsewhere references the old path. Mitigation: `pub use ::ai::local_provider::AgentProviderApiType` from the settings module is byte-identical to a re-import, so all `crate::settings::ai::AgentProviderApiType` references keep resolving. Verify with `cargo check -p warp` after Task 1 — any breakage shows up immediately.

2. **`Default` for `LocalProviderConfig`.** Today's struct has no `Default` impl; existing tests use literal-struct expressions. Adding the `Default` impl plus extending each test `cfg()` helper with `api_type: AgentProviderApiType::OpenAi` is straightforward but touches ~6 test files. Mitigation: search-and-replace pass during Task 2.

3. **Debug-dump body extraction.** The current `run_chat_turn` calls `debug_dump_request(&body_json)` with the body string in hand. After Task 5, the body lives inside the `RequestBuilder`. Two paths: (a) `try_clone` + `.build()` + `.body()` to extract; (b) have `build_chat_request` return `(RequestBuilder, body_json)`. Path (b) is simpler — pick it if (a) is fiddly. Either way, debug dump remains an env-gated dev-only feature.

4. **`StreamDecoder` is `Send` not `Send + Sync`.** Stream decoders are per-turn state, never shared across threads. Choosing `Send` (without `Sync`) avoids forcing future implementors to wrap internal state in `Mutex`. The existing `synthesize_stream` callsite owns the decoder in one task — no `Sync` requirement.

5. **`SummarizerError::Adapter(String)` vs structured.** The string variant loses error context for downstream callers, but `AdapterError::InvalidConfig` wraps `LocalProviderConfigError` which `SummarizerError` separately wraps under `InvalidConfig` — adding `AdapterError` directly creates a duplicate-source-error problem. The string approach is simpler. Revisit if a caller actually needs to discriminate.

6. **Probe scope = `OpenAiAdapter`-only.** Phase 3 adapters need to implement `build_probe_request` too — easy: each adapter has its own canonical health-check (Anthropic: a tiny test message; Gemini: `GET /v1beta/models`; Ollama-native: `GET /api/tags`). Stub adapters can't probe — `UnsupportedApiType` surfaces as the friendly "not implemented yet" message in the UI, which is useful signal during Phase 3 rollout.

7. **No Phase 3 lock-in.** The trait shape is a Phase 2 commitment, but design.md §2.2 explicitly says "final shape may change with implementation." If Phase 3a (Anthropic) finds the trait awkward — e.g. needs streaming-headers helpers, custom auth headers, or message-vs-event semantics that don't fit `feed(&str)` — we extend it. Backwards compatibility within this branch is fine; the trait is internal.

8. **Two PRs, not one?** This plan is structured as a single PR per the plan-phase-1b-2 precedent. If review prefers smaller chunks, the natural splits are: (a) Stages A+B = trait skeleton + stub OpenAiAdapter, ~250 lines, mergeable on its own as a no-op refactor; (b) Stages C+D = wire it up + add probe, ~150 lines, depends on (a). Decide based on review preference.

---

## Next plan (Phase 3a)

After Phase 2 ships green, Phase 3a will cover:

- New `crates/ai/src/local_provider/adapters/anthropic.rs` implementing the Messages API (request body shape, `x-api-key` auth, SSE event types `content_block_delta` / `message_stop`).
- Decision threshold per design.md §2.3: hand-roll vs. pull in `genai`. Switch to `genai` if hand-rolling exceeds 1 week.
- `AnthropicSseDecoder` impl of `StreamDecoder` — converts Anthropic's event-shape to the same `api::ResponseEvent` stream the controller already speaks.
- Live test against `api.anthropic.com` (gated behind a manual run; not in CI).
- `select_adapter` match arm flips from `Err(UnsupportedApiType)` to `Ok(Box::new(AnthropicAdapter))`.

That plan will be written after Phase 2 is approved + executed.
