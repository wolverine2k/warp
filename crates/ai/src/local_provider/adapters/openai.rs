//! OpenAI Chat Completions adapter — wraps the existing wire code behind the
//! `ProviderAdapter` trait. Phase 2 stage B stub: real bodies land in Task 4.

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
        _input: &LocalProviderInput,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        unimplemented!("Task 4: hoist run_chat_turn body composition")
    }

    fn create_stream_decoder(
        &self,
        _ids: Option<StreamIds>,
        _skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        unimplemented!("Task 4: lift OpenAiSseAdapter construction")
    }

    fn build_summarizer_request(
        &self,
        _input: &SummarizerInput,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        unimplemented!("Task 4: hoist run_summarizer_turn body composition")
    }

    fn parse_summarizer_response(&self, _body: &str) -> Result<String, SummarizerError> {
        unimplemented!("Task 4: lift run_summarizer_turn parse logic")
    }

    fn build_probe_request(
        &self,
        _cfg: &LocalProviderConfig,
        _http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        unimplemented!("Task 4: implement probe (GET /models)")
    }
}
