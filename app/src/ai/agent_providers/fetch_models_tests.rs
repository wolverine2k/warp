//! Phase 4a tests for `fetch_models()`. Each test spins up a mockito mock
//! server and confirms the helper handles success, failure, and pagination
//! correctly.

use std::sync::Once;

use mockito::Server;

use ai::local_provider::{api_type::AgentProviderApiType, config::LocalProviderConfig};

use super::{fetch_models, FetchModelsOutcome};

/// Install the rustls aws-lc-rs crypto provider exactly once per test
/// process. `reqwest::Client::new()` panics with "No provider set" without
/// this. Mirrors the pattern in `crates/ai/src/local_provider/adapters/mod.rs`.
fn ensure_rustls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn http_client() -> reqwest::Client {
    ensure_rustls_provider();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Build a minimal valid OpenAI config pointing at the given mock server URL.
fn cfg_openai(base_url: String, key: &str) -> LocalProviderConfig {
    LocalProviderConfig {
        api_type: AgentProviderApiType::OpenAi,
        display_name: "Test".into(),
        base_url,
        model_id: "test-model".into(),
        api_key: Some(key.into()),
        ..Default::default()
    }
}

/// Build a minimal valid Anthropic config.
fn cfg_anthropic(base_url: String, key: &str) -> LocalProviderConfig {
    LocalProviderConfig {
        api_type: AgentProviderApiType::Anthropic,
        display_name: "Test".into(),
        base_url,
        model_id: "claude-test".into(),
        api_key: Some(key.into()),
        ..Default::default()
    }
}

/// Build a minimal valid Ollama config (no API key required).
fn cfg_ollama(base_url: String) -> LocalProviderConfig {
    LocalProviderConfig {
        api_type: AgentProviderApiType::Ollama,
        display_name: "Test".into(),
        base_url,
        model_id: "llama3.1".into(),
        api_key: None,
        ..Default::default()
    }
}

// ── basic success / failure ──────────────────────────────────────────────────

#[tokio::test]
async fn single_page_returns_models() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"}]}"#)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_openai(server.url(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else {
        panic!("expected Ok, got {outcome:?}")
    };
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-4o");
}

#[tokio::test]
async fn http_401_returns_failed_with_status() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/models")
        .with_status(401)
        .with_body(r#"{"error":"unauthorized"}"#)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_openai(server.url(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else {
        panic!("expected Failed")
    };
    assert!(msg.contains("HTTP 401"), "got: {msg}");
}

#[tokio::test]
async fn http_404_returns_failed() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/models")
        .with_status(404)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_openai(server.url(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else {
        panic!("expected Failed")
    };
    assert!(msg.contains("HTTP 404"), "got: {msg}");
}

#[tokio::test]
async fn http_500_returns_failed_with_body() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/models")
        .with_status(503)
        .with_body("upstream busy")
        .create_async()
        .await;
    let outcome = fetch_models(cfg_openai(server.url(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else {
        panic!("expected Failed")
    };
    assert!(
        msg.contains("HTTP") && msg.contains("upstream busy"),
        "got: {msg}"
    );
}

#[tokio::test]
async fn malformed_body_returns_parse_error() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"data": ["#)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_openai(server.url(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else {
        panic!("expected Failed")
    };
    assert!(msg.starts_with("Parse error:"), "got: {msg}");
}

#[tokio::test]
async fn empty_models_array_returns_ok_with_empty_vec() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"data":[]}"#)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_openai(server.url(), "sk-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else {
        panic!("expected Ok")
    };
    assert!(models.is_empty());
}

// ── API-key pre-flight ───────────────────────────────────────────────────────

#[tokio::test]
async fn missing_api_key_short_circuits_for_openai() {
    // Mock with expect(0) proves no HTTP request fires when the key is absent.
    let mut server = Server::new_async().await;
    let no_call_mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body("UNREACHABLE")
        .expect(0)
        .create_async()
        .await;
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::OpenAi,
        display_name: "Test".into(),
        base_url: server.url(),
        model_id: "test-model".into(),
        api_key: None,
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else {
        panic!("expected Failed")
    };
    assert_eq!(msg, "API key required");
    no_call_mock.assert();
}

#[tokio::test]
async fn missing_api_key_allowed_for_ollama() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body(r#"{"models":[]}"#)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_ollama(server.url()), http_client()).await;
    assert!(outcome.is_ok(), "got {outcome:?}");
}

// ── pagination ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn pagination_loop_aggregates_three_pages() {
    // Anthropic-style cursor pagination: limit=100 on every page, after_id on pages 2+.
    let mut server = Server::new_async().await;

    // Page 1: limit=100, no after_id
    let _p1 = server
        .mock("GET", "/v1/models?limit=100")
        .with_status(200)
        .with_body(
            r#"{"data":[{"type":"model","id":"m1","display_name":"M1"},
                        {"type":"model","id":"m2","display_name":"M2"}],
               "last_id":"m2","has_more":true}"#,
        )
        .expect(1)
        .create_async()
        .await;
    // Page 2: after_id=m2
    let _p2 = server
        .mock("GET", "/v1/models?limit=100&after_id=m2")
        .with_status(200)
        .with_body(
            r#"{"data":[{"type":"model","id":"m3","display_name":"M3"},
                        {"type":"model","id":"m4","display_name":"M4"}],
               "last_id":"m4","has_more":true}"#,
        )
        .expect(1)
        .create_async()
        .await;
    // Page 3: after_id=m4, no more
    let _p3 = server
        .mock("GET", "/v1/models?limit=100&after_id=m4")
        .with_status(200)
        .with_body(
            r#"{"data":[{"type":"model","id":"m5","display_name":"M5"}],
               "last_id":"m5","has_more":false}"#,
        )
        .expect(1)
        .create_async()
        .await;

    let outcome = fetch_models(cfg_anthropic(server.url(), "sk-ant-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else {
        panic!("expected Ok, got {outcome:?}")
    };
    assert_eq!(models.len(), 5);
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["m1", "m2", "m3", "m4", "m5"]
    );
}

