//! Phase 2 Task 4: connectivity-probe builder tests for OpenAiAdapter.
//! Network-level success/failure is exercised by the manual smoke test —
//! these unit tests cover URL composition + auth header application.

use super::*;
use crate::local_provider::config::LocalProviderConfig;

fn http_client() -> reqwest::Client {
    super::ensure_rustls_provider();
    reqwest::Client::new()
}

fn cfg(base: &str, api_key: Option<&str>) -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Local".into(),
        base_url: base.into(),
        model_id: "llama3.1".into(),
        api_key: api_key.map(str::to_string),
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::OpenAi,
    }
}

#[test]
fn probe_url_targets_models_list() {
    let req = OpenAiAdapter
        .build_probe_request(&cfg("http://localhost:11434/v1", Some("k")), &http_client())
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(req.method().as_str(), "GET");
    assert_eq!(req.url().as_str(), "http://localhost:11434/v1/models");
}

#[test]
fn probe_url_handles_trailing_slash_base_url() {
    let req = OpenAiAdapter
        .build_probe_request(
            &cfg("http://localhost:11434/v1/", Some("k")),
            &http_client(),
        )
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(req.url().as_str(), "http://localhost:11434/v1/models");
}

#[test]
fn probe_request_includes_bearer_when_key_set() {
    let req = OpenAiAdapter
        .build_probe_request(&cfg("http://localhost:11434/v1", Some("k")), &http_client())
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        req.headers()
            .get("authorization")
            .map(|v| v.to_str().unwrap()),
        Some("Bearer k"),
    );
}

#[test]
fn probe_request_omits_bearer_when_key_absent() {
    let req = OpenAiAdapter
        .build_probe_request(&cfg("http://localhost:11434/v1", None), &http_client())
        .unwrap()
        .build()
        .unwrap();
    assert!(req.headers().get("authorization").is_none());
}

#[test]
fn probe_request_omits_bearer_when_key_empty_string() {
    let req = OpenAiAdapter
        .build_probe_request(&cfg("http://localhost:11434/v1", Some("")), &http_client())
        .unwrap()
        .build()
        .unwrap();
    assert!(req.headers().get("authorization").is_none());
}
