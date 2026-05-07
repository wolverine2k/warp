//! Custom Local LLM Provider — Path 1 of issue #9303.
//!
//! Lets the Warp client chat directly with a user-configured OpenAI-compatible
//! HTTP endpoint (Ollama, LM Studio, vLLM, llama.cpp, NVIDIA NIM, etc.) instead
//! of routing through warp.dev's backend.
//!
//! Spec: `specs/GH9303/`. Gated by `FeatureFlag::LocalLlmProvider`.
//!
//! High-level flow per turn:
//! 1. Caller snapshots a `LocalProviderConfig` from settings + secure storage.
//! 2. `run_chat_turn` translates the in-memory `RequestParams` into an OpenAI
//!    Chat Completions request and POSTs it to the user's endpoint.
//! 3. `OpenAiSseAdapter` consumes the response SSE stream and emits a stream
//!    of `warp_multi_agent_api::ResponseEvent`s matching the contract the
//!    Warp client controller already speaks.

pub mod agent_provider_secrets;
pub mod compaction;
pub mod config;
pub mod llm_id;
pub mod prompt;
pub mod request;
pub mod response;
pub mod run;
pub mod tools;
pub mod wire;

pub use agent_provider_secrets::{AgentProviderSecrets, AgentProviderSecretsEvent};
pub use config::{LocalProviderConfig, LocalProviderConfigError};
pub use response::{AdapterError, OpenAiSseAdapter};
pub use run::{run_chat_turn, LocalResponseStream, LocalRunError};

#[cfg(test)]
mod response_tests;

#[cfg(test)]
mod tools_tests;
