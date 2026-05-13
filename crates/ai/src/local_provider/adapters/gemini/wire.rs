//! Serde types for the native Google Gemini
//! `:streamGenerateContent` / `:generateContent` endpoints.
//!
//! Coverage:
//! - **Request:** `systemInstruction` (lifted to top level — Gemini rejects
//!   role:system in `contents`), `contents[].parts[]` as a typed union of
//!   `{text}`, `{functionCall}`, `{functionResponse}`, `tools` wrapped in a
//!   single `functionDeclarations` envelope, `generationConfig` always emitted
//!   (as `{}` in Phase 3c).
//! - **Streaming response:** anonymous SSE `data:` chunks, each a complete
//!   `GenerateContentResponse` partial. `candidates[0].finishReason` is the
//!   terminator signal. `usageMetadata` may appear on any chunk (last value
//!   wins). `error` envelope surfaced for pre-stream 4xx bodies.
//!
//! Key wire divergences from OpenAI / Anthropic:
//! - Role vocabulary is `user` / `model` (not `assistant`).
//! - `functionCall.args` and `functionResponse.response` are **JSON objects**,
//!   not stringified-JSON strings (same as Ollama; opposite of OpenAI).
//! - No tool-call ids — matched by `name` only.
//! - Model lives in the **URL path**, not the request body.
//!
//! Anything Gemini defines that we don't read is silently ignored
//! (`#[serde(default)]` on every optional inbound field) — the Gemini API
//! evolves quickly and we want to be a forgiving consumer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- Request (outbound) ----------

