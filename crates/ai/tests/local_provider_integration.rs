//! Phase 7 integration tests for the Custom Local LLM Provider.
//!
//! Each test boots a tiny in-process HTTP/SSE mock server, runs `run_chat_turn`
//! against it, and asserts on the produced `ResponseEvent` stream.
//!
//! The mock server is a hand-rolled `tokio::net::TcpListener` that handles
//! exactly one connection per test, reads the HTTP request just far enough to
//! confirm the body, and writes back a scripted `text/event-stream` response.
//!
//! Run with:
//! ```bash
//! cargo test -p ai --test local_provider_integration -- --nocapture
//! ```

use std::sync::Once;
use std::time::Duration;

use ai::local_provider::{
    compaction::{
        commit_summarization, try_compact, AutoCompactionOutcome, CompactionConfig,
        CompactionState, TokenCounts,
    },
    config::LocalProviderConfig,
    request::LocalProviderInput,
    run::{
        build_summarizer_messages, run_chat_turn, run_summarizer_turn, LocalRunError,
        SummarizerInput,
    },
};
use futures::stream::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use warp_multi_agent_api as api;

// ---------- crypto provider init ----------
//
// reqwest's default rustls feature requires a crypto provider to be installed
// before any TLS use. The Warp app does this at lib.rs startup; tests need to
// do it themselves. Idempotent via `Once`.
fn init_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

// ---------- mock SSE server ----------

/// Spin up a one-shot mock server on a random port, scripted to write the
/// supplied SSE chunks (separated by blank lines as standard) and close.
/// Returns the bound URL like `http://127.0.0.1:NNNN/v1`.
async fn spawn_mock_server(scripted_sse_lines: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");

    tokio::spawn(async move {
        let (mut socket, _) = match listener.accept().await {
            Ok(p) => p,
            Err(_) => return,
        };
        // Drain the incoming HTTP request — read until "\r\n\r\n" plus
        // any body content. Don't bother parsing.
        let _ = drain_http_request(&mut socket).await;

        // Write SSE response.
        let header = "HTTP/1.1 200 OK\r\n\
                      Content-Type: text/event-stream\r\n\
                      Cache-Control: no-cache\r\n\
                      Connection: close\r\n\
                      \r\n";
        if socket.write_all(header.as_bytes()).await.is_err() {
            return;
        }
        for line in scripted_sse_lines {
            // SSE message format: `data: <payload>\n\n`
            if line == "<<DELAY>>" {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            let frame = format!("data: {line}\n\n");
            if socket.write_all(frame.as_bytes()).await.is_err() {
                break;
            }
        }
        // Close cleanly.
        let _ = socket.shutdown().await;
    });

    url
}

async fn drain_http_request(socket: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = vec![0u8; 8192];
    let mut total = Vec::new();
    loop {
        let n = socket.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        total.extend_from_slice(&buf[..n]);
        if total.windows(4).any(|w| w == b"\r\n\r\n") {
            // Headers done. Read a bit more to drain the body.
            let _ = tokio::time::timeout(Duration::from_millis(20), socket.read(&mut buf)).await;
            return Ok(());
        }
    }
}

// ---------- helpers ----------

fn cfg_for(url: &str) -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Test".into(),
        base_url: url.into(),
        model_id: "test-model".into(),
        api_key: None,
        supports_tools: true,
        context_window: None,
    }
}

fn empty_input() -> LocalProviderInput {
    LocalProviderInput {
        user_query: Some("hi".into()),
        tasks: vec![],
        supported_tools: vec![],
        ..Default::default()
    }
}

async fn collect_events(
    cfg: LocalProviderConfig,
    input: LocalProviderInput,
) -> Result<Vec<api::ResponseEvent>, LocalRunError> {
    init_crypto_provider();
    let (_cancel_tx, cancel_rx) = futures::channel::oneshot::channel();
    let http = reqwest::Client::new();
    let mut stream = run_chat_turn(input, cfg, cancel_rx, http).await?;
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }
    Ok(events)
}

