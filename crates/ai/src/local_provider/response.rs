//! OpenAI Chat Completions SSE → `warp_multi_agent_api::ResponseEvent` adapter.
//!
//! Decoupled from HTTP so it's testable in isolation: the input is a stream
//! of message-data strings (the `data:` lines an SSE parser already extracted),
//! the output is a stream of fully-formed proto events that match the contract
//! the existing controller speaks (see `app/src/ai/agent/conversation.rs:2200+`).
//!
//! Output contract per turn:
//! ```text
//! Init { conversation_id, request_id, run_id }
//! ClientActions { BeginTransaction }
//! [ ClientActions { AddMessagesToTask { agent_output(empty) } }    // first text chunk
//!   ClientActions { AppendToMessageContent { agent_output(more) } }  // subsequent text chunks
//!   ClientActions { AddMessagesToTask { agent_reasoning(...) } }     // first reasoning chunk
//!   ClientActions { AppendToMessageContent { agent_reasoning(...) } } // subsequent reasoning
//!   ClientActions { AddMessagesToTask { tool_call(...) } }           // a complete tool call
//! ]*
//! ClientActions { CommitTransaction or RollbackTransaction }
//! Finished { reason }
//! ```
//!
//! See `specs/GH9303/tech.md` §6 for the full state-machine spec.

use std::collections::HashMap;

use thiserror::Error;
use uuid::Uuid;
use warp_multi_agent_api as api;

use crate::local_provider::wire::{ChatCompletionChunk, OpenAiUsage, ToolCallDelta};

/// Errors emitted while parsing the upstream SSE stream. These get translated
/// into a `Finished{InternalError}` proto event by the adapter; they don't
/// abort the stream.
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("malformed SSE chunk JSON: {0}")]
    MalformedChunk(#[from] serde_json::Error),
    #[error("upstream server reported an error: {message}")]
    UpstreamError {
        message: String,
        code: Option<String>,
    },
    #[error("stream ended unexpectedly without a finish_reason")]
    PrematureEof,
    #[error("tool-call argument parsing failed for `{name}`: {detail}")]
    ToolCallParse { name: String, detail: String },
}

/// State of the synthesizer mid-stream. After construction, call `feed` with each
/// SSE message-data string the upstream emits, and `finish` once the stream
/// terminates (cleanly or otherwise). Each call returns zero or more events
/// for the caller to forward downstream.
pub struct OpenAiSseAdapter {
    state: State,
    /// One synthetic task we attribute all messages to. Generated per turn.
    task_id: String,
    /// Generated per turn (StreamInit.conversation_id).
    conversation_id: String,
    /// Generated per turn (StreamInit.request_id).
    request_id: String,
    /// Generated per turn (StreamInit.run_id).
    run_id: String,
    /// Whether we've emitted Init yet.
    sent_init: bool,
    /// Whether we've emitted BeginTransaction yet.
    sent_begin: bool,
    /// Whether we've emitted CreateTask yet. Local-only conversations need
    /// the synthetic adapter to "initialize" the optimistic root task by
    /// emitting `Action::CreateTask` (with the same id the controller already
    /// has in `task_store`) — otherwise `AddMessagesToTask` returns
    /// `UpdateTask(TaskNotInitialized)` and the entire transaction rolls back.
    /// Server-driven responses get this for free; the local adapter has to
    /// synthesize it.
    sent_create_task: bool,
    /// Whether the in-progress assistant text message has been opened with AddMessagesToTask.
    text_message_id: Option<String>,
    /// Same for the in-progress reasoning message.
    reasoning_message_id: Option<String>,
    /// Per-index tool-call accumulators. Streamed in fragments by OpenAI.
    tool_buffers: HashMap<u32, ToolBuffer>,
    /// Captured finish_reason once the model reports one.
    captured_finish: Option<api::response_event::stream_finished::Reason>,
    /// User-visible upstream error message recorded by the HTTP runner when
    /// the SSE stream errors before any `finish_reason` arrives (e.g. the
    /// server returned a 4xx/5xx JSON error body that isn't valid SSE). Used
    /// by `finish()` to surface the real reason instead of the generic
    /// "stream ended without finish_reason".
    upstream_error: Option<String>,
    /// Phase B-3a: OpenAI-format `usage` from the final chunk (when
    /// `stream_options.include_usage = true`). Mapped to
    /// `StreamFinished.token_usage` on `finish()`. The auto-compaction hook
    /// reads it off the conversation state to decide whether to summarize.
    captured_usage: Option<OpenAiUsage>,
    /// Model id echoed back by the upstream server in chunk responses. Falls
    /// through to `StreamFinished.token_usage[0].model_id` so the controller
    /// keeps per-model accumulators correct. `None` until we observe a chunk
    /// with `model` set; servers that omit it leave us with no attribution
    /// and we fall back to `"local"` on emit.
    captured_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Streaming,
    /// `finish_reason` arrived. Phase B-3a keeps the stream open in this
    /// state because OpenAI emits the usage chunk *after* `finish_reason`
    /// (with `stream_options.include_usage = true`). Subsequent content
    /// chunks are ignored; only `usage` and `[DONE]` are still meaningful.
    /// Becomes `Done` on `[DONE]` or upstream EOF.
    Finishing,
    Done,
    Errored,
}

