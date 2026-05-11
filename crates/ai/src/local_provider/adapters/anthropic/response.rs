//! Anthropic Messages API streaming-SSE → `warp_multi_agent_api::ResponseEvent`
//! decoder. Mirrors `OpenAiSseAdapter`'s public surface and emitted event
//! shape so the controller doesn't have to know which adapter produced a
//! given stream.
//!
//! Event mapping:
//!
//! ```text
//! message_start          → Init + BeginTransaction + CreateTask (prelude),
//!                          capture input_tokens / cache_* usage.
//! content_block_start    → record (index → block kind) in the per-turn map.
//!                          Text/Thinking blocks open lazily on first delta.
//!                          ToolUse blocks start an empty arg accumulator.
//! content_block_delta    → text_delta:        Open/append AgentOutput.
//!                          thinking_delta:    Open/append AgentReasoning.
//!                          input_json_delta:  Append to that block's
//!                                             arg accumulator.
//!                          signature_delta:   Ignored (extended thinking).
//! content_block_stop     → ToolUse: parse accumulated JSON args, emit a
//!                          single `AddMessagesToTask{ToolCall}` event.
//!                          Text/Thinking: no-op (already streamed).
//! message_delta          → Capture stop_reason + running output_tokens.
//!                          Transition state to Finishing.
//! message_stop           → Flush any unemitted tool-use buffers; transition
//!                          to Done. The runner observes `is_terminal()` and
//!                          calls `finish()` to drain the closing events.
//!                          (Anthropic has no `[DONE]` equivalent.)
//! ping                   → No-op.
//! error                  → Capture `upstream_error`, transition to Errored.
//! ```
//!
//! Each content-block gets its **own** AgentOutput / AgentReasoning message
//! (per-block `opened_message_id`) — distinct from `OpenAiSseAdapter` which
//! shares a single open message across all text deltas in the turn. Anthropic
//! emits text in discrete blocks (one before a tool call, one after the tool
//! result), so per-block messages match the wire model. Multiple text blocks
//! in a single turn render as separate messages in the task store, preserving
//! ordering relative to interspersed tool calls.

use std::collections::HashMap;

use uuid::Uuid;
use warp_multi_agent_api as api;

use super::wire::{
    AnthropicStreamEvent, MessageDeltaPayload, MessageDeltaUsage, StreamContentBlock,
    StreamContentDelta, StreamMessageStart,
};

/// Stream decoder. Constructed per turn; the runner feeds it SSE
/// message-data strings (with the SSE `event:` name passed through) and
/// drains the closing events with `finish()` after the upstream closes.
pub struct AnthropicSseDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,
    /// Per-block state keyed by Anthropic's `index` field. Populated on
    /// `content_block_start`, consumed by `content_block_delta` /
    /// `content_block_stop`, drained by `flush_pending_blocks` on
    /// `message_stop` or `finish()`.
    blocks: HashMap<u32, BlockState>,
    captured_stop_reason: Option<String>,
    captured_model: Option<String>,
    captured_usage: CapturedUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Streaming,
    /// `message_delta` with a `stop_reason` arrived. We accept further
    /// message-level events (additional message_delta with usage updates,
    /// the final message_stop) but ignore stray content deltas in this
    /// state.
    Finishing,
    Done,
    Errored,
}

#[derive(Debug, Default, Clone, Copy)]
struct CapturedUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
}

impl CapturedUsage {
    fn has_any(&self) -> bool {
        self.input_tokens != 0
            || self.output_tokens != 0
            || self.cache_creation_input_tokens != 0
            || self.cache_read_input_tokens != 0
    }
}

struct BlockState {
    kind: BlockKind,
    /// For text/thinking blocks: id of the open AgentOutput/AgentReasoning
    /// message. Lazily populated on the first delta. `None` for tool_use
    /// blocks (they never open a streamed message — the tool_call event is
    /// emitted in one shot on `content_block_stop`).
    opened_message_id: Option<String>,
}

enum BlockKind {
    Text,
    Thinking,
    ToolUse {
        id: String,
        name: String,
        args_acc: String,
        emitted: bool,
    },
}

