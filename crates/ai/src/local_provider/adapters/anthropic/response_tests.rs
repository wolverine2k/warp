//! Unit tests for `AnthropicSseDecoder`. Driving by feeding event JSON
//! strings directly — no HTTP, no SSE parser.

use warp_multi_agent_api as api;

use super::response::AnthropicSseDecoder;

fn decoder() -> AnthropicSseDecoder {
    AnthropicSseDecoder::with_ids(
        "conv-1".into(),
        "req-1".into(),
        "run-1".into(),
        "task-1".into(),
    )
}

fn feed_named(d: &mut AnthropicSseDecoder, name: &str, data: &str) -> Vec<api::ResponseEvent> {
    d.feed_event(Some(name), data)
}

// ---- helpers for matching events ----

fn is_init(ev: &api::ResponseEvent) -> bool {
    matches!(ev.r#type, Some(api::response_event::Type::Init(_)))
}

fn extract_init(ev: &api::ResponseEvent) -> &api::response_event::StreamInit {
    match &ev.r#type {
        Some(api::response_event::Type::Init(i)) => i,
        other => panic!("expected Init, got {other:?}"),
    }
}

fn extract_action(ev: &api::ResponseEvent) -> &api::client_action::Action {
    match &ev.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => {
            assert_eq!(ca.actions.len(), 1, "expected 1 action, got {}", ca.actions.len());
            ca.actions[0]
                .action
                .as_ref()
                .expect("action present on ClientAction")
        }
        other => panic!("expected ClientActions, got {other:?}"),
    }
}

fn extract_finished(ev: &api::ResponseEvent) -> &api::response_event::StreamFinished {
    match &ev.r#type {
        Some(api::response_event::Type::Finished(f)) => f,
        other => panic!("expected Finished, got {other:?}"),
    }
}

fn extract_add_messages(action: &api::client_action::Action) -> &api::client_action::AddMessagesToTask {
    match action {
        api::client_action::Action::AddMessagesToTask(a) => a,
        other => panic!("expected AddMessagesToTask, got {other:?}"),
    }
}

fn extract_append_content(action: &api::client_action::Action) -> &api::client_action::AppendToMessageContent {
    match action {
        api::client_action::Action::AppendToMessageContent(a) => a,
        other => panic!("expected AppendToMessageContent, got {other:?}"),
    }
}

// ---- prelude ----

#[test]
fn first_event_emits_init_begin_create_task_prelude() {
    let mut d = decoder();
    let out = feed_named(
        &mut d,
        "message_start",
        r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6","usage":{"input_tokens":10,"output_tokens":1}}}"#,
    );
    // The prelude itself is 3 events (Init + BeginTransaction + CreateTask).
    // MessageStart itself doesn't add anything beyond capturing usage.
    assert_eq!(out.len(), 3);
    assert!(is_init(&out[0]));
    let init = extract_init(&out[0]);
    assert_eq!(init.conversation_id, "conv-1");
    assert_eq!(init.request_id, "req-1");
    assert_eq!(init.run_id, "run-1");
    assert!(matches!(
        extract_action(&out[1]),
        api::client_action::Action::BeginTransaction(_)
    ));
    match extract_action(&out[2]) {
        api::client_action::Action::CreateTask(ct) => {
            assert_eq!(ct.task.as_ref().unwrap().id, "task-1");
        }
        other => panic!("expected CreateTask, got {other:?}"),
    }
}

#[test]
fn skip_create_task_suppresses_create_task_action() {
    let mut d = decoder();
    d.skip_create_task();
    let out = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    assert_eq!(out.len(), 2); // Init + BeginTransaction only
    assert!(is_init(&out[0]));
    assert!(matches!(
        extract_action(&out[1]),
        api::client_action::Action::BeginTransaction(_)
    ));
}

#[test]
fn with_ids_round_trips_in_init_event() {
    let mut d = AnthropicSseDecoder::with_ids(
        "my-conv".into(),
        "my-req".into(),
        "my-run".into(),
        "my-task".into(),
    );
    let out = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let init = extract_init(&out[0]);
    assert_eq!(init.conversation_id, "my-conv");
    assert_eq!(init.request_id, "my-req");
    assert_eq!(init.run_id, "my-run");
}

// ---- text streaming ----

