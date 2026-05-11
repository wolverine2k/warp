# Multi-Local-LLM — Phase 3b (Ollama-native Adapter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a native `OllamaAdapter` targeting `POST {base_url}/api/chat` and flip `select_adapter` for `AgentProviderApiType::Ollama` from `Err(UnsupportedApiType)` to a real impl. **First adapter to use a non-SSE streaming format** — adds a small `streaming_format()` trait method + an NDJSON drive loop in `synthesize_stream`. Unblocks Ollama users who want native tool-call streaming + the `options.num_ctx` knob for KV-cache sizing (the OpenAI-compat layer at `/v1/chat/completions` doesn't expose either).

**Architecture:** Four logical stages, atomic in one PR (split into 3b-i / 3b-ii / 3b-iii / 3b-iv if review prefers):

- **Stage A (Tasks 1-3)** — Wire types, request translator, URL helpers. Mirrors Phase 3a's submodule layout. Reuses `summarize_tool_call_input` (added in Phase 3a) for proto-to-JSON-Value tool-call arg conversion.
- **Stage B (Tasks 4-5)** — `streaming_format()` trait method (default returns SSE so OpenAi + Anthropic stay unchanged) + `OllamaDecoder` that consumes NDJSON chunks and emits the canonical `ResponseEvent` shape.
- **Stage C (Tasks 6-7)** — Extract existing SSE drive loop into `synthesize_sse_stream`, add `synthesize_ndjson_stream` that drives `response.bytes_stream()` through a line splitter, and have `synthesize_stream` branch on the adapter's streaming format. Implement `OllamaAdapter` and flip the dispatch.
- **Stage D (Task 8)** — Manual smoke against a local `ollama serve` instance with a tool-using model (llama3.1, qwen2.5-coder), spec doc updates.

**Branch:** `multi-local-llm`. Forks from `0b33ece3` (Phase 3a tip). Estimated ~600 lines net code (~100 wire types + 250 translator + 250 decoder + 80 NDJSON drive loop + 50 adapter glue — Ollama's native shape is closer to OpenAI than Anthropic, so the translator is simpler than 3a's), ~5 hours of subagent-driven work.

**Spec references:**
- `specs/multi-local-llm/design.md` §9 (Phase 3b row — gets the "code complete" status flip in Task 8).
- `specs/multi-local-llm/plan-phase-3a.md` §"Next plan (Phase 3b — Ollama-native adapter)" — superseded by this document.
- Ollama Chat API: https://github.com/ollama/ollama/blob/main/docs/api.md#generate-a-chat-completion.

**Test gate:** All existing `cargo nextest run -p ai` tests pass; new Ollama-specific tests added (~45). Manual smoke: a single Ollama provider configured with `base_url = http://localhost:11434`, api_type `Ollama`, runs a turn that streams assistant text, fires a tool call, gets a result, and emits a final assistant message. "Test connection" probe succeeds.

**Out of Phase 3b (deferred):**
- Gemini adapter — Phase 3c.
- DeepSeek adapter — Phase 3d.
- `<think>...</think>` content parsing (Ollama models like DeepSeek-R1-distill embed reasoning in `content` rather than a separate channel) — Phase 4 polish.
- `/api/show` per-model capability fetch (context window, tool support flag) — Phase 4a.
- `keep_alive` model-loading control — Phase 4 polish.

---

## Design refinement

### Endpoint + auth

