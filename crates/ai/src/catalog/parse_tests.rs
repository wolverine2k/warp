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
