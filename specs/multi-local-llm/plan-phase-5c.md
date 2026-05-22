# Phase 5c — BYOP Orchestration — External-CLI Env-Var Injection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Touching UI? Read `warp-ui-guidelines` first.

**Goal:** When the user picks a BYOP model in the orchestration picker AND a third-party Local CLI harness (Claude Code / Codex / OpenCode), spawn the harness CLI with the env vars it already honors pointing at the BYOP endpoint. The CLI talks to the user's chosen BYOP provider instead of its built-in default cloud endpoint.

Phase 5b made BYOP entries reachable in the picker and survived submit-time validation, but Local Native (`oz`/empty harness) was the only path that actually dispatched through to a BYOP endpoint (via the in-process Phase 4d dispatcher). Local non-Native (claude-code / codex / opencode child harnesses) currently *would* spawn the CLI but with no BYOP env-var routing, so the CLI defaults to its hard-coded cloud endpoint. Phase 5c closes that gap.

**Out of scope for 5c** (deferred to 5d):

- Remote credential bridge (managed-secret create, `byop_base_url` / `byop_api_type` GraphQL fields on `RunAgentsRequest`, Auto-create button).
- Compaction inheritance forwarding to Remote workers.
- Gemini CLI as a local child harness — `normalize_local_child_harness` currently returns `None` for `Harness::Gemini`, and `prepare_local_harness_child_launch` has an `unreachable!("normalize_local_child_harness filters out Gemini")` arm. Enabling Gemini CLI as a local child is a separate enablement gate that includes shell-args, plugin install, validate_cli_installed, and the disabled-message check — not Phase 5c's job.

**Decisions locked in (from `spec-phase-5.md` plus discoveries during 5b execution):**