- **Chat:** `POST {base_url}/api/chat`
- **Probe:** `GET {base_url}/api/tags` (Ollama's installed-model list; cheap, no model load).
- **Auth:** **Optional Bearer.** Most Ollama instances run locally and don't require auth. Hosted/proxied deployments (a fronted Ollama behind a reverse-proxy with token auth) include `Authorization: Bearer <key>` when `cfg.api_key` is set. No anthropic-version equivalent.
- **Default `base_url`:** `http://localhost:11434` (no path prefix). The adapter joins `/api/chat` and `/api/tags` directly — no `/v1` idempotency dance needed because Ollama's native paths are unambiguous.

### Streaming format: NDJSON, not SSE

**This is the biggest design wrinkle.** Ollama's `/api/chat` streams via **newline-delimited JSON** (`Content-Type: application/x-ndjson` or `application/json`) — each line of the response body is a complete `OllamaChatChunk` object, terminated by the chunk with `done: true`. There is no SSE `data:` / `event:` framing.

The existing `synthesize_stream` in `run.rs` builds a `reqwest_eventsource::EventSource` which only understands SSE. For Ollama we need a parallel drive loop over `response.bytes_stream()` with a line splitter.

**Resolution:** add a small trait method that lets the runner branch:

```rust
// crates/ai/src/local_provider/adapters/mod.rs

/// Wire framing for an adapter's chat stream. Drives `synthesize_stream`'s
/// HTTP-loop dispatch. Default returns `ServerSentEvents` so existing
/// adapters (OpenAi, Anthropic) keep their current behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingFormat {
    ServerSentEvents,
    NewlineDelimitedJson,
}

pub trait ProviderAdapter: Send + Sync {
    // ... existing methods ...

    /// Default returns SSE. `OllamaAdapter` overrides to
    /// `NewlineDelimitedJson`. New adapters that stream via SSE need not
    /// implement this.
    fn streaming_format(&self) -> StreamingFormat {
        StreamingFormat::ServerSentEvents
    }
}
```

`OpenAiAdapter` and `AnthropicAdapter` inherit the default — no change. Phase 3c (Gemini, SSE) and 3d (DeepSeek, SSE) will also inherit the default. Only `OllamaAdapter` overrides.

The `synthesize_stream` function in `run.rs` becomes a thin dispatcher:

```rust
async fn run_chat_turn(...) -> Result<LocalResponseStream, LocalRunError> {
    let adapter = select_adapter(cfg.api_type)?;
    let request_builder = adapter.build_chat_request(&input, &cfg, &http)?;
    // ... debug dump + stream id setup unchanged ...
    let decoder = adapter.create_stream_decoder(stream_ids, !input.needs_create_task);

    let synthesized = match adapter.streaming_format() {
        StreamingFormat::ServerSentEvents => {
            let mut event_source = request_builder.eventsource()
                .expect("eventsource() on a fresh RequestBuilder cannot fail");
            event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));
            synthesize_sse_stream(decoder, event_source, cancel_rx).boxed()
        }
        StreamingFormat::NewlineDelimitedJson => {
            let response = request_builder.send().await?;
            synthesize_ndjson_stream(decoder, response, cancel_rx).boxed()
        }
    };
    Ok(synthesized)
}
```

`synthesize_sse_stream` is the existing `synthesize_stream` body, renamed verbatim. `synthesize_ndjson_stream` is new (~80 lines, design below).

### NDJSON drive loop

```rust
fn synthesize_ndjson_stream(
    mut decoder: Box<dyn StreamDecoder>,
    response: reqwest::Response,
    mut cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send {
    let status = response.status();
    let mut pending: VecDeque<api::ResponseEvent> = Default::default();
    let mut closed = false;

    // HTTP-level errors: read body up to ERROR_BODY_EXCERPT_CHARS, record
    // as upstream error, finish with Rollback. Mirrors the SSE error path.
    if !status.is_success() {
        let prefix = format!("HTTP {} {}", status.as_u16(),
            status.canonical_reason().unwrap_or(""));
        let body_read: BodyReadFuture = Box::pin(response.text());
        return ndjson_error_stream(decoder, prefix, body_read);
    }

    let mut byte_stream = response.bytes_stream();
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
            // Cancellation.
            if Pin::new(&mut cancel_rx).poll(cx).is_ready() {
                for ev in decoder.finish() { pending.push_back(ev); }
                closed = true;
                continue;
            }
            // Drain any complete lines from the buffer first.
            while let Some(newline_idx) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline_idx).collect();
                let line_str = String::from_utf8_lossy(&line[..line.len() - 1]);
                // Skip empty lines (defensive — Ollama doesn't emit them,
                // but a proxy in front might).
                if line_str.trim().is_empty() { continue; }
                for ev in decoder.feed_event(None, &line_str) {
                    pending.push_back(ev);
                }
                if decoder.is_terminal() {
                    for ev in decoder.finish() { pending.push_back(ev); }
                    closed = true;
                    break;
                }
            }
            if !pending.is_empty() || closed { continue; }
            // Pull more bytes.
            match Pin::new(&mut byte_stream).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // EOF. Drain any final unterminated line (shouldn't
                    // happen — Ollama always sends a `done:true` chunk
                    // terminated by `\n`).
                    if !buffer.is_empty() {
                        let line_str = String::from_utf8_lossy(&buffer);
                        if !line_str.trim().is_empty() {
                            for ev in decoder.feed_event(None, &line_str) {
                                pending.push_back(ev);
                            }
                        }
                    }
                    for ev in decoder.finish() { pending.push_back(ev); }
                    closed = true;
                }
                Poll::Ready(Some(Ok(bytes))) => buffer.extend_from_slice(&bytes),
                Poll::Ready(Some(Err(e))) => {
                    decoder.record_upstream_error(format!("network error: {e}"));
                    for ev in decoder.finish() { pending.push_back(ev); }
                    closed = true;
                }
            }
        }
    })
}
```

The `ndjson_error_stream` helper (for HTTP 4xx/5xx with a JSON error body) mirrors the SSE error path: read the body, splice into a user-visible message, decoder.record_upstream_error, decoder.finish, Rollback + Finished{InternalError}.

### Request body shape (native Ollama)

```jsonc
{
  "model": "llama3.1",
  "stream": true,
  "messages": [
    {"role": "system", "content": "..."},
    {"role": "user",   "content": "..."},
    {"role": "assistant", "content": "",
     "tool_calls": [{"function": {"name": "read_files", "arguments": {"paths":["x"]}}}]},
    {"role": "tool", "content": "...rendered tool result..."}
  ],
  "tools": [
    {"type": "function",
     "function": {"name": "read_files", "description": "...", "parameters": {...}}}
  ],
  "options": {
    "num_ctx": 128000
  }
}
```

**Differences from the OpenAI-compat shape Ollama also accepts at `/v1/chat/completions`:**
1. `system` lives as a `role:"system"` message — same as OpenAI. (Ollama also accepts a top-level `system` string; we use the message form to share code with the OpenAI translator's structure.)
2. `tool_calls[]` entries have **no `id` field** and **no `type:"function"` field**. Just `{function: {name, arguments}}`. Ollama tolerates the OpenAI-style fields when they're present — but we emit the native shape to keep the body minimal and avoid lying with synthesized ids.
3. **`tool_calls[].function.arguments` is a JSON OBJECT**, not a stringified-JSON string. This is the most important shape difference at the wire level — `serde_json::Value` instead of `String`.
4. `role:"tool"` messages omit `tool_call_id` and `name` — Ollama ignores them. We always omit for native cleanness.
5. `options.num_ctx` threads `cfg.context_window` through so Ollama sizes the KV cache appropriately. Without this, Ollama uses the model's default (typically 2048 or 4096), which truncates long histories silently. **Critical for the BYOP use case** where users configure large-context models.
6. `max_tokens` is **not required** (unlike Anthropic). Ollama uses `options.num_predict` for max output tokens; we omit it entirely (let the model decide; Phase 4 polish can wire it from a new `max_output_tokens` model field).

### Streaming response shape

NDJSON. Each line:

```jsonc
{"model":"llama3.1","created_at":"2026-05-11T...","message":{"role":"assistant","content":"Hello"},"done":false}
{"model":"llama3.1","created_at":"...","message":{"role":"assistant","content":" world"},"done":false}

// Tool call — arrives complete in ONE chunk (no fragmentation):
{"model":"llama3.1","created_at":"...","message":{"role":"assistant","content":"",
  "tool_calls":[{"function":{"name":"read_files","arguments":{"paths":["x"]}}}]},
  "done":false}

// Terminator chunk:
{"model":"llama3.1","created_at":"...","message":{"role":"assistant","content":""},
  "done":true,"done_reason":"stop",
  "total_duration":<ns>,"load_duration":<ns>,
  "prompt_eval_count":50,"eval_count":120,"prompt_eval_duration":<ns>,"eval_duration":<ns>}
```

**Key wire-format quirks:**
- Each chunk has `message.role` (always `"assistant"` on chat responses), `message.content` (possibly empty), optional `message.tool_calls`.
- `done: bool` indicates whether this is the final chunk. **The decoder uses this as the terminator** — no `[DONE]` line, no `message_stop` event.
- Final chunk's `message.content` is usually `""` — the textual content has already been streamed.
- `done_reason` (final chunk only): `"stop"` / `"length"` / `"load"` / `"unload"`.
- Usage: `prompt_eval_count` (input tokens) + `eval_count` (output tokens). No cache fields.

### Decoder design

Simpler than Anthropic's per-block state machine because each NDJSON chunk is a complete delta for a single conceptual message:

```rust
pub struct OllamaDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,
    // One shared open AgentOutput message across the turn (matches
    // OpenAi's pattern, not Anthropic's per-block). Ollama emits text in
    // one logical stream per turn — distinct content blocks don't exist.
    text_message_id: Option<String>,
    captured_done_reason: Option<String>,
    captured_model: Option<String>,
    captured_input_tokens: u64,
    captured_output_tokens: u64,
}

enum State { Streaming, Done, Errored }
```

Per-chunk dispatch:

```rust
fn feed_event(&mut self, _event_name: Option<&str>, data: &str)
    -> Vec<api::ResponseEvent>
{
    if matches!(self.state, State::Done | State::Errored) { return vec![]; }
    let trimmed = data.trim();
    if trimmed.is_empty() { return vec![]; }

    let mut out = self.ensure_prelude();
    let chunk: OllamaChatChunk = match serde_json::from_str(trimmed) {
        Ok(c) => c,
        Err(e) => {
            self.state = State::Errored;
            self.upstream_error.get_or_insert_with(|| format!("malformed Ollama chunk: {e}"));
            return out;
        }
    };

    // Top-level `error` field (rare; some Ollama versions emit this).
    if let Some(err_msg) = chunk.error {
        self.upstream_error = Some(err_msg);
        self.state = State::Errored;
        return out;
    }

    if self.captured_model.is_none() {
        if let Some(m) = chunk.model.filter(|s| !s.is_empty()) {
            self.captured_model = Some(m);
        }
    }

    if let Some(message) = chunk.message {
        // Visible text — append (or open) the shared AgentOutput.
        if !message.content.is_empty() {
            self.append_text(&message.content, &mut out);
        }
        // Tool calls — each arrives complete; emit one
        // AddMessagesToTask{ToolCall} per entry.
        if let Some(tool_calls) = message.tool_calls {
            for tc in tool_calls {
                self.emit_tool_call(&tc, &mut out);
            }
        }
    }

    if chunk.done {
        self.captured_done_reason = chunk.done_reason;
        if let Some(n) = chunk.prompt_eval_count { self.captured_input_tokens = n; }
        if let Some(n) = chunk.eval_count { self.captured_output_tokens = n; }
        self.state = State::Done;
    }

    out
}
```

**Tool-call id synthesis:**

Ollama doesn't send tool-call ids. The decoder synthesizes one per call:

```rust
fn emit_tool_call(&mut self, tc: &OllamaToolCall, out: &mut Vec<api::ResponseEvent>) {
    let id = format!("ollama-call-{}", uuid::Uuid::new_v4());
    let args_json = serde_json::to_string(&tc.function.arguments).unwrap_or_default();
    if let Some(ev) = build_tool_call_event(&self.task_id, &id, &tc.function.name, &args_json) {
        out.push(ev);
    }
}
```

`build_tool_call_event` is the same helper Phase 3a's response.rs uses — reuses `translate_openai_tool_call` for JSON-args → typed proto conversion. The synthesized id flows through the proto `Message::ToolCall.tool_call_id` field; the controller threads it back into `action_results` keyed by that id; the translator emits the tool result with no id reference (Ollama doesn't need it).

### `done_reason` mapping

```rust
fn map_ollama_done_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "stop" => Reason::Done(Done {}),
        "length" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        // "load" means the model finished loading mid-stream — shouldn't
        // happen for chat (it does for /generate when first-loading);
        // surface as Other. "unload" is "model was unloaded mid-stream"
        // — also Other.
        _ => Reason::Other(Other {}),
    }
}
```

### URL helpers

In `LocalProviderConfig`:

```rust
pub fn ollama_chat_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.join_ollama_path("api/chat")
}

pub fn ollama_tags_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.join_ollama_path("api/tags")
}

fn join_ollama_path(&self, leaf: &str) -> Result<Url, LocalProviderConfigError> {
    let mut base = Url::parse(&self.base_url)
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))?;
    if !base.path().ends_with('/') {
        let p = format!("{}/", base.path());
        base.set_path(&p);
    }
    base.join(leaf).map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))
}
```

No idempotent `/v1` handling needed — Ollama's path layout has no prefix to worry about.

### Adapter file structure

Mirrors Phase 3a:

```
crates/ai/src/local_provider/adapters/ollama/
├── mod.rs              # OllamaAdapter + ProviderAdapter trait impl
├── wire.rs             # Serde types for /api/chat
├── request.rs          # compose_ollama_chat_request
├── request_tests.rs
├── response.rs         # OllamaDecoder
└── response_tests.rs
```

### `select_adapter` flip

```rust
pub fn select_adapter(api_type: AgentProviderApiType)
    -> Result<Box<dyn ProviderAdapter>, AdapterError>
{
    use AgentProviderApiType::*;
    match api_type {
        OpenAi    => Ok(Box::new(OpenAiAdapter)),
        Anthropic => Ok(Box::new(AnthropicAdapter)),
        Ollama    => Ok(Box::new(OllamaAdapter)),       // flipped in Phase 3b
        OpenAiResp | Gemini | DeepSeek => {
            Err(AdapterError::UnsupportedApiType(api_type))
        }
    }
}
```

### Settings UI

Same situation as Phase 3a — the widget already renders every `AgentProviderApiType` variant as a clickable chip via `EnumIter` without per-variant gating. No UI change needed; selecting `Ollama` now dispatches correctly.

---

## File map

**Files created:**
- `crates/ai/src/local_provider/adapters/ollama/mod.rs` — adapter impl.
- `crates/ai/src/local_provider/adapters/ollama/wire.rs` — serde types.
- `crates/ai/src/local_provider/adapters/ollama/request.rs` — translator.
- `crates/ai/src/local_provider/adapters/ollama/request_tests.rs` — sibling tests.
- `crates/ai/src/local_provider/adapters/ollama/response.rs` — NDJSON decoder.
- `crates/ai/src/local_provider/adapters/ollama/response_tests.rs` — sibling tests.

**Files modified:**
- `crates/ai/src/local_provider/adapters/mod.rs` — register `pub mod ollama;`, re-export `OllamaAdapter`, add `StreamingFormat` enum + `streaming_format()` trait method, flip `select_adapter(Ollama)`.
- `crates/ai/src/local_provider/adapters/adapters_tests.rs` — add `select_adapter_returns_ollama_for_*`; remove `Ollama` from the unimplemented-variants loop.
- `crates/ai/src/local_provider/run.rs` — rename existing `synthesize_stream` body to `synthesize_sse_stream`; add `synthesize_ndjson_stream`; turn the outer `synthesize_stream` into a dispatcher on `adapter.streaming_format()`.
- `crates/ai/src/local_provider/config.rs` — `ollama_chat_url()` + `ollama_tags_url()` helpers and tests.

**Cargo deps:** none added. `bytes_stream` is already available on `reqwest::Response` via the existing feature set.

---

## Stage A: Wire types + request composition

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git log --oneline -1                     # 0b33ece3 docs(specs/multi-local-llm): add plan-phase-3a.md
cargo nextest run -p ai 2>&1 | tail -3   # 426 / 426 passed
```

If anything diverges, STOP and report.

### Task 1: Ollama wire types

**File:** Create `crates/ai/src/local_provider/adapters/ollama/wire.rs`.

- [ ] **Step 1.1: Request types**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub stream: bool,
    pub messages: Vec<OllamaChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OllamaToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaChatMessage {
    pub role: OllamaRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OllamaOutboundToolCall>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OllamaRole { System, User, Assistant, Tool }

#[derive(Debug, Clone, Serialize)]
pub struct OllamaOutboundToolCall {
    pub function: OllamaOutboundToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaOutboundToolCallFunction {
    pub name: String,
    /// JSON object — Ollama's wire format expects an object here (vs OpenAI's stringified JSON).
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaToolDef {
    #[serde(rename = "type")]
    pub kind: &'static str, // "function"
    pub function: OllamaToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct OllamaOptions {
    /// Maps to `cfg.context_window` — Ollama uses this to size the KV cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
}
```

- [ ] **Step 1.2: Streaming response types**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaChatChunk {
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub created_at: Option<String>,
    #[serde(default)] pub message: Option<OllamaInboundMessage>,
    #[serde(default)] pub done: bool,
    #[serde(default)] pub done_reason: Option<String>,
    #[serde(default)] pub prompt_eval_count: Option<u64>,
    #[serde(default)] pub eval_count: Option<u64>,
    /// Some Ollama versions surface top-level errors mid-stream.
    #[serde(default)] pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaInboundMessage {
    #[serde(default)] pub role: Option<String>,
    #[serde(default)] pub content: String,
    #[serde(default)] pub tool_calls: Option<Vec<OllamaInboundToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaInboundToolCall {
    pub function: OllamaInboundToolCallFunction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaInboundToolCallFunction {
    pub name: String,
    /// Object form. Ollama emits a typed JSON object here.
    #[serde(default)]
    pub arguments: Value,
}
```

- [ ] **Step 1.3: Inline tests + commit**

Tests cover:
- Request serializes with `role: "user"`, `stream: true`, `options.num_ctx` when set, tools array with `type: "function"` wrapper.
- Deserialize a streaming-text chunk: confirms `done: false` and `message.content` extraction.
- Deserialize a final chunk with `done: true`, `done_reason`, `prompt_eval_count`, `eval_count`.
- Deserialize a tool-call chunk: confirms `tool_calls[0].function.name` and `arguments` as object.
- Deserialize a chunk with top-level `error`.

Commit:

```
feat(ai/local_provider/adapters/ollama): add wire types

Phase 3b stage A. Serde types for the native Ollama /api/chat
request (with options.num_ctx, native tool_calls without ids, no
type:function wrapper on tool_calls), streaming NDJSON chunks
(message{content, tool_calls}, done bool, done_reason, eval counts),
and a top-level error envelope. tool_calls[].function.arguments is
a serde_json::Value (object), not a string — the Ollama wire-level
shape divergence from OpenAI.
```

### Task 2: Request translator

**Files:**
- Create `crates/ai/src/local_provider/adapters/ollama/request.rs`.
- Create `crates/ai/src/local_provider/adapters/ollama/request_tests.rs`.

- [ ] **Step 2.1: `compose_ollama_chat_request`**

Mirror the OpenAI translator's high-level structure but emit Ollama-native shape:

```rust
use std::collections::HashMap;
use warp_multi_agent_api as api;

use super::wire::*;
use crate::local_provider::{
    compaction,
    config::LocalProviderConfig,
    prompt,
    request::{
        enabled_local_tools, summarize_tool_call_input, summarize_tool_result, LocalProviderInput,
    },
    tools::LocalTool,
};

pub fn compose_ollama_chat_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> OllamaChatRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tool_definitions_ollama(&local_tools))
    } else {
        None
    };

    // System prompt as a role:"system" message (Ollama accepts both
    // shapes; the message form keeps the translator pipeline uniform
    // with OpenAI's).
    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system_prompt = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );

    let mut messages: Vec<OllamaChatMessage> = vec![OllamaChatMessage {
        role: OllamaRole::System,
        content: system_prompt,
        tool_calls: None,
    }];

    // Compaction projection — same as OpenAI/Anthropic translators.
    // (Synthesize the user "Continue..." + assistant <summary> pair when
    // compaction_state has a completed entry; skip pre-tail history per
    // the projection's tail_start_id.)
    // ... walker logic mirrors crate::local_provider::request::compose_chat_completion_request ...

    // Synthetic user-query anchoring (Phase B-6 parity).
    // ... mirror existing pattern ...

    // Final user_query append.
    // ... mirror existing pattern ...

    // Orphan tool_call backfill: ensure every assistant tool_calls is
    // followed by a role:"tool" message per tool_call_id. Ollama is
    // less strict than OpenAI/Anthropic here (tolerates missing tool
    // results) but we backfill anyway for parity.

    let options = cfg.context_window.filter(|n| *n > 0).map(|num_ctx| OllamaOptions {
        num_ctx: Some(num_ctx),
    });

    OllamaChatRequest {
        model: cfg.model_id.clone(),
        stream: true,
        messages,
        tools,
        options,
    }
}

