//! HTTP runner: ties together the request translator, HTTP client, and the
//! per-adapter stream decoder.
//!
//! Per `specs/GH9303/tech.md` §6:
//! 1. Select an adapter via `cfg.api_type`.
//! 2. Build the chat request (URL + headers + body via the adapter).
//! 3. Drive the response stream through the adapter's `StreamDecoder`. Phase
//!    3b: branches on `adapter.streaming_format()` — SSE (OpenAi, Anthropic,
//!    future Gemini/DeepSeek) goes through `synthesize_sse_stream`;
//!    NDJSON (Ollama-native) goes through `synthesize_ndjson_stream`.
//! 4. Wrap with `take_until(cancel_rx)` so cancellation matches existing
//!    behavior.
//!
//! Errors that prevent even producing a stream (connect refused, DNS, auth)
//! are returned as `Result::Err` for the SSE path (`eventsource()` doesn't
//! await `send()`; HTTP errors arrive mid-stream from the `EventSource`).
//! The NDJSON path awaits `send()` before returning a stream — pre-flight
//! errors are encoded as `Finished{InternalError}` events on a synthetic
//! single-emit stream, matching the in-stream error behavior.

use std::pin::Pin;

use futures::{
    channel::oneshot,
    stream::{self, BoxStream, Stream, StreamExt},
    Future,
};
use reqwest_eventsource::{Event, RequestBuilderExt};
use warp_multi_agent_api as api;

use crate::local_provider::{config::LocalProviderConfig, request::LocalProviderInput};

/// Errors that prevent the local provider from producing any response stream.
/// Mid-stream errors are encoded as `Finished{InternalError}` events instead.
#[derive(Debug, thiserror::Error)]
pub enum LocalRunError {
    #[error("invalid local provider config: {0}")]
    InvalidConfig(#[from] crate::local_provider::config::LocalProviderConfigError),
    #[error("adapter error: {0}")]
    Adapter(#[from] crate::local_provider::adapters::AdapterError),
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("failed to encode request body: {0}")]
    EncodeRequest(#[from] serde_json::Error),
}

/// The output stream type produced by `run_chat_turn`. Items are
/// `api::ResponseEvent`s the controller can consume. Errors are encoded as
/// `Finished{InternalError}` events within the stream — this stream type
/// does not surface a `Result` per item because the adapter takes care of
/// translating SSE-level failures into proto-level finish events.
pub type LocalResponseStream = BoxStream<'static, api::ResponseEvent>;

/// Run a single chat turn against the user's configured local endpoint.
/// Phase 2: dispatches to the wire-protocol adapter selected by
/// `cfg.api_type` (today only `OpenAi`; Phase 3 adds the rest).
pub async fn run_chat_turn(
    input: LocalProviderInput,
    cfg: LocalProviderConfig,
    cancel_rx: oneshot::Receiver<()>,
    http: reqwest::Client,
) -> Result<LocalResponseStream, LocalRunError> {
    let provider_adapter = crate::local_provider::adapters::select_adapter(cfg.api_type)?;
    let request_builder = provider_adapter.build_chat_request(&input, &cfg, &http)?;

    // Capture the body string for the env-gated debug dump. `try_clone()`
    // returns `None` for non-cloneable bodies (e.g. streamed) — our adapter
    // sets a String body, so cloning succeeds in practice. We swallow the
    // None case rather than fail the turn — diagnostics are best-effort.
    if debug_dump_enabled() {
        let body_dump = request_builder
            .try_clone()
            .and_then(|rb| rb.build().ok())
            .as_ref()
            .and_then(|r| r.body())
            .and_then(|b| b.as_bytes())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_else(|| "<body unavailable for dump>".to_string());
        debug_dump_request(&body_dump);
    }

    // Construct the SSE adapter with the conversation's actual ids when the
    // caller plumbed them through (real agent flow). Without this, the
    // adapter's randomly-generated `local:<uuid>` task id never matches the
    // task the controller is driving, every emitted event triggers
    // `UpdateConversationError::TaskNotFound`, and the user sees no output.
    // Falls back to fresh ids when the caller didn't provide any (test paths
    // that drive the adapter in isolation, where matching isn't required).
    let stream_ids = input.task_id.as_deref().map(|task_id| {
        // The task_id is the load-bearing match — every emitted
        // AddMessagesToTask and AppendToMessageContent carries it, and the
        // controller looks each up in `task_store`. The conversation_id only
        // appears in the synthetic Init event (informational). Synthesize a
        // conversation_id when missing (true on the very first turn, before
        // any server token is assigned).
        let conversation_id = input
            .conversation_id
            .clone()
            .unwrap_or_else(|| format!("local:{}", uuid::Uuid::new_v4()));
        crate::local_provider::adapters::StreamIds {
            conversation_id,
            request_id: uuid::Uuid::new_v4().to_string(),
            run_id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
        }
    });
    let decoder = provider_adapter.create_stream_decoder(stream_ids, !input.needs_create_task);

    // Phase 3b: branch on the adapter's streaming format. SSE adapters
    // build an EventSource and drive it through synthesize_sse_stream;
    // NDJSON adapters await send() (to surface pre-flight HTTP errors as
    // events on a synthetic stream) and drive bytes_stream() through
    // synthesize_ndjson_stream.
    use crate::local_provider::adapters::StreamingFormat;
    let synthesized: LocalResponseStream = match provider_adapter.streaming_format() {
        StreamingFormat::ServerSentEvents => {
            // The only error eventsource() can return is
            // CannotCloneRequestError, and it can't actually fire on a
            // one-shot builder we just constructed. We surface it as a
            // panic with a clear message so future regressions stand out.
            let mut event_source = request_builder
                .eventsource()
                .expect("eventsource() on a fresh, single-use RequestBuilder cannot fail");
            // Disable reqwest_eventsource's built-in exponential-backoff
            // retries. We surface transient failures as
            // Finished{InternalError} immediately so the user can act; the
            // controller's higher-level retry policy decides whether to
            // re-issue the whole turn. Without this, an unreachable local
            // endpoint would block for ~31s of retries before our adapter
            // observes the EOF.
            event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));
            synthesize_sse_stream(decoder, event_source, cancel_rx).boxed()
        }
        StreamingFormat::NewlineDelimitedJson => {
            synthesize_ndjson_stream(decoder, request_builder, cancel_rx).await
        }
    };
    Ok(synthesized)
}