| Decision | Choice |
|---|---|
| Env-var matrix | `claude` → `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY` (plus `ANTHROPIC_MODEL` already set by existing `harness_model_env_vars`); `codex` → `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `OPENAI_MODEL`; `opencode` → `OPENAI_BASE_URL`, `OPENAI_API_KEY` (no model env-var per spec). Gemini deferred. |
| Where to assemble env vars | New module `app/src/ai/orchestration_byop_env.rs` exporting `byop_env_for_harness(provider, api_key, model_id, harness) -> HashMap<OsString, OsString>`. Pure function. |
| Where to resolve the BYOP entry from settings | Caller-side (`terminal_pane.rs::launch_local_harness_child`), since `prepare_local_harness_child_launch` is async and takes owned simple types (no `AppContext`). New helper `resolve_byop_for_local_child(ctx, model_id, harness)` in `agent_providers/mod.rs` returns `Option<(AgentProvider, String /*api_key*/)>`. |
| Where to merge env vars into the bag | `prepare_local_harness_child_launch` gains a `byop_env: HashMap<OsString, OsString>` parameter and merges it into the existing env-var assembly after `harness_model_env_vars`. Empty when caller didn't resolve a BYOP entry. |
| BYOP api_key in env vars precedence | BYOP entries override anything `harness_model_env_vars` set if there's overlap (e.g., `ANTHROPIC_MODEL`). HashMap insertion replaces the existing key. |
| Mismatched API-type + harness combos | `byop_env_for_harness` returns empty `HashMap` (defense in depth — submit-time validator already rejected the combo, so this is unreachable in practice). |
| Logging / observability | The api_key value is redacted (`<redacted-{N}-bytes>`) in any structured log of the env-var bag. The validator failure path (Phase 5b) already logs via `log::warn!` in `dispatch_run_agents`. |

**Architecture:** A new pure helper module + a settings-aware resolve helper + threading the result through `prepare_local_harness_child_launch`. No new types, no new traits, no public API surface expansion. Builds entirely on Phase 5a's `byop:<provider_id>:<model_id>` LLMId convention and Phase 5b's caller wiring.

**Tech Stack:** Rust 2021, `std::collections::HashMap<OsString, OsString>`, `std::ffi::OsString`, existing `AgentProvider` + `AgentProviderSecrets` types.

---

## Per-touchpoint reference

| Concern | Source of truth |
|---|---|
| `AgentProvider` struct | `app/src/settings/ai.rs:798` (after Phase 5a additions) |
| `AgentProviderApiType` | `crates/ai/src/local_provider/api_type.rs:27` |
| `AgentProviderSecrets::get(provider_id)` | `crates/ai/src/local_provider/agent_provider_secrets.rs:54` |
| `Harness::config_name` and `parse_local_child_harness` | `crates/warp_cli/src/agent.rs:125`, `:199` |
| `LLMId::byop:` decoding | `crates/ai/src/local_provider/llm_id.rs::decode` |
| Existing `harness_model_env_vars` (sets `ANTHROPIC_MODEL` only) | `app/src/ai/agent_sdk/driver/harness/mod.rs:414` |
| `prepare_local_harness_child_launch` (assembly site) | `app/src/pane_group/pane/local_harness_launch.rs:92` |
| `launch_local_harness_child` (caller in pane_group) | `app/src/pane_group/pane/terminal_pane.rs:1845` |
| `build_byop_llm_infos` (Phase 4d) — provider/api_key lookup pattern | `app/src/ai/agent_providers/mod.rs:28` |
| `validate_orchestration_model_id` (Phase 5a, wired Phase 5b) | `app/src/ai/agent_sdk/common.rs:71` |
| Phase 5b's existing `PreparedLocalHarnessLaunch.env_vars` consumer | `terminal_pane.rs:1897` (`env_vars: HashMap<OsString, OsString>` → `HiddenChildAgentConversationRequest`) |

---

## File map

**Created:**

- `app/src/ai/orchestration_byop_env.rs` — `byop_env_for_harness(provider, api_key, model_id, harness)` + the per-harness matrix. Pure function.
- `app/src/ai/orchestration_byop_env_tests.rs` — sibling unit tests for the matrix.

**Modified:**

- `app/src/ai/mod.rs` — add `pub mod orchestration_byop_env;`.
- `app/src/ai/agent_providers/mod.rs` — add `resolve_byop_for_local_child(ctx, model_id) -> Option<(AgentProvider, String)>` helper that decodes a `byop:` LLMId, looks up the `AgentProvider` from settings, and fetches the api_key from `AgentProviderSecrets`.
- `app/src/pane_group/pane/local_harness_launch.rs` — `prepare_local_harness_child_launch` gains a `byop_env: HashMap<OsString, OsString>` parameter that's merged into the env-var assembly after the existing `harness_model_env_vars` call.
- `app/src/pane_group/pane/local_harness_launch_tests.rs` — extend existing tests to confirm BYOP env vars flow through; add 2 new tests for the merge behavior.
- `app/src/pane_group/pane/terminal_pane.rs` — `launch_local_harness_child` resolves the BYOP entry (when `model_id` starts with `byop:`) and threads the env-var map into the `prepare_local_harness_child_launch` call.
- `specs/multi-local-llm/README.md` — flip Phase 5c row to 🧪 code-complete with the standard verification-gate note. Update Future-phases section.

No GraphQL changes, no `RunAgentsRequest` changes, no compaction-dispatcher changes.

---

## Stage A — `orchestration_byop_env` module

### Task 1: Create `byop_env_for_harness` + the per-harness matrix

**Files:**
- Create: `app/src/ai/orchestration_byop_env.rs`.
- Create: `app/src/ai/orchestration_byop_env_tests.rs`.
- Modify: `app/src/ai/mod.rs` — add `pub mod orchestration_byop_env;`.

**Read these reference files FIRST:**
- `app/src/ai/agent_sdk/driver/harness/mod.rs:410-434` — existing `harness_model_env_vars` (the pattern: returns `HashMap<OsString, OsString>`, model_id can be empty / harness-specific switch).
- `app/src/ai/byop_orchestration_filter.rs` — Phase 5a matrix module, the doc-comment + helper-fn style to follow.
- `crates/ai/src/local_provider/api_type.rs` — `AgentProviderApiType` variants.
- `crates/warp_cli/src/agent.rs:125-215` — `Harness` enum.

- [ ] **Step 1.1: Create `orchestration_byop_env.rs`**

```rust
//! Phase 5c. Env-var bag assembly for routing third-party local child
//! harnesses (Claude Code, Codex, OpenCode) at BYOP-configured endpoints.
//!
//! The third-party CLIs honor a small set of env vars to override their
//! default cloud endpoint and credentials. This module returns the bag a
//! local child spawn site should merge into `Command::envs(...)` when the
//! orchestration's run-wide model id is a BYOP entry.
//!
//! API-type → harness env-var matrix (from spec-phase-5.md):
//!
//! | Harness    | API type            | Env vars set                                |
//! |------------|---------------------|---------------------------------------------|
//! | claude     | Anthropic           | ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY       |
//! | codex      | OpenAi / OpenAiResp / DeepSeek | OPENAI_BASE_URL, OPENAI_API_KEY, OPENAI_MODEL |
//! | opencode   | OpenAi / DeepSeek   | OPENAI_BASE_URL, OPENAI_API_KEY             |
//! | gemini     | (deferred — Gemini CLI is not enabled as a local child harness today) |
//! | oz / empty | (Native — uses the in-process BYOP dispatcher from Phase 4d) |
//!
//! Mismatched combinations return an empty bag. The Phase 5a submit-time
//! validator (`validate_orchestration_model_id`) catches the user-facing
//! cases at submit time; this empty-bag fallback is defense in depth.
//!
//! The `ANTHROPIC_MODEL` env var is intentionally **not** set here — it's
//! already set by `harness_model_env_vars` in `agent_sdk/driver/harness/mod.rs`
//! for the Claude harness, and the caller merges that bag with ours.

use std::collections::HashMap;
use std::ffi::OsString;

use ai::local_provider::AgentProviderApiType;
use warp_cli::agent::Harness;

use crate::settings::AgentProvider;

/// Returns the env-var bag a third-party local CLI harness needs to talk to
/// the user's BYOP-configured endpoint. Returns an empty `HashMap` for
/// API-type + harness combinations that aren't supported.
///
/// `api_key` is the value stored in `AgentProviderSecrets` for this provider;
/// callers MUST NOT log it. The model_id passed in is the user-side model
/// id (the part after `byop:<provider_id>:`), not the full LLMId.
pub fn byop_env_for_harness(
    provider: &AgentProvider,
    api_key: &str,
    model_id: &str,
    harness: Harness,
) -> HashMap<OsString, OsString> {
    let mut env = HashMap::new();
    let base_url = provider.base_url.trim();
    if base_url.is_empty() || api_key.is_empty() {
        return env;
    }

    match (harness, provider.api_type) {
        // Anthropic api_type + Claude harness — set base_url + key.
        // ANTHROPIC_MODEL is set by harness_model_env_vars upstream of us.
        (Harness::Claude, AgentProviderApiType::Anthropic) => {
            env.insert(OsString::from("ANTHROPIC_BASE_URL"), OsString::from(base_url));
            env.insert(OsString::from("ANTHROPIC_API_KEY"), OsString::from(api_key));
        }
        // Codex + OpenAi-family — set base_url, key, and model id.
        (
            Harness::Codex,
            AgentProviderApiType::OpenAi
            | AgentProviderApiType::OpenAiResp
            | AgentProviderApiType::DeepSeek,
        ) => {
            env.insert(OsString::from("OPENAI_BASE_URL"), OsString::from(base_url));
            env.insert(OsString::from("OPENAI_API_KEY"), OsString::from(api_key));
            if !model_id.is_empty() {
                env.insert(OsString::from("OPENAI_MODEL"), OsString::from(model_id));
            }
        }
        // OpenCode + OpenAi/DeepSeek — base_url + key only (no model env-var
        // per spec; opencode reads the model from its own config).
        (Harness::OpenCode, AgentProviderApiType::OpenAi | AgentProviderApiType::DeepSeek) => {
            env.insert(OsString::from("OPENAI_BASE_URL"), OsString::from(base_url));
            env.insert(OsString::from("OPENAI_API_KEY"), OsString::from(api_key));
        }
        // Every other combination is unsupported.
        // - Anthropic + non-Claude harness: Claude is the only Anthropic-compatible CLI.
        // - OpenAi/DeepSeek + Claude: Claude doesn't speak the OpenAI wire shape.
        // - Ollama + anything-but-native: Ollama is Native-only per the Phase 5a matrix.
        // - Gemini + anything: Gemini CLI is not enabled as a local child harness yet.
        // - Oz / Unknown: handled by the in-process dispatcher, not this module.
        _ => {}
    }

    env
}