fn tool_definitions_ollama(enabled: &[LocalTool]) -> Vec<OllamaToolDef> {
    enabled.iter().filter_map(|t| {
        crate::local_provider::tools::schema_for_pub(*t).map(|parameters| OllamaToolDef {
            kind: "function",
            function: OllamaToolFunction {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters,
            },
        })
    }).collect()
}
```

**Helper promotion:** `schema_for` in `tools.rs` is private today. Promote to `pub(crate) schema_for_pub(LocalTool) -> Option<Value>` so both Ollama and OpenAI tool-definitions builders can share the JSON Schema bodies. Trivial change.

**History walking:** Map proto messages to Ollama shape:

```rust
fn push_proto_message(out: &mut Vec<OllamaChatMessage>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => out.push(OllamaChatMessage {
            role: OllamaRole::User,
            content: q.query.clone(),
            tool_calls: None,
        }),
        Some(M::AgentOutput(a)) => out.push(OllamaChatMessage {
            role: OllamaRole::Assistant,
            content: a.text.clone(),
            tool_calls: None,
        }),
        Some(M::ToolCall(call)) => {
            if let Some((name, args)) = summarize_tool_call_input(call) {
                out.push(OllamaChatMessage {
                    role: OllamaRole::Assistant,
                    content: String::new(),
                    tool_calls: Some(vec![OllamaOutboundToolCall {
                        function: OllamaOutboundToolCallFunction {
                            name,
                            arguments: args,
                        },
                    }]),
                });
            }
        }
        Some(M::ToolCallResult(result)) => out.push(OllamaChatMessage {
            role: OllamaRole::Tool,
            content: summarize_tool_result(result),
            tool_calls: None,
        }),
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}
```

Note: unlike Anthropic, we don't merge adjacent same-role messages (Ollama tolerates consecutive same-role). And tool_results stay on `role:"tool"` rather than getting folded into a user message.

- [ ] **Step 2.2: Sibling tests (~12)**

- System prompt is a `role:"system"` message at index 0.
- Simple user query yields system + user messages.
- Tool-call history becomes assistant message with `tool_calls[].function.arguments` as a JSON object (not a string).
- Tool result becomes a `role:"tool"` message.
- `options.num_ctx` set when `context_window` is Some, omitted otherwise.
- `tools` field absent when `supports_tools = false`.
- Tools advertised in Ollama shape (`type:"function"` wrapper, same as OpenAI — the wire envelope matches OpenAI for tools, only request body for tool_calls is different).
- `stream` is always `true`.
- AgentReasoning is dropped.
- Compaction projection synthesizes the head pair correctly.
- Synthetic user-query anchoring works.
- Multi-turn round-trip (similar to Phase 3a's test).

- [ ] **Step 2.3: Build + tests + commit**

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/ollama): request translator

Phase 3b stage A. compose_ollama_chat_request walks LocalProviderInput
and emits native /api/chat body shape: system as role:system message,
tool_calls with arguments as JSON object (not stringified), tool
results as role:tool messages, options.num_ctx threaded from
cfg.context_window. Reuses summarize_tool_call_input + summarize_tool_result
from Phase 3a (added there for adapter-agnostic reuse). Promotes
tools::schema_for to pub(crate) schema_for_pub so the Ollama tool
definitions builder can reuse the v1 schemas.
```