/// Drive the SSE event source through `OpenAiSseAdapter` and emit
/// `ResponseEvent`s. Cancellation is observed via `cancel_rx`; on cancel we
/// emit a Rollback + Finished{Other} sequence.
///
/// The polling loop is an internal `loop` inside `poll_fn` — each invocation
/// drains as many event_source poll cycles as it can until either it produces
/// a downstream event, hits Pending on the inner stream (and properly registers
/// the waker via the standard `cx` propagation), or terminates. Don't try to
/// trampoline through `wake_by_ref()` — `reqwest_eventsource::EventSource`
/// handles its own wake-ups when network data arrives, and self-waking only
/// causes tight-spin behavior that masks legitimate Pending states.
/// How many characters of the upstream HTTP error body to surface in the
/// `Finished{InternalError}` reason. 500 is enough to fit OpenAI's
/// `{"error":{"message":"..."}}` envelopes, including the rate-limit
/// "exceeded your current quota" copy, without flooding the chat UI.
const ERROR_BODY_EXCERPT_CHARS: usize = 500;

/// Boxed async body-read future for an HTTP error response. Stored in the
/// poll_fn closure so subsequent polls can drive it to completion without
/// blocking the stream.
type BodyReadFuture =
    Pin<Box<dyn Future<Output = Result<String, reqwest::Error>> + Send + 'static>>;

