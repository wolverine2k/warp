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