fn count_actions<F: Fn(&api::client_action::Action) -> bool>(
    events: &[api::ResponseEvent],
    pred: F,
) -> usize {
    events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(a)) => Some(&a.actions),
            _ => None,
        })
        .flat_map(|a| a.iter())
        .filter_map(|a| a.action.as_ref())
        .filter(|a| pred(a))
        .count()
}

fn last_finish_reason(
    events: &[api::ResponseEvent],
) -> Option<&api::response_event::stream_finished::Reason> {
    events.iter().rev().find_map(|e| {
        if let Some(api::response_event::Type::Finished(f)) = &e.r#type {
            f.reason.as_ref()
        } else {
            None
        }
    })
}

// ---------- tests ----------

#[tokio::test]
async fn basic_text_only_turn_streams_and_commits() {
    let scripted = vec![
        r#"{"choices":[{"index":0,"delta":{"content":"hello "},"finish_reason":null}]}"#
            .to_string(),
        r#"{"choices":[{"index":0,"delta":{"content":"world"},"finish_reason":null}]}"#.to_string(),
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input())
        .await
        .expect("turn succeeds");

    // Init first
    assert!(matches!(
        events.first().and_then(|e| e.r#type.as_ref()),
        Some(api::response_event::Type::Init(_))
    ));
    // Begin transaction, at least one AddMessages, at least one Append, Commit, Finished{Done}
    assert_eq!(
        count_actions(&events, |a| matches!(
            a,
            api::client_action::Action::BeginTransaction(_)
        )),
        1
    );
    let appends = count_actions(&events, |a| {
        matches!(a, api::client_action::Action::AppendToMessageContent(_))
    });
    let opens = count_actions(&events, |a| {
        matches!(a, api::client_action::Action::AddMessagesToTask(_))
    });
    assert!(opens >= 1, "expected an opening AddMessagesToTask, got 0");
    assert!(appends >= 1, "expected appends for the second chunk");
    assert_eq!(
        count_actions(&events, |a| matches!(
            a,
            api::client_action::Action::CommitTransaction(_)
        )),
        1
    );
    assert!(matches!(
        last_finish_reason(&events),
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[tokio::test]
async fn finish_reason_length_maps_to_max_token_limit() {
    let scripted = vec![
        r#"{"choices":[{"index":0,"delta":{"content":"x"},"finish_reason":"length"}]}"#.to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();
    assert!(matches!(
        last_finish_reason(&events),
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(
            _
        ))
    ));
}

#[tokio::test]
async fn unreachable_endpoint_is_handled_gracefully() {
    // Point at a port we never bind to.
    let cfg = cfg_for("http://127.0.0.1:1");
    let events = collect_events(cfg, empty_input()).await.unwrap();
    // Stream still produces Init + BeginTx + Rollback + Finished{InternalError}
    assert!(matches!(
        events.first().and_then(|e| e.r#type.as_ref()),
        Some(api::response_event::Type::Init(_))
    ));
    assert_eq!(
        count_actions(&events, |a| matches!(
            a,
            api::client_action::Action::RollbackTransaction(_)
        )),
        1,
        "expected rollback when the endpoint is unreachable"
    );
    assert!(matches!(
        last_finish_reason(&events),
        Some(api::response_event::stream_finished::Reason::InternalError(
            _
        ))
    ));
}

#[tokio::test]
async fn tool_call_round_trips_into_typed_proto() {
    // Mock emits a streamed read_files tool call followed by finish_reason="tool_calls".
    let scripted = vec![
        // First fragment: id + name
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"read_files","arguments":""}}]}}]}"#.to_string(),
        // Argument fragments
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"paths\":[\"src/main.rs\"]}"}}]}}]}"#.to_string(),
        // Finish
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();

    // Find the AddMessagesToTask carrying the ToolCall.
    let tool_message = events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(a)) => Some(&a.actions),
            _ => None,
        })
        .flat_map(|a| a.iter())
        .find_map(|a| match a.action.as_ref()? {
            api::client_action::Action::AddMessagesToTask(amt) => {
                amt.messages.iter().find_map(|m| match m.message.as_ref()? {
                    api::message::Message::ToolCall(tc) => Some(tc.clone()),
                    _ => None,
                })
            }
            _ => None,
        });
    let tc = tool_message.expect("expected a ToolCall message");
    assert_eq!(tc.tool_call_id, "call_abc");
    match tc.tool.as_ref().expect("typed tool variant") {
        api::message::tool_call::Tool::ReadFiles(rf) => {
            assert_eq!(rf.files.len(), 1);
            assert_eq!(rf.files[0].name, "src/main.rs");
        }
        _ => panic!("expected ReadFiles variant"),
    }
    // finish_reason="tool_calls" maps to Done
    assert!(matches!(
        last_finish_reason(&events),
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[tokio::test]
async fn cancellation_mid_stream_rolls_back() {
    // Mock writes one chunk then sleeps. Our cancel fires during the sleep.
    let scripted = vec![
        r#"{"choices":[{"index":0,"delta":{"content":"first chunk"}}]}"#.to_string(),
        "<<DELAY>>".to_string(),
        r#"{"choices":[{"index":0,"delta":{"content":"never seen"},"finish_reason":"stop"}]}"#
            .to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    init_crypto_provider();
    let (cancel_tx, cancel_rx) = futures::channel::oneshot::channel();
    let http = reqwest::Client::new();
    let mut stream = run_chat_turn(empty_input(), cfg_for(&url), cancel_rx, http)
        .await
        .unwrap();

    // Read the Init + BeginTransaction + first text chunk (3 events).
    let mut early = Vec::new();
    for _ in 0..3 {
        if let Some(ev) = stream.next().await {
            early.push(ev);
        }
    }
    // Trigger cancellation now.
    let _ = cancel_tx.send(());

    // Drain remaining events.
    let mut events = early;
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }
    // Cancellation path emits a Rollback (or Commit if done already; here we
    // expect Rollback because we cancelled before finish_reason).
    let rollbacks = count_actions(&events, |a| {
        matches!(a, api::client_action::Action::RollbackTransaction(_))
    });
    let commits = count_actions(&events, |a| {
        matches!(a, api::client_action::Action::CommitTransaction(_))
    });
    assert!(
        rollbacks + commits >= 1,
        "expected at least one Commit or Rollback after cancel, got {:?}",
        events.len()
    );
    // We should have gotten *some* finish event regardless of variant.
    assert!(last_finish_reason(&events).is_some(), "expected Finished");
}

#[tokio::test]
async fn http_500_response_produces_internal_error_finish() {
    // Write a 500 response (no SSE) and close.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = drain_http_request(&mut socket).await;
        let header = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\n\r\nboom!";
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();
    // Adapter rolls back and emits InternalError or Other.
    let reason = last_finish_reason(&events).expect("Finished");
    assert!(
        matches!(
            reason,
            api::response_event::stream_finished::Reason::InternalError(_)
                | api::response_event::stream_finished::Reason::Other(_)
        ),
        "expected InternalError or Other, got {reason:?}"
    );
}

#[tokio::test]
async fn multiple_http_error_statuses_finish_cleanly() {
    for status in [401u16, 403, 404, 422, 429, 503] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/v1");
        let status_body =
            format!("HTTP/1.1 {status} Error\r\nContent-Length: 11\r\n\r\nbody-text!!");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = drain_http_request(&mut socket).await;
            let _ = socket.write_all(status_body.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();
        let reason = last_finish_reason(&events)
            .unwrap_or_else(|| panic!("status {status}: no Finished event"));
        assert!(
            matches!(
                reason,
                api::response_event::stream_finished::Reason::InternalError(_)
                    | api::response_event::stream_finished::Reason::Other(_)
            ),
            "status {status}: expected InternalError|Other, got {reason:?}"
        );
        // No CommitTransaction should fire on the error path.
        assert_eq!(
            count_actions(&events, |a| matches!(
                a,
                api::client_action::Action::CommitTransaction(_)
            )),
            0,
            "status {status}: should not Commit on HTTP error"
        );
    }
}

#[tokio::test]
async fn malformed_tool_args_emit_synthetic_assistant_message() {
    // The model emits a tool_call whose arguments are NOT valid JSON.
    // The adapter shouldn't drop the turn — it surfaces the failure as
    // visible assistant text so the user sees the model's intent.
    let scripted = vec![
        // First: id + name
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_bad","type":"function","function":{"name":"read_files","arguments":""}}]}}]}"#.to_string(),
        // Then garbage args
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"this is { not valid json"}}]}}]}"#.to_string(),
        // Finish
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();

    // Should have produced an AddMessagesToTask whose inner Message is
    // AgentOutput (not ToolCall) describing the parse failure.
    let synthetic_text = events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(a)) => Some(&a.actions),
            _ => None,
        })
        .flat_map(|a| a.iter())
        .filter_map(|a| match a.action.as_ref()? {
            api::client_action::Action::AddMessagesToTask(amt) => {
                amt.messages.iter().find_map(|m| match m.message.as_ref()? {
                    api::message::Message::AgentOutput(ao) => Some(ao.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .next();
    let synthetic_text = synthetic_text.expect("expected a synthetic AgentOutput message");
    assert!(
        synthetic_text.contains("read_files") && synthetic_text.contains("unusable"),
        "synthetic message should explain the failure, got: {synthetic_text}"
    );
    // The turn should still finish cleanly (Done) — we don't error the whole stream.
    assert!(matches!(
        last_finish_reason(&events),
        Some(api::response_event::stream_finished::Reason::Done(_))
    ));
}

#[tokio::test]
async fn reasoning_content_emits_separately_from_visible_text() {
    let scripted = vec![
        r#"{"choices":[{"index":0,"delta":{"reasoning_content":"Let me think..."}}]}"#.to_string(),
        r#"{"choices":[{"index":0,"delta":{"reasoning_content":" 2+2=4"}}]}"#.to_string(),
        r#"{"choices":[{"index":0,"delta":{"content":"The answer is 4."}, "finish_reason":"stop"}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();

    // Walk all messages and bucket by inner type.
    let mut reasoning = String::new();
    let mut visible = String::new();
    for ev in &events {
        let Some(api::response_event::Type::ClientActions(ca)) = &ev.r#type else {
            continue;
        };
        for action in &ca.actions {
            let inner_msg = match action.action.as_ref() {
                Some(api::client_action::Action::AddMessagesToTask(amt)) => {
                    amt.messages.first().and_then(|m| m.message.as_ref())
                }
                Some(api::client_action::Action::AppendToMessageContent(append)) => {
                    append.message.as_ref().and_then(|m| m.message.as_ref())
                }
                _ => None,
            };
            match inner_msg {
                Some(api::message::Message::AgentOutput(a)) => visible.push_str(&a.text),
                Some(api::message::Message::AgentReasoning(r)) => reasoning.push_str(&r.reasoning),
                _ => {}
            }
        }
    }
    assert_eq!(reasoning, "Let me think... 2+2=4");
    assert_eq!(visible, "The answer is 4.");
}

#[tokio::test]
async fn two_interleaved_tool_calls_in_one_response() {
    // Model emits two tool calls (different indices). Both must be translated
    // into typed Message::ToolCall variants; both must appear in the stream.
    let scripted = vec![
        // Call 0: read_files first fragment
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"read_files","arguments":""}}]}}]}"#.to_string(),
        // Call 0: rest of args
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"paths\":[\"a.rs\"]}"}}]}}]}"#.to_string(),
        // Call 1: grep first fragment — higher index signals call 0 is complete
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"grep","arguments":""}}]}}]}"#.to_string(),
        // Call 1: rest of args
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"queries\":[\"TODO\"]}"}}]}}]}"#.to_string(),
        // Finish
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();

    // Collect emitted ToolCalls in order.
    let tool_calls: Vec<api::message::ToolCall> = events
        .iter()
        .filter_map(|e| match &e.r#type {
            Some(api::response_event::Type::ClientActions(a)) => Some(&a.actions),
            _ => None,
        })
        .flat_map(|a| a.iter())
        .flat_map(|a| match a.action.as_ref()? {
            api::client_action::Action::AddMessagesToTask(amt) => Some(amt.messages.iter()),
            _ => None,
        })
        .flatten()
        .filter_map(|m| match m.message.as_ref()? {
            api::message::Message::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(tool_calls.len(), 2, "expected two tool calls in stream");
    assert_eq!(tool_calls[0].tool_call_id, "call_a");
    assert_eq!(tool_calls[1].tool_call_id, "call_b");
    assert!(matches!(
        tool_calls[0].tool.as_ref().unwrap(),
        api::message::tool_call::Tool::ReadFiles(_)
    ));
    assert!(matches!(
        tool_calls[1].tool.as_ref().unwrap(),
        api::message::tool_call::Tool::Grep(_)
    ));
}

#[tokio::test]
async fn conversation_id_round_trips_in_init_event() {
    // Smoke: every Init event carries a synthetic local:* conversation_id.
    let scripted = vec![
        r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let url = spawn_mock_server(scripted).await;
    let events = collect_events(cfg_for(&url), empty_input()).await.unwrap();

    let init = events
        .first()
        .and_then(|e| match e.r#type.as_ref()? {
            api::response_event::Type::Init(init) => Some(init),
            _ => None,
        })
        .expect("Init first");
    assert!(
        init.conversation_id.starts_with("local:"),
        "conversation_id should carry the local: prefix, got {}",
        init.conversation_id
    );
    assert!(
        !init.request_id.is_empty(),
        "request_id should be populated"
    );
    assert!(!init.run_id.is_empty(), "run_id should be populated");
    assert_ne!(init.conversation_id, init.request_id);
    assert_ne!(init.request_id, init.run_id);
}

#[tokio::test]
async fn api_key_attaches_authorization_header() {
    // Capture the request and assert the Authorization header is present.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");
    let captured = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let captured_for_task = captured.clone();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut acc = Vec::new();
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            acc.extend_from_slice(&buf[..n]);
            if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        *captured_for_task.lock().await = acc;
        let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket
            .write_all(
                b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            )
            .await;
        let _ = socket.shutdown().await;
    });

    let mut cfg = cfg_for(&url);
    cfg.api_key = Some("sk-test-token-xyz".into());
    let _events = collect_events(cfg, empty_input()).await.unwrap();
    let captured = captured.lock().await;
    let request_str = String::from_utf8_lossy(&captured);
    assert!(
        request_str
            .to_lowercase()
            .contains("authorization: bearer sk-test-token-xyz"),
        "expected Authorization header in request, got:\n{request_str}"
    );
}

