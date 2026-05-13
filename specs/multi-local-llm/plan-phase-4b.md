# Phase 4b — models.dev catalog + quick-add chips — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Catalog-driven model onboarding for BYOP provider cards — both inline chips beside the empty "+ Add Model" row and a separate "Browse catalog" modal — sourced from `https://models.dev/api.json` with a 7-day on-disk cache and a baked-in snapshot fallback.

**Architecture:** A new `crates/ai/src/catalog/` module owns fetch / parse / cache / snapshot. The settings page gains a pure-logic `CatalogModalState` (mirroring `FetchedModelsModalState` from 4a), new action variants, and handler arms. The widget renders inline chips inside the empty model row and a card-style modal panel. Catalog metadata can opt-in enrich 4a's `DiscoveredModel` rows.

**Tech Stack:** Rust 2021, `serde` + `serde_json`, `reqwest`, `tokio`, the existing WarpUI element framework.

---

## Wire schema (from a live fetch of `https://models.dev/api.json` 2026-05-13)

Top-level is `HashMap<provider_id, ProviderEntry>`; each `ProviderEntry` owns `id` + `name` and a `models: HashMap<model_id, ModelEntry>`.

```json
{
  "anthropic": {
    "id": "anthropic",
    "name": "Anthropic",
    "models": {
      "claude-opus-4-7": {
        "id": "claude-opus-4-7",
        "name": "claude-opus-4-7",
        "family": "claude-opus",
        "tool_call": true,
        "reasoning": true,
        "open_weights": false,
        "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
        "limit": { "context": 1000000, "output": 128000 }
      }
    }
  }
}
```

Field mapping into Warp's existing `AgentProviderModel`:
- `id`, `name` → `id`, `name`
- `limit.context` → `context_window` (default 0 if missing)
- `limit.output` → `max_output_tokens` (default 0 if missing)
- `tool_call` → `tool_call` (default `true` if missing — matches `AgentProviderModel::from_id`)
- `reasoning` → `reasoning` (default `false`)
- `modalities.input` contains `"image"` → `image = Some(true)` (likewise `pdf`, `audio`)
- `open_weights` → kept on the catalog row, used by the Ollama filter rule (chips suggest only `open_weights: true` models for `api_type::Ollama`).

All `ModelEntry` fields except `id` and `name` are optional; the parser must tolerate missing fields with `#[serde(default)]`.

Provider keys (sample): `openai`, `anthropic`, `google`, `deepseek`, plus many third-party gateways (`302ai`, `nano-gpt`, `scaleway`, `alibaba`, …). The api_type filter map is:

| `AgentProviderApiType` | catalog provider key(s) |
|---|---|
| `OpenAi`               | `"openai"` |
| `Anthropic`            | `"anthropic"` |
| `Gemini`               | `"google"` |
| `DeepSeek`             | `"deepseek"` |
| `Ollama`               | union across providers where `model.open_weights == true` |
| `OpenAiResp`           | none (falls through to the modal's "All providers" filter) |

---

## File map

**New:**
- `crates/ai/src/catalog/mod.rs` — public API: `CatalogModel`, `CatalogError`, `lookup_catalog_entry`, `api_type_to_catalog_provider`. Re-exports submodules.
- `crates/ai/src/catalog/wire.rs` — `WireRoot`, `WireProvider`, `WireModel`, `WireModalities`, `WireLimit`. Tolerant `#[serde(default)]` everywhere except `id` + `name`.
- `crates/ai/src/catalog/parse.rs` — `parse_catalog(body: &str) -> Result<Vec<CatalogModel>, CatalogError>`. Flattens the nested JSON into a flat `Vec<CatalogModel>`; maps `modalities.input` into the three multimodal booleans.
- `crates/ai/src/catalog/fetch.rs` — `fetch_catalog(http: &reqwest::Client) -> Result<Vec<CatalogModel>, CatalogError>`. GET `https://models.dev/api.json`, 10s timeout, 5 MB body cap.
- `crates/ai/src/catalog/cache.rs` — `CatalogCache::{load_or_default, save, needs_refresh, all, lookup}`. On-disk format `{ "fetched_at": <epoch_secs>, "models": [...] }` at `<config_dir>/byop_catalog.json`.
- `crates/ai/src/catalog/snapshot.rs` — `BAKED_IN_SNAPSHOT` via `include_str!`.
- `crates/ai/src/catalog/snapshot.json` — ~30 curated entries across `openai` / `anthropic` / `google` / `deepseek` / `ollama` (open-weights).
- `crates/ai/src/catalog/{wire,parse,fetch,cache}_tests.rs` — sibling test files per the repo convention.
- `app/src/settings_view/catalog_modal.rs` — pure-logic `CatalogModalState`.
- `app/src/settings_view/catalog_modal_tests.rs`.

**Modified:**
- `crates/ai/src/lib.rs` — `pub mod catalog;`
- `crates/ai/Cargo.toml` — `dirs.workspace = true` (or equivalent) for `<config_dir>` resolution if not already present.
- `app/src/settings_view/mod.rs` — `mod catalog_modal;`
- `app/src/settings_view/ai_page.rs` — 9 new action variants, 3 new view fields, 9 handler arms.
- `app/src/settings_view/agent_providers_widget.rs` — inline-chip render in the empty `+ Add Model` row, "Browse catalog" button in the card footer, modal panel render.

---

## Stage A — Catalog module

### Task 1: Wire types + parser

**Files:**
- Create: `crates/ai/src/catalog/wire.rs`
- Create: `crates/ai/src/catalog/parse.rs`
- Create: `crates/ai/src/catalog/parse_tests.rs`
- Modify: `crates/ai/src/catalog/mod.rs` (created in Task 5; for Task 1, declare the submodules temporarily)
- Modify: `crates/ai/src/lib.rs` — `pub mod catalog;`

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/openai/wire.rs` — for the `#[serde(default)]` + tolerant-parsing pattern used in this repo.
- `crates/ai/src/local_provider/adapters/openai.rs` lines around `parse_list_models_response` — for the lift-from-nested-JSON-to-flat-Vec pattern Phase 4a established.

- [ ] **Step 1.1: Create `wire.rs` with the tolerant deserialize types**

```rust
//! Wire-level deserialize types matching `https://models.dev/api.json`.
//! Every field except `id` and `name` is `#[serde(default)]` so upstream
//! schema drift produces a partial parse rather than a hard failure.

use std::collections::HashMap;

use serde::Deserialize;

/// Top-level: keyed by catalog provider id (e.g. "anthropic", "openai").
pub type WireRoot = HashMap<String, WireProvider>;

#[derive(Debug, Deserialize)]
pub struct WireProvider {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub models: HashMap<String, WireModel>,
}

#[derive(Debug, Deserialize)]
pub struct WireModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub tool_call: Option<bool>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub open_weights: Option<bool>,
    #[serde(default)]
    pub modalities: Option<WireModalities>,
    #[serde(default)]
    pub limit: Option<WireLimit>,
}

#[derive(Debug, Deserialize)]
pub struct WireModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct WireLimit {
    #[serde(default)]
    pub context: Option<u32>,
    #[serde(default)]
    pub output: Option<u32>,
}
```

- [ ] **Step 1.2: Create `parse.rs` with the flatten + map**

```rust
//! Parse `models.dev/api.json` into a flat `Vec<CatalogModel>`.
//!
//! The catalog JSON nests models under `{provider_id}.models.{model_id}`.
//! `parse_catalog` flattens this into one entry per model, copying the
//! catalog provider id onto each entry so the api_type filter map can
//! match against it. `modalities.input` is reduced to three booleans.

use thiserror::Error;

use super::wire::WireRoot;

/// One model entry as seen by the rest of the app. Slimmed-down from the
/// raw wire shape — only the fields Warp's `AgentProviderModel` cares
/// about, plus the catalog provider id and the `open_weights` flag used
/// by the Ollama filter rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    /// Catalog provider id, e.g. "anthropic", "openai". Used by the
    /// api_type filter map to decide which chips to suggest.
    pub catalog_provider: String,
    /// Model id as expected by the upstream provider.
    pub id: String,
    /// Human-readable name; for many catalog entries this duplicates `id`.
    pub name: String,
    /// `limit.context`. `None` means "unknown" — chip auto-fill leaves the
    /// row's `context_window` at 0 (the existing "unknown" sentinel on
    /// `AgentProviderModel`).
    pub context_window: Option<u32>,
    /// `limit.output`. Same semantics as `context_window`.
    pub max_output_tokens: Option<u32>,
    /// Whether the model is known to advertise tool/function calling.
    /// Defaults to `true` when absent — matches `AgentProviderModel`'s
    /// existing `default_true` for `tool_call`.
    pub tool_call: bool,
    /// Whether the model emits chain-of-thought reasoning.
    pub reasoning: bool,
    /// `modalities.input` contains `"image"`.
    pub image: bool,
    /// `modalities.input` contains `"pdf"`.
    pub pdf: bool,
    /// `modalities.input` contains `"audio"`.
    pub audio: bool,
    /// `open_weights == true`. The Ollama filter rule unions across all
    /// catalog providers and shows only entries with this flag set, since
    /// Ollama hosts open-weight models regardless of upstream provider.
    pub open_weights: bool,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("HTTP fetch failed: {0}")]
    Fetch(String),
    #[error("HTTP {0}")]
    HttpStatus(u16),
    #[error("response body exceeds 5 MB cap")]
    BodyTooLarge,
    #[error("JSON parse failed: {0}")]
    Parse(String),
    #[error("I/O error: {0}")]
    Io(String),
}

pub fn parse_catalog(body: &str) -> Result<Vec<CatalogModel>, CatalogError> {
    let root: WireRoot =
        serde_json::from_str(body).map_err(|e| CatalogError::Parse(format!("{e}")))?;
    let mut out = Vec::with_capacity(root.values().map(|p| p.models.len()).sum());
    for (provider_id, provider) in root {
        for (_, wire_model) in provider.models {
            let modalities_in = wire_model
                .modalities
                .as_ref()
                .map(|m| m.input.as_slice())
                .unwrap_or(&[]);
            out.push(CatalogModel {
                catalog_provider: provider_id.clone(),
                id: wire_model.id,
                name: if wire_model.name.is_empty() {
                    provider_id.clone()
                } else {
                    wire_model.name
                },
                context_window: wire_model.limit.as_ref().and_then(|l| l.context),
                max_output_tokens: wire_model.limit.as_ref().and_then(|l| l.output),
                tool_call: wire_model.tool_call.unwrap_or(true),
                reasoning: wire_model.reasoning.unwrap_or(false),
                image: modalities_in.iter().any(|s| s == "image"),
                pdf: modalities_in.iter().any(|s| s == "pdf"),
                audio: modalities_in.iter().any(|s| s == "audio"),
                open_weights: wire_model.open_weights.unwrap_or(false),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
```

- [ ] **Step 1.3: Create `parse_tests.rs` with eight tests**

```rust
use super::{parse_catalog, CatalogError};

const ONE_PROVIDER_ONE_MODEL: &str = r#"{
    "anthropic": {
        "id": "anthropic",
        "name": "Anthropic",
        "models": {
            "claude-opus-4-7": {
                "id": "claude-opus-4-7",
                "name": "claude-opus-4-7",
                "tool_call": true,
                "reasoning": true,
                "open_weights": false,
                "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
                "limit": { "context": 1000000, "output": 128000 }
            }
        }
    }
}"#;

#[test]
fn parses_one_provider_one_model() {
    let v = parse_catalog(ONE_PROVIDER_ONE_MODEL).unwrap();
    assert_eq!(v.len(), 1);
    let m = &v[0];
    assert_eq!(m.catalog_provider, "anthropic");
    assert_eq!(m.id, "claude-opus-4-7");
    assert_eq!(m.context_window, Some(1_000_000));
    assert_eq!(m.max_output_tokens, Some(128_000));
    assert!(m.tool_call);
    assert!(m.reasoning);
    assert!(m.image);
    assert!(m.pdf);
    assert!(!m.audio);
    assert!(!m.open_weights);
}

#[test]
fn missing_optional_fields_use_defaults() {
    let body = r#"{
        "deepseek": {
            "id": "deepseek",
            "name": "DeepSeek",
            "models": {
                "deepseek-chat": { "id": "deepseek-chat", "name": "DeepSeek Chat" }
            }
        }
    }"#;
    let v = parse_catalog(body).unwrap();
    assert_eq!(v.len(), 1);
    let m = &v[0];
    assert_eq!(m.context_window, None);
    assert_eq!(m.max_output_tokens, None);
    assert!(m.tool_call, "tool_call defaults to true");
    assert!(!m.reasoning);
    assert!(!m.image && !m.pdf && !m.audio);
    assert!(!m.open_weights);
}

#[test]
fn unknown_fields_are_tolerated() {
    let body = r#"{
        "openai": {
            "id": "openai",
            "name": "OpenAI",
            "completely_new_field": 42,
            "models": {
                "gpt-9": {
                    "id": "gpt-9",
                    "name": "GPT 9",
                    "future_field": "ignored",
                    "limit": { "context": 1000, "output": 500, "input": 999 }
                }
            }
        }
    }"#;
    let v = parse_catalog(body).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].context_window, Some(1000));
    assert_eq!(v[0].max_output_tokens, Some(500));
}