### Task 3: URL helpers

**File:** Modify `crates/ai/src/local_provider/config.rs`.

- [ ] **Step 3.1: Add `ollama_chat_url` + `ollama_tags_url`**

```rust
pub fn ollama_chat_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.join_ollama_path("api/chat")
}

pub fn ollama_tags_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.join_ollama_path("api/tags")
}

fn join_ollama_path(&self, leaf: &str) -> Result<Url, LocalProviderConfigError> {
    let mut base = Url::parse(&self.base_url)
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))?;
    if !base.path().ends_with('/') {
        let p = format!("{}/", base.path());
        base.set_path(&p);
    }
    base.join(leaf)
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))
}
```

- [ ] **Step 3.2: Tests (~4)**

- `ollama_chat_url_from_default_localhost` → `http://localhost:11434/api/chat`
- `ollama_chat_url_with_trailing_slash` (idempotent)
- `ollama_tags_url_from_default_localhost` → `http://localhost:11434/api/tags`
- `ollama_chat_url_works_with_relay_base_path` (e.g. `https://relay.example.com/ollama`)

- [ ] **Step 3.3: Commit**

```
feat(ai/local_provider/config): ollama_chat_url + ollama_tags_url

Phase 3b stage A. {base_url}/api/chat for the native chat endpoint
and {base_url}/api/tags for the test-connection probe. No /v1
idempotency dance — Ollama's paths are unambiguous.
```

