//! DeepSeek `/chat/completions` SSE → `warp_multi_agent_api::ResponseEvent` decoder.
//! Mirrors `GeminiSseDecoder`'s public surface and emitted event shape so the
//! controller doesn't have to know which adapter produced a given stream.
//!
//! Per-chunk dispatch (one anonymous `data:` SSE chunk per frame, same framing
//! as OpenAI's `/chat/completions` stream; `[DONE]` is the terminator):
//!
//! ```text
//! first non-empty chunk  → emit Init + BeginTransaction + CreateTask prelude.
//! delta.reasoning_content → open (first) or append (subsequent) shared
//!                          AgentReasoning message. One per turn — streams
//!                          alongside the AgentOutput text channel.
//! delta.content          → open (first) or append (subsequent) shared
//!                          AgentOutput message. One per turn.
//! delta.tool_calls       → accumulate fragments by index. Emitted on finish().
//! choices[0].finish_reason → captured; [DONE] arrives separately (like OpenAI).
//! usage                  → may appear on the final chunk; last-seen wins via
//!                          .max() (defensive).
//! top-level `error`      → record upstream error; transition to Errored.
//! malformed chunk JSON   → record error; transition to Errored.
//! [DONE]                 → transition to Done.
//! ```
//!
//! `is_terminal()` becomes true after `[DONE]` or any error. The runner calls
//! `finish()` to drain the closing transaction + Finished event.

use uuid::Uuid;
use warp_multi_agent_api as api;

use super::wire::{DeepSeekChatChunk, DeepSeekStreamToolCall};
use crate::local_provider::adapters::proto_helpers::*;

pub struct DeepSeekSseDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,

    /// One shared open AgentReasoning message across the turn — opens on
    /// first `delta.reasoning_content`, appends on subsequent reasoning
    /// chunks. Never closed explicitly; the controller groups it with the
    /// AgentOutput at render time.
    reasoning_message_id: Option<String>,

    /// One shared open AgentOutput message across the turn — opens on
    /// first `delta.content`, appends on subsequent text chunks.
    text_message_id: Option<String>,

    /// Pending tool-call accumulator. Tool-call fragments arrive
    /// incrementally; each call's name + arguments concatenate as
    /// fragments arrive on the SAME `index`. See OpenAI's equivalent.
    pending_tool_calls: Vec<PendingToolCall>,

    captured_finish_reason: Option<String>,
    captured_model: Option<String>,
    captured_input_tokens: u64,
    captured_output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Streaming,
    Done,
    Errored,
}

/// Per-call accumulator. Mirrors OpenAI's PendingToolCall pattern.
struct PendingToolCall {
    index: u32,
    id: Option<String>,
    name: Option<String>,
    /// Accumulating string fragment of JSON arguments.
    arguments: String,
}

impl Default for DeepSeekSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepSeekSseDecoder {
    pub fn new() -> Self {
        Self::with_ids(
            format!("local:{}", Uuid::new_v4()),
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            format!("local-task:{}", Uuid::new_v4()),
        )
    }

    pub fn with_ids(
        conversation_id: String,
        request_id: String,
        run_id: String,
        task_id: String,
    ) -> Self {
        Self {
            state: State::Streaming,
            task_id,
            conversation_id,
            request_id,
            run_id,
            upstream_error: None,
            sent_init: false,
            sent_begin: false,
            sent_create_task: false,
            reasoning_message_id: None,
            text_message_id: None,
            pending_tool_calls: Vec::new(),
            captured_finish_reason: None,
            captured_model: None,
            captured_input_tokens: 0,
            captured_output_tokens: 0,
        }
    }

    /// Suppress the synthetic `CreateTask` emission. Used by the HTTP
    /// runner after the first turn — the optimistic root task has already
    /// been upgraded to a server-created task.
    pub fn skip_create_task(&mut self) {
        self.sent_create_task = true;
    }

    /// Feed one SSE data line. `event_name` is ignored — DeepSeek's SSE
    /// stream uses anonymous `data:` chunks identical to OpenAI's framing.
    pub fn feed_event(
        &mut self,
        _event_name: Option<&str>,
        data: &str,
    ) -> Vec<api::ResponseEvent> {
        if matches!(self.state, State::Done | State::Errored) {
            return vec![];
        }
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return vec![];
        }

        // [DONE] terminator (same as OpenAI).
        if trimmed == "[DONE]" {
            self.state = State::Done;
            return vec![];
        }

        let mut out = self.ensure_prelude();

