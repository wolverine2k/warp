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
//! | gemini     | (settings.json injection — Gemini CLI BYOP via ~/.gemini/settings.json; see `gemini.rs::prepare_gemini_environment_config`) |
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
    let api_key = api_key.trim();
    let model_id = model_id.trim();
    if base_url.is_empty() || api_key.is_empty() {
        return env;
    }

    match harness {
        // Anthropic api_type + Claude harness — set base_url + key.
        // ANTHROPIC_MODEL is set by harness_model_env_vars upstream of us.
        Harness::Claude => {
            if !matches!(provider.api_type, AgentProviderApiType::Anthropic) {
                return env;
            }
            env.insert(
                OsString::from("ANTHROPIC_BASE_URL"),
                OsString::from(base_url),
            );
            env.insert(OsString::from("ANTHROPIC_API_KEY"), OsString::from(api_key));
        }
        // Codex + OpenAi-family — set base_url, key, and model id.
        Harness::Codex => {
            if !matches!(
                provider.api_type,
                AgentProviderApiType::OpenAi
                    | AgentProviderApiType::OpenAiResp
                    | AgentProviderApiType::DeepSeek
            ) {
                return env;
            }
            env.insert(OsString::from("OPENAI_BASE_URL"), OsString::from(base_url));
            env.insert(OsString::from("OPENAI_API_KEY"), OsString::from(api_key));
            if !model_id.is_empty() {
                env.insert(OsString::from("OPENAI_MODEL"), OsString::from(model_id));
            }
        }
        // OpenCode + OpenAi/DeepSeek — base_url + key only (no model env-var
        // per spec; opencode reads the model from its own config).
        Harness::OpenCode => {
            if !matches!(
                provider.api_type,
                AgentProviderApiType::OpenAi | AgentProviderApiType::DeepSeek
            ) {
                return env;
            }
            env.insert(OsString::from("OPENAI_BASE_URL"), OsString::from(base_url));
            env.insert(OsString::from("OPENAI_API_KEY"), OsString::from(api_key));
        }
        // Every other combination is unsupported:
        // - Gemini CLI: not enabled as a local child harness yet.
        // - Oz / Unknown: handled by the in-process dispatcher, not this module.
        // - Ollama + any third-party harness: Ollama is Native-only per the Phase 5a matrix.
        Harness::Oz | Harness::Gemini | Harness::Unknown => {}
    }

    env
}

#[cfg(test)]
#[path = "orchestration_byop_env_tests.rs"]
mod tests;
