//! Phase 4a parser tests for `OllamaAdapter::parse_list_models_response`.
//! Fixtures match the documented `/api/tags` response shape.

use super::{OllamaAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> OllamaAdapter {
    OllamaAdapter
}

#[test]
fn parses_happy_path_with_details() {
    let body = r#"{
        "models": [
            {"name": "llama3.1:latest",
             "modified_at": "2025-04-12T10:30:00Z",
             "size": 4661230977,
             "digest": "sha256:abc",
             "details": {"format": "gguf", "family": "llama",
                         "families": ["llama"],
                         "parameter_size": "8B", "quantization_level": "Q4_0"}}
        ]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 1);
    assert_eq!(
        page.models[0],
        DiscoveredModel {
            id: "llama3.1:latest".into(),
            display_name: Some("Llama (8B)".into()),
            context_window: None,
            max_output_tokens: None,
        }
    );
}

#[test]
fn parses_row_without_details_block() {
    let body = r#"{"models": [{"name": "custom:v1"}]}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "custom:v1");
    assert_eq!(page.models[0].display_name, None);
}

#[test]
fn parses_details_with_only_family() {
    let body = r#"{
        "models": [{"name": "x:v1",
                    "details": {"family": "mistral"}}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].display_name, Some("Mistral".into()));
}

#[test]
fn parses_details_with_only_parameter_size() {
    let body = r#"{
        "models": [{"name": "x:v1",
                    "details": {"parameter_size": "70B"}}]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].display_name, Some("70B".into()));
}

#[test]
fn parses_empty_models_array() {
    let body = r#"{"models": []}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
}

#[test]
fn details_with_empty_strings_yields_no_display_name() {
    let body = r#"{"models": [{"name": "x:v1",
                                "details": {"family": "", "parameter_size": ""}}]}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].display_name, None);
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"models": ["#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}

#[test]
fn errors_on_row_missing_name() {
    let body = r#"{"models": [{"size": 100}]}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)));
}
