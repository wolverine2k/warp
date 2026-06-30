//! Serde types for the DeepSeek `/chat/completions` endpoint.
//!
//! Coverage:
//! - **Request:** `model`, `stream`, `messages` (system/user/assistant/tool
//!   roles; optional `content`, `tool_calls`, `tool_call_id`), `tools`
//!   (`{type:"function", function:{name, description, parameters}}`).
//! - **Streaming response:** one `DeepSeekChatChunk` per SSE `data:` line.
//!   `choices[0].delta` carries `content`, `reasoning_content` (DeepSeek-
//!   reasoner specific), and incremental `tool_calls` fragments. The final
//!   chunk has `choices[0].finish_reason` set and an optional `usage` object.
//! - **Non-streaming response:** `DeepSeekChatResponse` used by the
//!   summarizer path. `choices[0].message` may carry `reasoning_content`
//!   (ignored by the summarizer; only `content` is read).
//!
//! DeepSeek's wire shape is intentionally OpenAI-compatible. The ONE
//! meaningful divergence is `delta.reasoning_content` / `message.reasoning_content`,
//! which `deepseek-reasoner` emits alongside the normal content channel to
//! surface chain-of-thought reasoning.
//!
//! Anything DeepSeek defines that we don't read is silently ignored
//! (`#[serde(default)]` on every optional inbound field) — we want to be a
//! forgiving consumer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// Re-export the shared content types so callers in this module don't need to
// reach into `crate::local_provider::wire` directly.
pub use crate::local_provider::wire::{ChatContentPart, ChatMessageContent, ImageUrlSpec};

// ---------- Request (outbound) ----------

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekChatRequest {
    pub model: String,
    pub stream: bool,
    pub messages: Vec<DeepSeekChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<DeepSeekToolDef>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekChatMessage {
    pub role: DeepSeekRole,
    /// Either a plain text string (text-only turn) or an array of content
    /// parts (turn with image attachments). Uses the same untagged
    /// `ChatMessageContent` enum as the OpenAi adapter — DeepSeek's API
    /// accepts the identical wire shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatMessageContent>,
    /// Required on assistant messages that carry tool_calls; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeepSeekOutboundToolCall>>,
    /// Required on role:"tool" messages; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeepSeekRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekOutboundToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: DeepSeekOutboundToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekOutboundToolCallFunction {
    pub name: String,
    /// Stringified JSON — same as OpenAI's convention. NOT a Value
    /// object. The translator stringifies the typed proto args before
    /// emitting.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekToolDef {
    #[serde(rename = "type")]
    pub kind: &'static str, // "function"
    pub function: DeepSeekToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// ---------- Streaming response (inbound) ----------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekChatChunk {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<DeepSeekStreamChoice>,
    #[serde(default)]
    pub usage: Option<DeepSeekUsage>,
    #[serde(default)]
    pub error: Option<DeepSeekErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamChoice {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub delta: Option<DeepSeekStreamDelta>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    /// THE Phase-3d-specific field. Streams the reasoning channel
    /// alongside `content`. Present only on `deepseek-reasoner`.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<DeepSeekStreamToolCall>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamToolCall {
    /// Per-call slot index — required. Fragments accumulate by index.
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub function: Option<DeepSeekStreamToolCallFunction>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamToolCallFunction {
    #[serde(default)]
    pub name: Option<String>,
    /// Fragment of stringified-JSON arguments. Accumulate across chunks.
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct DeepSeekUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// DeepSeek-specific: prompt-cache hit/miss counters. Phase 3d
    /// deserializes but doesn't surface (Phase 4 polish).
    #[serde(default)]
    pub prompt_cache_hit_tokens: u64,
    #[serde(default)]
    pub prompt_cache_miss_tokens: u64,
    /// DeepSeek-specific: reasoning vs final-answer token split. Phase 3d
    /// deserializes but doesn't surface (folded into completion_tokens).
    #[serde(default)]
    pub completion_tokens_details: Option<DeepSeekCompletionDetails>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct DeepSeekCompletionDetails {
    #[serde(default)]
    pub reasoning_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekErrorEnvelope {
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub code: Option<String>,
}

// ---------- Models list response (GET /models) ----------

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct DeepSeekModelsListResponse {
    #[serde(default)]
    pub data: Vec<DeepSeekListedModel>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct DeepSeekListedModel {
    pub id: String,
    // `object`, `owned_by` ignored — DeepSeek doesn't return metadata we need.
}

// ---------- Non-streaming response (summarizer path) ----------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekChatResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<DeepSeekResponseChoice>,
    #[serde(default)]
    pub usage: Option<DeepSeekUsage>,
    #[serde(default)]
    pub error: Option<DeepSeekErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekResponseChoice {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub message: Option<DeepSeekResponseMessage>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekResponseMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    /// Present on `deepseek-reasoner` non-streaming responses. The
    /// summarizer ignores this and reads only `content`.
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
