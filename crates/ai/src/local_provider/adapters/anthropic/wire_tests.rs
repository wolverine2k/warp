use serde_json::json;

use super::*;

// ---- Request serialization ----

#[test]
fn serializes_minimal_text_only_request() {
    let req = AnthropicMessagesRequest {
        model: "claude-sonnet-4-6".into(),
        max_tokens: 4096,
        system: Some("You are a helpful assistant.".into()),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: vec![AnthropicContentBlock::Text {
                text: "Hello!".into(),
            }],
        }],
        tools: None,
        tool_choice: None,
        stream: true,
    };
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["model"], "claude-sonnet-4-6");
    assert_eq!(json["max_tokens"], 4096);
    assert_eq!(json["system"], "You are a helpful assistant.");
    assert_eq!(json["stream"], true);
    // tools / tool_choice skipped when None.
    assert!(json.get("tools").is_none());
    assert!(json.get("tool_choice").is_none());
    // Role lowercase, content is array of {type:"text", text:...}.
    assert_eq!(json["messages"][0]["role"], "user");
    assert_eq!(json["messages"][0]["content"][0]["type"], "text");
    assert_eq!(json["messages"][0]["content"][0]["text"], "Hello!");
}

#[test]
fn serializes_assistant_tool_use_block() {
    let block = AnthropicContentBlock::ToolUse {
        id: "toolu_01ABC".into(),
        name: "read_files".into(),
        input: json!({"paths": ["Cargo.toml"]}),
    };
    let v = serde_json::to_value(&block).unwrap();
    assert_eq!(v["type"], "tool_use");
    assert_eq!(v["id"], "toolu_01ABC");
    assert_eq!(v["name"], "read_files");
    assert_eq!(v["input"]["paths"][0], "Cargo.toml");
}

#[test]
fn serializes_user_tool_result_block() {
    let block = AnthropicContentBlock::ToolResult {
        tool_use_id: "toolu_01ABC".into(),
        content: "file contents...".into(),
        is_error: None,
    };
    let v = serde_json::to_value(&block).unwrap();
    assert_eq!(v["type"], "tool_result");
    assert_eq!(v["tool_use_id"], "toolu_01ABC");
    assert_eq!(v["content"], "file contents...");
    // is_error skipped when None.
    assert!(v.get("is_error").is_none());
}

#[test]
fn serializes_tool_definition_without_function_wrapper() {
    let t = AnthropicToolDef {
        name: "read_files".into(),
        description: "Read file contents.".into(),
        input_schema: json!({"type": "object", "properties": {}}),
    };
    let v = serde_json::to_value(&t).unwrap();
    // No `function: {...}` wrapper; fields live at top level.
    assert_eq!(v["name"], "read_files");
    assert_eq!(v["description"], "Read file contents.");
    assert_eq!(v["input_schema"]["type"], "object");
    assert!(v.get("function").is_none());
    assert!(v.get("type").is_none());
}

#[test]
fn serializes_tool_choice_auto_and_named() {
    let auto = serde_json::to_value(AnthropicToolChoice::Auto).unwrap();
    assert_eq!(auto["type"], "auto");

    let named = serde_json::to_value(AnthropicToolChoice::Tool {
        name: "read_files".into(),
    })
    .unwrap();
    assert_eq!(named["type"], "tool");
    assert_eq!(named["name"], "read_files");
}

// ---- Stream event deserialization (samples taken verbatim from
//      Anthropic's docs at https://docs.anthropic.com/en/docs/build-with-claude/streaming) ----

