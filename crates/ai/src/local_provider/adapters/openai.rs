//! OpenAI Chat Completions adapter — wraps the existing wire code behind the
//! `ProviderAdapter` trait. Lifts request composition, summarizer body
//! composition + parsing, and stream-decoder construction out of `run.rs`
//! verbatim. Net new logic: the probe builder (`GET {base_url}/models`).

use crate::local_provider::{
    request::compose_chat_completion_request,
    response::OpenAiSseAdapter,
    run::first_chars,
    wire::{ChatCompletionRequest, ChatCompletionResponse},
};

use super::{
    AdapterError, AgentProviderApiType, LocalProviderConfig, LocalProviderInput, ProviderAdapter,
    StreamDecoder, StreamIds, SummarizerError, SummarizerInput,
};

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
}