#[tokio::test]
async fn truncates_at_max_entries_cap() {
    // Single page returns 250 unique models with has_more=true; helper must
    // truncate to MAX_ENTRIES=200 before returning.
    let mut server = Server::new_async().await;
    let body = serde_json::to_string(&serde_json::json!({
        "data": (0..250u32).map(|i| serde_json::json!({
            "type": "model",
            "id": format!("model-{i:04}"),
            "display_name": format!("Model {i}")
        })).collect::<Vec<_>>(),
        "last_id": "model-0249",
        "has_more": true,
    }))
    .unwrap();
    let _mock = server
        .mock("GET", "/v1/models?limit=100")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_anthropic(server.url(), "sk-ant-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else {
        panic!("expected Ok, got {outcome:?}")
    };
    assert_eq!(models.len(), 200, "should be capped at MAX_ENTRIES");
}

#[tokio::test]
async fn deduplicates_overlapping_pages() {
    // Two pages; second page repeats m2 (rare but defensive).
    let mut server = Server::new_async().await;
    let _p1 = server
        .mock("GET", "/v1/models?limit=100")
        .with_status(200)
        .with_body(
            r#"{"data":[{"type":"model","id":"m1","display_name":"M1"},
                        {"type":"model","id":"m2","display_name":"M2"}],
               "last_id":"m2","has_more":true}"#,
        )
        .expect(1)
        .create_async()
        .await;
    let _p2 = server
        .mock("GET", "/v1/models?limit=100&after_id=m2")
        .with_status(200)
        .with_body(
            r#"{"data":[{"type":"model","id":"m2","display_name":"M2-dup"},
                        {"type":"model","id":"m3","display_name":"M3"}],
               "last_id":"m3","has_more":false}"#,
        )
        .expect(1)
        .create_async()
        .await;
    let outcome = fetch_models(cfg_anthropic(server.url(), "sk-ant-fake"), http_client()).await;
    let FetchModelsOutcome::Ok(models) = outcome else {
        panic!("expected Ok")
    };
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["m1", "m2", "m3"]);
    // First occurrence wins for dedup: m2's display_name is "M2", not "M2-dup".
    assert_eq!(models[1].display_name.as_deref(), Some("M2"));
}

// ── unsupported api_type ─────────────────────────────────────────────────────

#[tokio::test]
async fn unsupported_api_type_returns_failed() {
    // OpenAiResp is the only variant that surfaces UnsupportedApiType at
    // select_adapter. Pass it through and confirm the helper reports.
    let cfg = LocalProviderConfig {
        api_type: AgentProviderApiType::OpenAiResp,
        display_name: "Test".into(),
        base_url: "http://localhost:1".into(), // never hit
        model_id: "ignored".into(),
        api_key: Some("ignored".into()),
        ..Default::default()
    };
    let outcome = fetch_models(cfg, http_client()).await;
    let FetchModelsOutcome::Failed(msg) = outcome else {
        panic!("expected Failed")
    };
    assert!(msg.contains("Fetch models not supported"), "got: {msg}");
}

// ── enrich_with_catalog ──────────────────────────────────────────────────────

use ai::catalog::CatalogModel;
use ai::local_provider::adapters::DiscoveredModel;

use super::enrich_with_catalog;

fn catalog_entry(provider: &str, id: &str) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.into(),
        id: id.into(),
        name: format!("Display {id}"),
        context_window: Some(200000),
        max_output_tokens: Some(8192),
        tool_call: true,
        reasoning: false,
        image: false,
        pdf: false,
        audio: false,
        open_weights: false,
    }
}

#[test]
fn enrich_fills_missing_display_name() {
    let d = DiscoveredModel {
        id: "claude-sonnet-4-6".into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
    };
    let catalog = vec![catalog_entry("anthropic", "claude-sonnet-4-6")];
    let enriched = enrich_with_catalog(
        vec![d.clone()],
        AgentProviderApiType::Anthropic,
        &catalog,
    );
    assert_eq!(
        enriched[0].display_name.as_deref(),
        Some("Display claude-sonnet-4-6")
    );
    assert_eq!(enriched[0].context_window, Some(200000));
    assert_eq!(enriched[0].max_output_tokens, Some(8192));
}

#[test]
fn enrich_does_not_overwrite_existing_values() {
    let d = DiscoveredModel {
        id: "claude-sonnet-4-6".into(),
        display_name: Some("User-set name".into()),
        context_window: Some(99),
        max_output_tokens: Some(11),
    };
    let catalog = vec![catalog_entry("anthropic", "claude-sonnet-4-6")];
    let enriched =
        enrich_with_catalog(vec![d], AgentProviderApiType::Anthropic, &catalog);
    assert_eq!(enriched[0].display_name.as_deref(), Some("User-set name"));
    assert_eq!(enriched[0].context_window, Some(99));
    assert_eq!(enriched[0].max_output_tokens, Some(11));
}

#[test]
fn enrich_with_empty_catalog_is_noop() {
    let d = DiscoveredModel {
        id: "claude-sonnet-4-6".into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
    };
    let enriched =
        enrich_with_catalog(vec![d.clone()], AgentProviderApiType::Anthropic, &[]);
    assert_eq!(enriched[0].display_name, None);
}
