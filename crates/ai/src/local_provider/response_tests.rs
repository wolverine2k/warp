//! Fixture-driven tests for the SSE → ResponseEvent adapter.
//!
//! Each test feeds one or more SSE message-data strings to a fresh
//! `OpenAiSseAdapter`, then asserts the shape of the events it emits.

use warp_multi_agent_api as api;

use super::response::OpenAiSseAdapter;

// ---------- helpers ----------

fn drive(adapter: &mut OpenAiSseAdapter, chunks: &[&str]) -> Vec<api::ResponseEvent> {
    let mut all = Vec::new();
    for c in chunks {
        all.extend(adapter.feed(c));
    }
    all.extend(adapter.finish());
    all
}

fn is_init(ev: &api::ResponseEvent) -> bool {
    matches!(ev.r#type, Some(api::response_event::Type::Init(_)))
}

fn is_finished(ev: &api::ResponseEvent) -> bool {
    matches!(ev.r#type, Some(api::response_event::Type::Finished(_)))
}

fn finish_reason(ev: &api::ResponseEvent) -> Option<&api::response_event::stream_finished::Reason> {
    if let Some(api::response_event::Type::Finished(f)) = ev.r#type.as_ref() {
        f.reason.as_ref()
    } else {
        None
    }
}

fn unwrap_actions(ev: &api::ResponseEvent) -> Option<&[api::ClientAction]> {
    if let Some(api::response_event::Type::ClientActions(a)) = ev.r#type.as_ref() {
        Some(&a.actions)
    } else {
        None
    }
}

fn count_text_appends(events: &[api::ResponseEvent]) -> usize {
    events
        .iter()
        .filter_map(unwrap_actions)
        .flat_map(|a| a.iter())
        .filter(|a| {
            matches!(
                a.action,
                Some(api::client_action::Action::AppendToMessageContent(_))
            )
        })
        .count()
}

fn count_add_messages(events: &[api::ResponseEvent]) -> usize {
    events
        .iter()
        .filter_map(unwrap_actions)
        .flat_map(|a| a.iter())
        .filter(|a| {
            matches!(
                a.action,
                Some(api::client_action::Action::AddMessagesToTask(_))
            )
        })
        .count()
}

fn count_begin_tx(events: &[api::ResponseEvent]) -> usize {
    events
        .iter()
        .filter_map(unwrap_actions)
        .flat_map(|a| a.iter())
        .filter(|a| {
            matches!(
                a.action,
                Some(api::client_action::Action::BeginTransaction(_))
            )
        })
        .count()
}

fn count_commit_tx(events: &[api::ResponseEvent]) -> usize {
    events
        .iter()
        .filter_map(unwrap_actions)
        .flat_map(|a| a.iter())
        .filter(|a| {
            matches!(
                a.action,
                Some(api::client_action::Action::CommitTransaction(_))
            )
        })
        .count()
}

fn count_rollback_tx(events: &[api::ResponseEvent]) -> usize {
    events
        .iter()
        .filter_map(unwrap_actions)
        .flat_map(|a| a.iter())
        .filter(|a| {
            matches!(
                a.action,
                Some(api::client_action::Action::RollbackTransaction(_))
            )
        })
        .count()
}

// ---------- tests ----------

#[test]
fn empty_stream_emits_init_begin_rollback_finished() {
    let mut adapter = OpenAiSseAdapter::new();
    let events = adapter.finish();
    assert!(is_init(&events[0]), "first event must be Init");
    assert_eq!(count_begin_tx(&events), 1);
    // No content + no finish_reason ⇒ rollback path
    assert_eq!(count_rollback_tx(&events), 1);
    assert_eq!(count_commit_tx(&events), 0);
    assert!(is_finished(events.last().unwrap()));
    // The finish reason should be InternalError (no finish_reason ever arrived)
    assert!(matches!(
        finish_reason(events.last().unwrap()),
        Some(api::response_event::stream_finished::Reason::InternalError(
            _
        ))
    ));
}

