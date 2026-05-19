//! Translator: `LocalProviderInput` → `GeminiGenerateRequest`.
//!
//! Mirrors the Ollama translator's structure
//! (`crate::local_provider::adapters::ollama::request::compose_ollama_chat_request`)
//! but emits Gemini's native `:streamGenerateContent` body shape:
//!
//! - **System prompt** is lifted to the top-level `systemInstruction` field.
//!   Gemini rejects `role:system` in the `contents` array.
//! - **Role vocabulary** is `user` / `model` (not `assistant`).
//! - **Tool calls** become `{functionCall: {name, args}}` parts on a `model`
//!   message. `args` is a JSON object (same as Ollama; opposite of OpenAI).
//! - **Tool results** become `{functionResponse: {name, response}}` parts on a
//!   `user` message. Gemini matches `functionResponse` to its prior
//!   `functionCall` by **name** (not id), so we carry a running
//!   `tool_call_id → name` map as history is walked.
//! - **Adjacent same-role merging** — folding consecutive same-role entries
//!   keeps the body clean. Gemini accepts consecutive same-role messages but
//!   the merge produces cleaner `parts` arrays and mirrors Anthropic's behavior.

use std::collections::HashMap;

use warp_multi_agent_api as api;

use super::wire::{
    GeminiContent, GeminiGenerateRequest, GeminiGenerationConfig, GeminiInlineData,
    GeminiInlineDataPart, GeminiOutboundFunctionCall, GeminiOutboundFunctionCallPart,
    GeminiOutboundFunctionResponse, GeminiOutboundFunctionResponsePart, GeminiOutboundPart,
    GeminiRole, GeminiSystemInstruction, GeminiTextPart, GeminiToolEnvelope,
    GeminiFunctionDeclaration,
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

pub fn compose_gemini_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> GeminiGenerateRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(vec![GeminiToolEnvelope {
            function_declarations: function_declarations_gemini(&local_tools),
        }])
    } else {
        None
    };

    // System prompt lifts to top-level systemInstruction.
    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system_text = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );
    let system_instruction = if system_text.trim().is_empty() {
        None
    } else {
        Some(GeminiSystemInstruction {
            parts: vec![GeminiTextPart { text: system_text }],
        })
    };

    let mut contents: Vec<GeminiContent> = Vec::new();

    // Running map of tool_call_id → function name. Gemini matches
    // functionResponse to functionCall by name (not id), so we populate
    // this as ToolCall messages are walked and look it up when a matching
    // ToolCallResult arrives.
    let mut tool_call_names: HashMap<String, String> = HashMap::new();

    let projection = compaction_projection(input);
    let mut mode = match projection.as_ref() {
        None => Mode::RenderAll,
        Some(p) => {
            // Synthetic (user "Continue...", model <summary>) pair at head.
            // Mirrors the Ollama translator's compaction-projection block,
            // substituting GeminiContent for StagedMessage.
            contents.push(GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                    text: p.continue_prompt.clone(),
                })],
            });
            contents.push(GeminiContent {
                role: GeminiRole::Model,
                parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                    text: p.summary_text.clone(),
                })],
            });
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
                contents.push(GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                        text: q.to_string(),
                    })],
                });
            }
            push_proto_message(&mut contents, &mut tool_call_names, proto_msg);
        }
    }

    if let Some(q) = input.user_query.as_deref() {
        let mut parts = vec![GeminiOutboundPart::Text(GeminiTextPart {
            text: q.to_string(),
        })];
        for attachment in &input.attachments {
            if attachment.is_image() || attachment.is_pdf() || attachment.is_audio() {
                parts.push(GeminiOutboundPart::InlineData(GeminiInlineDataPart {
                    inline_data: GeminiInlineData {
                        mime_type: attachment.mime.clone(),
                        data: crate::attachments::encode_base64(&attachment.bytes),
                    },
                }));
            } else {
                log::warn!(
                    "Gemini adapter: dropping attachment with unrecognized mime {}",
                    attachment.mime
                );
            }
        }
        contents.push(GeminiContent {
            role: GeminiRole::User,
            parts,
        });
    }

    // Adjacent same-role merge. Gemini accepts consecutive same-role messages
    // but folding them produces cleaner bodies.
    //
    // NOTE: Anthropic's translator has an equivalent `merge_alternating` pass;
    // this is the second occurrence of the same pattern. Phase 4 polish can
    // hoist both sites (plus any future DeepSeek adapter) into a shared helper.
    merge_adjacent_same_role(&mut contents);

    GeminiGenerateRequest {
        system_instruction,
        contents,
        tools,
        generation_config: GeminiGenerationConfig::default(),
    }
}

fn function_declarations_gemini(enabled: &[LocalTool]) -> Vec<GeminiFunctionDeclaration> {
    enabled
        .iter()
        .filter_map(|t| {
            tools::schema_for(*t).map(|parameters| GeminiFunctionDeclaration {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters,
            })
        })
        .collect()
}

fn push_proto_message(
    out: &mut Vec<GeminiContent>,
    tool_call_names: &mut HashMap<String, String>,
    proto_msg: &api::Message,
) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => out.push(GeminiContent {
            role: GeminiRole::User,
            parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                text: q.query.clone(),
            })],
        }),
        Some(M::AgentOutput(a)) => out.push(GeminiContent {
            role: GeminiRole::Model,
            parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                text: a.text.clone(),
            })],
        }),
        Some(M::ToolCall(call)) => {
            if let Some((name, args)) = summarize_tool_call_input(call) {
                // Record tool_call_id → name so the matching ToolCallResult
                // can look up the function name (Gemini matches by name, not id).
                tool_call_names.insert(call.tool_call_id.clone(), name.clone());
                out.push(GeminiContent {
                    role: GeminiRole::Model,
                    parts: vec![GeminiOutboundPart::FunctionCall(
                        GeminiOutboundFunctionCallPart {
                            function_call: GeminiOutboundFunctionCall { name, args },
                        },
                    )],
                });
            }
        }
        Some(M::ToolCallResult(result)) => {
            let rendered = summarize_tool_result(result);
            // Gemini matches functionResponse to the prior functionCall by
            // name (not id). Look up the function name from the running map.
            let function_name = tool_call_names
                .get(&result.tool_call_id)
                .cloned()
                .unwrap_or_default();
            out.push(GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiOutboundPart::FunctionResponse(
                    GeminiOutboundFunctionResponsePart {
                        function_response: GeminiOutboundFunctionResponse {
                            name: function_name,
                            response: serde_json::json!({ "content": rendered }),
                        },
                    },
                )],
            });
        }
        // AgentReasoning is dropped on the request side (it's transient
        // model state, not history that should be replayed). Other proto
        // variants we don't handle stay silently dropped — matches
        // Ollama/OpenAI/Anthropic translators.
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}

/// Collapse adjacent entries with the same role into a single `GeminiContent`
/// with concatenated `parts`. Gemini accepts consecutive same-role messages but
/// folding them produces cleaner bodies.
///
/// NOTE: Anthropic's translator has an equivalent pass (`merge_alternating`);
/// this is the second occurrence. Phase 4 polish can dedupe all three sites
/// (Anthropic + Gemini + future DeepSeek) into a shared helper.
fn merge_adjacent_same_role(out: &mut Vec<GeminiContent>) {
    let mut i = 1;
    while i < out.len() {
        if out[i].role == out[i - 1].role {
            let mut tail_parts = std::mem::take(&mut out[i].parts);
            out[i - 1].parts.append(&mut tail_parts);
            out.remove(i);
        } else {
            i += 1;
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
