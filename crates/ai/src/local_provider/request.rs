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

use crate::local_provider::config::LocalProviderConfig;
use crate::local_provider::prompt;
use crate::local_provider::tools::{self, LocalTool};
use crate::local_provider::wire::{
    ChatCompletionRequest, ChatContentPart, ChatMessage, ChatMessageContent, ImageUrlSpec, Role,
    StreamOptions, ToolCall, ToolCallFunction, ToolChoice,
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
            ChatMessageContent::Text(input.user_query.clone().unwrap_or_default())
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
#[path = "request_tests.rs"]
mod tests;
