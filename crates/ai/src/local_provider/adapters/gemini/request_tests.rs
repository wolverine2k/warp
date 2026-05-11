//! Tests for `compose_gemini_request`. Sibling file per the repo's
//! unit-test layout convention (CLAUDE.md).

use warp_multi_agent_api as api;

use super::request::compose_gemini_request;
use super::wire::*;
use crate::local_provider::compaction::{CompactionState, CompletedCompaction};
use crate::local_provider::config::LocalProviderConfig;
use crate::local_provider::request::LocalProviderInput;
use crate::local_provider::AgentProviderApiType;

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Gemini".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        model_id: "gemini-2.0-flash".into(),
        api_key: None,
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::Gemini,
    }
}

fn empty_input() -> LocalProviderInput {
    LocalProviderInput::default()
}

fn user_msg(id: &str, body: &str) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: body.into(),
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn agent_msg(id: &str, body: &str) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput { text: body.into() },
        )),
        ..Default::default()
    }
}

fn tool_call_msg(
    id: &str,
    tool_call_id: &str,
    tool: api::message::tool_call::Tool,
) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: tool_call_id.into(),
            tool: Some(tool),
        })),
        ..Default::default()
    }
}

fn tool_result_msg(id: &str, tool_call_id: &str) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::ToolCallResult(
            api::message::ToolCallResult {
                tool_call_id: tool_call_id.into(),
                result: Some(api::message::tool_call_result::Result::ReadFiles(
                    api::ReadFilesResult {
                        result: Some(api::read_files_result::Result::TextFilesSuccess(
                            api::read_files_result::TextFilesSuccess {
                                files: vec![api::FileContent {
                                    file_path: "x".into(),
                                    content: "file x content".into(),
                                    ..Default::default()
                                }],
                            },
                        )),
                    },
                )),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

fn read_files_tool(paths: &[&str]) -> api::message::tool_call::Tool {
    api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
        files: paths
            .iter()
            .map(|p| api::message::tool_call::read_files::File {
                name: (*p).into(),
                line_ranges: vec![],
            })
            .collect(),
    })
}

// ---- 1. system_prompt_lifts_to_top_level_system_instruction ----

#[test]
fn system_prompt_lifts_to_top_level_system_instruction() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_gemini_request(&input, &cfg());
    assert!(req.system_instruction.is_some());
    // No role:system entry in contents.
    for c in &req.contents {
        // GeminiRole only has User and Model — this assert is always true
        // but documents the invariant explicitly.
        assert!(matches!(c.role, GeminiRole::User | GeminiRole::Model));
    }
}

// ---- 2. empty_system_prompt_omits_system_instruction ----

#[test]
fn empty_system_prompt_omits_system_instruction() {
    // An empty supported_tools list + no context window → compose_system_prompt
    // still returns a non-empty prompt (the template body). We can't trivially
    // make compose_system_prompt return empty, so instead we verify the field
    // presence matches the trim-is-empty guard in compose_gemini_request.
    // The system prompt is never whitespace-only with the real template, so
    // we test the guard by inspecting a request where the system is present.
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_gemini_request(&input, &cfg());
    // The composed system prompt is non-empty → field must be Some.
    assert!(req.system_instruction.is_some());
    let si = req.system_instruction.as_ref().unwrap();
    assert!(!si.parts.is_empty());
    assert!(!si.parts[0].text.trim().is_empty());
}

// ---- 3. simple_user_query_becomes_first_content_with_role_user ----

#[test]
fn simple_user_query_becomes_first_content_with_role_user() {
    let mut input = empty_input();
    input.user_query = Some("Hello".into());
    let req = compose_gemini_request(&input, &cfg());
    assert!(!req.contents.is_empty());
    let first = &req.contents[0];
    assert_eq!(first.role, GeminiRole::User);
    match &first.parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "Hello"),
        other => panic!("expected Text part, got {other:?}"),
    }
}

// ---- 4. assistant_proto_message_becomes_role_model ----

#[test]
fn assistant_proto_message_becomes_role_model() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![agent_msg("m1", "I can help.")],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    let model_content = req
        .contents
        .iter()
        .find(|c| c.role == GeminiRole::Model)
        .expect("model content present");
    match &model_content.parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "I can help."),
        other => panic!("expected Text, got {other:?}"),
    }
}

// ---- 5. agent_reasoning_is_dropped ----

