use super::*;

fn cfg(base: &str, model: &str) -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Local".into(),
        base_url: base.into(),
        model_id: model.into(),
        api_key: None,
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::OpenAi,
    }
}

#[test]
fn validate_happy_path() {
    assert!(cfg("http://localhost:11434/v1", "llama3.1")
        .validate()
        .is_ok());
    assert!(cfg("https://api.example.com/v1", "gpt-4")
        .validate()
        .is_ok());
}

#[test]
fn validate_rejects_empty_url() {
    assert_eq!(
        cfg("", "llama3.1").validate(),
        Err(LocalProviderConfigError::EmptyBaseUrl)
    );
}

#[test]
fn validate_rejects_non_http_scheme() {
    let result = cfg("ftp://nope/v1", "llama3.1").validate();
    assert!(matches!(
        result,
        Err(LocalProviderConfigError::InvalidBaseUrl(_))
    ));
}

#[test]
fn validate_rejects_unparseable_url() {
    let result = cfg("not a url", "llama3.1").validate();
    assert!(matches!(
        result,
        Err(LocalProviderConfigError::InvalidBaseUrl(_))
    ));
}

#[test]
fn validate_rejects_empty_model() {
    assert_eq!(
        cfg("http://localhost:11434/v1", "").validate(),
        Err(LocalProviderConfigError::EmptyModelId)
    );
}

#[test]
fn synthetic_llm_id_format() {
    assert_eq!(
        cfg("http://x/v1", "llama3.1").synthetic_llm_id(),
        "local:llama3.1"
    );
    assert_eq!(
        cfg("http://x/v1", "qwen2.5-coder:7b").synthetic_llm_id(),
        "local:qwen2.5-coder:7b"
    );
}

#[test]
fn chat_completions_url_joins_path() {
    let url = cfg("http://localhost:11434/v1", "llama3.1")
        .chat_completions_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/v1/chat/completions");
}

#[test]
fn chat_completions_url_with_trailing_slash() {
    let url = cfg("http://localhost:11434/v1/", "llama3.1")
        .chat_completions_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/v1/chat/completions");
}

#[test]
fn chat_completions_url_no_path() {
    let url = cfg("http://localhost:11434", "llama3.1")
        .chat_completions_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/chat/completions");
}

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

// ---- Anthropic endpoint helpers ----

#[test]
fn messages_url_appends_v1_messages_to_bare_host() {
    let url = cfg("https://api.anthropic.com", "claude-sonnet-4-6")
        .messages_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://api.anthropic.com/v1/messages");
}

#[test]
fn messages_url_with_v1_path_is_idempotent() {
    let url = cfg("https://api.anthropic.com/v1", "claude-sonnet-4-6")
        .messages_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://api.anthropic.com/v1/messages");
}

#[test]
fn messages_url_with_v1_trailing_slash_is_idempotent() {
    let url = cfg("https://api.anthropic.com/v1/", "claude-sonnet-4-6")
        .messages_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://api.anthropic.com/v1/messages");
}

#[test]
fn messages_url_works_with_relay_base_path() {
    // Self-hosted Claude relays or enterprise gateways often live under
    // a path prefix like /anthropic; the helper should still prepend
    // /v1 if not already present.
    let url = cfg("https://relay.example.com/anthropic", "claude-sonnet-4-6")
        .messages_url()
        .unwrap();
    assert_eq!(
        url.as_str(),
        "https://relay.example.com/anthropic/v1/messages"
    );
}

#[test]
fn anthropic_models_url_appends_v1_models_to_bare_host() {
    let url = cfg("https://api.anthropic.com", "claude-sonnet-4-6")
        .anthropic_models_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://api.anthropic.com/v1/models");
}

#[test]
fn anthropic_models_url_with_v1_path_is_idempotent() {
    let url = cfg("https://api.anthropic.com/v1", "claude-sonnet-4-6")
        .anthropic_models_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://api.anthropic.com/v1/models");
}

#[test]
fn anthropic_models_url_with_v1_trailing_slash_is_idempotent() {
    let url = cfg("https://api.anthropic.com/v1/", "claude-sonnet-4-6")
        .anthropic_models_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://api.anthropic.com/v1/models");
}

// ---- Ollama endpoint helpers ----

#[test]
fn ollama_chat_url_from_default_localhost() {
    let url = cfg("http://localhost:11434", "llama3.1")
        .ollama_chat_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/api/chat");
}

#[test]
fn ollama_chat_url_with_trailing_slash() {
    let url = cfg("http://localhost:11434/", "llama3.1")
        .ollama_chat_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/api/chat");
}

#[test]
fn ollama_tags_url_from_default_localhost() {
    let url = cfg("http://localhost:11434", "llama3.1")
        .ollama_tags_url()
        .unwrap();
    assert_eq!(url.as_str(), "http://localhost:11434/api/tags");
}

#[test]
fn ollama_chat_url_works_with_relay_base_path() {
    // Self-hosted Ollama relays / reverse proxies that mount the API
    // under a path prefix.
    let url = cfg("https://relay.example.com/ollama", "llama3.1")
        .ollama_chat_url()
        .unwrap();
    assert_eq!(url.as_str(), "https://relay.example.com/ollama/api/chat");
}

// ---- Gemini endpoint helpers ----

#[test]
fn gemini_stream_generate_url_from_default_host() {
    let url = cfg(
        "https://generativelanguage.googleapis.com",
        "gemini-1.5-pro",
    )
    .gemini_stream_generate_url()
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse"
    );
}

#[test]
fn gemini_stream_generate_url_with_v1beta_path_is_idempotent() {
    let url = cfg(
        "https://generativelanguage.googleapis.com/v1beta",
        "gemini-1.5-pro",
    )
    .gemini_stream_generate_url()
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse"
    );
}

#[test]
fn gemini_stream_generate_url_with_v1beta_trailing_slash_is_idempotent() {
    let url = cfg(
        "https://generativelanguage.googleapis.com/v1beta/",
        "gemini-1.5-pro",
    )
    .gemini_stream_generate_url()
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse"
    );
}

#[test]
fn gemini_generate_url_uses_generate_content_suffix() {
    let url = cfg(
        "https://generativelanguage.googleapis.com",
        "gemini-1.5-pro",
    )
    .gemini_generate_url()
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:generateContent"
    );
}

#[test]
fn gemini_models_url_from_default_host() {
    let url = cfg(
        "https://generativelanguage.googleapis.com",
        "gemini-1.5-pro",
    )
    .gemini_models_url()
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models"
    );
}

#[test]
fn gemini_models_url_with_v1beta_path_is_idempotent() {
    let url = cfg(
        "https://generativelanguage.googleapis.com/v1beta",
        "gemini-1.5-pro",
    )
    .gemini_models_url()
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models"
    );
}

#[test]
fn gemini_stream_generate_url_works_with_relay_base_path() {
    let url = cfg("https://relay.example.com/google", "gemini-1.5-pro")
        .gemini_stream_generate_url()
        .unwrap();
    assert_eq!(
        url.as_str(),
        "https://relay.example.com/google/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse"
    );
}

#[test]
fn gemini_stream_generate_url_preserves_query_string() {
    let url = cfg(
        "https://generativelanguage.googleapis.com",
        "gemini-1.5-pro",
    )
    .gemini_stream_generate_url()
    .unwrap();
    assert_eq!(url.query(), Some("alt=sse"));
}