#[test]
fn deserializes_message_start_event() {
    let s = r#"{"type":"message_start","message":{"id":"msg_01","role":"assistant","model":"claude-sonnet-4-6","usage":{"input_tokens":25,"output_tokens":1}}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::MessageStart { message } => {
            assert_eq!(message.id.as_deref(), Some("msg_01"));
            assert_eq!(message.model.as_deref(), Some("claude-sonnet-4-6"));
            let usage = message.usage.unwrap();
            assert_eq!(usage.input_tokens, 25);
            assert_eq!(usage.output_tokens, 1);
            assert_eq!(usage.cache_creation_input_tokens, 0);
            assert_eq!(usage.cache_read_input_tokens, 0);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_content_block_start_text() {
    let s = r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            assert_eq!(index, 0);
            assert!(matches!(content_block, StreamContentBlock::Text { .. }));
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_content_block_start_tool_use() {
    let s = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"read_files","input":{}}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            assert_eq!(index, 1);
            match content_block {
                StreamContentBlock::ToolUse { id, name, .. } => {
                    assert_eq!(id, "toolu_01");
                    assert_eq!(name, "read_files");
                }
                other => panic!("expected tool_use, got {other:?}"),
            }
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_text_delta() {
    let s =
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
            assert_eq!(index, 0);
            match delta {
                StreamContentDelta::TextDelta { text } => assert_eq!(text, "Hello"),
                other => panic!("expected text_delta, got {other:?}"),
            }
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_input_json_delta() {
    let s = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::ContentBlockDelta { delta, .. } => match delta {
            StreamContentDelta::InputJsonDelta { partial_json } => {
                assert_eq!(partial_json, "{\"path\":");
            }
            other => panic!("expected input_json_delta, got {other:?}"),
        },
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_thinking_delta() {
    let s = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::ContentBlockDelta { delta, .. } => assert!(matches!(
            delta,
            StreamContentDelta::ThinkingDelta { ref thinking } if thinking == "hmm"
        )),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_signature_delta_silently() {
    // Extended-thinking emits these; the decoder ignores them, but the
    // type must still deserialize (otherwise the stream errors out).
    let s = r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    assert!(matches!(
        ev,
        AnthropicStreamEvent::ContentBlockDelta {
            delta: StreamContentDelta::SignatureDelta { .. },
            ..
        }
    ));
}

#[test]
fn deserializes_content_block_stop() {
    let s = r#"{"type":"content_block_stop","index":2}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    assert!(matches!(
        ev,
        AnthropicStreamEvent::ContentBlockStop { index: 2 }
    ));
}

#[test]
fn deserializes_message_delta_with_usage() {
    let s = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::MessageDelta { delta, usage } => {
            assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
            assert_eq!(usage.unwrap().output_tokens, 15);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn deserializes_message_stop() {
    let s = r#"{"type":"message_stop"}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    assert!(matches!(ev, AnthropicStreamEvent::MessageStop));
}

#[test]
fn deserializes_ping() {
    let s = r#"{"type":"ping"}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    assert!(matches!(ev, AnthropicStreamEvent::Ping));
}

#[test]
fn deserializes_error_event() {
    let s = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let ev: AnthropicStreamEvent = serde_json::from_str(s).unwrap();
    match ev {
        AnthropicStreamEvent::Error { error } => {
            assert_eq!(error.kind, "overloaded_error");
            assert_eq!(error.message, "Overloaded");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

// ---- Non-streaming response ----

#[test]
fn deserializes_non_streaming_text_response() {
    let s = r#"{
        "id":"msg_01",
        "model":"claude-sonnet-4-6",
        "content":[{"type":"text","text":"Hi there."}],
        "stop_reason":"end_turn"
    }"#;
    let parsed: AnthropicMessageResponse = serde_json::from_str(s).unwrap();
    assert_eq!(parsed.id.as_deref(), Some("msg_01"));
    assert_eq!(parsed.content.len(), 1);
    assert!(matches!(
        &parsed.content[0],
        ResponseContentBlock::Text { text } if text == "Hi there."
    ));
    assert!(parsed.error.is_none());
}

#[test]
fn deserializes_non_streaming_error_envelope() {
    // 4xx body shape: {"type":"error","error":{...}}. We only read the
    // error field; the wrapping type is ignored.
    let s = r#"{
        "type":"error",
        "error":{"type":"invalid_request_error","message":"max_tokens is required"}
    }"#;
    let parsed: AnthropicMessageResponse = serde_json::from_str(s).unwrap();
    let err = parsed.error.expect("error present");
    assert_eq!(err.kind, "invalid_request_error");
    assert!(err.message.contains("max_tokens"));
}