// ---------- Phase B-3 head-summary compaction tests ----------

/// Spin up a one-shot mock server that responds to a single non-streaming
/// JSON Chat Completions request with a canned `application/json` body.
/// Returns `(url, captured_body_handle)`.
async fn spawn_json_mock_server(
    response_body: String,
) -> (String, std::sync::Arc<tokio::sync::Mutex<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");
    let captured = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let captured_for_task = captured.clone();
    tokio::spawn(async move {
        let (mut socket, _) = match listener.accept().await {
            Ok(p) => p,
            Err(_) => return,
        };
        // Read until \r\n\r\n then drain a bit of the body so we capture it.
        let mut buf = vec![0u8; 16 * 1024];
        let mut acc = Vec::new();
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            acc.extend_from_slice(&buf[..n]);
            if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                let _ = tokio::time::timeout(Duration::from_millis(50), socket.read(&mut buf))
                    .await
                    .ok()
                    .and_then(|res| res.ok())
                    .map(|n| acc.extend_from_slice(&buf[..n]));
                break;
            }
        }
        *captured_for_task.lock().await = acc;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            response_body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(response_body.as_bytes()).await;
        let _ = socket.shutdown().await;
    });
    (url, captured)
}

#[tokio::test]
async fn summarizer_parses_non_streaming_json_and_returns_assistant_text() {
    init_crypto_provider();
    let body = "{\
        \"id\":\"cmpl-1\",\
        \"model\":\"test-model\",\
        \"choices\":[{\
            \"index\":0,\
            \"message\":{\"role\":\"assistant\",\"content\":\"## Goal\\n- single-sentence task summary\"},\
            \"finish_reason\":\"stop\"\
        }]\
    }";
    let (url, captured) = spawn_json_mock_server(body.to_string()).await;
    let cfg = cfg_for(&url);
    let messages = build_summarizer_messages(
        Some("You are a summarizer."),
        vec![],
        "Summarize the conversation".to_string(),
    );
    let input = SummarizerInput { messages };
    let http = reqwest::Client::new();
    let summary = run_summarizer_turn(input, &cfg, &http)
        .await
        .expect("summarizer call succeeds");
    assert!(summary.contains("## Goal"));
    assert!(summary.contains("single-sentence task summary"));
    // Confirm the request was non-streaming.
    let captured = captured.lock().await;
    let request_str = String::from_utf8_lossy(&captured);
    assert!(
        request_str.contains("\"stream\":false"),
        "summarizer request body should include stream:false, got:\n{request_str}"
    );
}

