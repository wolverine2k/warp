//! Gemini native protocol adapter. Phase 3c.
//!
//! Submodule layout mirrors Phase 3a/3b:
//! - `wire`: serde types for :streamGenerateContent (+ :generateContent
//!   for the summarizer).
//! - `request`: translator from `LocalProviderInput` to a
//!   `GeminiGenerateRequest`.
//! - `response`: SSE stream decoder (`GeminiSseDecoder`).
//!
//! Wire-format differences from OpenAi handled here:
//! - `x-goog-api-key` header (not `Authorization: Bearer`).
//! - Model lives in the URL path (`/v1beta/models/{model}:streamGenerateContent`),
//!   not the body.
//! - Top-level `systemInstruction`; alternating user/model roles (not
//!   user/assistant) with content-parts; functionCall.args is a JSON
//!   object; functionResponse parts replace OpenAI's role:tool messages;
//!   tool definitions are wrapped in a single `functionDeclarations`
//!   array.
//! - SSE is anonymous-chunk (same as OpenAI); `finishReason` is the
//!   terminator inside the last chunk.

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

use request::compose_gemini_request;
use response::GeminiSseDecoder;
use wire::{GeminiGenerateRequest, GeminiGenerateResponse};

pub struct GeminiAdapter;

impl ProviderAdapter for GeminiAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::Gemini
    }

    // streaming_format() inherits the SSE default — no override needed.

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.gemini_stream_generate_url()?;
        let body = compose_gemini_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_gemini_headers(
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
            Some(ids) => GeminiSseDecoder::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => GeminiSseDecoder::new(),
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
        let url = cfg.gemini_generate_url()?;
        let body = build_gemini_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_gemini_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: GeminiGenerateResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!(
                "{e}: {}",
                crate::local_provider::run::first_chars(body, 200)
            ))
        })?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(format!(
                "{}: {}",
                err.status, err.message
            )));
        }
        let combined = parsed
            .candidates
            .into_iter()
            .next()
            .and_then(|c| c.content)
            .map(|content| {
                content
                    .parts
                    .into_iter()
                    .filter_map(|p| match p {
                        wire::GeminiInboundPart::Text { text } if !text.trim().is_empty() => {
                            Some(text)
                        }
                        wire::GeminiInboundPart::Text { .. }
                        | wire::GeminiInboundPart::FunctionCall { .. }
                        | wire::GeminiInboundPart::FunctionResponse { .. }
                        | wire::GeminiInboundPart::InlineData { .. }
                        | wire::GeminiInboundPart::Unknown(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
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
        let url = cfg.gemini_models_url()?;
        Ok(apply_gemini_headers(http.get(url), cfg.api_key.as_deref()))
    }

    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        // Gemini list-models endpoint is GET /v1beta/models. Always pass
        // ?pageSize=100 to bound per-page round-trips (Gemini default is 50).
        // Cursor is `pageToken` per Gemini's pagination docs.
        cfg.validate()?;
        let mut url = cfg.gemini_models_url()?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("pageSize", "100");
            if let Some(c) = cursor {
                q.append_pair("pageToken", c);
            }
        }
        Ok(apply_gemini_headers(http.get(url), cfg.api_key.as_deref()))
    }

    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        let parsed: wire::GeminiModelsListResponse = serde_json::from_str(body)?;
        let models: Vec<DiscoveredModel> = parsed
            .models
            .into_iter()
            // Filter: only models that support `generateContent` pass.
            // Removes embedding-only and TTS-only entries.
            .filter(|m| {
                m.supported_generation_methods
                    .iter()
                    .any(|s| s == "generateContent")
            })
            .map(|m| {
                // Strip the "models/" prefix; keep raw name if absent (defensive).
                let id = m.name.strip_prefix("models/").unwrap_or(&m.name).to_string();
                DiscoveredModel {
                    id,
                    display_name: m.display_name,
                    context_window: m.input_token_limit.map(|n| n.min(u32::MAX as u64) as u32),
                    max_output_tokens: m.output_token_limit.map(|n| n.min(u32::MAX as u64) as u32),
                }
            })
            .collect();
        Ok(ListModelsPage {
            models,
            next_cursor: parsed.next_page_token,
        })
    }
}

fn apply_gemini_headers(
    rb: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    match api_key.filter(|k| !k.is_empty()) {
        Some(k) => rb.header("x-goog-api-key", k),
        None => rb,
    }
}

/// Translate the OpenAI-shaped `SummarizerInput.messages` list (which the
/// compaction pipeline produces uniformly across adapters) into the
/// Gemini :generateContent shape. System messages lift to top-level
/// `systemInstruction`; user/assistant become user/model `contents`;
/// adjacent same-role entries merge. Tool roles aren't expected in
/// summarizer bodies (compaction sends `tools: None`); if any appear we
/// drop them silently.
fn build_gemini_summarizer_body(
    input: &SummarizerInput,
    _cfg: &LocalProviderConfig,
) -> GeminiGenerateRequest {
    use crate::local_provider::wire::Role;
    let mut system_parts: Vec<wire::GeminiTextPart> = Vec::new();
    let mut entries: Vec<wire::GeminiContent> = Vec::new();
    for msg in &input.messages {
        let text = msg.content.clone().unwrap_or_default();
        match msg.role {
            Role::System => {
                if !text.is_empty() {
                    system_parts.push(wire::GeminiTextPart { text });
                }
            }
            Role::User | Role::Assistant => {
                let role = match msg.role {
                    Role::User => wire::GeminiRole::User,
                    Role::Assistant => wire::GeminiRole::Model,
                    _ => unreachable!(),
                };
                let part = wire::GeminiOutboundPart::Text(wire::GeminiTextPart { text });
                match entries.last_mut() {
                    Some(last) if last.role == role => last.parts.push(part),
                    _ => entries.push(wire::GeminiContent {
                        role,
                        parts: vec![part],
                    }),
                }
            }
            Role::Tool => {
                // Compaction never emits role:Tool, but be defensive — drop
                // rather than misencode.
            }
        }
    }
    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(wire::GeminiSystemInstruction {
            parts: system_parts,
        })
    };
    GeminiGenerateRequest {
        system_instruction,
        contents: entries,
        tools: None,
        generation_config: wire::GeminiGenerationConfig::default(),
    }
}
