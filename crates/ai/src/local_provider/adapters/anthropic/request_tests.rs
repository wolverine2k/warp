//! Tests for `compose_anthropic_messages_request`. Sibling file rather
//! than inline `#[cfg(test)] mod tests` per the repo's unit-test layout
//! convention (CLAUDE.md).

use std::collections::HashMap;

use warp_multi_agent_api as api;

use super::request::{compose_anthropic_messages_request, resolve_max_tokens};
use super::wire::*;
use crate::local_provider::compaction::{CompactionState, CompletedCompaction};
use crate::local_provider::config::LocalProviderConfig;
use crate::local_provider::request::LocalProviderInput;
use crate::local_provider::AgentProviderApiType;

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Anthropic".into(),
        base_url: "https://api.anthropic.com".into(),
        model_id: "claude-sonnet-4-6".into(),
        api_key: Some("sk-ant-test".into()),
        supports_tools: true,
        context_window: None,
        api_type: AgentProviderApiType::Anthropic,
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

// ---- system prompt ----

#[test]
fn system_prompt_lives_in_top_level_field_not_messages() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert!(req.system.is_some(), "system field must be set");
    assert!(!req.system.as_deref().unwrap().is_empty());
    // Roles in `messages` are only `user` or `assistant` per the enum;
    // verify by serializing — Anthropic rejects `role:"system"` entries.
    let v = serde_json::to_value(&req).unwrap();
    for m in v["messages"].as_array().unwrap() {
        let role = m["role"].as_str().unwrap();
        assert!(
            role == "user" || role == "assistant",
            "unexpected role: {role}"
        );
    }
}

#[test]
fn context_window_threads_into_system_prompt() {
    let mut c = cfg();
    c.context_window = Some(200_000);
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_anthropic_messages_request(&input, &c);
    let sys = req.system.unwrap();
    assert!(sys.contains("200000") || sys.contains("200_000"));
}

// ---- simple shapes ----

#[test]
fn simple_user_query_yields_single_user_message_with_text_block() {
    let mut input = empty_input();
    input.user_query = Some("hello".into());
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, AnthropicRole::User);
    assert_eq!(req.messages[0].content.len(), 1);
    match &req.messages[0].content[0] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "hello"),
        other => panic!("expected text block, got {other:?}"),
    }
}

#[test]
fn stream_is_always_true() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert!(req.stream);
}

#[test]
fn alternating_user_assistant_user_preserved_through_history() {
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
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[0].role, AnthropicRole::User);
    assert_eq!(req.messages[1].role, AnthropicRole::Assistant);
    assert_eq!(req.messages[2].role, AnthropicRole::User);
}

// ---- agent_reasoning dropped ----

#[test]
fn agent_reasoning_messages_dropped_from_history() {
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
    let req = compose_anthropic_messages_request(&input, &cfg());
    // Just the appended user_query — reasoning is dropped.
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, AnthropicRole::User);
}

// ---- tool_use blocks ----

#[test]
fn tool_call_history_becomes_tool_use_content_block_on_assistant() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![tool_call_msg(
            "m1",
            "toolu_alpha",
            read_files_tool(&["Cargo.toml"]),
        )],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_anthropic_messages_request(&input, &cfg());
    // Assistant tool_use + backfilled user tool_result (no action_results provided
    // → placeholder).
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[0].role, AnthropicRole::Assistant);
    match &req.messages[0].content[0] {
        AnthropicContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "toolu_alpha");
            assert_eq!(name, "read_files");
            assert_eq!(input["paths"][0], "Cargo.toml");
        }
        other => panic!("expected tool_use, got {other:?}"),
    }
    assert_eq!(req.messages[1].role, AnthropicRole::User);
    match &req.messages[1].content[0] {
        AnthropicContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_alpha");
            assert_eq!(content, "(tool result not available)");
        }
        other => panic!("expected tool_result, got {other:?}"),
    }
}

#[test]
fn orphan_tool_use_uses_action_results_when_present() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![tool_call_msg(
            "m1",
            "toolu_alpha",
            read_files_tool(&["Cargo.toml"]),
        )],
        ..Default::default()
    };
    let mut action_results = HashMap::new();
    action_results.insert("toolu_alpha".into(), "[package]\nname = \"foo\"".into());
    let input = LocalProviderInput {
        tasks: vec![task],
        action_results,
        ..Default::default()
    };
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert_eq!(req.messages.len(), 2);
    match &req.messages[1].content[0] {
        AnthropicContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_alpha");
            assert_eq!(content, "[package]\nname = \"foo\"");
        }
        other => panic!("expected tool_result, got {other:?}"),
    }
}

