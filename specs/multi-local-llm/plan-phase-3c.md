# Multi-Local-LLM — Phase 3c (Gemini-native Adapter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a native `GeminiAdapter` targeting `POST {base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse` and flip `select_adapter` for `AgentProviderApiType::Gemini` from `Err(UnsupportedApiType)` to a real impl. **First SSE adapter since Anthropic** — `streaming_format()` inherits the SSE default; no `run.rs` changes. Unblocks users who want to talk to Google's Gemini API directly (the `generativelanguage.googleapis.com` endpoint doesn't speak OpenAI-compat, so the existing `OpenAi` api_type path can't reach it).

**Architecture:** Four logical stages, atomic in one PR (split into 3c-i / 3c-ii / 3c-iii / 3c-iv if review prefers):

- **Stage A (Tasks 1–3)** — Wire types, request translator, URL helpers. Mirrors Phase 3b's submodule layout. Reuses `summarize_tool_call_input` + `summarize_tool_result` already promoted by Phase 3a for adapter-agnostic proto→JSON-Value conversion.
- **Stage B (Task 4)** — `GeminiSseDecoder` consumes anonymous `data:` chunks (no SSE `event:` names — same SSE shape as OpenAI) and emits the canonical `ResponseEvent` shape. No trait extensions needed — `feed_event(None, data)` covers it.
- **Stage C (Tasks 5–6)** — `GeminiAdapter` impl + flip `select_adapter`. The existing `synthesize_sse_stream` in `run.rs` drives the stream unchanged.
- **Stage D (Task 7)** — Manual smoke against `generativelanguage.googleapis.com` with a real `AIza…` API key, spec doc updates.

**Branch:** `multi-local-llm`. Forks from `3f0f2b30` (Phase 3b code-complete tip). Estimated ~700 lines net code (~110 wire types + 260 translator + 230 decoder + 60 adapter glue + 40 URL helpers), ~5 hours of subagent-driven work.

**Spec references:**
- `specs/multi-local-llm/design.md` §9 (Phase 3c row — gets the "code complete" status flip in Task 7).
- `specs/multi-local-llm/plan-phase-3b.md` §"Next plan (Phase 3c — Gemini adapter)" — superseded by this document.
- Gemini API docs: <https://ai.google.dev/api/generate-content#method:-models.streamgeneratecontent>.
- SSE mode: <https://ai.google.dev/gemini-api/docs/text-generation#stream> (the `?alt=sse` query is required — without it the endpoint returns a JSON array, not SSE).

**Test gate:** All existing `cargo nextest run -p ai` tests pass (496/496 baseline from Phase 3b); new Gemini-specific tests added (~50). Manual smoke: a single Gemini provider configured with `base_url = https://generativelanguage.googleapis.com`, api_type `Gemini`, runs a turn that streams assistant text, fires a tool call, gets a result, and emits a final assistant message. "Test connection" probe succeeds.

