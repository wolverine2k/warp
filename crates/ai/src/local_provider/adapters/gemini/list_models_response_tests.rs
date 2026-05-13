//! Phase 4a parser tests for `GeminiAdapter::parse_list_models_response`.
//! Fixtures match the documented `/v1beta/models` response shape.

use super::{AdapterError, GeminiAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> GeminiAdapter {
    GeminiAdapter
}

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
    assert_eq!(
        page.models[0],
        DiscoveredModel {
            id: "gemini-2.5-pro".into(),
            display_name: Some("Gemini 2.5 Pro".into()),
            context_window: Some(2_000_000),
            max_output_tokens: Some(8192),
        }
    );
}

#[test]
fn filters_out_models_without_generate_content_method() {
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
    assert!(matches!(err, AdapterError::EncodeRequest(_)));
}
