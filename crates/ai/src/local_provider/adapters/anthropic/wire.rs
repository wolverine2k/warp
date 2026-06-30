//! Serde types for the Anthropic Messages API.
//!
//! Coverage is the subset we send and receive:
//! - **Request:** `model`, `max_tokens` (required by Anthropic), top-level
//!   `system`, alternating `user`/`assistant` messages with content blocks,
//!   `tools`, `tool_choice`, `stream`.
//! - **Streaming response:** the named SSE event family
//!   (`message_start` / `content_block_start` / `content_block_delta` /
//!   `content_block_stop` / `message_delta` / `message_stop` / `ping` /
//!   `error`). Tagged on the JSON `type` field — equivalent to the SSE
//!   `event:` header so the decoder doesn't have to thread both signals
//!   through its state machine.
//! - **Non-streaming response:** used by the summarizer (`stream: false`).
//!
//! Anything Anthropic defines that we don't read is silently ignored (every
//! optional field uses `#[serde(default)]`). Local-relay servers tend to add
//! and remove fields freely; we want to be a forgiving consumer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- Request (outbound) ----------

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    /// Anthropic requires `max_tokens` on every Messages API request — unlike
    /// OpenAI where it's optional. See `request::resolve_max_tokens` for the
    /// heuristic that picks a value from `context_window`.
    pub max_tokens: u32,
    /// Top-level system prompt. Anthropic does **not** accept system messages
    /// in the `messages` array; we lift our synthesized system prompt out
    /// during request composition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<AnthropicToolChoice>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    /// Always an array of content blocks (not a bare string). The translator
    /// emits the array form uniformly so text-only and tool-using messages
    /// share the same shape.
    pub content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlock {
    /// Plain visible text. Both user and assistant messages use this for text.
    Text { text: String },
    /// Assistant invoking a tool. The `input` is a JSON object whose schema
    /// is determined by the tool definition.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// User message carrying the result of a previous assistant `tool_use`.
    /// `content` is a string today — Anthropic also accepts an array of
    /// blocks for multimodal results (Phase 4c).
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// Phase 4c-2. Inline image attachment. Anthropic wire shape:
    /// `{"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}}`
    #[serde(rename = "image")]
    Image { source: AnthropicMediaSource },
    /// Phase 4c-2. Inline document attachment (PDF). Anthropic wire shape:
    /// `{"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"..."}}`
    #[serde(rename = "document")]
    Document { source: AnthropicMediaSource },
}

/// Phase 4c-2. Shared source descriptor for `Image` and `Document` content
/// blocks. Only `base64` source type is wired today; the enum leaves room
/// for a future `url` variant without breaking wire compatibility.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnthropicMediaSource {
    #[serde(rename = "type")]
    pub source_type: AnthropicSourceType,
    pub media_type: String,
    /// Raw base64-encoded bytes — no `data:...;base64,` URI prefix.
    pub data: String,
}

/// Phase 4c-2. Source type discriminator for `AnthropicMediaSource`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicSourceType {
    Base64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's input shape.
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

// ---------- Streaming response events (inbound) ----------

/// One Server-Sent Event from the streaming Messages endpoint. Tagged on the
/// JSON `type` field (which mirrors the SSE `event:` header line — the
/// decoder gets the discriminator from the JSON itself so it doesn't have to
/// rely on the SSE parser surfacing the event name).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicStreamEvent {
    MessageStart {
        message: StreamMessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: StreamContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: StreamContentDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaPayload,
        #[serde(default)]
        usage: Option<MessageDeltaUsage>,
    },
    MessageStop,
    /// Periodic keep-alive event. No payload; ignore.
    Ping,
    Error {
        error: AnthropicErrorEnvelope,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StreamMessageStart {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub usage: Option<MessageStartUsage>,
}

/// Usage emitted on `message_start`. `input_tokens` is final here;
/// `output_tokens` starts at 0/1 and the running total comes back via
/// subsequent `message_delta.usage` events.
#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct MessageStartUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamContentDelta {
    TextDelta {
        text: String,
    },
    /// Tool input streamed as a series of JSON-fragment strings; the decoder
    /// concatenates them and parses the result on `content_block_stop`.
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    /// Extended-thinking signature delta — ignored by the decoder today.
    /// Kept as a known variant so unfamiliar payloads don't fail
    /// deserialization on Claude 4.x extended-thinking streams.
    SignatureDelta {
        #[serde(default)]
        signature: String,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageDeltaPayload {
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct MessageDeltaUsage {
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AnthropicErrorEnvelope {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

// ---------- List-models response (inbound, GET /v1/models) ----------

/// Wire type for Anthropic's `GET /v1/models` paginated response.
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct AnthropicModelsListResponse {
    #[serde(default)]
    pub data: Vec<AnthropicListedModel>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub last_id: Option<String>,
}

/// One entry in `AnthropicModelsListResponse::data`.
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct AnthropicListedModel {
    /// Required — missing `id` is a parse error.
    pub id: String,
    /// Human-readable name. Present on all production models; absent on some
    /// alpha/internal entries.
    #[serde(default)]
    pub display_name: Option<String>,
    // `type` and `created_at` deliberately ignored — Phase 4a doesn't surface them.
}

// ---------- Non-streaming response (used by the summarizer path) ----------

/// One-shot non-streaming Messages response. Used by `run_summarizer_turn`;
/// the streaming SSE path returns `AnthropicStreamEvent`s instead.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AnthropicMessageResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: Vec<ResponseContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Top-level error envelope on 4xx/5xx responses with a JSON body. The
    /// summarizer treats this as `SummarizerError::UpstreamErrorEnvelope`.
    #[serde(default)]
    pub error: Option<AnthropicErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