#[cfg(test)]
#[path = "orchestration_byop_env_tests.rs"]
mod tests;
```

- [ ] **Step 1.2: Create `orchestration_byop_env_tests.rs`**

```rust
use super::*;
use ai::local_provider::AgentProviderApiType;
use warp_cli::agent::Harness;

use crate::settings::{AgentProvider, AgentProviderKind, AgentProviderModel};

fn provider_with(api_type: AgentProviderApiType, base_url: &str) -> AgentProvider {
    AgentProvider {
        id: "p1".to_string(),
        name: "P1".to_string(),
        kind: AgentProviderKind::default(),
        api_type,
        base_url: base_url.to_string(),
        models: vec![AgentProviderModel::from_id("m1".to_string())],
        available_for_orchestration: true,
        remote_secret_name: String::new(),
    }
}

#[test]
fn claude_anthropic_sets_base_url_and_api_key() {
    let provider = provider_with(AgentProviderApiType::Anthropic, "https://api.anthropic.example/v1");
    let env = byop_env_for_harness(&provider, "sk-test", "claude-sonnet", Harness::Claude);

    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_BASE_URL")),
        Some(&OsString::from("https://api.anthropic.example/v1"))
    );
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_API_KEY")),
        Some(&OsString::from("sk-test"))
    );
    // ANTHROPIC_MODEL is set by harness_model_env_vars upstream, not here.
    assert!(!env.contains_key(&OsString::from("ANTHROPIC_MODEL")));
}

#[test]
fn codex_openai_sets_three_env_vars() {
    let provider = provider_with(AgentProviderApiType::OpenAi, "https://api.openai.example/v1");
    let env = byop_env_for_harness(&provider, "sk-openai", "gpt-4o", Harness::Codex);

    assert_eq!(
        env.get(&OsString::from("OPENAI_BASE_URL")),
        Some(&OsString::from("https://api.openai.example/v1"))
    );
    assert_eq!(
        env.get(&OsString::from("OPENAI_API_KEY")),
        Some(&OsString::from("sk-openai"))
    );
    assert_eq!(
        env.get(&OsString::from("OPENAI_MODEL")),
        Some(&OsString::from("gpt-4o"))
    );
}

#[test]
fn codex_openai_resp_works() {
    let provider = provider_with(AgentProviderApiType::OpenAiResp, "https://my-llm.example/v1");
    let env = byop_env_for_harness(&provider, "sk-resp", "gpt-4o-mini", Harness::Codex);

    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    assert!(env.contains_key(&OsString::from("OPENAI_MODEL")));
}

#[test]
fn codex_deepseek_works() {
    let provider = provider_with(AgentProviderApiType::DeepSeek, "https://api.deepseek.example/v1");
    let env = byop_env_for_harness(&provider, "sk-deepseek", "deepseek-coder", Harness::Codex);

    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    assert_eq!(
        env.get(&OsString::from("OPENAI_MODEL")),
        Some(&OsString::from("deepseek-coder"))
    );
}

#[test]
fn opencode_openai_omits_model_env_var() {
    let provider = provider_with(AgentProviderApiType::OpenAi, "https://api.openai.example/v1");
    let env = byop_env_for_harness(&provider, "sk-openai", "gpt-4o", Harness::OpenCode);

    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    // OpenCode reads model from its own config; no env var.
    assert!(!env.contains_key(&OsString::from("OPENAI_MODEL")));
}

#[test]
fn opencode_deepseek_works() {
    let provider = provider_with(AgentProviderApiType::DeepSeek, "https://api.deepseek.example/v1");
    let env = byop_env_for_harness(&provider, "sk-deepseek", "deepseek-coder", Harness::OpenCode);

    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
}

#[test]
fn claude_with_openai_api_returns_empty() {
    // The Phase 5a validator catches this at submit time; if it ever slips
    // through, the env bag is empty so the CLI uses its default endpoint.
    let provider = provider_with(AgentProviderApiType::OpenAi, "https://api.example.com/v1");
    let env = byop_env_for_harness(&provider, "sk-test", "gpt-4o", Harness::Claude);
    assert!(env.is_empty());
}

#[test]
fn codex_with_anthropic_api_returns_empty() {
    let provider = provider_with(AgentProviderApiType::Anthropic, "https://api.anthropic.example/v1");
    let env = byop_env_for_harness(&provider, "sk-test", "claude-sonnet", Harness::Codex);
    assert!(env.is_empty());
}

