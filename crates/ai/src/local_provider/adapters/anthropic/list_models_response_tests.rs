//! Phase 4a parser tests for `AnthropicAdapter::parse_list_models_response`.
//! Fixtures match the documented Anthropic `/v1/models` response shape.

use super::{AnthropicAdapter, ProviderAdapter};
use crate::local_provider::adapters::DiscoveredModel;

fn adapter() -> AnthropicAdapter {
    AnthropicAdapter
}

#[test]
fn parses_happy_path_with_display_name() {
    let body = r#"{
        "data": [
            {"type": "model", "id": "claude-opus-4-5-20251101",   "display_name": "Claude Opus 4.5",   "created_at": "2025-11-01T00:00:00Z"},
            {"type": "model", "id": "claude-sonnet-4-6-20251020", "display_name": "Claude Sonnet 4.6", "created_at": "2025-10-20T00:00:00Z"}
        ],
        "first_id": "claude-opus-4-5-20251101",
        "last_id":  "claude-sonnet-4-6-20251020",
        "has_more": false
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.models.len(), 2);
    assert_eq!(
        page.models[0],
        DiscoveredModel {
            id: "claude-opus-4-5-20251101".into(),
            display_name: Some("Claude Opus 4.5".into()),
            context_window: None,
            max_output_tokens: None,
        }
    );
    assert_eq!(
        page.models[1].display_name,
        Some("Claude Sonnet 4.6".into())
    );
}

#[test]
fn parses_empty_data_array() {
    let body =
        r#"{"data": [], "has_more": false, "first_id": null, "last_id": null}"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.models.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn errors_on_malformed_json() {
    let body = r#"{"data": ["#; // truncated
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(
        matches!(err, super::AdapterError::EncodeRequest(_)),
        "got {err:?}"
    );
}

#[test]
fn errors_on_row_missing_id() {
    let body =
        r#"{"data": [{"type": "model", "display_name": "X"}], "has_more": false}"#;
    let err = adapter().parse_list_models_response(body).unwrap_err();
    assert!(
        matches!(err, super::AdapterError::EncodeRequest(_)),
        "got {err:?}"
    );
}

#[test]
fn surfaces_next_cursor_when_has_more_true() {
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1", "display_name": "Claude 1"}],
        "first_id": "claude-1", "last_id": "claude-1",
        "has_more": true
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("claude-1"));
}

#[test]
fn next_cursor_none_when_has_more_false_even_with_last_id() {
    // Anthropic does send last_id on the final page too; we must ignore it.
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1", "display_name": "Claude 1"}],
        "first_id": "claude-1", "last_id": "claude-1",
        "has_more": false
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert!(page.next_cursor.is_none(), "had {:?}", page.next_cursor);
}

#[test]
fn missing_display_name_yields_none() {
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1"}],
        "has_more": false
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models[0].id, "claude-1");
    assert!(page.models[0].display_name.is_none());
}

#[test]
fn ignores_unknown_top_level_fields() {
    let body = r#"{
        "data": [{"type": "model", "id": "claude-1", "display_name": "Claude 1"}],
        "has_more": false,
        "future_field": {"nested": true}
    }"#;
    let page = adapter().parse_list_models_response(body).unwrap();
    assert_eq!(page.models.len(), 1);
}
