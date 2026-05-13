//! Phase 4a parser tests for `DeepSeekAdapter::parse_list_models_response`.
//! Fixtures match the documented DeepSeek `/models` response shape (OpenAI-compatible).

use super::{DeepSeekAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> DeepSeekAdapter {
    DeepSeekAdapter
}

#[test]
fn parses_happy_path_two_models() {
    let body = r#"{
        "object": "list",
        "data": [
            {"id": "deepseek-chat",     "object": "model", "owned_by": "deepseek"},
            {"id": "deepseek-reasoner", "object": "model", "owned_by": "deepseek"}
        ]
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 2);
    assert_eq!(
        page.models[0],
        DiscoveredModel {
            id: "deepseek-chat".into(),
            display_name: None,
            context_window: None,
            max_output_tokens: None,
        }
    );
    assert_eq!(
        page.models[1],
        DiscoveredModel {
            id: "deepseek-reasoner".into(),
            display_name: None,
            context_window: None,
            max_output_tokens: None,
        }
    );
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
    let body = r#"{"object": "list", "data": ["#; // truncated
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(
        matches!(err, super::AdapterError::EncodeRequest(_)),
        "got {err:?}"
    );
}

#[test]
fn errors_on_row_missing_id() {
    let body = r#"{"data": [{"object": "model", "owned_by": "deepseek"}]}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(
        matches!(err, super::AdapterError::EncodeRequest(_)),
        "got {err:?}"
    );
}

#[test]
fn ignores_unknown_top_level_fields() {
    let body = r#"{"object": "list", "data": [{"id": "deepseek-chat"}],
                   "future_field": {"nested": "value"}, "another_field": 42}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.models[0].id, "deepseek-chat");
}