#[test]
fn simple_text_message_emits_canonical_event_sequence() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();

    events.extend(feed_named(
        &mut d,
        "message_start",
        r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6","usage":{"input_tokens":10,"output_tokens":1}}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_stop",
        r#"{"type":"content_block_stop","index":0}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "message_delta",
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#,
    ));
    assert!(!d.is_terminal(), "Finishing state isn't terminal yet");
    events.extend(feed_named(
        &mut d,
        "message_stop",
        r#"{"type":"message_stop"}"#,
    ));
    assert!(d.is_terminal(), "message_stop transitions to Done");
    events.extend(d.finish());

    // Expected event order:
    //   0  Init
    //   1  BeginTransaction
    //   2  CreateTask
    //   3  AddMessagesToTask{ AgentOutput "Hello" }
    //   4  AppendToMessageContent{ AgentOutput " world" }
    //   5  CommitTransaction
    //   6  Finished{ Done, usage }
    assert_eq!(events.len(), 7, "events: {:#?}", events);
    assert!(is_init(&events[0]));
    let add = extract_add_messages(extract_action(&events[3]));
    assert_eq!(add.task_id, "task-1");
    assert_eq!(add.messages.len(), 1);
    match add.messages[0].message.as_ref().unwrap() {
        api::message::Message::AgentOutput(a) => assert_eq!(a.text, "Hello"),
        other => panic!("expected AgentOutput, got {other:?}"),
    }
    let append = extract_append_content(extract_action(&events[4]));
    assert_eq!(append.task_id, "task-1");
    match append.message.as_ref().unwrap().message.as_ref().unwrap() {
        api::message::Message::AgentOutput(a) => assert_eq!(a.text, " world"),
        other => panic!("expected AgentOutput, got {other:?}"),
    }
    assert!(matches!(
        extract_action(&events[5]),
        api::client_action::Action::CommitTransaction(_)
    ));
    let finished = extract_finished(&events[6]);
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

// ---- tool use ----

#[test]
fn tool_use_block_emits_single_tool_call_event() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();

    events.extend(feed_named(
        &mut d,
        "message_start",
        r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6","usage":{"input_tokens":20,"output_tokens":1}}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_01","name":"read_files","input":{}}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"paths\":"}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"[\"Cargo.toml\"]}"}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_stop",
        r#"{"type":"content_block_stop","index":0}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "message_delta",
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":25}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "message_stop",
        r#"{"type":"message_stop"}"#,
    ));
    events.extend(d.finish());

    // Walk forward looking for the AddMessagesToTask{ToolCall} event.
    let tool_event = events
        .iter()
        .find_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => {
                ca.actions.first().and_then(|a| {
                    if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                        amt.messages.first().and_then(|m| match m.message.as_ref() {
                            Some(api::message::Message::ToolCall(tc)) => Some(tc),
                            _ => None,
                        })
                    } else {
                        None
                    }
                })
            }
            _ => None,
        })
        .expect("ToolCall event not found");
    assert_eq!(tool_event.tool_call_id, "toolu_01");
    match tool_event.tool.as_ref().unwrap() {
        api::message::tool_call::Tool::ReadFiles(rf) => {
            assert_eq!(rf.files.len(), 1);
            assert_eq!(rf.files[0].name, "Cargo.toml");
        }
        other => panic!("expected ReadFiles, got {other:?}"),
    }

    let finished = extract_finished(events.last().unwrap());
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn interleaved_text_and_tool_use_at_different_indices() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();

    events.extend(feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#));
    // Text block at index 0
    events.extend(feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Reading file."}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_stop",
        r#"{"type":"content_block_stop","index":0}"#,
    ));
    // Tool-use block at index 1
    events.extend(feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_x","name":"run_shell_command","input":{}}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_stop",
        r#"{"type":"content_block_stop","index":1}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "message_delta",
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
    ));
    events.extend(feed_named(&mut d, "message_stop", r#"{"type":"message_stop"}"#));
    events.extend(d.finish());

    // Find the AddMessagesToTask events in order: text first, then tool_call.
    let add_msgs: Vec<&api::client_action::AddMessagesToTask> = events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                    Some(amt)
                } else {
                    None
                }
            }),
            _ => None,
        })
        .collect();
    assert!(
        add_msgs.len() >= 2,
        "expected at least 2 AddMessagesToTask events, got {}",
        add_msgs.len()
    );
    // First AddMessages is the text block; second is the tool_call.
    assert!(matches!(
        add_msgs[0].messages[0].message.as_ref().unwrap(),
        api::message::Message::AgentOutput(_)
    ));
    assert!(matches!(
        add_msgs[1].messages[0].message.as_ref().unwrap(),
        api::message::Message::ToolCall(_)
    ));
}

