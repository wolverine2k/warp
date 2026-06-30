//! Serde types for the Ollama native `/api/chat` endpoint.
//!
//! Coverage:
//! - **Request:** `model`, `stream`, `messages` (alternating roles
//!   user/assistant/system/tool with content + optional tool_calls), `tools`
//!   (`{type:"function", function:{name, description, parameters}}` — wire-
//!   identical to OpenAI's tool def envelope), `options.num_ctx`.
//! - **Streaming response:** one `OllamaChatChunk` per NDJSON line. Each
//!   chunk has `message.{role, content, tool_calls?}` and `done:bool`. The
//!   final chunk has `done:true` plus `done_reason` and `eval_count` /
//!   `prompt_eval_count` for token usage.
//!
//! Anything Ollama defines that we don't read is silently ignored (`#[serde(default)]`
//! on every optional field) — Ollama's API shape evolves quickly and we
//! want to be a forgiving consumer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- Request (outbound) ----------

#[derive(Debug, Clone, Serialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub stream: bool,
    pub messages: Vec<OllamaChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OllamaToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaChatMessage {
    pub role: OllamaRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OllamaOutboundToolCall>>,
    /// Phase 4c-2. Base64-encoded image attachments (no data-URI prefix —
    /// raw base64 only). Empty Vec is the default; serialized only when
    /// non-empty so text-only turns produce the same wire bytes as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OllamaRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Outbound tool call (in an assistant history message we send back).
/// Native shape: no `id`, no `type:"function"` — just
/// `{function: {name, arguments: <object>}}`. Ollama tolerates extra
/// fields if present but we emit the minimal native form.
#[derive(Debug, Clone, Serialize)]
pub struct OllamaOutboundToolCall {
    pub function: OllamaOutboundToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaOutboundToolCallFunction {
    pub name: String,
    /// JSON **object**, not a stringified-JSON `String`. This is the
    /// wire-level divergence from OpenAI's tool_call shape.
    pub arguments: Value,
}

/// Tool definition advertised in the request `tools` array. Same envelope
/// as OpenAI's `ToolDefinition` so the JSON Schema bodies port directly
/// from the v1 curated set.
#[derive(Debug, Clone, Serialize)]
pub struct OllamaToolDef {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: OllamaToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaToolFunction {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool arguments.
    pub parameters: Value,
}

/// Per-request runtime options. Phase 3b only threads `num_ctx`; the
/// remaining knobs (`num_predict`, `temperature`, `top_p`, etc.) are
/// Phase 4 polish exposing them per-model in settings.
#[derive(Debug, Clone, Serialize, Default)]
pub struct OllamaOptions {
    /// Sizes the model's KV-cache context window. **Critical for BYOP** —
    /// without this Ollama defaults to 2048/4096 and truncates long
    /// histories silently. Threaded from `cfg.context_window`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
}

// ---------- /api/tags list-models response ----------

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct OllamaTagsResponse {
    #[serde(default)]
    pub models: Vec<OllamaListedTag>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct OllamaListedTag {
    pub name: String,
    #[serde(default)]
    pub details: Option<OllamaTagDetails>,
    // `modified_at`, `size`, `digest` ignored.
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct OllamaTagDetails {
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub parameter_size: Option<String>,
    // `format`, `families`, `quantization_level` ignored.
}

// ---------- Streaming response (inbound) ----------

/// One NDJSON line from a streaming `/api/chat` response. The final chunk
/// is the only one with `done: true`; it also carries `done_reason` and
/// token-usage counts.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaChatChunk {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub message: Option<OllamaInboundMessage>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    /// Input tokens evaluated for the prompt (final chunk only).
    #[serde(default)]
    pub prompt_eval_count: Option<u64>,
    /// Output tokens generated (final chunk only).
    #[serde(default)]
    pub eval_count: Option<u64>,
    /// Some Ollama versions surface a top-level `error` mid-stream
    /// (e.g. model load failure). When present the decoder transitions to
    /// `Errored` and reports the message verbatim.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaInboundMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Option<Vec<OllamaInboundToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaInboundToolCall {
    pub function: OllamaInboundToolCallFunction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaInboundToolCallFunction {
    pub name: String,
    /// Object form. Ollama emits a structured JSON object here, matching
    /// the outbound tool_call shape.
    #[serde(default)]
    pub arguments: Value,
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