**Out of Phase 3c (deferred):**
- DeepSeek adapter — Phase 3d.
- `reasoning` channel (Gemini 2.5 Flash/Pro thinking mode via `thinkingConfig` + the `thought:true` part flag) — Phase 4 polish.
- Multimodal `inlineData` parts (image / pdf / audio) — Phase 4c.
- `generationConfig.maxOutputTokens` user-tunable knob — Phase 4 polish (we omit it; Gemini's default is generous).
- Vertex AI dispatch (`{endpoint}/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:streamGenerateContent`) — distinct auth flow (Google OAuth2 service account), separate api_type at most.
- Safety-rating surfacing — Phase 4 polish.
- Code-execution / Google-Search built-in tools — out of scope (we expose our v1 tool set only).

---

## Design refinement

### Endpoint + auth

- **Chat (streaming):** `POST {base_url}/v1beta/models/{model_id}:streamGenerateContent?alt=sse`.
  - The `?alt=sse` query parameter is **mandatory** for SSE framing. Without it Gemini returns a JSON array (`Content-Type: application/json`) — fine for non-streaming, useless for incremental UX. Always set it on the streaming path.
- **Chat (non-streaming, used by the summarizer):** `POST {base_url}/v1beta/models/{model_id}:generateContent`.
  - Same body shape, but the response is a single `GenerateContentResponse` JSON object (not a stream).
- **Probe:** `GET {base_url}/v1beta/models` — Gemini's model-list endpoint. Returns 200 + JSON when `x-goog-api-key` is valid; 401 / 403 when not. No model load.
- **Auth:** `x-goog-api-key: <key>` request header. Gemini also accepts `?key=<key>` as a query parameter, but the header form keeps the key out of access logs / URL-bar telemetry. **The API key is required**; Gemini has no anonymous path (unlike Ollama). If `cfg.api_key` is `None` or empty the request 401s — the adapter sends the header empty rather than synthesizing an error, so the upstream error surfaces normally through the SSE error path.
- **Default `base_url`:** `https://generativelanguage.googleapis.com` (no `/v1beta` prefix — the URL helper adds it idempotently). Users with a Vertex relay or a Gateway in front can paste a different host; the path-join logic handles a trailing `/v1beta` already in the base.

### Streaming format: SSE (anonymous chunks)

**Same SSE shape as OpenAI**: each `data: {...}` line is a `GenerateContentResponse` JSON object; no `event:` discriminator. Each chunk is a complete *delta* of a partial response — `candidates[0].content.parts[0].text` accumulates, `functionCall` parts arrive complete in one chunk.

There is **no `[DONE]` terminator**. The end of the stream is signaled by:
1. The final chunk's `candidates[0].finishReason` field being set (`"STOP"`, `"MAX_TOKENS"`, `"SAFETY"`, etc.) — same logical position as Anthropic's `message_stop`.
2. The SSE event source emitting EOF after the final chunk.

The decoder transitions to `State::Done` when it observes a `finishReason`; `is_terminal()` then returns `true`, which signals `synthesize_sse_stream` to call `finish()`. This is the same flow Anthropic's decoder uses — Anthropic just emits an explicit `event: message_stop` payload, where Gemini embeds the equivalent signal inside the last data chunk. No `streaming_format()` override; the existing SSE drive loop in `run.rs` handles both cases.

### Request body shape (native Gemini)

```jsonc
{
  "systemInstruction": {
    "parts": [{"text": "You are a helpful assistant. ..."}]
  },
  "contents": [
    {"role": "user",
     "parts": [{"text": "Read Cargo.toml."}]},
    {"role": "model",
     "parts": [
       {"text": "I'll read it."},
       {"functionCall": {"name": "read_files", "args": {"paths": ["Cargo.toml"]}}}
     ]},
    {"role": "user",
     "parts": [
       {"functionResponse": {
         "name": "read_files",
         "response": {"content": "...rendered tool result..."}
       }}
     ]}
  ],
  "tools": [
    {"functionDeclarations": [
      {"name": "read_files",
       "description": "...",
       "parameters": {"type":"object","properties": {...}}}
    ]}
  ],
  "generationConfig": {}
}
```

**Differences from OpenAI / Anthropic / Ollama at the wire level:**

1. **Role vocabulary:** `"user"` and `"model"` (not `"assistant"`). The translator emits this mapping.
2. **No `role: "system"` in contents.** Gemini rejects system messages in the `contents` array. The synthesized system prompt lifts to the top-level `systemInstruction.parts[0].text`. Empty system prompts omit the field entirely.
3. **`contents[].parts[]` is an array of typed parts.** Each part is **exactly one of** `{text}`, `{functionCall}`, `{functionResponse}`, `{inlineData}` (multimodal — Phase 4c). Within one message we may concatenate multiple parts (e.g. an assistant message with a text preamble followed by a `functionCall`).
4. **Tool calls are messages with `model` role and a `functionCall` part** — not a separate message kind. `functionCall.args` is a **JSON OBJECT**, not a stringified-JSON string (same as Ollama; opposite of OpenAI).
5. **Tool results are messages with `user` role and a `functionResponse` part.** `functionResponse.response` is a JSON object whose schema is free-form; we wrap the rendered text under a single `{content: "..."}` key (Gemini's docs recommend an object so future structured tool results can land here without a wire change).
6. **No tool-call ids.** Gemini doesn't emit or accept tool-call ids; `functionResponse` matches to the prior `functionCall` by **`name` only**. The translator omits ids during proto→Gemini conversion; the decoder synthesizes a UUID id on each emitted `ToolCall` proto (same as Ollama) so the controller's `action_results` map continues to key by id internally. The translator silently drops that id when re-encoding tool results back to Gemini.
7. **Tool definitions live under a single `tools[0].functionDeclarations` array.** `functionDeclarations[i]` shape: `{name, description, parameters}` — same fields as OpenAI's `function:{...}` body, no `type:"function"` wrapper. We emit exactly one `tools` entry containing all v1 declarations.
8. **No `max_tokens` requirement.** Gemini's `generationConfig.maxOutputTokens` is optional; we omit it (Phase 4 polish wires it from a future per-model field). `generationConfig` itself is also optional, but we always emit `{}` so the body shape stays stable for snapshot tests (Gemini tolerates either way).
9. **Model lives in the URL path**, not in the body. `compose_gemini_request` does not include a `model` field; the URL helper takes `cfg.model_id` and builds `…/models/{model}:streamGenerateContent`.

### Streaming response shape

`Content-Type: text/event-stream`. Each SSE event is anonymous (`event:` line is absent or omitted) with a `data:` payload that's a complete `GenerateContentResponse`:

```jsonc
// Streaming text fragment:
data: {"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]},"index":0}]}

// Continued text:
data: {"candidates":[{"content":{"role":"model","parts":[{"text":" world"}]},"index":0}]}

// Tool call arrives complete in one chunk:
data: {"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_files","args":{"paths":["Cargo.toml"]}}}]},"index":0}]}

// Final chunk: finishReason + usageMetadata.
data: {"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":50,"candidatesTokenCount":120,"totalTokenCount":170}}
```

**Key wire-format quirks:**

- `candidates` is a list; we only consume `candidates[0]`. Gemini supports `candidateCount > 1` (parallel sampling) but our `generationConfig` always leaves it default (1). If a chunk arrives with `candidates.len() > 1` we still take index 0; this matches Anthropic / OpenAI / Ollama.
- `candidates[0].content.parts` may be empty on the final chunk (the model's text already streamed; the last chunk only carries `finishReason` + usage). The decoder handles this without emitting spurious empty `Append` events.
- `finishReason` is the terminator signal. Values: `"STOP"` / `"MAX_TOKENS"` / `"SAFETY"` / `"RECITATION"` / `"OTHER"` / `"MALFORMED_FUNCTION_CALL"` / `"LANGUAGE"` / `"BLOCKLIST"` / `"PROHIBITED_CONTENT"` / `"SPII"`. Mapping below.
- `usageMetadata` is **typically** on the final chunk but can appear on any chunk (some Gemini revisions emit it on the first chunk too). We accumulate; the last seen wins.
- **Tool calls don't fragment.** Gemini emits each `functionCall` as a single complete part inside one chunk — unlike OpenAI / Anthropic which stream args incrementally. The decoder doesn't need an args-accumulator state machine.
- **Text fragments don't carry an index.** Gemini doesn't number content blocks; the decoder treats all incoming text as appends to a single shared `AgentOutput` message (same pattern as OpenAI / Ollama; not the per-block model used by Anthropic).
- **SSE error envelope** (4xx with a JSON body — Gemini surfaces these mid-handshake before the SSE stream opens):

```jsonc
{"error":{"code":400,"message":"...","status":"INVALID_ARGUMENT","details":[...]}}
```

  This shows up as the HTTP response body when `request_builder.send().await` returns a non-2xx status; the existing SSE error path in `synthesize_sse_stream` already reads the body and calls `record_upstream_error`. We just need a small text extractor (`extract_gemini_error_text`) that pulls `.error.message` out of the body for nicer messaging — same idea as Anthropic's `AnthropicErrorEnvelope`.

### Decoder design

Architecturally most similar to OpenAI / Ollama (one shared `AgentOutput` message across the turn — no per-block state). The state machine:

```rust
pub struct GeminiSseDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,
    /// One shared open AgentOutput message across the turn — Gemini's text
    /// stream is a single conceptual block per turn.
    text_message_id: Option<String>,
    captured_finish_reason: Option<String>,
    captured_model: Option<String>,
    captured_input_tokens: u64,
    captured_output_tokens: u64,
}

enum State { Streaming, Done, Errored }
```

Per-chunk dispatch (called via the default `feed` path — Gemini's SSE has no `event:` names):

```rust
fn feed_event(&mut self, _event_name: Option<&str>, data: &str)
    -> Vec<api::ResponseEvent>
{
    if matches!(self.state, State::Done | State::Errored) { return vec![]; }
    let trimmed = data.trim();
    if trimmed.is_empty() { return vec![]; }

    let mut out = self.ensure_prelude();
    let chunk: GeminiStreamChunk = match serde_json::from_str(trimmed) {
        Ok(c) => c,
        Err(e) => {
            self.state = State::Errored;
            self.upstream_error.get_or_insert_with(|| format!("malformed Gemini chunk: {e}"));
            return out;
        }
    };

    // Top-level error envelope — rare in SSE stream (more common pre-stream
    // as a 4xx body). Surface as upstream_error + Errored.
    if let Some(err) = chunk.error {
        self.upstream_error = Some(format!("{}: {}", err.status, err.message));
        self.state = State::Errored;
        return out;
    }

    if let Some(usage) = chunk.usage_metadata {
        self.captured_input_tokens = usage.prompt_token_count.max(self.captured_input_tokens);
        self.captured_output_tokens = usage.candidates_token_count.max(self.captured_output_tokens);
    }

    if let Some(candidate) = chunk.candidates.into_iter().next() {
        if self.captured_model.is_none() {
            // Gemini doesn't echo the model in stream chunks; rely on
            // cfg.model_id at finish() time.
        }
        if let Some(content) = candidate.content {
            for part in content.parts {
                self.handle_part(part, &mut out);
            }
        }
        if let Some(reason) = candidate.finish_reason {
            self.captured_finish_reason = Some(reason);
            self.state = State::Done;
        }
    }

    out
}

fn handle_part(&mut self, part: GeminiInboundPart, out: &mut Vec<api::ResponseEvent>) {
    match part {
        GeminiInboundPart::Text { text } => {
            if !text.is_empty() {
                self.append_text(&text, out);
            }
        }
        GeminiInboundPart::FunctionCall { function_call } => {
            self.emit_function_call(&function_call, out);
        }
        // FunctionResponse and InlineData parts are output-only on the model
        // side — never emitted by the API in streaming responses. Tolerate
        // by ignoring rather than erroring.
        GeminiInboundPart::FunctionResponse { .. } |
        GeminiInboundPart::InlineData { .. } |
        GeminiInboundPart::Unknown => {}
    }
}
```

**Tool-call id synthesis:** Gemini doesn't send tool-call ids. The decoder synthesizes one per call via `uuid::Uuid::new_v4()` (same approach Ollama uses). The synthesized id flows through the proto `Message::ToolCall.tool_call_id` field; the controller threads it back into `action_results` keyed by that id; the translator drops it when re-encoding the response as a Gemini `functionResponse` (which keys by `name` only).

**`finishReason` mapping:**

```rust
fn map_gemini_finish_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "STOP" => Reason::Done(Done {}),
        "MAX_TOKENS" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        // SAFETY / RECITATION / BLOCKLIST / PROHIBITED_CONTENT / SPII /
        // LANGUAGE / MALFORMED_FUNCTION_CALL / OTHER all surface as Other —
        // we don't yet have dedicated Reason variants for safety blocks
        // (Phase 4 polish can split SAFETY into a Refused variant the UI
        // can render distinctly).
        _ => Reason::Other(Other {}),
    }
}
```

### URL helpers

In `LocalProviderConfig`:

```rust
/// `{base_url}/v1beta/models/{model_id}:streamGenerateContent?alt=sse` —
/// Gemini's native streaming endpoint. The `?alt=sse` query is required
/// for SSE framing; without it Gemini returns a JSON array. Handles
/// `/v1beta` already present in the base path idempotently.
pub fn gemini_stream_generate_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.gemini_models_endpoint(&format!(
        "{}:streamGenerateContent?alt=sse",
        self.model_id
    ))
}

/// `{base_url}/v1beta/models/{model_id}:generateContent` — Gemini's
/// non-streaming endpoint. Used by the summarizer path.
pub fn gemini_generate_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.gemini_models_endpoint(&format!("{}:generateContent", self.model_id))
}

/// `{base_url}/v1beta/models` — Gemini's model-list endpoint, used by the
/// test-connection probe.
pub fn gemini_models_url(&self) -> Result<Url, LocalProviderConfigError> {
    self.gemini_endpoint("models")
}

fn gemini_models_endpoint(&self, leaf_after_models: &str) -> Result<Url, LocalProviderConfigError> {
    self.gemini_endpoint(&format!("models/{leaf_after_models}"))
}

fn gemini_endpoint(&self, leaf: &str) -> Result<Url, LocalProviderConfigError> {
    let mut base = Url::parse(&self.base_url)
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))?;
    if !base.path().ends_with('/') {
        let p = format!("{}/", base.path());
        base.set_path(&p);
    }
    let target: std::borrow::Cow<'_, str> = if base.path().ends_with("/v1beta/") {
        leaf.into()
    } else {
        format!("v1beta/{leaf}").into()
    };
    base.join(target.as_ref())
        .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))
}
```

**`url::Url::join` and the `:streamGenerateContent` suffix:** The `:` character inside a path segment is valid per RFC 3986 and `url::Url::join` treats it as part of the segment. We've verified this by tracing the same pattern through other crates; the existing Url-helper tests cover the round trip.

**Query-string handling:** `?alt=sse` is appended directly to the leaf string and gets parsed by `Url::join` as part of the URL. The `Url::query()` accessor sees it correctly.

### Adapter file structure

Mirrors Phase 3a / 3b:

```
crates/ai/src/local_provider/adapters/gemini/
├── mod.rs              # GeminiAdapter + ProviderAdapter trait impl
├── wire.rs             # Serde types for streamGenerateContent
├── request.rs          # compose_gemini_request
├── request_tests.rs
├── response.rs         # GeminiSseDecoder
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
        Ollama    => Ok(Box::new(OllamaAdapter)),
        Gemini    => Ok(Box::new(GeminiAdapter)),         // flipped in Phase 3c
        OpenAiResp | DeepSeek => Err(AdapterError::UnsupportedApiType(api_type)),
    }
}
```

The file-level doc comment on `adapters/mod.rs` (currently says "Phase 3a added Anthropic; Phase 3b adds Ollama-native. Gemini and DeepSeek remain Phase 3c/d work") gets updated to: "Phase 3a added Anthropic; Phase 3b added Ollama-native; Phase 3c adds Gemini. DeepSeek remains Phase 3d work."

### Settings UI

Same situation as Phase 3a / 3b — the widget already renders every `AgentProviderApiType` variant as a clickable chip via `EnumIter` without per-variant gating. No UI change needed; selecting `Gemini` now dispatches correctly instead of erroring with `UnsupportedApiType`.

---

## File map

**Files created:**
- `crates/ai/src/local_provider/adapters/gemini/mod.rs` — adapter impl.
- `crates/ai/src/local_provider/adapters/gemini/wire.rs` — serde types.
- `crates/ai/src/local_provider/adapters/gemini/request.rs` — translator.
- `crates/ai/src/local_provider/adapters/gemini/request_tests.rs` — sibling tests.
- `crates/ai/src/local_provider/adapters/gemini/response.rs` — SSE decoder.
- `crates/ai/src/local_provider/adapters/gemini/response_tests.rs` — sibling tests.

**Files modified:**
- `crates/ai/src/local_provider/adapters/mod.rs` — register `pub mod gemini;`, re-export `GeminiAdapter`, flip `select_adapter(Gemini)`, update file-level doc comment.
- `crates/ai/src/local_provider/adapters/adapters_tests.rs` — add `select_adapter_returns_gemini_for_gemini_api_type`; remove `Gemini` from the unimplemented-variants loop.
- `crates/ai/src/local_provider/config.rs` — `gemini_stream_generate_url()` + `gemini_generate_url()` + `gemini_models_url()` helpers and tests.

**Cargo deps:** none added.

**Files unchanged (importantly):**
- `crates/ai/src/local_provider/run.rs` — Gemini uses SSE; the existing `synthesize_sse_stream` drives it without modification.
- `crates/ai/src/local_provider/adapters/proto_helpers.rs` — Gemini reuses the existing helpers (`build_kind_message`, `client_action_*`, `build_tool_call_event`, `internal_error_reason`).

---

## Stage A: Wire types + request composition

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git log --oneline -1                     # 3f0f2b30 docs(specs/multi-local-llm): record Phase 3b code-complete status
cargo nextest run -p ai 2>&1 | tail -3   # 496 / 496 passed
```

If anything diverges, STOP and report.

### Task 1: Gemini wire types

**File:** Create `crates/ai/src/local_provider/adapters/gemini/wire.rs`.

- [ ] **Step 1.1: Request types**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct GeminiGenerateRequest {
    /// Top-level system prompt. Gemini does NOT accept system messages in
    /// the `contents` array; the translator lifts the synthesized prompt
    /// here. Omitted when empty.
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiSystemInstruction>,
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolEnvelope>>,
    /// Always emit (possibly empty) so the body shape stays stable for
    /// snapshot tests. Gemini tolerates `{}` and the absent form
    /// equivalently.
    #[serde(rename = "generationConfig")]
    pub generation_config: GeminiGenerationConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiSystemInstruction {
    pub parts: Vec<GeminiTextPart>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiContent {
    pub role: GeminiRole,
    pub parts: Vec<GeminiOutboundPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GeminiRole {
    User,
    Model,
}

/// Outbound (request-side) parts. Each variant serializes with exactly one
/// of `{text}`, `{functionCall}`, or `{functionResponse}` at the top level
/// (untagged enum so the serde shape matches Gemini's wire format).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum GeminiOutboundPart {
    Text(GeminiTextPart),
    FunctionCall(GeminiOutboundFunctionCallPart),
    FunctionResponse(GeminiOutboundFunctionResponsePart),
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiTextPart {
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionCallPart {
    #[serde(rename = "functionCall")]
    pub function_call: GeminiOutboundFunctionCall,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionCall {
    pub name: String,
    /// JSON object — Gemini's wire format expects an object here (same as
    /// Ollama; opposite of OpenAI's stringified-JSON-string convention).
    pub args: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionResponsePart {
    #[serde(rename = "functionResponse")]
    pub function_response: GeminiOutboundFunctionResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiOutboundFunctionResponse {
    pub name: String,
    /// Free-form JSON object. We always emit `{content: <string>}` for v1
    /// tool results; future structured tool outputs (Phase 4c) can land
    /// alongside without a wire change.
    pub response: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiToolEnvelope {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the function's input shape. Same shape as
    /// OpenAI's `function.parameters` — we reuse `tools::schema_for`
    /// directly (it's `pub(crate)` and reachable from the adapter module
    /// without a re-export).
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GeminiGenerationConfig {
    // Phase 4 polish wires `max_output_tokens`, `temperature`, `top_p`, etc.
    // Phase 3c emits an empty object; Gemini tolerates it.
}
```

- [ ] **Step 1.2: Streaming response types**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiStreamChunk {
    #[serde(default)]
    pub candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    pub usage_metadata: Option<GeminiUsageMetadata>,
    /// Top-level error envelope (rare in SSE stream; more common as the
    /// body of a 4xx pre-stream response). Surfaced via `record_upstream_error`.
    #[serde(default)]
    pub error: Option<GeminiErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiCandidate {
    #[serde(default)]
    pub content: Option<GeminiInboundContent>,
    #[serde(default, rename = "finishReason")]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiInboundContent {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub parts: Vec<GeminiInboundPart>,
}

/// Inbound (response-side) parts. Tagged manually via `#[serde(untagged)]`
/// because the wire form is "one of {text, functionCall, functionResponse,
/// inlineData}" with no discriminator field. `Unknown` is a catch-all so
/// future part types (multimodal, code execution) don't fail
/// deserialization.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GeminiInboundPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiInboundFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiInboundFunctionResponse,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: Value,
    },
    /// Catch-all for unknown part shapes — keeps unfamiliar payloads from
    /// erroring out mid-stream. Variants we recognize but ignore for now
    /// (e.g. thought / code-execution parts in 2.5 Pro thinking mode) also
    /// fall here. **NOTE:** `#[serde(other)]` does not compile on
    /// `#[serde(untagged)]` enums, so the catch-all is modeled as
    /// `Unknown(serde_json::Value)`. Serde's untagged dispatch tries the
    /// named variants top-to-bottom and falls through to `Value`, which
    /// always succeeds.
    Unknown(serde_json::Value),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiInboundFunctionCall {
    #[serde(default)]
    pub name: String,
    /// JSON object (Gemini's native shape). Default is an empty object.
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiInboundFunctionResponse {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub response: Value,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    pub prompt_token_count: u64,
    #[serde(default, rename = "candidatesTokenCount")]
    pub candidates_token_count: u64,
    #[serde(default, rename = "totalTokenCount")]
    pub total_token_count: u64,
    #[serde(default, rename = "cachedContentTokenCount")]
    pub cached_content_token_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GeminiErrorEnvelope {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: String,
}
```

- [ ] **Step 1.3: Non-streaming response (summarizer path)**

```rust
/// One-shot non-streaming `:generateContent` response — used by the
/// summarizer. Identical shape to a `GeminiStreamChunk` (the stream is
/// just incrementally-emitted instances of the same envelope), but the
/// non-streaming variant is decoded as a single value rather than line
/// by line.
pub type GeminiGenerateResponse = GeminiStreamChunk;
```

- [ ] **Step 1.4: Inline tests + commit**

Tests (~14) cover:

Request serialization:
- `serializes_minimal_text_request` — verifies role, parts[0].text, generation_config = {} present.
- `serializes_system_instruction_lifted_to_top_level` — confirms no role:system in contents.
- `omits_system_instruction_when_empty`.
- `serializes_model_role_for_assistant_messages`.
- `serializes_function_call_part_with_object_args` — confirms `{functionCall: {name, args: {...}}}` shape, args is an object not a string.
- `serializes_function_response_part_with_content_wrapper` — confirms `response: {content: "..."}`.
- `serializes_tool_envelope_with_function_declarations`.
- `omits_tools_when_empty`.

Streaming response deserialization (use samples from Google's docs):
- `deserializes_text_chunk` — `{candidates:[{content:{role:"model",parts:[{text:"Hello"}]}}]}`.
- `deserializes_function_call_chunk` — args is a JSON object.
- `deserializes_final_chunk_with_finish_reason_and_usage_metadata` — `finishReason: STOP`, usageMetadata fields parsed.
- `deserializes_chunk_with_empty_parts_array` — final chunk has no text/functionCall, just finishReason.
- `deserializes_unknown_part_variant_as_unknown` — forward-compat with future part types.
- `deserializes_error_envelope` — `{error: {code, message, status}}` body.

Commit:

```
feat(ai/local_provider/adapters/gemini): add wire types

Phase 3c stage A. Serde types for the native Gemini
:streamGenerateContent / :generateContent request (with
systemInstruction lifted to top level, contents[].parts as a typed
union of {text, functionCall, functionResponse}, tools wrapped in a
single functionDeclarations envelope, generationConfig always emitted
as {}) and streaming chunks (candidates[].content.parts, finishReason
as terminator signal, usageMetadata, error envelope, Unknown
catch-all on the inbound part union for forward-compat).
functionCall.args and functionResponse.response are JSON objects,
not strings — the Gemini wire-level shape divergence from OpenAI.
```

### Task 2: Request translator

**Files:**
- Create `crates/ai/src/local_provider/adapters/gemini/request.rs`.
- Create `crates/ai/src/local_provider/adapters/gemini/request_tests.rs`.

- [ ] **Step 2.1: `compose_gemini_request`**

Mirror the Ollama translator's high-level structure but emit Gemini-native shape:

```rust
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

pub fn compose_gemini_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> GeminiGenerateRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(vec![GeminiToolEnvelope {
            function_declarations: function_declarations_gemini(&local_tools),
        }])
    } else {
        None
    };

    // System prompt lifts to top-level systemInstruction.
    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system_text = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );
    let system_instruction = if system_text.trim().is_empty() {
        None
    } else {
        Some(GeminiSystemInstruction {
            parts: vec![GeminiTextPart { text: system_text }],
        })
    };

    let mut contents: Vec<GeminiContent> = Vec::new();

    // Compaction projection — same as OpenAI / Anthropic / Ollama translators.
    // Synthesize the user "Continue..." + model <summary> pair when
    // compaction_state has a completed entry; skip pre-tail history per
    // the projection's tail_start_id.
    // ... walker logic mirrors crate::local_provider::adapters::ollama::request::compose_ollama_chat_request ...

    // History walk: each proto message maps to one or more GeminiContent
    // entries. See push_proto_message below.

    // Synthetic user-query anchoring (Phase B-6 parity).
    // ... mirror existing pattern ...

    // Final user_query append.
    // ... mirror existing pattern ...

    // Adjacent same-role merge: Gemini accepts consecutive same-role
    // messages but folding them keeps the body smaller and matches the
    // examples in Google's docs. The merger walks `contents` in place and
    // collapses adjacent (role, parts) pairs by extending the parts array
    // on the previous entry. Identical logic to Anthropic's
    // adjacent-same-role merger; factor out if it becomes a third site
    // (Phase 4 polish).

    GeminiGenerateRequest {
        system_instruction,
        contents,
        tools,
        generation_config: GeminiGenerationConfig::default(),
    }
}

fn function_declarations_gemini(enabled: &[LocalTool]) -> Vec<GeminiFunctionDeclaration> {
    enabled
        .iter()
        .filter_map(|t| {
            crate::local_provider::tools::schema_for(*t).map(|parameters| {
                GeminiFunctionDeclaration {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters,
                }
            })
        })
        .collect()
}
```

**History walking:** Map proto messages to Gemini shape:

```rust
fn push_proto_message(out: &mut Vec<GeminiContent>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => out.push(GeminiContent {
            role: GeminiRole::User,
            parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                text: q.query.clone(),
            })],
        }),
        Some(M::AgentOutput(a)) => out.push(GeminiContent {
            role: GeminiRole::Model,
            parts: vec![GeminiOutboundPart::Text(GeminiTextPart {
                text: a.text.clone(),
            })],
        }),
        Some(M::ToolCall(call)) => {
            if let Some((name, args)) = summarize_tool_call_input(call) {
                out.push(GeminiContent {
                    role: GeminiRole::Model,
                    parts: vec![GeminiOutboundPart::FunctionCall(
                        GeminiOutboundFunctionCallPart {
                            function_call: GeminiOutboundFunctionCall { name, args },
                        },
                    )],
                });
            }
        }
        Some(M::ToolCallResult(result)) => {
            let rendered = summarize_tool_result(result);
            let function_name = tool_call_name_for_result(result).unwrap_or_default();
            out.push(GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiOutboundPart::FunctionResponse(
                    GeminiOutboundFunctionResponsePart {
                        function_response: GeminiOutboundFunctionResponse {
                            name: function_name,
                            response: serde_json::json!({ "content": rendered }),
                        },
                    },
                )],
            });
        }
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}

