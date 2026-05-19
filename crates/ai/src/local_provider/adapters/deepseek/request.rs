//! Translator: `LocalProviderInput` → `DeepSeekChatRequest`.
//!
//! DeepSeek's wire shape is OpenAI-compatible. Two DeepSeek-specific points:
//!
//! 1. **Tool-call `arguments` is stringified JSON** (a `String`, not a
//!    `serde_json::Value`) — matches OpenAI's convention, opposite of Ollama
//!    and Gemini.
//! 2. **`AgentReasoning` proto messages are DROPPED** from outbound history.
//!    DeepSeek's API returns HTTP 400 if `reasoning_content` appears on
//!    inbound `messages`. The reasoning channel is response-only. The decoder
//!    (Task 3) still emits `AgentReasoning` for the UI; this translator never
//!    sends it back.
//!
//! The compaction projection, synthetic user-query anchoring, and final
//! `user_query` append mirror the Ollama translator's structure exactly.

use std::collections::HashMap;

use warp_multi_agent_api as api;

use super::wire::{
    ChatContentPart, ChatMessageContent, DeepSeekChatMessage, DeepSeekChatRequest,
    DeepSeekOutboundToolCall, DeepSeekOutboundToolCallFunction, DeepSeekRole, DeepSeekToolDef,
    DeepSeekToolFunction, ImageUrlSpec,
};
use crate::local_provider::{
    compaction,
    config::LocalProviderConfig,
    prompt,
    request::{
        enabled_local_tools, summarize_tool_call_input, summarize_tool_result, LocalProviderInput,
    },
    tools::{self, LocalTool},
};

pub fn compose_deepseek_chat_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> DeepSeekChatRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tool_definitions_deepseek(&local_tools))
    } else {
        None
    };

    // System prompt as role:"system" message at messages[0] (OpenAI convention).
    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system_prompt = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );

    let mut staged: Vec<StagedMessage> = Vec::new();
    staged.push(StagedMessage::system(system_prompt));

    // Compaction projection: synthesize (user "Continue...", assistant <summary>)
    // pair from the most recent CompletedCompaction and skip pre-tail history.
    let projection = compaction_projection(input);
    let mut mode = match projection.as_ref() {
        None => Mode::RenderAll,
        Some(p) => {
            staged.push(StagedMessage::user_text(&p.continue_prompt));
            staged.push(StagedMessage::assistant_text(&p.summary_text));
            match p.tail_start_id.as_deref() {
                Some(id) => Mode::SkipUntil(id.to_string()),
                None => Mode::DropAll,
            }
        }
    };

    // Synthetic user-query anchoring: emits a role:"user" message immediately
    // before the anchor task-message, restoring user-then-assistant turn order.
    let synthetic_by_anchor: HashMap<&str, &str> = input
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
            if let Some(q) = synthetic_by_anchor.get(proto_msg.id.as_str()) {
                staged.push(StagedMessage::user_text(q));
            }
            push_proto_message(&mut staged, proto_msg);
        }
    }

    backfill_orphaned_tool_calls(&mut staged, &input.action_results);

    // Phase 4c-2: user turn with optional image attachments.
    // DeepSeek's API is OpenAi-compatible — it accepts the same content-array
    // shape (`[{type:"text",...},{type:"image_url",...}]`). Non-image
    // attachments (pdf, audio) are dropped with a warning.
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
                        "DeepSeek adapter: dropping unsupported attachment mime {} \
                         (only image/* is supported on this api_type)",
                        attachment.mime
                    );
                }
            }
            ChatMessageContent::Parts(parts)
        };
        staged.push(StagedMessage {
            role: DeepSeekRole::User,
            content: Some(user_content),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        });
    }

    let messages: Vec<DeepSeekChatMessage> =
        staged.into_iter().map(|s| s.finalize()).collect();

    DeepSeekChatRequest {
        model: cfg.model_id.clone(),
        stream: true,
        messages,
        tools,
    }
}

/// Build the DeepSeek `tools` array. Reuses the v1 JSON Schemas via
/// `tools::schema_for` — the only DeepSeek-specific bit is wrapping in
/// the `{type:"function", function:{...}}` envelope (same as OpenAI/Ollama).
fn tool_definitions_deepseek(enabled: &[LocalTool]) -> Vec<DeepSeekToolDef> {
    enabled
        .iter()
        .filter_map(|t| {
            tools::schema_for(*t).map(|parameters| DeepSeekToolDef {
                kind: "function",
                function: DeepSeekToolFunction {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters,
                },
            })
        })
        .collect()
}

/// Intermediate per-message representation used during walking. Carries
/// the proto `tool_call_id` so the orphan backfill can look up
/// `action_results` and splice the correct content. Stripped to plain
/// `DeepSeekChatMessage` before serialization.
struct StagedMessage {
    role: DeepSeekRole,
    content: Option<ChatMessageContent>,
    tool_calls: Option<Vec<DeepSeekOutboundToolCall>>,
    /// Set when this assistant message represents a proto `Message::ToolCall`.
    /// Drives the backfill's id-keyed action_results lookup.
    proto_tool_call_id: Option<String>,
    /// Set when this is a `role:"tool"` message. The DeepSeek wire shape
    /// includes `tool_call_id` on role:"tool" messages (unlike Ollama which
    /// strips it). Marks the assistant tool_call this satisfies.
    proto_tool_result_id: Option<String>,
}

