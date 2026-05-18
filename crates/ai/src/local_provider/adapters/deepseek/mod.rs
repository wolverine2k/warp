//! DeepSeek native protocol adapter. Phase 3d.
//!
//! Submodule layout mirrors Phase 3a/3b/3c:
//! - `wire`: serde types for /chat/completions (OpenAI-shaped with
//!   reasoning_content extensions on inbound types).
//! - `request`: translator from `LocalProviderInput` to a
//!   `DeepSeekChatRequest`.
//! - `response`: SSE stream decoder (`DeepSeekSseDecoder`).
//!
//! DeepSeek's wire format is intentionally OpenAI-compatible — the only
//! semantic divergence is the `reasoning_content` channel on assistant
//! messages (deepseek-reasoner model only). Phase 3d handles it on the
//! response side (decoder emits AgentReasoning proto messages) but NOT
//! on the request side: the API returns HTTP 400 if reasoning_content
//! appears on inbound messages, so the translator drops AgentReasoning
//! from outbound history.

pub mod request;
pub mod response;
pub mod wire;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
#[cfg(test)]
#[path = "response_tests.rs"]
mod response_tests;
#[cfg(test)]
#[path = "list_models_response_tests.rs"]
mod list_models_tests;

use super::{
    AdapterError, AgentProviderApiType, DiscoveredModel, ListModelsPage, LocalProviderConfig,
    LocalProviderInput, ProviderAdapter, StreamDecoder, StreamIds, SummarizerError, SummarizerInput,
};

use request::compose_deepseek_chat_request;
use response::DeepSeekSseDecoder;
use wire::{DeepSeekChatRequest, DeepSeekChatResponse};

pub struct DeepSeekAdapter;

impl ProviderAdapter for DeepSeekAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::DeepSeek
    }

    // streaming_format() inherits the SSE default — no override needed.

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.chat_completions_url()?;
        let body = compose_deepseek_chat_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_deepseek_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "text/event-stream")
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
            Some(ids) => DeepSeekSseDecoder::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => DeepSeekSseDecoder::new(),
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
        let url = cfg.chat_completions_url()?;
        let body = build_deepseek_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_deepseek_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    /// Parse a non-streaming `/chat/completions` response into the assistant
    /// summary text. Reads `choices[0].message.content` only — the
    /// `reasoning_content` field is intentionally dropped on the summarizer
    /// path. The streaming SSE decoder surfaces reasoning to the UI via
    /// `AgentReasoning` proto messages; the summarizer doesn't need it
    /// because compaction summaries are final-answer text, not chain-of-thought.
    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: DeepSeekChatResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!(
                "{e}: {}",
                crate::local_provider::run::first_chars(body, 200)
            ))
        })?;
        if let Some(err) = parsed.error {
            let kind = if err.kind.is_empty() { "error".to_string() } else { err.kind };
            return Err(SummarizerError::UpstreamErrorEnvelope(format!(
                "{}: {}",
                kind, err.message
            )));
        }
        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .and_then(|m| m.content)
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
        let url = cfg.models_list_url()?;
        Ok(apply_deepseek_headers(http.get(url), cfg.api_key.as_deref()))
    }

    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        _cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.models_list_url()?;
        Ok(apply_deepseek_headers(http.get(url), cfg.api_key.as_deref()))
    }

    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        let parsed: wire::DeepSeekModelsListResponse = serde_json::from_str(body)?;
        let models = parsed
            .data
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                display_name: None,
                context_window: None,
                max_output_tokens: None,
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor: None })
    }
}

fn apply_deepseek_headers(
    rb: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    match api_key.filter(|k| !k.is_empty()) {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    }
}

/// Translate the OpenAI-shaped `SummarizerInput.messages` list into a
/// non-streaming DeepSeek /chat/completions body. Same shape as OpenAI's
/// summarizer body — system / user / assistant messages, no tools,
/// stream: false. role:Tool messages from compaction (never emitted in
/// practice) are silently dropped.
fn build_deepseek_summarizer_body(
    input: &SummarizerInput,
    cfg: &LocalProviderConfig,
) -> DeepSeekChatRequest {
    use crate::local_provider::wire::Role;
    use wire::{DeepSeekChatMessage, DeepSeekRole};
    let messages: Vec<DeepSeekChatMessage> = input
        .messages
        .iter()
        .filter_map(|msg| {
            let role = match msg.role {
                Role::System => DeepSeekRole::System,
                Role::User => DeepSeekRole::User,
                Role::Assistant => DeepSeekRole::Assistant,
                Role::Tool => return None, // compaction never emits Tool
            };
            Some(DeepSeekChatMessage {
                role,
                content: msg
                    .content
                    .as_ref()
                    .and_then(|c| c.as_text())
                    .map(|s| s.to_string()),
                tool_calls: None,
                tool_call_id: None,
            })
        })
        .collect();
    DeepSeekChatRequest {
        model: cfg.model_id.clone(),
        stream: false,
        messages,
        tools: None,
    }
}
