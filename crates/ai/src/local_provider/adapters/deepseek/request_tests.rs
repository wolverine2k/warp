//! Tests for `compose_deepseek_chat_request`. Sibling file per the repo's
//! unit-test layout convention (CLAUDE.md).

use warp_multi_agent_api as api;

use super::request::compose_deepseek_chat_request;
use super::wire::*;
use crate::local_provider::compaction::{CompactionState, CompletedCompaction};
use crate::local_provider::config::LocalProviderConfig;
use crate::local_provider::request::LocalProviderInput;
use crate::local_provider::AgentProviderApiType;

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "DeepSeek".into(),
        base_url: "https://api.deepseek.com/v1".into(),
        model_id: "deepseek-chat".into(),
        api_key: None,
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::DeepSeek,
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
                                    content: "file content".into(),
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

// ---- 1: system prompt ----

#[test]
fn system_prompt_becomes_first_message_with_role_system() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_deepseek_chat_request(&input, &cfg());
    assert!(req.messages.len() >= 2);
    assert_eq!(req.messages[0].role, DeepSeekRole::System);
    assert!(req.messages[0].content.as_deref().map(|s| !s.is_empty()).unwrap_or(false));
}

// ---- 2: user query ----

#[test]
fn user_query_becomes_role_user_message() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_deepseek_chat_request(&input, &cfg());
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, DeepSeekRole::User);
    assert_eq!(req.messages[1].content.as_deref(), Some("hello"));
}

// ---- 3: agent output ----

#[test]
fn agent_output_becomes_role_assistant_message() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![agent_msg("m1", "I can help with that.")],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_deepseek_chat_request(&input, &cfg());
    // system + assistant
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, DeepSeekRole::Assistant);
    assert_eq!(
        req.messages[1].content.as_deref(),
        Some("I can help with that.")
    );
}

// ---- 4: AgentReasoning dropped ----

#[test]
fn agent_reasoning_is_dropped_from_outbound_history() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::AgentReasoning(
                api::message::AgentReasoning {
                    reasoning: "internal chain-of-thought".into(),
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
    let req = compose_deepseek_chat_request(&input, &cfg());
    // system + user_query; reasoning is gone
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, DeepSeekRole::User);
    // No message should carry reasoning_content — structurally impossible
    // because DeepSeekChatMessage has no reasoning_content field, but we
    // additionally verify no tool_calls or unexpected content appears.
    for msg in &req.messages {
        assert!(msg.tool_calls.is_none());
    }
}

// ---- 5: tool_call arguments are stringified JSON ----

#[test]
fn tool_call_proto_becomes_assistant_with_stringified_arguments() {
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
    let req = compose_deepseek_chat_request(&input, &cfg());
    // system + assistant(tool_call) + backfilled placeholder tool result
    assert_eq!(req.messages[1].role, DeepSeekRole::Assistant);
    let v = serde_json::to_value(&req).unwrap();
    // arguments must be a JSON string, not an object
    assert!(
        matches!(
            v["messages"][1]["tool_calls"][0]["function"]["arguments"],
            serde_json::Value::String(_)
        ),
        "arguments must serialize as a JSON string, got: {:?}",
        v["messages"][1]["tool_calls"][0]["function"]["arguments"]
    );
}

// ---- 6: tool_call_id round-trip ----

#[test]
fn tool_call_carries_tool_call_id_from_proto() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![tool_call_msg(
            "m1",
            "call_xyz_123",
            read_files_tool(&["src/main.rs"]),
        )],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_deepseek_chat_request(&input, &cfg());
    let tcs = req.messages[1]
        .tool_calls
        .as_ref()
        .expect("tool_calls present");
    assert_eq!(tcs[0].id, "call_xyz_123");
}

// ---- 7: tool result becomes role:tool with tool_call_id ----

#[test]
fn tool_result_becomes_role_tool_with_tool_call_id() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "call_abc", read_files_tool(&["x"])),
            tool_result_msg("m2", "call_abc"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_deepseek_chat_request(&input, &cfg());
    // system + assistant(tool_call) + tool(result)
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[2].role, DeepSeekRole::Tool);
    assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_abc"));
}

// ---- 8: tools envelope uses function type wrapper ----

#[test]
fn tools_envelope_uses_function_type_wrapper() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_deepseek_chat_request(&input, &cfg());
    let tools = req.tools.as_ref().expect("tools present");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].kind, "function");
    assert_eq!(tools[0].function.name, "read_files");
    assert_eq!(tools[0].function.parameters["type"], "object");
}

// ---- 9: tools omitted when supports_tools false ----

#[test]
fn tools_omitted_when_supports_tools_false() {
    let mut c = cfg();
    c.supports_tools = false;
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_deepseek_chat_request(&input, &c);
    assert!(req.tools.is_none());
}

// ---- 10: tools omitted when enabled tools empty ----

#[test]
fn tools_omitted_when_enabled_tools_empty() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    // Only non-v1 tools signaled → enabled list is empty
    input.supported_tools = vec![api::ToolType::SearchCodebase, api::ToolType::CallMcpTool];
    let req = compose_deepseek_chat_request(&input, &cfg());
    assert!(req.tools.is_none());
}

// ---- 11: stream is always true ----

#[test]
fn stream_is_always_true() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_deepseek_chat_request(&input, &cfg());
    assert!(req.stream);
}

// ---- 12: compaction projection ----

