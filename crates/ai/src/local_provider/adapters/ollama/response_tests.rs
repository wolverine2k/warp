//! Unit tests for `OllamaDecoder`. Driven by feeding NDJSON chunks
//! directly — no HTTP, no line splitter.

use warp_multi_agent_api as api;

use super::response::OllamaDecoder;

fn decoder() -> OllamaDecoder {
    OllamaDecoder::with_ids(
        "conv-1".into(),
        "req-1".into(),
        "run-1".into(),
        "task-1".into(),
    )
}

// ---- helpers ----

fn feed(d: &mut OllamaDecoder, chunk: &str) -> Vec<api::ResponseEvent> {
    d.feed_event(None, chunk)
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
            assert_eq!(ca.actions.len(), 1);
            ca.actions[0].action.as_ref().unwrap()
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

// ---- prelude ----

#[test]
fn first_chunk_emits_init_begin_create_task_prelude() {
    let mut d = decoder();
    let out = feed(
        &mut d,
        r#"{"model":"llama3.1","message":{"role":"assistant","content":""},"done":false}"#,
    );
    // Prelude is 3 events. The chunk itself doesn't add anything because
    // content is empty.
    assert_eq!(out.len(), 3);
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
    let out = feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":""},"done":false}"#,
    );
    // Init + BeginTransaction only.
    assert_eq!(out.len(), 2);
    assert!(!out
        .iter()
        .any(|ev| matches!(ev.r#type, Some(api::response_event::Type::ClientActions(ref ca))
            if matches!(ca.actions[0].action,
                Some(api::client_action::Action::CreateTask(_))
            ))));
}

#[test]
fn with_ids_round_trips_in_init() {
    let mut d = OllamaDecoder::with_ids(
        "my-conv".into(),
        "my-req".into(),
        "my-run".into(),
        "my-task".into(),
    );
    let out = feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":""},"done":false}"#,
    );
    let init = extract_init(&out[0]);
    assert_eq!(init.conversation_id, "my-conv");
    assert_eq!(init.request_id, "my-req");
    assert_eq!(init.run_id, "my-run");
}

// ---- text streaming ----

#[test]
fn simple_text_streaming_emits_canonical_sequence() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(
        &mut d,
        r#"{"model":"llama3.1","message":{"role":"assistant","content":"Hello"},"done":false}"#,
    ));
    events.extend(feed(
        &mut d,
        r#"{"model":"llama3.1","message":{"role":"assistant","content":" world"},"done":false}"#,
    ));
    events.extend(feed(
        &mut d,
        r#"{"model":"llama3.1","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":20}"#,
    ));
    assert!(d.is_terminal());
    events.extend(d.finish());

    // Expected: Init + Begin + CreateTask + AddMessages("Hello") +
    // AppendContent(" world") + Commit + Finished{Done}.
    assert_eq!(events.len(), 7, "{:#?}", events);
    // The 4th event (index 3) is the AddMessages with first chunk text.
    match extract_action(&events[3]) {
        api::client_action::Action::AddMessagesToTask(amt) => {
            assert_eq!(amt.task_id, "task-1");
            match amt.messages[0].message.as_ref().unwrap() {
                api::message::Message::AgentOutput(ao) => assert_eq!(ao.text, "Hello"),
                other => panic!("expected AgentOutput, got {other:?}"),
            }
        }
        other => panic!("expected AddMessagesToTask, got {other:?}"),
    }
    match extract_action(&events[4]) {
        api::client_action::Action::AppendToMessageContent(app) => {
            match app.message.as_ref().unwrap().message.as_ref().unwrap() {
                api::message::Message::AgentOutput(ao) => assert_eq!(ao.text, " world"),
                other => panic!("expected AgentOutput, got {other:?}"),
            }
        }
        other => panic!("expected AppendToMessageContent, got {other:?}"),
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

#[test]
fn empty_message_content_silently_skipped() {
    let mut d = decoder();
    let out = feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":""},"done":false}"#,
    );
    // Just the prelude — no Append/AddMessages for empty content.
    assert_eq!(out.len(), 3);
}

// ---- tool calls ----

#[test]
fn tool_call_emits_one_addmessages_event_per_call() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_files","arguments":{"paths":["Cargo.toml"]}}}]},"done":false}"#,
    ));
    events.extend(feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#,
    ));
    events.extend(d.finish());

    let tool_call = events.iter().find_map(|e| match &e.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
            if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                amt.messages.first().and_then(|m| match m.message.as_ref() {
                    Some(api::message::Message::ToolCall(tc)) => Some(tc),
                    _ => None,
                })
            } else {
                None
            }
        }),
        _ => None,
    });
    let tc = tool_call.expect("ToolCall event not found");
    // Synthesized id starts with "ollama-call-".
    assert!(
        tc.tool_call_id.starts_with("ollama-call-"),
        "unexpected id: {}",
        tc.tool_call_id
    );
    match tc.tool.as_ref().unwrap() {
        api::message::tool_call::Tool::ReadFiles(rf) => {
            assert_eq!(rf.files[0].name, "Cargo.toml");
        }
        other => panic!("expected ReadFiles, got {other:?}"),
    }
}

