//! Translator: `LocalProviderInput` → `AnthropicMessagesRequest`.
//!
//! Mirrors the OpenAI translator's structure
//! (`crate::local_provider::request::compose_chat_completion_request`) but
//! emits Anthropic's alternating-role-with-content-blocks shape:
//!
//! - The synthesized system prompt is **lifted to the top-level `system`
//!   field**, never appears in `messages`.
//! - Assistant tool calls become `{type:"tool_use", id, name, input}`
//!   content blocks inside the assistant message — Anthropic has no
//!   separate `tool_calls` field.
//! - Tool-call results become `{type:"tool_result", tool_use_id, content}`
//!   content blocks on a **user-role** message — Anthropic has no
//!   `role:"tool"`.
//! - After flattening proto history into per-message entries, adjacent
//!   same-role entries are **merged** into a single message with
//!   concatenated content blocks. Anthropic rejects consecutive same-role
//!   messages.
//! - Every assistant `tool_use` block must be followed by a matching
//!   `tool_result` block on the next user message. `backfill_orphan_tool_uses`
//!   splices placeholders for ids missing from history's `tool_result`
//!   set (real values come from `input.action_results` when present;
//!   otherwise the placeholder mirrors OpenAI's
//!   `"(tool result not available)"` string).
//!
//! Out of Phase 3a: the OpenAI translator runs an
//! `apply_prune` pass over its `Vec<ChatMessage>` for the Phase A
//! tool-output byte budget. That helper is shaped for OpenAI's wire types;
//! porting it to Anthropic's content-block list is a Phase 4 polish (see
//! `plan-phase-3a.md` §Risks).

use std::collections::{HashMap, HashSet};

use warp_multi_agent_api as api;

use super::wire::{
    AnthropicContentBlock, AnthropicMessage, AnthropicMessagesRequest, AnthropicRole,
    AnthropicToolChoice,
};
use crate::local_provider::{
    compaction,
    config::LocalProviderConfig,
    prompt,
    request::{
        enabled_local_tools, summarize_tool_call_input, summarize_tool_result, LocalProviderInput,
    },
    tools::{tool_definitions_anthropic, LocalTool},
};

/// Build the Anthropic request body for a single turn.
pub fn compose_anthropic_messages_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> AnthropicMessagesRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tool_definitions_anthropic(&local_tools))
    } else {
        None
    };
    let tool_choice = tools.as_ref().map(|_| AnthropicToolChoice::Auto);

    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system = Some(prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    ));

    // Phase B-6 parity: pre-index synthetic user queries by anchor id so the
    // walker can splice each before its anchor proto message.
    let synthetic_by_anchor: HashMap<&str, &str> = input
        .synthetic_user_queries
        .iter()
        .map(|(anchor_id, query)| (anchor_id.as_str(), query.as_str()))
        .collect();

    let mut entries: Vec<RoleAndBlocks> = Vec::new();

    // Compaction projection: when state has a completed summary, splice the
    // synthetic `(user "Continue...", assistant <summary>)` pair at the head
    // and skip pre-`tail_start_id` history. Mirrors OpenAI's behavior.
    let projection = compaction_projection(input);
    let mut mode = match projection.as_ref() {
        None => Mode::RenderAll,
        Some(p) => {
            entries.push(RoleAndBlocks::user_text(&p.continue_prompt));
            entries.push(RoleAndBlocks::assistant_text(&p.summary_text));
            match p.tail_start_id.as_deref() {
                Some(id) => Mode::SkipUntil(id.to_string()),
                None => Mode::DropAll,
            }
        }
    };

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
                entries.push(RoleAndBlocks::user_text(q));
            }
            push_proto_message(&mut entries, proto_msg);
        }
    }

    if let Some(q) = input.user_query.as_deref() {
        entries.push(RoleAndBlocks::user_text(q));
    }

    // Splice missing tool_result blocks **before** merging so per-turn
    // assistant/user pairs stay interleaved. Doing it post-merge collapses
    // back-to-back `Assistant(tool_use)` entries from a multi-turn agent
    // loop into a single assistant message, which loses the (assistant, user
    // tool_result, assistant, user tool_result) shape Anthropic expects.
    let with_tool_results = splice_missing_tool_results(entries, &input.action_results);
    let final_messages = merge_alternating(with_tool_results);

    AnthropicMessagesRequest {
        model: cfg.model_id.clone(),
        max_tokens: resolve_max_tokens(cfg),
        system,
        messages: final_messages,
        tools,
        tool_choice,
        stream: true,
    }
}

/// Resolve `max_tokens` from the optional `context_window` setting.
///
/// Anthropic requires `max_tokens` on every Messages API request. We pick a
/// conservative value:
/// - `None` or `< 8192`: **4096** (Anthropic docs' recommended default).
/// - `>= 8192`: `min(context_window / 4, 8192)` — quarter of the window
///   capped at 8K. The cap matches Claude Sonnet/Haiku's per-turn output
///   ceiling (without the `output-128k-2025-02-19` beta opt-in we don't
///   send today).
///
/// Phase 4 polish: expose `max_output_tokens` per model in the provider
/// settings so users can opt into longer outputs.
pub(crate) fn resolve_max_tokens(cfg: &LocalProviderConfig) -> u32 {
    match cfg.context_window {
        Some(n) if n >= 8192 => (n / 4).min(8192),
        _ => 4096,
    }
}

