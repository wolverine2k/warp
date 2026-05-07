# Multi-Local-LLM — Phase 1b-1 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the BYOP foundation — new settings types (`AgentProvider`, `AgentProviderModel`, `AgentProviderKind`, `AgentProviderApiType`), new setting markers (`agent_providers: Vec<AgentProvider>`, `byop_last_used_model_id`, the 7-field `agents.byop_compaction.*` group), and the new `llm_id` module — **alongside** the existing single-provider code with **zero behavior change**. Sets up the scaffolding for Phase 1b-2 (dispatch + migration + secrets HashMap refactor).

**Architecture:** Pure additions only. The existing `LocalProviderConfig`, `agents.local_provider.*` TOML schema, `AgentProviderSecrets` singleton (single key), and `local:` LLMId prefix all stay untouched and live alongside the new types. No code reads the new types yet — they exist for Phase 1b-2 to wire up.

**Tech Stack:** Rust, `define_settings_group!` macro, serde with custom `Deserialize`, `strum`/`strum_macros`, `schemars` for JSON schema, the `LLMId` newtype from the `ai` crate.

**Branch:** `multi-local-llm` (HEAD `1894e872`, 8 commits ahead of `nmehta/local-llm-provider`).

**Spec reference:** `specs/multi-local-llm/design.md` §1 (Data model — Phase 1b-1 lands the schema; Phase 1b-2 lands secrets/migration/dispatch; Phase 1b-3 lands UI).

**Source of truth for shapes:** the `openwarp` branch (`git show openwarp:app/src/settings/ai.rs`, `git show openwarp:app/src/ai/agent_providers/llm_id.rs`). We adopt the openwarp shapes verbatim per design §1, with English-language doc comments substituted for the originals.

**Estimated scope:** ~250 lines net code across 3 files, two atomic commits, ~30-45 minutes.

---

## File map

**Files modified:**
- `app/src/settings/ai.rs` — add 4 new types + helpers (Task 1) and 9 setting markers (still Task 1).
- `crates/ai/src/local_provider/mod.rs` — wire `pub mod llm_id;` (Task 2).

**Files created:**
- `crates/ai/src/local_provider/llm_id.rs` — `BYOP_PREFIX`, `encode`, `decode`, `is_byop`, plus unit tests (Task 2).

**Files NOT touched in 1b-1 (deferred):**
- `crates/ai/src/local_provider/agent_provider_secrets.rs` (Phase 1b-2 refactors the singleton to a `HashMap`).
- `app/src/ai/local_provider_config.rs` (Phase 1b-2 wires dispatch).
- `app/src/ai/agent/api/impl.rs` (Phase 1b-2 wires dispatch).
- `app/src/settings_view/ai_page.rs` (Phase 1b-3 replaces the widget body).

**Cargo dependencies:** `uuid`, `strum`, `strum_macros`, `schemars` are already present in both `app/Cargo.toml` and `crates/ai/Cargo.toml` (verified) — no `Cargo.toml` edits needed.

---

## Task 0: Pre-flight — verify clean baseline

**Files:** none (read-only verification)

- [ ] **Step 0.1: Confirm branch and clean state**

```bash
git rev-parse --abbrev-ref HEAD
git status --short
git log --oneline -1
```

Expected: branch `multi-local-llm`; status shows only `.claude/scheduled_tasks.lock` and `.omc/` untracked; HEAD `1894e872`.

- [ ] **Step 0.2: Baseline build of the two affected crates**

```bash
cargo build -p ai 2>&1 | tail -3
cargo build -p warp 2>&1 | tail -3
```

Expected: both `Finished`.

- [ ] **Step 0.3: Baseline tests for the `ai` crate**

```bash
cargo nextest run -p ai 2>&1 | tail -3
```

Expected: `314 tests run: 314 passed, 0 skipped`.

---

## Task 1: Add settings types + setting markers

**File:** `app/src/settings/ai.rs`

Single commit. All additions go inside the existing file. No removals. Search for the existing `define_settings_group!(AISettings, settings: [ ... ])` block — the 9 new markers go inside that list at the end (just before the closing `])`). The 4 new types + helper fns go just above the `define_settings_group!` invocation, in module scope.

- [ ] **Step 1.1: Add helper functions and the four new types**