        let chunk: DeepSeekChatChunk = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                self.state = State::Errored;
                self.upstream_error
                    .get_or_insert_with(|| format!("malformed DeepSeek chunk: {e}"));
                return out;
            }
        };

        if let Some(err) = chunk.error {
            let kind = if err.kind.is_empty() {
                "error".to_string()
            } else {
                err.kind
            };
            self.upstream_error = Some(format!("{kind}: {}", err.message));
            self.state = State::Errored;
            return out;
        }

        if self.captured_model.is_none() {
            if let Some(m) = chunk.model.filter(|s| !s.is_empty()) {
                self.captured_model = Some(m);
            }
        }

        if let Some(usage) = chunk.usage {
            self.captured_input_tokens =
                usage.prompt_tokens.max(self.captured_input_tokens);
            self.captured_output_tokens =
                usage.completion_tokens.max(self.captured_output_tokens);
        }

        if let Some(choice) = chunk.choices.into_iter().next() {
            if let Some(delta) = choice.delta {
                // Reasoning channel — open/append AgentReasoning.
                if let Some(reasoning) = delta.reasoning_content {
                    if !reasoning.is_empty() {
                        self.append_reasoning(&reasoning, &mut out);
                    }
                }
                // Content channel — open/append AgentOutput.
                if let Some(content) = delta.content {
                    if !content.is_empty() {
                        self.append_text(&content, &mut out);
                    }
                }
                // Tool calls — accumulate fragments by index.
                if let Some(tool_calls) = delta.tool_calls {
                    for tc in tool_calls {
                        self.absorb_tool_call_fragment(tc, &mut out);
                    }
                }
            }
            if let Some(reason) = choice.finish_reason {
                self.captured_finish_reason = Some(reason);
                // Don't transition to Done here — [DONE] comes in a separate
                // SSE event afterward (same as OpenAI's two-step pattern).
            }
        }

        out
    }

    /// Called by the runner when the SSE stream closes or `[DONE]` has arrived.
    /// Flushes pending tool calls, then emits the closing transaction + Finished.
    pub fn finish(&mut self) -> Vec<api::ResponseEvent> {
        let mut out = self.ensure_prelude();

        // Flush any pending tool-call accumulator.
        let pending: Vec<PendingToolCall> = std::mem::take(&mut self.pending_tool_calls);
        for tc in pending {
            let id = tc.id.unwrap_or_else(|| Uuid::new_v4().to_string());
            let name = tc.name.unwrap_or_default();
            if let Some(ev) = build_tool_call_event(&self.task_id, &id, &name, &tc.arguments) {
                out.push(ev);
            }
        }

        let healthy = self.state == State::Done && self.captured_finish_reason.is_some();
        let closing = if healthy {
            client_action_commit()
        } else {
            client_action_rollback()
        };
        out.push(build_client_action_event(closing));

        let reason = match (
            self.captured_finish_reason.take(),
            self.upstream_error.take(),
        ) {
            (Some(r), _) => map_deepseek_finish_reason(&r),
            (None, Some(msg)) => internal_error_reason(&msg),
            (None, None) => internal_error_reason("stream ended without finish_reason"),
        };

        let token_usage = self.token_usage_proto();
        out.push(api::ResponseEvent {
            r#type: Some(api::response_event::Type::Finished(
                api::response_event::StreamFinished {
                    reason: Some(reason),
                    token_usage,
                    ..Default::default()
                },
            )),
        });

        self.state = State::Done;
        out
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.state, State::Done | State::Errored)
    }

    pub fn record_upstream_error(&mut self, msg: String) {
        self.upstream_error.get_or_insert(msg);
    }

    // ---- private helpers ----

    /// Lazily emits Init + BeginTransaction + CreateTask on first non-empty feed.
    fn ensure_prelude(&mut self) -> Vec<api::ResponseEvent> {
        let mut out = Vec::new();
        if !self.sent_init {
            out.push(api::ResponseEvent {
                r#type: Some(api::response_event::Type::Init(
                    api::response_event::StreamInit {
                        conversation_id: self.conversation_id.clone(),
                        request_id: self.request_id.clone(),
                        run_id: self.run_id.clone(),
                    },
                )),
            });
            self.sent_init = true;
        }
        if !self.sent_begin {
            out.push(build_client_action_event(client_action_begin()));
            self.sent_begin = true;
        }
        if !self.sent_create_task {
            out.push(build_client_action_event(client_action_create_task(
                &self.task_id,
            )));
            self.sent_create_task = true;
        }
        out
    }

    /// Opens or appends the shared `AgentOutput` message for this turn.
    fn append_text(&mut self, text: &str, out: &mut Vec<api::ResponseEvent>) {
        if self.text_message_id.is_none() {
            let message_id = Uuid::new_v4().to_string();
            self.text_message_id = Some(message_id.clone());
            out.push(build_client_action_event(
                api::client_action::Action::AddMessagesToTask(
                    api::client_action::AddMessagesToTask {
                        task_id: self.task_id.clone(),
                        messages: vec![build_kind_message(
                            &message_id,
                            MessageKind::AgentOutput,
                            text,
                        )],
                    },
                ),
            ));
            return;
        }
        let message_id = self.text_message_id.as_ref().expect("just checked").clone();
        out.push(build_client_action_event(
            api::client_action::Action::AppendToMessageContent(
                api::client_action::AppendToMessageContent {
                    task_id: self.task_id.clone(),
                    message: Some(build_kind_message(
                        &message_id,
                        MessageKind::AgentOutput,
                        text,
                    )),
                    mask: Some(prost_types::FieldMask {
                        paths: vec!["agent_output.text".to_string()],
                    }),
                },
            ),
        ));
    }

    /// Opens or appends the shared `AgentReasoning` message for this turn.
    /// Structurally identical to `append_text` but uses `MessageKind::AgentReasoning`
    /// and tracks `self.reasoning_message_id` separately from `text_message_id`.
    fn append_reasoning(&mut self, text: &str, out: &mut Vec<api::ResponseEvent>) {
        if self.reasoning_message_id.is_none() {
            let message_id = Uuid::new_v4().to_string();
            self.reasoning_message_id = Some(message_id.clone());
            out.push(build_client_action_event(
                api::client_action::Action::AddMessagesToTask(
                    api::client_action::AddMessagesToTask {
                        task_id: self.task_id.clone(),
                        messages: vec![build_kind_message(
                            &message_id,
                            MessageKind::AgentReasoning,
                            text,
                        )],
                    },
                ),
            ));
            return;
        }
        let message_id = self
            .reasoning_message_id
            .as_ref()
            .expect("just checked")
            .clone();
        out.push(build_client_action_event(
            api::client_action::Action::AppendToMessageContent(
                api::client_action::AppendToMessageContent {
                    task_id: self.task_id.clone(),
                    message: Some(build_kind_message(
                        &message_id,
                        MessageKind::AgentReasoning,
                        text,
                    )),
                    mask: Some(prost_types::FieldMask {
                        paths: vec!["agent_reasoning.reasoning".to_string()],
                    }),
                },
            ),
        ));
    }

    /// Accumulates tool-call fragments by index. No event emitted yet —
    /// calls are emitted in `finish()`. Mirrors OpenAI's by-index pattern.
    fn absorb_tool_call_fragment(
        &mut self,
        tc: DeepSeekStreamToolCall,
        _out: &mut Vec<api::ResponseEvent>,
    ) {
        let slot = self
            .pending_tool_calls
            .iter_mut()
            .find(|p| p.index == tc.index);
        let slot = if let Some(existing) = slot {
            existing
        } else {
            self.pending_tool_calls.push(PendingToolCall {
                index: tc.index,
                id: None,
                name: None,
                arguments: String::new(),
            });
            self.pending_tool_calls.last_mut().expect("just pushed")
        };

        if let Some(id) = tc.id.filter(|s| !s.is_empty()) {
            slot.id = Some(id);
        }
        if let Some(ref func) = tc.function {
            if let Some(name) = func.name.as_deref().filter(|s| !s.is_empty()) {
                slot.name = Some(name.to_string());
            }
            if let Some(args) = &func.arguments {
                slot.arguments.push_str(args);
            }
        }
    }

    fn token_usage_proto(&self) -> Vec<api::response_event::stream_finished::TokenUsage> {
        use api::response_event::stream_finished::TokenUsage;
        if self.captured_input_tokens == 0 && self.captured_output_tokens == 0 {
            return Vec::new();
        }
        let to_u32 = |n: u64| -> u32 { n.try_into().unwrap_or(u32::MAX) };
        vec![TokenUsage {
            model_id: self
                .captured_model
                .clone()
                .unwrap_or_else(|| "deepseek".to_string()),
            total_input: to_u32(self.captured_input_tokens),
            output: to_u32(self.captured_output_tokens),
            input_cache_read: 0,
            input_cache_write: 0,
            cost_in_cents: 0.0,
        }]
    }
}