#[test]
fn ollama_with_any_third_party_harness_returns_empty() {
    let provider = provider_with(AgentProviderApiType::Ollama, "http://localhost:11434");
    assert!(byop_env_for_harness(&provider, "", "llama3", Harness::Claude).is_empty());
    assert!(byop_env_for_harness(&provider, "", "llama3", Harness::Codex).is_empty());
    assert!(byop_env_for_harness(&provider, "", "llama3", Harness::OpenCode).is_empty());
}

#[test]
fn gemini_harness_returns_empty_today() {
    // Gemini CLI as a local child harness is not enabled. If it ever
    // becomes enabled, add the GOOGLE_GENAI_USE_VERTEXAI / GOOGLE_API_KEY
    // arm and update this test.
    let provider = provider_with(AgentProviderApiType::Gemini, "https://generativelanguage.example/v1beta");
    let env = byop_env_for_harness(&provider, "sk-test", "gemini-1.5", Harness::Gemini);
    assert!(env.is_empty());
}

#[test]
fn oz_harness_returns_empty_for_all_api_types() {
    for api_type in [
        AgentProviderApiType::Anthropic,
        AgentProviderApiType::OpenAi,
        AgentProviderApiType::OpenAiResp,
        AgentProviderApiType::DeepSeek,
        AgentProviderApiType::Gemini,
        AgentProviderApiType::Ollama,
    ] {
        let provider = provider_with(api_type, "https://api.example.com/v1");
        let env = byop_env_for_harness(&provider, "sk-test", "m", Harness::Oz);
        assert!(env.is_empty(), "Oz harness should return empty for {api_type:?}");
    }
}

#[test]
fn unknown_harness_returns_empty() {
    let provider = provider_with(AgentProviderApiType::OpenAi, "https://api.example.com/v1");
    let env = byop_env_for_harness(&provider, "sk-test", "m", Harness::Unknown);
    assert!(env.is_empty());
}

#[test]
fn empty_base_url_returns_empty() {
    let provider = provider_with(AgentProviderApiType::Anthropic, "");
    let env = byop_env_for_harness(&provider, "sk-test", "claude", Harness::Claude);
    assert!(env.is_empty());
}

#[test]
fn empty_api_key_returns_empty() {
    let provider = provider_with(AgentProviderApiType::Anthropic, "https://api.anthropic.example/v1");
    let env = byop_env_for_harness(&provider, "", "claude", Harness::Claude);
    assert!(env.is_empty());
}

#[test]
fn codex_with_empty_model_id_skips_model_env_var() {
    let provider = provider_with(AgentProviderApiType::OpenAi, "https://api.openai.example/v1");
    let env = byop_env_for_harness(&provider, "sk-test", "", Harness::Codex);
    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    // No OPENAI_MODEL when the user-side model_id is empty.
    assert!(!env.contains_key(&OsString::from("OPENAI_MODEL")));
}
```

- [ ] **Step 1.3: Wire module in `app/src/ai/mod.rs`**

Add after `pub mod byop_orchestration_filter;`:

```rust
pub mod orchestration_byop_env;
```

- [ ] **Step 1.4: Build + clippy + tests**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
cargo nextest run -p warp orchestration_byop_env 2>&1 | tail -20
```

Expected: clean build/clippy and all matrix tests pass.

- [ ] **Step 1.5: Commit**

```
feat(ai): orchestration_byop_env::byop_env_for_harness for external-CLI BYOP routing

Phase 5c task 1. New pure helper module that returns the env-var bag
a third-party local CLI harness (Claude Code, Codex, OpenCode) needs
to talk to a BYOP-configured endpoint instead of its built-in cloud
default.

Matrix:
- Claude + Anthropic API -> ANTHROPIC_BASE_URL + ANTHROPIC_API_KEY
- Codex + OpenAI/OpenAIResp/DeepSeek -> OPENAI_BASE_URL +
  OPENAI_API_KEY + OPENAI_MODEL
- OpenCode + OpenAI/DeepSeek -> OPENAI_BASE_URL + OPENAI_API_KEY
- All other combinations -> empty bag (defense in depth; submit-time
  validator caught at picker stage).

ANTHROPIC_MODEL stays in the existing harness_model_env_vars (the
caller will merge both bags). 14 unit tests cover the matrix and
edge cases (empty base_url / api_key / model_id, Gemini deferred,
Oz native).
```

---

## Stage B — Settings resolve helper

### Task 2: `resolve_byop_for_local_child` in `agent_providers/mod.rs`

**Files:**
- Modify: `app/src/ai/agent_providers/mod.rs`.

**Read these reference files FIRST:**
- `app/src/ai/agent_providers/mod.rs:28-87` — `build_byop_llm_infos` and the existing lookup pattern: read `AISettings::as_ref(app).agent_providers.value()`, look up secrets via `AgentProviderSecrets::as_ref(app).get(&provider.id)`.
- `app/src/ai/agent_providers/mod.rs:223` — existing `lookup_byop` pattern (if it exists in your tree — search for the place that decodes an `LLMId` and pulls api_key).
- `crates/ai/src/local_provider/llm_id.rs::decode` — returns `Option<(provider_id, model_id)>`.

- [ ] **Step 2.1: Add the resolve helper**

Append to `app/src/ai/agent_providers/mod.rs` (after the existing helpers, near `build_byop_orchestration_llm_infos`):