Locate a stable insertion point in `app/src/settings/ai.rs` — choose a spot in module scope above the `define_settings_group!(AISettings, ...)` invocation. (Use `grep -n "define_settings_group!(AISettings" app/src/settings/ai.rs` to confirm position.)

Insert this block. The English comments are deliberate — openwarp's source uses Chinese comments which we replace 1:1 with English.

```rust
// ===== BYOP (Bring Your Own Provider) data model — Phase 1b-1 =====
//
// These types describe the user-configured Agent providers stored under
// `agents.warp_agent.providers`. They are not yet read by any dispatch path
// (Phase 1b-2 wires dispatch + migration + the secrets HashMap; Phase 1b-3
// rebuilds the settings widget). They live alongside the existing
// LocalProviderConfig + agents.local_provider.* schema with zero behavior
// change.

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_true(v: &bool) -> bool {
    *v
}
fn default_true() -> bool {
    true
}

/// Top-level kind of an Agent provider. Currently a single variant —
/// "user-managed OpenAI-compatible endpoint". The wire-protocol decision is
/// made by [`AgentProviderApiType`], not by this enum.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize,
    strum_macros::EnumIter, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderKind {
    /// OpenAI-compatible Chat Completions / `/v1/models` protocol.
    OpenAiCompatible,
}

impl Default for AgentProviderKind {
    fn default() -> Self {
        Self::OpenAiCompatible
    }
}

/// The wire-protocol variant the provider's `base_url` actually speaks. Used
/// at request time by the dispatch layer to choose the right
/// request/response codec. Phase 1b-1 only defines the enum; Phase 3 adds
/// per-variant adapter implementations beyond OpenAI.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize,
    strum_macros::EnumIter, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderApiType {
    /// OpenAI Chat Completions (`POST /v1/chat/completions`). Covers OpenAI,
    /// DeepSeek, SiliconFlow, OpenRouter, Moonshot, vLLM, llama.cpp, Ollama
    /// behind its OpenAI-compat shim, and most "OpenAI-compatible" gateways.
    OpenAi,
    /// OpenAI Responses API (`POST /v1/responses`). Used by GPT-5 / Codex /
    /// Pro tier models.
    OpenAiResp,
    /// Google Gemini native protocol (generativelanguage.googleapis.com).
    Gemini,
    /// Anthropic Messages API native protocol (api.anthropic.com).
    Anthropic,
    /// Ollama native protocol (`/api/chat`). Distinct from Ollama's
    /// OpenAI-compat shim, which uses `OpenAi` instead.
    Ollama,
    /// DeepSeek native protocol. Differs from `OpenAi` in that thinking-mode
    /// models require `reasoning_content` round-tripped back to the server;
    /// only this variant handles that field.
    DeepSeek,
}

impl Default for AgentProviderApiType {
    fn default() -> Self {
        Self::OpenAi
    }
}

/// One user-configured Agent provider (a base URL + a list of models the
/// user wants exposed in the picker). The API key is stored separately in
/// `AgentProviderSecrets` keyed by [`AgentProvider::id`]; this struct
/// deliberately doesn't carry the secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentProvider {
    /// Stable provider identifier, generated on first creation. Used as the
    /// keychain map key for the API secret and embedded in the BYOP `LLMId`
    /// (`byop:<id>:<model_id>`).
    #[serde(default = "AgentProvider::default_id")]
    pub id: String,

    /// User-visible display name (e.g. "DeepSeek Official", "Local Ollama").
    pub name: String,

    /// Kind discriminator — currently always `OpenAiCompatible`.
    #[serde(default)]
    pub kind: AgentProviderKind,

    /// Wire-protocol variant. Old configs without this field deserialize as
    /// `OpenAi` (the original behavior).
    #[serde(default)]
    pub api_type: AgentProviderApiType,

    /// API base URL, e.g. `https://api.deepseek.com/v1` or
    /// `http://localhost:11434/v1`. Stored without trailing slash by
    /// convention; the request layer does its own normalization.
    pub base_url: String,

    /// Models exposed to the picker for this provider. Each entry's `id` is
    /// what gets sent as the upstream `model` field; `name` is the picker
    /// display.
    #[serde(default)]
    pub models: Vec<AgentProviderModel>,
}