/// Looks up the function name for a ToolCallResult by scanning prior
/// messages for the matching ToolCall (by `tool_call_id`). Gemini needs
/// the name on functionResponse parts (it keys to the prior functionCall
/// by name, not id). Falls back to "" — Gemini will error with
/// `INVALID_ARGUMENT`, which surfaces to the user as a clean upstream
/// error rather than silent confusion.
fn tool_call_name_for_result(_result: &api::message::ToolCallResult) -> Option<String> {
    // Implementation reads the surrounding history; the actual call site
    // in compose_gemini_request passes the running `proto_history` slice
    // for this lookup. See request_tests.rs::function_response_carries_name.
    None
}
```

**Adjacent-same-role merging:** Gemini doesn't reject consecutive `model`/`user` messages, but folding them produces cleaner bodies. Reuse Anthropic's merging pattern — walk `contents` and `Vec::splice` adjacent same-role entries' parts into the prior message.

**Compaction projection:** Identical pattern to Ollama / OpenAI translators. See `crates/ai/src/local_provider/adapters/ollama/request.rs::compose_ollama_chat_request` for the reference implementation. The projection synthesizes:
- A `user` "Continue from prior summary." message.
- A `model` `<summary>...</summary>` text block.

then resumes walking from `tail_start_id`.

- [ ] **Step 2.2: Sibling tests (~18) — `request_tests.rs`**

- System prompt lifts to `systemInstruction` (not in `contents`).
- Empty system prompt omits `systemInstruction` entirely.
- Simple user query becomes `contents[0]` with role `"user"` and one text part.
- Assistant proto message becomes role `"model"`.
- AgentReasoning is dropped.
- Tool-call proto message becomes role `"model"` with one `functionCall` part (`args` is an object).
- Tool-result proto message becomes role `"user"` with one `functionResponse` part (`response.content` is the rendered string).
- `functionResponse.name` carries the prior tool-call's function name (history-walking lookup).
- Tool-result with no prior matching tool-call carries `name: ""` (defensive fallback).
- Tools envelope wraps a single `functionDeclarations` array.
- Tools omitted when `supports_tools = false`.
- Tools omitted when `enabled_local_tools` is empty.
- `generationConfig` always emitted as `{}`.
- Compaction projection synthesizes the user/model summary pair correctly.
- Synthetic user-query anchoring works.
- Adjacent same-role merge collapses model+model into one message with two parts.
- Multi-turn round-trip (similar to Phase 3a/3b's parity test).
- Empty `contents` (e.g. first turn with only synthetic anchoring) builds a valid body.

- [ ] **Step 2.3: Build + tests + commit**

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/gemini): request translator

Phase 3c stage A. compose_gemini_request walks LocalProviderInput and
emits native :streamGenerateContent body shape: systemInstruction at
top level (Gemini rejects role:system in contents), contents with role
user/model (not user/assistant), functionCall parts with args as a
JSON object, functionResponse parts wrapping the rendered tool result
under {content: "..."}, tools envelope with a single
functionDeclarations array. Adjacent same-role messages are merged.
Reuses summarize_tool_call_input + summarize_tool_result + tools::schema_for
(pub(crate), reachable directly from the adapter module).
```