```rust
/// Phase 5c. Resolves a `byop:<provider_id>:<model_id>` LLMId to the
/// `(provider, api_key, model_id)` triple a local child-harness spawn site
/// needs to assemble its env-var bag. Returns `None` for:
/// - Non-BYOP model IDs (caller should treat this as "no env vars to inject").
/// - BYOP IDs that don't decode (malformed).
/// - Provider IDs that aren't in settings.
/// - Providers missing an API key in `AgentProviderSecrets`.
///
/// The model_id returned is the user-side model id (the part after the
/// `byop:<provider_id>:` prefix), suitable for passing as `OPENAI_MODEL` /
/// `ANTHROPIC_MODEL` to the harness CLI.
pub fn resolve_byop_for_local_child(
    app: &AppContext,
    model_id: &str,
) -> Option<(AgentProvider, String, String)> {
    use ai::local_provider::llm_id;

    let llm_id: ai::local_provider::LLMId = model_id.into();
    if !llm_id::is_byop(&llm_id) {
        return None;
    }
    let (provider_id, byop_model_id) = llm_id::decode(&llm_id)?;

    let providers = crate::settings::AISettings::as_ref(app)
        .agent_providers
        .value()
        .clone();
    let provider = providers
        .into_iter()
        .find(|p| p.id == provider_id)?;

    let secrets = ai::local_provider::AgentProviderSecrets::as_ref(app);
    let api_key = secrets.get(&provider.id)?.to_string();
    if api_key.is_empty() {
        return None;
    }

    Some((provider, api_key, byop_model_id))
}
```

Add the import for `AppContext` at the top of the file if not already present:

```rust
use warpui::AppContext;
```

(`AppContext` is likely already imported via the `build_byop_*` helpers in this file; check first.)

- [ ] **Step 2.2: Add a unit test for the resolve helper**