impl AgentProvider {
    fn default_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

impl settings_value::SettingsValue for AgentProvider {}

/// One model entry within an [`AgentProvider`]. The custom `Deserialize`
/// impl supports both the full struct shape and a bare-string shorthand
/// (`models = ["llama3.1"]`) for ergonomic TOML hand-editing.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct AgentProviderModel {
    pub name: String,
    pub id: String,

    /// Context window in tokens. 0 means "unknown" — dispatch falls back to
    /// not enforcing a token budget locally and lets the upstream surface
    /// any 4xx context-overflow errors.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub context_window: u32,

    /// Max output tokens per response. 0 means "unspecified".
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub max_output_tokens: u32,

    /// Whether this model emits CoT/reasoning output. Phase 4c wires this
    /// into the streaming layer; Phase 1b-1 only persists the value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reasoning: bool,

    /// Whether to advertise tool/function-calling schemas to this model.
    /// Default `true` — preserves the existing behavior of the
    /// single-provider config's `supports_tools` toggle, which defaulted on.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub tool_call: bool,

    // Multimodal capability flags — three-state semantics:
    //   None         = "Auto", inferred at runtime (Phase 4c).
    //   Some(true)   = user-forced ON.
    //   Some(false)  = user-forced OFF.
    /// Image input capability (image/* MIME).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<bool>,
    /// PDF input capability (application/pdf).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf: Option<bool>,
    /// Audio input capability (audio/* MIME).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
}

impl AgentProviderModel {
    pub fn from_id(id: String) -> Self {
        Self {
            name: id.clone(),
            id,
            context_window: 0,
            max_output_tokens: 0,
            reasoning: false,
            tool_call: true,
            image: None,
            pdf: None,
            audio: None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for AgentProviderModel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Either {
            Plain(String),
            Full {
                #[serde(default)]
                name: String,
                id: String,
                #[serde(default)]
                context_window: u32,
                #[serde(default)]
                max_output_tokens: u32,
                #[serde(default)]
                reasoning: bool,
                #[serde(default = "default_true")]
                tool_call: bool,
                #[serde(default)]
                image: Option<bool>,
                #[serde(default)]
                pdf: Option<bool>,
                #[serde(default)]
                audio: Option<bool>,
            },
        }
        match Either::deserialize(deserializer)? {
            Either::Plain(id) => Ok(AgentProviderModel::from_id(id)),
            Either::Full {
                name,
                id,
                context_window,
                max_output_tokens,
                reasoning,
                tool_call,
                image,
                pdf,
                audio,
            } => {
                let name = if name.is_empty() { id.clone() } else { name };
                Ok(AgentProviderModel {
                    name,
                    id,
                    context_window,
                    max_output_tokens,
                    reasoning,
                    tool_call,
                    image,
                    pdf,
                    audio,
                })
            }
        }
    }
}

impl settings_value::SettingsValue for AgentProviderModel {}
```

**Notes:**
- Use the fully-qualified `serde::Deserialize` / `serde::Deserializer` paths inside the custom impl to avoid stepping on whatever the file already imports. (If the file already has `use serde::{Deserialize, Deserializer, Serialize};` at the top — and it does — you can drop the qualifications.)
- The implementer should pick exact line placement after reading the file. The order doesn't matter as long as the types are above the `define_settings_group!` invocation.
- After insertion, run `cargo build -p warp 2>&1 | tail -10` to catch any name collisions or missing imports before proceeding.

- [ ] **Step 1.2: Add 9 setting markers to the `AISettings` group**

Locate the closing `]);` of `define_settings_group!(AISettings, settings: [ ... ])` in `app/src/settings/ai.rs`. Insert these 9 markers immediately before that closing — keeping the trailing comma convention of the surrounding entries.

```rust
    // ===== BYOP settings (Phase 1b-1) — list + last-used + compaction =====

    // The user-configured Agent providers list. Each provider's API key
    // lives in `AgentProviderSecrets` keyed by `AgentProvider::id` (Phase
    // 1b-2 refactors that singleton; for now the legacy `LocalProviderApiKey`
    // keychain key is still in effect and unrelated to these entries).
    agent_providers: AgentProviders {
        type: Vec<AgentProvider>,
        default: Vec::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "agents.warp_agent.providers",
        description: "User-configured custom Agent providers (OpenAI-compatible).",
    }

    // The most-recently picked BYOP model encoded as a `byop:<provider_id>:<model_id>`
    // LLMId string. Empty = no last-used yet; new tabs/sessions fall back to
    // the profile default. Hydrated from this setting by the picker so the
    // user's choice persists across restarts.
    byop_last_used_model_id: ByopLastUsedModelId {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop.last_used_model_id",
        description: "Last selected BYOP model id (picker hydrates new tabs/sessions from this).",
    }

    // Auto-trigger compaction on token-overflow. Mirrors the existing
    // `local_provider_compaction_auto` field — Phase 1b-2 will read this
    // path going forward, after migration copies the legacy value across.
    byop_compaction_auto: ByopCompactionAuto {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.auto",
        description: "Enable BYOP automatic conversation compaction on context overflow.",
    }

    byop_compaction_prune: ByopCompactionPrune {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.prune",
        description: "Auto-prune older tool outputs to free BYOP context.",
    }

    byop_compaction_tail_turns: ByopCompactionTailTurns {
        type: u32,
        default: 2u32,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.tail_turns",
        description: "Number of recent user turns to keep verbatim during compaction.",
    }

    byop_compaction_preserve_recent_tokens: ByopCompactionPreserveRecentTokens {
        type: u32,
        default: 0u32,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.preserve_recent_tokens",
        description: "Override the recent-tokens preservation budget (0 = auto).",
    }

    byop_compaction_reserved: ByopCompactionReserved {
        type: u32,
        default: 0u32,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.reserved",
        description: "Reserved buffer tokens for compaction overflow check (0 = auto).",
    }

    byop_compaction_model_provider_id: ByopCompactionModelProviderId {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.model.provider_id",
        description: "Optional dedicated provider id for compaction LLM calls.",
    }

    byop_compaction_model_id: ByopCompactionModelId {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.byop_compaction.model.model_id",
        description: "Optional dedicated model id for compaction LLM calls.",
    }
