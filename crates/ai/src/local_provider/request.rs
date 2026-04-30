//! Translator: Warp in-memory conversation -> OpenAI ChatCompletionRequest.
//!
//! Per `specs/GH9303/tech.md` §6.4: Warp's tuned system prompt is server-only,
//! so we synthesize one in `prompt::compose_system_prompt`. Tool schemas come
//! from `tools::tool_definitions` (the curated v1 set).
//!
//! This module defines a minimal `LocalProviderInput` struct that the dispatch
//! fork populates from `RequestParams` at the call site. Keeping the inputs
//! minimal keeps this module unit-testable without dragging the `app/` types
//! into the `ai` crate's test harness.

use warp_multi_agent_api as api;

use crate::local_provider::{
    config::LocalProviderConfig,
    prompt,
    tools::{self, LocalTool},
    wire::{
        ChatCompletionRequest, ChatMessage, Role, ToolCall, ToolCallFunction, ToolChoice,
    },
};

/// Minimal projection of `RequestParams` plus history that the request
/// translator needs. The dispatch fork builds this; the translator consumes it.
#[derive(Debug, Clone)]
pub struct LocalProviderInput {
    /// The new turn's user query, when there is one. `None` for variants like
    /// `ResumeConversation` where history alone drives the next request.
    pub user_query: Option<String>,
    /// The prior conversation history — full `Task` list straight from
    /// `RequestParams.tasks`. The translator walks each task's `messages`.
    pub tasks: Vec<api::Task>,
    /// Tools the local model is allowed to use. The translator filters to
    /// only those `LocalTool::from_name` recognizes; unsupported entries
    /// are silently dropped (they wouldn't have schemas anyway).
    pub supported_tools: Vec<api::ToolType>,
}