#[test]
fn tool_result_proto_message_becomes_user_tool_result_block() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "toolu_beta", read_files_tool(&["x"])),
            api::Message {
                id: "m2".into(),
                message: Some(api::message::Message::ToolCallResult(
                    api::message::ToolCallResult {
                        tool_call_id: "toolu_beta".into(),
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
    let req = compose_anthropic_messages_request(&input, &cfg());
    // [assistant: tool_use, user: tool_result(real)]
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, AnthropicRole::User);
    match &req.messages[1].content[0] {
        AnthropicContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_beta");
            assert!(content.contains("--- x ---"));
            assert!(content.contains("file x content"));
        }
        other => panic!("expected tool_result, got {other:?}"),
    }
}

// ---- merging adjacent same-role ----

#[test]
fn adjacent_same_role_entries_merge_into_single_message() {
    // tool_result + user_query should merge: both produce role:User entries.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m1", "toolu_x", read_files_tool(&["a"])),
            api::Message {
                id: "m2".into(),
                message: Some(api::message::Message::ToolCallResult(
                    api::message::ToolCallResult {
                        tool_call_id: "toolu_x".into(),
                        result: Some(api::message::tool_call_result::Result::ReadFiles(
                            api::ReadFilesResult {
                                result: Some(api::read_files_result::Result::TextFilesSuccess(
                                    api::read_files_result::TextFilesSuccess { files: vec![] },
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
        user_query: Some("now what?".into()),
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_anthropic_messages_request(&input, &cfg());
    // [assistant: tool_use, user: tool_result + text("now what?")]
    assert_eq!(req.messages.len(), 2);
    let user = &req.messages[1];
    assert_eq!(user.role, AnthropicRole::User);
    assert_eq!(user.content.len(), 2);
    assert!(matches!(
        user.content[0],
        AnthropicContentBlock::ToolResult { .. }
    ));
    match &user.content[1] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "now what?"),
        other => panic!("expected text, got {other:?}"),
    }
}

// ---- max_tokens resolution ----

#[test]
fn max_tokens_defaults_to_4096_when_context_window_unset() {
    let c = cfg();
    assert_eq!(resolve_max_tokens(&c), 4096);
}

#[test]
fn max_tokens_defaults_to_4096_when_context_window_below_8k() {
    let mut c = cfg();
    c.context_window = Some(4096);
    assert_eq!(resolve_max_tokens(&c), 4096);
    c.context_window = Some(8191);
    assert_eq!(resolve_max_tokens(&c), 4096);
}

#[test]
fn max_tokens_is_quarter_of_window_above_8k() {
    let mut c = cfg();
    c.context_window = Some(8192);
    assert_eq!(resolve_max_tokens(&c), 2048);
    c.context_window = Some(32_768);
    assert_eq!(resolve_max_tokens(&c), 8192);
}

#[test]
fn max_tokens_capped_at_8192_for_huge_windows() {
    let mut c = cfg();
    c.context_window = Some(200_000);
    assert_eq!(resolve_max_tokens(&c), 8192);
    c.context_window = Some(1_000_000);
    assert_eq!(resolve_max_tokens(&c), 8192);
}

// ---- tools advertisement ----

#[test]
fn tools_absent_when_supports_tools_false() {
    let mut c = cfg();
    c.supports_tools = false;
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_anthropic_messages_request(&input, &c);
    assert!(req.tools.is_none());
    assert!(req.tool_choice.is_none());
}

#[test]
fn tools_absent_when_no_v1_tools_signaled() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::SearchCodebase, api::ToolType::CallMcpTool];
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert!(req.tools.is_none());
}

#[test]
fn tools_present_in_anthropic_shape_when_supported() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_anthropic_messages_request(&input, &cfg());
    let tools = req.tools.as_ref().expect("tools present");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "read_files");
    // No `function: {...}` wrapper — verify via serde.
    let v = serde_json::to_value(&tools[0]).unwrap();
    assert_eq!(v["name"], "read_files");
    assert!(v.get("function").is_none());
    assert!(v.get("type").is_none());
    assert!(v.get("input_schema").is_some());
    assert!(matches!(req.tool_choice, Some(AnthropicToolChoice::Auto)));
}

// ---- compaction projection ----

#[test]
fn compaction_projection_synthesizes_head_and_drops_pre_tail_history() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            user_msg("u_old1", "old turn 1"),
            agent_msg("a_old1", "old reply 1"),
            user_msg("u_new", "post-compact ask"),
        ],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "compaction-trigger".into(),
        assistant_msg_id: "compaction-summary".into(),
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
    let req = compose_anthropic_messages_request(&input, &cfg());

    // Expected after merge: [user(continue), assistant(summary), user(post-compact ask)]
    // The two pre-tail proto messages are dropped.
    assert_eq!(req.messages.len(), 3, "msgs: {:#?}", req.messages);
    assert_eq!(req.messages[0].role, AnthropicRole::User);
    match &req.messages[0].content[0] {
        AnthropicContentBlock::Text { text } => assert!(text.contains("Continue")),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(req.messages[1].role, AnthropicRole::Assistant);
    match &req.messages[1].content[0] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "## Goal\n- summary"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(req.messages[2].role, AnthropicRole::User);
    match &req.messages[2].content[0] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "post-compact ask"),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn compaction_projection_drops_all_history_when_tail_start_id_is_none() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![user_msg("u1", "hi"), agent_msg("a1", "hello")],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "ct".into(),
        assistant_msg_id: "cs".into(),
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
    let req = compose_anthropic_messages_request(&input, &cfg());
    // [user(continue), assistant(digest)] only — proto history dropped.
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[1].role, AnthropicRole::Assistant);
    match &req.messages[1].content[0] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "manual digest"),
        other => panic!("expected text, got {other:?}"),
    }
}

