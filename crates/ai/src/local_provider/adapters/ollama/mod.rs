//! Ollama native API adapter. Phase 3b.
//!
//! Submodule layout:
//! - `wire`: serde types for the native `/api/chat` request and NDJSON
//!   streaming response.
//! - `request`: translator from `LocalProviderInput` to an
//!   `OllamaChatRequest`.
//! - `response`: NDJSON stream decoder (`OllamaDecoder`).
//!
//! Wire-format differences from OpenAi handled here:
//! - **NDJSON streaming** instead of SSE — `streaming_format()` returns
//!   `NewlineDelimitedJson` so the runner uses the bytes-stream + line
//!   splitter path in `synthesize_ndjson_stream`.
//! - Optional Bearer auth — most local Ollama instances are unauthed.
//! - Native tool_call shape: `tool_calls[].function.arguments` is a JSON
//!   object, not a stringified-JSON string; no `id` / `type:function`
//!   fields. The decoder synthesizes UUID ids since Ollama doesn't send
//!   any.
//! - `options.num_ctx` threaded from `cfg.context_window` to size the KV
//!   cache appropriately for large-context models.

pub mod request;
pub mod response;
pub mod wire;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
#[cfg(test)]
#[path = "response_tests.rs"]
mod response_tests;

use super::{
    AdapterError, AgentProviderApiType, LocalProviderConfig, LocalProviderInput, ProviderAdapter,
    StreamDecoder, StreamIds, StreamingFormat, SummarizerError, SummarizerInput,
};

use request::compose_ollama_chat_request;
use response::OllamaDecoder;
use wire::{
    OllamaChatChunk, OllamaChatMessage, OllamaChatRequest, OllamaOptions, OllamaRole,
};

pub struct OllamaAdapter;

impl ProviderAdapter for OllamaAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::Ollama
    }

    fn streaming_format(&self) -> StreamingFormat {
        StreamingFormat::NewlineDelimitedJson
    }

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.ollama_chat_url()?;
        let body = compose_ollama_chat_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_ollama_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/x-ndjson")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        let mut decoder = match ids {
            Some(ids) => OllamaDecoder::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => OllamaDecoder::new(),
        };
        if skip_create_task {
            decoder.skip_create_task();
        }
        Box::new(decoder)
    }

    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.ollama_chat_url()?;
        let body = build_ollama_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_ollama_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        // Ollama's non-streaming /api/chat returns ONE OllamaChatChunk
        // with done:true and the full content in message.content.
        let parsed: OllamaChatChunk = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!(
                "{e}: {}",
                crate::local_provider::run::first_chars(body, 200)
            ))
        })?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(err));
        }
        let text = parsed
            .message
            .map(|m| m.content)
            .unwrap_or_default();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Err(SummarizerError::NoContent)
        } else {
            Ok(trimmed.to_string())
        }
    }

    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.ollama_tags_url()?;
        Ok(apply_ollama_headers(http.get(url), cfg.api_key.as_deref()))
    }
}

fn apply_ollama_headers(
    rb: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    match api_key.filter(|k| !k.is_empty()) {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    }
}

/// Translate the OpenAI-shaped `SummarizerInput.messages` (which the
/// compaction pipeline produces uniformly across adapters) into a
/// non-streaming `OllamaChatRequest`. System messages stay as
/// `role:"system"` entries; tool messages are dropped (compaction never
/// emits them). `options.num_ctx` is threaded from `cfg.context_window`.
fn build_ollama_summarizer_body(
    input: &SummarizerInput,
    cfg: &LocalProviderConfig,
) -> OllamaChatRequest {
    use crate::local_provider::wire::Role;
    let mut messages: Vec<OllamaChatMessage> = Vec::with_capacity(input.messages.len());
    for msg in &input.messages {
        let content = msg.content.clone().unwrap_or_default();
        let role = match msg.role {
            Role::System => OllamaRole::System,
            Role::User => OllamaRole::User,
            Role::Assistant => OllamaRole::Assistant,
            // Compaction never emits role:Tool; drop defensively.
            Role::Tool => continue,
        };
        messages.push(OllamaChatMessage {
            role,
            content,
            tool_calls: None,
        });
    }
    let options = cfg
        .context_window
        .filter(|n| *n > 0)
        .map(|num_ctx| OllamaOptions {
            num_ctx: Some(num_ctx),
        });
    OllamaChatRequest {
        model: cfg.model_id.clone(),
        stream: false,
        messages,
        tools: None,
        options,
    }
}