#[tokio::test]
async fn summarizer_surfaces_http_error_with_body_excerpt() {
    init_crypto_provider();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = drain_http_request(&mut socket).await;
        let body = r#"{"error":{"message":"model not found"}}"#;
        let header = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.shutdown().await;
    });
    let cfg = cfg_for(&url);
    let messages = build_summarizer_messages(None, vec![], "summarize".to_string());
    let input = SummarizerInput { messages };
    let http = reqwest::Client::new();
    let err = run_summarizer_turn(input, &cfg, &http)
        .await
        .expect_err("404 should surface as UpstreamHttp");
    let msg = format!("{err}");
    assert!(
        msg.contains("404"),
        "error message should include status: {msg}"
    );
    assert!(
        msg.contains("model not found"),
        "error message should include body excerpt: {msg}"
    );
}

#[tokio::test]
async fn auto_compact_round_trip_overflow_summarizes_and_commits() {
    use warp_multi_agent_api as api;
    init_crypto_provider();

    // Mock summarizer endpoint returning a canned summary.
    let body = "{\
        \"id\":\"cmpl-2\",\
        \"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"## Goal\\n- auto-compact summary\"},\"finish_reason\":\"stop\"}]\
    }";
    let (url, _captured) = spawn_json_mock_server(body.to_string()).await;
    let mut cfg = cfg_for(&url);
    // Tiny window so even a few small messages overflow.
    cfg.context_window = Some(64);

    // History: enough messages that select() finds a head/tail split.
    let messages: Vec<api::Message> = (0..6)
        .map(|i| api::Message {
            id: format!("m{i}"),
            message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                query: format!("turn-{i} {}", "x".repeat(200)),
                ..Default::default()
            })),
            ..Default::default()
        })
        .collect();

    let mut state = CompactionState::default();
    let compaction_cfg = CompactionConfig::default();
    // Force overflow regardless of estimated size by handing in a giant
    // total — try_compact still has to call select on the actual messages.
    let tokens = TokenCounts {
        total: 1_000_000,
        ..Default::default()
    };
    let http = reqwest::Client::new();

    let outcome = try_compact(&messages, &mut state, &cfg, &compaction_cfg, tokens, &http)
        .await
        .expect("auto-compact succeeds");

    match outcome {
        AutoCompactionOutcome::Compacted(o) => {
            assert!(o.auto, "auto-trigger should mark Compacted as auto=true");
            assert!(
                o.summary_text.contains("auto-compact summary"),
                "outcome should carry the summarizer text, got: {}",
                o.summary_text
            );
        }
        AutoCompactionOutcome::Skipped => {
            panic!("expected Compacted, got Skipped — select() may not be picking a tail")
        }
    }
    assert_eq!(state.completed().len(), 1);
    let last = &state.completed()[0];
    assert!(last.auto);
    assert!(last.overflow);
    assert!(
        last.summary_text
            .as_deref()
            .map(|s| s.contains("auto-compact summary"))
            .unwrap_or(false),
        "completed entry should cache the summary text"
    );
}