#[test]
fn multiple_tool_calls_in_one_chunk_emit_separate_events() {
    let mut d = decoder();
    let out = feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":"","tool_calls":[
            {"function":{"name":"read_files","arguments":{"paths":["a"]}}},
            {"function":{"name":"read_files","arguments":{"paths":["b"]}}}
        ]},"done":false}"#,
    );
    // Prelude (3) + 2 tool-call events.
    assert_eq!(out.len(), 5);
    let tool_calls: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                    amt.messages.first().and_then(|m| match m.message.as_ref() {
                        Some(api::message::Message::ToolCall(tc)) => Some(tc),
                        _ => None,
                    })
                } else {
                    None
                }
            }),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 2);
    // Each tool call gets a distinct synthesized id.
    assert_ne!(tool_calls[0].tool_call_id, tool_calls[1].tool_call_id);
}

#[test]
fn text_then_tool_call_in_same_chunk_emits_both() {
    let mut d = decoder();
    let out = feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":"Reading.","tool_calls":[{"function":{"name":"read_files","arguments":{"paths":["x"]}}}]},"done":false}"#,
    );
    // Prelude (3) + AddMessages(text) + AddMessages(ToolCall).
    assert_eq!(out.len(), 5);
}

// ---- done / done_reason ----

#[test]
fn done_true_transitions_to_terminal() {
    let mut d = decoder();
    feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":"hi"},"done":false}"#,
    );
    assert!(!d.is_terminal());
    feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#,
    );
    assert!(d.is_terminal());
}

#[test]
fn done_reason_stop_maps_to_done() {
    let finished = drive_to_done_reason("stop");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn done_reason_length_maps_to_max_token_limit() {
    let finished = drive_to_done_reason("length");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(_))
    ));
}

#[test]
fn unknown_done_reason_maps_to_other() {
    let finished = drive_to_done_reason("vendor_specific");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

fn drive_to_done_reason(reason: &str) -> api::response_event::StreamFinished {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(
        &mut d,
        &format!(
            r#"{{"message":{{"role":"assistant","content":""}},"done":true,"done_reason":"{reason}"}}"#
        ),
    ));
    events.extend(d.finish());
    extract_finished(events.last().unwrap()).clone()
}

// ---- errors ----

#[test]
fn top_level_error_surfaces_as_internal_error() {
    let mut d = decoder();
    feed(&mut d, r#"{"error":"model 'foo' not found"}"#);
    assert!(d.is_terminal());
    let closing = d.finish();
    // Rollback emitted (no done_reason), then Finished{InternalError}.
    assert!(closing.iter().any(|e| matches!(
        e.r#type,
        Some(api::response_event::Type::ClientActions(ref ca))
            if matches!(
                ca.actions[0].action,
                Some(api::client_action::Action::RollbackTransaction(_))
            )
    )));
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(ie.message.contains("model 'foo' not found"));
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn malformed_chunk_transitions_to_errored() {
    let mut d = decoder();
    feed(&mut d, r#"{not valid json"#);
    assert!(d.is_terminal());
}

#[test]
fn premature_eof_produces_rollback_internal_error() {
    let mut d = decoder();
    feed(
        &mut d,
        r#"{"message":{"role":"assistant","content":"partial"},"done":false}"#,
    );
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
fn record_upstream_error_surfaces_when_no_done_reason() {
    let mut d = decoder();
    feed(&mut d, r#"{"message":{"role":"assistant","content":""},"done":false}"#);
    d.record_upstream_error("HTTP 503: model loading".into());
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(ie.message.contains("HTTP 503"));
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

// ---- usage ----

#[test]
fn usage_from_done_chunk_merged_into_token_usage() {
    let mut d = decoder();
    feed(&mut d, r#"{"model":"llama3.1","message":{"role":"assistant","content":"hi"},"done":false}"#);
    feed(&mut d, r#"{"model":"llama3.1","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":50,"eval_count":120}"#);
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert_eq!(finished.token_usage.len(), 1);
    let usage = &finished.token_usage[0];
    assert_eq!(usage.model_id, "llama3.1");
    assert_eq!(usage.total_input, 50);
    assert_eq!(usage.output, 120);
    assert_eq!(usage.input_cache_read, 0);
    assert_eq!(usage.input_cache_write, 0);
}

#[test]
fn token_usage_model_defaults_to_ollama_when_chunk_omits_model() {
    let mut d = decoder();
    feed(&mut d, r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":10}"#);
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert_eq!(finished.token_usage[0].model_id, "ollama");
}

#[test]
fn token_usage_omitted_when_no_eval_counts() {
    let mut d = decoder();
    feed(&mut d, r#"{"message":{"role":"assistant","content":"hi"},"done":true,"done_reason":"stop"}"#);
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    // No usage when counts are zero (or absent).
    assert!(finished.token_usage.is_empty());
}

// ---- terminal safety ----

#[test]
fn post_done_feeds_are_no_ops() {
    let mut d = decoder();
    feed(&mut d, r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#);
    assert!(d.is_terminal());
    let stray = feed(&mut d, r#"{"message":{"role":"assistant","content":"ignored"},"done":false}"#);
    assert_eq!(stray.len(), 0);
}

#[test]
fn empty_data_lines_silently_skipped() {
    let mut d = decoder();
    let out_empty = feed(&mut d, "   ");
    assert!(out_empty.is_empty());
    // Followed by a real chunk, prelude still emits.
    let out_chunk = feed(&mut d, r#"{"message":{"role":"assistant","content":""},"done":false}"#);
    assert_eq!(out_chunk.len(), 3); // just the prelude
}