---

## Stage B: NDJSON decoder + `streaming_format` trait extension

### Task 4: Add `StreamingFormat` enum + trait method

**File:** Modify `crates/ai/src/local_provider/adapters/mod.rs`.

- [ ] **Step 4.1: Add the enum + default trait method**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingFormat {
    ServerSentEvents,
    NewlineDelimitedJson,
}

pub trait ProviderAdapter: Send + Sync {
    // ... existing methods ...

    fn streaming_format(&self) -> StreamingFormat {
        StreamingFormat::ServerSentEvents
    }
}
```

OpenAi and Anthropic adapters inherit the default — no changes required to their impls.

- [ ] **Step 4.2: Build + commit**

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
```

Commit:

```
refactor(ai/local_provider/adapters): add streaming_format trait method

Phase 3b stage B. Adds StreamingFormat enum (SSE | NDJSON) and a
streaming_format() trait method with default returning SSE so
existing adapters (OpenAi, Anthropic) keep their current behavior.
The Ollama adapter (next commit) overrides to NDJSON; the runner
branches on this in synthesize_stream (Task 6).
```

### Task 5: `OllamaDecoder`

**Files:**
- Create `crates/ai/src/local_provider/adapters/ollama/response.rs`.
- Create `crates/ai/src/local_provider/adapters/ollama/response_tests.rs`.

- [ ] **Step 5.1: Decoder impl**

Mirror `OpenAiSseAdapter`'s public surface — same `with_ids` / `new` / `skip_create_task` / `is_terminal` / `record_upstream_error` / `feed_event` / `finish`. Internal state matches the design refinement above.

The bulk of the work is the per-chunk dispatch in `feed_event`:

1. Skip empty data lines.
2. Lazily emit Init + BeginTransaction + CreateTask on first non-empty feed.
3. Deserialize `OllamaChatChunk`; on parse error, transition to Errored.
4. If `chunk.error` is set, record + Errored.
5. If `chunk.message.content` is non-empty, open/append the shared AgentOutput message.
6. If `chunk.message.tool_calls` is set, emit one AddMessagesToTask{ToolCall} per entry (synthesize a UUID id).
7. If `chunk.done: true`, capture `done_reason` + usage; transition to Done.

The `finish()` method:
- Emits the prelude if not yet (shouldn't happen — `feed_event` would have).
- Emits CommitTransaction (if healthy: `state == Done && done_reason captured`) or Rollback.
- Builds `TokenUsage` from `captured_input_tokens` + `captured_output_tokens` (model_id from `captured_model`, falling back to `"ollama"`).
- Emits `Finished{reason, token_usage}`.

`map_ollama_done_reason("stop")` → `Done`; `"length"` → `MaxTokenLimit`; everything else → `Other`.

Shared helpers (`client_action_*`, `build_kind_message`, `build_client_action_event`, `build_tool_call_event`, `internal_error_reason`) — duplicate from Phase 3a's `anthropic/response.rs` or factor into a shared `adapters/proto_helpers.rs` module. **Recommendation:** factor out during this task — three adapters now use the same helpers (OpenAi inline, Anthropic in its mod, Ollama). Net negative line count.

- [ ] **Step 5.2: Sibling tests (~18)**

- Prelude emitted on first feed.
- `with_ids` round-trips in Init.
- `skip_create_task` suppresses CreateTask.
- Simple text streaming over multiple chunks builds the canonical event sequence (Init + Begin + Create + AddMessages(text "Hello") + Append(" world") + Commit + Finished{Done}).
- Tool call in one chunk emits one AddMessages{ToolCall}.
- Multiple tool calls in one chunk emit multiple events.
- Tool call followed by `done: true` emits the tool then transitions to Done.
- `done_reason: "length"` maps to MaxTokenLimit.
- `done_reason: "stop"` maps to Done.
- Unknown `done_reason` maps to Other.
- Top-level `error` field surfaces as InternalError on finish.
- Malformed JSON chunk transitions to Errored.
- Premature EOF (no `done: true`) → Rollback + InternalError("stream ended").
- `record_upstream_error` surfaces in finish when no done_reason.
- Usage from final chunk (`prompt_eval_count`, `eval_count`) merged into TokenUsage.
- Empty `message.content` is silently skipped (no spurious Append events).
- Terminal-state safety: post-Done feeds are no-ops.
- AgentReasoning is not emitted (Ollama has no reasoning channel today; `<think>` parsing is Phase 4 polish).

- [ ] **Step 5.3: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/ollama): NDJSON decoder

Phase 3b stage B. OllamaDecoder consumes the /api/chat NDJSON stream
(one OllamaChatChunk per line) and emits the canonical ResponseEvent
shape. Text streams as a single shared AgentOutput message (matches
OpenAi's pattern, not Anthropic's per-block). Tool calls arrive
complete in one chunk; synthesize a UUID id since Ollama doesn't
send one. done:true is the terminator (no [DONE], no message_stop).
done_reason maps to the same Reason variants OpenAi and Anthropic
use.
```

---

## Stage C: Runner NDJSON branch + adapter impl + dispatch flip

### Task 6: NDJSON drive loop in `run.rs`

**File:** Modify `crates/ai/src/local_provider/run.rs`.

- [ ] **Step 6.1: Rename existing SSE body**

Move the existing `synthesize_stream` body into a new private function `synthesize_sse_stream` with the same signature. Keep the function logic byte-identical — just renamed.

- [ ] **Step 6.2: Add `synthesize_ndjson_stream`**

Implementation per the design refinement above. ~80 lines. Key correctness points:

- HTTP status check before driving the byte stream; 4xx/5xx → read body, record_upstream_error, decoder.finish() with Rollback.
- Buffer accumulation pattern: drain complete lines (terminated by `\n`) on each poll cycle before pulling more bytes.
- Empty-line skip (defensive).
- `decoder.is_terminal()` after each line: emit `decoder.finish()`, set `closed = true`.
- EOF without `done: true`: drain unterminated final line if present, then `decoder.finish()` (which emits Rollback + InternalError per the decoder's contract).
- Cancellation: poll `cancel_rx` alongside the byte stream. On cancel, `decoder.finish()` emits Rollback.
- Network errors mid-stream: `record_upstream_error("network error: {e}")` then finish.

- [ ] **Step 6.3: `synthesize_stream` becomes a dispatcher**

```rust
fn synthesize_stream(
    decoder: Box<dyn StreamDecoder>,
    request_builder: reqwest::RequestBuilder,
    streaming_format: StreamingFormat,
    cancel_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = api::ResponseEvent> + Send {
    match streaming_format {
        StreamingFormat::ServerSentEvents => {
            // Build the EventSource; lift retry-policy reset to here.
            let mut event_source = request_builder
                .eventsource()
                .expect("eventsource() on a fresh single-use RequestBuilder cannot fail");
            event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));
            synthesize_sse_stream(decoder, event_source, cancel_rx).left_stream()
        }
        StreamingFormat::NewlineDelimitedJson => {
            synthesize_ndjson_stream(decoder, request_builder, cancel_rx).right_stream()
        }
    }
}
```

This signature change ripples into `run_chat_turn`: the caller passes the `RequestBuilder` (instead of just the EventSource) plus the streaming-format selector. Adjust accordingly.

- [ ] **Step 6.4: Build + tests + commit**

The existing OpenAi and Anthropic integration tests should still pass — the SSE branch is byte-identical to the prior synthesize_stream.

```bash
cargo nextest run -p ai 2>&1 | tail -3       # all 426 prior tests + new ones
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
refactor(ai/local_provider/run): branch synthesize_stream on streaming_format

Phase 3b stage C. The existing SSE drive loop moves verbatim into
synthesize_sse_stream; a new synthesize_ndjson_stream drives
response.bytes_stream() through a buffer-and-split-on-\n line
splitter, calling decoder.feed_event(None, line) per complete line
and honoring decoder.is_terminal() as the early-exit. synthesize_stream
becomes a one-line match on adapter.streaming_format(). OpenAi and
Anthropic tests stay green — their SSE path is unchanged.
```

### Task 7: `OllamaAdapter` impl + flip `select_adapter`

**Files:**
- Create `crates/ai/src/local_provider/adapters/ollama/mod.rs`.
- Modify `crates/ai/src/local_provider/adapters/mod.rs`.
- Modify `crates/ai/src/local_provider/adapters/adapters_tests.rs`.

- [ ] **Step 7.1: `OllamaAdapter` impl**

```rust
pub struct OllamaAdapter;

