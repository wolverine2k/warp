//! HTTP runner: ties together the request translator, HTTP client, and SSE adapter.
//!
//! Per `specs/GH9303/tech.md` §6:
//! 1. Translate `LocalProviderInput` + `LocalProviderConfig` → OpenAI request body.
//! 2. POST to `{base_url}/chat/completions` with `Authorization: Bearer <key>` if set.
//! 3. Pipe the SSE response through `OpenAiSseAdapter`.
//! 4. Wrap with `take_until(cancel_rx)` so cancellation matches existing behavior.
//!
//! Errors that prevent even producing a stream (connect refused, DNS, auth) are
//! returned as `Result::Err`. Errors that interrupt an already-flowing stream
//! are encoded as `Finished{InternalError}` events by the adapter.

use std::pin::Pin;

use futures::{
    channel::oneshot,
    stream::{self, BoxStream, Stream, StreamExt},
    Future,
};
use reqwest_eventsource::{Event, RequestBuilderExt};
use warp_multi_agent_api as api;

use crate::local_provider::{
    config::LocalProviderConfig,
    request::{compose_chat_completion_request, LocalProviderInput},
    response::OpenAiSseAdapter,
};

/// Errors that prevent the local provider from producing any response stream.
/// Mid-stream errors are encoded as `Finished{InternalError}` events instead.
#[derive(Debug, thiserror::Error)]
pub enum LocalRunError {
    #[error("invalid local provider config: {0}")]
    InvalidConfig(#[from] crate::local_provider::config::LocalProviderConfigError),
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
pub async fn run_chat_turn(
    input: LocalProviderInput,
    cfg: LocalProviderConfig,
    cancel_rx: oneshot::Receiver<()>,
    http: reqwest::Client,
) -> Result<LocalResponseStream, LocalRunError> {
    cfg.validate()?;
    let url = cfg.chat_completions_url()?;
    let body = compose_chat_completion_request(&input, &cfg);
    let body_json = serde_json::to_string(&body)?;

    // Construct the SSE adapter with the conversation's actual ids when the
    // caller plumbed them through (real agent flow). Without this, the
    // adapter's randomly-generated `local:<uuid>` task id never matches the
    // task the controller is driving, every emitted event triggers
    // `UpdateConversationError::TaskNotFound`, and the user sees no output.
    // Falls back to fresh ids when the caller didn't provide any (test paths
    // that drive the adapter in isolation, where matching isn't required).
    let adapter = match (input.conversation_id.as_deref(), input.task_id.as_deref()) {
        (Some(conversation_id), Some(task_id)) => OpenAiSseAdapter::with_ids(
            conversation_id.to_string(),
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            task_id.to_string(),
        ),
        _ => OpenAiSseAdapter::new(),
    };

    let mut request_builder = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .body(body_json);
    if let Some(key) = &cfg.api_key {
        if !key.is_empty() {
            request_builder = request_builder.bearer_auth(key);
        }
    }

    // The only error eventsource() can return is CannotCloneRequestError, and
    // it can't actually fire on a one-shot builder we just constructed. We
    // surface it as a panic with a clear message so future regressions stand out.
    let mut event_source = request_builder
        .eventsource()
        .expect("eventsource() on a fresh, single-use RequestBuilder cannot fail");
    // Disable reqwest_eventsource's built-in exponential-backoff retries.
    // We surface transient failures as Finished{InternalError} immediately
    // so the user can act; the controller's higher-level retry policy
    // decides whether to re-issue the whole turn. Without this, an unreachable
    // local endpoint would block for ~31s of retries before our adapter
    // observes the EOF.
    event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

    let synthesized = synthesize_stream(adapter, event_source, cancel_rx).boxed();
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

fn synthesize_stream(
    mut adapter: OpenAiSseAdapter,
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
    // `adapter.record_upstream_error` before flushing.
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
                        let body = result
                            .unwrap_or_else(|e| format!("(failed to read body: {e})"));
                        let trimmed = body.trim();
                        let excerpt: String =
                            trimmed.chars().take(ERROR_BODY_EXCERPT_CHARS).collect();
                        let msg = if excerpt.is_empty() {
                            prefix.clone()
                        } else {
                            format!("{prefix}: {excerpt}")
                        };
                        log::warn!("local provider stream errored before EOF: {msg}");
                        adapter.record_upstream_error(msg);
                        for ev in adapter.finish() {
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
                for ev in adapter.finish() {
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
                        adapter.record_upstream_error(msg);
                    }
                    for ev in adapter.finish() {
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
                    for ev in adapter.feed(&msg.data) {
                        pending.push_back(ev);
                    }
                    // If the chunk pushed the adapter into a terminal state
                    // (e.g. `[DONE]` or a `finish_reason`), flush its closing
                    // events now and stop pulling from event_source. Some
                    // OpenAI-compatible servers keep the connection open
                    // past `[DONE]` for HTTP/2 multiplexing or keepalive,
                    // and we don't want the response stream hanging on that.
                    if adapter.is_terminal() {
                        for ev in adapter.finish() {
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