#[test]
fn modalities_audio_only_sets_audio_flag() {
    let body = r#"{
        "openai": {"id":"openai","name":"OpenAI","models":{
            "whisper": {"id":"whisper","name":"Whisper",
                "modalities":{"input":["audio"],"output":["text"]}}
        }}
    }"#;
    let v = parse_catalog(body).unwrap();
    assert!(v[0].audio);
    assert!(!v[0].image && !v[0].pdf);
}

#[test]
fn open_weights_flag_propagates() {
    let body = r#"{
        "meta": {"id":"meta","name":"Meta","models":{
            "llama-3-70b": {"id":"llama-3-70b","name":"Llama 3 70B","open_weights":true}
        }}
    }"#;
    let v = parse_catalog(body).unwrap();
    assert!(v[0].open_weights);
}

#[test]
fn name_falls_back_to_provider_id_when_empty() {
    let body = r#"{
        "anthropic": {"id":"anthropic","name":"Anthropic","models":{
            "m": {"id":"m","name":""}
        }}
    }"#;
    let v = parse_catalog(body).unwrap();
    assert_eq!(v[0].name, "anthropic");
}

#[test]
fn multiple_providers_multiple_models() {
    let body = r#"{
        "openai":   {"id":"openai","name":"OpenAI","models":{
            "gpt-4o":     {"id":"gpt-4o","name":"GPT-4o"},
            "gpt-4o-mini":{"id":"gpt-4o-mini","name":"GPT-4o Mini"}
        }},
        "anthropic":{"id":"anthropic","name":"Anthropic","models":{
            "claude-sonnet-4-6":{"id":"claude-sonnet-4-6","name":"Claude Sonnet 4.6"}
        }}
    }"#;
    let v = parse_catalog(body).unwrap();
    assert_eq!(v.len(), 3);
    assert_eq!(
        v.iter().filter(|m| m.catalog_provider == "openai").count(),
        2
    );
    assert_eq!(
        v.iter()
            .filter(|m| m.catalog_provider == "anthropic")
            .count(),
        1
    );
}

#[test]
fn malformed_json_returns_parse_error() {
    let result = parse_catalog("{not json");
    assert!(matches!(result, Err(CatalogError::Parse(_))));
}
```

- [ ] **Step 1.4: Create a temporary `mod.rs` so `crates/ai` builds**

```rust
//! Catalog module — Phase 4b.
//!
//! Public API is stabilized in Task 5; until then we just expose the
//! parse types so the lower-level tests can build.

pub mod parse;
pub mod wire;

pub use parse::{parse_catalog, CatalogError, CatalogModel};
```

- [ ] **Step 1.5: Wire into `crates/ai/src/lib.rs`**

Find the existing `pub mod local_provider;` block and add alongside:

```rust
pub mod catalog;
```

- [ ] **Step 1.6: Build + run the parser tests**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai catalog::parse 2>&1 | tail -10   # 8/8 passed
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 1.7: Commit**

```bash
git add crates/ai/src/catalog/ crates/ai/src/lib.rs
git commit -m "feat(ai/catalog): wire types + tolerant parser

Phase 4b stage A task 1. Adds the catalog module skeleton with
serde-tolerant WireRoot/WireProvider/WireModel/WireModalities/WireLimit
types matching the live models.dev/api.json shape (captured 2026-05-13)
and a parse_catalog() helper that flattens the nested
provider.models.{id} map into a Vec<CatalogModel>.

CatalogModel slims the wire shape to just the fields AgentProviderModel
uses: id, name, context_window, max_output_tokens, tool_call, reasoning,
plus three booleans derived from modalities.input (image/pdf/audio)
and an open_weights flag used by the Ollama filter rule.

8 unit tests cover happy path, missing-optional defaults, unknown-field
tolerance, audio-only modalities, open_weights propagation, empty-name
fallback, multi-provider flattening, and malformed JSON."
```

---

### Task 2: Catalog fetch (HTTP)

**Files:**
- Create: `crates/ai/src/catalog/fetch.rs`
- Create: `crates/ai/src/catalog/fetch_tests.rs`
- Modify: `crates/ai/src/catalog/mod.rs` — add `pub mod fetch;`

**Read these reference files FIRST:**
- `app/src/ai/agent_providers/fetch_models.rs` lines 50-100 — for the timeout-wrapping idiom used in Phase 4a's fetch helper.
- `app/src/ai/agent_providers/fetch_models_tests.rs` lines 1-30 — for the mockito + rustls-provider-install boilerplate.

- [ ] **Step 2.1: Create `fetch.rs`**

```rust
//! HTTP fetch of `https://models.dev/api.json`. No auth, 10s timeout,
//! 5 MB response cap. Returns the parsed `Vec<CatalogModel>` on success.

use std::time::Duration;

use super::parse::{parse_catalog, CatalogError, CatalogModel};

const CATALOG_URL: &str = "https://models.dev/api.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

pub async fn fetch_catalog(
    http: &reqwest::Client,
) -> Result<Vec<CatalogModel>, CatalogError> {
    let body = tokio::time::timeout(FETCH_TIMEOUT, fetch_body(http, CATALOG_URL))
        .await
        .map_err(|_| {
            CatalogError::Fetch(format!(
                "request timed out after {}s",
                FETCH_TIMEOUT.as_secs()
            ))
        })??;
    parse_catalog(&body)
}

/// Lower-level helper used by tests to point at a mock server.
pub async fn fetch_body(http: &reqwest::Client, url: &str) -> Result<String, CatalogError> {
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| CatalogError::Fetch(format!("{e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CatalogError::HttpStatus(status.as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CatalogError::Fetch(format!("{e}")))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(CatalogError::BodyTooLarge);
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| CatalogError::Parse(format!("{e}")))
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
```

- [ ] **Step 2.2: Create `fetch_tests.rs`**

```rust
use std::sync::Once;

use mockito::Server;

use super::{fetch_body, CatalogError};

fn ensure_rustls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn http_client() -> reqwest::Client {
    ensure_rustls_provider();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn returns_body_on_200() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/api.json")
        .with_status(200)
        .with_body(r#"{"openai":{"id":"openai","name":"OpenAI","models":{}}}"#)
        .create_async()
        .await;
    let body = fetch_body(&http_client(), &format!("{}/api.json", server.url()))
        .await
        .unwrap();
    assert!(body.contains("openai"));
}

#[tokio::test]
async fn http_500_returns_http_status_error() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/api.json")
        .with_status(503)
        .with_body("upstream busy")
        .create_async()
        .await;
    let err = fetch_body(&http_client(), &format!("{}/api.json", server.url()))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::HttpStatus(503)));
}

#[tokio::test]
async fn rejects_body_over_5mb_cap() {
    let mut server = Server::new_async().await;
    let big = "x".repeat(5 * 1024 * 1024 + 1);
    let _m = server
        .mock("GET", "/api.json")
        .with_status(200)
        .with_body(big)
        .create_async()
        .await;
    let err = fetch_body(&http_client(), &format!("{}/api.json", server.url()))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::BodyTooLarge));
}
```

- [ ] **Step 2.3: Wire into mod.rs + build/test/clippy**

```bash
# Append to crates/ai/src/catalog/mod.rs:
#   pub mod fetch;
#   pub use fetch::fetch_catalog;
cargo nextest run -p ai catalog::fetch 2>&1 | tail -10   # 3/3 passed
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 2.4: Commit**

```bash
git add crates/ai/src/catalog/fetch.rs crates/ai/src/catalog/fetch_tests.rs crates/ai/src/catalog/mod.rs
git commit -m "feat(ai/catalog): HTTP fetch with timeout + body cap

Phase 4b stage A task 2. Adds fetch_catalog() that issues GET against
https://models.dev/api.json with a 10s tokio::time::timeout and a 5 MB
response-body cap. Returns the parsed Vec<CatalogModel> on success or
a typed CatalogError on transport/HTTP/oversize/parse failure.

Lower-level fetch_body(http, url) is exposed so the tests can point
at a mockito mock server without round-tripping through DNS.

3 unit tests cover the 200 happy path, the HTTP 5xx error path, and
the >5 MB body-cap rejection."
```

---

### Task 3: Catalog cache (disk + TTL)

**Files:**
- Create: `crates/ai/src/catalog/cache.rs`
- Create: `crates/ai/src/catalog/cache_tests.rs`
- Modify: `crates/ai/src/catalog/mod.rs` — `pub mod cache;`
- Possibly modify: `crates/ai/Cargo.toml` — add `dirs.workspace = true` if not present.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/agent_provider_secrets.rs` lines 1-50 — for the existing "save-to-disk, load-on-boot" idiom this repo uses for AI-related state.
- Repo-wide search: `rg "dirs::config_dir|directories::" crates app | head -5` — confirm which config-dir crate is in use; reuse it.

- [ ] **Step 3.1: Check existing config-dir resolution helper**

```bash
rg -n "dirs::config_dir|directories::|ProjectDirs|BaseDirs" crates app 2>&1 | head -10
```

Use the helper that surfaces. If `dirs` is in use, the cache path is `dirs::config_dir().join("warp").join("byop_catalog.json")` (or whatever the project root subdir is — match the existing helper exactly). If nothing is in use, add `dirs = "5.0"` to the workspace and the crate.

- [ ] **Step 3.2: Create `cache.rs`**

```rust
//! On-disk cache for the parsed catalog.
//!
//! Lifecycle:
//!   1. `CatalogCache::load_or_default()` reads the on-disk JSON. If the
//!      file doesn't exist or fails to parse, returns a `CatalogCache`
//!      backed by the baked-in snapshot with `fetched_at = None` so the
//!      next `needs_refresh()` call returns `true`.
//!   2. The settings page checks `needs_refresh()` on open and kicks off
//!      a background `fetch_catalog()` if stale; the UI renders against
//!      the in-memory copy in the meantime.
//!   3. When fetch completes, `replace_with_fresh(models)` rotates the
//!      in-memory state and calls `save()` to persist.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::parse::{CatalogError, CatalogModel};
use super::snapshot::BAKED_IN_SNAPSHOT;

const TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const CACHE_FILE_NAME: &str = "byop_catalog.json";

#[derive(Debug, Clone)]
pub struct CatalogCache {
    models: Vec<CatalogModel>,
    /// Unix seconds when the catalog was last successfully fetched.
    /// `None` when backed by the baked-in snapshot (forces a refresh).
    fetched_at: Option<u64>,
    /// Whether the in-memory copy is the baked-in snapshot (signals the
    /// settings UI to show a "using built-in fallback" caption).
    is_snapshot: bool,
}

/// On-disk serialization format. Versioned to allow future schema bumps
/// without invalidating existing caches silently.
#[derive(Debug, Serialize, Deserialize)]
struct OnDisk {
    version: u8,
    fetched_at: u64,
    models: Vec<CatalogModel>,
}

const ON_DISK_VERSION: u8 = 1;

