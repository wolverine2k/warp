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
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Request serialization ----

    #[test]
    fn serializes_minimal_text_only_request() {
        let req = OllamaChatRequest {
            model: "llama3.1".into(),
            stream: true,
            messages: vec![
                OllamaChatMessage {
                    role: OllamaRole::System,
                    content: "You are helpful.".into(),
                    tool_calls: None,
                },
                OllamaChatMessage {
                    role: OllamaRole::User,
                    content: "Hello!".into(),
                    tool_calls: None,
                },
            ],
            tools: None,
            options: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "llama3.1");
        assert_eq!(v["stream"], true);
        assert!(v.get("tools").is_none());
        assert!(v.get("options").is_none());
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][0]["content"], "You are helpful.");
        assert_eq!(v["messages"][1]["role"], "user");
    }

    #[test]
    fn serializes_assistant_tool_call_with_arguments_as_object() {
        let msg = OllamaChatMessage {
            role: OllamaRole::Assistant,
            content: String::new(),
            tool_calls: Some(vec![OllamaOutboundToolCall {
                function: OllamaOutboundToolCallFunction {
                    name: "read_files".into(),
                    arguments: json!({"paths": ["Cargo.toml"]}),
                },
            }]),
        };
        let v = serde_json::to_value(&msg).unwrap();
        // arguments must serialize as an object, not a string.
        assert!(v["tool_calls"][0]["function"]["arguments"].is_object());
        assert_eq!(
            v["tool_calls"][0]["function"]["arguments"]["paths"][0],
            "Cargo.toml"
        );
        // No `id` or `type:"function"` on the tool_call itself.
        assert!(v["tool_calls"][0].get("id").is_none());
        assert!(v["tool_calls"][0].get("type").is_none());
    }

    #[test]
    fn serializes_tool_definition_in_openai_style_envelope() {
        let t = OllamaToolDef {
            kind: "function",
            function: OllamaToolFunction {
                name: "read_files".into(),
                description: "Read files.".into(),
                parameters: json!({"type": "object"}),
            },
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "read_files");
        assert_eq!(v["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn serializes_options_num_ctx() {
        let opts = OllamaOptions {
            num_ctx: Some(128_000),
        };
        let v = serde_json::to_value(&opts).unwrap();
        assert_eq!(v["num_ctx"], 128_000);
    }

    #[test]
    fn options_skips_none_num_ctx() {
        let opts = OllamaOptions { num_ctx: None };
        let v = serde_json::to_value(&opts).unwrap();
        assert!(v.get("num_ctx").is_none());
    }

    #[test]
    fn serializes_tool_role_message_with_just_content() {
        // Native Ollama doesn't need tool_call_id or name on tool messages.
        let msg = OllamaChatMessage {
            role: OllamaRole::Tool,
            content: "result text".into(),
            tool_calls: None,
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["role"], "tool");
        assert_eq!(v["content"], "result text");
        assert!(v.get("tool_calls").is_none());
    }

    // ---- Streaming chunk deserialization ----

    #[test]
    fn deserializes_text_streaming_chunk() {
        let s = r#"{"model":"llama3.1","created_at":"2026-05-11T00:00:00Z","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(s).unwrap();
        assert_eq!(chunk.model.as_deref(), Some("llama3.1"));
        assert!(!chunk.done);
        let msg = chunk.message.unwrap();
        assert_eq!(msg.role.as_deref(), Some("assistant"));
        assert_eq!(msg.content, "Hello");
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn deserializes_tool_call_chunk_with_arguments_as_object() {
        let s = r#"{"model":"llama3.1","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_files","arguments":{"paths":["x"]}}}]},"done":false}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(s).unwrap();
        let tool_calls = chunk.message.unwrap().tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "read_files");
        assert!(tool_calls[0].function.arguments.is_object());
        assert_eq!(tool_calls[0].function.arguments["paths"][0], "x");
    }

    #[test]
    fn deserializes_final_done_chunk_with_usage() {
        let s = r#"{
            "model":"llama3.1",
            "created_at":"...",
            "message":{"role":"assistant","content":""},
            "done":true,
            "done_reason":"stop",
            "prompt_eval_count":50,
            "eval_count":120
        }"#;
        let chunk: OllamaChatChunk = serde_json::from_str(s).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("stop"));
        assert_eq!(chunk.prompt_eval_count, Some(50));
        assert_eq!(chunk.eval_count, Some(120));
    }

    #[test]
    fn deserializes_chunk_with_top_level_error() {
        let s = r#"{"error":"model 'foo' not found"}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(s).unwrap();
        assert_eq!(chunk.error.as_deref(), Some("model 'foo' not found"));
        assert!(chunk.message.is_none());
        assert!(!chunk.done);
    }

    #[test]
    fn deserializes_chunk_ignores_unknown_fields() {
        // Forward-compat: future Ollama versions may add new fields. We
        // shouldn't fail on them.
        let s = r#"{"model":"llama3.1","message":{"role":"assistant","content":"hi"},"done":false,"some_future_field":42}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(s).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.unwrap().content, "hi");
    }
}
