//! Ollama native API NDJSON → `warp_multi_agent_api::ResponseEvent` decoder.
//! Mirrors `OpenAiSseAdapter`'s public surface and emitted event shape so
//! the controller doesn't have to know which adapter produced a given
//! stream.
//!
//! Per-chunk dispatch (one `OllamaChatChunk` per NDJSON line):
//!
//! ```text
//! first non-empty chunk → emit Init + BeginTransaction + CreateTask prelude.
//! message.content       → open (first) or append (subsequent) shared
//!                         AgentOutput message. One per turn — Ollama emits
//!                         text in a single logical stream (unlike Anthropic's
//!                         per-block model).
//! message.tool_calls    → emit one AddMessagesToTask{ToolCall} per entry.
//!                         Tool calls arrive complete (no fragmentation);
//!                         the decoder synthesizes a UUID id since Ollama
//!                         doesn't send one.
//! done: true            → capture done_reason + usage; transition to Done.
//!                         The runner calls `finish()` next to drain the
//!                         closing transaction + Finished event.
//! top-level `error`     → record upstream error; transition to Errored.
//! malformed chunk JSON  → record error; transition to Errored.
//! ```
//!
//! `is_terminal()` becomes true on `done: true` or any error. No
//! intermediate "Finishing" state — Ollama doesn't emit content after the
//! done chunk.

use uuid::Uuid;
use warp_multi_agent_api as api;

use super::wire::{OllamaChatChunk, OllamaInboundToolCall};
use crate::local_provider::adapters::proto_helpers::*;

pub struct OllamaDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,
    /// Single shared AgentOutput message id for the entire turn. Mirrors
    /// `OpenAiSseAdapter`'s text_message_id approach — Ollama emits text
    /// in one logical stream per turn (not multiple content blocks).
    text_message_id: Option<String>,
    captured_done_reason: Option<String>,
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

impl Default for OllamaDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaDecoder {
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
            text_message_id: None,
            captured_done_reason: None,
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

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.state, State::Done | State::Errored)
    }

    pub fn record_upstream_error(&mut self, msg: String) {
        if self.upstream_error.is_none() {
            self.upstream_error = Some(msg);
        }
    }

    /// Feed one NDJSON line. `event_name` is ignored — Ollama's stream
    /// has no SSE event-name framing; the discriminator is the body
    /// itself.
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

        let mut out = self.ensure_prelude();

        let chunk: OllamaChatChunk = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                self.state = State::Errored;
                self.upstream_error
                    .get_or_insert_with(|| format!("malformed Ollama NDJSON chunk: {e}"));
                return out;
            }
        };

        if let Some(err_msg) = chunk.error {
            self.upstream_error = Some(err_msg);
            self.state = State::Errored;
            return out;
        }

        if self.captured_model.is_none() {
            if let Some(m) = chunk.model.filter(|s| !s.is_empty()) {
                self.captured_model = Some(m);
            }
        }

        if let Some(message) = chunk.message {
            if !message.content.is_empty() {
                self.append_text(&message.content, &mut out);
            }
            if let Some(tool_calls) = message.tool_calls {
                for tc in tool_calls {
                    emit_tool_call(&self.task_id, &tc, &mut out);
                }
            }
        }

        if chunk.done {
            self.captured_done_reason = chunk.done_reason;
            if let Some(n) = chunk.prompt_eval_count {
                self.captured_input_tokens = n;
            }
            if let Some(n) = chunk.eval_count {
                self.captured_output_tokens = n;
            }
            self.state = State::Done;
        }

        out
    }

    pub fn finish(&mut self) -> Vec<api::ResponseEvent> {
        let mut out = self.ensure_prelude();

        let healthy = self.state == State::Done && self.captured_done_reason.is_some();
        let closing = if healthy {
            client_action_commit()
        } else {
            client_action_rollback()
        };
        out.push(build_client_action_event(closing));

        let reason = match (
            self.captured_done_reason.take(),
            self.upstream_error.take(),
        ) {
            (Some(r), _) => map_ollama_done_reason(&r),
            (None, Some(msg)) => internal_error_reason(&msg),
            (None, None) => internal_error_reason("stream ended without done:true"),
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

    // ---- internals ----

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
                .unwrap_or_else(|| "ollama".to_string()),
            total_input: to_u32(self.captured_input_tokens),
            output: to_u32(self.captured_output_tokens),
            input_cache_read: 0,
            input_cache_write: 0,
            cost_in_cents: 0.0,
        }]
    }
}

fn emit_tool_call(
    task_id: &str,
    tc: &OllamaInboundToolCall,
    out: &mut Vec<api::ResponseEvent>,
) {
    // Ollama doesn't send tool_call ids; synthesize one. The controller
    // keys `action_results` by this id; the translator (request.rs) will
    // omit the id on the way back out per Ollama's native shape.
    let id = format!("ollama-call-{}", Uuid::new_v4());
    let args_json = serde_json::to_string(&tc.function.arguments).unwrap_or_default();
    if let Some(ev) = build_tool_call_event(task_id, &id, &tc.function.name, &args_json) {
        out.push(ev);
    }
}

/// Map Ollama's `done_reason` string to the proto's `StreamFinished.Reason`:
/// - `stop`   → Done
/// - `length` → MaxTokenLimit
/// - `load` / `unload` / anything else → Other
fn map_ollama_done_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "stop" => Reason::Done(Done {}),
        "length" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        _ => Reason::Other(Other {}),
    }
}

// ---- StreamDecoder trait impl ----

impl crate::local_provider::adapters::StreamDecoder for OllamaDecoder {
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