#[test]
fn tool_use_with_empty_args_passes_object_default() {
    // Defensive: a tool_use block that emits no input_json_delta chunks
    // should still produce a valid ToolCall (parsed against "{}").
    // Most tools require fields, so this exercises the parse-error path.
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#));
    events.extend(feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_y","name":"read_files","input":{}}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_stop",
        r#"{"type":"content_block_stop","index":0}"#,
    ));
    // The synthesized AgentOutput message describes the parse failure
    // (read_files requires `paths`) — see build_tool_call_event.
    let synthetic = events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                    Some(amt)
                } else {
                    None
                }
            }),
            _ => None,
        })
        .find(|amt| {
            matches!(
                amt.messages[0].message.as_ref().unwrap(),
                api::message::Message::AgentOutput(_)
            )
        })
        .expect("expected synthetic AgentOutput for unparseable tool args");
    if let api::message::Message::AgentOutput(ao) = synthetic.messages[0].message.as_ref().unwrap() {
        assert!(
            ao.text.contains("read_files") && ao.text.contains("unusable"),
            "unexpected synthetic text: {}",
            ao.text
        );
    }
}

// ---- thinking ----

#[test]
fn thinking_blocks_emit_agent_reasoning() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#));
    events.extend(feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"reasoning step 1"}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "content_block_stop",
        r#"{"type":"content_block_stop","index":0}"#,
    ));

    let reasoning = events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                    Some(amt)
                } else {
                    None
                }
            }),
            _ => None,
        })
        .find(|amt| {
            matches!(
                amt.messages[0].message.as_ref().unwrap(),
                api::message::Message::AgentReasoning(_)
            )
        })
        .expect("expected AgentReasoning message");
    if let api::message::Message::AgentReasoning(ar) = reasoning.messages[0].message.as_ref().unwrap() {
        assert_eq!(ar.reasoning, "reasoning step 1");
    }
}

// ---- ignored events ----

#[test]
fn signature_delta_is_silently_consumed() {
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let _ = feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
    );
    let out = feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
    );
    // Signature delta produces no downstream events.
    assert_eq!(out.len(), 0);
}