#[test]
fn compaction_projection_synthesizes_user_assistant_summary_pair() {
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
    let req = compose_deepseek_chat_request(&input, &cfg());
    // system + synthetic-user(continue) + synthetic-assistant(summary) +
    // tail-user(post-compact ask). Two pre-tail proto messages dropped.
    assert_eq!(req.messages.len(), 4, "{:#?}", req.messages);
    assert_eq!(req.messages[0].role, DeepSeekRole::System);
    assert_eq!(req.messages[1].role, DeepSeekRole::User);
    assert!(req.messages[1]
        .content
        .as_deref()
        .unwrap()
        .contains("Continue"));
    assert_eq!(req.messages[2].role, DeepSeekRole::Assistant);
    assert_eq!(
        req.messages[2].content.as_deref(),
        Some("## Goal\n- summary")
    );
    assert_eq!(req.messages[3].role, DeepSeekRole::User);
    assert_eq!(req.messages[3].content.as_deref(), Some("post-compact ask"));
}

// ---- 13: synthetic user query anchoring ----

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
    let req = compose_deepseek_chat_request(&input, &cfg());
    // system + user(what is X?) + assistant(first answer)
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[1].role, DeepSeekRole::User);
    assert_eq!(req.messages[1].content.as_deref(), Some("what is X?"));
    assert_eq!(req.messages[2].role, DeepSeekRole::Assistant);
    assert_eq!(req.messages[2].content.as_deref(), Some("first answer"));
}

// ---- 14: multi-turn round-trip ----

#[test]
fn multi_turn_round_trip_with_text_and_tool_call() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            user_msg("m1", "read Cargo.toml"),
            tool_call_msg("m2", "call_alpha", read_files_tool(&["Cargo.toml"])),
            tool_result_msg("m3", "call_alpha"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        user_query: Some("summarize".into()),
        tasks: vec![task],
        supported_tools: vec![api::ToolType::ReadFiles],
        ..Default::default()
    };
    let req = compose_deepseek_chat_request(&input, &cfg());
    // Verify it serializes cleanly to JSON with no errors.
    let body = serde_json::to_string(&req).expect("serializes cleanly");
    let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    let msgs = v["messages"].as_array().expect("messages array");
    // system + user + assistant(tool_call) + tool(result) + user(summarize)
    assert_eq!(msgs.len(), 5, "unexpected message count: {:#?}", msgs);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(msgs[2]["role"], "assistant");
    assert_eq!(msgs[3]["role"], "tool");
    assert_eq!(msgs[3]["tool_call_id"], "call_alpha");
    assert_eq!(msgs[4]["role"], "user");
    assert_eq!(msgs[4]["content"], "summarize");
}

// ---- 15: model_id threads from cfg ----

#[test]
fn model_id_threads_from_cfg() {
    let mut c = cfg();
    c.model_id = "deepseek-reasoner".into();
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_deepseek_chat_request(&input, &c);
    assert_eq!(req.model, "deepseek-reasoner");
}

// ---- 16.5: orphan tool_call with action_results populated ----

#[test]
fn orphan_tool_call_uses_action_results_when_present_and_carries_tool_call_id() {
    // When the proto history has an assistant ToolCall without a matching
    // ToolCallResult, backfill_orphaned_tool_calls injects a synthetic
    // role:"tool" message. If `action_results` carries a value for that
    // tool_call_id, use it as the content (instead of the
    // "(tool result not available)" placeholder).
    //
    // DeepSeek-specific check: the backfilled role:"tool" message MUST
    // carry tool_call_id on the wire — Ollama strips it; DeepSeek requires
    // it. This test guards both behaviors.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "call_xyz", read_files_tool(&["x"])),
            // NOTE: no matching ToolCallResult — orphaned.
        ],
        ..Default::default()
    };
    let mut action_results = std::collections::HashMap::new();
    action_results.insert(
        "call_xyz".to_string(),
        "(action result content from controller)".to_string(),
    );
    let input = LocalProviderInput {
        user_query: Some("follow-up".into()),
        tasks: vec![task],
        action_results,
        ..Default::default()
    };

    let req = compose_deepseek_chat_request(&input, &cfg());

    // Walk messages and find the synthetic role:"tool" backfill.
    let tool_msg = req
        .messages
        .iter()
        .find(|m| m.role == DeepSeekRole::Tool)
        .expect("backfilled role:tool message should exist");
    assert_eq!(
        tool_msg.content.as_deref(),
        Some("(action result content from controller)"),
        "action_results content should be used, not the placeholder"
    );
    assert_eq!(
        tool_msg.tool_call_id.as_deref(),
        Some("call_xyz"),
        "backfilled tool message must carry tool_call_id on the wire (DeepSeek requirement)"
    );

    // Serialization check: tool_call_id is present in the JSON body.
    let body = serde_json::to_string(&req).unwrap();
    assert!(
        body.contains("\"tool_call_id\":\"call_xyz\""),
        "tool_call_id should serialize on the backfilled role:tool message; body = {body}"
    );
}

// ---- 16: paranoia test — reasoning_content never in serialized body ----

#[test]
fn reasoning_content_never_appears_in_serialized_body() {
    // Build an input with both AgentReasoning and AgentOutput proto messages.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            api::Message {
                id: "m1".into(),
                message: Some(api::message::Message::AgentReasoning(
                    api::message::AgentReasoning {
                        reasoning: "chain-of-thought: the answer is 42".into(),
                        finished_duration: None,
                    },
                )),
                ..Default::default()
            },
            agent_msg("m2", "The answer is 42."),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        user_query: Some("what is the answer?".into()),
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_deepseek_chat_request(&input, &cfg());
    let body = serde_json::to_string(&req).unwrap();
    // Spec-mandated regression guard: DeepSeek returns HTTP 400 when
    // reasoning_content appears on inbound messages.
    assert!(
        !body.contains("reasoning_content"),
        "reasoning_content must never appear in serialized request body, got: {body}"
    );
}
