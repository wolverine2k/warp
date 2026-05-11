//! Provider adapter trait — abstracts request composition and stream decoding
//! over wire-protocol variants. Phase 2 implements only `OpenAi`; Phase 3
//! will add Anthropic / Gemini / Ollama-native / DeepSeek as additional
//! impls without touching `run.rs`.

use thiserror::Error;
use warp_multi_agent_api as api;

use crate::local_provider::{
    api_type::AgentProviderApiType,
    config::{LocalProviderConfig, LocalProviderConfigError},
    request::LocalProviderInput,
    run::{SummarizerError, SummarizerInput},
};

pub mod anthropic;
pub mod openai;
pub use openai::OpenAiAdapter;

#[cfg(test)]
#[path = "adapters_tests.rs"]
mod adapters_tests;

#[cfg(test)]
#[path = "probe_tests.rs"]
mod probe_tests;

/// Install the rustls aws-lc-rs crypto provider exactly once per test
/// process. `reqwest::Client::new()` panics with "No provider set" without
/// this; the workspace doesn't pin a default. Mirrors the pattern in
/// `crates/ai/tests/local_provider_integration.rs` and `compaction/auto.rs`.
#[cfg(test)]
pub(super) fn ensure_rustls_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Trait-level errors. Distinct from `response::AdapterError` (parser-level).
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("invalid local provider config: {0}")]
    InvalidConfig(#[from] LocalProviderConfigError),
    #[error("failed to encode request body: {0}")]
    EncodeRequest(#[from] serde_json::Error),
    #[error("provider api_type {0:?} is not implemented yet")]
    UnsupportedApiType(AgentProviderApiType),
}

/// Per-turn identifiers passed by the controller into the stream decoder so
/// emitted events match the conversation/task the controller is driving.
/// `None` at the call site means "synthesize fresh ids" — used only by tests
/// that drive the adapter in isolation.
#[derive(Debug, Clone)]
pub struct StreamIds {
    pub conversation_id: String,
    pub request_id: String,
    pub run_id: String,
    pub task_id: String,
}

/// Stateful per-turn SSE/event decoder. Each call to `feed` may emit zero or
/// more `ResponseEvent`s downstream; `finish` drains pending state and emits
/// any closing events; `is_terminal` reports whether the upstream stream is
/// logically closed (so the runner knows to stop pulling).
pub trait StreamDecoder: Send {
    fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent>;
    fn finish(&mut self) -> Vec<api::ResponseEvent>;
    fn is_terminal(&self) -> bool;
    fn record_upstream_error(&mut self, msg: String);
}

/// Wire-protocol adapter. Stateless; one instance per `AgentProviderApiType`.
/// Phase 2 ships only `OpenAiAdapter`; Phase 3 adds Anthropic / Gemini /
/// Ollama-native / DeepSeek implementations.
pub trait ProviderAdapter: Send + Sync {
    fn api_type(&self) -> AgentProviderApiType;

    /// Build the per-turn streaming chat request. The returned
    /// `RequestBuilder` carries body + headers + auth; the runner POSTs it.
    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    /// Construct the stream decoder used for the lifetime of this turn.
    /// `ids` carries controller-supplied conversation/task identifiers (or
    /// `None` for test paths that don't have them).
    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder>;

    /// Build the non-streaming summarizer request used by the compaction
    /// pipeline. Returned `RequestBuilder` carries body + headers + auth.
    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;

    /// Decode the upstream summarizer body (already a successful HTTP 200)
    /// into the assistant's summary text.
    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError>;

    /// Build a lightweight GET probe for the "Test connection" button. The
    /// adapter chooses the most compatible endpoint (e.g. `GET /v1/models`
    /// for `OpenAi`). Caller fires it; success is HTTP 2xx, body content
    /// is not parsed.
    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError>;
}

/// Pick an adapter for the given wire-protocol variant. Phase 2 returns
/// `OpenAiAdapter` for `OpenAi`; the five non-OpenAI variants surface a
/// structured `UnsupportedApiType` error that's flipped to a real impl in
/// the corresponding Phase 3 sub-phase. Match is intentionally exhaustive
/// (no `_ =>` arm) so adding/removing a variant triggers a compile error
/// at this dispatch site per repo convention.
pub fn select_adapter(
    api_type: AgentProviderApiType,
) -> Result<Box<dyn ProviderAdapter>, AdapterError> {
    use AgentProviderApiType::*;
    match api_type {
        OpenAi => Ok(Box::new(OpenAiAdapter)),
        OpenAiResp | Gemini | Anthropic | Ollama | DeepSeek => {
            Err(AdapterError::UnsupportedApiType(api_type))
        }
    }
}