```

- [ ] **Step 1.3: Build the workspace**

```bash
cargo build -p warp 2>&1 | tail -10
```

Expected: `Finished`. Common failure modes:
- Missing `use settings::SyncToCloud::Globally` etc. — already in scope from the file's existing imports; no action needed.
- `RespectUserSyncSetting::Yes` import — if the file doesn't import it, look for an existing `byop_*` or `cloud_sync_*` setting that uses it as a template.
- Trait bounds on `AgentProvider` for the `Vec<AgentProvider>` setting — `define_settings_group!` requires `Default`, `Clone`, `PartialEq`, `Serialize`, `Deserialize`. `Vec<AgentProvider>` gets `Default` automatically; the macro should accept it. If it complains, derive `Default` manually on `AgentProvider` (the `id` field needs `default_id()` so a manual `Default` impl that calls `default_id` is appropriate).
- If `define_settings_group!` requires `JsonSchema`, the derives are already in place.

If a build error references a trait bound issue, **stop and report BLOCKED with the exact compiler message** — do not invent additional derives without confirmation.

- [ ] **Step 1.4: Run tests for the `ai` crate**

```bash
cargo nextest run -p ai 2>&1 | tail -3
```

Expected: same baseline `314 tests run: 314 passed`. (No new tests yet — those land in Step 2.x.)

- [ ] **Step 1.5: Run cargo fmt**

```bash
cargo fmt
git diff --stat
```

If fmt produced changes inside `app/src/settings/ai.rs`, that's expected — let it run and stage them.

- [ ] **Step 1.6: Stage and commit**

```bash
git add app/src/settings/ai.rs
git status --short
```

Expected: `M app/src/settings/ai.rs`. Then:

```bash
git commit -m "$(cat <<'EOF'
feat(settings/ai): add BYOP types + setting markers (Phase 1b-1)

Adds the AgentProvider/AgentProviderModel/AgentProviderKind/
AgentProviderApiType types from openwarp's BYOP design verbatim, plus
the agent_providers Vec<AgentProvider> setting marker, the
byop_last_used_model_id picker memory, and the 7-field
agents.byop_compaction.* group.

Pure additions. No existing code reads these yet — Phase 1b-2 wires
dispatch + migration + the secrets HashMap; Phase 1b-3 rebuilds the
settings widget. The legacy LocalProviderConfig + agents.local_provider.*
schema + LocalProviderApiKey keychain key all stay untouched and
behavioral parity is preserved.

