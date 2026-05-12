//! Unit tests for `DeepSeekSseDecoder`. Driven by feeding SSE data chunks
//! directly — no HTTP, no line splitter.

use warp_multi_agent_api as api;

use super::response::DeepSeekSseDecoder;

// ---- helpers ----

fn decoder() -> DeepSeekSseDecoder {
    DeepSeekSseDecoder::with_ids(
        "conv-1".into(),
        "req-1".into(),
        "run-1".into(),
        "task-1".into(),
    )
}

fn feed(d: &mut DeepSeekSseDecoder, chunk: &str) -> Vec<api::ResponseEvent> {
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

/// A chunk with only `delta.content`.
fn make_text_chunk(text: &str) -> String {
    format!(
        r#"{{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{{"index":0,"delta":{{"content":"{text}"}},"finish_reason":null}}]}}"#
    )
}

/// A chunk with only `delta.reasoning_content`.
fn make_reasoning_chunk(text: &str) -> String {
    format!(
        r#"{{"id":"chatcmpl-x","model":"deepseek-reasoner","choices":[{{"index":0,"delta":{{"reasoning_content":"{text}"}},"finish_reason":null}}]}}"#
    )
}

/// A chunk that sets `finish_reason` with usage counters.
fn make_finish_chunk(reason: &str) -> String {
    format!(
        r#"{{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{{"index":0,"delta":{{}},"finish_reason":"{reason}"}}],"usage":{{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}}}"#
    )
}

/// Drive the decoder to a completed state and return the `StreamFinished` event.
fn drive_to_done(reason: &str) -> api::response_event::StreamFinished {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(&mut d, &make_finish_chunk(reason)));
    events.extend(feed(&mut d, "[DONE]"));
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
    let mut d = DeepSeekSseDecoder::with_ids(
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
    // Init and Begin are still present.
    assert_eq!(out.len(), 3);
}

// ---- text streaming ----

#[test]
fn simple_text_streaming_builds_canonical_event_sequence() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(&mut d, &make_text_chunk("Hello")));
    events.extend(feed(&mut d, &make_text_chunk(" world")));
    events.extend(feed(&mut d, &make_finish_chunk("stop")));
    events.extend(feed(&mut d, "[DONE]"));
    assert!(d.is_terminal());
    events.extend(d.finish());

    // Expected: Init + Begin + CreateTask + AddMessages("Hello") +
    // Append(" world") + Commit + Finished{Done}.
    assert_eq!(events.len(), 7, "{events:#?}");

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
    // Index 4: AppendToMessageContent(" world")
    match extract_action(&events[4]) {
        api::client_action::Action::AppendToMessageContent(app) => {
            match app.message.as_ref().unwrap().message.as_ref().unwrap() {
                api::message::Message::AgentOutput(ao) => assert_eq!(ao.text, " world"),
                other => panic!("expected AgentOutput, got {other:?}"),
            }
        }
        other => panic!("expected AppendToMessageContent, got {other:?}"),
    }
    // Index 5: CommitTransaction
    assert!(matches!(
        extract_action(&events[5]),
        api::client_action::Action::CommitTransaction(_)
    ));
    // Index 6: Finished{Done}
    let finished = extract_finished(&events[6]);
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

// ---- reasoning channel ----

#[test]
fn reasoning_streaming_emits_agent_reasoning_message() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    events.extend(feed(&mut d, &make_reasoning_chunk("Let me ")));
    events.extend(feed(&mut d, &make_reasoning_chunk("think...")));

    // First reasoning chunk: AddMessagesToTask with AgentReasoning.
    let first_reasoning_ev = events.iter().find_map(|ev| match &ev.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => {
            if let Some(api::client_action::Action::AddMessagesToTask(amt)) =
                &ca.actions[0].action
            {
                amt.messages.first().and_then(|m| match m.message.as_ref() {
                    Some(api::message::Message::AgentReasoning(ar)) => Some(ar.reasoning.clone()),
                    _ => None,
                })
            } else {
                None
            }
        }
        _ => None,
    });
    assert_eq!(
        first_reasoning_ev.as_deref(),
        Some("Let me "),
        "expected first AgentReasoning open with 'Let me '"
    );

    // Second reasoning chunk: AppendToMessageContent with AgentReasoning.
    let append_reasoning_ev = events.iter().find_map(|ev| match &ev.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => {
            if let Some(api::client_action::Action::AppendToMessageContent(app)) =
                &ca.actions[0].action
            {
                app.message.as_ref().and_then(|m| match m.message.as_ref() {
                    Some(api::message::Message::AgentReasoning(ar)) => Some(ar.reasoning.clone()),
                    _ => None,
                })
            } else {
                None
            }
        }
        _ => None,
    });
    assert_eq!(
        append_reasoning_ev.as_deref(),
        Some("think..."),
        "expected AppendToMessageContent AgentReasoning with 'think...'"
    );
}

