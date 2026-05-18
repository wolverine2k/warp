//! Anthropic Messages API adapter. Phase 3a.
//!
//! Submodule layout:
//! - `wire`: serde types for the Messages API (request, streaming events,
//!   non-streaming response).
//! - `request`: translator from `LocalProviderInput` to an
//!   `AnthropicMessagesRequest`.
//! - `response`: SSE stream decoder (`AnthropicSseDecoder`).
//!
//! Wire-format differences from OpenAi handled here:
//! - `x-api-key` + `anthropic-version` headers (not `Authorization: Bearer`).
//! - Top-level `system` field; alternating user/assistant roles with
//!   content blocks (translator handles this).
//! - Streaming events are named (`event: message_start`, etc.); the
//!   `feed_event` trait method threads the SSE event-name through.

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

use request::{compose_anthropic_messages_request, resolve_max_tokens};
use response::AnthropicSseDecoder;
use wire::{
    AnthropicContentBlock, AnthropicMessage, AnthropicMessageResponse, AnthropicMessagesRequest,
    AnthropicModelsListResponse, AnthropicRole, ResponseContentBlock,
};

/// Anthropic-version pin sent on every request. The 2023-06-01 value is the
/// stable Messages API version across all Claude 3.x and 4.x models. We
/// don't currently surface the 1M-context or 128K-output beta opt-ins —
/// Phase 4 polish can expose them per-model.
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicAdapter;

impl ProviderAdapter for AnthropicAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::Anthropic
    }

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.messages_url()?;
        let body = compose_anthropic_messages_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_anthropic_headers(
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
            Some(ids) => AnthropicSseDecoder::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => AnthropicSseDecoder::new(),
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
        let url = cfg.messages_url()?;
        let body = build_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_anthropic_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: AnthropicMessageResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!(
                "{e}: {}",
                crate::local_provider::run::first_chars(body, 200)
            ))
        })?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(format!(
                "{}: {}",
                err.kind, err.message
            )));
        }
        let combined = parsed
            .content
            .into_iter()
            .filter_map(|b| match b {
                // Prefer `text` blocks; fall back to `thinking` only if text
                // is absent (well-behaved summarizers put the visible
                // summary in `text`). `tool_use` blocks aren't expected in
                // summarizer responses — we send `tools: None`.
                ResponseContentBlock::Text { text } if !text.trim().is_empty() => Some(text),
                ResponseContentBlock::Thinking { thinking } if !thinking.trim().is_empty() => {
                    Some(thinking)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = combined.trim();
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
        let url = cfg.anthropic_models_url()?;
        Ok(apply_anthropic_headers(http.get(url), cfg.api_key.as_deref()))
    }

    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        // Anthropic's list-models endpoint is GET /v1/models — same path the
        // probe uses. Always append `limit=100` to reduce round-trips (the
        // Anthropic default is 20). When paginating, also append `after_id`.
        let mut url = cfg.anthropic_models_url()?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("limit", "100");
            if let Some(c) = cursor {
                q.append_pair("after_id", c);
            }
        }
        Ok(apply_anthropic_headers(http.get(url), cfg.api_key.as_deref()))
    }

    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        let parsed: AnthropicModelsListResponse = serde_json::from_str(body)?;
        // `next_cursor` is `Some(last_id)` IFF `has_more: true`. Anthropic
        // emits `last_id` on the final page too — we must ignore it there.
        let next_cursor = if parsed.has_more { parsed.last_id } else { None };
        let models = parsed
            .data
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                display_name: m.display_name,
                context_window: None,
                max_output_tokens: None,
            })
            .collect();
        Ok(ListModelsPage { models, next_cursor })
    }
}

fn apply_anthropic_headers(
    rb: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    let rb = rb.header("anthropic-version", ANTHROPIC_VERSION);
    match api_key.filter(|k| !k.is_empty()) {
        Some(k) => rb.header("x-api-key", k),
        None => rb,
    }
}

/// Translate the OpenAI-shaped `SummarizerInput.messages` list (which the
/// compaction pipeline produces uniformly across adapters) into the
/// Anthropic Messages request shape. System messages lift to the top-level
/// `system` field; user/assistant become content-block lists with one text
/// block each; adjacent same-role entries merge. Tool roles aren't
/// expected in summarizer bodies (compaction sends `tools: None`); if any
/// appear we drop them silently rather than synthesize tool_result blocks.
fn build_summarizer_body(
    input: &SummarizerInput,
    cfg: &LocalProviderConfig,
) -> AnthropicMessagesRequest {
    use crate::local_provider::wire::Role;
    let mut system_parts: Vec<String> = Vec::new();
    let mut entries: Vec<AnthropicMessage> = Vec::new();
    for msg in &input.messages {
        let text = msg
            .content
            .as_ref()
            .and_then(|c| c.as_text())
            .unwrap_or_default()
            .to_string();
        match msg.role {
            Role::System => {
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            Role::User | Role::Assistant => {
                let role = match msg.role {
                    Role::User => AnthropicRole::User,
                    Role::Assistant => AnthropicRole::Assistant,
                    _ => unreachable!(),
                };
                let block = AnthropicContentBlock::Text { text };
                match entries.last_mut() {
                    Some(last) if last.role == role => last.content.push(block),
                    _ => entries.push(AnthropicMessage {
                        role,
                        content: vec![block],
                    }),
                }
            }
            Role::Tool => {
                // Compaction never emits role:Tool, but be defensive — drop
                // rather than misencode.
            }
        }
    }
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    AnthropicMessagesRequest {
        model: cfg.model_id.clone(),
        max_tokens: resolve_max_tokens(cfg),
        system,
        messages: entries,
        tools: None,
        tool_choice: None,
        stream: false,
    }
}