See specs/multi-local-llm/design.md §1 (data model) and
specs/multi-local-llm/plan-phase-1b-1-foundation.md.
EOF
)"
```

Expected: commit succeeds.

---

## Task 2: Add `llm_id` module

**Files:**
- Create: `crates/ai/src/local_provider/llm_id.rs`
- Modify: `crates/ai/src/local_provider/mod.rs`

- [ ] **Step 2.1: Create `crates/ai/src/local_provider/llm_id.rs`**

```rust
//! BYOP (Bring Your Own Provider) `LLMId` prefix codec.
//!
//! BYOP-routed conversations identify their model with the `LLMId` string
//! prefix `byop:` so dispatch can branch between the cloud-Warp path and
//! the user-configured OpenAI-compatible path at request time.
//!
//! Encoding: `byop:<provider_id>:<model_id>`
//!   - `provider_id` is `AgentProvider::id` (a UUID v4 string, no colons).
//!   - `model_id` is `AgentProviderModel::id` (the value sent upstream as
//!     the `model` field). Some upstreams use vendor-prefixed model names
//!     like `vendor:model:variant`, so the codec splits only on the first
//!     colon after the prefix and treats the rest as the model id.
//!
//! Example: `byop:6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34:deepseek-chat`
//!
//! Phase 1b-1 only ships the codec — no caller decodes BYOP IDs yet.
//! Phase 1b-2 wires this into dispatch and the picker.

use ai::LLMId;

pub const BYOP_PREFIX: &str = "byop:";

/// Encode `(provider_id, model_id)` into a BYOP `LLMId`.
pub fn encode(provider_id: &str, model_id: &str) -> LLMId {
    LLMId::from(format!("{BYOP_PREFIX}{provider_id}:{model_id}"))
}

/// If `id` is a BYOP-encoded `LLMId`, return `(provider_id, model_id)`.
/// Returns `None` for the legacy `local:` prefix or any non-BYOP value.
pub fn decode(id: &LLMId) -> Option<(String, String)> {
    let s = id.as_str().strip_prefix(BYOP_PREFIX)?;
    let (pid, mid) = s.split_once(':')?;
    if pid.is_empty() || mid.is_empty() {
        return None;
    }
    Some((pid.to_owned(), mid.to_owned()))
}

/// Quick `starts_with(BYOP_PREFIX)` check for callers that just need to
/// route between BYOP vs. cloud-Warp without splitting fields.
pub fn is_byop(id: &LLMId) -> bool {
    id.as_str().starts_with(BYOP_PREFIX)
}

#[cfg(test)]
#[path = "llm_id_tests.rs"]
mod tests;
```

The repo convention (per `CLAUDE.md`) is to put unit tests in a sibling `*_tests.rs` file rather than inline `#[cfg(test)] mod tests { … }`. Sibling location avoids growing the module's public surface and matches the rest of `local_provider/`.

- [ ] **Step 2.2: Create `crates/ai/src/local_provider/llm_id_tests.rs`**

```rust
use super::*;
use ai::LLMId;

#[test]
fn round_trip() {
    let id = encode("6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34", "deepseek-chat");
    assert_eq!(
        id.as_str(),
        "byop:6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34:deepseek-chat"
    );
    assert_eq!(
        decode(&id),
        Some((
            "6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34".to_owned(),
            "deepseek-chat".to_owned(),
        ))
    );
}

#[test]
fn model_id_with_colon_is_preserved() {
    // Some gateways (notably OpenRouter-style "vendor/model" or
    // "vendor:model:variant") use multiple colons in the model id. The
    // codec must split only on the first colon after the prefix.
    let id = encode("uuid-1", "vendor:model:v2");
    assert_eq!(
        decode(&id),
        Some(("uuid-1".to_owned(), "vendor:model:v2".to_owned()))
    );
}

#[test]
fn legacy_local_prefix_is_not_byop() {
    let legacy = LLMId::from("local:llama3.1");
    assert_eq!(decode(&legacy), None);
    assert!(!is_byop(&legacy));
}

#[test]
fn empty_provider_or_model_decodes_to_none() {
    assert_eq!(decode(&LLMId::from("byop::deepseek-chat")), None);
    assert_eq!(decode(&LLMId::from("byop:uuid-1:")), None);
    assert_eq!(decode(&LLMId::from("byop::")), None);
}

#[test]
fn missing_separator_decodes_to_none() {
    // `byop:<provider_id>` without the second colon is malformed.
    assert_eq!(decode(&LLMId::from("byop:uuid-only-no-model")), None);
}

#[test]
fn is_byop_recognizes_prefix_only() {
    assert!(is_byop(&LLMId::from("byop:x:y")));
    assert!(is_byop(&LLMId::from("byop:")));   // even malformed
    assert!(!is_byop(&LLMId::from("local:foo")));
    assert!(!is_byop(&LLMId::from("claude-3")));
    assert!(!is_byop(&LLMId::from("")));
}
```