/// Intermediate per-proto-message entry. Walked into a flat list, then
/// collapsed by `merge_alternating` into a `Vec<AnthropicMessage>`.
struct RoleAndBlocks {
    role: AnthropicRole,
    blocks: Vec<AnthropicContentBlock>,
}

impl RoleAndBlocks {
    fn user_text(text: &str) -> Self {
        Self {
            role: AnthropicRole::User,
            blocks: vec![AnthropicContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }
    fn assistant_text(text: &str) -> Self {
        Self {
            role: AnthropicRole::Assistant,
            blocks: vec![AnthropicContentBlock::Text {
                text: text.to_string(),
            }],
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

fn push_proto_message(out: &mut Vec<RoleAndBlocks>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => out.push(RoleAndBlocks::user_text(&q.query)),
        Some(M::AgentOutput(a)) => out.push(RoleAndBlocks::assistant_text(&a.text)),
        Some(M::ToolCall(call)) => {
            // Skip variants we don't have schemas for (matches OpenAI
            // behavior — see summarize_tool_call_input docs).
            if let Some((name, input)) = summarize_tool_call_input(call) {
                out.push(RoleAndBlocks {
                    role: AnthropicRole::Assistant,
                    blocks: vec![AnthropicContentBlock::ToolUse {
                        id: call.tool_call_id.clone(),
                        name,
                        input,
                    }],
                });
            }
        }
        Some(M::ToolCallResult(result)) => {
            out.push(RoleAndBlocks {
                role: AnthropicRole::User,
                blocks: vec![AnthropicContentBlock::ToolResult {
                    tool_use_id: result.tool_call_id.clone(),
                    content: summarize_tool_result(result),
                    is_error: None,
                }],
            });
        }
        // AgentReasoning is NOT replayed in history (matches the OpenAI
        // translator's drop-from-history behavior). Other Message variants
        // are server-side metadata the local model doesn't need.
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}

/// Collapse adjacent entries with the same role into a single
/// `AnthropicMessage` with concatenated content blocks. Anthropic rejects
/// consecutive same-role messages with HTTP 400.
fn merge_alternating(entries: Vec<RoleAndBlocks>) -> Vec<AnthropicMessage> {
    let mut out: Vec<AnthropicMessage> = Vec::with_capacity(entries.len());
    for entry in entries {
        match out.last_mut() {
            Some(last) if last.role == entry.role => {
                last.content.extend(entry.blocks);
            }
            _ => out.push(AnthropicMessage {
                role: entry.role,
                content: entry.blocks,
            }),
        }
    }
    out
}

/// For each assistant entry containing `tool_use` blocks, ensure the next
/// entry is a `User` with a matching `tool_result` block per id. Inserts
/// User entries (or prepends placeholder blocks to an existing next-User)
/// for missing ids. Runs **before** `merge_alternating` so per-turn
/// assistant/user-result pairs stay interleaved instead of collapsing into
/// a single assistant message when a multi-turn agent loop emits N
/// consecutive `Assistant(tool_use)` entries.
///
/// Anthropic returns HTTP 400 if any `tool_use` lacks a matching
/// `tool_result` on the next user message.
fn splice_missing_tool_results(
    mut entries: Vec<RoleAndBlocks>,
    action_results: &HashMap<String, String>,
) -> Vec<RoleAndBlocks> {
    let mut i = 0;
    while i < entries.len() {
        let needs_check = entries[i].role == AnthropicRole::Assistant
            && entries[i]
                .blocks
                .iter()
                .any(|b| matches!(b, AnthropicContentBlock::ToolUse { .. }));
        if !needs_check {
            i += 1;
            continue;
        }
        let tool_use_ids: Vec<String> = entries[i]
            .blocks
            .iter()
            .filter_map(|b| match b {
                AnthropicContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        let satisfied: HashSet<String> = match entries.get(i + 1) {
            Some(next) if next.role == AnthropicRole::User => next
                .blocks
                .iter()
                .filter_map(|b| match b {
                    AnthropicContentBlock::ToolResult { tool_use_id, .. } => {
                        Some(tool_use_id.clone())
                    }
                    _ => None,
                })
                .collect(),
            _ => HashSet::new(),
        };

        let missing: Vec<String> = tool_use_ids
            .into_iter()
            .filter(|id| !satisfied.contains(id))
            .collect();
        if missing.is_empty() {
            i += 1;
            continue;
        }

        let placeholder_blocks: Vec<AnthropicContentBlock> = missing
            .into_iter()
            .map(|id| {
                let content = action_results
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "(tool result not available)".to_string());
                AnthropicContentBlock::ToolResult {
                    tool_use_id: id,
                    content,
                    is_error: None,
                }
            })
            .collect();

        match entries.get(i + 1).map(|e| e.role) {
            Some(AnthropicRole::User) => {
                // Prepend placeholders to the existing user's blocks so
                // tool_results land before any subsequent text block.
                let next = &mut entries[i + 1];
                let mut new_blocks = placeholder_blocks;
                new_blocks.append(&mut next.blocks);
                next.blocks = new_blocks;
            }
            _ => {
                entries.insert(
                    i + 1,
                    RoleAndBlocks {
                        role: AnthropicRole::User,
                        blocks: placeholder_blocks,
                    },
                );
            }
        }
        i += 1;
    }
    entries
}