impl CatalogCache {
    /// Load from `<config_dir>/<CACHE_FILE_NAME>` if present and valid;
    /// otherwise fall back to the baked-in snapshot.
    pub fn load_or_default() -> Self {
        match cache_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(body) => match serde_json::from_str::<OnDisk>(&body) {
                Ok(d) if d.version == ON_DISK_VERSION => Self {
                    models: d.models,
                    fetched_at: Some(d.fetched_at),
                    is_snapshot: false,
                },
                _ => Self::from_snapshot(),
            },
            None => Self::from_snapshot(),
        }
    }

    fn from_snapshot() -> Self {
        Self {
            models: BAKED_IN_SNAPSHOT.clone(),
            fetched_at: None,
            is_snapshot: true,
        }
    }

    pub fn all(&self) -> &[CatalogModel] {
        &self.models
    }

    pub fn is_snapshot(&self) -> bool {
        self.is_snapshot
    }

    pub fn fetched_at(&self) -> Option<u64> {
        self.fetched_at
    }

    /// Returns `true` when the cache is older than `TTL_SECONDS`, when no
    /// fetch has happened yet, or when the current copy is the snapshot.
    pub fn needs_refresh(&self) -> bool {
        if self.is_snapshot {
            return true;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match self.fetched_at {
            None => true,
            Some(t) => now.saturating_sub(t) > TTL_SECONDS,
        }
    }

    /// Replace the in-memory state with a fresh fetch and persist to disk.
    /// The persistence error is logged but not propagated — a failed save
    /// shouldn't break the user's session.
    pub fn replace_with_fresh(&mut self, models: Vec<CatalogModel>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.models = models;
        self.fetched_at = Some(now);
        self.is_snapshot = false;
        if let Err(e) = self.save() {
            log::warn!("catalog cache save failed: {e}");
        }
    }

    fn save(&self) -> Result<(), CatalogError> {
        let path = cache_path().ok_or_else(|| {
            CatalogError::Io("config dir unavailable".to_string())
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CatalogError::Io(format!("{e}")))?;
        }
        let on_disk = OnDisk {
            version: ON_DISK_VERSION,
            fetched_at: self.fetched_at.unwrap_or(0),
            models: self.models.clone(),
        };
        let body = serde_json::to_string_pretty(&on_disk)
            .map_err(|e| CatalogError::Parse(format!("{e}")))?;
        atomic_write(&path, &body).map_err(|e| CatalogError::Io(format!("{e}")))
    }

    /// Lookup by `(catalog_provider, model_id)`. Returns the first match.
    pub fn lookup(&self, catalog_provider: &str, model_id: &str) -> Option<&CatalogModel> {
        self.models
            .iter()
            .find(|m| m.catalog_provider == catalog_provider && m.id == model_id)
    }
}

fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("warp").join(CACHE_FILE_NAME))
}

/// Write `body` to `path` via a temp-file + rename so a crashed save
/// can't leave a half-written cache that fails to parse on the next boot.
fn atomic_write(path: &Path, body: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
```

`CatalogModel` must derive `Serialize` for the on-disk envelope to roundtrip; add that derive in Task 3.3 below.

- [ ] **Step 3.3: Add `Serialize` to `CatalogModel` in parse.rs**

Edit `crates/ai/src/catalog/parse.rs`:

```rust
// Change:
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel { ... }

// To:
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogModel { ... }
```

- [ ] **Step 3.4: Create `cache_tests.rs`**

```rust
use super::{CatalogCache, CatalogModel};

fn sample(id: &str, provider: &str) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: id.to_string(),
        context_window: Some(8000),
        max_output_tokens: Some(4000),
        tool_call: true,
        reasoning: false,
        image: false,
        pdf: false,
        audio: false,
        open_weights: false,
    }
}

#[test]
fn snapshot_default_signals_needs_refresh() {
    let cache = CatalogCache::load_or_default();
    // In a clean test env there is no on-disk cache, so this should be
    // the snapshot — which always needs a refresh.
    assert!(cache.needs_refresh());
    // The snapshot exposes at least one model — see Task 4 / snapshot.json.
    assert!(!cache.all().is_empty());
}

#[test]
fn replace_with_fresh_marks_not_snapshot_and_not_stale() {
    let mut cache = CatalogCache::load_or_default();
    cache.replace_with_fresh(vec![sample("m1", "openai")]);
    assert!(!cache.is_snapshot());
    assert!(!cache.needs_refresh(), "fresh cache shouldn't need refresh");
    assert_eq!(cache.all().len(), 1);
    assert_eq!(cache.lookup("openai", "m1").unwrap().id, "m1");
}

#[test]
fn lookup_returns_none_for_unknown_provider_or_id() {
    let mut cache = CatalogCache::load_or_default();
    cache.replace_with_fresh(vec![sample("m1", "openai")]);
    assert!(cache.lookup("openai", "missing").is_none());
    assert!(cache.lookup("missing", "m1").is_none());
}
```

- [ ] **Step 3.5: Wire into mod.rs + build + run tests**

```bash
# Append to crates/ai/src/catalog/mod.rs:
#   pub mod cache;
#   pub use cache::CatalogCache;
cargo nextest run -p ai catalog::cache 2>&1 | tail -10   # 3/3 passed
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 3.6: Commit**

```bash
git add crates/ai/src/catalog/cache.rs crates/ai/src/catalog/cache_tests.rs \
        crates/ai/src/catalog/parse.rs crates/ai/src/catalog/mod.rs
git commit -m "feat(ai/catalog): on-disk cache with 7-day TTL + atomic write

Phase 4b stage A task 3. Adds CatalogCache::{load_or_default, save,
replace_with_fresh, needs_refresh, lookup}. Persists the parsed catalog
to <config_dir>/warp/byop_catalog.json as a versioned envelope
{ version: 1, fetched_at: <epoch_secs>, models: [...] }. Atomic save
via temp-file + rename so a crashed write can't leave a half-flushed
cache. needs_refresh() returns true when older than 7 days, when the
in-memory copy is the baked-in snapshot, or when the timestamp is 0.

CatalogModel gains Serialize + Deserialize derives so the on-disk
envelope can roundtrip without a manual translator.

3 unit tests cover snapshot-default-needs-refresh, replace-marks-fresh,
and lookup misses."
```

---

### Task 4: Baked-in snapshot

**Files:**
- Create: `crates/ai/src/catalog/snapshot.rs`
- Create: `crates/ai/src/catalog/snapshot.json`
- Modify: `crates/ai/src/catalog/mod.rs` — `pub mod snapshot;`

- [ ] **Step 4.1: Create `snapshot.json`** with ~30 curated entries

The snapshot is a one-shot capture of the most common models across the 5 active api_types. Use the same wire format models.dev exposes so the existing parser handles it. Drop fields the parser ignores (cost, dates, etc.) to keep the bundled JSON small.

```json
{
  "openai": {
    "id": "openai",
    "name": "OpenAI",
    "models": {
      "gpt-4o":      {"id":"gpt-4o","name":"GPT-4o","tool_call":true,"modalities":{"input":["text","image","audio"],"output":["text"]},"limit":{"context":128000,"output":16384}},
      "gpt-4o-mini": {"id":"gpt-4o-mini","name":"GPT-4o Mini","tool_call":true,"modalities":{"input":["text","image"],"output":["text"]},"limit":{"context":128000,"output":16384}},
      "gpt-4-turbo": {"id":"gpt-4-turbo","name":"GPT-4 Turbo","tool_call":true,"modalities":{"input":["text","image"],"output":["text"]},"limit":{"context":128000,"output":4096}},
      "o1":          {"id":"o1","name":"o1","tool_call":true,"reasoning":true,"modalities":{"input":["text","image"],"output":["text"]},"limit":{"context":200000,"output":100000}},
      "o1-mini":     {"id":"o1-mini","name":"o1 Mini","tool_call":true,"reasoning":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":128000,"output":65536}}
    }
  },
  "anthropic": {
    "id": "anthropic",
    "name": "Anthropic",
    "models": {
      "claude-opus-4-7":       {"id":"claude-opus-4-7","name":"Claude Opus 4.7","tool_call":true,"reasoning":true,"modalities":{"input":["text","image","pdf"],"output":["text"]},"limit":{"context":1000000,"output":128000}},
      "claude-sonnet-4-6":     {"id":"claude-sonnet-4-6","name":"Claude Sonnet 4.6","tool_call":true,"reasoning":true,"modalities":{"input":["text","image","pdf"],"output":["text"]},"limit":{"context":1000000,"output":128000}},
      "claude-haiku-4-5":      {"id":"claude-haiku-4-5","name":"Claude Haiku 4.5","tool_call":true,"modalities":{"input":["text","image","pdf"],"output":["text"]},"limit":{"context":200000,"output":8192}},
      "claude-3-5-sonnet-20241022": {"id":"claude-3-5-sonnet-20241022","name":"Claude 3.5 Sonnet (2024-10-22)","tool_call":true,"modalities":{"input":["text","image","pdf"],"output":["text"]},"limit":{"context":200000,"output":8192}},
      "claude-3-5-haiku-20241022": {"id":"claude-3-5-haiku-20241022","name":"Claude 3.5 Haiku (2024-10-22)","tool_call":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":200000,"output":8192}}
    }
  },
  "google": {
    "id": "google",
    "name": "Google",
    "models": {
      "gemini-2-pro":      {"id":"gemini-2-pro","name":"Gemini 2 Pro","tool_call":true,"modalities":{"input":["text","image","pdf","audio"],"output":["text"]},"limit":{"context":2000000,"output":8192}},
      "gemini-2-flash":    {"id":"gemini-2-flash","name":"Gemini 2 Flash","tool_call":true,"modalities":{"input":["text","image","pdf","audio"],"output":["text"]},"limit":{"context":1000000,"output":8192}},
      "gemini-1.5-pro":    {"id":"gemini-1.5-pro","name":"Gemini 1.5 Pro","tool_call":true,"modalities":{"input":["text","image","pdf","audio"],"output":["text"]},"limit":{"context":2000000,"output":8192}},
      "gemini-1.5-flash":  {"id":"gemini-1.5-flash","name":"Gemini 1.5 Flash","tool_call":true,"modalities":{"input":["text","image","pdf","audio"],"output":["text"]},"limit":{"context":1000000,"output":8192}}
    }
  },
  "deepseek": {
    "id": "deepseek",
    "name": "DeepSeek",
    "models": {
      "deepseek-chat":      {"id":"deepseek-chat","name":"DeepSeek Chat","tool_call":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":65536,"output":8192}},
      "deepseek-reasoner":  {"id":"deepseek-reasoner","name":"DeepSeek Reasoner","tool_call":true,"reasoning":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":65536,"output":8192}}
    }
  },
  "meta": {
    "id": "meta",
    "name": "Meta (Ollama)",
    "models": {
      "llama-3.3-70b":  {"id":"llama-3.3-70b","name":"Llama 3.3 70B","tool_call":true,"open_weights":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":128000,"output":4096}},
      "llama-3.1-70b":  {"id":"llama-3.1-70b","name":"Llama 3.1 70B","tool_call":true,"open_weights":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":128000,"output":4096}},
      "llama-3.1-8b":   {"id":"llama-3.1-8b","name":"Llama 3.1 8B","tool_call":true,"open_weights":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":128000,"output":4096}}
    }
  },
  "alibaba": {
    "id": "alibaba",
    "name": "Alibaba (Ollama)",
    "models": {
      "qwen2.5-coder-32b": {"id":"qwen2.5-coder-32b","name":"Qwen 2.5 Coder 32B","tool_call":true,"open_weights":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":131072,"output":4096}},
      "qwen2.5-coder-7b":  {"id":"qwen2.5-coder-7b","name":"Qwen 2.5 Coder 7B","tool_call":true,"open_weights":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":131072,"output":4096}}
    }
  },
  "mistral": {
    "id": "mistral",
    "name": "Mistral (Ollama)",
    "models": {
      "mistral-small":  {"id":"mistral-small","name":"Mistral Small","tool_call":true,"open_weights":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":32768,"output":8192}}
    }
  }
}
```

- [ ] **Step 4.2: Create `snapshot.rs`** that compiles the JSON into `BAKED_IN_SNAPSHOT`

```rust
//! Baked-in catalog snapshot — last-resort fallback when both the
//! on-disk cache and the live fetch fail. Source: `snapshot.json` in
//! this directory, parsed at startup via the same `parse_catalog`
//! helper that processes a fresh fetch.

use std::sync::LazyLock;

use super::parse::{parse_catalog, CatalogModel};

const SNAPSHOT_JSON: &str = include_str!("snapshot.json");

pub static BAKED_IN_SNAPSHOT: LazyLock<Vec<CatalogModel>> = LazyLock::new(|| {
    parse_catalog(SNAPSHOT_JSON)
        .expect("baked-in catalog snapshot must parse — fix snapshot.json")
});
```