### Task 3: URL helpers

**File:** Modify `crates/ai/src/local_provider/config.rs`.

- [ ] **Step 3.1: Add Gemini URL helpers**

See "Design refinement → URL helpers" above for full code.

```rust
pub fn gemini_stream_generate_url(&self) -> Result<Url, LocalProviderConfigError> { ... }
pub fn gemini_generate_url(&self) -> Result<Url, LocalProviderConfigError> { ... }
pub fn gemini_models_url(&self) -> Result<Url, LocalProviderConfigError> { ... }
fn gemini_models_endpoint(&self, leaf_after_models: &str) -> Result<Url, LocalProviderConfigError> { ... }
fn gemini_endpoint(&self, leaf: &str) -> Result<Url, LocalProviderConfigError> { ... }
```

- [ ] **Step 3.2: Tests (~8)**

- `gemini_stream_generate_url_from_default_host` →
  `https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse`.
- `gemini_stream_generate_url_with_v1beta_path_is_idempotent`.
- `gemini_stream_generate_url_with_v1beta_trailing_slash_is_idempotent`.
- `gemini_generate_url_uses_generateContent_suffix` (non-streaming).
- `gemini_models_url_from_default_host`.
- `gemini_models_url_with_v1beta_path_is_idempotent`.
- `gemini_stream_generate_url_works_with_relay_base_path` (e.g. `https://relay.example.com/google`).
- `gemini_stream_generate_url_preserves_query_string` — confirms `?alt=sse` parses correctly via `Url::query()`.