impl ProviderAdapter for OllamaAdapter {
    fn api_type(&self) -> AgentProviderApiType { AgentProviderApiType::Ollama }

    fn streaming_format(&self) -> StreamingFormat { StreamingFormat::NewlineDelimitedJson }

    fn build_chat_request(&self, input, cfg, http) -> ... {
        cfg.validate()?;
        let url = cfg.ollama_chat_url()?;
        let body = compose_ollama_chat_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        let mut rb = http.post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/x-ndjson")
            .body(body_json);
        if let Some(k) = cfg.api_key.as_deref().filter(|s| !s.is_empty()) {
            rb = rb.bearer_auth(k);
        }
        Ok(rb)
    }

    fn create_stream_decoder(&self, ids, skip_create_task) -> ... {
        let mut decoder = match ids { ... };
        if skip_create_task { decoder.skip_create_task(); }
        Box::new(decoder)
    }

    fn build_summarizer_request(&self, input, cfg, http) -> ... {
        cfg.validate()?;
        let url = cfg.ollama_chat_url()?;
        let body = build_ollama_summarizer_body(input, cfg);   // stream:false
        let body_json = serde_json::to_string(&body)?;
        let mut rb = http.post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body_json);
        if let Some(k) = cfg.api_key.as_deref().filter(|s| !s.is_empty()) {
            rb = rb.bearer_auth(k);
        }
        Ok(rb)
    }

    fn parse_summarizer_response(&self, body) -> ... {
        // Ollama's non-streaming /api/chat returns ONE OllamaChatChunk with
        // done:true. Parse, extract message.content.
        let parsed: OllamaChatChunk = serde_json::from_str(body)
            .map_err(|e| SummarizerError::DecodeResponse(format!("{e}: {}",
                crate::local_provider::run::first_chars(body, 200))))?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(err));
        }
        let text = parsed.message.map(|m| m.content).unwrap_or_default();
        let trimmed = text.trim();
        if trimmed.is_empty() { Err(SummarizerError::NoContent) }
        else { Ok(trimmed.to_string()) }
    }

    fn build_probe_request(&self, cfg, http) -> ... {
        cfg.validate()?;
        let url = cfg.ollama_tags_url()?;
        let mut rb = http.get(url);
        if let Some(k) = cfg.api_key.as_deref().filter(|s| !s.is_empty()) {
            rb = rb.bearer_auth(k);
        }
        Ok(rb)
    }
}

fn build_ollama_summarizer_body(input: &SummarizerInput, cfg: &LocalProviderConfig)
    -> OllamaChatRequest
{
    // Map OpenAI-shaped ChatMessage list to Ollama messages. System lifts to
    // role:system; user/assistant pass through; tool messages drop (compaction
    // never emits them).
    let messages = input.messages.iter().filter_map(|msg| { ... }).collect();
    OllamaChatRequest {
        model: cfg.model_id.clone(),
        stream: false,
        messages,
        tools: None,
        options: cfg.context_window.filter(|n| *n > 0).map(|num_ctx| OllamaOptions {
            num_ctx: Some(num_ctx),
        }),
    }
}
```

- [ ] **Step 7.2: Adapter tests (~8) — add to `request_tests.rs`**

- Chat URL is `{base_url}/api/chat` with Bearer (when key set) + Accept `application/x-ndjson`.
- Chat omits Bearer when key absent.
- Chat URL handles trailing-slash base correctly.
- Summarizer URL same as chat, Accept `application/json`, `stream: false`.
- Probe URL is `{base_url}/api/tags`.
- `parse_summarizer_response` extracts `message.content`.
- `parse_summarizer_response` empty → `NoContent`.
- `parse_summarizer_response` top-level error → `UpstreamErrorEnvelope`.

- [ ] **Step 7.3: Flip `select_adapter` + update `adapters_tests.rs`**

```rust
pub fn select_adapter(api_type: AgentProviderApiType)
    -> Result<Box<dyn ProviderAdapter>, AdapterError>
{
    use AgentProviderApiType::*;
    match api_type {
        OpenAi    => Ok(Box::new(OpenAiAdapter)),
        Anthropic => Ok(Box::new(AnthropicAdapter)),
        Ollama    => Ok(Box::new(OllamaAdapter)),
        OpenAiResp | Gemini | DeepSeek => Err(AdapterError::UnsupportedApiType(api_type)),
    }
}
```

In `adapters_tests.rs`:
- Add `select_adapter_returns_ollama_for_ollama_api_type`.
- Remove `Ollama` from the `select_adapter_errors_for_each_unimplemented_variant` loop.

- [ ] **Step 7.4: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3       # ~470 tests total
cargo nextest run -p warp --lib 2>&1 | tail -3
cargo clippy -p ai -p warp --lib --tests -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/ollama): implement OllamaAdapter + dispatch flip

Phase 3b stage C. Ollama api_type now dispatches to OllamaAdapter
instead of returning UnsupportedApiType. Chat targets
{base_url}/api/chat (NDJSON streaming via streaming_format()
override). Summarizer reuses the same endpoint with stream:false.
Probe targets {base_url}/api/tags. Optional Bearer auth — most
Ollama instances are unauthed; hosted relays may require it.
```

---

## Stage D: Manual smoke + docs

### Task 8: Live test + spec docs

- [ ] **Step 8.1: Live test against local `ollama serve`**

Documented checklist — requires a working Ollama install:

1. Run `ollama serve` (or have one running on `localhost:11434`).
2. Pull a tool-using model: `ollama pull llama3.1` (or `qwen2.5-coder`, `mistral-nemo`).
3. In the app, Settings → AI → Custom AI Providers, add provider:
   - Name: `Ollama (local)`
   - API type: `Ollama`
   - Base URL: `http://localhost:11434`
   - API key: leave blank
   - Model id: `llama3.1` (one row; context window 128000; tool_call enabled).