impl StagedMessage {
    fn system(text: String) -> Self {
        Self {
            role: DeepSeekRole::System,
            content: Some(ChatMessageContent::Text(text)),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        }
    }

    fn user_text(text: &str) -> Self {
        Self {
            role: DeepSeekRole::User,
            content: Some(ChatMessageContent::Text(text.to_string())),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        }
    }

    fn assistant_text(text: &str) -> Self {
        Self {
            role: DeepSeekRole::Assistant,
            content: Some(ChatMessageContent::Text(text.to_string())),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        }
    }

    fn finalize(self) -> DeepSeekChatMessage {
        DeepSeekChatMessage {
            role: self.role,
            content: self.content,
            tool_calls: self.tool_calls,
            // role:"tool" messages carry tool_call_id on the wire; other
            // roles always emit None (skipped via skip_serializing_if).
            tool_call_id: self.proto_tool_result_id,
        }
    }
}

enum Mode {
    RenderAll,
    SkipUntil(String),
    DropAll,
}

struct CompactionProjection {
    continue_prompt: String,
    summary_text: String,
    tail_start_id: Option<String>,
}

fn compaction_projection(input: &LocalProviderInput) -> Option<CompactionProjection> {
    let last = input.compaction_state.completed().last()?;
    let summary_text = last.summary_text.clone()?;
    Some(CompactionProjection {
        continue_prompt: compaction::prompt::build_continue_message(last.overflow),
        summary_text,
        tail_start_id: last.tail_start_id.clone(),
    })
}

fn push_proto_message(out: &mut Vec<StagedMessage>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => out.push(StagedMessage::user_text(&q.query)),
        Some(M::AgentOutput(a)) => out.push(StagedMessage::assistant_text(&a.text)),
        Some(M::ToolCall(call)) => {
            // Skip variants we don't have schemas for.
            if let Some((name, args)) = summarize_tool_call_input(call) {
                // Stringify args — DeepSeek expects function.arguments as
                // a JSON STRING, matching OpenAI's convention (not Ollama's
                // or Gemini's object form).
                let args_string =
                    serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                out.push(StagedMessage {
                    role: DeepSeekRole::Assistant,
                    content: None,
                    tool_calls: Some(vec![DeepSeekOutboundToolCall {
                        id: call.tool_call_id.clone(),
                        kind: "function",
                        function: DeepSeekOutboundToolCallFunction {
                            name,
                            arguments: args_string,
                        },
                    }]),
                    proto_tool_call_id: Some(call.tool_call_id.clone()),
                    proto_tool_result_id: None,
                });
            }
        }
        Some(M::ToolCallResult(result)) => out.push(StagedMessage {
            role: DeepSeekRole::Tool,
            content: Some(ChatMessageContent::Text(summarize_tool_result(result))),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: Some(result.tool_call_id.clone()),
        }),
        // AgentReasoning is DROPPED from outbound history — DeepSeek's API
        // returns HTTP 400 if reasoning_content appears in inbound messages.
        // The decoder still emits AgentReasoning into the proto stream for
        // the UI; the translator just never sends it back.
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}

/// Walk the staged messages list and ensure every assistant
/// `proto_tool_call_id` is followed by a `role:"tool"` message with a
/// matching `proto_tool_result_id`. Inserts a synthetic tool message
/// using `action_results[id]` when present, otherwise the placeholder
/// `"(tool result not available)"`.
fn backfill_orphaned_tool_calls(
    messages: &mut Vec<StagedMessage>,
    action_results: &HashMap<String, String>,
) {
    let mut i = 0;
    while i < messages.len() {
        let Some(tool_call_id) = messages[i].proto_tool_call_id.clone() else {
            i += 1;
            continue;
        };
        // Is the next message a role:tool with the matching id?
        let satisfied_at = if let Some(next) = messages.get(i + 1) {
            if next.proto_tool_result_id.as_deref() == Some(tool_call_id.as_str()) {
                Some(i + 1)
            } else {
                None
            }
        } else {
            None
        };

        if satisfied_at.is_some() {
            // Skip the assistant + the tool result.
            i += 2;
            continue;
        }

        // Splice a synthetic tool message right after the assistant.
        let content = action_results
            .get(&tool_call_id)
            .cloned()
            .unwrap_or_else(|| "(tool result not available)".to_string());
        messages.insert(
            i + 1,
            StagedMessage {
                role: DeepSeekRole::Tool,
                content: Some(ChatMessageContent::Text(content)),
                tool_calls: None,
                proto_tool_call_id: None,
                proto_tool_result_id: Some(tool_call_id),
            },
        );
        i += 2;
    }
}