// ---- synthetic user-query anchoring (Phase B-6 parity) ----

#[test]
fn synthetic_user_query_anchored_before_proto_anchor_message() {
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
    let req = compose_anthropic_messages_request(&input, &cfg());
    // [user("what is X?"), assistant("first answer")] — synthetic user is
    // emitted before the assistant anchor message.
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[0].role, AnthropicRole::User);
    match &req.messages[0].content[0] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "what is X?"),
        other => panic!("{other:?}"),
    }
    assert_eq!(req.messages[1].role, AnthropicRole::Assistant);
}

// ---- multi-turn round-trip ----

#[test]
fn full_multi_turn_loop_round_trip() {
    // Turn 1 user asks; assistant emits tool_call A.
    // Turn 2 controller threads result; assistant emits tool_call B.
    // Turn 3 controller threads result; user asks "summarize".
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            tool_call_msg("m_t1_call", "toolu_a", read_files_tool(&["Cargo.toml"])),
            tool_call_msg(
                "m_t2_call",
                "toolu_b",
                api::message::tool_call::Tool::Grep(api::message::tool_call::Grep {
                    queries: vec!["fn main".into()],
                    path: ".".into(),
                }),
            ),
        ],
        ..Default::default()
    };
    let mut action_results = HashMap::new();
    action_results.insert("toolu_a".into(), "Cargo.toml body".into());
    action_results.insert("toolu_b".into(), "src/main.rs hits".into());

    let input = LocalProviderInput {
        user_query: Some("summarize".into()),
        tasks: vec![task],
        supported_tools: vec![api::ToolType::ReadFiles, api::ToolType::Grep],
        action_results,
        synthetic_user_queries: vec![("m_t1_call".into(), "read Cargo.toml".into())],
        ..Default::default()
    };
    let req = compose_anthropic_messages_request(&input, &cfg());

    // Expected sequence after walk + merge + backfill:
    //   0 user:        text("read Cargo.toml")
    //   1 assistant:   tool_use(toolu_a)
    //   2 user:        tool_result(toolu_a "Cargo.toml body")
    //   3 assistant:   tool_use(toolu_b)
    //   4 user:        tool_result(toolu_b "src/main.rs hits") + text("summarize")
    assert_eq!(req.messages.len(), 5, "msgs: {:#?}", req.messages);
    assert_eq!(req.messages[0].role, AnthropicRole::User);
    assert_eq!(req.messages[1].role, AnthropicRole::Assistant);
    assert_eq!(req.messages[2].role, AnthropicRole::User);
    assert_eq!(req.messages[3].role, AnthropicRole::Assistant);
    assert_eq!(req.messages[4].role, AnthropicRole::User);

    // tool_result content uses the real action_results value, not placeholder.
    match &req.messages[2].content[0] {
        AnthropicContentBlock::ToolResult { content, .. } => {
            assert_eq!(content, "Cargo.toml body");
        }
        other => panic!("{other:?}"),
    }
    match &req.messages[4].content[0] {
        AnthropicContentBlock::ToolResult { content, .. } => {
            assert_eq!(content, "src/main.rs hits");
        }
        other => panic!("{other:?}"),
    }
    match &req.messages[4].content[1] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "summarize"),
        other => panic!("{other:?}"),
    }
    // Tools advertised in Anthropic shape on every multi-turn body.
    let tools = req.tools.expect("tools present");
    assert!(!tools.is_empty());
}

// ---- model field ----

#[test]
fn model_field_is_user_model_id() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_anthropic_messages_request(&input, &cfg());
    assert_eq!(req.model, "claude-sonnet-4-6");
}
