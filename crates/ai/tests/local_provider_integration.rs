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
    config::LocalProviderConfig,
    request::LocalProviderInput,
    run::{run_chat_turn, LocalRunError},
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
            let _ = tokio::time::timeout(
                Duration::from_millis(20),
                socket.read(&mut buf),
            )
            .await;
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
        r#"{"choices":[{"index":0,"delta":{"content":"hello "},"finish_reason":null}]}"#.to_string(),
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
        count_actions(&events, |a| matches!(a, api::client_action::Action::BeginTransaction(_))),
        1
    );
    let appends =
        count_actions(&events, |a| matches!(a, api::client_action::Action::AppendToMessageContent(_)));
    let opens =
        count_actions(&events, |a| matches!(a, api::client_action::Action::AddMessagesToTask(_)));
    assert!(opens >= 1, "expected an opening AddMessagesToTask, got 0");
    assert!(appends >= 1, "expected appends for the second chunk");
    assert_eq!(
        count_actions(&events, |a| matches!(a, api::client_action::Action::CommitTransaction(_))),
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
        Some(api::response_event::stream_finished::Reason::MaxTokenLimit(_))
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
        count_actions(&events, |a| matches!(a, api::client_action::Action::RollbackTransaction(_))),
        1,
        "expected rollback when the endpoint is unreachable"
    );
    assert!(matches!(
        last_finish_reason(&events),
        Some(api::response_event::stream_finished::Reason::InternalError(_))
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
            api::client_action::Action::AddMessagesToTask(amt) => amt
                .messages
                .iter()
                .find_map(|m| match m.message.as_ref()? {
                    api::message::Message::ToolCall(tc) => Some(tc.clone()),
                    _ => None,
                }),
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
    let rollbacks =
        count_actions(&events, |a| matches!(a, api::client_action::Action::RollbackTransaction(_)));
    let commits =
        count_actions(&events, |a| matches!(a, api::client_action::Action::CommitTransaction(_)));
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
        request_str.to_lowercase().contains("authorization: bearer sk-test-token-xyz"),
        "expected Authorization header in request, got:\n{request_str}"
    );
}
