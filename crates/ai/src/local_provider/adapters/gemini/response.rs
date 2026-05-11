//! Gemini native SSE → `warp_multi_agent_api::ResponseEvent` decoder.
//! Mirrors `OllamaDecoder`'s public surface and emitted event shape so
//! the controller doesn't have to know which adapter produced a given
//! stream.
//!
//! Per-chunk dispatch (one anonymous `data:` SSE chunk per frame, each a
//! complete `GenerateContentResponse` partial):
//!
//! ```text
//! first non-empty chunk → emit Init + BeginTransaction + CreateTask prelude.
//! parts[].Text         → open (first) or append (subsequent) shared
//!                        AgentOutput message. One per turn — Gemini emits
//!                        text in a single logical stream per turn (same
//!                        pattern as OpenAI/Ollama; not the per-block model
//!                        used by Anthropic).
//! parts[].FunctionCall → emit one AddMessagesToTask{ToolCall} per entry.
//!                        Function calls arrive complete (no fragmentation);
//!                        the decoder synthesizes a UUID id since Gemini
//!                        doesn't send one.
//! candidates[0].finishReason → capture finish_reason; transition to Done.
//!                        The runner calls `finish()` next to drain the
//!                        closing transaction + Finished event.
//! usageMetadata        → may appear on any chunk; last-seen wins per field
//!                        via `.max()` (defensive — Gemini docs say final-only
//!                        in practice, but the streaming spec allows any chunk).
//! top-level `error`    → record upstream error; transition to Errored.
//! malformed chunk JSON → record error; transition to Errored.
//! ```
//!
//! `is_terminal()` becomes true on `finishReason` present or any error. No
//! intermediate "Finishing" state — Gemini doesn't emit content after the
//! final chunk.

use uuid::Uuid;
use warp_multi_agent_api as api;

use super::wire::{GeminiInboundFunctionCall, GeminiInboundPart, GeminiStreamChunk};
use crate::local_provider::adapters::proto_helpers::*;

pub struct GeminiSseDecoder {
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
    /// `OllamaDecoder`'s text_message_id approach — Gemini emits text in
    /// one logical stream per turn (not multiple content blocks).
    text_message_id: Option<String>,
    captured_finish_reason: Option<String>,
    /// Gemini stream chunks do not carry a model name; stays None. The
    /// controller falls back to cfg.model_id; `token_usage_proto` falls back
    /// to the literal "gemini" string.
    captured_input_tokens: u64,
    captured_output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Streaming,
    Done,
    Errored,
}

impl Default for GeminiSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiSseDecoder {
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
            captured_finish_reason: None,
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

    /// Feed one SSE data line. `event_name` is ignored — Gemini's SSE stream
    /// has no `event:` name framing; each anonymous `data:` chunk is a
    /// complete `GenerateContentResponse` partial.
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

        let chunk: GeminiStreamChunk = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                self.state = State::Errored;
                self.upstream_error
                    .get_or_insert_with(|| format!("malformed Gemini chunk: {e}"));
                return out;
            }
        };

        if let Some(err) = chunk.error {
            self.upstream_error = Some(format!("{}: {}", err.status, err.message));
            self.state = State::Errored;
            return out;
        }

        if let Some(usage) = chunk.usage_metadata {
            self.captured_input_tokens =
                usage.prompt_token_count.max(self.captured_input_tokens);
            self.captured_output_tokens =
                usage.candidates_token_count.max(self.captured_output_tokens);
        }

        if let Some(candidate) = chunk.candidates.into_iter().next() {
            if let Some(content) = candidate.content {
                for part in content.parts {
                    self.handle_part(part, &mut out);
                }
            }
            if let Some(reason) = candidate.finish_reason {
                self.captured_finish_reason = Some(reason);
                self.state = State::Done;
            }
        }

        out
    }

    pub fn finish(&mut self) -> Vec<api::ResponseEvent> {
        let mut out = self.ensure_prelude();

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
            (Some(r), _) => map_gemini_finish_reason(&r),
            (None, Some(msg)) => internal_error_reason(&msg),
            (None, None) => internal_error_reason("stream ended without finishReason"),
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

    fn handle_part(&mut self, part: GeminiInboundPart, out: &mut Vec<api::ResponseEvent>) {
        match part {
            GeminiInboundPart::Text { text } => {
                if !text.is_empty() {
                    self.append_text(&text, out);
                }
            }
            GeminiInboundPart::FunctionCall { function_call } => {
                self.emit_function_call(&function_call, out);
            }
            // FunctionResponse and InlineData parts are output-only on the model
            // side — never emitted by the API in streaming responses. Tolerate
            // by ignoring rather than erroring. Unknown is the forward-compat
            // catch-all (e.g. thought / executableCode parts from Gemini 2.5
            // thinking mode).
            GeminiInboundPart::FunctionResponse { .. }
            | GeminiInboundPart::InlineData { .. }
            | GeminiInboundPart::Unknown(_) => {}
        }
    }

    fn emit_function_call(
        &mut self,
        call: &GeminiInboundFunctionCall,
        out: &mut Vec<api::ResponseEvent>,
    ) {
        // Gemini doesn't send tool-call ids; synthesize one. The controller
        // keys `action_results` by this id; the translator (request.rs) will
        // match by name on the way back out per Gemini's native shape.
        let id = format!("gemini-call-{}", Uuid::new_v4());
        let args_json =
            serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".to_string());
        if let Some(ev) = build_tool_call_event(&self.task_id, &id, &call.name, &args_json) {
            out.push(ev);
        }
    }

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
            // Gemini stream chunks carry no model name; fall back to a literal
            // sentinel. The controller uses cfg.model_id for display; this
            // field is used for billing attribution where it appears in usage
            // records.
            model_id: "gemini".to_string(),
            total_input: to_u32(self.captured_input_tokens),
            output: to_u32(self.captured_output_tokens),
            input_cache_read: 0,
            input_cache_write: 0,
            cost_in_cents: 0.0,
        }]
    }
}

/// Map Gemini's `finishReason` string to the proto's `StreamFinished.Reason`.
///
/// String arms use `_ =>` as a catch-all rather than an exhaustive enum match
/// because Gemini's finish reasons are open-ended strings, not a Rust enum —
/// the full set cannot be known at compile time. This matches the precedent set
/// by `map_ollama_done_reason` in `ollama/response.rs`.
///
/// Known mappings:
/// - `STOP`                     → Done (natural end of output)
/// - `MAX_TOKENS`               → MaxTokenLimit
/// - `SAFETY` / `RECITATION` / `OTHER` / `MALFORMED_FUNCTION_CALL` /
///   `BLOCKLIST` / `PROHIBITED_CONTENT` / `SPII` / `LANGUAGE` / anything else
///   → Other (Phase 4 polish can split SAFETY into Refused)
fn map_gemini_finish_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "STOP" => Reason::Done(Done {}),
        "MAX_TOKENS" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        _ => Reason::Other(Other {}),
    }
}

// ---- StreamDecoder trait impl ----

impl crate::local_provider::adapters::StreamDecoder for GeminiSseDecoder {
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