- [ ] **Step 4.3: Wire into mod.rs + verify the parse succeeds at boot**

Append `pub mod snapshot;` to `crates/ai/src/catalog/mod.rs`.

```bash
cargo build -p ai 2>&1 | tail -5   # build verifies snapshot.json parses

# Add one quick smoke test to confirm the snapshot is non-empty and contains the
# expected api_types. Append to cache_tests.rs:
```

```rust
#[test]
fn baked_in_snapshot_covers_all_active_api_types() {
    let cache = CatalogCache::load_or_default();
    let models = cache.all();
    assert!(models.iter().any(|m| m.catalog_provider == "openai"));
    assert!(models.iter().any(|m| m.catalog_provider == "anthropic"));
    assert!(models.iter().any(|m| m.catalog_provider == "google"));
    assert!(models.iter().any(|m| m.catalog_provider == "deepseek"));
    assert!(models.iter().any(|m| m.open_weights));
}
```

```bash
cargo nextest run -p ai catalog::cache 2>&1 | tail -10   # 4/4 passed
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 4.4: Commit**

```bash
git add crates/ai/src/catalog/snapshot.rs crates/ai/src/catalog/snapshot.json \
        crates/ai/src/catalog/cache_tests.rs crates/ai/src/catalog/mod.rs
git commit -m "feat(ai/catalog): baked-in snapshot fallback

Phase 4b stage A task 4. Bundles ~25 curated catalog entries across
openai / anthropic / google / deepseek / meta / alibaba / mistral
(the last three open_weights for the Ollama filter) so first-launch,
offline, and 'both cache + live fetch failed' paths still surface a
usable model list. The snapshot uses the same wire format models.dev
publishes so the existing parse_catalog handles it.

BAKED_IN_SNAPSHOT is a LazyLock<Vec<CatalogModel>> parsed once at
first access; a malformed snapshot.json fails the assertion at
startup (build verifies via cargo build -p ai)."
```

---

### Task 5: Catalog mod.rs public API + filter map + lookup

**Files:**
- Modify: `crates/ai/src/catalog/mod.rs` — consolidate the public API.
- Create: `crates/ai/src/catalog/mod_tests.rs` — tests for the api_type filter map.

- [ ] **Step 5.1: Replace the temporary `mod.rs` with the full public API**

```rust
//! BYOP model catalog (Phase 4b).
//!
//! Sourced from `https://models.dev/api.json` (a curated open-source
//! catalog covering every major commercial provider plus the most-common
//! open-weight models). A `CatalogCache` persists to disk with a 7-day
//! TTL; a baked-in snapshot ships in the binary as a last-resort fallback.
//!
//! Public consumers (the AgentProvidersWidget for inline chips and the
//! Browse-catalog modal) call `lookup_catalog_provider(api_type)` to map
//! a user-configured `AgentProviderApiType` onto the catalog provider id
//! used for chip filtering, then iterate `CatalogCache::all()` to render
//! the matched rows.

pub mod cache;
pub mod fetch;
pub mod parse;
pub mod snapshot;
pub mod wire;

pub use cache::CatalogCache;
pub use fetch::fetch_catalog;
pub use parse::{parse_catalog, CatalogError, CatalogModel};

use super::local_provider::AgentProviderApiType;

/// Map a Warp `AgentProviderApiType` onto the catalog provider id (the
/// top-level key in `models.dev/api.json`). Returns `None` for api_types
/// the catalog doesn't model directly (`OpenAiResp`) — those callers
/// fall through to the "All providers" filter in the Browse-catalog
/// modal and don't render inline chips.
///
/// `Ollama` is special: it has no single catalog provider key because
/// Ollama hosts open-weight models from every upstream. Callers handle
/// the Ollama case by iterating `CatalogCache::all().filter(|m| m.open_weights)`
/// instead of by provider id; this helper returns `None` so the api-type
/// table doesn't try to map it.
pub fn lookup_catalog_provider(api_type: AgentProviderApiType) -> Option<&'static str> {
    match api_type {
        AgentProviderApiType::OpenAi => Some("openai"),
        AgentProviderApiType::Anthropic => Some("anthropic"),
        AgentProviderApiType::Gemini => Some("google"),
        AgentProviderApiType::DeepSeek => Some("deepseek"),
        AgentProviderApiType::Ollama => None,
        AgentProviderApiType::OpenAiResp => None,
    }
}

/// Filter `models` to entries matching the given `api_type`. Ollama gets
/// the open-weights union; OpenAiResp gets nothing.
pub fn filter_models_for_api_type<'a>(
    api_type: AgentProviderApiType,
    models: &'a [CatalogModel],
) -> Vec<&'a CatalogModel> {
    match api_type {
        AgentProviderApiType::Ollama => {
            models.iter().filter(|m| m.open_weights).collect()
        }
        AgentProviderApiType::OpenAiResp => Vec::new(),
        other => match lookup_catalog_provider(other) {
            Some(provider) => models
                .iter()
                .filter(|m| m.catalog_provider == provider)
                .collect(),
            None => Vec::new(),
        },
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
```

- [ ] **Step 5.2: Create `mod_tests.rs`**

```rust
use super::{filter_models_for_api_type, lookup_catalog_provider, CatalogModel};
use crate::local_provider::AgentProviderApiType;

fn m(id: &str, provider: &str, open_weights: bool) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: id.to_string(),
        context_window: None,
        max_output_tokens: None,
        tool_call: true,
        reasoning: false,
        image: false,
        pdf: false,
        audio: false,
        open_weights,
    }
}

#[test]
fn lookup_catalog_provider_known_mappings() {
    assert_eq!(
        lookup_catalog_provider(AgentProviderApiType::OpenAi),
        Some("openai")
    );
    assert_eq!(
        lookup_catalog_provider(AgentProviderApiType::Anthropic),
        Some("anthropic")
    );
    assert_eq!(
        lookup_catalog_provider(AgentProviderApiType::Gemini),
        Some("google")
    );
    assert_eq!(
        lookup_catalog_provider(AgentProviderApiType::DeepSeek),
        Some("deepseek")
    );
    assert_eq!(lookup_catalog_provider(AgentProviderApiType::Ollama), None);
    assert_eq!(
        lookup_catalog_provider(AgentProviderApiType::OpenAiResp),
        None
    );
}

#[test]
fn filter_for_openai_returns_only_openai_rows() {
    let models = vec![
        m("gpt-4o", "openai", false),
        m("claude-opus", "anthropic", false),
        m("llama", "meta", true),
    ];
    let v = filter_models_for_api_type(AgentProviderApiType::OpenAi, &models);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].id, "gpt-4o");
}

#[test]
fn filter_for_ollama_returns_open_weights_union() {
    let models = vec![
        m("gpt-4o", "openai", false),
        m("llama-3", "meta", true),
        m("qwen", "alibaba", true),
        m("mistral-small", "mistral", true),
    ];
    let v = filter_models_for_api_type(AgentProviderApiType::Ollama, &models);
    assert_eq!(v.len(), 3);
    let ids: Vec<&str> = v.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"llama-3") && ids.contains(&"qwen") && ids.contains(&"mistral-small"));
}

#[test]
fn filter_for_openai_resp_returns_empty() {
    let models = vec![m("gpt-4o", "openai", false)];
    let v = filter_models_for_api_type(AgentProviderApiType::OpenAiResp, &models);
    assert!(v.is_empty());
}
```

- [ ] **Step 5.3: Build + run + commit**

```bash
cargo nextest run -p ai catalog 2>&1 | tail -10   # 8 + 3 + 4 + 4 = ~19 catalog tests, all green
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
git add crates/ai/src/catalog/mod.rs crates/ai/src/catalog/mod_tests.rs
git commit -m "feat(ai/catalog): public API + api_type filter map

Phase 4b stage A task 5. Stabilizes the catalog module's public
surface: re-exports CatalogCache, fetch_catalog, parse_catalog,
CatalogModel, CatalogError; adds lookup_catalog_provider(api_type)
and filter_models_for_api_type(api_type, models) helpers used by
the widget chip + modal rendering.

Ollama is special: no single catalog provider key, because Ollama
hosts open-weight models from every upstream. lookup_catalog_provider
returns None for Ollama; filter_models_for_api_type handles the
case by returning the union of open_weights==true rows across the
full catalog.

4 unit tests cover the known mappings, OpenAi filtering, Ollama
open-weights union, and OpenAiResp returning empty."
```

---

## Stage B — Settings action wiring

### Task 6: `CatalogModalState` pure-logic module

**Files:**
- Create: `app/src/settings_view/catalog_modal.rs`
- Create: `app/src/settings_view/catalog_modal_tests.rs`
- Modify: `app/src/settings_view/mod.rs` — `mod catalog_modal;`

**Read these reference files FIRST:**
- `app/src/settings_view/fetched_models_modal.rs` — the Phase 4a sibling this file mirrors.
- `app/src/settings_view/fetched_models_modal_tests.rs` — the test pattern.

- [ ] **Step 6.1: Create `catalog_modal.rs`**

```rust
//! Pure state for the "Browse catalog" modal opened by the Browse
//! catalog button in `AgentProvidersWidget` (Phase 4b). Mirrors the
//! Phase 4a `FetchedModelsModalState` pattern: the modal-state
//! transitions live in this module so they can be unit-tested without
//! a GPUI `ViewContext`, and the handler arms in `ai_page.rs` are thin
//! glue over the helpers here.

use std::collections::HashSet;

use ai::catalog::CatalogModel;

use crate::settings::AgentProviderModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogFilter {
    /// Filter to entries matching the provider card's `api_type`
    /// (default — shows ~10 relevant rows out of the box).
    ThisProvider,
    /// Show every catalog entry, ordered by catalog_provider then id.
    /// Used for OpenAiResp providers (which have no chip suggestions)
    /// and for users who want to add a model from an unexpected source.
    AllProviders,
}

#[derive(Debug, Clone)]
pub struct CatalogModalState {
    pub provider_index: usize,
    /// Captured at open time; if the provider was removed mid-modal,
    /// the commit handler drops the action.
    pub provider_id: String,
    pub filter: CatalogFilter,
    /// Free-form search; matched case-insensitively against `id` + `name`.
    pub search: String,
    /// Model ids currently checked. Default: empty (catalog browsing is
    /// opt-in row-by-row; default-checking would surprise users).
    pub checked: HashSet<String>,
    /// Model ids already on the provider; rendered dimmed and not
    /// commitable. Captured at modal-open time so the user sees a stable
    /// view even if they navigate.
    pub already_added: HashSet<String>,
}

impl CatalogModalState {
    pub fn new(provider_index: usize, provider_id: String, already_added: HashSet<String>) -> Self {
        Self {
            provider_index,
            provider_id,
            filter: CatalogFilter::ThisProvider,
            search: String::new(),
            checked: HashSet::new(),
            already_added,
        }
    }

    pub fn toggle(&mut self, model_id: &str, checked: bool) {
        if self.already_added.contains(model_id) {
            return;
        }
        if checked {
            self.checked.insert(model_id.to_owned());
        } else {
            self.checked.remove(model_id);
        }
    }

    pub fn set_filter(&mut self, filter: CatalogFilter) {
        self.filter = filter;
    }

    pub fn set_search(&mut self, search: String) {
        self.search = search;
    }