fn synthesize_sse_stream(
    mut decoder: Box<dyn crate::local_provider::adapters::StreamDecoder>,
    mut event_source: reqwest_eventsource::EventSource,
    mut cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send {
    let mut pending: std::collections::VecDeque<api::ResponseEvent> = Default::default();
    let mut closed = false;
    let mut errored: Option<String> = None;
    // Holds a (prefix, body-read-future) pair when the upstream returned an
    // HTTP error response that still has a readable body. The prefix is the
    // user-visible status / content-type string; once the future resolves we
    // splice in the body excerpt and feed the combined message to
    // `decoder.record_upstream_error` before flushing.
    let mut body_read: Option<(String, BodyReadFuture)> = None;
    stream::poll_fn(move |cx| {
        use std::task::Poll;
        loop {
            // Drain any pending events first.
            if let Some(ev) = pending.pop_front() {
                return Poll::Ready(Some(ev));
            }
            if closed {
                return Poll::Ready(None);
            }

            // If we kicked off a body-read for an HTTP error response, drive
            // it to completion before anything else. The event_source has
            // already errored out, so polling it further would just churn.
            if let Some((prefix, fut)) = body_read.as_mut() {
                match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => {
                        let body = result.unwrap_or_else(|e| format!("(failed to read body: {e})"));
                        let trimmed = body.trim();
                        let excerpt: String =
                            trimmed.chars().take(ERROR_BODY_EXCERPT_CHARS).collect();
                        let msg = if excerpt.is_empty() {
                            prefix.clone()
                        } else {
                            format!("{prefix}: {excerpt}")
                        };
                        log::warn!("local provider stream errored before EOF: {msg}");
                        decoder.record_upstream_error(msg);
                        for ev in decoder.finish() {
                            pending.push_back(ev);
                        }
                        closed = true;
                        body_read = None;
                        continue;
                    }
                }
            }

            // Cancellation check. Treat both an explicit `send(())` and a
            // dropped sender (`Err(Canceled)`) as a cancel — callers that
            // never plan to cancel still drop the tx side, and we shouldn't
            // hang on event_source if the upstream cancel channel is gone.
            if Pin::new(&mut cancel_rx).poll(cx).is_ready() {
                for ev in decoder.finish() {
                    pending.push_back(ev);
                }
                closed = true;
                continue;
            }

            // Drive the SSE source.
            match Pin::new(&mut event_source).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    if let Some(msg) = errored.take() {
                        log::warn!("local provider stream errored before EOF: {msg}");
                        // Surface the captured upstream error as the
                        // InternalError reason on Finished so the user sees
                        // the real cause in the UI (e.g. HTTP 401 / 400 with
                        // a server-side JSON error body), instead of the
                        // generic "stream ended without finish_reason".
                        decoder.record_upstream_error(msg);
                    }
                    for ev in decoder.finish() {
                        pending.push_back(ev);
                    }
                    closed = true;
                    continue;
                }
                Poll::Ready(Some(Ok(Event::Open))) => {
                    // SSE handshake complete — connection established. No
                    // downstream event to emit; loop and try to read messages.
                    continue;
                }
                Poll::Ready(Some(Ok(Event::Message(msg)))) => {
                    debug_dump_response_chunk(&msg.data);
                    // SSE default `event:` is `"message"`; Anthropic prefixes
                    // each chunk with a named event (`message_start`,
                    // `content_block_delta`, etc.). Pass `None` for the
                    // default so OpenAi's anonymous-chunk path doesn't see
                    // a misleading event name.
                    let event_name = if msg.event.is_empty() || msg.event == "message" {
                        None
                    } else {
                        Some(msg.event.as_str())
                    };
                    for ev in decoder.feed_event(event_name, &msg.data) {
                        pending.push_back(ev);
                    }
                    // If the chunk pushed the decoder into a terminal state
                    // (e.g. `[DONE]` or a `finish_reason`), flush its closing
                    // events now and stop pulling from event_source. Some
                    // OpenAI-compatible servers keep the connection open
                    // past `[DONE]` for HTTP/2 multiplexing or keepalive,
                    // and we don't want the response stream hanging on that.
                    if decoder.is_terminal() {
                        for ev in decoder.finish() {
                            pending.push_back(ev);
                        }
                        closed = true;
                    }
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    // For HTTP error responses (4xx/5xx, wrong content-type)
                    // the response is still in the error variant and its body
                    // is unread — that body usually contains the actionable
                    // message (e.g. OpenAI's quota / model / auth JSON
                    // envelope). Take ownership and kick off an async read;
                    // the next poll will drive it to completion. For other
                    // variants (transport, parser, utf8) the status line is
                    // all we have, so fall back to `e.to_string()`.
                    use reqwest_eventsource::Error;
                    match e {
                        Error::InvalidStatusCode(status, response) => {
                            let prefix = format!(
                                "HTTP {} {}",
                                status.as_u16(),
                                status.canonical_reason().unwrap_or("")
                            );
                            body_read = Some((prefix, Box::pin(response.text())));
                        }
                        Error::InvalidContentType(content_type, response) => {
                            let prefix = format!(
                                "Server returned non-SSE content-type {content_type:?} \
                                 (expected text/event-stream — check Base URL is OpenAI \
                                 Chat Completions compatible)"
                            );
                            body_read = Some((prefix, Box::pin(response.text())));
                        }
                        other => {
                            errored = Some(other.to_string());
                        }
                    }
                    // Some error variants leave the source dead immediately;
                    // we'll observe Ready(None) on the next iteration (or
                    // resolve `body_read` first, whichever comes earlier).
                    continue;
                }
            }
        }
    })
}