impl Default for AnthropicSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicSseDecoder {
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
            blocks: HashMap::new(),
            captured_stop_reason: None,
            captured_model: None,
            captured_usage: CapturedUsage::default(),
        }
    }

    /// Suppress the synthetic `CreateTask` emission. Used by the HTTP runner
    /// after the first turn — the optimistic root task has already been
    /// upgraded to a server-created task, so re-emitting `CreateTask` would
    /// trip `UpgradeOptimisticTask::UnexpectedUpgrade`.
    pub fn skip_create_task(&mut self) {
        self.sent_create_task = true;
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Returns true once the stream is logically closed (`message_stop` /
    /// fatal error). The runner uses this to know when to call `finish()`.
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, State::Done | State::Errored)
    }

    /// Record an upstream HTTP-level error (e.g. 4xx body) so `finish()`
    /// surfaces it as the `InternalError` reason instead of the generic
    /// "stream ended without finish_reason". Idempotent — only the first
    /// call wins, since later errors during teardown are typically
    /// downstream symptoms.
    pub fn record_upstream_error(&mut self, msg: String) {
        if self.upstream_error.is_none() {
            self.upstream_error = Some(msg);
        }
    }

    /// Feed one SSE message. `event_name` is the SSE `event:` header (or
    /// `None` for the spec default `"message"`); the body's `type` field
    /// provides the variant discriminator regardless. Anthropic sets both
    /// in lockstep; we deserialize via the `type` tag and ignore
    /// `event_name` as a redundancy check.
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

        let event: AnthropicStreamEvent = match serde_json::from_str(trimmed) {
            Ok(ev) => ev,
            Err(e) => {
                self.state = State::Errored;
                self.captured_stop_reason = None;
                self.upstream_error
                    .get_or_insert_with(|| format!("malformed Anthropic SSE chunk: {e}"));
                return out;
            }
        };

        match event {
            AnthropicStreamEvent::MessageStart { message } => self.on_message_start(message),
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.on_block_start(index, content_block),
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                self.on_block_delta(index, delta, &mut out);
            }
            AnthropicStreamEvent::ContentBlockStop { index } => {
                self.on_block_stop(index, &mut out);
            }
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                self.on_message_delta(delta, usage);
            }
            AnthropicStreamEvent::MessageStop => {
                self.flush_pending_blocks(&mut out);
                self.state = State::Done;
            }
            AnthropicStreamEvent::Ping => {}
            AnthropicStreamEvent::Error { error } => {
                self.upstream_error = Some(format!("{}: {}", error.kind, error.message));
                self.state = State::Errored;
            }
        }
        out
    }

    /// Emit the closing transaction + `Finished` event. Called by the
    /// runner once the upstream HTTP stream has ended (cleanly or
    /// otherwise).
    pub fn finish(&mut self) -> Vec<api::ResponseEvent> {
        let mut out = self.ensure_prelude();

        let healthy = matches!(self.state, State::Done | State::Finishing)
            && self.captured_stop_reason.is_some();
        let closing = if healthy {
            client_action_commit()
        } else {
            client_action_rollback()
        };
        out.extend(self.build_action(closing));

        let reason = match (
            self.captured_stop_reason.take(),
            self.upstream_error.take(),
        ) {
            (Some(stop), _) => map_anthropic_stop_reason(&stop),
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

    // ---------- per-event handlers ----------

    fn on_message_start(&mut self, message: StreamMessageStart) {
        if self.captured_model.is_none() {
            if let Some(m) = message.model.filter(|s| !s.is_empty()) {
                self.captured_model = Some(m);
            }
        }
        if let Some(u) = message.usage {
            self.captured_usage.input_tokens = u.input_tokens;
            self.captured_usage.output_tokens = u.output_tokens;
            self.captured_usage.cache_creation_input_tokens = u.cache_creation_input_tokens;
            self.captured_usage.cache_read_input_tokens = u.cache_read_input_tokens;
        }
    }

    fn on_block_start(&mut self, index: u32, block: StreamContentBlock) {
        let kind = match block {
            StreamContentBlock::Text { .. } => BlockKind::Text,
            StreamContentBlock::Thinking { .. } => BlockKind::Thinking,
            StreamContentBlock::ToolUse { id, name, .. } => BlockKind::ToolUse {
                id,
                name,
                args_acc: String::new(),
                emitted: false,
            },
        };
        self.blocks.insert(
            index,
            BlockState {
                kind,
                opened_message_id: None,
            },
        );
    }

    fn on_block_delta(
        &mut self,
        index: u32,
        delta: StreamContentDelta,
        out: &mut Vec<api::ResponseEvent>,
    ) {
        // Stray deltas after `finish_reason` are silently dropped — the
        // model has signaled it's done speaking and any further content
        // would be non-compliant.
        if self.state == State::Finishing {
            return;
        }
        // Track what we need to do, without holding a mutable borrow on
        // `self.blocks` past the dispatch decision.
        let action = {
            let Some(state) = self.blocks.get_mut(&index) else {
                return;
            };
            match (&mut state.kind, delta) {
                (BlockKind::Text, StreamContentDelta::TextDelta { text }) if !text.is_empty() => {
                    DeltaAction::AppendText {
                        kind: MessageKind::AgentOutput,
                        text,
                    }
                }
                (
                    BlockKind::Thinking,
                    StreamContentDelta::ThinkingDelta { thinking },
                ) if !thinking.is_empty() => DeltaAction::AppendText {
                    kind: MessageKind::AgentReasoning,
                    text: thinking,
                },
                (
                    BlockKind::ToolUse { args_acc, .. },
                    StreamContentDelta::InputJsonDelta { partial_json },
                ) => {
                    args_acc.push_str(&partial_json);
                    DeltaAction::None
                }
                // SignatureDelta is the extended-thinking opaque signature;
                // we don't surface it. Mismatched delta-vs-block-kind
                // combos shouldn't happen on the wire but are silently
                // ignored as a forward-compat safeguard.
                _ => DeltaAction::None,
            }
        };

        if let DeltaAction::AppendText { kind, text } = action {
            self.append_to_block(index, kind, &text, out);
        }
    }

    fn on_block_stop(&mut self, index: u32, out: &mut Vec<api::ResponseEvent>) {
        // Capture what we need for emit while holding the borrow briefly.
        let emit: Option<(String, String, String)> = {
            let Some(state) = self.blocks.get_mut(&index) else {
                return;
            };
            if let BlockKind::ToolUse {
                id,
                name,
                args_acc,
                emitted,
            } = &mut state.kind
            {
                if !*emitted {
                    *emitted = true;
                    Some((id.clone(), name.clone(), std::mem::take(args_acc)))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((id, name, args)) = emit {
            if let Some(ev) = build_tool_call_event(&self.task_id, &id, &name, &args) {
                out.push(ev);
            }
        }
    }

    fn on_message_delta(
        &mut self,
        delta: MessageDeltaPayload,
        usage: Option<MessageDeltaUsage>,
    ) {
        if let Some(reason) = delta.stop_reason {
            if self.captured_stop_reason.is_none() {
                self.captured_stop_reason = Some(reason);
            }
            self.state = State::Finishing;
        }
        if let Some(u) = usage {
            self.captured_usage.output_tokens = u.output_tokens;
        }
    }

    fn flush_pending_blocks(&mut self, out: &mut Vec<api::ResponseEvent>) {
        // Collect pending tool emissions first, then borrow self.task_id.
        let mut indices: Vec<u32> = self.blocks.keys().copied().collect();
        indices.sort();
        let mut pending: Vec<(String, String, String)> = Vec::new();
        for idx in indices {
            if let Some(state) = self.blocks.get_mut(&idx) {
                if let BlockKind::ToolUse {
                    id,
                    name,
                    args_acc,
                    emitted,
                } = &mut state.kind
                {
                    if !*emitted {
                        *emitted = true;
                        pending.push((id.clone(), name.clone(), std::mem::take(args_acc)));
                    }
                }
            }
        }
        for (id, name, args) in pending {
            if let Some(ev) = build_tool_call_event(&self.task_id, &id, &name, &args) {
                out.push(ev);
            }
        }
    }

    fn append_to_block(
        &mut self,
        index: u32,
        kind: MessageKind,
        text: &str,
        out: &mut Vec<api::ResponseEvent>,
    ) {
        let task_id = self.task_id.clone();
        let opened = match self.blocks.get_mut(&index) {
            Some(state) => &mut state.opened_message_id,
            None => return,
        };
        if opened.is_none() {
            let message_id = Uuid::new_v4().to_string();
            *opened = Some(message_id.clone());
            out.push(build_client_action_event(
                api::client_action::Action::AddMessagesToTask(
                    api::client_action::AddMessagesToTask {
                        task_id: task_id.clone(),
                        messages: vec![build_kind_message(&message_id, kind, text)],
                    },
                ),
            ));
            return;
        }
        let message_id = opened.as_ref().expect("just checked").clone();
        let mask_path = match kind {
            MessageKind::AgentOutput => "agent_output.text",
            MessageKind::AgentReasoning => "agent_reasoning.reasoning",
        };
        out.push(build_client_action_event(
            api::client_action::Action::AppendToMessageContent(
                api::client_action::AppendToMessageContent {
                    task_id,
                    message: Some(build_kind_message(&message_id, kind, text)),
                    mask: Some(prost_types::FieldMask {
                        paths: vec![mask_path.to_string()],
                    }),
                },
            ),
        ));
    }

    // ---------- prelude / proto builders ----------

    fn ensure_prelude(&mut self) -> Vec<api::ResponseEvent> {
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
        out
    }

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
        vec![build_client_action_event(action)]
    }

    fn token_usage_proto(&self) -> Vec<api::response_event::stream_finished::TokenUsage> {
        use api::response_event::stream_finished::TokenUsage;
        if !self.captured_usage.has_any() {
            return Vec::new();
        }
        let to_u32 = |n: u64| -> u32 { n.try_into().unwrap_or(u32::MAX) };
        vec![TokenUsage {
            model_id: self
                .captured_model
                .clone()
                .unwrap_or_else(|| "anthropic".to_string()),
            total_input: to_u32(self.captured_usage.input_tokens),
            output: to_u32(self.captured_usage.output_tokens),
            input_cache_read: to_u32(self.captured_usage.cache_read_input_tokens),
            input_cache_write: to_u32(self.captured_usage.cache_creation_input_tokens),
            cost_in_cents: 0.0,
        }]
    }
}

// ---------- trait impl ----------

impl crate::local_provider::adapters::StreamDecoder for AnthropicSseDecoder {
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

// ---------- internal helpers ----------

enum DeltaAction {
    None,
    AppendText { kind: MessageKind, text: String },
}

#[derive(Debug, Clone, Copy)]
enum MessageKind {
    AgentOutput,
    AgentReasoning,
}

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

fn build_client_action_event(action: api::client_action::Action) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(action),
                }],
            },
        )),
    }
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