#[test]
fn reasoning_then_content_emits_distinct_messages() {
    let mut d = decoder();
    let mut events: Vec<api::ResponseEvent> = Vec::new();
    // Feed reasoning first, then content.
    events.extend(feed(&mut d, &make_reasoning_chunk("thinking")));
    events.extend(feed(&mut d, &make_text_chunk("answer")));

    // Collect all AddMessagesToTask events and check for distinct message kinds.
    let add_messages: Vec<_> = events
        .iter()
        .filter_map(|ev| match &ev.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) =
                    &ca.actions[0].action
                {
                    Some(amt.messages.first()?.message.as_ref()?.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    // Must have two distinct AddMessages events: one AgentReasoning, one AgentOutput.
    assert_eq!(add_messages.len(), 2, "expected two distinct message opens");
    let has_reasoning = add_messages
        .iter()
        .any(|m| matches!(m, api::message::Message::AgentReasoning(_)));
    let has_output = add_messages
        .iter()
        .any(|m| matches!(m, api::message::Message::AgentOutput(_)));
    assert!(has_reasoning, "expected an AgentReasoning message");
    assert!(has_output, "expected an AgentOutput message");
}

#[test]
fn interleaved_reasoning_and_content_still_dispatches_correctly() {
    // A single chunk with BOTH reasoning_content and content non-empty.
    let chunk = r#"{"id":"chatcmpl-x","model":"deepseek-reasoner","choices":[{"index":0,"delta":{"reasoning_content":"thinking","content":"answer"},"finish_reason":null}]}"#;
    let mut d = decoder();
    let out = feed(&mut d, chunk);

    // Should produce: Init + Begin + CreateTask + AddMessages(AgentReasoning) + AddMessages(AgentOutput)
    assert_eq!(out.len(), 5, "{out:#?}");

    let add_msgs: Vec<_> = out
        .iter()
        .filter_map(|ev| match &ev.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) =
                    &ca.actions[0].action
                {
                    Some(amt.messages.first()?.message.as_ref()?.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    assert_eq!(add_msgs.len(), 2);
    assert!(matches!(add_msgs[0], api::message::Message::AgentReasoning(_)));
    assert!(matches!(add_msgs[1], api::message::Message::AgentOutput(_)));
}

#[test]
fn empty_reasoning_content_silently_skipped() {
    let chunk = r#"{"id":"chatcmpl-x","model":"deepseek-reasoner","choices":[{"index":0,"delta":{"reasoning_content":""},"finish_reason":null}]}"#;
    let mut d = decoder();
    let out = feed(&mut d, chunk);

    // Only prelude (3 events) — no AgentReasoning open.
    assert_eq!(out.len(), 3, "expected only prelude, got {out:#?}");
    let has_reasoning = out.iter().any(|ev| match &ev.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => {
            if let Some(api::client_action::Action::AddMessagesToTask(amt)) =
                &ca.actions[0].action
            {
                amt.messages.first().is_some_and(|m| {
                    matches!(m.message.as_ref(), Some(api::message::Message::AgentReasoning(_)))
                })
            } else {
                false
            }
        }
        _ => false,
    });
    assert!(!has_reasoning, "empty reasoning_content should be skipped");
}

#[test]
fn empty_content_silently_skipped() {
    let chunk = r#"{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{"index":0,"delta":{"content":""},"finish_reason":null}]}"#;
    let mut d = decoder();
    let out = feed(&mut d, chunk);

    // Only prelude (3 events) — no AgentOutput open.
    assert_eq!(out.len(), 3, "expected only prelude, got {out:#?}");
}

// ---- tool calls ----

#[test]
fn tool_call_fragments_accumulate_and_emit_on_completion() {
    let mut d = decoder();

    // Fragment 1: tool call index 0 — name + first arg fragment.
    let chunk1 = r#"{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"read_files","arguments":"{\"paths\":"}}]},"finish_reason":null}]}"#;
    // Fragment 2: remaining arg fragment.
    let chunk2 = r#"{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"[\"Cargo.toml\"]}"}}]},"finish_reason":null}]}"#;

    feed(&mut d, chunk1);
    feed(&mut d, chunk2);
    feed(&mut d, &make_finish_chunk("tool_calls"));
    feed(&mut d, "[DONE]");
    assert!(d.is_terminal());

    let closing = d.finish();

    // Find the ToolCall message in the finish output.
    let tool_call = closing.iter().find_map(|ev| match &ev.r#type {
        Some(api::response_event::Type::ClientActions(ca)) => {
            if let Some(api::client_action::Action::AddMessagesToTask(amt)) =
                &ca.actions[0].action
            {
                amt.messages.first().and_then(|m| match m.message.as_ref() {
                    Some(api::message::Message::ToolCall(tc)) => Some(tc.clone()),
                    _ => None,
                })
            } else {
                None
            }
        }
        _ => None,
    });
    let tc = tool_call.expect("expected a ToolCall event in finish output");
    assert_eq!(tc.tool_call_id, "call_abc");
    match tc.tool.as_ref().unwrap() {
        api::message::tool_call::Tool::ReadFiles(rf) => {
            assert_eq!(rf.files[0].name, "Cargo.toml");
        }
        other => panic!("expected ReadFiles tool, got {other:?}"),
    }
}

#[test]
fn multiple_tool_calls_emit_separately() {
    let mut d = decoder();

    // Two tool calls at index 0 and 1.
    let chunk1 = r#"{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"read_files","arguments":"{\"paths\":[\"a\"]}"}}]},"finish_reason":null}]}"#;
    let chunk2 = r#"{"id":"chatcmpl-x","model":"deepseek-chat","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"read_files","arguments":"{\"paths\":[\"b\"]}"}}]},"finish_reason":null}]}"#;

    feed(&mut d, chunk1);
    feed(&mut d, chunk2);
    feed(&mut d, &make_finish_chunk("tool_calls"));
    feed(&mut d, "[DONE]");
    assert!(d.is_terminal());

    let closing = d.finish();

    let tool_calls: Vec<_> = closing
        .iter()
        .filter_map(|ev| match &ev.r#type {
            Some(api::response_event::Type::ClientActions(ca)) => {
                if let Some(api::client_action::Action::AddMessagesToTask(amt)) =
                    &ca.actions[0].action
                {
                    amt.messages.first().and_then(|m| match m.message.as_ref() {
                        Some(api::message::Message::ToolCall(tc)) => Some(tc.clone()),
                        _ => None,
                    })
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    assert_eq!(tool_calls.len(), 2, "expected two ToolCall events");
    assert_eq!(tool_calls[0].tool_call_id, "call_a");
    assert_eq!(tool_calls[1].tool_call_id, "call_b");
}

// ---- finish reason mapping ----

#[test]
fn finish_reason_stop_maps_to_done() {
    let finished = drive_to_done("stop");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn finish_reason_tool_calls_maps_to_done() {
    // DeepSeek's "tool_calls" finish reason maps to Done so the controller
    // continues the turn loop.
    let finished = drive_to_done("tool_calls");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn finish_reason_length_maps_to_max_token_limit() {
    let finished = drive_to_done("length");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(_))
    ));
}

#[test]
fn finish_reason_content_filter_maps_to_other() {
    let finished = drive_to_done("content_filter");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

#[test]
fn finish_reason_unknown_maps_to_other() {
    let finished = drive_to_done("insufficient_system_resource");
    assert!(matches!(
        finished.reason,
        Some(api::response_event::stream_finished::Reason::Other(_))
    ));
}

// ---- errors ----

#[test]
fn top_level_error_field_surfaces_as_internal_error() {
    let mut d = decoder();
    let chunk = r#"{"error":{"message":"Invalid API key.","type":"invalid_request_error","code":"invalid_api_key"}}"#;
    feed(&mut d, chunk);
    assert!(d.is_terminal());

    let closing = d.finish();
    // Rollback emitted (no finishReason), then Finished{InternalError}.
    assert!(closing.iter().any(|ev| matches!(
        &ev.r#type,
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
                ie.message.contains("Invalid API key."),
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
    assert!(closing.iter().any(|ev| matches!(
        &ev.r#type,
        Some(api::response_event::Type::ClientActions(ca))
            if matches!(
                ca.actions[0].action,
                Some(api::client_action::Action::RollbackTransaction(_))
            )
    )));
}

#[test]
fn premature_eof_without_done_emits_rollback() {
    let mut d = decoder();
    feed(&mut d, &make_text_chunk("partial"));
    assert!(!d.is_terminal());

    let closing = d.finish();
    // Should have a RollbackTransaction.
    assert!(closing.iter().any(|ev| matches!(
        &ev.r#type,
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
                ie.message.contains("stream ended"),
                "unexpected message: {}",
                ie.message
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn record_upstream_error_surfaces_in_finish() {
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

// ---- usage ----

#[test]
fn usage_with_reasoning_tokens_still_folds_into_completion_tokens() {
    // usage.completion_tokens is what shows up in TokenUsage.output_tokens;
    // completion_tokens_details.reasoning_tokens is deserialized but ignored.
    let chunk = r#"{"id":"chatcmpl-x","model":"deepseek-reasoner","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":200,"total_tokens":300,"completion_tokens_details":{"reasoning_tokens":150}}}"#;
    let mut d = decoder();
    feed(&mut d, chunk);
    feed(&mut d, "[DONE]");
    let closing = d.finish();
    let finished = extract_finished(closing.last().unwrap());
    assert_eq!(finished.token_usage.len(), 1);
    let usage = &finished.token_usage[0];
    assert_eq!(usage.total_input, 100);
    assert_eq!(usage.output, 200, "output_tokens should be completion_tokens=200, not reasoning split");
    assert_eq!(usage.model_id, "deepseek-reasoner");
}

// ---- edge cases ----

#[test]
fn terminal_state_safety_post_done_feeds_are_no_ops() {
    let mut d = decoder();
    feed(&mut d, &make_finish_chunk("stop"));
    feed(&mut d, "[DONE]");
    assert!(d.is_terminal());

    let stray = feed(&mut d, &make_text_chunk("should be ignored"));
    assert_eq!(stray.len(), 0, "expected no-op after terminal state");

    // State should still be terminal, no panics.
    assert!(d.is_terminal());
}