// Note on testing: full integration tests for run_chat_turn live in
// crates/integration/ (Phase 7) where a mock OpenAI HTTP server can be booted
// in-process. Unit tests at this layer would need to instantiate a
// `reqwest::Client`, which triggers rustls provider initialization that may
// panic in unit-test contexts depending on workspace TLS-provider setup.
// The pieces this function composes (config validation, request translation,
// SSE adapter) are independently unit-tested in their own modules.

// ---------- NDJSON drive loop (Phase 3b, used by OllamaAdapter) ----------
//
// Differences from synthesize_sse_stream:
//
// - Awaits `request_builder.send()` before driving the byte stream. SSE
//   defers HTTP errors to mid-stream events; NDJSON has no such framing,
//   so we check the response status up front and short-circuit to a
//   pre-baked Finished{InternalError} stream on 4xx/5xx or transport
//   failure.
// - Reads from `response.bytes_stream()` (chunked HTTP transfer) and
//   accumulates into a Vec<u8> buffer, draining complete lines on each
//   poll cycle before pulling more bytes. Buffer-across-chunks because
//   HTTP chunk boundaries don't align with JSON line boundaries.
// - Cancellation, terminal-state checks, and the `pending` event queue
//   match the SSE loop's pattern verbatim.

async fn synthesize_ndjson_stream(
    decoder: Box<dyn crate::local_provider::adapters::StreamDecoder>,
    request_builder: reqwest::RequestBuilder,
    cancel_rx: oneshot::Receiver<()>,
) -> LocalResponseStream {
    let response = match request_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            return synthesize_pre_flight_error(decoder, format!("request failed: {e}"));
        }
    };
    let status = response.status();
    if !status.is_success() {
        let prefix = format!(
            "HTTP {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        // Read body up to ERROR_BODY_EXCERPT_CHARS for the user-visible
        // reason. Defensive on .text() errors — if the body can't be
        // read, surface just the HTTP status.
        let body = response.text().await.unwrap_or_default();
        let excerpt: String = body
            .trim()
            .chars()
            .take(ERROR_BODY_EXCERPT_CHARS)
            .collect();
        let msg = if excerpt.is_empty() {
            prefix
        } else {
            format!("{prefix}: {excerpt}")
        };
        return synthesize_pre_flight_error(decoder, msg);
    }
    drive_ndjson(decoder, response.bytes_stream(), cancel_rx).boxed()
}

/// Build a single-shot stream that records `msg` as the upstream error,
/// then drains `decoder.finish()` — same shape as the SSE loop's
/// error-then-finish path. Used by `synthesize_ndjson_stream` for
/// pre-flight failures (transport error, 4xx/5xx) where there's no
/// byte stream to drive at all.
fn synthesize_pre_flight_error(
    mut decoder: Box<dyn crate::local_provider::adapters::StreamDecoder>,
    msg: String,
) -> LocalResponseStream {
    log::warn!("local provider stream errored before EOF: {msg}");
    decoder.record_upstream_error(msg);
    let events = decoder.finish();
    stream::iter(events).boxed()
}

