//! Unit tests for `GeminiSseDecoder`. Driven by feeding SSE data chunks
//! directly — no HTTP, no line splitter.

use warp_multi_agent_api as api;

use super::response::GeminiSseDecoder;

fn decoder() -> GeminiSseDecoder {
    GeminiSseDecoder::with_ids(
        "conv-1".into(),
        "req-1".into(),
        "run-1".into(),
        "task-1".into(),
    )
}

// ---- helpers ----

fn feed(d: &mut GeminiSseDecoder, chunk: &str) -> Vec<api::ResponseEvent> {
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

fn make_text_chunk(text: &str) -> String {
    format!(
        r#"{{"candidates":[{{"content":{{"role":"model","parts":[{{"text":"{text}"}}]}}}}]}}"#
    )
}

fn make_finish_chunk(reason: &str) -> String {
    format!(r#"{{"candidates":[{{"finishReason":"{reason}","index":0}}]}}"#)
}

fn make_finish_chunk_with_usage(reason: &str, input: u64, output: u64) -> String {
    format!(
        r#"{{"candidates":[{{"finishReason":"{reason}","index":0}}],"usageMetadata":{{"promptTokenCount":{input},"candidatesTokenCount":{output},"totalTokenCount":{}}}}}"#,
        input + output
    )
}

fn drive_to_finish_reason(reason: &str) -> api::response_event::StreamFinished {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(&mut d, &make_finish_chunk(reason)));
    events.extend(d.finish());
    extract_finished(events.last().unwrap()).clone()
}

// ---- prelude ----

#[test]
fn prelude_emitted_on_first_non_empty_feed() {
    let mut d = decoder();
    let out = feed(&mut d, &make_text_chunk("hi"));
    // Prelude is 3 events (Init + Begin + CreateTask) + 1 AddMessages.
    assert_eq!(out.len(), 4);
    extract_init(&out[0]);
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
fn with_ids_round_trips_into_init() {
    let mut d = GeminiSseDecoder::with_ids(
        "my-conv".into(),
        "my-req".into(),
        "my-run".into(),
        "my-task".into(),
    );
    let out = feed(&mut d, &make_text_chunk("hello"));
    let init = extract_init(&out[0]);
    assert_eq!(init.conversation_id, "my-conv");
    assert_eq!(init.request_id, "my-req");
    assert_eq!(init.run_id, "my-run");
}

#[test]
fn skip_create_task_suppresses_create_task() {
    let mut d = decoder();
    d.skip_create_task();
    let out = feed(&mut d, &make_text_chunk("hi"));
    // Init + BeginTransaction + AddMessages — no CreateTask.
    assert!(!out.iter().any(|ev| matches!(
        &ev.r#type,
        Some(api::response_event::Type::ClientActions(ca))
            if matches!(
                ca.actions[0].action,
                Some(api::client_action::Action::CreateTask(_))
            )
    )));
    // Also confirm Init and Begin are still present.
    assert_eq!(out.len(), 3);
}

// ---- text streaming ----

#[test]
fn simple_text_streaming_builds_canonical_event_sequence() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(&mut d, &make_text_chunk("Hello")));
    events.extend(feed(&mut d, &make_text_chunk(" ")));
    events.extend(feed(&mut d, &make_text_chunk("world")));
    events.extend(feed(&mut d, &make_finish_chunk("STOP")));
    assert!(d.is_terminal());
    events.extend(d.finish());

    // Expected: Init + Begin + CreateTask + AddMessages("Hello") +
    // AppendContent(" ") + AppendContent("world") + Commit + Finished{Done}.
    assert_eq!(events.len(), 8, "{events:#?}");

    // Index 3: AddMessages with "Hello"
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
    // Index 4: AppendToMessageContent(" ")
    match extract_action(&events[4]) {
        api::client_action::Action::AppendToMessageContent(app) => {
            match app.message.as_ref().unwrap().message.as_ref().unwrap() {
                api::message::Message::AgentOutput(ao) => assert_eq!(ao.text, " "),
                other => panic!("expected AgentOutput, got {other:?}"),
            }
        }
        other => panic!("expected AppendToMessageContent, got {other:?}"),
    }
    // Index 5: AppendToMessageContent("world")
    match extract_action(&events[5]) {
        api::client_action::Action::AppendToMessageContent(app) => {
            match app.message.as_ref().unwrap().message.as_ref().unwrap() {
                api::message::Message::AgentOutput(ao) => assert_eq!(ao.text, "world"),
                other => panic!("expected AgentOutput, got {other:?}"),
            }
        }
        other => panic!("expected AppendToMessageContent, got {other:?}"),
    }
    // Index 6: CommitTransaction
    assert!(matches!(
        extract_action(&events[6]),
        api::client_action::Action::CommitTransaction(_)
    ));
    // Index 7: Finished{Done}
    let finished = extract_finished(&events[7]);
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

// ---- function calls ----

#[test]
fn function_call_in_one_chunk_emits_add_messages_tool_call() {
    let mut d = decoder();
    let chunk = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_files","args":{"paths":["Cargo.toml"]}}}]}}]}"#;
    let out = feed(&mut d, chunk);

    let tool_call = out.iter().find_map(|e| match &e.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().and_then(|a| {
            if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                amt.messages.first().and_then(|m| match m.message.as_ref() {
                    Some(api::message::Message::ToolCall(tc)) => Some(tc.clone()),
                    _ => None,
                })
            } else {
                None
            }
        }),
        _ => None,
    });
    let tc = tool_call.expect("ToolCall event not found");
    assert!(
        tc.tool_call_id.starts_with("gemini-call-"),
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
fn multiple_function_calls_in_one_chunk_emit_multiple_events() {
    let mut d = decoder();
    let chunk = r#"{"candidates":[{"content":{"role":"model","parts":[
        {"functionCall":{"name":"read_files","args":{"paths":["a"]}}},
        {"functionCall":{"name":"read_files","args":{"paths":["b"]}}}
    ]}}]}"#;
    let out = feed(&mut d, chunk);

    let tool_calls: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => {
                ca.actions.first().and_then(|a| {
                    if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                        amt.messages.first().and_then(|m| match m.message.as_ref() {
                            Some(api::message::Message::ToolCall(tc)) => Some(tc.clone()),
                            _ => None,
                        })
                    } else {
                        None
                    }
                })
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 2);
    // Each tool call gets a distinct synthesized id.
    assert_ne!(tool_calls[0].tool_call_id, tool_calls[1].tool_call_id);
}