- [ ] **Step 2.3: Wire `pub mod llm_id;` into `crates/ai/src/local_provider/mod.rs`**

Open `crates/ai/src/local_provider/mod.rs`. The existing module list (post-fmt from Phase 1a) is alphabetically sorted starting with `agent_provider_secrets`. Insert `pub mod llm_id;` in alphabetical order, between `config` and `prompt`. The block currently looks like:

```rust
pub mod agent_provider_secrets;
pub mod compaction;
pub mod config;
pub mod prompt;
pub mod request;
pub mod response;
pub mod run;
pub mod tools;
pub mod wire;
```

After the edit:

```rust
pub mod agent_provider_secrets;
pub mod compaction;
pub mod config;
pub mod llm_id;
pub mod prompt;
pub mod request;
pub mod response;
pub mod run;
pub mod tools;
pub mod wire;
```

No re-export needed — `llm_id` is a small enough module that callers can `use ai::local_provider::llm_id::{encode, decode, is_byop, BYOP_PREFIX};` directly.

- [ ] **Step 2.4: Build the `ai` crate**

```bash
cargo build -p ai 2>&1 | tail -5
```

Expected: `Finished`. If `LLMId` import fails, check what the rest of the file imports — `LLMId` is re-exported from the `ai` crate root (the file currently uses `use ai::LLMId;` patterns elsewhere; mirror that).

- [ ] **Step 2.5: Run the `ai` crate tests**

```bash
cargo nextest run -p ai 2>&1 | tail -10
```

Expected: **320 tests run: 320 passed** (314 baseline + 6 new in `llm_id_tests.rs`). If the count is off, the new test file may not be wired in correctly — check `cargo nextest run -p ai llm_id 2>&1 | tail -10` to see if the 6 specific tests are discovered.

- [ ] **Step 2.6: Run cargo fmt**

```bash
cargo fmt
git status --short
```

Expected: only the new + modified files appear (no fmt churn elsewhere). If fmt rewrote the new file, that's expected — let it stand.

- [ ] **Step 2.7: Stage and commit**

```bash
git add crates/ai/src/local_provider/llm_id.rs \
        crates/ai/src/local_provider/llm_id_tests.rs \
        crates/ai/src/local_provider/mod.rs
git status --short
```

Expected: 3 lines (1 mod-rs `M`, 2 new files `??`/`A`). Then:

```bash
git commit -m "$(cat <<'EOF'
feat(ai/local_provider): add llm_id module for BYOP encoding (Phase 1b-1)

Adds crates/ai/src/local_provider/llm_id.rs with encode / decode /
is_byop helpers for the byop:<provider_id>:<model_id> LLMId format.
Splits only on the first colon after the prefix so model ids that
contain colons (e.g. some OpenRouter-style "vendor:model:variant" names)
round-trip cleanly.

Pure addition. No caller decodes BYOP ids yet — Phase 1b-2 wires this
into agent/api/impl.rs dispatch and the picker.

Tests cover: round trip, colon-bearing model ids, legacy local: prefix
distinction, empty/malformed decoding, is_byop edge cases.

See specs/multi-local-llm/design.md §1.3 and
specs/multi-local-llm/plan-phase-1b-1-foundation.md Task 2.
EOF
)"
```

---

## Task 3: Final verification

**Files:** none (verification only)

- [ ] **Step 3.1: Confirm two new commits land cleanly**

```bash
git log --oneline -4
```

Expected:

```
<sha-2> feat(ai/local_provider): add llm_id module for BYOP encoding (Phase 1b-1)
<sha-1> feat(settings/ai): add BYOP types + setting markers (Phase 1b-1)
1894e872 fix(workspace/view_test): register AgentProviderSecrets in initialize_app
<...older Phase 1a commits>
```