fn drive_ndjson<S>(
    mut decoder: Box<dyn crate::local_provider::adapters::StreamDecoder>,
    byte_stream: S,
    mut cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send + 'static
where
    S: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    let mut byte_stream = Box::pin(byte_stream);
    let mut pending: std::collections::VecDeque<api::ResponseEvent> = Default::default();
    let mut closed = false;
    let mut buffer: Vec<u8> = Vec::new();
    stream::poll_fn(move |cx| {
        use std::task::Poll;
        loop {
            if let Some(ev) = pending.pop_front() {
                return Poll::Ready(Some(ev));
            }
            if closed {
                return Poll::Ready(None);
            }
            // Cancellation: explicit send(()) OR sender drop both unblock.
            if Pin::new(&mut cancel_rx).poll(cx).is_ready() {
                for ev in decoder.finish() {
                    pending.push_back(ev);
                }
                closed = true;
                continue;
            }
            // Drain any complete lines from the buffer before pulling
            // more bytes. Empty lines are silently skipped (defensive —
            // Ollama doesn't emit them, but a relay might).
            while let Some(idx) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=idx).collect();
                let line_str = String::from_utf8_lossy(&line[..line.len() - 1]);
                if line_str.trim().is_empty() {
                    continue;
                }
                debug_dump_response_chunk(&line_str);
                for ev in decoder.feed_event(None, &line_str) {
                    pending.push_back(ev);
                }
                if decoder.is_terminal() {
                    for ev in decoder.finish() {
                        pending.push_back(ev);
                    }
                    closed = true;
                    break;
                }
            }
            if !pending.is_empty() || closed {
                continue;
            }
            // Pull more bytes.
            match byte_stream.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // EOF. Flush any final unterminated line, then drain.
                    if !buffer.is_empty() {
                        let line_str = String::from_utf8_lossy(&buffer);
                        if !line_str.trim().is_empty() {
                            debug_dump_response_chunk(&line_str);
                            for ev in decoder.feed_event(None, &line_str) {
                                pending.push_back(ev);
                            }
                        }
                        buffer.clear();
                    }
                    for ev in decoder.finish() {
                        pending.push_back(ev);
                    }
                    closed = true;
                }
                Poll::Ready(Some(Ok(bytes))) => buffer.extend_from_slice(&bytes),
                Poll::Ready(Some(Err(e))) => {
                    log::warn!("local provider stream errored before EOF: network error: {e}");
                    decoder.record_upstream_error(format!("network error: {e}"));
                    for ev in decoder.finish() {
                        pending.push_back(ev);
                    }
                    closed = true;
                }
            }
        }
    })
}

// ---------- diagnostic dump (env-gated, dev-only) ----------
//
// Set `WARP_LOCAL_PROVIDER_DEBUG_DUMP=1` to write the outbound request body
// to `/tmp/warp-local-provider-last-request.json` (overwritten per turn) and
// each inbound SSE chunk to `/tmp/warp-local-provider-last-response.log`
// (appended; cleared on each new turn). Useful for diagnosing
// "model isn't following our system prompt / tools advertisement" issues
// when the upstream is a third-party OpenAI-compat endpoint that may
// transform messages in unexpected ways. Defaults to no-op so production
// builds don't touch the filesystem unless an operator opts in.

const DEBUG_DUMP_ENV: &str = "WARP_LOCAL_PROVIDER_DEBUG_DUMP";
const DEBUG_REQUEST_PATH: &str = "/tmp/warp-local-provider-last-request.json";
const DEBUG_RESPONSE_PATH: &str = "/tmp/warp-local-provider-last-response.log";