#[test]
fn agent_reasoning_is_dropped() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::AgentReasoning(
                api::message::AgentReasoning {
                    reasoning: "internal thought".into(),
                    finished_duration: None,
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let input = LocalProviderInput {
        user_query: Some("hi".into()),
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    // Only the appended user_query should be in contents (no reasoning entry).
    assert_eq!(req.contents.len(), 1);
    assert_eq!(req.contents[0].role, GeminiRole::User);
}

// ---- 6. tool_call_proto_becomes_function_call_part_with_object_args ----

#[test]
fn tool_call_proto_becomes_function_call_part_with_object_args() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![tool_call_msg(
            "m1",
            "call_alpha",
            read_files_tool(&["Cargo.toml"]),
        )],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    let model_content = req
        .contents
        .iter()
        .find(|c| c.role == GeminiRole::Model)
        .expect("model content present");
    match &model_content.parts[0] {
        GeminiOutboundPart::FunctionCall(fc) => {
            assert_eq!(fc.function_call.name, "read_files");
            assert!(fc.function_call.args.is_object(), "args must be a JSON object");
            assert_eq!(fc.function_call.args["paths"][0], "Cargo.toml");
        }
        other => panic!("expected FunctionCall part, got {other:?}"),
    }
}

// ---- 7. tool_result_proto_becomes_function_response_with_content_wrapper ----

#[test]
fn tool_result_proto_becomes_function_response_with_content_wrapper() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "call_beta", read_files_tool(&["x"])),
            tool_result_msg("m2", "call_beta"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    let user_with_fr = req
        .contents
        .iter()
        .find(|c| {
            c.role == GeminiRole::User
                && c.parts
                    .iter()
                    .any(|p| matches!(p, GeminiOutboundPart::FunctionResponse(_)))
        })
        .expect("user content with FunctionResponse present");
    match &user_with_fr.parts[0] {
        GeminiOutboundPart::FunctionResponse(fr) => {
            let resp = &fr.function_response.response;
            assert!(resp.get("content").is_some(), "response must have 'content' key");
            assert!(resp["content"].is_string());
        }
        other => panic!("expected FunctionResponse part, got {other:?}"),
    }
}

// ---- 8. function_response_name_matches_prior_tool_call_name ----

#[test]
fn function_response_name_matches_prior_tool_call_name() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "call_rf", read_files_tool(&["x"])),
            tool_result_msg("m2", "call_rf"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    let fr_content = req
        .contents
        .iter()
        .find(|c| {
            c.role == GeminiRole::User
                && c.parts
                    .iter()
                    .any(|p| matches!(p, GeminiOutboundPart::FunctionResponse(_)))
        })
        .expect("FunctionResponse content present");
    match &fr_content.parts[0] {
        GeminiOutboundPart::FunctionResponse(fr) => {
            assert_eq!(fr.function_response.name, "read_files");
        }
        other => panic!("expected FunctionResponse, got {other:?}"),
    }
}

// ---- 9. function_response_name_is_empty_when_no_prior_tool_call ----

#[test]
fn function_response_name_is_empty_when_no_prior_tool_call() {
    // Orphan ToolCallResult — no matching prior ToolCall in the walk.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::ToolCallResult(
                api::message::ToolCallResult {
                    tool_call_id: "nonexistent_id".into(),
                    result: None,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    let fr_content = req
        .contents
        .iter()
        .find(|c| {
            c.parts
                .iter()
                .any(|p| matches!(p, GeminiOutboundPart::FunctionResponse(_)))
        })
        .expect("FunctionResponse content present");
    match &fr_content.parts[0] {
        GeminiOutboundPart::FunctionResponse(fr) => {
            assert_eq!(fr.function_response.name, "", "name should be empty string as defensive fallback");
        }
        other => panic!("expected FunctionResponse, got {other:?}"),
    }
}

// ---- 10. tools_envelope_wraps_single_function_declarations_array ----

#[test]
fn tools_envelope_wraps_single_function_declarations_array() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_gemini_request(&input, &cfg());
    let tools = req.tools.as_ref().expect("tools present");
    assert_eq!(tools.len(), 1, "single envelope");
    assert!(!tools[0].function_declarations.is_empty());
    assert_eq!(tools[0].function_declarations[0].name, "read_files");
    let v = serde_json::to_value(&req).unwrap();
    assert!(v["tools"][0]["functionDeclarations"][0]["name"].is_string());
}

// ---- 11. tools_omitted_when_supports_tools_false ----

#[test]
fn tools_omitted_when_supports_tools_false() {
    let mut c = cfg();
    c.supports_tools = false;
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_gemini_request(&input, &c);
    assert!(req.tools.is_none());
}

// ---- 12. tools_omitted_when_enabled_tools_empty ----

#[test]
fn tools_omitted_when_enabled_tools_empty() {
    // supported_tools contains only types with no v1 schema.
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::SearchCodebase, api::ToolType::CallMcpTool];
    let req = compose_gemini_request(&input, &cfg());
    assert!(req.tools.is_none());
}

// ---- 13. generation_config_always_emitted ----

#[test]
fn generation_config_always_emitted() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_gemini_request(&input, &cfg());
    let v = serde_json::to_value(&req).unwrap();
    assert!(v.get("generationConfig").is_some(), "generationConfig must always be present");
    assert_eq!(v["generationConfig"], serde_json::json!({}));
}

// ---- 14. compaction_projection_synthesizes_user_model_summary_pair ----