- [ ] **Step 3.2: Sweep — confirm no Phase-1a names regressed**

```bash
grep -rn "LocalProviderKeyManager\|LocalProviderWidget" --include="*.rs" .
```

Expected: empty (Phase 1a invariant must hold).

- [ ] **Step 3.3: Sweep — confirm Phase-1b-1 names exist**

```bash
grep -rn "AgentProvider\b\|AgentProviderModel\b\|AgentProviderKind\b\|AgentProviderApiType\b" \
     --include="*.rs" app/src/settings/ai.rs
grep -rn "BYOP_PREFIX\|is_byop\|byop:" \
     --include="*.rs" crates/ai/src/local_provider/
```

Expected: each grep returns multiple matches.

- [ ] **Step 3.4: Confirm tests + build still green**

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo build -p warp 2>&1 | tail -3
```

Expected: 320 tests pass; both builds `Finished`.

- [ ] **Step 3.5: Run cargo clippy on the workspace**

```bash
cargo clippy --workspace --all-targets --all-features --tests -- -D warnings 2>&1 | tail -10
```

Expected: clean (no warnings emitted as errors). The new code mirrors openwarp's patterns and shouldn't trigger any lints, but if clippy flags inline format args or similar, address them before declaring complete.

- [ ] **Step 3.6: Stop here — do not push**

The user reviews the diff before push, same as Phase 1a. Report back to the controller with:
- Both commit SHAs.
- Test count delta (314 → 320).
- Diff stats (`git show --stat <sha-1> <sha-2>` summary).
- Any lints / surprises encountered.

---

## Self-review checklist (writer's note)

This plan was written against `specs/multi-local-llm/design.md` §1 (data model) restricted to the Phase 1b-1 subset (types + settings markers + llm_id codec, no secrets refactor, no migration, no dispatch, no UI).

- [x] **Scope:** every change here is a pure addition. No existing code is modified beyond adding `pub mod llm_id;` to `local_provider/mod.rs`. The `agents.local_provider.*` schema, `LocalProviderApiKey` keychain, and `local:` LLMId prefix are all untouched.
- [x] **Coverage:** the openwarp BYOP type system (4 types + 4 helpers + custom Deserialize), the 9 setting markers, and the BYOP `LLMId` codec all land in this plan. Items deferred per design (multimodal wiring → Phase 4c; secrets HashMap + migration + dispatch → Phase 1b-2; UI → Phase 1b-3) are explicitly out of scope and not touched.
- [x] **No placeholders:** every code block is the exact text to insert. The custom `Deserialize` for `AgentProviderModel` is transcribed verbatim from openwarp.
- [x] **Type consistency:** struct/field/method names match openwarp exactly so future merge with `openwarp` is conflict-free.
- [x] **Verification gates:** Step 0 baseline, Step 1.3-1.4 build+tests after types, Step 2.4-2.5 build+tests after llm_id, Step 3.5 final clippy.
- [x] **Test count expectation:** 314 → 320 (6 new tests in `llm_id_tests.rs`).

---

## Next plan (Phase 1b-2)

After 1b-1 ships green, the Phase 1b-2 plan will cover:
- Refactor `AgentProviderSecrets` from `key: Option<String>` (singleton) to `keys: HashMap<ProviderId, String>` — atomic with the migration helper that copies the legacy `LocalProviderApiKey` keychain value into the new map.
- New `agent_providers` module with `lookup_byop` (resolve `(AgentProvider, AgentProviderModel, api_key)` from a BYOP `LLMId`) and `build_byop_llm_infos` (picker enumeration).
- `app/src/ai/agent/api/impl.rs` dispatch: branch on `byop:` prefix → look up runtime config → call existing `local_provider::run_chat_turn` with a `ProviderRuntimeConfig` snapshot.
- Migration helper `crates/ai/src/local_provider/migration.rs`: synthesize one `AgentProvider` from legacy `agents.local_provider.*` fields, copy the API key into the new keychain map, rewrite persisted conversation `LLMId`s from `local:<model>` to `byop:<uuid>:<model>`, set the `legacy_local_provider_migrated` marker.
- Conversation-load-time fallback in `agent_conversations_model` for any `local:` IDs that escape migration.
- Tests: migration fixture, `lookup_byop` failure modes, dispatch routing.

That plan will be written after 1b-1 is approved + executed.