/// Build the OpenAI request body for a single turn.
pub fn compose_chat_completion_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> ChatCompletionRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tools::tool_definitions(&local_tools))
    } else {
        None
    };
    let tool_choice = tools.as_ref().map(|_| ToolChoice::Auto);

    let mut messages = Vec::new();
    messages.push(system_message(&local_tools, cfg));

    for task in &input.tasks {
        for proto_msg in &task.messages {
            push_history_messages(&mut messages, proto_msg);
        }
    }

    if let Some(q) = input.user_query.as_deref() {
        messages.push(ChatMessage {
            role: Role::User,
            content: Some(q.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
    }

    ChatCompletionRequest {
        model: cfg.model_id.clone(),
        messages,
        tools,
        tool_choice,
        stream: true,
    }
}

/// Tools that are both signaled by the server (`supported_tools`) and have a
/// schema in the v1 curated set. `LocalTool::from_name` rejects anything else.
fn enabled_local_tools(
    supported: impl IntoIterator<Item = api::ToolType>,
    cfg: &LocalProviderConfig,
) -> Vec<LocalTool> {
    if !cfg.supports_tools {
        return vec![];
    }
    supported
        .into_iter()
        .filter_map(|t| LocalTool::from_name(tool_type_name(t)))
        .collect()
}

fn system_message(local_tools: &[LocalTool], cfg: &LocalProviderConfig) -> ChatMessage {
    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let prompt = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );
    ChatMessage {
        role: Role::System,
        content: Some(prompt),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

fn push_history_messages(out: &mut Vec<ChatMessage>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => {
            out.push(ChatMessage {
                role: Role::User,
                content: Some(q.query.clone()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }
        Some(M::AgentOutput(a)) => {
            out.push(ChatMessage {
                role: Role::Assistant,
                content: Some(a.text.clone()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }
        Some(M::ToolCall(call)) => {
            // OpenAI represents tool calls on an assistant message with `tool_calls`.
            // We don't have the original arguments-string the server emitted; we
            // serialize the typed proto variant back to JSON best-effort.
            let (name, arguments) = match summarize_tool_call(call) {
                Some(t) => t,
                None => return, // unknown variant -> silently skip in history
            };
            out.push(ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: call.tool_call_id.clone(),
                    kind: "function",
                    function: ToolCallFunction { name, arguments },
                }]),
                tool_call_id: None,
                name: None,
            });
        }
        Some(M::ToolCallResult(result)) => {
            out.push(ChatMessage {
                role: Role::Tool,
                content: Some(summarize_tool_result(result)),
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id.clone()),
                name: None,
            });
        }
        // AgentReasoning is intentionally NOT replayed in history (matches OpenAI
        // semantics where only final assistant text persists across turns).
        Some(M::AgentReasoning(_)) => {}
        // Other Message variants (ServerEvent, UpdateTodos, WebSearch, etc.) are
        // metadata that the local model doesn't need to see. Silent skip.
        Some(_) | None => {}
    }
}

fn tool_type_name(tt: api::ToolType) -> &'static str {
    use api::ToolType::*;
    // The proto enum names are TitleCase but the conventional tool names the
    // client uses (and that LocalTool::from_name accepts) are snake_case.
    // Add to this map when we ship a new tool in tools.rs.
    match tt {
        ReadFiles => "read_files",
        ApplyFileDiffs => "apply_file_diffs",
        RunShellCommand => "run_shell_command",
        Grep => "grep",
        FileGlobV2 => "file_glob_v2",
        _ => "<unsupported>",
    }
}

fn summarize_tool_call(call: &api::message::ToolCall) -> Option<(String, String)> {
    use api::message::tool_call::Tool;
    let (name, args) = match call.tool.as_ref()? {
        Tool::ReadFiles(rf) => {
            let names: Vec<&str> = rf.files.iter().map(|f| f.name.as_str()).collect();
            (
                "read_files".to_string(),
                serde_json::json!({ "paths": names }).to_string(),
            )
        }
        Tool::RunShellCommand(rsc) => (
            "run_shell_command".to_string(),
            serde_json::json!({ "command": rsc.command }).to_string(),
        ),
        Tool::Grep(g) => (
            "grep".to_string(),
            serde_json::json!({ "queries": g.queries, "path": g.path }).to_string(),
        ),
        // Variants we don't have schemas for yet are skipped from history. The
        // local model wouldn't have emitted them; if they exist in history it's
        // because the conversation started against a Warp-hosted model.
        _ => return None,
    };
    Some((name, args))
}

fn summarize_tool_result(_result: &api::message::ToolCallResult) -> String {
    // Phase 2 placeholder: serialize a compact textual summary. Phase 7 (tool-call
    // cycle) replaces this with proper text-extraction per result variant.
    "<tool result>".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LocalProviderConfig {
        LocalProviderConfig {
            display_name: "Local".into(),
            base_url: "http://localhost:11434/v1".into(),
            model_id: "llama3.1".into(),
            api_key: None,
            supports_tools: true,
            context_window: None,
        }
    }

    fn empty_input() -> LocalProviderInput {
        LocalProviderInput {
            user_query: None,
            tasks: vec![],
            supported_tools: vec![],
        }
    }

    #[test]
    fn always_emits_system_message_first() {
        let req = compose_chat_completion_request(&empty_input(), &cfg());
        assert_eq!(req.messages.len(), 1);
        assert!(matches!(req.messages[0].role, Role::System));
    }

    #[test]
    fn appends_user_query_when_set() {
        let mut input = empty_input();
        input.user_query = Some("hi".into());
        let req = compose_chat_completion_request(&input, &cfg());
        assert_eq!(req.messages.len(), 2);
        assert!(matches!(req.messages[1].role, Role::User));
        assert_eq!(req.messages[1].content.as_deref(), Some("hi"));
    }

    #[test]
    fn model_field_is_user_id_not_synthetic() {
        let req = compose_chat_completion_request(&empty_input(), &cfg());
        assert_eq!(req.model, "llama3.1");
        assert!(!req.model.starts_with("local:"));
    }

    #[test]
    fn stream_is_always_true() {
        let req = compose_chat_completion_request(&empty_input(), &cfg());
        assert!(req.stream);
    }

    #[test]
    fn tools_present_when_supported_and_v1_listed() {
        let mut input = empty_input();
        input.supported_tools = vec![api::ToolType::ReadFiles];
        let req = compose_chat_completion_request(&input, &cfg());
        let tools = req.tools.as_ref().expect("tools present");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "read_files");
        assert!(matches!(req.tool_choice, Some(ToolChoice::Auto)));
    }

    #[test]
    fn tools_absent_when_supports_tools_false() {
        let mut config = cfg();
        config.supports_tools = false;
        let mut input = empty_input();
        input.supported_tools = vec![api::ToolType::ReadFiles];
        let req = compose_chat_completion_request(&input, &config);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
    }

    #[test]
    fn tools_absent_when_no_v1_tools_signaled() {
        let mut input = empty_input();
        // None of these are in the v1 curated set
        input.supported_tools = vec![api::ToolType::SearchCodebase, api::ToolType::CallMcpTool];
        let req = compose_chat_completion_request(&input, &cfg());
        assert!(req.tools.is_none());
    }

    #[test]
    fn history_walks_in_proto_order() {
        // Build one task with three messages: user_query, agent_output, user_query.
        let task = api::Task {
            id: "t1".into(),
            messages: vec![
                api::Message {
                    id: "m1".into(),
                    message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                        query: "first".into(),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                api::Message {
                    id: "m2".into(),
                    message: Some(api::message::Message::AgentOutput(api::message::AgentOutput {
                        text: "ok".into(),
                    })),
                    ..Default::default()
                },
                api::Message {
                    id: "m3".into(),
                    message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                        query: "second".into(),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let input = LocalProviderInput {
            user_query: None,
            tasks: vec![task],
            supported_tools: vec![],
        };
        let req = compose_chat_completion_request(&input, &cfg());
        // system + 3 history messages
        assert_eq!(req.messages.len(), 4);
        assert!(matches!(req.messages[0].role, Role::System));
        assert!(matches!(req.messages[1].role, Role::User));
        assert_eq!(req.messages[1].content.as_deref(), Some("first"));
        assert!(matches!(req.messages[2].role, Role::Assistant));
        assert_eq!(req.messages[2].content.as_deref(), Some("ok"));
        assert!(matches!(req.messages[3].role, Role::User));
        assert_eq!(req.messages[3].content.as_deref(), Some("second"));
    }

    #[test]
    fn reasoning_messages_dropped_from_history() {
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
            user_query: None,
            tasks: vec![task],
            supported_tools: vec![],
        };
        let req = compose_chat_completion_request(&input, &cfg());
        assert_eq!(req.messages.len(), 1, "only system; reasoning is dropped");
    }

    #[test]
    fn tool_call_history_translated_to_openai_format() {
        let tool = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
            files: vec![api::message::tool_call::read_files::File {
                name: "src/main.rs".into(),
                line_ranges: vec![],
            }],
        });
        let task = api::Task {
            id: "t1".into(),
            messages: vec![api::Message {
                id: "m1".into(),
                message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                    tool_call_id: "call_xyz".into(),
                    tool: Some(tool),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        };
        let input = LocalProviderInput {
            user_query: None,
            tasks: vec![task],
            supported_tools: vec![],
        };
        let req = compose_chat_completion_request(&input, &cfg());
        assert_eq!(req.messages.len(), 2);
        let assistant = &req.messages[1];
        assert!(matches!(assistant.role, Role::Assistant));
        assert!(assistant.content.is_none(), "tool-call assistant has no text content");
        let tcs = assistant.tool_calls.as_ref().expect("tool_calls present");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_xyz");
        assert_eq!(tcs[0].function.name, "read_files");
        assert!(tcs[0].function.arguments.contains("src/main.rs"));
    }

    #[test]
    fn context_window_threads_into_prompt() {
        let mut config = cfg();
        config.context_window = Some(4096);
        let req = compose_chat_completion_request(&empty_input(), &config);
        let sys_content = req.messages[0].content.as_deref().unwrap();
        assert!(sys_content.contains("4096"));
    }
}