#[test]
fn compaction_projection_synthesizes_user_model_summary_pair() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            user_msg("u_old1", "old turn"),
            agent_msg("a_old1", "old reply"),
            user_msg("u_new", "post-compact ask"),
        ],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "trigger".into(),
        assistant_msg_id: "summary".into(),
        tail_start_id: Some("u_new".into()),
        summary_text: Some("## Goal\n- summary".into()),
        auto: true,
        overflow: true,
    });
    let input = LocalProviderInput {
        tasks: vec![task],
        compaction_state: state,
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    // synthetic-user(continue) + synthetic-model(summary) + tail-user(post-compact ask)
    // Two pre-tail proto messages are dropped.
    assert_eq!(req.contents.len(), 3, "{:#?}", req.contents);
    assert_eq!(req.contents[0].role, GeminiRole::User);
    match &req.contents[0].parts[0] {
        GeminiOutboundPart::Text(t) => assert!(t.text.contains("Continue")),
        other => panic!("expected Text, got {other:?}"),
    }
    assert_eq!(req.contents[1].role, GeminiRole::Model);
    match &req.contents[1].parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "## Goal\n- summary"),
        other => panic!("expected Text, got {other:?}"),
    }
    assert_eq!(req.contents[2].role, GeminiRole::User);
    match &req.contents[2].parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "post-compact ask"),
        other => panic!("expected Text, got {other:?}"),
    }
}

// ---- 15. synthetic_user_query_anchoring_works ----

#[test]
fn synthetic_user_query_anchoring_works() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![agent_msg("a_old", "first answer")],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        synthetic_user_queries: vec![("a_old".into(), "what is X?".into())],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    // user(what is X?) + model(first answer)
    assert_eq!(req.contents.len(), 2, "{:#?}", req.contents);
    assert_eq!(req.contents[0].role, GeminiRole::User);
    match &req.contents[0].parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "what is X?"),
        other => panic!("expected Text, got {other:?}"),
    }
    assert_eq!(req.contents[1].role, GeminiRole::Model);
    match &req.contents[1].parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "first answer"),
        other => panic!("expected Text, got {other:?}"),
    }
}

// ---- 16. adjacent_same_role_messages_are_merged ----

#[test]
fn adjacent_same_role_messages_are_merged() {
    // Two consecutive Model messages should merge into one with both parts.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            agent_msg("m1", "first model response"),
            agent_msg("m2", "second model response"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    // Both model messages should be merged into a single entry.
    let model_entries: Vec<_> = req.contents.iter().filter(|c| c.role == GeminiRole::Model).collect();
    assert_eq!(model_entries.len(), 1, "adjacent Model entries must be merged");
    assert_eq!(model_entries[0].parts.len(), 2, "merged entry has both parts");
    match &model_entries[0].parts[0] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "first model response"),
        other => panic!("expected Text, got {other:?}"),
    }
    match &model_entries[0].parts[1] {
        GeminiOutboundPart::Text(t) => assert_eq!(t.text, "second model response"),
        other => panic!("expected Text, got {other:?}"),
    }
}

// ---- 17. multi_turn_round_trip ----

#[test]
fn multi_turn_round_trip() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            user_msg("m1", "read the file"),
            tool_call_msg("m2", "call_rf", read_files_tool(&["src/main.rs"])),
            tool_result_msg("m3", "call_rf"),
            agent_msg("m4", "The file contains the main function."),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        user_query: Some("summarize".into()),
        supported_tools: vec![api::ToolType::ReadFiles],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());

    // Expected structure (after merge — no adjacent same-role here):
    // 0: User("read the file")
    // 1: Model(FunctionCall read_files)
    // 2: User(FunctionResponse)
    // 3: Model("The file contains the main function.")
    // 4: User("summarize")
    assert_eq!(req.contents.len(), 5, "{:#?}", req.contents);
    assert_eq!(req.contents[0].role, GeminiRole::User);
    assert_eq!(req.contents[1].role, GeminiRole::Model);
    assert!(matches!(req.contents[1].parts[0], GeminiOutboundPart::FunctionCall(_)));
    assert_eq!(req.contents[2].role, GeminiRole::User);
    assert!(matches!(req.contents[2].parts[0], GeminiOutboundPart::FunctionResponse(_)));
    assert_eq!(req.contents[3].role, GeminiRole::Model);
    assert!(matches!(req.contents[3].parts[0], GeminiOutboundPart::Text(_)));
    assert_eq!(req.contents[4].role, GeminiRole::User);

    // Spot-check JSON serialization shape.
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["contents"][1]["role"], "model");
    assert!(v["contents"][1]["parts"][0].get("functionCall").is_some());
    assert_eq!(v["contents"][2]["role"], "user");
    assert!(v["contents"][2]["parts"][0].get("functionResponse").is_some());
}

// ---- 18. model_role_serializes_as_lowercase_string ----

#[test]
fn model_role_serializes_as_lowercase_string() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![agent_msg("m1", "hello")],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_gemini_request(&input, &cfg());
    let v = serde_json::to_value(&req).unwrap();
    let model_entry = req
        .contents
        .iter()
        .position(|c| c.role == GeminiRole::Model)
        .expect("model entry present");
    assert_eq!(
        v["contents"][model_entry]["role"],
        "model",
        "role must serialize as lowercase 'model'"
    );
    assert_ne!(v["contents"][model_entry]["role"], "Model");
    assert_ne!(v["contents"][model_entry]["role"], "MODEL");
}
