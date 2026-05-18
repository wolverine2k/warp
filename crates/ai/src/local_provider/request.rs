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
        ChatCompletionRequest, ChatContentPart, ChatMessage, ChatMessageContent, ImageUrlSpec,
        Role, StreamOptions, ToolCall, ToolCallFunction, ToolChoice,
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
    ///
    /// Phase B-6: this map now includes results from ALL prior exchanges
    /// (not just the current turn), so multi-turn agent loops don't lose
    /// historical tool output to the placeholder backfill.
    pub action_results: std::collections::HashMap<String, String>,
    /// Phase 4c-2. Attachments carried alongside the user query. Empty
    /// `Vec` is the default — every existing call site builds one without
    /// touching this field. Each adapter's request translator reads
    /// `attachments` and emits the upstream's per-modality wire shape;
    /// when empty, the translator emits the same text-only request body
    /// as before Phase 4c-2 (back-compat).
    pub attachments: Vec<crate::attachments::AgentAttachment>,
    /// Phase B-6: synthetic user-query injections, paired with anchor
    /// task-message ids. For local-provider conversations the warp.dev
    /// server isn't around to echo `Message::UserQuery` back into
    /// `task.messages`, so each historical user query is anchored to the
    /// first task-message id of its exchange. The translator emits a
    /// `role:"user"` message immediately *before* the message with that id
    /// during history rendering, restoring the user-then-assistant turn
    /// order the model needs to see.
    ///
    /// Empty `Vec` is the no-op default (warp.dev path / legacy local-only
    /// tests).
    pub synthetic_user_queries: Vec<(String, String)>,
    /// Phase A compaction config (defaults to `prune=true`,
    /// `tail_turns=DEFAULT_TAIL_TURNS`). Phase B-1 populates this from
    /// `AISettings.local_provider_compaction_*` at request build time.
    pub compaction_config: super::compaction::CompactionConfig,
    /// Phase B-2 sidecar state. The translator forwards this to
    /// `compute_prune_set` so prune halts at prior summary boundaries and
    /// skips already-pruned tool outputs. `Default::default()` is the
    /// "never compacted" baseline.
    pub compaction_state: super::compaction::CompactionState,
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

    // Phase B-3 head-summary projection. When the conversation has a
    // completed compaction, synthesize the `(user "Continue...", assistant
    // <summary>)` pair from `CompactionState` itself — the synthetic ids
    // never appear in `tasks`, so the controller-side helper doesn't have
    // to mutate the task store. We then drop every task message before the
    // recorded `tail_start_id`. The model sees `[system, continue, summary,
    // tail...]` instead of the original overflowing head. Skipped silently
    // when `completed.is_empty()` (unaffected baseline).
    let projection = compaction_projection(input);
    if let Some(p) = &projection {
        messages.push(ChatMessage::text(Role::User, p.continue_prompt.clone()));
        messages.push(ChatMessage::text(Role::Assistant, p.summary_text.clone()));
    }

    // Rendering modes:
    // - No projection: render all messages.
    // - Projection with `tail_start_id = Some(id)`: skip until we reach
    //   that id, then render the rest.
    // - Projection with `tail_start_id = None`: drop everything (manual
    //   `/compact` with no preserved tail).
    enum Mode {
        RenderAll,
        SkipUntil(String),
        DropAll,
    }
    let mut mode = match projection.as_ref() {
        None => Mode::RenderAll,
        Some(p) => match p.tail_start_id.as_deref() {
            Some(id) => Mode::SkipUntil(id.to_string()),
            None => Mode::DropAll,
        },
    };

    // Phase B-6: pre-index synthetic user queries by anchor message id so
    // we can inject them before the right task message in the rendering
    // loop below. Each entry's anchor is the FIRST task-message id of an
    // exchange whose `input` contained a `Message::UserQuery`-equivalent
    // (`AIAgentInput::UserQuery`). The map is consumed during rendering
    // — once an anchor matches, that user query is emitted exactly once.
    let synthetic_user_query_by_anchor: std::collections::HashMap<&str, &str> = input
        .synthetic_user_queries
        .iter()
        .map(|(anchor_id, query)| (anchor_id.as_str(), query.as_str()))
        .collect();

    for task in &input.tasks {
        for proto_msg in &task.messages {
            match &mode {
                Mode::RenderAll => {}
                Mode::DropAll => continue,
                Mode::SkipUntil(id) => {
                    if proto_msg.id.as_str() == id.as_str() {
                        mode = Mode::RenderAll;
                    } else {
                        continue;
                    }
                }
            }
            // Phase B-6: emit the historical user query before its anchor
            // message so the model sees `[user, assistant, ...]` rather
            // than a sequence of unprompted assistant outputs.
            if let Some(query) = synthetic_user_query_by_anchor.get(proto_msg.id.as_str()) {
                messages.push(ChatMessage::text(Role::User, *query));
            }
            push_history_messages(&mut messages, proto_msg);
        }
    }

    backfill_orphaned_tool_calls(&mut messages, &input.action_results);

    // Phase A compaction: replace old tool-output content with a placeholder
    // once the cumulative byte budget is exceeded. Keeps long, tool-heavy
    // conversations under the model's token limit. See
    // `crate::local_provider::compaction` for the algorithm and Phase B notes.
    if input.compaction_config.prune {
        let prune_set = crate::local_provider::compaction::wire::compute_prune_set(
            &input.tasks,
            &input.compaction_state,
        );
        crate::local_provider::compaction::wire::apply_prune(&mut messages, &prune_set);
    }

    if input.user_query.is_some() || !input.attachments.is_empty() {
        let user_content = if input.attachments.is_empty() {
            ChatMessageContent::Text(
                input.user_query.clone().unwrap_or_default(),
            )
        } else {
            let mut parts: Vec<ChatContentPart> = Vec::new();
            if let Some(text) = input.user_query.as_ref() {
                if !text.is_empty() {
                    parts.push(ChatContentPart::Text { text: text.clone() });
                }
            }
            for attachment in &input.attachments {
                if attachment.is_image() {
                    parts.push(ChatContentPart::ImageUrl {
                        image_url: ImageUrlSpec {
                            url: crate::attachments::encode_data_uri(
                                &attachment.mime,
                                &attachment.bytes,
                            ),
                        },
                    });
                } else {
                    log::warn!(
                        "OpenAi adapter: dropping unsupported attachment mime {} \
                         (only image/* is supported on this api_type)",
                        attachment.mime
                    );
                }
            }
            ChatMessageContent::Parts(parts)
        };
        messages.push(ChatMessage {
            role: Role::User,
            content: Some(user_content),
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
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
    }
}

/// Phase B-3 projection bundle describing what to splice into the head of
/// the outbound request body and where the preserved tail begins.
struct CompactionProjection {
    continue_prompt: String,
    summary_text: String,
    /// Id of the first task message to render. `None` means "drop everything
    /// after the synthetic pair" (manual `/compact` without an unconsumed
    /// tail).
    tail_start_id: Option<String>,
}

/// Reads the most recent [`super::compaction::CompletedCompaction`] off
/// `input.compaction_state` and returns the projection bundle. Returns
/// `None` when no compaction has run (unaffected baseline) or when the
/// completed entry has no cached summary text (defensive — we can't
/// reconstruct the head without it, so we no-op rather than send a request
/// missing context).
fn compaction_projection(input: &LocalProviderInput) -> Option<CompactionProjection> {
    let last = input.compaction_state.completed().last()?;
    let summary_text = last.summary_text.clone()?;
    Some(CompactionProjection {
        continue_prompt: super::compaction::prompt::build_continue_message(last.overflow),
        summary_text,
        tail_start_id: last.tail_start_id.clone(),
    })
}

/// Tools that are both signaled by the server (`supported_tools`) and have a
/// schema in the v1 curated set. `LocalTool::from_name` rejects anything else.
///
/// `pub(crate)` so the Anthropic translator can reuse the same filtering
/// rules — adapter-agnostic helper.
pub(crate) fn enabled_local_tools(
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
    ChatMessage::text(Role::System, prompt)
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
            && messages[i]
                .tool_calls
                .as_ref()
                .is_some_and(|tcs| !tcs.is_empty());
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
        for id in tool_call_ids
            .into_iter()
            .filter(|id| !satisfied.contains(id))
        {
            let content = action_results
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "(tool result not available)".to_string());
            messages.insert(
                insert_at,
                ChatMessage {
                    role: Role::Tool,
                    content: Some(ChatMessageContent::Text(content)),
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

pub(crate) fn push_history_messages(out: &mut Vec<ChatMessage>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => {
            out.push(ChatMessage::text(Role::User, q.query.clone()));
        }
        Some(M::AgentOutput(a)) => {
            out.push(ChatMessage::text(Role::Assistant, a.text.clone()));
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
                content: Some(ChatMessageContent::Text(summarize_tool_result(result))),
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
    summarize_tool_call_input(call).map(|(name, input)| (name, input.to_string()))
}

/// Same as `summarize_tool_call` but returns the input arguments as a typed
/// `serde_json::Value` instead of a stringified JSON object. The Anthropic
/// translator (`adapters::anthropic::request`) needs the structured form
/// because the Messages API's `tool_use.input` field takes a JSON object,
/// not a string.
///
/// Returns `None` for proto tool variants we don't have schemas for —
/// matches `summarize_tool_call`'s skip-from-history behavior. The local
/// model wouldn't have emitted unknown variants; if they appear in history
/// the conversation started against a Warp-hosted model.
pub(crate) fn summarize_tool_call_input(
    call: &api::message::ToolCall,
) -> Option<(String, serde_json::Value)> {
    use api::message::tool_call::Tool;
    match call.tool.as_ref()? {
        Tool::ReadFiles(rf) => {
            let names: Vec<&str> = rf.files.iter().map(|f| f.name.as_str()).collect();
            Some((
                "read_files".to_string(),
                serde_json::json!({ "paths": names }),
            ))
        }
        Tool::RunShellCommand(rsc) => Some((
            "run_shell_command".to_string(),
            serde_json::json!({ "command": rsc.command }),
        )),
        Tool::Grep(g) => Some((
            "grep".to_string(),
            serde_json::json!({ "queries": g.queries, "path": g.path }),
        )),
        _ => None,
    }
}

/// Render a `Message::ToolCallResult` as the `content` string the OpenAI
/// `tool` role message expects. Each v1 tool variant gets a tailored format:
/// the model needs to *read* this content to decide its next turn, so the
/// shape matches what a typical CLI agent would print.
///
/// `pub(crate)` so the Anthropic translator can use the same rendered
/// strings inside Anthropic's `tool_result.content` field — the rendered
/// output is adapter-agnostic.
pub(crate) fn summarize_tool_result(result: &api::message::ToolCallResult) -> String {
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
                if r.command.is_empty() {
                    "<command>"
                } else {
                    &r.command
                },
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
            let deleted: Vec<&str> = s
                .deleted_files
                .iter()
                .map(|d| d.file_path.as_str())
                .collect();
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
                let lines: Vec<String> = fm
                    .matched_lines
                    .iter()
                    .map(|m| m.line_number.to_string())
                    .collect();
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

    /// Test helper: extract the plain text from a `ChatMessageContent::Text`
    /// variant, mirroring the old `Option<String>::as_deref()` call sites.
    fn content_text(msg: &ChatMessage) -> Option<&str> {
        msg.content.as_ref().and_then(|c| c.as_text())
    }

    fn cfg() -> LocalProviderConfig {
        LocalProviderConfig {
            display_name: "Local".into(),
            base_url: "http://localhost:11434/v1".into(),
            model_id: "llama3.1".into(),
            api_key: None,
            supports_tools: true,
            context_window: None,
            api_type: crate::local_provider::AgentProviderApiType::OpenAi,
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
        assert_eq!(content_text(&req.messages[1]), Some("hi"));
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
                    message: Some(api::message::Message::AgentOutput(
                        api::message::AgentOutput { text: "ok".into() },
                    )),
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
        assert_eq!(content_text(&req.messages[1]), Some("first"));
        assert!(matches!(req.messages[2].role, Role::Assistant));
        assert_eq!(content_text(&req.messages[2]), Some("ok"));
        assert!(matches!(req.messages[3].role, Role::User));
        assert_eq!(content_text(&req.messages[3]), Some("second"));
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
        assert!(
            assistant.content.is_none(),
            "tool-call assistant has no text content"
        );
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
        action_results.insert(
            "call_real".to_string(),
            "[package]\nname = \"foo\"".to_string(),
        );
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
        assert_eq!(
            content_text(tool_msg),
            Some("[package]\nname = \"foo\"")
        );
    }

    #[test]
    fn context_window_threads_into_prompt() {
        let mut config = cfg();
        config.context_window = Some(4096);
        let req = compose_chat_completion_request(&empty_input(), &config);
        let sys_content = content_text(&req.messages[0]).unwrap();
        assert!(sys_content.contains("4096"));
    }

    // ---- summarize_tool_result ----

    fn tool_result(inner: api::message::tool_call_result::Result) -> api::message::ToolCallResult {
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
                    api::grep_result::Success {
                        matched_files: vec![],
                    },
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

    // ---- Phase B-3 projection ----

    use crate::local_provider::compaction::{CompactionState, CompletedCompaction};

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

    #[test]
    fn projection_no_op_when_compaction_state_empty() {
        let task = api::Task {
            id: "t1".into(),
            messages: vec![user_msg("u1", "hi"), agent_msg("a1", "hello")],
            ..Default::default()
        };
        let input = LocalProviderInput {
            tasks: vec![task],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        // system + user + assistant
        assert_eq!(req.messages.len(), 3);
    }

    #[test]
    fn projection_synthesizes_head_and_drops_pre_tail_history() {
        // The synthetic compaction pair is NOT in `tasks` — the projection
        // synthesizes it from `compaction_state`.
        let task = api::Task {
            id: "t1".into(),
            messages: vec![
                user_msg("u_old1", "old turn 1"),
                agent_msg("a_old1", "old reply 1"),
                user_msg("u_old2", "old turn 2"),
                agent_msg("a_old2", "old reply 2"),
                user_msg("u_new", "post-compact ask"),
            ],
            ..Default::default()
        };
        let mut state = CompactionState::default();
        state.push_completed(CompletedCompaction {
            user_msg_id: "compaction-trigger-X".into(),
            assistant_msg_id: "compaction-summary-X".into(),
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
        let req = compose_chat_completion_request(&input, &cfg());

        // Expect: system + synthetic continue user + synthetic summary
        // assistant + tail user. The four pre-tail messages are dropped.
        assert_eq!(
            req.messages.len(),
            4,
            "wrong msg count: {:?}",
            req.messages
                .iter()
                .map(|m| (m.role, m.content.clone()))
                .collect::<Vec<_>>()
        );
        // Continue prompt has the overflow=true preamble.
        assert!(content_text(&req.messages[1]).unwrap().contains("Continue"));
        assert_eq!(content_text(&req.messages[2]), Some("## Goal\n- summary"));
        assert_eq!(content_text(&req.messages[3]), Some("post-compact ask"));
    }

    #[test]
    fn projection_drops_all_history_when_tail_start_id_is_none() {
        // Manual `/compact` with no preserved tail: the synthetic pair is
        // the entire head, every task message gets dropped.
        let task = api::Task {
            id: "t1".into(),
            messages: vec![user_msg("u1", "hi"), agent_msg("a1", "hello")],
            ..Default::default()
        };
        let mut state = CompactionState::default();
        state.push_completed(CompletedCompaction {
            user_msg_id: "compaction-trigger-Y".into(),
            assistant_msg_id: "compaction-summary-Y".into(),
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
        let req = compose_chat_completion_request(&input, &cfg());
        // system + synthetic user + synthetic assistant — that's it.
        assert_eq!(req.messages.len(), 3);
        assert_eq!(content_text(&req.messages[2]), Some("manual digest"));
    }

    #[test]
    fn projection_no_op_when_summary_text_missing() {
        // Defensive: completed entry without cached summary_text. We
        // can't reconstruct the head, so we render the original messages
        // rather than silently lose context.
        let task = api::Task {
            id: "t1".into(),
            messages: vec![user_msg("u1", "hi")],
            ..Default::default()
        };
        let mut state = CompactionState::default();
        state.push_completed(CompletedCompaction {
            user_msg_id: "compaction-trigger-Z".into(),
            assistant_msg_id: "compaction-summary-Z".into(),
            tail_start_id: None,
            summary_text: None,
            auto: true,
            overflow: true,
        });
        let input = LocalProviderInput {
            tasks: vec![task],
            compaction_state: state,
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        assert_eq!(req.messages.len(), 2);
        assert_eq!(content_text(&req.messages[1]), Some("hi"));
    }

    // ---- Phase B-6 multi-turn agent loop ----

    #[test]
    fn synthetic_user_query_is_injected_before_anchor_message() {
        // The local-provider path doesn't have a server echoing
        // `Message::UserQuery` back into `task.messages`. The controller
        // surfaces historical user queries as `(anchor_id, query)` pairs;
        // the translator emits `role:"user"` immediately before the
        // anchor's task message during history rendering.
        let task = api::Task {
            id: "t1".into(),
            messages: vec![
                agent_msg("a_old", "first answer"),
                agent_msg("a_new", "second answer"),
            ],
            ..Default::default()
        };
        let input = LocalProviderInput {
            tasks: vec![task],
            // Anchor "a_old" -> "what is X?", anchor "a_new" -> "and Y?"
            synthetic_user_queries: vec![
                ("a_old".into(), "what is X?".into()),
                ("a_new".into(), "and Y?".into()),
            ],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        // [system, user("what is X?"), assistant("first answer"),
        //  user("and Y?"), assistant("second answer")]
        assert_eq!(req.messages.len(), 5);
        assert!(matches!(req.messages[0].role, Role::System));
        assert!(matches!(req.messages[1].role, Role::User));
        assert_eq!(content_text(&req.messages[1]), Some("what is X?"));
        assert!(matches!(req.messages[2].role, Role::Assistant));
        assert_eq!(content_text(&req.messages[2]), Some("first answer"));
        assert!(matches!(req.messages[3].role, Role::User));
        assert_eq!(content_text(&req.messages[3]), Some("and Y?"));
        assert!(matches!(req.messages[4].role, Role::Assistant));
        assert_eq!(content_text(&req.messages[4]), Some("second answer"));
    }

    #[test]
    fn synthetic_user_query_with_unmatched_anchor_is_dropped_silently() {
        // Defensive: if the anchor id no longer exists in task.messages
        // (e.g. compaction dropped it), we silently skip emitting the
        // synthetic user message rather than appending it at an arbitrary
        // position. Anchored injection is a hint, not a hard guarantee —
        // the translator must stay correct even when the anchor is gone.
        let task = api::Task {
            id: "t1".into(),
            messages: vec![agent_msg("a1", "answer")],
            ..Default::default()
        };
        let input = LocalProviderInput {
            tasks: vec![task],
            synthetic_user_queries: vec![("missing".into(), "ghost".into())],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        // [system, assistant("answer")] — no ghost user message.
        assert_eq!(req.messages.len(), 2);
        assert!(matches!(req.messages[0].role, Role::System));
        assert!(matches!(req.messages[1].role, Role::Assistant));
        assert_eq!(content_text(&req.messages[1]), Some("answer"));
    }

    #[test]
    fn historical_action_results_resolve_orphan_tool_calls_across_turns() {
        // Simulates a multi-turn agent loop: turn 1 produced a tool call
        // (whose result lives in `action_results`), turn 2 produced
        // another tool call (whose result is also in `action_results`),
        // and turn 3 is the current user query. The translator must pair
        // each historical assistant `tool_calls` entry with its real
        // `role:"tool"` follower instead of the
        // `"(tool result not available)"` placeholder.
        let tool_1 = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
            files: vec![api::message::tool_call::read_files::File {
                name: "Cargo.toml".into(),
                line_ranges: vec![],
            }],
        });
        let tool_2 = api::message::tool_call::Tool::Grep(api::message::tool_call::Grep {
            queries: vec!["fn main".into()],
            path: ".".into(),
        });
        let task = api::Task {
            id: "t1".into(),
            messages: vec![
                api::Message {
                    id: "m_call_1".into(),
                    message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                        tool_call_id: "call_alpha".into(),
                        tool: Some(tool_1),
                    })),
                    ..Default::default()
                },
                api::Message {
                    id: "m_call_2".into(),
                    message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                        tool_call_id: "call_beta".into(),
                        tool: Some(tool_2),
                    })),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut action_results = std::collections::HashMap::new();
        action_results.insert("call_alpha".into(), "[package]\nname = \"foo\"".into());
        action_results.insert("call_beta".into(), "src/main.rs: lines 1\n".into());

        let input = LocalProviderInput {
            user_query: Some("now what?".into()),
            tasks: vec![task],
            supported_tools: vec![api::ToolType::ReadFiles, api::ToolType::Grep],
            action_results,
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());

        // [system, assistant(tool_calls call_alpha), tool(real result for alpha),
        //  assistant(tool_calls call_beta), tool(real result for beta),
        //  user("now what?")]
        assert_eq!(req.messages.len(), 6, "{:#?}", req.messages);
        assert!(matches!(req.messages[1].role, Role::Assistant));
        assert!(matches!(req.messages[2].role, Role::Tool));
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_alpha"));
        assert_eq!(content_text(&req.messages[2]), Some("[package]\nname = \"foo\""));
        assert!(matches!(req.messages[3].role, Role::Assistant));
        assert!(matches!(req.messages[4].role, Role::Tool));
        assert_eq!(req.messages[4].tool_call_id.as_deref(), Some("call_beta"));
        // No placeholder anywhere.
        for m in &req.messages {
            assert_ne!(
                content_text(m),
                Some("(tool result not available)"),
                "placeholder leaked: {:?}",
                m
            );
        }
        assert!(matches!(req.messages[5].role, Role::User));
        assert_eq!(content_text(&req.messages[5]), Some("now what?"));
    }

    #[test]
    fn full_multi_turn_loop_round_trip() {
        // End-to-end shape of a 3-turn local-provider conversation:
        //
        //   Turn 1 — user asks "read Cargo.toml"
        //     assistant emits tool_call call_alpha (ReadFiles)
        //   Turn 2 — controller threads call_alpha's result back
        //     assistant emits tool_call call_beta (Grep)
        //   Turn 3 — controller threads call_beta's result back; new
        //     user query "summarize"
        //
        // The captured-bug scenario from `phase-b-6-multi-turn-agent-loop.md`:
        // 0 user messages, all tool messages "(tool result not available)".
        // After Phase B-6 the request body must contain BOTH historical
        // user messages (anchored) plus the current one, and BOTH tool
        // results' real content.
        let tool_1 = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
            files: vec![api::message::tool_call::read_files::File {
                name: "Cargo.toml".into(),
                line_ranges: vec![],
            }],
        });
        let tool_2 = api::message::tool_call::Tool::Grep(api::message::tool_call::Grep {
            queries: vec!["fn main".into()],
            path: ".".into(),
        });
        let task = api::Task {
            id: "t1".into(),
            messages: vec![
                api::Message {
                    id: "m_t1_call".into(),
                    message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                        tool_call_id: "call_alpha".into(),
                        tool: Some(tool_1),
                    })),
                    ..Default::default()
                },
                api::Message {
                    id: "m_t2_call".into(),
                    message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                        tool_call_id: "call_beta".into(),
                        tool: Some(tool_2),
                    })),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut action_results = std::collections::HashMap::new();
        action_results.insert("call_alpha".into(), "Cargo.toml body".into());
        action_results.insert("call_beta".into(), "src/main.rs hits".into());

        let input = LocalProviderInput {
            user_query: Some("summarize".into()),
            tasks: vec![task],
            supported_tools: vec![api::ToolType::ReadFiles, api::ToolType::Grep],
            action_results,
            // Turn-1 user query is anchored to the first task message
            // ("m_t1_call"). Turn-2 had no fresh user query (auto follow-
            // up from action result).
            synthetic_user_queries: vec![("m_t1_call".into(), "read Cargo.toml".into())],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());

        // Expected (in order):
        //   0: system
        //   1: user "read Cargo.toml"
        //   2: assistant tool_calls call_alpha
        //   3: tool call_alpha "Cargo.toml body"
        //   4: assistant tool_calls call_beta
        //   5: tool call_beta "src/main.rs hits"
        //   6: user "summarize"
        assert_eq!(req.messages.len(), 7);
        assert!(matches!(req.messages[0].role, Role::System));
        assert!(matches!(req.messages[1].role, Role::User));
        assert_eq!(content_text(&req.messages[1]), Some("read Cargo.toml"));
        assert!(matches!(req.messages[2].role, Role::Assistant));
        assert!(matches!(req.messages[3].role, Role::Tool));
        assert_eq!(content_text(&req.messages[3]), Some("Cargo.toml body"));
        assert!(matches!(req.messages[4].role, Role::Assistant));
        assert!(matches!(req.messages[5].role, Role::Tool));
        assert_eq!(content_text(&req.messages[5]), Some("src/main.rs hits"));
        assert!(matches!(req.messages[6].role, Role::User));
        assert_eq!(content_text(&req.messages[6]), Some("summarize"));

        // Tools advertised non-null on every multi-turn body.
        let tools = req.tools.expect("tools should be advertised");
        assert!(!tools.is_empty());
    }

    // ---- Phase 4c-2 attachment tests ----

    use crate::attachments::AgentAttachment;

    fn png_attachment() -> AgentAttachment {
        AgentAttachment {
            mime: "image/png".into(),
            bytes: vec![0x89, 0x50, 0x4e, 0x47],
            display_name: Some("test.png".into()),
        }
    }

    #[test]
    fn text_only_turn_emits_string_content() {
        let input = LocalProviderInput {
            user_query: Some("hello".into()),
            attachments: Vec::new(),
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        let user_msg = req.messages.iter().find(|m| matches!(m.role, Role::User)).unwrap();
        assert!(
            matches!(&user_msg.content, Some(ChatMessageContent::Text(t)) if t == "hello"),
            "expected Text(\"hello\"), got {:?}",
            user_msg.content
        );
    }

    #[test]
    fn turn_with_image_emits_parts_array() {
        let input = LocalProviderInput {
            user_query: Some("what is this".into()),
            attachments: vec![png_attachment()],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        let user_msg = req.messages.iter().find(|m| matches!(m.role, Role::User)).unwrap();
        let parts = match &user_msg.content {
            Some(ChatMessageContent::Parts(p)) => p,
            other => panic!("expected Parts, got {other:?}"),
        };
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[0], ChatContentPart::Text { text } if text == "what is this"),
            "unexpected first part: {:?}",
            parts[0]
        );
        assert!(
            matches!(&parts[1], ChatContentPart::ImageUrl { image_url } if image_url.url.starts_with("data:image/png;base64,")),
            "unexpected second part: {:?}",
            parts[1]
        );
    }

    #[test]
    fn pdf_attachment_is_dropped_and_only_text_part_remains() {
        let input = LocalProviderInput {
            user_query: Some("read this".into()),
            attachments: vec![AgentAttachment {
                mime: "application/pdf".into(),
                bytes: vec![1, 2, 3],
                display_name: None,
            }],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        let user_msg = req.messages.iter().find(|m| matches!(m.role, Role::User)).unwrap();
        let parts = match &user_msg.content {
            Some(ChatMessageContent::Parts(p)) => p,
            other => panic!("expected Parts, got {other:?}"),
        };
        // PDF is dropped; only the text part remains.
        assert_eq!(parts.len(), 1, "expected 1 part (text only), got {parts:?}");
        assert!(matches!(&parts[0], ChatContentPart::Text { .. }));
    }

    #[test]
    fn empty_user_query_with_image_emits_only_image_part() {
        let input = LocalProviderInput {
            user_query: Some("".into()),
            attachments: vec![png_attachment()],
            ..Default::default()
        };
        let req = compose_chat_completion_request(&input, &cfg());
        let user_msg = req.messages.iter().find(|m| matches!(m.role, Role::User)).unwrap();
        let parts = match &user_msg.content {
            Some(ChatMessageContent::Parts(p)) => p,
            other => panic!("expected Parts, got {other:?}"),
        };
        // Empty text is filtered out; only the image remains.
        assert_eq!(parts.len(), 1, "expected 1 part (image only), got {parts:?}");
        assert!(matches!(&parts[0], ChatContentPart::ImageUrl { .. }));
    }
}