#[test]
fn function_call_then_finish_reason_in_same_chunk_emits_tool_then_transitions_to_done() {
    let mut d = decoder();
    let chunk = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_files","args":{"paths":["x"]}}}]},"finishReason":"STOP"}]}"#;
    let out = feed(&mut d, chunk);

    // Tool-call event must be present.
    let has_tool_call = out.iter().any(|e| match &e.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().is_some_and(|a| {
            if let Some(api::client_action::Action::AddMessagesToTask(amt)) = &a.action {
                amt.messages.first().is_some_and(|m| {
                    matches!(m.message.as_ref(), Some(api::message::Message::ToolCall(_)))
                })
            } else {
                false
            }
        }),
        _ => false,
    });
    assert!(has_tool_call, "expected tool-call event in output");
    assert!(d.is_terminal());
}

// ---- finish reason mapping ----

#[test]
fn finish_reason_stop_maps_to_done() {
    let finished = drive_to_finish_reason("STOP");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn finish_reason_max_tokens_maps_to_max_token_limit() {
    let finished = drive_to_finish_reason("MAX_TOKENS");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(_))
    ));
}

#[test]
fn finish_reason_safety_maps_to_other() {
    let finished = drive_to_finish_reason("SAFETY");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

#[test]
fn finish_reason_malformed_function_call_maps_to_other() {
    let finished = drive_to_finish_reason("MALFORMED_FUNCTION_CALL");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

#[test]
fn finish_reason_unknown_maps_to_other() {
    let finished = drive_to_finish_reason("WEIRD");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

// ---- errors ----

#[test]
fn top_level_error_field_surfaces_as_internal_error_on_finish() {
    let mut d = decoder();
    let chunk = r#"{"error":{"code":400,"message":"API key not valid.","status":"INVALID_ARGUMENT"}}"#;
    feed(&mut d, chunk);
    assert!(d.is_terminal());

    let closing = d.finish();
    // Rollback emitted (no finishReason), then Finished{InternalError}.
    assert!(closing.iter().any(|e| matches!(
        &e.r#type,
        Some(api::response_event::Type::ClientActions(ca))
            if matches!(
                ca.actions[0].action,
                Some(api::client_action::Action::RollbackTransaction(_))
            )
    )));
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(
                ie.message.contains("API key not valid."),
                "unexpected message: {}",
                ie.message
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn malformed_json_chunk_transitions_to_errored() {
    let mut d = decoder();
    feed(&mut d, r#"{not valid json"#);
    assert!(d.is_terminal());

    // Subsequent feeds are no-ops.
    let stray = feed(&mut d, &make_text_chunk("ignored"));
    assert_eq!(stray.len(), 0);

    // finish() emits Rollback.
    let closing = d.finish();
    assert!(closing.iter().any(|e| matches!(
        &e.r#type,
        Some(api::response_event::Type::ClientActions(ca))
            if matches!(
                ca.actions[0].action,
                Some(api::client_action::Action::RollbackTransaction(_))
            )
    )));
}

#[test]
fn premature_eof_without_finish_reason_emits_rollback_and_internal_error() {
    let mut d = decoder();
    feed(&mut d, &make_text_chunk("partial"));
    assert!(!d.is_terminal());

    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(
                ie.message.contains("stream ended"),
                "unexpected message: {}",
                ie.message
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn record_upstream_error_surfaces_in_finish_when_no_finish_reason() {
    let mut d = decoder();
    feed(&mut d, &make_text_chunk("partial"));
    d.record_upstream_error("custom error from HTTP layer".into());
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    match finished.reason.as_ref().unwrap() {
        api::response_event::stream_finished::Reason::InternalError(ie) => {
            assert!(
                ie.message.contains("custom error from HTTP layer"),
                "unexpected message: {}",
                ie.message
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

// ---- usage metadata ----

#[test]
fn usage_metadata_from_any_chunk_merged_into_token_usage() {
    let mut d = decoder();
    // First chunk with lower counts.
    let chunk1 = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#;
    // Second chunk with higher counts — last-seen wins via .max().
    let chunk2 = make_finish_chunk_with_usage("STOP", 50, 120);
    feed(&mut d, chunk1);
    feed(&mut d, &chunk2);
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert_eq!(finished.token_usage.len(), 1);
    let usage = &finished.token_usage[0];
    assert_eq!(usage.total_input, 50);
    assert_eq!(usage.output, 120);
    assert_eq!(usage.model_id, "gemini");
}

// ---- edge cases ----

#[test]
fn final_chunk_with_empty_parts_array_does_not_emit_spurious_append() {
    let mut d = decoder();
    // Chunk with empty candidates content + finishReason but no parts.
    let chunk = r#"{"candidates":[{"finishReason":"STOP","index":0}]}"#;
    let out = feed(&mut d, chunk);
    assert!(d.is_terminal());

    // Only prelude (3 events) — no AddMessages or AppendToMessageContent.
    assert_eq!(out.len(), 3, "unexpected events: {out:#?}");
    let has_append = out.iter().any(|e| match &e.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().is_some_and(|a| {
            matches!(
                &a.action,
                Some(api::client_action::Action::AddMessagesToTask(_))
                    | Some(api::client_action::Action::AppendToMessageContent(_))
            )
        }),
        _ => false,
    });
    assert!(!has_append, "spurious text event emitted for empty parts");

    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn unknown_part_variant_is_silently_ignored() {
    let mut d = decoder();
    // A "thought" part that doesn't match any known variant falls into Unknown.
    let chunk = r#"{"candidates":[{"content":{"role":"model","parts":[{"thought":"hmm"}]},"finishReason":"STOP"}]}"#;
    let out = feed(&mut d, chunk);

    // Should not error — state is Done.
    assert!(d.is_terminal());

    // No text/tool-call events emitted for the unknown part.
    let has_content_event = out.iter().any(|e| match &e.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => ca.actions.first().is_some_and(|a| {
            matches!(
                &a.action,
                Some(api::client_action::Action::AddMessagesToTask(_))
                    | Some(api::client_action::Action::AppendToMessageContent(_))
            )
        }),
        _ => false,
    });
    assert!(!has_content_event, "unexpected content event for Unknown part");

    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn terminal_state_safety_post_done_feeds_are_no_ops() {
    let mut d = decoder();
    feed(&mut d, &make_finish_chunk("STOP"));
    assert!(d.is_terminal());

    let stray = feed(&mut d, &make_text_chunk("should be ignored"));
    assert_eq!(stray.len(), 0, "expected no-op after terminal state");

    // State should still be terminal, no panics.
    assert!(d.is_terminal());
}
