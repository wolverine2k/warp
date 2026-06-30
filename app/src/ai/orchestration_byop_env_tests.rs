use std::ffi::OsString;

use ai::local_provider::AgentProviderApiType;
use warp_cli::agent::Harness;

use super::byop_env_for_harness;
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
    let provider = provider_with(
        AgentProviderApiType::Anthropic,
        "https://api.anthropic.example/v1",
    );
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
    let provider = provider_with(
        AgentProviderApiType::OpenAi,
        "https://api.openai.example/v1",
    );
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
    let provider = provider_with(
        AgentProviderApiType::OpenAiResp,
        "https://my-llm.example/v1",
    );
    let env = byop_env_for_harness(&provider, "sk-resp", "gpt-4o-mini", Harness::Codex);

    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    assert!(env.contains_key(&OsString::from("OPENAI_MODEL")));
}

#[test]
fn codex_deepseek_works() {
    let provider = provider_with(
        AgentProviderApiType::DeepSeek,
        "https://api.deepseek.example/v1",
    );
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
    let provider = provider_with(
        AgentProviderApiType::OpenAi,
        "https://api.openai.example/v1",
    );
    let env = byop_env_for_harness(&provider, "sk-openai", "gpt-4o", Harness::OpenCode);

    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    // OpenCode reads model from its own config; no env var.
    assert!(!env.contains_key(&OsString::from("OPENAI_MODEL")));
}

#[test]
fn opencode_deepseek_works() {
    let provider = provider_with(
        AgentProviderApiType::DeepSeek,
        "https://api.deepseek.example/v1",
    );
    let env = byop_env_for_harness(
        &provider,
        "sk-deepseek",
        "deepseek-coder",
        Harness::OpenCode,
    );

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
    let provider = provider_with(
        AgentProviderApiType::Anthropic,
        "https://api.anthropic.example/v1",
    );
    let env = byop_env_for_harness(&provider, "sk-test", "claude-sonnet", Harness::Codex);
    assert!(env.is_empty());
}

#[test]
fn ollama_with_any_third_party_harness_returns_empty() {
    let provider = provider_with(AgentProviderApiType::Ollama, "http://localhost:11434");
    // Ollama requires api_key to be non-empty for the guard to pass, so provide one.
    // But the match arm falls through to Oz/Gemini/Unknown for all third-party harnesses
    // because Ollama has no harness arm — it returns empty.
    // For Claude/Codex/OpenCode the api_type guard rejects Ollama.
    assert!(byop_env_for_harness(&provider, "unused", "llama3", Harness::Claude).is_empty());
    assert!(byop_env_for_harness(&provider, "unused", "llama3", Harness::Codex).is_empty());
    assert!(byop_env_for_harness(&provider, "unused", "llama3", Harness::OpenCode).is_empty());
}

#[test]
fn gemini_harness_uses_settings_json_not_env_vars() {
    // Phase 5e: Gemini CLI BYOP routing goes through ~/.gemini/settings.json
    // (security.auth.apiKey + security.auth.endpoint), not env vars.
    // byop_env_for_harness intentionally returns an empty bag for any
    // (provider, Harness::Gemini) combination. The settings.json write
    // happens in app/src/ai/agent_sdk/driver/harness/gemini.rs via
    // prepare_gemini_environment_config(..., byop_config).
    let provider = provider_with(
        AgentProviderApiType::Gemini,
        "https://generativelanguage.example/v1beta",
    );
    let env = byop_env_for_harness(&provider, "sk-test", "gemini-1.5", Harness::Gemini);
    assert!(
        env.is_empty(),
        "Gemini uses settings.json — env bag must stay empty"
    );
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
        assert!(
            env.is_empty(),
            "Oz harness should return empty for {api_type:?}"
        );
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
    let provider = provider_with(
        AgentProviderApiType::Anthropic,
        "https://api.anthropic.example/v1",
    );
    let env = byop_env_for_harness(&provider, "", "claude", Harness::Claude);
    assert!(env.is_empty());
}

#[test]
fn codex_with_empty_model_id_skips_model_env_var() {
    let provider = provider_with(
        AgentProviderApiType::OpenAi,
        "https://api.openai.example/v1",
    );
    let env = byop_env_for_harness(&provider, "sk-test", "", Harness::Codex);
    assert!(env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert!(env.contains_key(&OsString::from("OPENAI_API_KEY")));
    // No OPENAI_MODEL when the user-side model_id is empty.
    assert!(!env.contains_key(&OsString::from("OPENAI_MODEL")));
}

#[test]
fn whitespace_only_api_key_is_rejected() {
    // A user who pasted whitespace into the API key field should NOT see
    // a header like `Authorization: Bearer    ` go out — the env-var bag
    // must come back empty so the CLI falls back to its default (likely
    // failing fast at the next auth call instead of leaking a malformed
    // header).
    let provider = provider_with(
        AgentProviderApiType::Anthropic,
        "https://api.anthropic.example/v1",
    );
    let env = byop_env_for_harness(&provider, "   ", "claude", Harness::Claude);
    assert!(env.is_empty());
}

#[test]
fn whitespace_padded_inputs_are_trimmed_before_insertion() {
    // base_url, api_key, and model_id all get a `.trim()` defensively so
    // a copy/pasted credential with surrounding whitespace works as the
    // user expected.
    let provider = provider_with(
        AgentProviderApiType::OpenAi,
        "  https://api.openai.example/v1  ",
    );
    let env = byop_env_for_harness(&provider, "  sk-test  ", "  gpt-4o  ", Harness::Codex);
    assert_eq!(
        env.get(&OsString::from("OPENAI_BASE_URL")),
        Some(&OsString::from("https://api.openai.example/v1"))
    );
    assert_eq!(
        env.get(&OsString::from("OPENAI_API_KEY")),
        Some(&OsString::from("sk-test"))
    );
    assert_eq!(
        env.get(&OsString::from("OPENAI_MODEL")),
        Some(&OsString::from("gpt-4o"))
    );
}
