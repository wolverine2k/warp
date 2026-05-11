//! Tests for `compose_ollama_chat_request`. Sibling file per the repo's
//! unit-test layout convention (CLAUDE.md).

use std::collections::HashMap;

use warp_multi_agent_api as api;

use super::request::compose_ollama_chat_request;
use super::wire::*;
use crate::local_provider::compaction::{CompactionState, CompletedCompaction};
use crate::local_provider::config::LocalProviderConfig;
use crate::local_provider::request::LocalProviderInput;
use crate::local_provider::AgentProviderApiType;

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Ollama".into(),
        base_url: "http://localhost:11434".into(),
        model_id: "llama3.1".into(),
        api_key: None,
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::Ollama,
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

fn tool_call_msg(id: &str, tool_call_id: &str, tool: api::message::tool_call::Tool) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: tool_call_id.into(),
            tool: Some(tool),
        })),
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

// ---- baseline shape ----

#[test]
fn system_prompt_is_first_message_with_system_role() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_ollama_chat_request(&input, &cfg());
    assert!(req.messages.len() >= 2);
    assert_eq!(req.messages[0].role, OllamaRole::System);
    assert!(!req.messages[0].content.is_empty());
}

#[test]
fn simple_user_query_appended_after_system() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_ollama_chat_request(&input, &cfg());
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, OllamaRole::User);
    assert_eq!(req.messages[1].content, "hello");
}

#[test]
fn stream_is_always_true() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_ollama_chat_request(&input, &cfg());
    assert!(req.stream);
}

#[test]
fn model_is_user_model_id() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_ollama_chat_request(&input, &cfg());
    assert_eq!(req.model, "llama3.1");
}

// ---- options.num_ctx ----

#[test]
fn options_num_ctx_set_when_context_window_present() {
    let mut c = cfg();
    c.context_window = Some(128_000);
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_ollama_chat_request(&input, &c);
    let opts = req.options.expect("options present");
    assert_eq!(opts.num_ctx, Some(128_000));
}

#[test]
fn options_absent_when_context_window_unset() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_ollama_chat_request(&input, &cfg());
    assert!(req.options.is_none());
}

#[test]
fn options_absent_when_context_window_zero() {
    let mut c = cfg();
    c.context_window = Some(0);
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_ollama_chat_request(&input, &c);
    assert!(req.options.is_none());
}

// ---- tools ----

#[test]
fn tools_absent_when_supports_tools_false() {
    let mut c = cfg();
    c.supports_tools = false;
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_ollama_chat_request(&input, &c);
    assert!(req.tools.is_none());
}

#[test]
fn tools_absent_when_no_v1_tools_signaled() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::SearchCodebase, api::ToolType::CallMcpTool];
    let req = compose_ollama_chat_request(&input, &cfg());
    assert!(req.tools.is_none());
}

#[test]
fn tools_present_in_openai_style_envelope() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_ollama_chat_request(&input, &cfg());
    let tools = req.tools.as_ref().expect("tools present");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].kind, "function");
    assert_eq!(tools[0].function.name, "read_files");
    assert_eq!(tools[0].function.parameters["type"], "object");
}

// ---- history walking ----

#[test]
fn user_assistant_user_history_preserved() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            user_msg("m1", "first"),
            agent_msg("m2", "ok"),
            user_msg("m3", "second"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + 3 history messages
    assert_eq!(req.messages.len(), 4);
    assert_eq!(req.messages[0].role, OllamaRole::System);
    assert_eq!(req.messages[1].role, OllamaRole::User);
    assert_eq!(req.messages[1].content, "first");
    assert_eq!(req.messages[2].role, OllamaRole::Assistant);
    assert_eq!(req.messages[2].content, "ok");
    assert_eq!(req.messages[3].role, OllamaRole::User);
    assert_eq!(req.messages[3].content, "second");
}

#[test]
fn agent_reasoning_dropped_from_history() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::AgentReasoning(
                api::message::AgentReasoning {
                    reasoning: "hidden".into(),
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
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + the appended user_query — reasoning gone.
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, OllamaRole::User);
}

// ---- tool calls ----

#[test]
fn tool_call_history_becomes_assistant_message_with_object_arguments() {
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
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + assistant(tool_call) + backfilled placeholder tool result.
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[1].role, OllamaRole::Assistant);
    assert_eq!(req.messages[1].content, "");
    let tcs = req.messages[1].tool_calls.as_ref().expect("tool_calls present");
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].function.name, "read_files");
    // Arguments must be an object, not a string.
    assert!(tcs[0].function.arguments.is_object());
    assert_eq!(tcs[0].function.arguments["paths"][0], "Cargo.toml");
    assert_eq!(req.messages[2].role, OllamaRole::Tool);
    assert_eq!(req.messages[2].content, "(tool result not available)");
}

#[test]
fn outbound_tool_call_omits_id_and_type_fields() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![tool_call_msg(
            "m1",
            "call_alpha",
            read_files_tool(&["x"]),
        )],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_ollama_chat_request(&input, &cfg());
    let v = serde_json::to_value(&req).unwrap();
    let tool_call = &v["messages"][1]["tool_calls"][0];
    assert!(tool_call.get("id").is_none(), "tool_call should not have id");
    assert!(
        tool_call.get("type").is_none(),
        "tool_call should not have type field"
    );
    assert!(tool_call["function"]["arguments"].is_object());
}

