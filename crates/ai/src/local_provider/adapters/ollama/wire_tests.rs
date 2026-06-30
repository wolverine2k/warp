use serde_json::json;

use super::*;

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
                images: Vec::new(),
            },
            OllamaChatMessage {
                role: OllamaRole::User,
                content: "Hello!".into(),
                tool_calls: None,
                images: Vec::new(),
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
        images: Vec::new(),
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
        images: Vec::new(),
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