    /// Returns a sub-slice of `available` matching the current filter +
    /// search. The caller (the widget render path) does the filtering;
    /// this helper just encodes the predicates so the test surface is
    /// the same logic the widget uses.
    pub fn matches_search<'a>(&self, model: &'a CatalogModel) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let needle = self.search.to_ascii_lowercase();
        model.id.to_ascii_lowercase().contains(&needle)
            || model.name.to_ascii_lowercase().contains(&needle)
    }

    /// Build the AgentProviderModel rows to commit. Filters unchecked
    /// rows and (defensively) already-added rows.
    pub fn committed_rows<'a, I>(&self, available: I) -> Vec<AgentProviderModel>
    where
        I: IntoIterator<Item = &'a CatalogModel>,
    {
        available
            .into_iter()
            .filter(|m| self.checked.contains(&m.id))
            .filter(|m| !self.already_added.contains(&m.id))
            .map(|c| AgentProviderModel {
                name: c.name.clone(),
                id: c.id.clone(),
                context_window: c.context_window.unwrap_or(0),
                max_output_tokens: c.max_output_tokens.unwrap_or(0),
                reasoning: c.reasoning,
                tool_call: c.tool_call,
                image: if c.image { Some(true) } else { None },
                pdf: if c.pdf { Some(true) } else { None },
                audio: if c.audio { Some(true) } else { None },
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "catalog_modal_tests.rs"]
mod tests;
```

- [ ] **Step 6.2: Create `catalog_modal_tests.rs` with eight tests**

```rust
use std::collections::HashSet;

use ai::catalog::CatalogModel;

use super::{CatalogFilter, CatalogModalState};

fn catalog(id: &str, provider: &str) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: format!("Display {id}"),
        context_window: Some(8000),
        max_output_tokens: Some(4000),
        tool_call: true,
        reasoning: false,
        image: false,
        pdf: false,
        audio: false,
        open_weights: false,
    }
}

#[test]
fn new_state_has_empty_checked_and_this_provider_filter() {
    let s = CatalogModalState::new(0, "prov-1".into(), HashSet::new());
    assert!(s.checked.is_empty());
    assert_eq!(s.filter, CatalogFilter::ThisProvider);
    assert!(s.search.is_empty());
}

#[test]
fn toggle_flips_state_and_skips_already_added() {
    let mut already = HashSet::new();
    already.insert("m1".to_string());
    let mut s = CatalogModalState::new(0, "prov-1".into(), already);
    s.toggle("m1", true); // already-added, ignored
    assert!(!s.checked.contains("m1"));
    s.toggle("m2", true);
    assert!(s.checked.contains("m2"));
    s.toggle("m2", false);
    assert!(!s.checked.contains("m2"));
}

#[test]
fn set_filter_switches_between_modes() {
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    s.set_filter(CatalogFilter::AllProviders);
    assert_eq!(s.filter, CatalogFilter::AllProviders);
}

#[test]
fn search_empty_matches_all() {
    let s = CatalogModalState::new(0, "p".into(), HashSet::new());
    assert!(s.matches_search(&catalog("anything", "openai")));
}

#[test]
fn search_matches_case_insensitive_id_or_name() {
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    s.set_search("OPUS".into());
    assert!(s.matches_search(&catalog("claude-opus-4-7", "anthropic")));
    assert!(s.matches_search(&CatalogModel {
        name: "Claude Opus".into(),
        ..catalog("x", "anthropic")
    }));
    assert!(!s.matches_search(&catalog("gpt-4o", "openai")));
}

#[test]
fn committed_rows_lifts_capability_flags() {
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    let c = CatalogModel {
        image: true,
        pdf: false,
        audio: true,
        ..catalog("m1", "openai")
    };
    s.toggle("m1", true);
    let rows = s.committed_rows(std::iter::once(&c));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].image, Some(true));
    assert_eq!(rows[0].pdf, None);
    assert_eq!(rows[0].audio, Some(true));
}

#[test]
fn committed_rows_skips_already_added_even_if_checked() {
    let mut already = HashSet::new();
    already.insert("m1".to_string());
    let mut s = CatalogModalState::new(0, "p".into(), already);
    s.checked.insert("m1".to_string()); // defensive: shouldn't happen via toggle
    let c = catalog("m1", "openai");
    let rows = s.committed_rows(std::iter::once(&c));
    assert!(rows.is_empty());
}

#[test]
fn committed_rows_fills_zero_when_metadata_is_none() {
    let c = CatalogModel {
        context_window: None,
        max_output_tokens: None,
        ..catalog("m1", "openai")
    };
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    s.toggle("m1", true);
    let rows = s.committed_rows(std::iter::once(&c));
    assert_eq!(rows[0].context_window, 0);
    assert_eq!(rows[0].max_output_tokens, 0);
}
```

- [ ] **Step 6.3: Wire into mod.rs**

In `app/src/settings_view/mod.rs`, alongside the existing `mod fetched_models_modal;`:

```rust
mod catalog_modal;
```

- [ ] **Step 6.4: Build + test + clippy + commit**

```bash
cargo nextest run -p warp --lib catalog_modal 2>&1 | tail -10   # 8/8 passed
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
git add app/src/settings_view/catalog_modal.rs app/src/settings_view/catalog_modal_tests.rs \
        app/src/settings_view/mod.rs
git commit -m "feat(app/settings_view/catalog_modal): pure-logic modal state

Phase 4b stage B task 6. Adds CatalogModalState with new(), toggle(),
set_filter(), set_search(), matches_search(), and committed_rows().
Mirrors the Phase 4a FetchedModelsModalState pattern so the state
transitions are unit-testable without a GPUI ViewContext.

committed_rows lifts catalog capability booleans (image/pdf/audio)
into the AgentProviderModel three-state Option<bool> shape:
present-and-true → Some(true), absent → None. The 'forced off'
state (Some(false)) is reserved for user overrides via the row
toggle chips that ship in Phase 4c.

8 unit tests cover construction, toggle skipping already-added,
filter switching, search empty/case-insensitive/no-match, and
committed_rows capability lift + already-added skip + zero defaults."
```

---

### Task 7: `AISettingsPageAction` variants + page-view state + handlers

**Files:**
- Modify: `app/src/settings_view/ai_page.rs` — 9 action variants, 3 view fields, 9 handler arms.

**Read these reference files FIRST:**
- `app/src/settings_view/ai_page.rs` around `pub enum AISettingsPageAction` — the existing variants and where Phase 4a's 5 variants live.
- `app/src/settings_view/ai_page.rs` Phase 4a handler arms for `FetchAgentProviderModels` and friends — the ctx.spawn + inline-resolve pattern. Phase 4b's catalog-load handler uses the same idiom.

- [ ] **Step 7.1: Add 9 new action variants**

Append to `AISettingsPageAction` (after the Phase 4a `CancelFetchedAgentProviderModelsModal` variant):

```rust
/// Phase 4b. Lazy-load the catalog on demand. Dispatched by the
/// widget the first time any catalog-consuming render path needs
/// it (inline chip render or Browse catalog button click). The
/// handler kicks off a background `fetch_catalog()` if the cache
/// is stale and updates `view.catalog_cache` when it lands.
LoadCatalog,

/// Phase 4b. Force a fresh fetch even if the cache is still warm.
/// Dispatched by the "Refresh catalog" button in the modal.
RefreshCatalog,

/// Phase 4b. User clicked an inline quick-add chip in an empty
/// "+ Add Model" row. Auto-fills the row's AgentProviderModel
/// fields from the catalog entry.
QuickAddCatalogModel {
    provider_index: usize,
    catalog_model_id: String,
},

/// Phase 4b. User clicked "Browse catalog" on a provider card.
/// Opens the modal scoped to that provider.
OpenCatalogModal {
    provider_index: usize,
},

/// Phase 4b. Esc / Cancel / Close — discards the modal.
CloseCatalogModal,

/// Phase 4b. User toggled a single row in the open catalog modal.
ToggleCatalogModelInModal {
    model_id: String,
    checked: bool,
},

/// Phase 4b. User clicked "This provider" / "All providers" filter.
SetCatalogModalFilter {
    filter: crate::settings_view::catalog_modal::CatalogFilter,
},

/// Phase 4b. User typed in the search input.
SetCatalogModalSearch {
    text: String,
},

/// Phase 4b. User clicked "Add N models" — commits the checked
/// catalog rows to the provider's models list.
CommitCatalogModelsFromModal {
    provider_index: usize,
},
```

- [ ] **Step 7.2: Add 3 new fields to `AISettingsPageView`**

Below the Phase 4a fields (`fetched_models_modal`, `fetch_models_in_flight`, `last_fetch_failure`):

```rust
/// Phase 4b. Lazy-loaded model catalog. `None` until the first
/// LoadCatalog action lands; populated from disk + the baked-in
/// snapshot on the first read, refreshed in the background if stale.
pub(super) catalog_cache: Option<ai::catalog::CatalogCache>,
/// Phase 4b. Currently-open Browse-catalog modal; mirrors Phase 4a's
/// fetched_models_modal pattern.
pub(super) catalog_modal:
    Option<super::catalog_modal::CatalogModalState>,
/// Phase 4b. Latest catalog-load failure (HTTP error, parse error,
/// etc.). Rendered as a dim caption on the Browse catalog button.
pub(super) catalog_load_failure: Option<String>,
```

…and in the constructor's `Self { ... }`:

```rust
catalog_cache: None,
catalog_modal: None,
catalog_load_failure: None,
```

- [ ] **Step 7.3: Add 9 handler arms**

Insert just after the Phase 4a `CancelFetchedAgentProviderModelsModal` handler:

```rust
AISettingsPageAction::LoadCatalog => {
    // Idempotent: if we already have a non-stale cache, no-op.
    if let Some(cache) = self.catalog_cache.as_ref() {
        if !cache.needs_refresh() {
            return;
        }
    } else {
        // First-time load: read from disk (or fall back to snapshot).
        self.catalog_cache = Some(ai::catalog::CatalogCache::load_or_default());
        // load_or_default may already be fresh; check before fetching.
        if !self
            .catalog_cache
            .as_ref()
            .map(|c| c.needs_refresh())
            .unwrap_or(true)
        {
            ctx.notify();
            return;
        }
    }
    self.catalog_load_failure = None;
    ctx.notify();

    let http = reqwest::Client::new();
    let _ = ctx.spawn(
        async move { ai::catalog::fetch_catalog(&http).await },
        move |this, outcome, ctx| {
            match outcome {
                Ok(models) => {
                    if let Some(cache) = this.catalog_cache.as_mut() {
                        cache.replace_with_fresh(models);
                    }
                    this.catalog_load_failure = None;
                }
                Err(e) => {
                    let msg = truncate_to_120(&format!("{e}"));
                    log::warn!("LoadCatalog: failed — {msg}");
                    this.catalog_load_failure = Some(msg);
                }
            }
            ctx.notify();
        },
    );
}

AISettingsPageAction::RefreshCatalog => {
    // Force a fetch regardless of cache age. Reuse the LoadCatalog
    // body by clearing the timestamp first.
    if let Some(cache) = self.catalog_cache.as_mut() {
        // Mark stale by replacing with a fresh-empty state that
        // needs_refresh() will surface as true; the next handler
        // body runs the fetch.
        *cache = ai::catalog::CatalogCache::load_or_default();
    }
    self.handle_action(&AISettingsPageAction::LoadCatalog, ctx);
}

AISettingsPageAction::QuickAddCatalogModel {
    provider_index,
    catalog_model_id,
} => {
    let provider_index = *provider_index;
    let catalog_model_id = catalog_model_id.clone();
    let Some(cache) = self.catalog_cache.as_ref() else {
        log::debug!("QuickAddCatalogModel: catalog not loaded yet, dropping");
        return;
    };
    // Find the chip's CatalogModel. Catalog provider lookup is by
    // (api_type → provider_id), then by model id within that subset.
    let providers = AISettings::as_ref(ctx).agent_providers.value().clone();
    let Some(provider) = providers.get(provider_index) else {
        return;
    };
    let candidate_set =
        ai::catalog::filter_models_for_api_type(provider.api_type, cache.all());
    let Some(catalog_model) = candidate_set.iter().find(|m| m.id == catalog_model_id) else {
        log::debug!(
            "QuickAddCatalogModel: model {catalog_model_id} not found for api_type \
             {:?}, dropping",
            provider.api_type
        );
        return;
    };
    let new_row = AgentProviderModel {
        name: catalog_model.name.clone(),
        id: catalog_model.id.clone(),
        context_window: catalog_model.context_window.unwrap_or(0),
        max_output_tokens: catalog_model.max_output_tokens.unwrap_or(0),
        reasoning: catalog_model.reasoning,
        tool_call: catalog_model.tool_call,
        image: if catalog_model.image { Some(true) } else { None },
        pdf: if catalog_model.pdf { Some(true) } else { None },
        audio: if catalog_model.audio { Some(true) } else { None },
    };
    AISettings::handle(ctx).update(ctx, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        if let Some(p) = providers.get_mut(provider_index) {
            p.models.push(new_row);
            report_if_error!(settings.agent_providers.set_value(providers, ctx));
        }
    });
    self.page = Self::build_page(self.active_subpage, ctx);
    ctx.notify();
}