#[test]
fn ping_event_is_silently_consumed() {
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let out = feed_named(&mut d, "ping", r#"{"type":"ping"}"#);
    assert_eq!(out.len(), 0);
    assert!(!d.is_terminal());
}

// ---- errors ----

#[test]
fn error_event_surfaces_as_internal_error_on_finish() {
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let _ = feed_named(
        &mut d,
        "error",
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
    );
    assert!(d.is_terminal());
    let closing = d.finish();
    // Closing emits Rollback (no stop_reason captured) + Finished{InternalError}.
    let rollback_seen = closing
        .iter()
        .any(|e| matches!(e.r#type, Some(api::response_event::Type::ClientActions(ref ca))
            if matches!(
                ca.actions.first().and_then(|a| a.action.as_ref()),
                Some(api::client_action::Action::RollbackTransaction(_))
            )));
    assert!(rollback_seen, "expected Rollback in closing events");
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(
                ie.message.contains("overloaded_error") && ie.message.contains("Overloaded"),
                "unexpected reason: {}",
                ie.message
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn premature_eof_produces_rollback_and_internal_error_finished() {
    // Feed only message_start, then call finish() without ever seeing
    // message_delta(stop_reason) or message_stop.
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    assert!(!d.is_terminal());
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(ie.message.contains("stream ended"));
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn record_upstream_error_surfaces_in_finish_when_no_stop_reason() {
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    d.record_upstream_error("HTTP 401: invalid x-api-key".into());
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(ie.message.contains("HTTP 401"));
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn malformed_chunk_transitions_to_errored_state() {
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let _ = feed_named(&mut d, "content_block_delta", r#"{ this is not valid json"#);
    assert!(d.is_terminal());
}

// ---- stop_reason mapping ----

fn drive_to_stop_reason(stop_reason: &str) -> api::response_event::StreamFinished {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#));
    events.extend(feed_named(
        &mut d,
        "message_delta",
        &format!(r#"{{"type":"message_delta","delta":{{"stop_reason":"{stop_reason}"}}}}"#),
    ));
    events.extend(feed_named(&mut d, "message_stop", r#"{"type":"message_stop"}"#));
    events.extend(d.finish());
    extract_finished(events.last().unwrap()).clone()
}

#[test]
fn end_turn_stop_reason_maps_to_done() {
    let finished = drive_to_stop_reason("end_turn");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn tool_use_stop_reason_maps_to_done() {
    let finished = drive_to_stop_reason("tool_use");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn max_tokens_stop_reason_maps_to_max_token_limit() {
    let finished = drive_to_stop_reason("max_tokens");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(_))
    ));
}

#[test]
fn unknown_stop_reason_maps_to_other() {
    let finished = drive_to_stop_reason("vendor_specific_foo");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

// ---- usage ----

#[test]
fn usage_from_message_start_and_message_delta_merged_into_token_usage() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed_named(
        &mut d,
        "message_start",
        r#"{"type":"message_start","message":{"id":"m","model":"claude-sonnet-4-6","usage":{"input_tokens":100,"output_tokens":1,"cache_creation_input_tokens":50,"cache_read_input_tokens":30}}}"#,
    ));
    events.extend(feed_named(
        &mut d,
        "message_delta",
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":250}}"#,
    ));
    events.extend(feed_named(&mut d, "message_stop", r#"{"type":"message_stop"}"#));
    events.extend(d.finish());

    let finished = extract_finished(events.last().unwrap());
    assert_eq!(finished.token_usage.len(), 1);
    let usage = &finished.token_usage[0];
    assert_eq!(usage.model_id, "claude-sonnet-4-6");
    assert_eq!(usage.total_input, 100);
    assert_eq!(usage.output, 250); // updated by message_delta
    assert_eq!(usage.input_cache_write, 50);
    assert_eq!(usage.input_cache_read, 30);
}

#[test]
fn token_usage_falls_back_to_anthropic_when_model_absent() {
    let mut d = decoder();
    let _ = feed_named(
        &mut d,
        "message_start",
        r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":1}}}"#,
    );
    let _ = feed_named(
        &mut d,
        "message_delta",
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}}"#,
    );
    let _ = feed_named(&mut d, "message_stop", r#"{"type":"message_stop"}"#);
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert_eq!(finished.token_usage[0].model_id, "anthropic");
}

// ---- terminal-state safety ----

#[test]
fn terminal_state_ignored_for_subsequent_feeds() {
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let _ = feed_named(&mut d, "message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#);
    let _ = feed_named(&mut d, "message_stop", r#"{"type":"message_stop"}"#);
    assert!(d.is_terminal());
    // Further feeds are no-ops.
    let out = feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ignored"}}"#,
    );
    assert_eq!(out.len(), 0);
}

#[test]
fn finishing_state_ignores_stray_content_deltas() {
    // Once stop_reason has arrived, content deltas before message_stop are
    // dropped (servers that emit them post-stop_reason are non-compliant).
    let mut d = decoder();
    let _ = feed_named(&mut d, "message_start", r#"{"type":"message_start","message":{}}"#);
    let _ = feed_named(
        &mut d,
        "content_block_start",
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
    );
    let _ = feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}"#,
    );
    let _ = feed_named(&mut d, "message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#);
    // Stray content delta — should produce no events.
    let stray = feed_named(
        &mut d,
        "content_block_delta",
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ignored"}}"#,
    );
    assert_eq!(stray.len(), 0);
    let _ = feed_named(&mut d, "message_stop", r#"{"type":"message_stop"}"#);
    let closing = d.finish();
    // Confirm the appended text in the resulting append events is only
    // the pre-stop chunk.
    let appended_texts: Vec<String> = closing
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
                match &a.action {
                    Some(api::client_action::Action::AppendToMessageContent(app)) => {
                        match app.message.as_ref().unwrap().message.as_ref().unwrap() {
                            api::message::Message::AgentOutput(ao) => Some(ao.text.clone()),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }),
            _ => None,
        })
        .collect();
    assert!(!appended_texts.iter().any(|t| t.contains("ignored")));
}
