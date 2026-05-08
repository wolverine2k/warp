//! Wire-protocol variant of an Agent provider. The dispatch layer uses this
//! to select a `ProviderAdapter` impl. Lives in the ai crate (not the
//! settings module) because adapter selection is a wire-protocol decision —
//! the settings module just re-exports it for serde compatibility.

use serde::{Deserialize, Serialize};

/// The wire-protocol variant the provider's `base_url` actually speaks. Used
/// at request time by the dispatch layer to choose the right
/// request/response codec. Phase 1b-1 only defined the enum; Phase 2 wires
/// it through `LocalProviderConfig.api_type` and `select_adapter`. Phase 3
/// adds per-variant adapter implementations beyond OpenAI.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum_macros::EnumIter,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderApiType {
    /// OpenAI Chat Completions (`POST /v1/chat/completions`). Covers OpenAI,
    /// DeepSeek, SiliconFlow, OpenRouter, Moonshot, vLLM, llama.cpp, Ollama
    /// behind its OpenAI-compat shim, and most "OpenAI-compatible" gateways.
    #[default]
    OpenAi,
    /// OpenAI Responses API (`POST /v1/responses`). Used by GPT-5 / Codex /
    /// Pro tier models.
    OpenAiResp,
    /// Google Gemini native protocol (generativelanguage.googleapis.com).
    Gemini,
    /// Anthropic Messages API native protocol (api.anthropic.com).
    Anthropic,
    /// Ollama native protocol (`/api/chat`). Distinct from Ollama's
    /// OpenAI-compat shim, which uses `OpenAi` instead.
    Ollama,
    /// DeepSeek native protocol. Differs from `OpenAi` in that thinking-mode
    /// models require `reasoning_content` round-tripped back to the server;
    /// only this variant handles that field.
    DeepSeek,
}
