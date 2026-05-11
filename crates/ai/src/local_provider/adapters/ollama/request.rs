//! Translator: `LocalProviderInput` → `OllamaChatRequest`.
//!
//! Mirrors the OpenAI translator's structure (`crate::local_provider::request::
//! compose_chat_completion_request`) but emits Ollama's native shape:
//!
//! - **System prompt** is a `role:"system"` message at the head of the
//!   messages list (Ollama accepts both this form and a top-level `system`
//!   field; the message form keeps the translator pipeline uniform with
//!   the OpenAI one).
//! - **Tool calls** in assistant history messages use the native shape:
//!   `tool_calls[].function.arguments` is a JSON **object** (via
//!   `summarize_tool_call_input`, added in Phase 3a), no `id` or
//!   `type:"function"` fields on the tool_call.
//! - **Tool results** stay on `role:"tool"` messages (vs Anthropic's
//!   tool_result content blocks on user messages).
//! - **No alternation merge** — Ollama tolerates consecutive same-role
//!   messages (unlike Anthropic).
//! - **`options.num_ctx`** is threaded from `cfg.context_window` to size
//!   the KV cache appropriately for large-context models.
//!
//! Orphan tool-call backfill mirrors the OpenAI translator's logic:
//! every assistant `tool_calls` entry is followed by matching `role:"tool"`
//! messages. We carry the proto `tool_call_id` through the walker in an
//! internal `StagedMessage` representation so backfill can look real
//! values up in `action_results`; the id is stripped before serialization
//! (the wire shape doesn't include it). Ollama is more lenient than
//! OpenAI here — it tolerates missing tool results — but we backfill for
//! cross-adapter parity and to keep `action_results` usable on multi-turn
//! loops.

use std::collections::HashMap;

use warp_multi_agent_api as api;

use super::wire::{
    OllamaChatMessage, OllamaChatRequest, OllamaOptions, OllamaOutboundToolCall,
    OllamaOutboundToolCallFunction, OllamaRole, OllamaToolDef, OllamaToolFunction,
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

pub fn compose_ollama_chat_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> OllamaChatRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tool_definitions_ollama(&local_tools))
    } else {
        None
    };

    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system_prompt = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );

    let mut staged: Vec<StagedMessage> = Vec::new();
    staged.push(StagedMessage::system(system_prompt));

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

    if let Some(q) = input.user_query.as_deref() {
        staged.push(StagedMessage::user_text(q));
    }

    let messages: Vec<OllamaChatMessage> = staged.into_iter().map(|s| s.finalize()).collect();

    let options = cfg
        .context_window
        .filter(|n| *n > 0)
        .map(|num_ctx| OllamaOptions {
            num_ctx: Some(num_ctx),
        });

    OllamaChatRequest {
        model: cfg.model_id.clone(),
        stream: true,
        messages,
        tools,
        options,
    }
}

/// Build the Ollama `tools` array. Reuses the v1 JSON Schemas via
/// `tools::schema_for` — the only Ollama-specific bit is wrapping in
/// the `{type:"function", function:{...}}` envelope.
fn tool_definitions_ollama(enabled: &[LocalTool]) -> Vec<OllamaToolDef> {
    enabled
        .iter()
        .filter_map(|t| {
            tools::schema_for(*t).map(|parameters| OllamaToolDef {
                kind: "function",
                function: OllamaToolFunction {
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
/// `OllamaChatMessage` before serialization — Ollama doesn't accept an
/// `id` field on tool_calls or `tool_call_id` on tool messages.
struct StagedMessage {
    role: OllamaRole,
    content: String,
    tool_calls: Option<Vec<OllamaOutboundToolCall>>,
    /// Set when this assistant message represents a proto `Message::ToolCall`.
    /// Drives the backfill's id-keyed action_results lookup.
    proto_tool_call_id: Option<String>,
    /// Set when this is a `role:"tool"` message. Marks the assistant
    /// tool_call this satisfies.
    proto_tool_result_id: Option<String>,
}

impl StagedMessage {
    fn system(text: String) -> Self {
        Self {
            role: OllamaRole::System,
            content: text,
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        }
    }
    fn user_text(text: &str) -> Self {
        Self {
            role: OllamaRole::User,
            content: text.to_string(),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        }
    }
    fn assistant_text(text: &str) -> Self {
        Self {
            role: OllamaRole::Assistant,
            content: text.to_string(),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: None,
        }
    }

    fn finalize(self) -> OllamaChatMessage {
        OllamaChatMessage {
            role: self.role,
            content: self.content,
            tool_calls: self.tool_calls,
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
            // Skip variants we don't have schemas for (matches OpenAI/
            // Anthropic translator behavior).
            if let Some((name, args)) = summarize_tool_call_input(call) {
                out.push(StagedMessage {
                    role: OllamaRole::Assistant,
                    content: String::new(),
                    tool_calls: Some(vec![OllamaOutboundToolCall {
                        function: OllamaOutboundToolCallFunction {
                            name,
                            arguments: args,
                        },
                    }]),
                    proto_tool_call_id: Some(call.tool_call_id.clone()),
                    proto_tool_result_id: None,
                });
            }
        }
        Some(M::ToolCallResult(result)) => out.push(StagedMessage {
            role: OllamaRole::Tool,
            content: summarize_tool_result(result),
            tool_calls: None,
            proto_tool_call_id: None,
            proto_tool_result_id: Some(result.tool_call_id.clone()),
        }),
        // AgentReasoning is dropped (matches OpenAi/Anthropic — only
        // final assistant text persists across turns).
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}

/// Walk the staged messages list and ensure every assistant
/// `proto_tool_call_id` is followed by a `role:"tool"` message with a
/// matching `proto_tool_result_id`. Inserts a synthetic tool message
/// using `action_results[id]` when present, otherwise the placeholder
/// `"(tool result not available)"`.
///
/// Unlike Anthropic's pre-merge splicer, this runs after the full walk
/// because Ollama doesn't merge same-role messages — the staged list IS
/// the final shape.
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
                role: OllamaRole::Tool,
                content,
                tool_calls: None,
                proto_tool_call_id: None,
                proto_tool_result_id: Some(tool_call_id),
            },
        );
        i += 2;
    }
}
