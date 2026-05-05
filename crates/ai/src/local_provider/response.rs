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

use crate::local_provider::wire::{ChatCompletionChunk, ToolCallDelta};

/// Errors emitted while parsing the upstream SSE stream. These get translated
/// into a `Finished{InternalError}` proto event by the adapter; they don't
/// abort the stream.
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("malformed SSE chunk JSON: {0}")]
    MalformedChunk(#[from] serde_json::Error),
    #[error("upstream server reported an error: {message}")]
    UpstreamError { message: String, code: Option<String> },
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Streaming,
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
            text_message_id: None,
            reasoning_message_id: None,
            tool_buffers: HashMap::new(),
            captured_finish: None,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Returns true once the adapter has observed either `[DONE]`, a chunk
    /// with `finish_reason`, or a fatal error — i.e. the LOGICAL stream has
    /// ended even if the underlying HTTP body hasn't closed yet. Callers
    /// driving the adapter from a network source should check this after
    /// each `feed` and call `finish` immediately when true, so a server that
    /// keeps the connection open past `[DONE]` doesn't leave the response
    /// stream hanging.
    pub fn is_terminal(&self) -> bool {
        self.state != State::Streaming
    }

    /// Feed one SSE message-data string. Returns the events to emit downstream.
    /// `[DONE]` is treated as a successful end-of-stream.
    pub fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent> {
        if self.state != State::Streaming {
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

        let chunk: ChatCompletionChunk = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                self.state = State::Errored;
                self.captured_finish = Some(internal_error_reason(&format!(
                    "malformed SSE chunk: {e}"
                )));
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

        // Finish reason captured for the closing event.
        if let Some(reason) = choice.finish_reason.as_deref() {
            self.captured_finish = Some(map_finish_reason(reason));
            self.flush_pending_tool_calls(&mut out);
            self.state = State::Done;
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

        let healthy = matches!(self.state, State::Done) && self.captured_finish.is_some();
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
        out.push(api::ResponseEvent {
            r#type: Some(api::response_event::Type::Finished(
                api::response_event::StreamFinished {
                    reason: Some(reason),
                    ..Default::default()
                },
            )),
        });

        self.state = State::Done;
        out
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
            open.extend(self.build_action(api::client_action::Action::AddMessagesToTask(
                api::client_action::AddMessagesToTask {
                    task_id: self.task_id.clone(),
                    messages: vec![build_kind_message(&message_id, kind, text)],
                },
            )));
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
                message: Some(api::message::Message::AgentOutput(api::message::AgentOutput {
                    text: body,
                })),
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

fn client_action_commit() -> api::client_action::Action {
    api::client_action::Action::CommitTransaction(api::client_action::CommitTransaction {})
}

fn client_action_rollback() -> api::client_action::Action {
    api::client_action::Action::RollbackTransaction(api::client_action::RollbackTransaction {})
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