4. Click **Test connection** — expect green check.
5. Open a new conversation, pick `Ollama (local) / llama3.1`.
6. Send: `Run the command "echo hello from ollama" and report the output.` — expect streamed assistant text, a `run_shell_command` tool call, the tool result rendered back, and a final summary.
7. Send a multi-turn follow-up; confirm history threads correctly.
8. Edit base URL to `http://localhost:1` (wrong port), click Test connection — expect red failure with a connection-refused message.

- [ ] **Step 8.2: Update spec docs**

- README:
  - Phase 3b row: ✅ shipped (or 🧪 code complete pending smoke) with date.
  - "What landed" Architecture section: add the Ollama adapter file paths + the `streaming_format` trait addition.
  - "What landed" User-visible section: add the Ollama option as a real api_type with native streaming + `options.num_ctx` knob.
  - "Future phases" section: drop the Ollama entry; remaining work is 3c (Gemini), 3d (DeepSeek), 4a-d (polish).
- design.md §9 phase table: mark Phase 3b row ✅ shipped.

- [ ] **Step 8.3: Commit**

```
docs(specs/multi-local-llm): mark Phase 3b (Ollama adapter) shipped
```

---

## Final verification

- [ ] **Verification 1: Sweeps**

```bash
echo "=== Ollama submodule wired ==="
grep -n "pub mod ollama" crates/ai/src/local_provider/adapters/mod.rs

echo "=== select_adapter flipped for Ollama ==="
grep -nA 1 "Ollama =>" crates/ai/src/local_provider/adapters/mod.rs

echo "=== streaming_format on trait ==="
grep -n "fn streaming_format\|enum StreamingFormat" crates/ai/src/local_provider/adapters/mod.rs

echo "=== OllamaAdapter overrides streaming_format ==="
grep -n "NewlineDelimitedJson" crates/ai/src/local_provider/adapters/ollama/mod.rs

echo "=== NDJSON drive loop ==="
grep -n "fn synthesize_ndjson_stream\|fn synthesize_sse_stream" crates/ai/src/local_provider/run.rs

echo "=== Ollama URL helpers ==="
grep -n "fn ollama_chat_url\|fn ollama_tags_url" crates/ai/src/local_provider/config.rs
```

- [ ] **Verification 2: Build + tests + clippy**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -5     # ~470 tests (426 + ~45 Ollama)
cargo nextest run -p warp --lib 2>&1 | tail -5
cargo clippy -p ai --all-targets --all-features -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

- [ ] **Verification 3: Manual smoke**

Per Task 8.1 — real `ollama serve` instance with a tool-using model.

- [ ] **Verification 4: Final reviewer + push**

Dispatch `oh-my-claudecode:code-reviewer` for the full Phase 3b diff. Stop before push; user reviews, then pushes manually.

---

## Risks & open questions

1. **NDJSON line splitter correctness on chunked-transfer responses.** `response.bytes_stream()` yields HTTP chunks, which may split JSON lines arbitrarily. The buffer-and-split-on-`\n` pattern handles this correctly as long as we *accumulate* across chunks and don't try to parse partial lines. Mitigated by drain-complete-lines-before-pulling-more-bytes loop structure. Unit-test the line splitter with a stream of bytes pre-split at non-`\n` boundaries.

2. **`options.num_ctx` cost.** Setting `num_ctx` larger than the user's RAM allows causes Ollama to fall back to disk-backed inference (extremely slow) or OOM-kill the model. The translator just passes `cfg.context_window` through — if a user pastes `200000` for a model that won't fit, Ollama handles it. Phase 4 polish could expose a `max_num_ctx` warning in the settings UI when the configured value exceeds available memory.

3. **Tool-call id stability across turns.** Ollama doesn't send ids, so the decoder synthesizes UUIDs. If the model regenerates the same tool call across retries (e.g. after a tool-result error), each gets a fresh id. The controller's `action_results` map is keyed by id, so prior results don't conflict — but the model can't reference a prior call by id. This matches the existing OpenAi behavior where ids are model-generated and may also change. Acceptable.

4. **Streaming-format dispatch in `run.rs` requires threading `RequestBuilder` into `synthesize_stream`.** Today the SSE path builds the `EventSource` inside `run_chat_turn` and threads that into `synthesize_stream`. After the refactor, `synthesize_stream` receives the raw `RequestBuilder` and decides whether to build an `EventSource` (SSE) or call `.send()` (NDJSON). Slight signature churn; documented in Task 6.

5. **Ollama-native body shape vs OpenAI-compat at `/v1/chat/completions`.** Users who previously used Ollama via the OpenAI-compat layer (api_type=OpenAi pointing at `http://localhost:11434/v1`) keep working — that path goes through OpenAiAdapter. Phase 3b adds a *second* way to talk to Ollama (native, via api_type=Ollama). Both coexist. No migration; users opt in to native when they create or edit a provider entry. Document this in the README so users understand the choice.

6. **`<think>...</think>` content parsing for DeepSeek-R1-distilled Ollama models.** These models embed reasoning text inline in `message.content`. Phase 3b renders it as visible AgentOutput text (no special handling). Phase 4 polish can add `<think>` tag parsing to split it into AgentReasoning. Documented as out-of-scope here.

7. **No live test in CI.** Same gate as Phase 3a — manual smoke. A future integration test using a mock NDJSON server (similar to `crates/ai/tests/local_provider_integration.rs` for OpenAI) would unblock CI coverage; deferred to Phase 4.

8. **`tool_definitions_ollama` reuses the v1 schemas via `schema_for_pub`.** Ollama's tool definitions look identical to OpenAI's (`{type:"function", function:{...}}`). The only thing keeping them separate is the type — `Vec<OllamaToolDef>` vs `Vec<ToolDefinition>`. The two structs are wire-equivalent, but the typed separation prevents accidentally sending OpenAI's `ToolDefinition` through a serde path expecting Ollama's. Minor duplication; deliberate.

---

## Next plan (Phase 3c — Gemini adapter)

After Phase 3b ships green, Phase 3c targets Gemini. Wire format: SSE (so no `streaming_format` override needed — Phase 3b's trait addition unblocks it). Endpoint: `POST {base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse`. Auth: `x-goog-api-key` header or `?key=` query param. Body shape: `{contents: [{role, parts: [{text} | {functionCall: {name, args}} | {functionResponse: {...}}]}]}` — Gemini uses content "parts" inside messages, similar to Anthropic but with different vocabulary. Plan written after 3b is approved + executed.
