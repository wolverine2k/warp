//! OpenAI Chat Completions adapter — wraps the existing wire code behind the
//! `ProviderAdapter` trait. Lifts request composition, summarizer body
//! composition + parsing, and stream-decoder construction out of `run.rs`
//! verbatim. Net new logic: the probe builder (`GET {base_url}/models`).

use serde::Deserialize;

use crate::local_provider::{
    request::compose_chat_completion_request,
    response::OpenAiSseAdapter,
    run::first_chars,
    wire::{ChatCompletionRequest, ChatCompletionResponse},
};

use super::{
    AdapterError, AgentProviderApiType, DiscoveredModel, ListModelsPage, LocalProviderConfig,
    LocalProviderInput, ProviderAdapter, StreamDecoder, StreamIds, SummarizerError, SummarizerInput,
};

// ---------------------------------------------------------------------------
// Wire types for GET /v1/models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
struct OpenAiModelsListResponse {
    #[serde(default)]
    data: Vec<OpenAiListedModel>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct OpenAiListedModel {
    /// Required. OpenAI always emits this; treat missing as a parse error.
    id: String,
    // `object` / `created` / `owned_by` deliberately ignored — Phase 4a
    // doesn't surface them.
}

pub struct OpenAiAdapter;

impl ProviderAdapter for OpenAiAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::OpenAi
    }

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.chat_completions_url()?;
        let body = compose_chat_completion_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        let mut req = http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .body(body_json);
        if let Some(key) = &cfg.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        Ok(req)
    }

    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        let mut adapter = match ids {
            Some(ids) => OpenAiSseAdapter::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => OpenAiSseAdapter::new(),
        };
        if skip_create_task {
            adapter.skip_create_task();
        }
        Box::new(adapter)
    }

    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.chat_completions_url()?;
        let body = ChatCompletionRequest {
            model: cfg.model_id.clone(),
            messages: input.messages.clone(),
            tools: None,
            tool_choice: None,
            stream: false,
            stream_options: None,
        };
        let body_json = serde_json::to_string(&body)?;
        let mut req = http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body_json);
        if let Some(key) = &cfg.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        Ok(req)
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: ChatCompletionResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!("{e}: {}", first_chars(body, 200)))
        })?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(err.message));
        }
        parsed
            .choices
            .into_iter()
            .find_map(|choice| {
                let m = choice.message?;
                let candidate = m
                    .content
                    .filter(|s| !s.trim().is_empty())
                    .or(m.reasoning_content)
                    .or(m.reasoning)?;
                let trimmed = candidate.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .ok_or(SummarizerError::NoContent)
    }

    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.models_list_url()?;
        let mut req = http.get(url);
        if let Some(key) = &cfg.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        Ok(req)
    }

    fn build_list_models_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
        _cursor: Option<&str>, // OpenAi is unpaginated; ignore
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        let url = cfg.models_list_url()?;
        let mut req = http.get(url);
        req = apply_openai_headers(req, cfg); // Authorization: Bearer
        Ok(req)
    }

    fn parse_list_models_response(
        &self,
        body: &str,
    ) -> Result<ListModelsPage, AdapterError> {
        let parsed: OpenAiModelsListResponse = serde_json::from_str(body)?;
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

fn apply_openai_headers(
    mut req: reqwest::RequestBuilder,
    cfg: &LocalProviderConfig,
) -> reqwest::RequestBuilder {
    if let Some(key) = &cfg.api_key {
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }
    }
    req
}

#[cfg(test)]
#[path = "openai_list_models_tests.rs"]
mod list_models_tests;