- [ ] **Step 3.3: Commit**

```
feat(ai/local_provider/config): gemini endpoint URL helpers

Phase 3c stage A. gemini_stream_generate_url builds
{base_url}/v1beta/models/{model_id}:streamGenerateContent?alt=sse
(the ?alt=sse query is required — Gemini returns a JSON array
without it). gemini_generate_url is the non-streaming sibling used
by the summarizer; gemini_models_url backs the test-connection
probe. All three handle a base_url that already contains /v1beta
idempotently.
```

---

## Stage B: SSE decoder

### Task 4: `GeminiSseDecoder`

**Files:**
- Create `crates/ai/src/local_provider/adapters/gemini/response.rs`.
- Create `crates/ai/src/local_provider/adapters/gemini/response_tests.rs`.

- [ ] **Step 4.1: Decoder impl**

Mirror `OllamaDecoder`'s public surface — same `with_ids` / `new` / `skip_create_task` / `is_terminal` / `record_upstream_error` / `feed_event` / `finish`. Internal state matches the design refinement above. Implementation uses the shared `proto_helpers` module for `build_kind_message`, `client_action_*`, `build_tool_call_event`, and `internal_error_reason`.

Per-chunk dispatch is described in the design-refinement section. Key correctness points:

1. Skip empty data lines.
2. Lazily emit `Init` + `BeginTransaction` + `CreateTask` on first non-empty feed (`ensure_prelude` helper).
3. Deserialize `GeminiStreamChunk`; on parse error, transition to `Errored` and record `upstream_error`.
4. Top-level `error` field surfaces as `upstream_error` + `Errored`.
5. Merge `usage_metadata` from any chunk (last one wins per field, via `.max()`).
6. Walk `candidates[0].content.parts`:
   - `Text { text }`: append to the shared `AgentOutput` message (open if not yet).
   - `FunctionCall { function_call }`: emit one `AddMessagesToTask{ToolCall}` per call, synthesizing a UUID id and stringifying `args` for the proto.
   - `FunctionResponse { .. }`: ignore (output-only on the model side — shouldn't appear in stream responses).
   - `InlineData { .. }`: ignore (Phase 4c).
   - `Unknown`: ignore (forward-compat catch-all).
7. If `candidates[0].finish_reason` is set, capture + transition to `Done`.

The `finish()` method:
- Emits the prelude if not yet (shouldn't happen — `feed_event` would have).
- Emits `CommitTransaction` (if healthy: `state == Done && finish_reason captured`) or `Rollback`.
- Builds `TokenUsage` from `captured_input_tokens` + `captured_output_tokens` (model_id from `captured_model`, falling back to `cfg.model_id` via the controller-provided id or `"gemini"`).
- Emits `Finished{reason, token_usage}` where `reason` is `map_gemini_finish_reason(captured_finish_reason)`, or `InternalError("stream ended without finishReason")` when premature.

`map_gemini_finish_reason("STOP")` → `Done`; `"MAX_TOKENS"` → `MaxTokenLimit`; everything else (`SAFETY` / `RECITATION` / `OTHER` / `MALFORMED_FUNCTION_CALL` / `BLOCKLIST` / `PROHIBITED_CONTENT` / `SPII` / `LANGUAGE`) → `Other`.

- [ ] **Step 4.2: Sibling tests (~20) — `response_tests.rs`**

- Prelude emitted on first feed.
- `with_ids` round-trips into `Init`.
- `skip_create_task` suppresses `CreateTask`.
- Simple text streaming over multiple chunks builds the canonical event sequence (Init + Begin + Create + AddMessages(text "Hello") + Append(" world") + Commit + Finished{Done}).
- Function-call in one chunk emits one `AddMessages{ToolCall}` with a synthesized UUID id.
- Multiple function-calls in one chunk emit multiple events.
- Function-call followed by `finishReason: STOP` in same chunk emits the tool then transitions to Done.
- `finishReason: STOP` → `Done`.
- `finishReason: MAX_TOKENS` → `MaxTokenLimit`.
- `finishReason: SAFETY` → `Other`.
- `finishReason: MALFORMED_FUNCTION_CALL` → `Other`.
- Unknown `finishReason` → `Other`.
- Top-level `error` field surfaces as InternalError on finish.
- Malformed JSON chunk transitions to Errored.
- Premature EOF (no `finishReason`) → Rollback + InternalError("stream ended without finishReason").
- `record_upstream_error` surfaces in finish when no `finishReason`.
- `usageMetadata` from any chunk merged into `TokenUsage`; later chunks override (`.max()`).
- Chunk with empty `parts` array (final chunk with only finishReason) doesn't emit spurious `Append` events.
- Unknown part variant is silently ignored (forward-compat).
- Terminal-state safety: post-Done feeds are no-ops.
- AgentReasoning is not emitted (Gemini 2.5 thinking mode is Phase 4 polish; `Unknown` swallows thought parts in 3c).

- [ ] **Step 4.3: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/gemini): SSE decoder

Phase 3c stage B. GeminiSseDecoder consumes the
:streamGenerateContent SSE stream (anonymous data: chunks, each a
complete GenerateContentResponse partial). Text streams as a single
shared AgentOutput message (same pattern as OpenAi/Ollama; not the
per-block model used by Anthropic). functionCall parts arrive complete
in one chunk; synthesize a UUID id since Gemini doesn't emit one.
finishReason inside the last chunk is the terminator (no [DONE], no
message_stop). finishReason values map to the same Reason variants
OpenAi / Anthropic / Ollama use. usageMetadata may arrive on any
chunk; last-seen wins per field.
```

---

## Stage C: Adapter impl + dispatch flip

### Task 5: `GeminiAdapter` impl

**Files:**
- Create `crates/ai/src/local_provider/adapters/gemini/mod.rs`.
- Modify `crates/ai/src/local_provider/adapters/mod.rs`.

- [ ] **Step 5.1: `mod.rs` — adapter glue**

```rust
//! Gemini native protocol adapter. Phase 3c.
//!
//! Submodule layout mirrors Phase 3a/3b:
//! - `wire`: serde types for :streamGenerateContent (+ :generateContent
//!   for the summarizer).
//! - `request`: translator from `LocalProviderInput` to a
//!   `GeminiGenerateRequest`.
//! - `response`: SSE stream decoder (`GeminiSseDecoder`).
//!
//! Wire-format differences from OpenAi handled here:
//! - `x-goog-api-key` header (not `Authorization: Bearer`).
//! - Model lives in the URL path (`/v1beta/models/{model}:streamGenerateContent`),
//!   not the body.
//! - Top-level `systemInstruction`; alternating user/model roles (not
//!   user/assistant) with content-parts; functionCall.args is a JSON
//!   object; functionResponse parts replace OpenAI's role:tool messages;
//!   tool definitions are wrapped in a single `functionDeclarations`
//!   array.
//! - SSE is anonymous-chunk (same as OpenAI); `finishReason` is the
//!   terminator inside the last chunk.

pub mod request;
pub mod response;
pub mod wire;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
#[cfg(test)]
#[path = "response_tests.rs"]
mod response_tests;

use super::{
    AdapterError, AgentProviderApiType, LocalProviderConfig, LocalProviderInput, ProviderAdapter,
    StreamDecoder, StreamIds, SummarizerError, SummarizerInput,
};

use request::compose_gemini_request;
use response::GeminiSseDecoder;
use wire::{GeminiGenerateRequest, GeminiGenerateResponse};

pub struct GeminiAdapter;

impl ProviderAdapter for GeminiAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::Gemini
    }

    // streaming_format() inherits the SSE default.

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.gemini_stream_generate_url()?;
        let body = compose_gemini_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_gemini_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn create_stream_decoder(
        &self,
        ids: Option<StreamIds>,
        skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        let mut decoder = match ids {
            Some(ids) => GeminiSseDecoder::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => GeminiSseDecoder::new(),
        };
        if skip_create_task {
            decoder.skip_create_task();
        }
        Box::new(decoder)
    }

    fn build_summarizer_request(
        &self,
        input: &SummarizerInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.gemini_generate_url()?;
        let body = build_gemini_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_gemini_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: GeminiGenerateResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!(
                "{e}: {}",
                crate::local_provider::run::first_chars(body, 200)
            ))
        })?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(format!(
                "{}: {}",
                err.status, err.message
            )));
        }
        let combined = parsed
            .candidates
            .into_iter()
            .next()
            .and_then(|c| c.content)
            .map(|content| {
                content
                    .parts
                    .into_iter()
                    .filter_map(|p| match p {
                        wire::GeminiInboundPart::Text { text } if !text.trim().is_empty() => {
                            Some(text)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            Err(SummarizerError::NoContent)
        } else {
            Ok(trimmed.to_string())
        }
    }

    fn build_probe_request(
        &self,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.gemini_models_url()?;
        Ok(apply_gemini_headers(http.get(url), cfg.api_key.as_deref()))
    }
}

fn apply_gemini_headers(
    rb: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    match api_key.filter(|k| !k.is_empty()) {
        Some(k) => rb.header("x-goog-api-key", k),
        None => rb,
    }
}

/// Translate the OpenAI-shaped `SummarizerInput.messages` list (which the
/// compaction pipeline produces uniformly across adapters) into the
/// Gemini :generateContent shape. System messages lift to top-level
/// `systemInstruction`; user/assistant become user/model `contents`;
/// adjacent same-role entries merge. Tool roles aren't expected in
/// summarizer bodies (compaction sends `tools: None`); if any appear we
/// drop them silently.
fn build_gemini_summarizer_body(
    input: &SummarizerInput,
    _cfg: &LocalProviderConfig,
) -> GeminiGenerateRequest {
    use crate::local_provider::wire::Role;
    let mut system_parts: Vec<wire::GeminiTextPart> = Vec::new();
    let mut entries: Vec<wire::GeminiContent> = Vec::new();
    for msg in &input.messages {
        let text = msg.content.clone().unwrap_or_default();
        match msg.role {
            Role::System => {
                if !text.is_empty() {
                    system_parts.push(wire::GeminiTextPart { text });
                }
            }
            Role::User | Role::Assistant => {
                let role = match msg.role {
                    Role::User => wire::GeminiRole::User,
                    Role::Assistant => wire::GeminiRole::Model,
                    _ => unreachable!(),
                };
                let part = wire::GeminiOutboundPart::Text(wire::GeminiTextPart { text });
                match entries.last_mut() {
                    Some(last) if last.role == role => last.parts.push(part),
                    _ => entries.push(wire::GeminiContent {
                        role,
                        parts: vec![part],
                    }),
                }
            }
            Role::Tool => {
                // Compaction never emits role:Tool, but be defensive — drop
                // rather than misencode.
            }
        }
    }
    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(wire::GeminiSystemInstruction {
            parts: system_parts,
        })
    };
    GeminiGenerateRequest {
        system_instruction,
        contents: entries,
        tools: None,
        generation_config: wire::GeminiGenerationConfig::default(),
    }
}
```

- [ ] **Step 5.2: Adapter tests (~8) — add to `request_tests.rs`**

- Chat URL is `{base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse` with `x-goog-api-key` header + Accept `text/event-stream`.
- Chat omits `x-goog-api-key` header when key absent (relay scenario).
- Chat handles trailing-slash base correctly.
- Summarizer URL uses `:generateContent` suffix, Accept `application/json`.
- Summarizer body: system lifts to top-level `systemInstruction`; user/assistant lift to user/model.
- Probe URL is `{base_url}/v1beta/models`.
- `parse_summarizer_response` extracts text from `candidates[0].content.parts[0].text`.
- `parse_summarizer_response` empty → `NoContent`.
- `parse_summarizer_response` top-level error → `UpstreamErrorEnvelope`.

- [ ] **Step 5.3: Build + tests + commit**

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/gemini): adapter glue

Phase 3c stage C. GeminiAdapter implements ProviderAdapter: chat
targets {base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse
with x-goog-api-key auth and text/event-stream Accept; summarizer
hits the :generateContent sibling with stream:false body; probe
targets {base_url}/v1beta/models. streaming_format() inherits SSE
default — no runtime branch needed. Summarizer body translator lifts
system messages to top-level systemInstruction and maps assistant to
the "model" role.
```

### Task 6: Flip `select_adapter`

**Files:**
- Modify `crates/ai/src/local_provider/adapters/mod.rs`.
- Modify `crates/ai/src/local_provider/adapters/adapters_tests.rs`.

- [ ] **Step 6.1: Wire in the new submodule + adapter**

```rust
// adapters/mod.rs

pub mod anthropic;
pub mod gemini;            // <- new
pub mod ollama;
pub mod openai;
pub(crate) mod proto_helpers;
pub use anthropic::AnthropicAdapter;
pub use gemini::GeminiAdapter;
pub use ollama::OllamaAdapter;
pub use openai::OpenAiAdapter;
```

Update the file-level doc comment:

```rust
//! Provider adapter trait — abstracts request composition and stream decoding
//! over wire-protocol variants. Phase 2 added `OpenAi`; Phase 3a added
//! `Anthropic`; Phase 3b added `Ollama`; Phase 3c added `Gemini`. `DeepSeek`
//! remains a Phase 3d impl; `OpenAiResp` is Phase 4 polish.
```

Update the `ProviderAdapter` trait's `streaming_format` rustdoc to drop Gemini from the "Future SSE-based adapters" example:

```rust
/// What wire framing does this adapter's chat stream use? Defaults to
/// SSE — `OllamaAdapter` overrides to `NewlineDelimitedJson`. Future
/// SSE-based adapters (DeepSeek) inherit the default and need not
/// implement this method.
fn streaming_format(&self) -> StreamingFormat {
    StreamingFormat::ServerSentEvents
}
```

Flip `select_adapter`:

```rust
pub fn select_adapter(
    api_type: AgentProviderApiType,
) -> Result<Box<dyn ProviderAdapter>, AdapterError> {
    use AgentProviderApiType::*;
    match api_type {
        OpenAi => Ok(Box::new(OpenAiAdapter)),
        Anthropic => Ok(Box::new(AnthropicAdapter)),
        Ollama => Ok(Box::new(OllamaAdapter)),
        Gemini => Ok(Box::new(GeminiAdapter)),
        OpenAiResp | DeepSeek => Err(AdapterError::UnsupportedApiType(api_type)),
    }
}
```

Update the rustdoc comment on `select_adapter` to reflect the new state.

- [ ] **Step 6.2: Update `adapters_tests.rs`**

```rust
#[test]
fn select_adapter_returns_gemini_for_gemini_api_type() {
    let a = select_adapter(AgentProviderApiType::Gemini).expect("ok");
    assert_eq!(a.api_type(), AgentProviderApiType::Gemini);
}

#[test]
fn select_adapter_errors_for_each_unimplemented_variant() {
    for ty in [
        AgentProviderApiType::OpenAiResp,
        AgentProviderApiType::DeepSeek,
    ] {
        match select_adapter(ty) {
            Ok(_) => panic!("expected UnsupportedApiType for {ty:?}"),
            Err(AdapterError::UnsupportedApiType(got)) => assert_eq!(got, ty),
            Err(other) => panic!("wrong variant for {ty:?}: {other:?}"),
        }
    }
}
```

Removed `AgentProviderApiType::Gemini` from the loop.

- [ ] **Step 6.3: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3       # ~545 tests total
cargo nextest run -p warp --lib 2>&1 | tail -3
cargo clippy -p ai -p warp --lib --tests -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters): flip select_adapter for Gemini

Phase 3c stage C. Gemini api_type now dispatches to GeminiAdapter
instead of returning UnsupportedApiType. Updates the adapters/mod.rs
file-level doc + the select_adapter rustdoc; updates adapters_tests
(adds the green-path test, drops Gemini from the unimplemented loop).
streaming_format() rustdoc updated — only DeepSeek remains as a
"future SSE-based adapter".
```

---

## Stage D: Manual smoke + docs

### Task 7: Live test + spec docs

- [ ] **Step 7.1: Live test against `generativelanguage.googleapis.com`**

Documented checklist — requires a Gemini API key from <https://aistudio.google.com/app/apikey>:

1. Create or reuse an API key (free-tier is fine for the smoke test).
2. In the app, Settings → AI → Custom AI Providers, add provider:
   - Name: `Gemini`
   - API type: `Gemini`
   - Base URL: `https://generativelanguage.googleapis.com`
   - API key: paste the `AIza…` value.
   - Model id: `gemini-1.5-pro` or `gemini-2.5-flash` (one row; context window 1048576 for 1.5-pro, tool_call enabled).
3. Click **Test connection** — expect green check (200 from `/v1beta/models`).
4. Open a new conversation, pick `Gemini / gemini-1.5-pro`.
5. Send: `Read the top 5 lines of Cargo.toml and tell me what version of the workspace package we're on.` — expect streamed assistant text, a `read_files` tool call, the tool result rendered back, and a final summary.
6. Send a multi-turn follow-up that re-references the prior result; confirm history threads correctly and the model can read the prior tool result via the `functionResponse` part shape.
7. Edit the API key to an obviously-invalid value, click Test connection — expect red failure with a 400/403 message that includes the `INVALID_ARGUMENT` / `PERMISSION_DENIED` status.
8. Restore the valid key, switch the model id to a non-existent value (e.g. `gemini-fake`) and run a turn — expect a clean upstream error surfaced inline.

Document any deviations from the expected behavior in the smoke-test PR comment.

- [ ] **Step 7.2: Update spec docs**

- `specs/multi-local-llm/README.md`:
  - Phase 3c row in the table: 🧪 code complete (then ✅ shipped once live smoke passes) with date.
  - "What landed" Architecture section: add the Gemini adapter file paths.
  - "What landed" User-visible section: add the Gemini option as a real api_type with native streaming + content-parts shape.
  - "Future phases" section: drop the Gemini entry; remaining work is 3d (DeepSeek), 4a-d (polish).
  - Reading-order Source list: add Gemini adapter paths.
- `specs/multi-local-llm/design.md` §9 phase table: mark Phase 3c row 🧪 code complete / ✅ shipped.

- [ ] **Step 7.3: Commit**

```
docs(specs/multi-local-llm): record Phase 3c code-complete status
```

Or, after live smoke passes:

```
docs(specs/multi-local-llm): mark Phase 3c (Gemini adapter) shipped
```

---

## Final verification

- [ ] **Verification 1: Sweeps**

```bash
echo "=== Gemini submodule wired ==="
grep -n "pub mod gemini" crates/ai/src/local_provider/adapters/mod.rs

echo "=== select_adapter flipped for Gemini ==="
grep -nA 1 "Gemini =>" crates/ai/src/local_provider/adapters/mod.rs

echo "=== GeminiAdapter inherits SSE default ==="
grep -n "streaming_format" crates/ai/src/local_provider/adapters/gemini/mod.rs || echo "(none — inherits default — expected)"

echo "=== Gemini URL helpers ==="
grep -n "fn gemini_stream_generate_url\|fn gemini_generate_url\|fn gemini_models_url" crates/ai/src/local_provider/config.rs

echo "=== ?alt=sse query is present in URL helper ==="
grep -n "alt=sse" crates/ai/src/local_provider/config.rs

echo "=== run.rs unchanged ==="
git diff --stat 3f0f2b30 -- crates/ai/src/local_provider/run.rs   # should be empty
```

- [ ] **Verification 2: Build + tests + clippy**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -5     # ~545 tests (496 + ~50 Gemini)
cargo nextest run -p warp --lib 2>&1 | tail -5
cargo clippy -p ai --all-targets --all-features -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

- [ ] **Verification 3: Manual smoke**

Per Task 7.1 — real Gemini API key.

- [ ] **Verification 4: Final reviewer + push**

Dispatch `oh-my-claudecode:code-reviewer` for the full Phase 3c diff. Stop before push; user reviews, then pushes manually.

---

## Risks & open questions

1. **`?alt=sse` query placement.** Without it, `:streamGenerateContent` returns `application/json` containing a JSON array — the `EventSource` then gets `Content-Type` rejected and the stream errors out before the first event. The URL helper composes the query directly; a unit test asserts `Url::query() == Some("alt=sse")` to lock this in.

2. **`url::Url::join` and the `:` in `:streamGenerateContent`.** The colon is a valid path-segment character per RFC 3986; `url::Url::join` preserves it. Verified by a unit test (`gemini_stream_generate_url_works_with_relay_base_path`) that round-trips through `Url::parse`.

3. **`functionResponse.name` lookup.** Gemini requires the function name on a `functionResponse` part (it matches the prior `functionCall` by name, not id). The translator scans the running history slice to find the matching tool-call name. If the name lookup fails (e.g. orphaned tool result, ordering bug) we send `name: ""` — Gemini errors with `INVALID_ARGUMENT`, which is surfaced cleanly via the SSE error path. Document this in `request.rs::tool_call_name_for_result`.

4. **Tool-call id stability across turns.** Same as Ollama — Gemini doesn't send ids, decoder synthesizes UUIDs. The model can't reference a prior call by id; references happen by `name`. This matches the existing behavior on the other native adapters. Acceptable.

5. **`finishReason` not on the same chunk as the last text.** Gemini sometimes emits the final `finishReason` in its own chunk with empty `parts`, sometimes alongside the last text fragment. The decoder handles both: text is appended (if present), then `finish_reason` is captured (if present), then state transitions to Done.

6. **`usageMetadata` may arrive on the first chunk.** Some Gemini revisions report partial usage on the first chunk and final usage on the last; others only the last. The decoder takes `.max()` per field so the final number wins.

7. **`Unknown` part variant on the inbound enum.** Gemini may emit `thought` parts (2.5 thinking mode), `executableCode` parts (code-execution tool), and `codeExecutionResult` parts. None of these are supported in Phase 3c — they fall through `GeminiInboundPart::Unknown` and the decoder ignores them. Phase 4 polish can split `thought` out into `AgentReasoning` (similar to Anthropic's `thinking_delta` handling).

8. **No SAFETY-specific surfacing.** Gemini's safety-filter blocks return `finishReason: "SAFETY"` plus `safetyRatings` on the candidate. Phase 3c maps SAFETY → `Reason::Other`; the UI renders this as a generic stop. Phase 4 polish can add a `Reason::SafetyBlock` variant the UI distinguishes.

9. **No vendor / region routing.** This adapter targets the consumer Gemini API at `generativelanguage.googleapis.com`. Users on Vertex AI (GCP project + region scoped, OAuth2 auth) can't use this path — that's a separate api_type (`GeminiVertex`?) we deferred from the Phase 3c scope. The README's user-visible blurb makes this distinction explicit.

10. **No live test in CI.** Same gate as Phase 3a / 3b — manual smoke against the real API. A future mock-server integration test would unblock CI coverage; deferred to Phase 4.

11. **Empty `candidates` array.** If Gemini emits a chunk with `candidates: []` (which can happen during a safety-block ramp), the decoder simply skips that chunk (no text, no finishReason). The `record_upstream_error` path doesn't fire because the envelope isn't an error per se. Tested via a unit test.

---

## Next plan (Phase 3d — DeepSeek adapter)

After Phase 3c ships green, Phase 3d targets DeepSeek. Wire format: SSE (so still no `streaming_format` override). Endpoint: `POST {base_url}/chat/completions` — same as OpenAI's compat shape, but with a critical twist: DeepSeek-Reasoner (`deepseek-reasoner`) emits a `reasoning_content` field alongside `content` on assistant messages, and the model expects the prior turn's `reasoning_content` to be round-tripped back in the history. Phase 3d wires this in via a small `reasoning_content` field on the assistant message wire type and an `AgentReasoning` emit path in the decoder. Plan written after 3c is approved + executed.