#[test]
fn text_only_short_emits_add_then_finished_done() {
    let mut adapter = OpenAiSseAdapter::new();
    let chunk = r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#;
    let stop = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;

    let events = drive(&mut adapter, &[chunk, stop]);

    assert!(is_init(&events[0]));
    assert_eq!(count_begin_tx(&events), 1);
    assert_eq!(
        count_add_messages(&events),
        1,
        "first text delta opens the message"
    );
    assert_eq!(
        count_text_appends(&events),
        0,
        "single chunk doesn't append"
    );
    assert_eq!(count_commit_tx(&events), 1);
    assert_eq!(count_rollback_tx(&events), 0);
    assert!(matches!(
        finish_reason(events.last().unwrap()),
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[test]
fn text_multi_chunk_uses_add_then_appends() {
    let mut adapter = OpenAiSseAdapter::new();
    let c1 = r#"{"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#;
    let c2 = r#"{"choices":[{"index":0,"delta":{"content":" "},"finish_reason":null}]}"#;
    let c3 = r#"{"choices":[{"index":0,"delta":{"content":"world"},"finish_reason":"stop"}]}"#;
    let events = drive(&mut adapter, &[c1, c2, c3]);

    assert_eq!(count_add_messages(&events), 1, "exactly one open");
    assert_eq!(count_text_appends(&events), 2, "two appends after the open");
    assert_eq!(count_commit_tx(&events), 1);
}

#[test]
fn done_marker_terminates_cleanly() {
    let mut adapter = OpenAiSseAdapter::new();
    let chunk = r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
    let events = drive(&mut adapter, &[chunk, "[DONE]"]);
    assert_eq!(count_commit_tx(&events), 1);
}

#[test]
fn finish_length_maps_to_max_token_limit() {
    let mut adapter = OpenAiSseAdapter::new();
    let chunk = r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"length"}]}"#;
    let events = drive(&mut adapter, &[chunk]);
    assert!(matches!(
        finish_reason(events.last().unwrap()),
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(
            _
        ))
    ));
}

#[test]
fn malformed_chunk_rolls_back_with_internal_error() {
    let mut adapter = OpenAiSseAdapter::new();
    let events = drive(&mut adapter, &["not valid json{"]);
    assert_eq!(count_rollback_tx(&events), 1);
    assert!(matches!(
        finish_reason(events.last().unwrap()),
        Some(api::response_event::stream_finished::Reason::InternalError(
            _
        ))
    ));
}

#[test]
fn empty_choices_chunk_is_silent() {
    let mut adapter = OpenAiSseAdapter::new();
    let empty = r#"{"choices":[]}"#;
    let real = r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
    let events = drive(&mut adapter, &[empty, real]);
    assert_eq!(count_add_messages(&events), 1);
    assert_eq!(count_commit_tx(&events), 1);
}

#[test]
fn unicode_streams_correctly_across_chunks() {
    let mut adapter = OpenAiSseAdapter::new();
    let c1 = r#"{"choices":[{"index":0,"delta":{"content":"héll"}}]}"#;
    let c2 = r#"{"choices":[{"index":0,"delta":{"content":"o 🌍"},"finish_reason":"stop"}]}"#;
    let events = drive(&mut adapter, &[c1, c2]);
    assert_eq!(count_text_appends(&events), 1);
    assert_eq!(count_commit_tx(&events), 1);
}

#[test]
fn reasoning_content_routes_to_reasoning_message() {
    let mut adapter = OpenAiSseAdapter::new();
    let c1 = r#"{"choices":[{"index":0,"delta":{"reasoning_content":"thinking..."}}]}"#;
    let c2 = r#"{"choices":[{"index":0,"delta":{"content":"answer"},"finish_reason":"stop"}]}"#;
    let events = drive(&mut adapter, &[c1, c2]);
    // First: reasoning open (AddMessagesToTask). Second: answer open (AddMessagesToTask).
    assert_eq!(count_add_messages(&events), 2);
    assert_eq!(count_commit_tx(&events), 1);
}

#[test]
fn init_event_uniqueness_across_adapters() {
    let mut a1 = OpenAiSseAdapter::new();
    let mut a2 = OpenAiSseAdapter::new();
    a1.finish();
    a2.finish();
    assert_ne!(a1.conversation_id(), a2.conversation_id());
}

#[test]
fn explicit_ids_round_trip_in_init() {
    let mut adapter = OpenAiSseAdapter::with_ids(
        "local:fixed-conv".into(),
        "fixed-req".into(),
        "fixed-run".into(),
        "fixed-task".into(),
    );
    let events = adapter.finish();
    let init = match &events[0].r#type {
        Some(api::response_event::Type::Init(i)) => i,
        _ => panic!("expected Init"),
    };
    assert_eq!(init.conversation_id, "local:fixed-conv");
    assert_eq!(init.request_id, "fixed-req");
    assert_eq!(init.run_id, "fixed-run");
}