/// Build an `AddMessagesToTask{ToolCall}` event from the accumulated tool-use
/// block state. Args parse failures surface as an `AgentOutput` message
/// explaining the failure (matches `OpenAiSseAdapter`'s behavior).
fn build_tool_call_event(
    task_id: &str,
    id: &str,
    name: &str,
    args: &str,
) -> Option<api::ResponseEvent> {
    // Anthropic emits the input JSON as a series of `input_json_delta`
    // chunks. The accumulated string can be empty if the model emitted a
    // tool_use with no inputs (rare but valid for schemas without required
    // fields); pass "{}" through to the parser so the typed proto variant
    // gets built with default values rather than rejecting outright.
    let args_for_parse = if args.trim().is_empty() { "{}" } else { args };
    let tool_call =
        match crate::local_provider::tools::translate_openai_tool_call(id, name, args_for_parse) {
            Ok(tc) => tc,
            Err(e) => {
                let body = format!(
                    "I tried to call `{name}` but its arguments were unusable: {e}\n\nRaw args: {args}"
                );
                let err_message = api::Message {
                    id: Uuid::new_v4().to_string(),
                    message: Some(api::message::Message::AgentOutput(
                        api::message::AgentOutput { text: body },
                    )),
                    ..Default::default()
                };
                return Some(build_client_action_event(
                    api::client_action::Action::AddMessagesToTask(
                        api::client_action::AddMessagesToTask {
                            task_id: task_id.to_string(),
                            messages: vec![err_message],
                        },
                    ),
                ));
            }
        };

    let message = api::Message {
        id: Uuid::new_v4().to_string(),
        message: Some(api::message::Message::ToolCall(tool_call)),
        ..Default::default()
    };
    Some(build_client_action_event(
        api::client_action::Action::AddMessagesToTask(api::client_action::AddMessagesToTask {
            task_id: task_id.to_string(),
            messages: vec![message],
        }),
    ))
}

/// Translate Anthropic's `stop_reason` string into the proto's
/// `StreamFinished.Reason`. Matches the OpenAI adapter's mapping where
/// possible:
/// - `end_turn` / `tool_use` / `stop_sequence` → `Done`
/// - `max_tokens` → `MaxTokenLimit`
/// - anything else → `Other`
fn map_anthropic_stop_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "end_turn" | "tool_use" | "stop_sequence" => Reason::Done(Done {}),
        "max_tokens" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
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