AISettingsPageAction::OpenCatalogModal { provider_index } => {
    let provider_index = *provider_index;
    let providers = AISettings::as_ref(ctx).agent_providers.value().clone();
    let Some(provider) = providers.get(provider_index) else {
        log::warn!(
            "OpenCatalogModal: invalid provider_index {provider_index}"
        );
        return;
    };
    let already_added: std::collections::HashSet<String> =
        provider.models.iter().map(|m| m.id.clone()).collect();
    self.catalog_modal = Some(super::catalog_modal::CatalogModalState::new(
        provider_index,
        provider.id.clone(),
        already_added,
    ));
    // Kick off a catalog load if we don't have one yet.
    if self.catalog_cache.is_none() {
        self.handle_action(&AISettingsPageAction::LoadCatalog, ctx);
    }
    ctx.notify();
}

AISettingsPageAction::CloseCatalogModal => {
    self.catalog_modal = None;
    ctx.notify();
}

AISettingsPageAction::ToggleCatalogModelInModal { model_id, checked } => {
    if let Some(modal) = self.catalog_modal.as_mut() {
        modal.toggle(model_id, *checked);
        ctx.notify();
    }
}

AISettingsPageAction::SetCatalogModalFilter { filter } => {
    if let Some(modal) = self.catalog_modal.as_mut() {
        modal.set_filter(*filter);
        ctx.notify();
    }
}

AISettingsPageAction::SetCatalogModalSearch { text } => {
    if let Some(modal) = self.catalog_modal.as_mut() {
        modal.set_search(text.clone());
        ctx.notify();
    }
}

AISettingsPageAction::CommitCatalogModelsFromModal { provider_index } => {
    let provider_index = *provider_index;
    let Some(modal) = self.catalog_modal.take() else {
        return;
    };
    if modal.provider_index != provider_index {
        return;
    }
    let Some(cache) = self.catalog_cache.as_ref() else {
        return;
    };
    let providers = AISettings::as_ref(ctx).agent_providers.value().clone();
    let Some(provider) = providers.get(provider_index) else {
        return;
    };
    let candidate_set =
        ai::catalog::filter_models_for_api_type(provider.api_type, cache.all());
    let candidate_iter = candidate_set.iter().copied();
    let rows = modal.committed_rows(candidate_iter);
    if rows.is_empty() {
        ctx.notify();
        return;
    }
    AISettings::handle(ctx).update(ctx, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        if let Some(p) = providers.get_mut(provider_index) {
            p.models.extend(rows);
            report_if_error!(settings.agent_providers.set_value(providers, ctx));
        }
    });
    self.page = Self::build_page(self.active_subpage, ctx);
    ctx.notify();
}
```

The `truncate_to_120` helper is already in scope from Phase 4a — no new helper needed.

- [ ] **Step 7.4: Build + clippy + commit**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5

git add app/src/settings_view/ai_page.rs
git commit -m "feat(app/settings_view/ai_page): wire catalog actions + state

Phase 4b stage B task 7. Adds 9 new AISettingsPageAction variants
(LoadCatalog, RefreshCatalog, QuickAddCatalogModel, OpenCatalogModal,
CloseCatalogModal, ToggleCatalogModelInModal, SetCatalogModalFilter,
SetCatalogModalSearch, CommitCatalogModelsFromModal) + their handler
arms, three new AISettingsPageView fields (catalog_cache,
catalog_modal, catalog_load_failure), and the resolve logic for the
async catalog fetch (inlined in ctx.spawn's callback, matching
Phase 4a's pattern).

LoadCatalog is idempotent: it short-circuits when the cache is warm
and reads from disk + snapshot on first call. RefreshCatalog forces
a fetch regardless of TTL. QuickAddCatalogModel appends a fully
auto-filled AgentProviderModel including the multimodal flags
lifted from the catalog's modalities.input array. The catalog modal
follows the same open / toggle / commit / cancel pattern as 4a's
fetched-models modal."
```

---

## Stage C — Widget rendering

### Task 8: Inline quick-add chips

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs` — render chips below each model row's empty inputs.

**Read these reference files FIRST:**
- `agent_providers_widget.rs::render_model_row` — where each model row's UI is built today.
- `agent_providers_widget.rs::render_api_type_field` — for the existing chip-row pattern with `MouseStateHandle`s held in a `HashMap`.

- [ ] **Step 8.1: Add quick-add chip mouse-state pool**

The chip row is rendered per-empty-model-row, but a single provider can have multiple empty rows (after multiple `+ Add Model` clicks without filling them in). Pre-allocate per-row chip pools sized to the number of catalog suggestions (5).

In `ModelRowHandles`:

```rust
/// Phase 4b. MouseStateHandles for up to 5 inline quick-add chips
/// rendered when the row is empty. Allocated at row-build time so
/// render never builds MouseStateHandle::default() inline.
quick_add_chip_states: [MouseStateHandle; 5],
```

Initialize in `build_model_row`:

```rust
quick_add_chip_states: [
    MouseStateHandle::default(),
    MouseStateHandle::default(),
    MouseStateHandle::default(),
    MouseStateHandle::default(),
    MouseStateHandle::default(),
],
```

- [ ] **Step 8.2: Render chips below empty rows**

After `render_model_row` returns its existing element, wrap it in a `Flex::column` that conditionally appends a chip row when `model.id.trim().is_empty() && model.name.trim().is_empty()` (the "fresh empty row" signal).

```rust
fn render_model_row(
    provider_index: usize,
    provider_api_type: AgentProviderApiType,
    model_index: usize,
    model: &AgentProviderModel,
    row: &ModelRowHandles,
    view: &AISettingsPageView,
    appearance: &Appearance,
) -> Box<dyn Element> {
    // ... existing per-row rendering body ...
    let row_element = /* the existing returned Box<dyn Element> */;

    if !model.id.trim().is_empty() || !model.name.trim().is_empty() {
        // Filled row — no chips.
        return row_element;
    }

    // Empty row — render up to 5 chip suggestions below.
    let Some(cache) = view.catalog_cache.as_ref() else {
        return row_element;   // catalog not loaded yet; lazy-load on first chip render path
    };
    let candidates =
        ai::catalog::filter_models_for_api_type(provider_api_type, cache.all());
    let suggestions: Vec<&ai::catalog::CatalogModel> =
        candidates.into_iter().take(5).collect();
    if suggestions.is_empty() {
        return row_element;
    }

    let mut chip_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_alignment(MainAxisAlignment::Start);
    for (i, c) in suggestions.iter().enumerate() {
        let chip = Self::render_card_button(
            format!("+ {}", c.name),
            row.quick_add_chip_states[i].clone(),
            AISettingsPageAction::QuickAddCatalogModel {
                provider_index,
                catalog_model_id: c.id.clone(),
            },
            appearance,
        );
        chip_row = chip_row.with_child(Container::new(chip).with_margin_right(6.).finish());
    }

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(row_element)
        .with_child(
            Container::new(chip_row.finish())
                .with_margin_top(4.)
                .finish(),
        )
        .finish()
}
```

Update the caller (`render_provider_card`) to pass `view` and `provider.api_type` through:

```rust
// Find the existing call to Self::render_model_row in render_provider_card
// and add the two new params:
let row_el = Self::render_model_row(
    provider_index,
    provider.api_type,
    model_index,
    model,
    row_handles,
    view,
    appearance,
);
```

- [ ] **Step 8.3: Kick off catalog lazy-load at widget construction**

In `AgentProvidersWidget::new(ctx)`, after building the cards, dispatch the load. `ViewContext` exposes `dispatch_typed_action_deferred` (used elsewhere in this file by the editor subscribe hooks):

```rust
pub(super) fn new(ctx: &mut ViewContext<AISettingsPageView>) -> Self {
    let providers = AISettings::as_ref(ctx).agent_providers.value().clone();
    let cards: Vec<ProviderCardHandles> = providers
        .iter()
        .enumerate()
        .map(|(provider_index, provider)| {
            Self::build_provider_card(provider, provider_index, ctx)
        })
        .collect();

    // Phase 4b. Trigger catalog load once at widget-build time so chips
    // are warm by the time the user clicks "+ Add Model". LoadCatalog is
    // idempotent against a warm cache so re-builds (e.g. after Add/Remove
    // provider) don't refetch.
    ctx.dispatch_typed_action_deferred(AISettingsPageAction::LoadCatalog);

    Self {
        add_button_state: MouseStateHandle::default(),
        cards,
        fetch_modal: FetchModalHandles::new(),
        catalog_modal: CatalogModalHandles::new(ctx),
    }
}
```

This runs whenever the page rebuilds the widget — which happens on the same actions that rebuild the cards (Add/Remove provider, Add/Remove model, Commit fetched models). Idempotent against a warm cache, so re-builds don't refetch.

- [ ] **Step 8.4: Build + clippy + commit**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5

git add app/src/settings_view/agent_providers_widget.rs
git commit -m "feat(app/settings_view/agent_providers_widget): quick-add chips

Phase 4b stage C task 8. Renders up to 5 catalog-derived chips below
each empty + Add Model row, filtered to the provider's api_type
(Ollama gets the open_weights union). Chips disappear the moment any
field is edited.

ModelRowHandles gains a [MouseStateHandle; 5] pool, pre-allocated at
row-build time so render never constructs MouseStateHandle::default()
inline (the repeated-init pitfall from CLAUDE.md).

The widget kicks off a deferred AISettingsPageAction::LoadCatalog on
first render so chips are warm by the time the user opens + Add
Model. Subsequent renders no-op because LoadCatalog is idempotent
against a warm cache."
```

---

### Task 9: "Browse catalog" button + modal panel

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs` — add Browse button, modal handles, modal render.

**Read these reference files FIRST:**
- The Phase 4a modal render (`render_fetched_models_modal`) — the parallel template this section mirrors.

- [ ] **Step 9.1: Add Browse button state to `ProviderCardHandles`**

```rust
struct ProviderCardHandles {
    // ... existing fields ...
    fetch_models_button_state: MouseStateHandle,
    /// Phase 4b. Mouse state for the "Browse catalog" footer button.
    browse_catalog_button_state: MouseStateHandle,
    // ... rest ...
}
```

…initialized to `MouseStateHandle::default()` in `build_provider_card`.

- [ ] **Step 9.2: Add `CatalogModalHandles` on `AgentProvidersWidget`**

Mirrors `FetchModalHandles` from Phase 4a. Pre-allocate enough handles for the catalog rows (the catalog can have hundreds of entries, but the modal applies the filter + search before rendering, so 200 is a safe ceiling — same as 4a's MAX_ENTRIES).

```rust
struct CatalogModalHandles {
    refresh_state: MouseStateHandle,
    filter_this_provider_state: MouseStateHandle,
    filter_all_providers_state: MouseStateHandle,
    cancel_state: MouseStateHandle,
    commit_state: MouseStateHandle,
    row_states: Vec<MouseStateHandle>,
    search_editor: ViewHandle<EditorView>,
}

impl CatalogModalHandles {
    fn new(ctx: &mut ViewContext<AISettingsPageView>) -> Self {
        let search_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("Search by id or name", ctx);
            editor
        });
        ctx.subscribe_to_view(&search_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::TextChanged) {
                let text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(
                    AISettingsPageAction::SetCatalogModalSearch { text },
                );
            }
        });
        Self {
            refresh_state: MouseStateHandle::default(),
            filter_this_provider_state: MouseStateHandle::default(),
            filter_all_providers_state: MouseStateHandle::default(),
            cancel_state: MouseStateHandle::default(),
            commit_state: MouseStateHandle::default(),
            row_states: (0..200).map(|_| MouseStateHandle::default()).collect(),
            search_editor,
        }
    }
}
```

If `EditorEvent::TextChanged` doesn't exist, fall back to `Blurred | Enter` like the existing editors — the search will only update on blur/Enter instead of live, which is acceptable.

Add it to the widget:

```rust
pub(super) struct AgentProvidersWidget {
    // ... existing fields ...
    fetch_modal: FetchModalHandles,
    catalog_modal: CatalogModalHandles,
}
```

Initialize `catalog_modal: CatalogModalHandles::new(ctx)` in `AgentProvidersWidget::new`.

- [ ] **Step 9.3: Render the Browse catalog button in the footer**

In `render_provider_card`, alongside the existing Fetch / Test connection / Remove buttons:

```rust
let browse_catalog_button = Self::render_card_button(
    "Browse catalog",
    card.browse_catalog_button_state.clone(),
    AISettingsPageAction::OpenCatalogModal { provider_index },
    appearance,
);

