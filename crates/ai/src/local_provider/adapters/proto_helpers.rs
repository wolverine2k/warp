//! Shared proto-event builders used by adapter stream decoders. Stateless
//! and adapter-agnostic — the produced events are identical regardless of
//! whether they came from an SSE, NDJSON, or future framing.
//!
//! Split out as part of Phase 3b. `OllamaDecoder` uses these from the
//! start; `AnthropicSseDecoder` still has inline copies (migration is a
//! Phase 4 polish — defers risk to a separately-reviewable diff).
//! `OpenAiSseAdapter` has its own copies that include OpenAi-specific
//! usage mapping; not worth migrating.

use uuid::Uuid;
use warp_multi_agent_api as api;

/// Kind of streamed message the decoder is opening or appending to.
/// `AgentReasoning` is currently only emitted by `AnthropicSseDecoder`
/// (which has its own inline copy of `build_kind_message` and doesn't go
/// through this module — see the file-level comment). When that decoder
/// migrates to `proto_helpers`, the variant is used here too. Kept now
/// to avoid an awkward enum-grows-later refactor.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum MessageKind {
    AgentOutput,
    AgentReasoning,
}

pub fn build_kind_message(message_id: &str, kind: MessageKind, text: &str) -> api::Message {
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

pub fn build_client_action_event(action: api::client_action::Action) -> api::ResponseEvent {
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

pub fn client_action_begin() -> api::client_action::Action {
    api::client_action::Action::BeginTransaction(api::client_action::BeginTransaction {})
}

pub fn client_action_create_task(task_id: &str) -> api::client_action::Action {
    api::client_action::Action::CreateTask(api::client_action::CreateTask {
        task: Some(api::Task {
            id: task_id.to_string(),
            ..Default::default()
        }),
    })
}

pub fn client_action_commit() -> api::client_action::Action {
    api::client_action::Action::CommitTransaction(api::client_action::CommitTransaction {})
}

pub fn client_action_rollback() -> api::client_action::Action {
    api::client_action::Action::RollbackTransaction(api::client_action::RollbackTransaction {})
}

pub fn internal_error_reason(message: &str) -> api::response_event::stream_finished::Reason {
    api::response_event::stream_finished::Reason::InternalError(
        api::response_event::stream_finished::InternalError {
            message: message.to_string(),
        },
    )
}

/// Build an `AddMessagesToTask{ToolCall}` event from accumulated tool-use
/// state. Args parse failures (unknown tool name, malformed JSON, schema
/// violation) surface as a synthetic `AgentOutput` explaining the
/// failure — matches `OpenAiSseAdapter` / `AnthropicSseDecoder` behavior.
///
/// `args_json` is the stringified-JSON arguments. Callers with structured
/// `serde_json::Value` arguments stringify first via
/// `serde_json::to_string`.
pub fn build_tool_call_event(
    task_id: &str,
    id: &str,
    name: &str,
    args_json: &str,
) -> Option<api::ResponseEvent> {
    // Adapters that accept tools with zero required fields may emit an
    // empty args string; pass "{}" to the parser so the typed proto
    // variant gets built with default values rather than rejecting
    // outright.
    let args_for_parse = if args_json.trim().is_empty() {
        "{}"
    } else {
        args_json
    };
    let tool_call = match crate::local_provider::tools::translate_openai_tool_call(
        id,
        name,
        args_for_parse,
    ) {
        Ok(tc) => tc,
        Err(e) => {
            let body = format!(
                "I tried to call `{name}` but its arguments were unusable: {e}\n\nRaw args: {args_json}"
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
