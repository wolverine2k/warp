use serde_json::json;

use super::*;

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
    assert_eq!(
        v["functionResponse"]["response"]["content"],
        "rendered tool result"
    );
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
    assert_eq!(
        v["tools"][0]["functionDeclarations"][0]["name"],
        "read_files"
    );
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
    assert_eq!(chunk.candidates[0].finish_reason.as_deref(), Some("STOP"));
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
    assert_eq!(chunk.candidates[0].finish_reason.as_deref(), Some("STOP"));
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
    assert!(
        parsed.args.is_object(),
        "args should default to an object, not null"
    );
    assert_eq!(parsed.args.as_object().unwrap().len(), 0);
}