#[derive(Debug, Clone, Serialize)]
pub struct GeminiGenerateRequest {
    /// Top-level system prompt. Gemini does NOT accept system messages in
    /// the `contents` array; the translator lifts the synthesized prompt
    /// here. Omitted when empty.
    #[serde(
        rename = "systemInstruction",
        skip_serializing_if = "Option::is_none"
    )]
    pub system_instruction: Option<GeminiSystemInstruction>,
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolEnvelope>>,
    /// Always emit (possibly empty) so the body shape stays stable for
    /// snapshot tests. Gemini tolerates `{}` and the absent form
    /// equivalently.
    #[serde(rename = "generationConfig")]
    pub generation_config: GeminiGenerationConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiSystemInstruction {
    pub parts: Vec<GeminiTextPart>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiContent {
    pub role: GeminiRole,
    pub parts: Vec<GeminiOutboundPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GeminiRole {
    User,
    Model,
}

/// Outbound (request-side) parts. Each variant serializes with exactly one
/// of `{text}`, `{functionCall}`, or `{functionResponse}` at the top level
/// (untagged enum so the serde shape matches Gemini's wire format).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum GeminiOutboundPart {
    Text(GeminiTextPart),
    FunctionCall(GeminiOutboundFunctionCallPart),
    FunctionResponse(GeminiOutboundFunctionResponsePart),
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiTextPart {
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionCallPart {
    #[serde(rename = "functionCall")]
    pub function_call: GeminiOutboundFunctionCall,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionCall {
    pub name: String,
    /// JSON object — Gemini's wire format expects an object here (same as
    /// Ollama; opposite of OpenAI's stringified-JSON-string convention).
    pub args: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionResponsePart {
    #[serde(rename = "functionResponse")]
    pub function_response: GeminiOutboundFunctionResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionResponse {
    pub name: String,
    /// Free-form JSON object. We always emit `{content: <string>}` for v1
    /// tool results; future structured tool outputs (Phase 4c) can land
    /// alongside without a wire change.
    pub response: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiToolEnvelope {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the function's input shape. Same shape as
    /// OpenAI's `function.parameters` — we reuse `schema_for_pub` from
    /// `tools.rs` (promoted in Phase 3b).
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GeminiGenerationConfig {
    // Phase 4 polish wires `max_output_tokens`, `temperature`, `top_p`, etc.
    // Phase 3c emits an empty object; Gemini tolerates it.
}

// ---------- Streaming response (inbound) ----------

/// One anonymous SSE `data:` chunk from a `:streamGenerateContent` response.
/// Each chunk is a complete `GenerateContentResponse` partial.
/// `candidates[0].finishReason` is the terminator signal.
/// `usageMetadata` may appear on any chunk; last value wins.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiStreamChunk {
    #[serde(default)]
    pub candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    pub usage_metadata: Option<GeminiUsageMetadata>,
    /// Top-level error envelope (rare in SSE stream; more common as the
    /// body of a 4xx pre-stream response). Surfaced via `record_upstream_error`.
    #[serde(default)]
    pub error: Option<GeminiErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiCandidate {
    #[serde(default)]
    pub content: Option<GeminiInboundContent>,
    #[serde(default, rename = "finishReason")]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiInboundContent {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub parts: Vec<GeminiInboundPart>,
}

/// Inbound (response-side) parts. Dispatched via `#[serde(untagged)]`
/// because the wire form is "one of {text, functionCall, functionResponse,
/// inlineData}" with no discriminator field. `Unknown` is a catch-all so
/// future part types (multimodal, code execution, thought) don't fail
/// deserialization.
///
/// Note: `#[serde(other)]` only works on tagged enums; for untagged enums
/// the idiomatic catch-all is a `Value`-wrapping variant which serde's
/// untagged dispatch reaches after exhausting all named variants.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GeminiInboundPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiInboundFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiInboundFunctionResponse,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: Value,
    },
    /// Catch-all for unknown part shapes — keeps unfamiliar payloads from
    /// erroring out mid-stream. Variants we recognize but ignore for now
    /// (e.g. thought / code-execution parts in 2.5 Pro thinking mode) also
    /// fall here.
    Unknown(Value),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiInboundFunctionCall {
    #[serde(default)]
    pub name: String,
    /// JSON object (Gemini's native shape). Defaults to an empty object
    /// when the field is absent, so downstream consumers can call
    /// `args.as_object()` without `Option::unwrap_or` ceremony.
    #[serde(default = "empty_object")]
    pub args: Value,
}

/// Default for `GeminiInboundFunctionCall::args` — Gemini always sends a
/// JSON object here semantically, but `#[serde(default)]` on
/// `serde_json::Value` produces `Value::Null`. We override so the field
/// has the same shape Gemini would have sent.
fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiInboundFunctionResponse {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub response: Value,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    pub prompt_token_count: u64,
    #[serde(default, rename = "candidatesTokenCount")]
    pub candidates_token_count: u64,
    #[serde(default, rename = "totalTokenCount")]
    pub total_token_count: u64,
    #[serde(default, rename = "cachedContentTokenCount")]
    pub cached_content_token_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiErrorEnvelope {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: String,
}

// ---------- List-models response (inbound) ----------

/// Top-level envelope for `GET /v1beta/models`.
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct GeminiModelsListResponse {
    #[serde(default)]
    pub models: Vec<GeminiListedModel>,
    #[serde(default, rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

/// One entry in the `models` array returned by `GET /v1beta/models`.
/// The `name` field includes a `"models/"` prefix that the parser strips.
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct GeminiListedModel {
    /// Full name including `"models/"` prefix; the parser strips it.
    pub name: String,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "inputTokenLimit")]
    pub input_token_limit: Option<u64>,
    #[serde(default, rename = "outputTokenLimit")]
    pub output_token_limit: Option<u64>,
    /// Methods like `"generateContent"`, `"embedContent"`, etc. Defaults to
    /// empty vec so entries with the field absent are filtered out.
    #[serde(default, rename = "supportedGenerationMethods")]
    pub supported_generation_methods: Vec<String>,
    // `version`, `description` are ignored.
}

// ---------- Non-streaming response (summarizer path) ----------

/// One-shot non-streaming `:generateContent` response — used by the
/// summarizer. Identical shape to a `GeminiStreamChunk` (the stream is
/// just incrementally-emitted instances of the same envelope), but the
/// non-streaming variant is decoded as a single value rather than line
/// by line.
pub type GeminiGenerateResponse = GeminiStreamChunk;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Request serialization ----

    #[test]
    fn serializes_minimal_text_request() {
        let req = GeminiGenerateRequest {
            system_instruction: None,
            contents: vec![GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                    text: "Hello!".into(),
                })],
            }],
            tools: None,
            generation_config: GeminiGenerationConfig::default(),
        };
        let v = serde_json::to_value(&req).unwrap();
        // camelCase wire keys
        assert!(v.get("systemInstruction").is_none());
        assert!(v.get("system_instruction").is_none());
        assert!(v.get("tools").is_none());
        assert!(v.get("generation_config").is_none());
        assert!(v.get("generationConfig").is_some());
        assert_eq!(v["generationConfig"], json!({}));
        assert_eq!(v["contents"][0]["role"], "user");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "Hello!");
    }

    #[test]
    fn serializes_system_instruction_lifted_to_top_level() {
        let req = GeminiGenerateRequest {
            system_instruction: Some(GeminiSystemInstruction {
                parts: vec![GeminiTextPart {
                    text: "You are helpful.".into(),
                }],
            }),
            contents: vec![GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                    text: "Hi".into(),
                })],
            }],
            tools: None,
            generation_config: GeminiGenerationConfig::default(),
        };
        let v = serde_json::to_value(&req).unwrap();
        // systemInstruction at top level, camelCase key
        assert_eq!(
            v["systemInstruction"]["parts"][0]["text"],
            "You are helpful."
        );
        // No role:system in contents
        for content in v["contents"].as_array().unwrap() {
            assert_ne!(content["role"], "system");
        }
    }

    #[test]
    fn omits_system_instruction_when_empty() {
        let req = GeminiGenerateRequest {
            system_instruction: None,
            contents: vec![GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                    text: "Hi".into(),
                })],
            }],
            tools: None,
            generation_config: GeminiGenerationConfig::default(),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("systemInstruction").is_none());
    }

    #[test]
    fn serializes_model_role_for_assistant_messages() {
        let content = GeminiContent {
            role: GeminiRole::Model,
            parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                text: "I can help.".into(),
            })],
        };
        let v = serde_json::to_value(&content).unwrap();
        // "model" not "assistant"
        assert_eq!(v["role"], "model");
        assert_ne!(v["role"], "assistant");
    }

    #[test]
    fn serializes_function_call_part_with_object_args() {
        let part = GeminiOutboundPart::FunctionCall(GeminiOutboundFunctionCallPart {
            function_call: GeminiOutboundFunctionCall {
                name: "read_files".into(),
                args: json!({"paths": ["Cargo.toml"]}),
            },
        });
        let v = serde_json::to_value(&part).unwrap();
        // camelCase wire key
        assert!(v.get("functionCall").is_some());
        assert!(v.get("function_call").is_none());
        assert_eq!(v["functionCall"]["name"], "read_files");
        // args is an object, NOT a string
        assert!(v["functionCall"]["args"].is_object());
        assert_eq!(v["functionCall"]["args"]["paths"][0], "Cargo.toml");
    }

    #[test]
    fn serializes_function_response_part_with_content_wrapper() {
        let part = GeminiOutboundPart::FunctionResponse(GeminiOutboundFunctionResponsePart {
            function_response: GeminiOutboundFunctionResponse {
                name: "read_files".into(),
                response: json!({"content": "rendered tool result"}),
            },
        });
        let v = serde_json::to_value(&part).unwrap();
        assert!(v.get("functionResponse").is_some());
        assert_eq!(v["functionResponse"]["name"], "read_files");
        assert_eq!(v["functionResponse"]["response"]["content"], "rendered tool result");
    }

    #[test]
    fn serializes_tool_envelope_with_function_declarations() {
        let req = GeminiGenerateRequest {
            system_instruction: None,
            contents: vec![],
            tools: Some(vec![GeminiToolEnvelope {
                function_declarations: vec![GeminiFunctionDeclaration {
                    name: "read_files".into(),
                    description: "Read files.".into(),
                    parameters: json!({"type": "object"}),
                }],
            }]),
            generation_config: GeminiGenerationConfig::default(),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["tools"][0]["functionDeclarations"][0]["name"], "read_files");
        assert_eq!(
            v["tools"][0]["functionDeclarations"][0]["description"],
            "Read files."
        );
        assert_eq!(
            v["tools"][0]["functionDeclarations"][0]["parameters"]["type"],
            "object"
        );
    }

    #[test]
    fn omits_tools_when_empty() {
        let req = GeminiGenerateRequest {
            system_instruction: None,
            contents: vec![],
            tools: None,
            generation_config: GeminiGenerationConfig::default(),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("tools").is_none());
    }

    // ---- Streaming response deserialization ----

    #[test]
    fn deserializes_text_chunk() {
        let s = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(s).unwrap();
        assert_eq!(chunk.candidates.len(), 1);
        let content = chunk.candidates[0].content.as_ref().unwrap();
        assert_eq!(content.role.as_deref(), Some("model"));
        assert_eq!(content.parts.len(), 1);
        match &content.parts[0] {
            GeminiInboundPart::Text { text } => assert_eq!(text, "Hello"),
            other => panic!("expected Text, got {other:?}"),
        }
        assert!(chunk.usage_metadata.is_none());
    }

    #[test]
    fn deserializes_function_call_chunk() {
        let s = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_files","args":{"paths":["x"]}}}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(s).unwrap();
        let parts = &chunk.candidates[0].content.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            GeminiInboundPart::FunctionCall { function_call } => {
                assert_eq!(function_call.name, "read_files");
                // args deserializes as a JSON object
                assert!(function_call.args.is_object());
                assert_eq!(function_call.args["paths"][0], "x");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn deserializes_final_chunk_with_finish_reason_and_usage_metadata() {
        let s = r#"{
            "candidates":[{"finishReason":"STOP","index":0}],
            "usageMetadata":{
                "promptTokenCount":50,
                "candidatesTokenCount":120,
                "totalTokenCount":170,
                "cachedContentTokenCount":10
            }
        }"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(s).unwrap();
        assert_eq!(
            chunk.candidates[0].finish_reason.as_deref(),
            Some("STOP")
        );
        let usage = chunk.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, 50);
        assert_eq!(usage.candidates_token_count, 120);
        assert_eq!(usage.total_token_count, 170);
        assert_eq!(usage.cached_content_token_count, 10);
    }

    #[test]
    fn deserializes_chunk_with_empty_parts_array() {
        // Final chunk may have no content/parts, just finishReason.
        let s = r#"{"candidates":[{"finishReason":"STOP"}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(s).unwrap();
        assert_eq!(
            chunk.candidates[0].finish_reason.as_deref(),
            Some("STOP")
        );
        assert!(chunk.candidates[0].content.is_none());
    }

    #[test]
    fn deserializes_unknown_part_variant_as_unknown() {
        // A payload shape we don't model (e.g. Gemini 2.5 Pro "thought" part)
        // must not error — it falls into the Unknown(Value) catch-all.
        let s = r#"{"candidates":[{"content":{"role":"model","parts":[{"thought":"hmm"}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(s).unwrap();
        let parts = &chunk.candidates[0].content.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], GeminiInboundPart::Unknown(_)));
    }

    #[test]
    fn deserializes_error_envelope() {
        let s = r#"{"error":{"code":400,"message":"API key not valid.","status":"INVALID_ARGUMENT"}}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(s).unwrap();
        let err = chunk.error.unwrap();
        assert_eq!(err.code, 400);
        assert_eq!(err.message, "API key not valid.");
        assert_eq!(err.status, "INVALID_ARGUMENT");
    }

    #[test]
    fn function_call_args_defaults_to_empty_object_when_field_absent() {
        let s = r#"{"name":"read_files"}"#;
        let parsed: GeminiInboundFunctionCall = serde_json::from_str(s).unwrap();
        assert_eq!(parsed.name, "read_files");
        assert!(parsed.args.is_object(), "args should default to an object, not null");
        assert_eq!(parsed.args.as_object().unwrap().len(), 0);
    }
}