// Add it to the left_buttons row, after fetch_models_button:
let left_buttons = Flex::row()
    .with_cross_axis_alignment(CrossAxisAlignment::Center)
    .with_child(add_model_button)
    .with_child(Container::new(test_connection_button).with_margin_left(8.).finish())
    .with_child(Container::new(fetch_models_button).with_margin_left(8.).finish())
    .with_child(Container::new(browse_catalog_button).with_margin_left(8.).finish())
    .finish();
```

- [ ] **Step 9.4: Render the modal panel**

Add a `render_catalog_modal` method mirroring `render_fetched_models_modal` from Phase 4a, and invoke it in the main `render` between the description and the cards (alongside the existing 4a modal call):

```rust
fn render_catalog_modal(
    &self,
    modal: &super::catalog_modal::CatalogModalState,
    providers: &[AgentProvider],
    catalog: &[ai::catalog::CatalogModel],
    catalog_load_failure: Option<&String>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let provider = providers.get(modal.provider_index);
    let api_type = provider.map(|p| p.api_type);
    let provider_label = provider
        .map(|p| {
            if p.name.is_empty() {
                "(unnamed provider)".to_string()
            } else {
                p.name.clone()
            }
        })
        .unwrap_or_else(|| "(removed provider)".into());
    let header_node = Container::new(
        Text::new(
            format!("Browse catalog — {provider_label}"),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(appearance.theme().active_ui_text_color().into())
        .finish(),
    )
    .with_margin_bottom(8.)
    .finish();

    // Filter + search the catalog rows for the current modal state.
    let filtered: Vec<&ai::catalog::CatalogModel> = match modal.filter {
        super::catalog_modal::CatalogFilter::ThisProvider => match api_type {
            Some(at) => ai::catalog::filter_models_for_api_type(at, catalog),
            None => Vec::new(),
        },
        super::catalog_modal::CatalogFilter::AllProviders => catalog.iter().collect(),
    };
    let filtered: Vec<&ai::catalog::CatalogModel> = filtered
        .into_iter()
        .filter(|m| modal.matches_search(m))
        .collect();

    // Build the filter chip row.
    let filter_chip = |label: &str, active: bool, state: MouseStateHandle, filter| {
        let prefix = if active { "● " } else { "" };
        Self::render_card_button(
            format!("{prefix}{label}"),
            state,
            AISettingsPageAction::SetCatalogModalFilter { filter },
            appearance,
        )
    };
    let filter_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(filter_chip(
            "This provider",
            modal.filter == super::catalog_modal::CatalogFilter::ThisProvider,
            self.catalog_modal.filter_this_provider_state.clone(),
            super::catalog_modal::CatalogFilter::ThisProvider,
        ))
        .with_child(
            Container::new(filter_chip(
                "All providers",
                modal.filter == super::catalog_modal::CatalogFilter::AllProviders,
                self.catalog_modal.filter_all_providers_state.clone(),
                super::catalog_modal::CatalogFilter::AllProviders,
            ))
            .with_margin_left(6.)
            .finish(),
        )
        .with_child(
            Container::new(Self::render_card_button(
                "Refresh",
                self.catalog_modal.refresh_state.clone(),
                AISettingsPageAction::RefreshCatalog,
                appearance,
            ))
            .with_margin_left(12.)
            .finish(),
        )
        .finish();

    // Search input.
    let search_input = Container::new(ChildView::new(&self.catalog_modal.search_editor).finish())
        .with_margin_top(8.)
        .with_margin_bottom(8.)
        .finish();

    // Caption: counts + load-failure indicator.
    let mut caption_segments = vec![format!("{} model(s) match", filtered.len())];
    if !modal.already_added.is_empty() {
        caption_segments.push(format!(
            "{} already on this provider",
            modal.already_added.len()
        ));
    }
    if let Some(reason) = catalog_load_failure {
        let excerpt: String = reason.chars().take(80).collect();
        caption_segments.push(format!("(load failed: {excerpt})"));
    }
    let caption_node = Container::new(
        Text::new(
            caption_segments.join(" · "),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(appearance.theme().disabled_ui_text_color().into())
        .soft_wrap(true)
        .finish(),
    )
    .with_margin_bottom(8.)
    .finish();

    // Row list.
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header_node)
        .with_child(filter_row)
        .with_child(search_input)
        .with_child(caption_node);

    for (row_index, model) in filtered.iter().take(200).enumerate() {
        let is_already = modal.already_added.contains(&model.id);
        let is_checked = modal.checked.contains(&model.id);
        let metadata = match (model.context_window, model.max_output_tokens) {
            (Some(c), Some(o)) => format!("  · {c} ctx · {o} out"),
            (Some(c), None) => format!("  · {c} ctx"),
            (None, Some(o)) => format!("  · {o} out"),
            (None, None) => String::new(),
        };
        let label = if is_already {
            format!("✓ {} ({}){metadata}", model.id, model.catalog_provider)
        } else if is_checked {
            format!("☑ {} ({}){metadata}", model.id, model.catalog_provider)
        } else {
            format!("☐ {} ({}){metadata}", model.id, model.catalog_provider)
        };

        let row_element: Box<dyn Element> = if is_already {
            Container::new(
                Text::new(label, appearance.ui_font_family(), CARD_BUTTON_FONT_SIZE)
                    .with_color(appearance.theme().disabled_ui_text_color().into())
                    .finish(),
            )
            .with_uniform_padding(CARD_BUTTON_PADDING)
            .finish()
        } else {
            let state = self
                .catalog_modal
                .row_states
                .get(row_index)
                .cloned()
                .unwrap_or_default();
            let model_id = model.id.clone();
            Self::render_card_button(
                label,
                state,
                AISettingsPageAction::ToggleCatalogModelInModal {
                    model_id,
                    checked: !is_checked,
                },
                appearance,
            )
        };

        column = column.with_child(
            Container::new(row_element)
                .with_margin_bottom(2.)
                .finish(),
        );
    }

    // Footer.
    let cancel_button = Self::render_card_button(
        "Cancel",
        self.catalog_modal.cancel_state.clone(),
        AISettingsPageAction::CloseCatalogModal,
        appearance,
    );
    let commit_label = format!("Add {} models", modal.checked.len());
    let commit_button = Self::render_card_button(
        commit_label,
        self.catalog_modal.commit_state.clone(),
        AISettingsPageAction::CommitCatalogModelsFromModal {
            provider_index: modal.provider_index,
        },
        appearance,
    );
    let footer = Flex::row()
        .with_main_axis_alignment(MainAxisAlignment::End)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(cancel_button)
        .with_child(Container::new(commit_button).with_margin_left(8.).finish())
        .finish();
    column = column.with_child(Container::new(footer).with_margin_top(10.).finish());

    Container::new(column.finish())
        .with_background(appearance.theme().surface_1())
        .with_uniform_padding(12.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_margin_bottom(12.)
        .finish()
}
```

Call site in `render`, right next to the existing 4a modal hook:

```rust
if let Some(modal) = view.catalog_modal.as_ref() {
    let catalog: &[ai::catalog::CatalogModel] = view
        .catalog_cache
        .as_ref()
        .map(|c| c.all())
        .unwrap_or(&[]);
    column.add_child(self.render_catalog_modal(
        modal,
        &providers,
        catalog,
        view.catalog_load_failure.as_ref(),
        appearance,
    ));
}
```

- [ ] **Step 9.5: Build + clippy + commit**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5

git add app/src/settings_view/agent_providers_widget.rs
git commit -m "feat(app/settings_view/agent_providers_widget): Browse catalog modal

Phase 4b stage C task 9. Adds a 'Browse catalog' button to each
provider card's footer (between Fetch models and Remove) and renders
the catalog-modal panel as a card-style Container at the top of the
widget's column when view.catalog_modal is Some.

Modal renders a header, a 'This provider / All providers' filter chip
row + Refresh button, a search input, a caption showing match counts
and (when present) the catalog-load-failure reason truncated to 80
chars, the filtered row list (up to 200 rows), and Cancel / Add N
models footer buttons.

Mouse-state handles for the modal live in a new CatalogModalHandles
struct on AgentProvidersWidget, pre-allocated at widget construction.
Row pool sized to 200 (matches the modal's hard render cap, well above
typical filter-result counts)."
```

---

## Stage D — 4a cross-phase integration

### Task 10: Enrich 4a's DiscoveredModel with catalog metadata

**Files:**
- Modify: `app/src/ai/agent_providers/fetch_models.rs` — `enrich_with_catalog` helper.
- Modify: `app/src/settings_view/ai_page.rs` — call the helper in the 4a resolve callback when the catalog is available.

- [ ] **Step 10.1: Add `enrich_with_catalog` to `fetch_models.rs`**

Append to the existing `fetch_models.rs`:

```rust
/// Phase 4b cross-phase enrichment. After a successful fetch_models()
/// call returns a Vec<DiscoveredModel> with whatever metadata the live
/// upstream returned (typically just `id`), this helper cross-references
/// each entry against the catalog and fills in missing `display_name`,
/// `context_window`, and `max_output_tokens`. Multimodal capability
/// flags are NOT lifted here — they end up on AgentProviderModel only
/// when the user commits the row via the modal.
///
/// Catalog lookup is opt-in: the caller passes the catalog slice (or
/// an empty slice to skip enrichment entirely). Existing fields on
/// `DiscoveredModel` always win — the catalog only fills `None`s.
pub fn enrich_with_catalog(
    mut models: Vec<DiscoveredModel>,
    api_type: AgentProviderApiType,
    catalog: &[ai::catalog::CatalogModel],
) -> Vec<DiscoveredModel> {
    if catalog.is_empty() {
        return models;
    }
    let candidate_set = ai::catalog::filter_models_for_api_type(api_type, catalog);
    for d in &mut models {
        let Some(c) = candidate_set.iter().find(|c| c.id == d.id) else {
            continue;
        };
        if d.display_name.is_none() && !c.name.is_empty() {
            d.display_name = Some(c.name.clone());
        }
        if d.context_window.is_none() {
            d.context_window = c.context_window;
        }
        if d.max_output_tokens.is_none() {
            d.max_output_tokens = c.max_output_tokens;
        }
    }
    models
}
```

- [ ] **Step 10.2: Add 3 enrichment tests**

In `fetch_models_tests.rs`:

```rust
use ai::catalog::CatalogModel;

use super::enrich_with_catalog;

fn catalog_entry(provider: &str, id: &str) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.into(),
        id: id.into(),
        name: format!("Display {id}"),
        context_window: Some(200000),
        max_output_tokens: Some(8192),
        tool_call: true,
        reasoning: false,
        image: false,
        pdf: false,
        audio: false,
        open_weights: false,
    }
}

#[test]
fn enrich_fills_missing_display_name() {
    let mut d = DiscoveredModel {
        id: "claude-sonnet-4-6".into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
    };
    let catalog = vec![catalog_entry("anthropic", "claude-sonnet-4-6")];
    let enriched = enrich_with_catalog(
        vec![d.clone()],
        AgentProviderApiType::Anthropic,
        &catalog,
    );
    assert_eq!(enriched[0].display_name.as_deref(), Some("Display claude-sonnet-4-6"));
    assert_eq!(enriched[0].context_window, Some(200000));
    assert_eq!(enriched[0].max_output_tokens, Some(8192));
}

#[test]
fn enrich_does_not_overwrite_existing_values() {
    let d = DiscoveredModel {
        id: "claude-sonnet-4-6".into(),
        display_name: Some("User-set name".into()),
        context_window: Some(99),
        max_output_tokens: Some(11),
    };
    let catalog = vec![catalog_entry("anthropic", "claude-sonnet-4-6")];
    let enriched =
        enrich_with_catalog(vec![d], AgentProviderApiType::Anthropic, &catalog);
    assert_eq!(enriched[0].display_name.as_deref(), Some("User-set name"));
    assert_eq!(enriched[0].context_window, Some(99));
    assert_eq!(enriched[0].max_output_tokens, Some(11));
}

#[test]
fn enrich_with_empty_catalog_is_noop() {
    let d = DiscoveredModel {
        id: "claude-sonnet-4-6".into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
    };
    let enriched =
        enrich_with_catalog(vec![d.clone()], AgentProviderApiType::Anthropic, &[]);
    assert_eq!(enriched[0].display_name, None);
}
```

- [ ] **Step 10.3: Wire enrichment into 4a's resolve callback**

In `ai_page.rs`, inside the `FetchAgentProviderModels` handler's `ctx.spawn` callback, after `outcome` is matched as `FetchModelsOutcome::Ok(fetched)`:

```rust
FetchModelsOutcome::Ok(fetched) => {
    // Phase 4b enrichment: if the catalog is loaded, fill missing
    // metadata before the modal opens.
    let fetched = if let Some(cache) = this.catalog_cache.as_ref() {
        crate::ai::agent_providers::fetch_models::enrich_with_catalog(
            fetched,
            provider.api_type,
            cache.all(),
        )
    } else {
        fetched
    };
    // ... existing modal-open body ...
}
```

- [ ] **Step 10.4: Build + test + clippy + commit**

```bash
cargo nextest run -p warp --lib fetch_models 2>&1 | tail -10   # 12 + 3 = 15 passed
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5

git add app/src/ai/agent_providers/fetch_models.rs \
        app/src/ai/agent_providers/fetch_models_tests.rs \
        app/src/settings_view/ai_page.rs
git commit -m "feat(app/ai/agent_providers): enrich 4a fetch with catalog metadata

Phase 4b stage D task 10. Adds enrich_with_catalog(fetched, api_type,
catalog) → Vec<DiscoveredModel> that fills missing display_name /
context_window / max_output_tokens on a fetched-models response from
the 4b catalog (opt-in via a non-empty catalog slice). Existing
DiscoveredModel fields always win — enrichment only fills None's.

Wired into Phase 4a's FetchAgentProviderModels resolve callback in
ai_page.rs so 4a's modal rows display catalog-quality metadata even
for upstreams whose /models endpoint returns id-only.

3 unit tests cover fill-missing, no-overwrite, empty-catalog-noop."
```

---

## Stage E — Manual smoke + spec docs

### Task 11: Live smoke + flip status to code-complete

**Files:**
- Modify: `specs/multi-local-llm/README.md` — flip the Phase 4b row from `📅 unscheduled` to `🧪 code complete — pending live smoke`; add the status paragraph mirroring 4a's.
- Modify: `specs/multi-local-llm/design.md` — add the §9 status flag and any deltas the implementation surfaced.

- [ ] **Step 11.1: Manual smoke**

```text
[ ] First-launch fresh: delete ~/Library/Application Support/warp/byop_catalog.json (or equivalent).
    Open Settings → AI → Custom AI Providers → "+ Add Provider".
    Add a card with api_type=Anthropic. Click "+ Add Model".
    Expect: 5 Claude chips appear within 1-2s; clicking one auto-fills
    the row's id, name, context_window, max_output_tokens, image, pdf.
[ ] Stale-cache refresh: touch the cache file's mtime to 8 days ago.
    Reopen settings. Chips render against the stale cache immediately;
    background refresh fires; chip list updates if catalog changed.
[ ] Offline / fetch failure: airplane mode + delete cache.
    Reopen settings. Chips render from the baked-in snapshot; the
    Browse catalog modal shows "(load failed: ...)" caption.
[ ] Browse catalog modal: click "Browse catalog" on each provider type.
    Expect: rows filter to "This provider" by default; switching to
    "All providers" shows the full list; search narrows; check ≥1
    row, commit, verify the new AgentProviderModel rows appear on the
    card.
[ ] 4a enrichment: configure a provider with a real upstream; click
    "Fetch models". Expect: modal rows show display_name + context_window
    pulled from the catalog even though the live /models response didn't
    include them.
```

Pass criterion: all five checkpoints succeed on at least one platform (macOS preferred). If any fails, file the failure as a §Risks line in this plan; block the phase only if it's a 4b regression (not a pre-existing 4a or upstream-API gap).

- [ ] **Step 11.2: Update README.md**

Append a status paragraph after the Phase 4a paragraph (mirroring 4a's shape):

```markdown
**Phase 4b (models.dev catalog + quick-add chips)** code is complete on `multi-local-llm` (final commit `<TBD>`). Adds catalog-driven model onboarding: 5 inline chips below each empty "+ Add Model" row, plus a "Browse catalog" modal panel mirroring 4a's pattern, sourced from `https://models.dev/api.json` with a 7-day on-disk cache and a baked-in ~25-model snapshot fallback. A new `crates/ai/src/catalog/{mod,wire,parse,fetch,cache,snapshot}.rs` owns the lifecycle; `app/src/settings_view/catalog_modal.rs` holds the pure-logic modal state. Cross-references 4a's `DiscoveredModel` rows to fill missing metadata when the catalog is loaded. **~26 new unit tests** (8 parse + 3 fetch + 3 cache + 4 mod + 8 modal-state); existing test suites stay green.

> **Verification gate:** live-test smoke against the live `models.dev/api.json` for fresh-fetch, stale-cache refresh, and offline-snapshot-fallback; manual smoke per api_type for chip auto-fill and the Browse modal. Once the five checkpoints in `plan-phase-4b.md` §Task 11.1 pass, the Phase 4b row flips to ✅.
```

Add a row to the status table:

```markdown
| 4b — models.dev catalog + quick-add chips | [`plan-phase-4b.md`](plan-phase-4b.md) | 🧪 code complete — pending live smoke |
```

Add a What-landed bullet under "User-visible":

```markdown
- **Phase 4b (pending live smoke):** quick-add chips below the empty "+ Add Model" row pre-fill model metadata from the open-source models.dev catalog. A new "Browse catalog" button opens a modal where the user can check rows to add. Catalog refreshes on a 7-day cache; offline / fetch-failure fall back to a baked-in snapshot.
```

And an Architecture bullet:

```markdown
- **Phase 4b:** New `crates/ai/src/catalog/{mod,wire,parse,fetch,cache,snapshot}.rs` module owns catalog lifecycle (fetch → parse → cache → snapshot fallback). The on-disk cache lives at `<config_dir>/warp/byop_catalog.json` as a versioned envelope; the 7-day TTL is checked at settings-page open and refreshed in the background so the UI never blocks. The widget renders chips inline below empty rows and a full Browse-catalog modal mirroring 4a's pattern; modal-state transitions live in their own pure-logic module `app/src/settings_view/catalog_modal.rs`.
```

- [ ] **Step 11.3: Update design.md §9 row**

```markdown
| **4b. models.dev catalog + quick-add chips** 🧪 code complete | (existing description) | (existing files) | Live test of fetch + offline fallback + chip auto-fill |
```

- [ ] **Step 11.4: Commit**

```bash
git add specs/multi-local-llm/README.md specs/multi-local-llm/design.md
git commit -m "docs(specs/multi-local-llm): record Phase 4b code-complete status

Phase 4b models.dev catalog + quick-add chips shipped end-to-end on
multi-local-llm (final commit <TBD>). Status table row flips from
'📅 unscheduled' to '🧪 code complete — pending live smoke'; README
adds the status paragraph mirroring 4a's shape; design.md §9 row
gets the same flag.

Manual smoke gate: fresh-fetch, stale-cache refresh, offline
fallback, Browse catalog modal commit, and 4a-enrichment cross-
phase test (see plan-phase-4b.md §Task 11.1). Once 5/5 pass, the
row flips to ✅."
```

---

## Final verification

- [ ] **Verification 1: Sweeps** — Catalog module is self-contained under `crates/ai/src/catalog/`; no churn outside the listed files. `crates/ai/src/lib.rs` gains exactly one `pub mod catalog;`. No new feature flag introduced (catalog gated by the existing `LocalLlmProvider` flag via its parent widget).
- [ ] **Verification 2: Build + tests + clippy** — `cargo build -p ai && cargo build -p warp` clean; `cargo nextest run -p ai catalog` shows ≥19 tests passing; `cargo nextest run -p warp --lib catalog_modal` shows 8 passing; `cargo nextest run -p warp --lib fetch_models` shows 15 passing (12 from 4a + 3 enrichment); `cargo clippy -p ai --all-targets --all-features -- -D warnings` clean; `cargo clippy -p warp --lib --tests -- -D warnings` clean.
- [ ] **Verification 3: Manual smoke** — 5/5 checkpoints in Task 11.1 pass.
- [ ] **Verification 4: Final reviewer + push** — dispatch `oh-my-claudecode:code-reviewer` for the full Phase 4b diff (`<phase-4a-final-sha>..HEAD`). Stop before push; user reviews, then pushes manually.

```bash
git log --oneline <phase-4a-final-sha>..HEAD
# Expected (11 commits, one per task):
#   <sha> docs(specs/multi-local-llm): record Phase 4b code-complete status
#   <sha> feat(app/ai/agent_providers): enrich 4a fetch with catalog metadata
#   <sha> feat(app/settings_view/agent_providers_widget): Browse catalog modal
#   <sha> feat(app/settings_view/agent_providers_widget): quick-add chips
#   <sha> feat(app/settings_view/ai_page): wire catalog actions + state
#   <sha> feat(app/settings_view/catalog_modal): pure-logic modal state
#   <sha> feat(ai/catalog): public API + api_type filter map
#   <sha> feat(ai/catalog): baked-in snapshot fallback
#   <sha> feat(ai/catalog): on-disk cache with 7-day TTL + atomic write
#   <sha> feat(ai/catalog): HTTP fetch with timeout + body cap
#   <sha> feat(ai/catalog): wire types + tolerant parser
```

---

## Risks & open questions

1. **models.dev availability + schema drift.** The catalog is external; an outage or schema change could break the fetch. **Mitigation:** tolerant `#[serde(default)]` parsing, on-disk cache, baked-in snapshot fallback. A really broken response just leaves users on the snapshot until the next refresh.
2. **Cache-write permission.** `<config_dir>/warp/byop_catalog.json` requires write access; sandboxed installs (Mac App Store, future Flatpak) may lack it. **Mitigation:** save failure is logged but not propagated; the in-memory cache works for the session.
3. **Per-row chip overflow on narrow widget widths.** 5 chips at variable lengths may overflow on small windows. **Mitigation:** horizontal scroll via `Flex::row` is the WarpUI default; if it doesn't auto-scroll, cap chip count dynamically based on widget width. Defer until smoke surfaces a layout issue.
4. **Catalog vs 4a fetch overlap.** A user with both inline chips and a Fetch-models call active for the same provider could see duplicate suggestions. **Mitigation:** the inline chips disappear once the row's id is filled, including by 4a's commit path — see Task 8.2's empty-row check.
5. **Custom OpenAI-compatible relays return OpenAI chips.** A user pointing `api_type: OpenAi` at a Mistral-compatible relay sees `gpt-4o` chips. **Mitigation:** the chips are suggestions, not constraints — the user can ignore them and type any model id. The Browse catalog modal's "All providers" filter offers the alternative.
6. **`structured_output` and `family` fields aren't used.** Phase 4b ignores them; Phase 4c or 4d can lift `structured_output` into a future capability flag, and `family` into a row-grouping affordance.
7. **No telemetry on catalog usage.** Same rationale as 4a — BYOP code intentionally doesn't emit telemetry per the privacy framing in the Phase 4a commit messages. Future polish only if telemetry policy changes.

---

## Next plan (Phase 4c — per-model multimodal capabilities)

Phase 4c wires the existing `image / pdf / audio: Option<bool>` flags on `AgentProviderModel` (added in Phase 1b-1) through a new `crates/ai/src/capabilities.rs` resolver with the precedence chain Explicit > 4b catalog > per-api_type heuristic > conservative-false. The send path adds a pre-send gate that blocks attachments to incapable models with an inline error. See `specs/multi-local-llm/design.md` §14 for the design.