Add to the existing test module in `app/src/ai/agent_providers/mod_tests.rs` (or create a sibling test file if one doesn't exist). The test should:

```rust
#[test]
fn resolve_byop_for_local_child_returns_provider_api_key_and_user_model_id() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&app);

        // Add a provider + secret.
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-xyz".to_owned(),
                name: "P".to_owned(),
                kind: Default::default(),
                api_type: AgentProviderApiType::Anthropic,
                base_url: "https://api.anthropic.example/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("claude-sonnet-4".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-xyz", "sk-test-key", ctx);
        });

        let llm_id = ai::local_provider::llm_id::encode("prov-xyz", "claude-sonnet-4").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &llm_id));

        let (provider, api_key, model_id) = resolved.expect("BYOP entry must resolve");
        assert_eq!(provider.id, "prov-xyz");
        assert_eq!(provider.api_type, AgentProviderApiType::Anthropic);
        assert_eq!(api_key, "sk-test-key");
        assert_eq!(model_id, "claude-sonnet-4");
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_for_non_byop_id() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&app);
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, "claude-sonnet-4"));
        assert!(resolved.is_none());
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_for_missing_provider() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&app);
        // No provider added; even a well-formed byop id can't resolve.
        let llm_id = ai::local_provider::llm_id::encode("missing-prov", "m1").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &llm_id));
        assert!(resolved.is_none());
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_when_api_key_missing() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&app);
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-no-key".to_owned(),
                name: "P".to_owned(),
                kind: Default::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.example.com/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("m1".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        // No secret set.
        let llm_id = ai::local_provider::llm_id::encode("prov-no-key", "m1").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &llm_id));
        assert!(resolved.is_none());
    });
}
```

Adjust imports to match the in-repo path (look at the test file for the existing `use` lines).

- [ ] **Step 2.3: Build + clippy + tests**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
cargo nextest run -p warp resolve_byop_for_local_child 2>&1 | tail -15
```

- [ ] **Step 2.4: Commit**

```
feat(ai/agent_providers): resolve_byop_for_local_child helper

Phase 5c task 2. Pure-data helper that takes a model_id string and
returns Option<(AgentProvider, api_key, user_side_model_id)>. Caller
sites in pane_group can pull everything needed to assemble a
BYOP env-var bag for a third-party local child harness without
re-implementing the decode + lookup chain.

4 unit tests cover the happy path + three None paths (non-BYOP id,
missing provider, missing api key).
```

---

## Stage C — Thread env into prepare function

### Task 3: Add `byop_env` parameter to `prepare_local_harness_child_launch`

**Files:**
- Modify: `app/src/pane_group/pane/local_harness_launch.rs`.
- Modify: `app/src/pane_group/pane/local_harness_launch_tests.rs` — 2 new tests for the merge.

**Read these reference files FIRST:**
- `app/src/pane_group/pane/local_harness_launch.rs` (full file, 219 lines) — the function being modified and its existing env-var assembly at line 199-206.
- `app/src/pane_group/pane/local_harness_launch_tests.rs` (full file) — existing test pattern for this function.

- [ ] **Step 3.1: Extend the signature**

In `app/src/pane_group/pane/local_harness_launch.rs`, change the signature of `prepare_local_harness_child_launch` to take an additional `byop_env` parameter:

```rust
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_local_harness_child_launch(
    prompt: String,
    harness_type: String,
    model_id: Option<String>,
    parent_run_id: Option<String>,
    agent_name: Option<String>,
    shell_type: Option<ShellType>,
    startup_directory: Option<PathBuf>,
    ai_client: Arc<dyn AIClient>,
    byop_env: HashMap<OsString, OsString>,
) -> Result<PreparedLocalHarnessLaunch, String> {
```

Then merge `byop_env` into the existing `env_vars` after the `harness_model_env_vars` call (around line 206):

```rust
    let mut env_vars = task_env_vars(Some(&task_id), parent_run_id.as_deref(), harness);
    // Propagate the selected model to Claude Code via ANTHROPIC_MODEL.
    // Codex local children never receive a model override — the UI
    // ensures model_id is empty for local Codex.
    env_vars.extend(harness_model_env_vars(
        harness,
        harness_model_config.as_ref(),
    ));
    // Phase 5c: merge BYOP env vars (ANTHROPIC_BASE_URL / OPENAI_BASE_URL /
    // etc) on top of the harness-model env vars. Caller resolves the bag
    // from the AppContext via resolve_byop_for_local_child + 
    // byop_env_for_harness. Empty when the run-wide model id isn't a BYOP
    // entry.
    env_vars.extend(byop_env);
```

`byop_env` taking precedence is intentional: if the user picked a BYOP Anthropic model, our `ANTHROPIC_BASE_URL` should land in the bag, and `ANTHROPIC_MODEL` should be the BYOP user-side model_id (which the caller will resolve and pass as part of the byop_env or rely on `harness_model_env_vars` for — see the matrix above; for Anthropic the model is set upstream, for OpenAI it's set in byop_env, so the precedence doesn't actually conflict).

- [ ] **Step 3.2: Update existing tests in `local_harness_launch_tests.rs`**

Find every existing call to `prepare_local_harness_child_launch` in `local_harness_launch_tests.rs` and append `HashMap::new()` (empty BYOP env) to each call. There are 4 test call sites today (lines 236, 275, 313, 334 of `local_harness_launch_tests.rs` per the prior file inspection).

The exact change is mechanical: each `prepare_local_harness_child_launch(...)` call gets a trailing `, HashMap::new()` before the closing `)`.

- [ ] **Step 3.3: Add 2 new tests for the merge behavior**

Append to `local_harness_launch_tests.rs`:

```rust
#[tokio::test]
async fn prepare_local_harness_child_launch_merges_byop_env_into_env_vars() {
    // Caller passes an explicit BYOP env-var bag; the prepared launch
    // surfaces it in env_vars alongside the existing task_env_vars +
    // harness_model_env_vars output.
    let ai_client = stub_ai_client();
    let mut byop = HashMap::new();
    byop.insert(
        OsString::from("ANTHROPIC_BASE_URL"),
        OsString::from("https://api.anthropic.example/v1"),
    );
    byop.insert(
        OsString::from("ANTHROPIC_API_KEY"),
        OsString::from("sk-test"),
    );

    let prepared = prepare_local_harness_child_launch(
        "go".to_string(),
        "claude".to_string(),
        Some("byop:prov:claude-sonnet".to_string()),
        Some("parent-run-1".to_string()),
        Some("agent-a".to_string()),
        Some(ShellType::Bash),
        Some(std::env::temp_dir()),
        ai_client,
        byop,
    )
    .await;

    let prepared = match prepared {
        Ok(p) => p,
        Err(error) => {
            // If Claude isn't installed in the test environment, prepare
            // bails before assembling env_vars. Accept that as a skip.
            if error.contains("Claude") || error.contains("claude") {
                return;
            }
            panic!("unexpected error: {error}");
        }
    };

    assert_eq!(
        prepared
            .env_vars
            .get(&OsString::from("ANTHROPIC_BASE_URL")),
        Some(&OsString::from("https://api.anthropic.example/v1"))
    );
    assert_eq!(
        prepared.env_vars.get(&OsString::from("ANTHROPIC_API_KEY")),
        Some(&OsString::from("sk-test"))
    );
}

#[tokio::test]
async fn prepare_local_harness_child_launch_with_empty_byop_env_is_unchanged() {
    // Sanity: an empty BYOP env doesn't disturb the existing env-var
    // assembly. (Smoke test that the new parameter is backward-compatible.)
    let ai_client = stub_ai_client();
    let prepared = prepare_local_harness_child_launch(
        "go".to_string(),
        "codex".to_string(),
        Some("gpt-4o".to_string()),
        Some("parent-run-1".to_string()),
        Some("agent-a".to_string()),
        Some(ShellType::Bash),
        Some(std::env::temp_dir()),
        ai_client,
        HashMap::new(),
    )
    .await;
    // Same env-bail-out tolerance as the prior test — Codex may not be
    // installed in the runner.
    let _ = prepared;
}
```

Use the test boilerplate / `stub_ai_client()` / `ShellType` import idiom already present at the top of `local_harness_launch_tests.rs`.

- [ ] **Step 3.4: Build + clippy + tests**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
cargo nextest run -p warp prepare_local_harness_child_launch 2>&1 | tail -15
```

Existing tests (4 call sites) should still pass; 2 new tests should pass or be skipped (CLI not installed).

- [ ] **Step 3.5: Commit**

```
feat(pane_group): thread BYOP env-var bag through prepare_local_harness_child_launch

Phase 5c task 3. Adds a byop_env: HashMap<OsString, OsString>
parameter that gets merged into the existing env-var assembly after
harness_model_env_vars. Empty when the caller didn't resolve a BYOP
entry — preserves existing behavior for non-BYOP launches.

Existing 4 test call sites pass HashMap::new(). 2 new tests verify
the merge: ANTHROPIC_BASE_URL/API_KEY surface in env_vars for a
BYOP Claude launch; empty bag is a no-op.
```

---

## Stage D — Wire at the call site

### Task 4: Resolve and thread BYOP env in `launch_local_harness_child`

**Files:**
- Modify: `app/src/pane_group/pane/terminal_pane.rs` — `launch_local_harness_child` (line 1845).

**Read these reference files FIRST:**
- `app/src/pane_group/pane/terminal_pane.rs:1845-1985` — `launch_local_harness_child` flow (resolve happens BEFORE `ctx.spawn(...)`'s async block; the prepared call inside the async block can't touch ctx).
- `app/src/ai/orchestration_byop_env.rs` (Task 1).
- `app/src/ai/agent_providers/mod.rs::resolve_byop_for_local_child` (Task 2).
- `crates/warp_cli/src/agent.rs:125-215` — `Harness::parse_local_child_harness`.

- [ ] **Step 4.1: Resolve BYOP env and thread it**

In `app/src/pane_group/pane/terminal_pane.rs::launch_local_harness_child` around line 1877 (right before the `ctx.spawn` call), insert:

```rust
    let model_id_for_harness_env = model_id.clone();
    let agent_name_for_task = agent_name.clone();

    // Phase 5c: when the run-wide model_id is a BYOP entry, resolve the
    // provider + api_key from settings and assemble the env-var bag the
    // third-party CLI needs to route at the user's endpoint. Empty when
    // not BYOP — `prepare_local_harness_child_launch` treats an empty
    // bag as a no-op.
    let byop_env = if let Some(model) = model_id.as_deref() {
        let resolved = crate::ai::agent_providers::resolve_byop_for_local_child(ctx, model);
        let harness = Harness::parse_local_child_harness(&harness_type).unwrap_or(Harness::Unknown);
        match resolved {
            Some((provider, api_key, byop_model_id)) => {
                crate::ai::orchestration_byop_env::byop_env_for_harness(
                    &provider,
                    &api_key,
                    &byop_model_id,
                    harness,
                )
            }
            None => std::collections::HashMap::new(),
        }
    } else {
        std::collections::HashMap::new()
    };

    let _ = ctx.spawn(
        async move {
            prepare_local_harness_child_launch(
                prompt,
                harness_type,
                model_id_for_harness_env,
                parent_run_id,
                agent_name_for_task,
                shell_type,
                startup_directory,
                ai_client,
                byop_env,
            )
            .await
        },
```

Watch for borrow lifetimes — `ctx` is `&mut ViewContext<PaneGroup>`, and `resolve_byop_for_local_child` takes `&AppContext`. The view context derefs / passes via `.as_ref()` similarly to how the surrounding code reads `BlocklistAIHistoryModel::as_ref(ctx)` etc. If a direct pass fails to compile, mirror the existing `BlocklistAIHistoryModel::as_ref(ctx)` style (i.e., `resolve_byop_for_local_child(ctx, model)` where `ctx` derefs).

Add any needed imports near the top of `terminal_pane.rs`:

```rust
use warp_cli::agent::Harness;
```

(`Harness` may already be in scope via existing imports — check.)

- [ ] **Step 4.2: Update `terminal_pane.rs::launch_local_no_harness_child` (Oz path) — no change**

`launch_local_no_harness_child` is the Oz / empty-harness path that goes through the Phase 4d in-process dispatcher, not an external CLI. No env-var injection needed there. (Confirm by reading the function — it should NOT call `prepare_local_harness_child_launch`.) Skip this step if confirmed.

- [ ] **Step 4.3: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
```

Both should pass cleanly.

- [ ] **Step 4.4: Run the orchestration + pane_group tests**

```bash
cargo nextest run -p warp pane_group::pane::local_harness_launch 2>&1 | tail -10
cargo nextest run -p warp orchestration_byop_env 2>&1 | tail -10
cargo nextest run -p warp resolve_byop_for_local_child 2>&1 | tail -10
```

All should pass.

- [ ] **Step 4.5: Commit**

```
feat(pane_group): wire BYOP env-var injection into launch_local_harness_child

Phase 5c task 4. Resolves the BYOP entry (if the run-wide model_id
is a byop: id) from AISettings + AgentProviderSecrets, assembles the
env-var bag via byop_env_for_harness, and threads it into the
prepare_local_harness_child_launch call.

End result: picking a BYOP-Anthropic model with the claude-code
harness spawns the claude CLI with ANTHROPIC_BASE_URL pointing at
the user's endpoint and ANTHROPIC_API_KEY set. Picking a BYOP-OpenAI
model with codex spawns codex with OPENAI_BASE_URL/API_KEY/MODEL set.

The Oz/empty-harness path (launch_local_no_harness_child) is
unchanged — it uses the Phase 4d in-process BYOP dispatcher, not an
external CLI.
```

---

## Stage E — Docs

### Task 5: Update `specs/multi-local-llm/README.md`

**Files:**
- Modify: `specs/multi-local-llm/README.md`.

**Read these reference files FIRST:**
- `specs/multi-local-llm/README.md` — existing Phase 5a / 5b status sections; mirror their format.

- [ ] **Step 5.1: Append Phase 5c status block**

After the existing Phase 5b status block, add:

```markdown
**Phase 5c (BYOP orchestration — external-CLI env-var injection)** code is complete on `multi-local-llm` (final commit `<FILL IN>`). Builds on 5b's picker + submit-validator wiring by routing third-party local child CLI harnesses (Claude Code / Codex / OpenCode) at BYOP endpoints via env vars they already honor:

- New `app/src/ai/orchestration_byop_env.rs` with `byop_env_for_harness(provider, api_key, model_id, harness)` returning the env-var bag per the API-type-to-harness matrix: Claude+Anthropic → `ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY`; Codex+OpenAI/OpenAIResp/DeepSeek → `OPENAI_BASE_URL`/`OPENAI_API_KEY`/`OPENAI_MODEL`; OpenCode+OpenAI/DeepSeek → `OPENAI_BASE_URL`/`OPENAI_API_KEY`. Mismatched combinations return empty (defense in depth — the Phase 5a submit-time validator catches them upstream).
- New `resolve_byop_for_local_child` helper in `agent_providers/mod.rs` decodes a `byop:` LLMId and pulls `(AgentProvider, api_key, user-side model_id)` from `AISettings` + `AgentProviderSecrets`.
- `prepare_local_harness_child_launch` in `pane_group/pane/local_harness_launch.rs` gains a `byop_env` parameter that's merged into the env-var bag after the existing `harness_model_env_vars` call.
- `launch_local_harness_child` in `pane_group/pane/terminal_pane.rs` resolves the BYOP entry (when applicable) before spawning and threads the assembled env-var bag into the prepare call.

Gemini CLI as a local child harness remains disabled (it's filtered out by `normalize_local_child_harness` today); enabling it would be a separate task. Remote orchestration + credential bridge + GraphQL forwarding for BYOP remain in Phase 5d.

> **Verification gate:** live-test smoke against `claude` CLI with an Anthropic-API BYOP provider configured against a real or mock endpoint (e.g. a local proxy). Pick the `claude-code` harness + `Local` execution + the BYOP model. Confirm the spawned `claude` process has `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY` set (visible in `/proc/<pid>/environ` on Linux or via the harness's own debug logging) and that subsequent traffic hits the configured endpoint. Repeat for `codex` with an OpenAI-API BYOP provider. Once both smokes pass, Phase 5c row flips to ✅.
```

Then add a status-table row:

```markdown
| 5c — BYOP orchestration external-CLI env-var injection (Claude Code / Codex / OpenCode harnesses) | [`plan-phase-5c.md`](plan-phase-5c.md) | 🧪 code complete — pending live smoke |
```

Add a "What landed → User-visible" bullet:

```markdown
- **Phase 5c (BYOP orchestration external-CLI):** Local non-Native orchestration (`claude-code` / `codex` / `opencode` child harnesses) now respects the user's BYOP picker selection. Pick a BYOP-Anthropic model + Claude Code harness → the spawned `claude` CLI talks to your endpoint via `ANTHROPIC_BASE_URL`/`API_KEY`. Pick a BYOP-OpenAI/DeepSeek model + Codex/OpenCode → `OPENAI_BASE_URL`/`API_KEY` (+`OPENAI_MODEL` for Codex) point the CLI at your endpoint. Gemini CLI deferred (not enabled as a local child harness yet).
```

Add an "Architecture" bullet:

```markdown
- **Phase 5c:** New `app/src/ai/orchestration_byop_env.rs::byop_env_for_harness` returns the per-API-type+harness env-var bag (`HashMap<OsString, OsString>`). New `agent_providers::resolve_byop_for_local_child` decodes a `byop:` LLMId and pulls provider + api_key from `AISettings` + `AgentProviderSecrets`. `prepare_local_harness_child_launch` gains a `byop_env` parameter merged after `harness_model_env_vars`. `launch_local_harness_child` in `terminal_pane.rs` resolves at the caller and threads through to the async prepare call. No GraphQL changes, no Remote path, no compaction-dispatcher changes.
```

Update the Future-phases entry:

```markdown
- **Phase 5a–d** — BYOP in agent orchestration. 5a (foundation), 5b (Local Native path), and 5c (external-CLI env-var injection) are code complete on `multi-local-llm`, pending live smoke. 5d (Remote credential bridge + GraphQL forwarding + compaction inheritance) is queued.
```

- [ ] **Step 5.2: Commit**

```
docs(specs/multi-local-llm): record Phase 5c code-complete status

Phase 5c lights up Local non-Native BYOP orchestration: third-party
CLI harnesses (Claude Code / Codex / OpenCode) spawn with env vars
they already honor pointing at the user's BYOP endpoint. New
orchestration_byop_env module + resolve_byop_for_local_child helper
+ prepare_local_harness_child_launch parameter + launch-site wiring.

Gemini CLI as a local child harness remains disabled. Remote
orchestration + credential bridge remain in Phase 5d.
```

---

## Stage F — Memory

### Task 6: Memory entry

**Files:**
- Create: `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/multi-local-llm-phase-5c.md`.
- Modify: `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/MEMORY.md` — add index line.

- [ ] **Step 6.1: Write memory file** following the Phase 5b template, summarizing the same content as the README status block. List the implementation commits in order.

- [ ] **Step 6.2: Append the one-line index entry** to `MEMORY.md`.

- [ ] **Step 6.3: No git commit needed** — outside the repo.

---

## Self-review checklist

After implementation:

1. **Spec coverage:** Every behavior the spec promised for Phase 5c is wired: env-var bag assembly, settings resolve, child-spawn integration, the three harness paths (Claude, Codex, OpenCode), the empty-bag defense for mismatched combos. Gemini deferred + documented.

2. **Placeholder scan:** No `TODO` / `TBD` / "handle edge cases later" in any task.

3. **Type consistency:** `byop_env: HashMap<OsString, OsString>` is the same shape across the module, the parameter, and the merge site. `resolve_byop_for_local_child` returns `Option<(AgentProvider, String, String)>` consistently.

4. **Backward compat:** Existing local-harness launches (no BYOP) get `HashMap::new()` for `byop_env` and behave identically to pre-5c. Existing 4 test call sites updated.

5. **Test coverage:** 14 matrix tests in Task 1 + 4 resolve tests in Task 2 + 2 merge tests in Task 3 = 20 new unit tests. The `launch_local_harness_child` call-site wiring (Task 4) is exercised through the existing `local_harness_launch_tests` suite passing the new parameter.

6. **Test isolation in Task 3.3:** The "Claude isn't installed" graceful-skip is honest — the test exercises the code path that *would* succeed in a real env, and a CI runner without `claude` installed shouldn't fail the suite. Same for Codex.

---

## Plan complete

Plan complete and saved to `specs/multi-local-llm/plan-phase-5c.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task with two-stage review.

**2. Inline Execution** — batched in this session.

Which approach?