#[test]
fn orphan_tool_call_uses_action_results_when_present() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![tool_call_msg(
            "m1",
            "call_alpha",
            read_files_tool(&["Cargo.toml"]),
        )],
        ..Default::default()
    };
    let mut action_results = HashMap::new();
    action_results.insert("call_alpha".into(), "[package]\nname = \"foo\"".into());
    let input = LocalProviderInput {
        tasks: vec![task],
        action_results,
        ..Default::default()
    };
    let req = compose_ollama_chat_request(&input, &cfg());
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[2].role, OllamaRole::Tool);
    assert_eq!(req.messages[2].content, "[package]\nname = \"foo\"");
}

#[test]
fn proto_tool_result_message_becomes_role_tool_message() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "call_beta", read_files_tool(&["x"])),
            api::Message {
                id: "m2".into(),
                message: Some(api::message::Message::ToolCallResult(
                    api::message::ToolCallResult {
                        tool_call_id: "call_beta".into(),
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
            },
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + assistant(tool_call) + tool(result) — no extra backfill
    // because the proto already supplied a matching ToolCallResult.
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[2].role, OllamaRole::Tool);
    assert!(req.messages[2].content.contains("--- x ---"));
    assert!(req.messages[2].content.contains("file x content"));
    // Wire shape on role:tool messages omits tool_call_id.
    let v = serde_json::to_value(&req).unwrap();
    let tool_msg = &v["messages"][2];
    assert!(tool_msg.get("tool_call_id").is_none());
    assert!(tool_msg.get("name").is_none());
    assert_eq!(tool_msg["role"], "tool");
}

#[test]
fn multi_turn_with_action_results_uses_real_content_per_turn() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m_t1_call", "call_alpha", read_files_tool(&["Cargo.toml"])),
            tool_call_msg(
                "m_t2_call",
                "call_beta",
                api::message::tool_call::Tool::Grep(api::message::tool_call::Grep {
                    queries: vec!["fn main".into()],
                    path: ".".into(),
                }),
            ),
        ],
        ..Default::default()
    };
    let mut action_results = HashMap::new();
    action_results.insert("call_alpha".into(), "Cargo.toml body".into());
    action_results.insert("call_beta".into(), "src/main.rs hits".into());
    let input = LocalProviderInput {
        user_query: Some("summarize".into()),
        tasks: vec![task],
        supported_tools: vec![api::ToolType::ReadFiles, api::ToolType::Grep],
        action_results,
        ..Default::default()
    };
    let req = compose_ollama_chat_request(&input, &cfg());
    // Expected: system + assistant(call_alpha) + tool(Cargo.toml body) +
    //           assistant(call_beta) + tool(main.rs hits) + user(summarize)
    assert_eq!(req.messages.len(), 6, "{:#?}", req.messages);
    assert_eq!(req.messages[0].role, OllamaRole::System);
    assert_eq!(req.messages[1].role, OllamaRole::Assistant);
    assert_eq!(req.messages[2].role, OllamaRole::Tool);
    assert_eq!(req.messages[2].content, "Cargo.toml body");
    assert_eq!(req.messages[3].role, OllamaRole::Assistant);
    assert_eq!(req.messages[4].role, OllamaRole::Tool);
    assert_eq!(req.messages[4].content, "src/main.rs hits");
    assert_eq!(req.messages[5].role, OllamaRole::User);
    assert_eq!(req.messages[5].content, "summarize");
}

// ---- compaction projection ----

#[test]
fn compaction_projection_synthesizes_head_and_drops_pre_tail_history() {
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
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + synthetic-user(continue) + synthetic-assistant(summary) +
    // tail-user(post-compact ask). Two pre-tail proto messages dropped.
    assert_eq!(req.messages.len(), 4, "{:#?}", req.messages);
    assert_eq!(req.messages[0].role, OllamaRole::System);
    assert_eq!(req.messages[1].role, OllamaRole::User);
    assert!(req.messages[1].content.contains("Continue"));
    assert_eq!(req.messages[2].role, OllamaRole::Assistant);
    assert_eq!(req.messages[2].content, "## Goal\n- summary");
    assert_eq!(req.messages[3].role, OllamaRole::User);
    assert_eq!(req.messages[3].content, "post-compact ask");
}

#[test]
fn compaction_projection_with_no_tail_start_id_drops_all_history() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![user_msg("u1", "hi"), agent_msg("a1", "hello")],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "trigger".into(),
        assistant_msg_id: "summary".into(),
        tail_start_id: None,
        summary_text: Some("manual digest".into()),
        auto: false,
        overflow: false,
    });
    let input = LocalProviderInput {
        tasks: vec![task],
        compaction_state: state,
        ..Default::default()
    };
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + synthetic-user(continue) + synthetic-assistant(summary).
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[2].content, "manual digest");
}

// ---- synthetic user query anchoring (Phase B-6 parity) ----

#[test]
fn synthetic_user_query_emitted_before_anchor_proto_message() {
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
    let req = compose_ollama_chat_request(&input, &cfg());
    // system + user(what is X?) + assistant(first answer)
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[1].role, OllamaRole::User);
    assert_eq!(req.messages[1].content, "what is X?");
    assert_eq!(req.messages[2].role, OllamaRole::Assistant);
    assert_eq!(req.messages[2].content, "first answer");
}

// ---- system content ----

#[test]
fn context_window_threads_into_system_prompt() {
    let mut c = cfg();
    c.context_window = Some(200_000);
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_ollama_chat_request(&input, &c);
    assert!(
        req.messages[0].content.contains("200000")
            || req.messages[0].content.contains("200_000")
    );
}