#[test]
fn init_emitted_exactly_once_even_with_many_chunks() {
    let mut adapter = OpenAiSseAdapter::new();
    let chunks = vec![
        r#"{"choices":[{"index":0,"delta":{"content":"a"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"content":"b"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"content":"c"},"finish_reason":"stop"}]}"#,
    ];
    let events = drive(&mut adapter, &chunks);
    let init_count = events.iter().filter(|e| is_init(e)).count();
    assert_eq!(init_count, 1);
}

#[test]
fn finish_event_emitted_exactly_once() {
    let mut adapter = OpenAiSseAdapter::new();
    let chunks = vec![
        r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
        "[DONE]",
    ];
    let events = drive(&mut adapter, &chunks);
    let fin_count = events.iter().filter(|e| is_finished(e)).count();
    assert_eq!(fin_count, 1);
}

#[test]
fn ordering_invariant_no_actions_after_commit() {
    let mut adapter = OpenAiSseAdapter::new();
    let chunk = r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
    let events = drive(&mut adapter, &[chunk]);
    let mut saw_commit = false;
    for ev in &events {
        if let Some(actions) = unwrap_actions(ev) {
            for a in actions {
                if matches!(
                    a.action,
                    Some(api::client_action::Action::CommitTransaction(_))
                ) {
                    saw_commit = true;
                } else if saw_commit
                    && !matches!(
                        a.action,
                        Some(api::client_action::Action::CommitTransaction(_))
                    )
                {
                    panic!("emitted a ClientAction after Commit");
                }
            }
        } else if saw_commit && !is_finished(ev) {
            panic!("emitted non-Finished event after Commit");
        }
    }
    assert!(saw_commit, "expected a Commit in this stream");
}

// ---------- Phase B-3a usage capture ----------

#[test]
fn final_chunk_with_usage_emits_token_usage_on_stream_finished() {
    let mut adapter = OpenAiSseAdapter::new();
    let events = drive(
        &mut adapter,
        &[
            r#"{"id":"c1","model":"my-model","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
            // OpenAI's final usage chunk: empty choices, populated usage.
            r#"{"id":"c1","model":"my-model","choices":[],"usage":{"prompt_tokens":42,"completion_tokens":17,"total_tokens":59,"prompt_tokens_details":{"cached_tokens":5}}}"#,
        ],
    );
    let finished = events.iter().find(|e| is_finished(e)).expect("Finished");
    let token_usage = match &finished.r#type {
        Some(api::response_event::Type::Finished(f)) => &f.token_usage,
        _ => unreachable!(),
    };
    assert_eq!(token_usage.len(), 1, "expected one TokenUsage entry");
    let u = &token_usage[0];
    assert_eq!(u.model_id, "my-model");
    assert_eq!(u.total_input, 42);
    assert_eq!(u.output, 17);
    assert_eq!(u.input_cache_read, 5);
    assert_eq!(u.input_cache_write, 0);
}

#[test]
fn no_usage_chunk_means_empty_token_usage_on_stream_finished() {
    let mut adapter = OpenAiSseAdapter::new();
    let events = drive(
        &mut adapter,
        &[
            r#"{"id":"c1","model":"my-model","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
        ],
    );
    let finished = events.iter().find(|e| is_finished(e)).expect("Finished");
    let token_usage = match &finished.r#type {
        Some(api::response_event::Type::Finished(f)) => &f.token_usage,
        _ => unreachable!(),
    };
    assert!(
        token_usage.is_empty(),
        "no upstream usage chunk should yield empty token_usage, got {token_usage:?}"
    );
}

#[test]
fn usage_chunk_without_model_field_falls_back_to_local() {
    // Server omits `model` entirely.
    let mut adapter = OpenAiSseAdapter::new();
    let events = drive(
        &mut adapter,
        &[
            r#"{"choices":[{"index":0,"delta":{"content":"x"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
        ],
    );
    let finished = events.iter().find(|e| is_finished(e)).expect("Finished");
    let token_usage = match &finished.r#type {
        Some(api::response_event::Type::Finished(f)) => &f.token_usage,
        _ => unreachable!(),
    };
    assert_eq!(token_usage.len(), 1);
    assert_eq!(token_usage[0].model_id, "local");
    assert_eq!(token_usage[0].total_input, 10);
    assert_eq!(token_usage[0].output, 2);
}
