//! Serde types for the OpenAI Chat Completions wire format.
//!
//! Coverage is the subset of the API we actually send and receive:
//! - Request: `model`, `messages`, `tools`, `tool_choice`, `stream`.
//! - Streaming response: `choices[].delta.{content,reasoning_content,reasoning,tool_calls}`,
//!   `choices[].finish_reason`, plus a defensive `error` envelope some servers emit.
//!
//! Anything OpenAI defines that we don't read is silently ignored (`#[serde(default)]`
//! on every optional field). This is deliberate: local servers add and remove fields
//! freely, and we want to be a forgiving consumer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- Request (outbound) ----------

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    pub stream: bool,
    /// `stream_options: {"include_usage": true}` is required by OpenAI to get
    /// the final usage chunk; many compatible servers honour it and the rest
    /// silently ignore it. Phase B-3a relies on the resulting usage stats to
    /// detect token-overflow and auto-trigger compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    /// Either text content (most messages) or `null` for assistant turns that only
    /// emit tool calls. We model it as `Option<String>` and skip-if-none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Only present on assistant messages that emit tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Only present on `tool` role messages — references the assistant's prior call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Only present on `tool` role messages on some servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function" today
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool arguments.
    pub parameters: Value,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    None,
    Auto,
    Required,
}

// ---------- Response (streaming, inbound) ----------

/// One streaming chunk from `chat/completions` with `stream:true`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// Some servers emit a top-level `error` object instead of an HTTP error.
    /// We surface it through the adapter as a finish-with-error.
    #[serde(default)]
    pub error: Option<ChunkError>,
    /// OpenAI usage stats. With `stream_options: {"include_usage": true}` set,
    /// servers emit a final chunk with `choices: []` and `usage: {...}` after
    /// the last content chunk. Phase B-3a uses this to drive auto-compaction.
    #[serde(default)]
    pub usage: Option<OpenAiUsage>,
}

/// OpenAI-format usage stats. Field names match the upstream contract; some
/// servers omit `total_tokens` or `prompt_tokens_details`.
#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct OpenAiUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct OpenAiPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChunkChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: ChunkDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChunkDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    /// DeepSeek/Qwen-style explicit reasoning channel.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// OpenAI o1-style reasoning summary (when/if surfaced via the standard endpoint).
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// A streamed fragment of a tool call. OpenAI streams tool calls in pieces:
/// the first fragment for an index has the function name; subsequent fragments
/// add to `function.arguments`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub function: Option<ToolCallFunctionDelta>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolCallFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChunkError {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

// ---------- Response (non-streaming, used by the summarizer path) ----------

/// One-shot non-streaming Chat Completions response. Used by
/// `run_summarizer_turn` — the streaming SSE path returns
/// [`ChatCompletionChunk`] instead.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChatCompletionResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<ResponseChoice>,
    #[serde(default)]
    pub error: Option<ChunkError>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ResponseChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub message: Option<ResponseMessage>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ResponseMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    /// Some servers still emit reasoning fields on non-streaming responses.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
}

// ---------- Outbound tool-call shape (echoed back in messages history) ----------

/// What we send back in the next turn's `messages` array on an assistant turn that
/// included tool calls. Mirrors what the model emitted, with `function.arguments`
/// concatenated into a complete JSON string.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// Stringified JSON object.
    pub arguments: String,
}
