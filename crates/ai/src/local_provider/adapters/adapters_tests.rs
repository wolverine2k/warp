//! Phase 2 Task 4: trait-level tests for select_adapter and the OpenAi
//! adapter glue. Wire-level behavior continues to be covered by the
//! existing tests in `request.rs`, `response.rs`, and the integration
//! tests in `crates/ai/tests/local_provider_integration.rs`.

use super::*;
use crate::local_provider::{config::LocalProviderConfig, request::LocalProviderInput};

fn http_client() -> reqwest::Client {
    super::ensure_rustls_provider();
    reqwest::Client::new()
}

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Local".into(),
        base_url: "http://localhost:11434/v1".into(),
        model_id: "llama3.1".into(),
        api_key: Some("k".into()),
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::OpenAi,
    }
}

#[test]
fn select_adapter_returns_openai_for_openai_api_type() {
    let a = select_adapter(AgentProviderApiType::OpenAi).expect("ok");
    assert_eq!(a.api_type(), AgentProviderApiType::OpenAi);
}

#[test]
fn select_adapter_returns_anthropic_for_anthropic_api_type() {
    let a = select_adapter(AgentProviderApiType::Anthropic).expect("ok");
    assert_eq!(a.api_type(), AgentProviderApiType::Anthropic);
}

#[test]
fn select_adapter_errors_for_each_unimplemented_variant() {
    for ty in [
        AgentProviderApiType::OpenAiResp,
        AgentProviderApiType::Gemini,
        AgentProviderApiType::Ollama,
        AgentProviderApiType::DeepSeek,
    ] {
        // `Box<dyn ProviderAdapter>` doesn't implement Debug — destructure via
        // `match` instead of `expect_err`.
        match select_adapter(ty) {
            Ok(_) => panic!("expected UnsupportedApiType for {ty:?}"),
            Err(AdapterError::UnsupportedApiType(got)) => assert_eq!(got, ty),
            Err(other) => panic!("wrong variant for {ty:?}: {other:?}"),
        }
    }
}

#[test]
fn openai_adapter_builds_chat_request_with_bearer_auth() {
    let http = http_client();
    let req = OpenAiAdapter
        .build_chat_request(&LocalProviderInput::default(), &cfg(), &http)
        .expect("ok")
        .build()
        .expect("buildable");
    assert_eq!(req.method().as_str(), "POST");
    assert_eq!(
        req.url().as_str(),
        "http://localhost:11434/v1/chat/completions"
    );
    assert_eq!(
        req.headers()
            .get("authorization")
            .map(|v| v.to_str().unwrap()),
        Some("Bearer k"),
    );
    assert_eq!(
        req.headers().get("accept").map(|v| v.to_str().unwrap()),
        Some("text/event-stream"),
    );
}

#[test]
fn openai_adapter_omits_bearer_when_key_absent() {
    let http = http_client();
    let mut c = cfg();
    c.api_key = None;
    let req = OpenAiAdapter
        .build_chat_request(&LocalProviderInput::default(), &c, &http)
        .unwrap()
        .build()
        .unwrap();
    assert!(req.headers().get("authorization").is_none());
}

#[test]
fn openai_adapter_decoder_returns_box_dyn_stream_decoder() {
    let dec = OpenAiAdapter.create_stream_decoder(None, false);
    assert!(!dec.is_terminal());
}

#[test]
fn openai_adapter_decoder_with_explicit_ids_round_trips_terminal_state() {
    let ids = StreamIds {
        conversation_id: "c".into(),
        request_id: "r".into(),
        run_id: "u".into(),
        task_id: "t".into(),
    };
    let mut dec = OpenAiAdapter.create_stream_decoder(Some(ids), true);
    assert!(!dec.is_terminal());
    // Feeding `[DONE]` should drive the decoder into a terminal state.
    let _ = dec.feed("[DONE]");
    assert!(dec.is_terminal());
}

#[test]
fn openai_adapter_parse_summarizer_response_extracts_content() {
    let body = r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#;
    let s = OpenAiAdapter.parse_summarizer_response(body).expect("ok");
    assert_eq!(s, "hi");
}

#[test]
fn openai_adapter_parse_summarizer_response_no_content_errors() {
    let body = r#"{"choices":[]}"#;
    let err = OpenAiAdapter
        .parse_summarizer_response(body)
        .expect_err("no content");
    assert!(matches!(err, SummarizerError::NoContent));
}

#[test]
fn openai_adapter_build_summarizer_request_uses_chat_completions_url_with_application_json_accept()
{
    let http = http_client();
    let input = SummarizerInput { messages: vec![] };
    let req = OpenAiAdapter
        .build_summarizer_request(&input, &cfg(), &http)
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(req.method().as_str(), "POST");
    assert_eq!(
        req.url().as_str(),
        "http://localhost:11434/v1/chat/completions"
    );
    assert_eq!(
        req.headers().get("accept").map(|v| v.to_str().unwrap()),
        Some("application/json"),
    );
}
