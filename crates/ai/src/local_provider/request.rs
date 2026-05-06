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
#[derive(Debug, Clone, Default)]
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
    /// Conversation token from the calling controller. When `Some`, the SSE
    /// adapter emits StreamInit/AddMessages with this id so the controller
    /// can match the resulting events to its existing AIConversation. When
    /// `None`, the adapter generates a fresh `local:<uuid>` (legacy/test
    /// path; produces `Conversation(TaskNotFound)` when wired into the real
    /// agent flow because the controller has no matching task).
    pub conversation_id: Option<String>,
    /// The id of the active task this turn writes into. Same matching
    /// reason as `conversation_id`. Should be the id of the most recent
    /// entry in `tasks` — the one the controller is actively driving.
    pub task_id: Option<String>,
    /// Whether the adapter should emit `Action::CreateTask` to upgrade the
    /// optimistic root task before its first `AddMessagesToTask`. True on the
    /// very first turn of a fresh local-provider conversation (no server-
    /// created tasks exist yet). False once any server-created task is
    /// present — emitting CreateTask on an already-initialized task triggers
    /// `UpgradeOptimisticTask::UnexpectedUpgrade` AND corrupts the task store
    /// (the controller's `?` propagation leaves the just-removed root task
    /// un-reinserted), cascading every subsequent action into TaskNotFound.
    pub needs_create_task: bool,
    /// Tool-call results carried alongside the request. Map of
    /// `tool_call_id` → rendered result string. Populated from the
    /// controller's `request_input.inputs` `ActionResult` entries — those
    /// don't land in `task.messages` for local-provider conversations the
    /// way they would for the server flow, so without this map the OpenAI
    /// request body would carry an assistant `tool_calls` message with no
    /// matching `role:"tool"` follow-up, and the upstream rejects with
    /// HTTP 400 ("tool_calls must be followed by tool messages").
    pub action_results: std::collections::HashMap<String, String>,
    /// Phase A compaction config (defaults to `prune=true`,
    /// `tail_turns=DEFAULT_TAIL_TURNS`). Phase B will populate this from
    /// `AISettings.byop_compaction_*` per the openwarp port.
    pub compaction_config: super::compaction::CompactionConfig,
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

    backfill_orphaned_tool_calls(&mut messages, &input.action_results);

    // Phase A compaction: replace old tool-output content with a placeholder
    // once the cumulative byte budget is exceeded. Keeps long, tool-heavy
    // conversations under the model's token limit. See
    // `crate::local_provider::compaction` for the algorithm and Phase B notes.
    if input.compaction_config.prune {
        let prune_set =
            crate::local_provider::compaction::wire::compute_prune_set(&input.tasks);
        crate::local_provider::compaction::wire::apply_prune(&mut messages, &prune_set);
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

/// Walk the rendered message list and ensure every assistant `tool_calls`
/// entry is followed by matching `role:"tool"` messages before any non-tool
/// message. For any `tool_call_id` that lacks a follower, splice in a
/// synthetic tool message: the rendered result from `action_results` if we
/// have one, or a placeholder so the upstream's strict-ordering validator
/// stops rejecting the request with HTTP 400.
fn backfill_orphaned_tool_calls(
    messages: &mut Vec<ChatMessage>,
    action_results: &std::collections::HashMap<String, String>,
) {
    use std::collections::HashSet;
    let mut i = 0;
    while i < messages.len() {
        let needs_check = matches!(messages[i].role, Role::Assistant)
            && messages[i].tool_calls.as_ref().is_some_and(|tcs| !tcs.is_empty());
        if !needs_check {
            i += 1;
            continue;
        }
        let tool_call_ids: Vec<String> = messages[i]
            .tool_calls
            .as_ref()
            .expect("checked above")
            .iter()
            .map(|tc| tc.id.clone())
            .collect();
        let mut satisfied: HashSet<String> = HashSet::new();
        let mut j = i + 1;
        while j < messages.len() && matches!(messages[j].role, Role::Tool) {
            if let Some(id) = &messages[j].tool_call_id {
                satisfied.insert(id.clone());
            }
            j += 1;
        }
        let mut insert_at = j;
        for id in tool_call_ids.into_iter().filter(|id| !satisfied.contains(id)) {
            let content = action_results
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "(tool result not available)".to_string());
            messages.insert(
                insert_at,
                ChatMessage {
                    role: Role::Tool,
                    content: Some(content),
                    tool_calls: None,
                    tool_call_id: Some(id),
                    name: None,
                },
            );
            insert_at += 1;
        }
        i = insert_at;
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

/// Render a `Message::ToolCallResult` as the `content` string the OpenAI
/// `tool` role message expects. Each v1 tool variant gets a tailored format:
/// the model needs to *read* this content to decide its next turn, so the
/// shape matches what a typical CLI agent would print.
fn summarize_tool_result(result: &api::message::ToolCallResult) -> String {
    use api::message::tool_call_result::Result as R;
    let Some(inner) = result.result.as_ref() else {
        return "<empty result>".to_string();
    };
    match inner {
        R::RunShellCommand(rsc) => render_run_shell(rsc),
        R::ReadFiles(rf) => render_read_files(rf),
        R::ApplyFileDiffs(afd) => render_apply_diffs(afd),
        R::Grep(g) => render_grep(g),
        R::FileGlobV2(g) => render_file_glob_v2(g),
        R::Cancel(_) => "<cancelled by user>".to_string(),
        // Other variants are server-only or future tools we don't expose.
        _ => "<result not supported by local provider>".to_string(),
    }
}

fn render_run_shell(r: &api::RunShellCommandResult) -> String {
    use api::run_shell_command_result::Result as R;
    match r.result.as_ref() {
        Some(R::CommandFinished(f)) => {
            format!(
                "$ {}\n{}\n[exit {}]",
                if r.command.is_empty() { "<command>" } else { &r.command },
                f.output,
                f.exit_code
            )
        }
        Some(R::LongRunningCommandSnapshot(_)) => {
            format!("$ {}\n<command still running>", r.command)
        }
        Some(R::PermissionDenied(_)) => {
            format!("$ {}\n<permission denied>", r.command)
        }
        None => "<empty shell result>".to_string(),
    }
}

fn render_read_files(r: &api::ReadFilesResult) -> String {
    use api::read_files_result::Result as R;
    match r.result.as_ref() {
        Some(R::TextFilesSuccess(s)) => {
            let mut out = String::new();
            for f in &s.files {
                out.push_str(&format!("\n--- {} ---\n{}\n", f.file_path, f.content));
            }
            if out.is_empty() {
                "<no files read>".to_string()
            } else {
                out
            }
        }
        Some(R::AnyFilesSuccess(_)) => {
            "<files read (binary; not rendered for the local model)>".to_string()
        }
        Some(R::Error(e)) => format!("<read failed: {}>", e.message),
        None => "<empty read result>".to_string(),
    }
}

fn render_apply_diffs(r: &api::ApplyFileDiffsResult) -> String {
    use api::apply_file_diffs_result::Result as R;
    match r.result.as_ref() {
        Some(R::Success(s)) => {
            let updated: Vec<&str> = s
                .updated_files_v2
                .iter()
                .filter_map(|u| u.file.as_ref())
                .map(|f| f.file_path.as_str())
                .collect();
            let deleted: Vec<&str> = s.deleted_files.iter().map(|d| d.file_path.as_str()).collect();
            let mut bits = Vec::new();
            if !updated.is_empty() {
                bits.push(format!("updated: {}", updated.join(", ")));
            }
            if !deleted.is_empty() {
                bits.push(format!("deleted: {}", deleted.join(", ")));
            }
            if bits.is_empty() {
                "<diffs applied (no files changed)>".to_string()
            } else {
                bits.join("; ")
            }
        }
        Some(R::Error(e)) => format!("<apply diffs failed: {}>", e.message),
        None => "<empty apply diffs result>".to_string(),
    }
}

fn render_grep(r: &api::GrepResult) -> String {
    use api::grep_result::Result as R;
    match r.result.as_ref() {
        Some(R::Success(s)) => {
            if s.matched_files.is_empty() {
                return "<no matches>".to_string();
            }
            let mut out = String::new();
            for fm in &s.matched_files {
                let lines: Vec<String> =
                    fm.matched_lines.iter().map(|m| m.line_number.to_string()).collect();
                out.push_str(&format!("{}: lines {}\n", fm.file_path, lines.join(",")));
            }
            out
        }
        Some(R::Error(e)) => format!("<grep failed: {}>", e.message),
        None => "<empty grep result>".to_string(),
    }
}

fn render_file_glob_v2(r: &api::FileGlobV2Result) -> String {
    use api::file_glob_v2_result::Result as R;
    match r.result.as_ref() {
        Some(R::Success(s)) => {
            if s.matched_files.is_empty() {
                return "<no files matched>".to_string();
            }
            let mut out = String::new();
            for f in &s.matched_files {
                out.push_str(&f.file_path);
                out.push('\n');
            }
            if !s.warnings.is_empty() {
                out.push_str(&format!("\n[warnings: {}]", s.warnings));
            }
            out
        }
        Some(R::Error(e)) => format!("<glob failed: {}>", e.message),
        None => "<empty glob result>".to_string(),
    }
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
                })),
                ..Default::default()
            }],
            ..Default::default()
        };
        let input = LocalProviderInput {
            user_query: None,
            tasks: vec![task],
            supported_tools: vec![],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        // system + assistant(tool_calls) + synthetic tool message backfilled by
        // `backfill_orphaned_tool_calls` so the upstream's strict-ordering
        // validator doesn't reject the request with HTTP 400.
        assert_eq!(req.messages.len(), 3);
        let assistant = &req.messages[1];
        assert!(matches!(assistant.role, Role::Assistant));
        assert!(assistant.content.is_none(), "tool-call assistant has no text content");
        let tcs = assistant.tool_calls.as_ref().expect("tool_calls present");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_xyz");
        assert_eq!(tcs[0].function.name, "read_files");
        assert!(tcs[0].function.arguments.contains("src/main.rs"));
        let tool_followup = &req.messages[2];
        assert!(matches!(tool_followup.role, Role::Tool));
        assert_eq!(tool_followup.tool_call_id.as_deref(), Some("call_xyz"));
    }

    #[test]
    fn orphaned_tool_call_gets_backfilled_from_action_results() {
        let tool = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
            files: vec![api::message::tool_call::read_files::File {
                name: "Cargo.toml".into(),
                line_ranges: vec![],
            }],
        });
        let task = api::Task {
            id: "t1".into(),
            messages: vec![api::Message {
                id: "m1".into(),
                message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                    tool_call_id: "call_real".into(),
                    tool: Some(tool),
                })),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut action_results = std::collections::HashMap::new();
        action_results.insert("call_real".to_string(), "[package]\nname = \"foo\"".to_string());
        let input = LocalProviderInput {
            user_query: Some("what's in Cargo.toml?".into()),
            tasks: vec![task],
            supported_tools: vec![],
            action_results,
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        // system + assistant(tool_calls) + tool(result) + user
        assert_eq!(req.messages.len(), 4);
        let tool_msg = &req.messages[2];
        assert!(matches!(tool_msg.role, Role::Tool));
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_real"));
        assert_eq!(tool_msg.content.as_deref(), Some("[package]\nname = \"foo\""));
    }

    #[test]
    fn context_window_threads_into_prompt() {
        let mut config = cfg();
        config.context_window = Some(4096);
        let req = compose_chat_completion_request(&empty_input(), &config);
        let sys_content = req.messages[0].content.as_deref().unwrap();
        assert!(sys_content.contains("4096"));
    }

    // ---- summarize_tool_result ----

    fn tool_result(
        inner: api::message::tool_call_result::Result,
    ) -> api::message::ToolCallResult {
        api::message::ToolCallResult {
            tool_call_id: "tc_1".into(),
            result: Some(inner),
            ..Default::default()
        }
    }

    #[test]
    fn run_shell_command_finished_renders_command_output_and_exit() {
        let r = tool_result(api::message::tool_call_result::Result::RunShellCommand(
            api::RunShellCommandResult {
                command: "ls -la".into(),
                result: Some(api::run_shell_command_result::Result::CommandFinished(
                    api::ShellCommandFinished {
                        output: "total 0\ndrwx 1 user 0 .\n".into(),
                        exit_code: 0,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        ));
        let s = summarize_tool_result(&r);
        assert!(s.contains("$ ls -la"));
        assert!(s.contains("total 0"));
        assert!(s.contains("[exit 0]"));
    }

    #[test]
    fn read_files_text_success_renders_each_file() {
        let r = tool_result(api::message::tool_call_result::Result::ReadFiles(
            api::ReadFilesResult {
                result: Some(api::read_files_result::Result::TextFilesSuccess(
                    api::read_files_result::TextFilesSuccess {
                        files: vec![
                            api::FileContent {
                                file_path: "src/main.rs".into(),
                                content: "fn main() {}".into(),
                                ..Default::default()
                            },
                            api::FileContent {
                                file_path: "Cargo.toml".into(),
                                content: "[package]\nname = \"foo\"".into(),
                                ..Default::default()
                            },
                        ],
                    },
                )),
            },
        ));
        let s = summarize_tool_result(&r);
        assert!(s.contains("--- src/main.rs ---"));
        assert!(s.contains("fn main() {}"));
        assert!(s.contains("--- Cargo.toml ---"));
    }

    #[test]
    fn read_files_error_renders_message() {
        let r = tool_result(api::message::tool_call_result::Result::ReadFiles(
            api::ReadFilesResult {
                result: Some(api::read_files_result::Result::Error(
                    api::read_files_result::Error {
                        message: "permission denied".into(),
                    },
                )),
            },
        ));
        let s = summarize_tool_result(&r);
        assert!(s.contains("read failed"));
        assert!(s.contains("permission denied"));
    }

    #[test]
    fn grep_success_with_matches_renders_paths_and_lines() {
        let r = tool_result(api::message::tool_call_result::Result::Grep(
            api::GrepResult {
                result: Some(api::grep_result::Result::Success(
                    api::grep_result::Success {
                        matched_files: vec![api::grep_result::success::GrepFileMatch {
                            file_path: "src/lib.rs".into(),
                            matched_lines: vec![
                                api::grep_result::success::grep_file_match::GrepLineMatch {
                                    line_number: 12,
                                },
                                api::grep_result::success::grep_file_match::GrepLineMatch {
                                    line_number: 47,
                                },
                            ],
                        }],
                    },
                )),
            },
        ));
        let s = summarize_tool_result(&r);
        assert!(s.contains("src/lib.rs"));
        assert!(s.contains("12"));
        assert!(s.contains("47"));
    }

    #[test]
    fn grep_no_matches_says_no_matches() {
        let r = tool_result(api::message::tool_call_result::Result::Grep(
            api::GrepResult {
                result: Some(api::grep_result::Result::Success(
                    api::grep_result::Success { matched_files: vec![] },
                )),
            },
        ));
        assert_eq!(summarize_tool_result(&r), "<no matches>");
    }

    #[test]
    fn file_glob_v2_renders_paths() {
        let r = tool_result(api::message::tool_call_result::Result::FileGlobV2(
            api::FileGlobV2Result {
                result: Some(api::file_glob_v2_result::Result::Success(
                    api::file_glob_v2_result::Success {
                        matched_files: vec![
                            api::file_glob_v2_result::success::FileGlobMatch {
                                file_path: "a.rs".into(),
                            },
                            api::file_glob_v2_result::success::FileGlobMatch {
                                file_path: "b.rs".into(),
                            },
                        ],
                        warnings: String::new(),
                    },
                )),
            },
        ));
        let s = summarize_tool_result(&r);
        assert!(s.contains("a.rs"));
        assert!(s.contains("b.rs"));
    }

    #[test]
    fn apply_file_diffs_success_lists_updates_and_deletes() {
        let r = tool_result(api::message::tool_call_result::Result::ApplyFileDiffs(
            api::ApplyFileDiffsResult {
                result: Some(api::apply_file_diffs_result::Result::Success(
                    api::apply_file_diffs_result::Success {
                        updated_files_v2: vec![
                            api::apply_file_diffs_result::success::UpdatedFileContent {
                                file: Some(api::FileContent {
                                    file_path: "edited.rs".into(),
                                    ..Default::default()
                                }),
                                was_edited_by_user: false,
                            },
                        ],
                        deleted_files: vec![api::apply_file_diffs_result::success::DeletedFile {
                            file_path: "removed.rs".into(),
                        }],
                        ..Default::default()
                    },
                )),
            },
        ));
        let s = summarize_tool_result(&r);
        assert!(s.contains("updated: edited.rs"));
        assert!(s.contains("deleted: removed.rs"));
    }

    #[test]
    fn cancel_result_renders_clearly() {
        let r = tool_result(api::message::tool_call_result::Result::Cancel(()));
        let s = summarize_tool_result(&r);
        assert!(s.to_lowercase().contains("cancel"));
    }

    #[test]
    fn empty_result_is_handled_safely() {
        let r = api::message::ToolCallResult {
            tool_call_id: "tc_x".into(),
            result: None,
            ..Default::default()
        };
        let s = summarize_tool_result(&r);
        assert!(s.contains("empty"));
    }
}