#[tokio::test]
async fn next_turn_after_compaction_drops_pre_compaction_history() {
    use warp_multi_agent_api as api;
    init_crypto_provider();

    // Step 1: simulate an overflow-summarize-commit cycle via the helper.
    // The compaction state is the only artifact — the synthetic
    // (user, assistant) pair lives entirely in `CompactionState` and is
    // synthesized by the request projection on the next turn. No task
    // mutation required.
    let mut state = CompactionState::default();
    let _outcome = commit_summarization(
        &mut state,
        "## Goal\n- distilled summary".to_string(),
        Some("u_new".into()),
        true,  // overflow
        false, // not manual
    );

    // Step 2: build a task list that includes an old "head" plus a new
    // tail user turn — the model never sees the head because the
    // projection drops it.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            api::Message {
                id: "u_old".into(),
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "old turn that was summarized".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            api::Message {
                id: "a_old".into(),
                message: Some(api::message::Message::AgentOutput(
                    api::message::AgentOutput {
                        text: "old assistant reply".into(),
                    },
                )),
                ..Default::default()
            },
            api::Message {
                id: "u_new".into(),
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "post-compaction question".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // Step 3: spin up a chat-mock that captures the request body, fire a
    // turn through `run_chat_turn`, and assert the body contains the summary
    // and excludes the dropped head.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");
    let captured = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let captured_for_task = captured.clone();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16 * 1024];
        let mut acc = Vec::new();
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            acc.extend_from_slice(&buf[..n]);
            if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                let _ = tokio::time::timeout(Duration::from_millis(50), socket.read(&mut buf))
                    .await
                    .ok()
                    .and_then(|res| res.ok())
                    .map(|n| acc.extend_from_slice(&buf[..n]));
                break;
            }
        }
        *captured_for_task.lock().await = acc;
        let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket
            .write_all(
                b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ack\"},\"finish_reason\":\"stop\"}]}\n\n",
            )
            .await;
        let _ = socket.write_all(b"data: [DONE]\n\n").await;
        let _ = socket.shutdown().await;
    });

    let input = LocalProviderInput {
        user_query: Some("anything new?".into()),
        tasks: vec![task],
        supported_tools: vec![],
        compaction_state: state,
        ..Default::default()
    };
    let _events = collect_events(cfg_for(&url), input).await.unwrap();

    let captured = captured.lock().await;
    let request_str = String::from_utf8_lossy(&captured);
    assert!(
        request_str.contains("distilled summary"),
        "post-compaction request should embed the summary, got:\n{request_str}"
    );
    assert!(
        !request_str.contains("old turn that was summarized"),
        "pre-compaction head should be dropped, got:\n{request_str}"
    );
    assert!(
        !request_str.contains("old assistant reply"),
        "pre-compaction reply should be dropped, got:\n{request_str}"
    );
    assert!(
        request_str.contains("post-compaction question"),
        "post-compaction tail should be preserved, got:\n{request_str}"
    );
}
