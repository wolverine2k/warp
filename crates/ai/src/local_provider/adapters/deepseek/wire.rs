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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
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
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Request serialization ----

    #[test]
    fn serializes_minimal_text_only_request() {
        let req = DeepSeekChatRequest {
            model: "deepseek-chat".into(),
            stream: true,
            messages: vec![DeepSeekChatMessage {
                role: DeepSeekRole::User,
                content: Some("Hello!".into()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "deepseek-chat");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "Hello!");
        // tools omitted when None
        assert!(v.get("tools").is_none());
    }

    #[test]
    fn serializes_system_user_assistant_sequence() {
        let req = DeepSeekChatRequest {
            model: "deepseek-chat".into(),
            stream: false,
            messages: vec![
                DeepSeekChatMessage {
                    role: DeepSeekRole::System,
                    content: Some("You are helpful.".into()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                DeepSeekChatMessage {
                    role: DeepSeekRole::User,
                    content: Some("Hi".into()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                DeepSeekChatMessage {
                    role: DeepSeekRole::Assistant,
                    content: Some("Hello!".into()),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            tools: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        // roles are lowercase strings
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][1]["role"], "user");
        assert_eq!(v["messages"][2]["role"], "assistant");
        // messages array has correct shape
        assert_eq!(v["messages"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn serializes_assistant_with_tool_calls() {
        let msg = DeepSeekChatMessage {
            role: DeepSeekRole::Assistant,
            content: None,
            tool_calls: Some(vec![DeepSeekOutboundToolCall {
                id: "call_abc123".into(),
                kind: "function",
                function: DeepSeekOutboundToolCallFunction {
                    name: "read_files".into(),
                    arguments: r#"{"paths":["Cargo.toml"]}"#.into(),
                },
            }]),
            tool_call_id: None,
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["tool_calls"][0]["id"], "call_abc123");
        assert_eq!(v["tool_calls"][0]["type"], "function");
        assert_eq!(v["tool_calls"][0]["function"]["name"], "read_files");
        // arguments is a STRING, not an object
        assert!(v["tool_calls"][0]["function"]["arguments"].is_string());
        assert_eq!(
            v["tool_calls"][0]["function"]["arguments"],
            r#"{"paths":["Cargo.toml"]}"#
        );
    }

    #[test]
    fn serializes_role_tool_with_tool_call_id() {
        let msg = DeepSeekChatMessage {
            role: DeepSeekRole::Tool,
            content: Some("file contents here".into()),
            tool_calls: None,
            tool_call_id: Some("call_abc123".into()),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["role"], "tool");
        assert_eq!(v["tool_call_id"], "call_abc123");
        assert_eq!(v["content"], "file contents here");
    }

    #[test]
    fn omits_tools_when_none() {
        let req = DeepSeekChatRequest {
            model: "deepseek-chat".into(),
            stream: true,
            messages: vec![],
            tools: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("tools").is_none());
    }

    #[test]
    fn serializes_tools_array_with_function_wrapper() {
        let req = DeepSeekChatRequest {
            model: "deepseek-chat".into(),
            stream: true,
            messages: vec![],
            tools: Some(vec![DeepSeekToolDef {
                kind: "function",
                function: DeepSeekToolFunction {
                    name: "read_files".into(),
                    description: "Read files from the filesystem.".into(),
                    parameters: json!({"type": "object", "properties": {}}),
                },
            }]),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "read_files");
        assert_eq!(
            v["tools"][0]["function"]["description"],
            "Read files from the filesystem."
        );
        assert_eq!(v["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn tool_call_arguments_serialize_as_string_not_object() {
        let call = DeepSeekOutboundToolCall {
            id: "call_xyz".into(),
            kind: "function",
            function: DeepSeekOutboundToolCallFunction {
                name: "run_command".into(),
                arguments: r#"{"cmd":"ls"}"#.into(),
            },
        };
        let v = serde_json::to_value(&call).unwrap();
        // Explicitly assert Value::String, NOT Value::Object
        assert!(
            matches!(v["function"]["arguments"], serde_json::Value::String(_)),
            "arguments must be a JSON string, not an object: got {:?}",
            v["function"]["arguments"]
        );
    }

    // ---- Streaming response deserialization ----

    #[test]
    fn deserializes_reasoning_delta_chunk() {
        let s = r#"{
            "id":"chatcmpl-1",
            "object":"chat.completion.chunk",
            "created":1700000000,
            "model":"deepseek-reasoner",
            "choices":[{
                "index":0,
                "delta":{"role":"assistant","reasoning_content":"Let me think..."},
                "finish_reason":null
            }]
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        assert_eq!(delta.reasoning_content.as_deref(), Some("Let me think..."));
        assert!(delta.content.is_none());
    }

    #[test]
    fn deserializes_content_delta_chunk() {
        let s = r#"{
            "id":"chatcmpl-2",
            "object":"chat.completion.chunk",
            "model":"deepseek-chat",
            "choices":[{
                "index":0,
                "delta":{"content":"Hello"},
                "finish_reason":null
            }]
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        assert_eq!(delta.content.as_deref(), Some("Hello"));
        assert!(delta.reasoning_content.is_none());
    }

    #[test]
    fn deserializes_tool_call_fragment_chunk() {
        let s = r#"{
            "id":"chatcmpl-3",
            "model":"deepseek-chat",
            "choices":[{
                "index":0,
                "delta":{
                    "tool_calls":[{
                        "index":0,
                        "id":"call_abc",
                        "type":"function",
                        "function":{"name":"read_files","arguments":"{\"paths\":["}
                    }]
                },
                "finish_reason":null
            }]
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        let tool_calls = delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls[0].index, 0);
        let func = tool_calls[0].function.as_ref().unwrap();
        assert_eq!(func.arguments.as_deref(), Some("{\"paths\":["));
    }

    #[test]
    fn deserializes_final_chunk_with_finish_reason_and_usage() {
        let s = r#"{
            "id":"chatcmpl-4",
            "model":"deepseek-chat",
            "choices":[{
                "index":0,
                "delta":{},
                "finish_reason":"stop"
            }],
            "usage":{
                "prompt_tokens":42,
                "completion_tokens":88,
                "total_tokens":130
            }
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 42);
        assert_eq!(usage.completion_tokens, 88);
    }

    #[test]
    fn deserializes_usage_with_reasoning_tokens() {
        let s = r#"{
            "id":"chatcmpl-5",
            "model":"deepseek-reasoner",
            "choices":[],
            "usage":{
                "prompt_tokens":100,
                "completion_tokens":200,
                "total_tokens":300,
                "prompt_cache_hit_tokens":50,
                "prompt_cache_miss_tokens":50,
                "completion_tokens_details":{"reasoning_tokens":150}
            }
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_cache_hit_tokens, 50);
        assert_eq!(usage.prompt_cache_miss_tokens, 50);
        let details = usage.completion_tokens_details.unwrap();
        assert_eq!(details.reasoning_tokens, 150);
    }

    #[test]
    fn deserializes_error_envelope() {
        let s = r#"{
            "error":{
                "message":"Invalid API key.",
                "type":"invalid_request_error",
                "code":"invalid_api_key"
            }
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        let err = chunk.error.unwrap();
        assert_eq!(err.message, "Invalid API key.");
        assert_eq!(err.kind, "invalid_request_error");
        assert_eq!(err.code.as_deref(), Some("invalid_api_key"));
    }

    #[test]
    fn deserializes_chunk_with_role_only_delta() {
        // First chunk from deepseek-chat: delta contains only role, no content.
        let s = r#"{
            "id":"chatcmpl-6",
            "model":"deepseek-chat",
            "choices":[{
                "index":0,
                "delta":{"role":"assistant"},
                "finish_reason":null
            }]
        }"#;
        let chunk: DeepSeekChatChunk = serde_json::from_str(s).unwrap();
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        assert_eq!(delta.role.as_deref(), Some("assistant"));
        assert!(delta.content.is_none());
        assert!(delta.reasoning_content.is_none());
        assert!(delta.tool_calls.is_none());
    }

    #[test]
    fn deserializes_non_streaming_response_with_reasoning_content() {
        // Mirrors a real deepseek-reasoner /chat/completions response (stream:false).
        // Confirms the non-streaming type set (DeepSeekChatResponse / ResponseChoice /
        // ResponseMessage) deserializes correctly — including the reasoning_content
        // field on the message — so Task 4's parse_summarizer_response path has
        // serde coverage before it lands.
        let s = r#"{
            "id":"chatcmpl-7",
            "model":"deepseek-reasoner",
            "choices":[{
                "index":0,
                "message":{
                    "role":"assistant",
                    "content":"The answer is 4.",
                    "reasoning_content":"2+2=4"
                },
                "finish_reason":"stop"
            }],
            "usage":{
                "prompt_tokens":10,
                "completion_tokens":20,
                "total_tokens":30
            }
        }"#;
        let resp: DeepSeekChatResponse = serde_json::from_str(s).unwrap();
        assert_eq!(resp.id.as_deref(), Some("chatcmpl-7"));
        assert_eq!(resp.model.as_deref(), Some("deepseek-reasoner"));
        assert_eq!(resp.choices.len(), 1);
        let choice = &resp.choices[0];
        assert_eq!(choice.index, Some(0));
        assert_eq!(choice.finish_reason.as_deref(), Some("stop"));
        let msg = choice.message.as_ref().expect("message present");
        assert_eq!(msg.role.as_deref(), Some("assistant"));
        assert_eq!(msg.content.as_deref(), Some("The answer is 4."));
        assert_eq!(msg.reasoning_content.as_deref(), Some("2+2=4"));
        let usage = resp.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
    }
}
