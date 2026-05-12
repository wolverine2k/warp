//! Phase 4a parser tests for `OpenAiAdapter::parse_list_models_response`.
//! Fixtures match the documented `/v1/models` response shape.

use super::{ListModelsPage, OpenAiAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> OpenAiAdapter { OpenAiAdapter }

#[test]
fn parses_happy_path_three_models() {
    let body = r#"{
        "object": "list",
        "data": [
            {"id": "gpt-4o",       "object": "model", "created": 1, "owned_by": "system"},
            {"id": "gpt-4o-mini",  "object": "model", "created": 2, "owned_by": "system"},
            {"id": "text-embedding-3-small", "object": "model", "created": 3, "owned_by": "system"}
        ]
    }"#;
    let ListModelsPage { models, next_cursor } = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(next_cursor, None);
    assert_eq!(models.len(), 3);
    assert_eq!(models[0], DiscoveredModel { id: "gpt-4o".into(),       display_name: None, context_window: None, max_output_tokens: None });
    assert_eq!(models[1], DiscoveredModel { id: "gpt-4o-mini".into(),  display_name: None, context_window: None, max_output_tokens: None });
    assert_eq!(models[2].id, "text-embedding-3-small");
}

#[test]
fn parses_empty_data_array() {
    let body = r#"{"object": "list", "data": []}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"object": "list", "data": ["#;   // truncated
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)), "got {err:?}");
}

#[test]
fn errors_on_row_missing_id() {
    let body = r#"{"data": [{"object": "model", "created": 1, "owned_by": "system"}]}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(matches!(err, super::AdapterError::EncodeRequest(_)), "got {err:?}");
}

#[test]
fn ignores_unknown_top_level_fields() {
    // Defensive: future-proofing against OpenAI adding fields we don't model.
    let body = r#"{"object": "list", "data": [{"id": "gpt-4o"}],
                   "future_field": {"nested": "value"}, "another_field": 42}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.models[0].id, "gpt-4o");
}

#[test]
fn next_cursor_always_none_for_openai() {
    // OpenAi is unpaginated. Even if a hypothetical OpenAI-compat upstream
    // returned a `next_cursor`, the parser correctly returns None because
    // it never reads that field.
    let body = r#"{"data": [{"id": "gpt-4o"}], "next_cursor": "ignored"}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.next_cursor.is_none());
}
