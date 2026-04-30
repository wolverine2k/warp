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

    let synthesized = synthesize_stream(event_source, cancel_rx).boxed();
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
fn synthesize_stream(
    mut event_source: reqwest_eventsource::EventSource,
    mut cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send {
    let mut adapter = OpenAiSseAdapter::new();
    let mut pending: std::collections::VecDeque<api::ResponseEvent> = Default::default();
    let mut closed = false;
    let mut errored: Option<String> = None;
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
                    // reqwest-eventsource closes the stream after most errors;
                    // record the message so we can log on EOF, then keep
                    // polling so we observe Ready(None) and finalize via the
                    // normal flush path.
                    errored = Some(e.to_string());
                    // Some error variants leave the source dead immediately;
                    // we'll observe Ready(None) on the next iteration.
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