fn debug_dump_enabled() -> bool {
    std::env::var(DEBUG_DUMP_ENV)
        .ok()
        .map(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

fn debug_dump_request(body_json: &str) {
    if !debug_dump_enabled() {
        return;
    }
    // Pretty-print so the body is readable; fall back to the original
    // string if reparse fails (very unlikely — we just serialized it).
    let pretty = serde_json::from_str::<serde_json::Value>(body_json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| body_json.to_string());
    if let Err(e) = std::fs::write(DEBUG_REQUEST_PATH, pretty) {
        log::warn!("debug_dump_request: failed to write {DEBUG_REQUEST_PATH}: {e}");
        return;
    }
    // Also truncate the response log so each turn starts fresh.
    let _ = std::fs::write(DEBUG_RESPONSE_PATH, "");
    log::info!(
        "[local-provider-debug] wrote request body to {DEBUG_REQUEST_PATH} \
         (response chunks will accumulate at {DEBUG_RESPONSE_PATH})"
    );
}

fn debug_dump_response_chunk(chunk_data: &str) {
    if !debug_dump_enabled() {
        return;
    }
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(DEBUG_RESPONSE_PATH)
    {
        Ok(mut f) => {
            let _ = writeln!(f, "{chunk_data}");
        }
        Err(e) => {
            log::warn!("debug_dump_response_chunk: failed to append {DEBUG_RESPONSE_PATH}: {e}");
        }
    }
}

// ---------- summarizer (non-streaming) ----------

use crate::local_provider::wire::{ChatMessage, Role};

/// Inputs for [`run_summarizer_turn`] — a self-contained summarization call.
/// Distinct from [`LocalProviderInput`] because the summarizer doesn't share
/// the SSE/controller plumbing — it's a one-shot helper.
#[derive(Debug, Clone)]
pub struct SummarizerInput {
    /// Pre-composed messages to send to the summarizer model. The caller is
    /// responsible for shape: typically `[system, ...history, user(prompt)]`
    /// where the user prompt comes from
    /// [`crate::local_provider::compaction::prompt::build_prompt`].
    pub messages: Vec<ChatMessage>,
}

/// Errors specific to [`run_summarizer_turn`]. Streaming-path errors come
/// back as `Finished{InternalError}` events; the summarizer is a one-shot
/// call so we expose them directly.
#[derive(Debug, thiserror::Error)]
pub enum SummarizerError {
    #[error("invalid local provider config: {0}")]
    InvalidConfig(#[from] crate::local_provider::config::LocalProviderConfigError),
    /// Phase 2: an `AdapterError` from the wire-protocol adapter, stringified
    /// to avoid wrapping `LocalProviderConfigError` twice (it's already
    /// reachable via `InvalidConfig`).
    #[error("adapter error: {0}")]
    Adapter(String),
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("failed to encode summarizer request body: {0}")]
    EncodeRequest(#[from] serde_json::Error),
    #[error("upstream returned HTTP {status}: {body}")]
    UpstreamHttp { status: u16, body: String },
    #[error("upstream returned a non-JSON or malformed body: {0}")]
    DecodeResponse(String),
    #[error("upstream reported an error envelope: {0}")]
    UpstreamErrorEnvelope(String),
    #[error("summarizer response had no assistant content")]
    NoContent,
}

/// Issue a single non-streaming Chat Completions request and return the
/// assistant text. Used by the head-summary compaction path; `run_chat_turn`
/// stays the only entry point for normal turns.
///
/// Behaviour notes:
/// - `stream` is forced to `false` regardless of caller setup — this path
///   reads `ChatCompletionResponse`, not `ChatCompletionChunk`s.
/// - `tools` / `tool_choice` are not sent — summarization is a plain text
///   completion and tool advertisements would only confuse the model.
/// - The reasoning channels (`reasoning_content` / `reasoning`) are
///   prepended to the visible text only when `content` is empty; a
///   well-behaved summarizer puts the structured Markdown in `content`.
pub async fn run_summarizer_turn(
    input: SummarizerInput,
    cfg: &crate::local_provider::config::LocalProviderConfig,
    http: &reqwest::Client,
) -> Result<String, SummarizerError> {
    let provider_adapter = crate::local_provider::adapters::select_adapter(cfg.api_type)
        .map_err(|e| SummarizerError::Adapter(e.to_string()))?;
    let request_builder = provider_adapter
        .build_summarizer_request(&input, cfg, http)
        .map_err(|e| SummarizerError::Adapter(e.to_string()))?;

    let resp = request_builder.send().await?;
    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(SummarizerError::UpstreamHttp {
            status: status.as_u16(),
            body: text.chars().take(500).collect(),
        });
    }
    provider_adapter.parse_summarizer_response(&text)
}

pub(crate) fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Convenience constructor for the canonical 3-message summarizer body
/// `[system?, history..., user(prompt)]`. Most callers should hand-build
/// `Vec<ChatMessage>` themselves; this helper exists to keep the common case
/// terse.
///
/// `system_prompt = None` skips the system message entirely. Some local
/// servers reject zero-system bodies; pass `Some("You are a summarizer.")`
/// or similar in that case.
pub fn build_summarizer_messages(
    system_prompt: Option<&str>,
    history: Vec<ChatMessage>,
    user_prompt: String,
) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(history.len() + 2);
    if let Some(sys) = system_prompt {
        out.push(ChatMessage {
            role: Role::System,
            content: Some(sys.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
    }
    out.extend(history);
    out.push(ChatMessage {
        role: Role::User,
        content: Some(user_prompt),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    });
    out
}