#[derive(Debug, Default)]
struct ToolBuffer {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    /// Whether we've emitted this tool call already (on `finish_reason` or new index).
    emitted: bool,
}

impl Default for OpenAiSseAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiSseAdapter {
    pub fn new() -> Self {
        Self::with_ids(
            format!("local:{}", Uuid::new_v4()),
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            format!("local-task:{}", Uuid::new_v4()),
        )
    }

    /// Construct with explicit IDs, primarily for tests that assert ID round-tripping.
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
            reasoning_message_id: None,
            tool_buffers: HashMap::new(),
            captured_finish: None,
            captured_usage: None,
            captured_model: None,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// Suppress the synthetic `CreateTask` emission. Used by the HTTP runner
    /// on every turn after the first — the optimistic root task has already
    /// been upgraded to a server-created task by the previous turn's
    /// `CreateTask`, so re-emitting one would error with
    /// `UpgradeOptimisticTask::UnexpectedUpgrade` and corrupt the task store.
    pub fn skip_create_task(&mut self) {
        self.sent_create_task = true;
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Returns true once the adapter has observed `[DONE]` or a fatal
    /// error — i.e. the network stream is logically closed. `finish_reason`
    /// alone does **not** terminate (Phase B-3a needs the post-`finish_reason`
    /// usage chunk before `[DONE]`). Callers driving the adapter from a
    /// network source should check this after each `feed` and call `finish`
    /// immediately when true, so a server that keeps the connection open
    /// past `[DONE]` doesn't leave the response stream hanging.
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, State::Done | State::Errored)
    }

    /// Feed one SSE message-data string. Returns the events to emit downstream.
    /// `[DONE]` is treated as a successful end-of-stream.
    pub fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent> {
        if matches!(self.state, State::Done | State::Errored) {
            return vec![];
        }
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return vec![];
        }
        if trimmed == "[DONE]" {
            self.state = State::Done;
            return vec![];
        }

        let mut out = Vec::new();
        if !self.sent_init {
            out.push(self.build_init());
            self.sent_init = true;
        }
        if !self.sent_begin {
            out.extend(self.build_action(client_action_begin()));
            self.sent_begin = true;
        }
        if !self.sent_create_task {
            out.extend(self.build_action(client_action_create_task(&self.task_id)));
            self.sent_create_task = true;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                self.state = State::Errored;
                self.captured_finish =
                    Some(internal_error_reason(&format!("malformed SSE chunk: {e}")));
                return out;
            }
        };

        if let Some(err) = chunk.error {
            self.state = State::Errored;
            self.captured_finish = Some(api::response_event::stream_finished::Reason::Other(
                api::response_event::stream_finished::Other {},
            ));
            // Surface error details via InternalError so the user sees them.
            self.captured_finish = Some(internal_error_reason(&err.message));
            return out;
        }

        // Phase B-3a: capture usage + model when the upstream provided them.
        // OpenAI emits a final chunk with `choices: []` and `usage: {...}`
        // when `stream_options.include_usage = true`; some servers include
        // usage on the same chunk as the final content. Don't overwrite
        // captured_model once it's set — first non-empty wins.
        if let Some(usage) = chunk.usage {
            self.captured_usage = Some(usage);
        }
        if self.captured_model.is_none() {
            if let Some(m) = chunk.model {
                if !m.is_empty() {
                    self.captured_model = Some(m);
                }
            }
        }

        // Once `finish_reason` has fired we're in `Finishing`, waiting for
        // the post-`finish_reason` usage chunk and `[DONE]`. Subsequent
        // content / tool deltas are ignored (servers that emit them after
        // `finish_reason` are non-compliant and the model has nothing more
        // to say).
        if self.state == State::Finishing {
            return out;
        }

        let Some(choice) = chunk.choices.into_iter().next() else {
            return out; // empty choices is silent — wait for the next chunk
        };

        // Visible content (assistant text)
        if let Some(text) = choice.delta.content.as_deref() {
            if !text.is_empty() {
                let (open, append) = self.append_to_kind(MessageKind::AgentOutput, text);
                out.extend(open);
                out.extend(append);
            }
        }

        // Reasoning content (DeepSeek/Qwen `reasoning_content` or OpenAI `reasoning`).
        let reasoning = choice
            .delta
            .reasoning_content
            .as_deref()
            .or(choice.delta.reasoning.as_deref())
            .filter(|s| !s.is_empty());
        if let Some(text) = reasoning {
            let (open, append) = self.append_to_kind(MessageKind::AgentReasoning, text);
            out.extend(open);
            out.extend(append);
        }

        // Tool-call fragments (accumulated; emitted on new-index or finish_reason).
        if let Some(deltas) = choice.delta.tool_calls {
            for delta in deltas {
                self.absorb_tool_delta(delta, &mut out);
            }
        }

        // Finish reason captured for the closing event. Phase B-3a:
        // transition to Finishing instead of Done so the post-`finish_reason`
        // usage chunk can still be fed; the runner observes Done only on
        // `[DONE]` (or upstream EOF, handled in the runner).
        if let Some(reason) = choice.finish_reason.as_deref() {
            self.captured_finish = Some(map_finish_reason(reason));
            self.flush_pending_tool_calls(&mut out);
            self.state = State::Finishing;
        }

        out
    }

    /// Record an upstream error message (e.g. an HTTP 4xx/5xx JSON body) so
    /// it's surfaced as the InternalError reason on the Finished event when
    /// the stream closes without a `finish_reason`. Idempotent: only the
    /// first call wins, since later errors during teardown are usually
    /// downstream symptoms of the first one.
    pub fn record_upstream_error(&mut self, msg: String) {
        if self.upstream_error.is_none() {
            self.upstream_error = Some(msg);
        }
    }

    /// Call once the upstream stream ends (cleanly or otherwise). Emits the
    /// Commit/Rollback action and the Finished event.
    pub fn finish(&mut self) -> Vec<api::ResponseEvent> {
        let mut out = Vec::new();
        if !self.sent_init {
            out.push(self.build_init());
            self.sent_init = true;
        }
        if !self.sent_begin {
            out.extend(self.build_action(client_action_begin()));
            self.sent_begin = true;
        }
        if !self.sent_create_task {
            out.extend(self.build_action(client_action_create_task(&self.task_id)));
            self.sent_create_task = true;
        }

        // Healthy = we observed a `finish_reason` (captured_finish set) and
        // the stream ended cleanly via `[DONE]` or upstream EOF after
        // `finish_reason` (state == Done or Finishing). State::Errored or
        // EOF before `finish_reason` (Streaming) means we roll back.
        let healthy =
            matches!(self.state, State::Done | State::Finishing) && self.captured_finish.is_some();
        let closing = if healthy {
            client_action_commit()
        } else {
            client_action_rollback()
        };
        out.extend(self.build_action(closing));

        let reason = self.captured_finish.take().unwrap_or_else(|| {
            // Prefer the upstream error message captured by the HTTP runner
            // (e.g. "400 Bad Request: model 'foo' not found") so users see
            // the real failure reason in the UI instead of the generic
            // "stream ended without finish_reason" — which historically hid
            // every misconfiguration behind one unhelpful sentence.
            let msg = self
                .upstream_error
                .take()
                .unwrap_or_else(|| "stream ended without finish_reason".to_string());
            internal_error_reason(&msg)
        });
        let token_usage = match self.captured_usage.take() {
            Some(u) => vec![open_ai_usage_to_proto(u, self.captured_model.as_deref())],
            None => Vec::new(),
        };
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

    /// Test/inspection accessor for the captured upstream usage stats.
    #[cfg(test)]
    pub(crate) fn captured_usage(&self) -> Option<OpenAiUsage> {
        self.captured_usage
    }

    // ---------- internals ----------

    fn build_init(&self) -> api::ResponseEvent {
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::Init(
                api::response_event::StreamInit {
                    conversation_id: self.conversation_id.clone(),
                    request_id: self.request_id.clone(),
                    run_id: self.run_id.clone(),
                },
            )),
        }
    }

    fn build_action(&self, action: api::client_action::Action) -> Vec<api::ResponseEvent> {
        vec![api::ResponseEvent {
            r#type: Some(api::response_event::Type::ClientActions(
                api::response_event::ClientActions {
                    actions: vec![api::ClientAction {
                        action: Some(action),
                    }],
                },
            )),
        }]
    }

    /// Either opens the message with AddMessagesToTask (first chunk for that kind)
    /// or extends it with AppendToMessageContent (subsequent chunks).
    fn append_to_kind(
        &mut self,
        kind: MessageKind,
        text: &str,
    ) -> (Vec<api::ResponseEvent>, Vec<api::ResponseEvent>) {
        let mut open = Vec::new();
        let opened = match kind {
            MessageKind::AgentOutput => &mut self.text_message_id,
            MessageKind::AgentReasoning => &mut self.reasoning_message_id,
        };
        if opened.is_none() {
            let message_id = Uuid::new_v4().to_string();
            *opened = Some(message_id.clone());
            // First chunk: create the message with the initial content.
            open.extend(
                self.build_action(api::client_action::Action::AddMessagesToTask(
                    api::client_action::AddMessagesToTask {
                        task_id: self.task_id.clone(),
                        messages: vec![build_kind_message(&message_id, kind, text)],
                    },
                )),
            );
            return (open, vec![]);
        }
        // Subsequent chunks: append.
        let message_id = opened.as_ref().expect("just checked").clone();
        let mask_path = match kind {
            MessageKind::AgentOutput => "agent_output.text",
            MessageKind::AgentReasoning => "agent_reasoning.reasoning",
        };
        let append = self.build_action(api::client_action::Action::AppendToMessageContent(
            api::client_action::AppendToMessageContent {
                task_id: self.task_id.clone(),
                message: Some(build_kind_message(&message_id, kind, text)),
                mask: Some(prost_types::FieldMask {
                    paths: vec![mask_path.to_string()],
                }),
            },
        ));
        (open, append)
    }

    fn absorb_tool_delta(&mut self, delta: ToolCallDelta, out: &mut Vec<api::ResponseEvent>) {
        // A higher-index fragment signals previous indices are complete.
        let arrived = delta.index;
        let prior_indices: Vec<u32> = self
            .tool_buffers
            .keys()
            .copied()
            .filter(|i| *i < arrived)
            .collect();
        for idx in prior_indices {
            if let Some(buf) = self.tool_buffers.get_mut(&idx) {
                if !buf.emitted {
                    if let Some(ev) = build_tool_call_event(&self.task_id, buf) {
                        out.push(ev);
                    }
                    buf.emitted = true;
                }
            }
        }

        let entry = self.tool_buffers.entry(arrived).or_default();
        if let Some(id) = delta.id {
            if entry.id.is_none() {
                entry.id = Some(id);
            }
        }
        if let Some(func) = delta.function {
            if let Some(name) = func.name {
                if entry.name.is_none() {
                    entry.name = Some(name);
                }
            }
            if let Some(args_fragment) = func.arguments {
                entry.arguments.push_str(&args_fragment);
            }
        }
    }

    fn flush_pending_tool_calls(&mut self, out: &mut Vec<api::ResponseEvent>) {
        let mut indices: Vec<u32> = self.tool_buffers.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some(buf) = self.tool_buffers.get_mut(&idx) {
                if !buf.emitted {
                    if let Some(ev) = build_tool_call_event(&self.task_id, buf) {
                        out.push(ev);
                    }
                    buf.emitted = true;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MessageKind {
    AgentOutput,
    AgentReasoning,
}

// ---------- proto construction helpers ----------

fn build_kind_message(message_id: &str, kind: MessageKind, text: &str) -> api::Message {
    let inner = match kind {
        MessageKind::AgentOutput => api::message::Message::AgentOutput(api::message::AgentOutput {
            text: text.to_string(),
        }),
        MessageKind::AgentReasoning => {
            api::message::Message::AgentReasoning(api::message::AgentReasoning {
                reasoning: text.to_string(),
                finished_duration: None,
            })
        }
    };
    api::Message {
        id: message_id.to_string(),
        message: Some(inner),
        ..Default::default()
    }
}

fn build_tool_call_event(task_id: &str, buf: &ToolBuffer) -> Option<api::ResponseEvent> {
    let id = buf.id.clone()?;
    let name = buf.name.clone()?;
    // Translate the OpenAI tool_call into the proto's strongly-typed
    // Message::ToolCall.tool::* variant via tools.rs. Failures (unknown tool
    // name, malformed args, etc.) produce a synthetic assistant text message
    // instead of dropping the turn — see tech.md §Risks.
    let tool_call = match crate::local_provider::tools::translate_openai_tool_call(
        &id,
        &name,
        &buf.arguments,
    ) {
        Ok(tc) => tc,
        Err(e) => {
            // Surface the parse error as visible assistant text. This event
            // type still uses AddMessagesToTask but with an AgentOutput
            // explaining the failure, so the user sees the model's intent.
            let body = format!(
                "I tried to call `{name}` but its arguments were unusable: {e}\n\nRaw args: {}",
                buf.arguments
            );
            let err_message = api::Message {
                id: Uuid::new_v4().to_string(),
                message: Some(api::message::Message::AgentOutput(
                    api::message::AgentOutput { text: body },
                )),
                ..Default::default()
            };
            return Some(api::ResponseEvent {
                r#type: Some(api::response_event::Type::ClientActions(
                    api::response_event::ClientActions {
                        actions: vec![api::ClientAction {
                            action: Some(api::client_action::Action::AddMessagesToTask(
                                api::client_action::AddMessagesToTask {
                                    task_id: task_id.to_string(),
                                    messages: vec![err_message],
                                },
                            )),
                        }],
                    },
                )),
            });
        }
    };

    let message = api::Message {
        id: Uuid::new_v4().to_string(),
        message: Some(api::message::Message::ToolCall(tool_call)),
        ..Default::default()
    };
    Some(api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::AddMessagesToTask(
                        api::client_action::AddMessagesToTask {
                            task_id: task_id.to_string(),
                            messages: vec![message],
                        },
                    )),
                }],
            },
        )),
    })
}