// ---- StreamDecoder trait impl ----

impl crate::local_provider::adapters::StreamDecoder for DeepSeekSseDecoder {
    fn feed_event(
        &mut self,
        event_name: Option<&str>,
        data: &str,
    ) -> Vec<api::ResponseEvent> {
        Self::feed_event(self, event_name, data)
    }
    fn finish(&mut self) -> Vec<api::ResponseEvent> {
        Self::finish(self)
    }
    fn is_terminal(&self) -> bool {
        Self::is_terminal(self)
    }
    fn record_upstream_error(&mut self, msg: String) {
        Self::record_upstream_error(self, msg)
    }
}

/// Map DeepSeek's `finish_reason` string to the proto's `StreamFinished.Reason`.
///
/// String match — `_ =>` wildcard is acceptable here per the established precedent
/// in `map_ollama_done_reason` and `map_gemini_finish_reason`. The full set of
/// DeepSeek finish reasons cannot be known at compile time.
///
/// Known mappings:
/// - `stop` / `tool_calls`  → Done
/// - `length`               → MaxTokenLimit
/// - anything else          → Other (Phase 4 polish can split content_filter into Refused)
fn map_deepseek_finish_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "stop" | "tool_calls" => Reason::Done(Done {}),
        "length" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        _ => Reason::Other(Other {}),
    }
}