fn client_action_begin() -> api::client_action::Action {
    api::client_action::Action::BeginTransaction(api::client_action::BeginTransaction {})
}

fn client_action_create_task(task_id: &str) -> api::client_action::Action {
    api::client_action::Action::CreateTask(api::client_action::CreateTask {
        task: Some(api::Task {
            id: task_id.to_string(),
            ..Default::default()
        }),
    })
}

fn client_action_commit() -> api::client_action::Action {
    api::client_action::Action::CommitTransaction(api::client_action::CommitTransaction {})
}

fn client_action_rollback() -> api::client_action::Action {
    api::client_action::Action::RollbackTransaction(api::client_action::RollbackTransaction {})
}

/// Phase B-3a: bridge OpenAI's `{prompt_tokens, completion_tokens, ...}`
/// into the Warp proto `TokenUsage`. `model_id` falls back to `"local"`
/// when the upstream didn't echo a `model` field — the controller's
/// per-model accumulator just needs a stable key.
fn open_ai_usage_to_proto(
    usage: OpenAiUsage,
    model_id: Option<&str>,
) -> api::response_event::stream_finished::TokenUsage {
    use api::response_event::stream_finished::TokenUsage;
    let cached = usage
        .prompt_tokens_details
        .map(|d| d.cached_tokens)
        .unwrap_or(0);
    // Saturating cast: OpenAI uses u64 internally; the proto is u32. A turn
    // that emits >4 billion tokens is so far past the model limit that the
    // cap-at-u32::MAX read is fine for accounting purposes.
    let to_u32 = |n: u64| -> u32 { n.try_into().unwrap_or(u32::MAX) };
    TokenUsage {
        model_id: model_id.unwrap_or("local").to_string(),
        // total_input maps to OpenAI prompt_tokens (cache reads are part of
        // prompt_tokens already in OpenAI's accounting; we surface them
        // separately on input_cache_read for consumers that want to subtract
        // them out).
        total_input: to_u32(usage.prompt_tokens),
        output: to_u32(usage.completion_tokens),
        input_cache_read: to_u32(cached),
        input_cache_write: 0,
        cost_in_cents: 0.0,
    }
}

fn map_finish_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "stop" | "tool_calls" => Reason::Done(Done {}),
        "length" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        "content_filter" => Reason::Other(Other {}),
        _ => Reason::Other(Other {}),
    }
}

fn internal_error_reason(message: &str) -> api::response_event::stream_finished::Reason {
    api::response_event::stream_finished::Reason::InternalError(
        api::response_event::stream_finished::InternalError {
            message: message.to_string(),
        },
    )
}
