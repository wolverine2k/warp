# Multi-Local-LLM — Phase 3a (Anthropic Adapter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a native `AnthropicAdapter` against the Anthropic Messages API and flip `select_adapter` for `AgentProviderApiType::Anthropic` from `Err(UnsupportedApiType)` to a real impl. This is the **first native non-OpenAI adapter** and validates the Phase 2 trait shape — if anything's awkward, fix the trait here, not later. Hand-roll first (per `design.md` §2.3); switch to the `genai` crate only if hand-rolling exceeds ~1 week.

**Architecture:** Three logical stages, atomic in one PR (split into 3a-i / 3a-ii / 3a-iii if review prefers):

- **Stage A (Tasks 1-3)** — Wire types and request composition. Add `adapters/anthropic/` submodule with `wire.rs` (serde types for the Messages API), `request.rs` (translator from `LocalProviderInput` → Anthropic request body), and unit tests for message-shape conversion (system lift-out, tool-result-as-user, tool schema reshape).
- **Stage B (Tasks 4-5)** — SSE decoder. Implement `AnthropicSseDecoder` that consumes the event stream (`message_start` / `content_block_*` / `message_delta` / `message_stop`) and emits the same `api::ResponseEvent` shape `OpenAiSseAdapter` does. Mirror its public surface (Init, BeginTransaction, CreateTask, Append/Add, Commit/Rollback, Finished) so the controller is unaware which adapter produced the events.
- **Stage C (Tasks 6-8)** — Adapter impl + dispatch flip + live test. Implement `ProviderAdapter for AnthropicAdapter` (build_chat_request, create_stream_decoder, build_summarizer_request, parse_summarizer_response, build_probe_request), flip the `select_adapter` match arm, update the settings UI api-type dropdown to *enable* Anthropic (today it's a reserved-but-disabled chip), and do a manual smoke test against `api.anthropic.com` with a real key.

**Branch:** `multi-local-llm`. Forks from `df0ec591` (Phase 2 tip). Estimated ~700 lines net code (~250 wire types + 250 request translator + 250 decoder + 150 adapter), ~6 hours of subagent-driven work.

**Spec references:**
- `specs/multi-local-llm/design.md` §2.3 (Phase 3 sketch), §9 (phase table — Phase 3a row).
- `specs/multi-local-llm/plan-phase-2.md` "Next plan (Phase 3a)" section — superseded by this document.
- Anthropic Messages API: https://docs.anthropic.com/en/api/messages and https://docs.anthropic.com/en/docs/build-with-claude/streaming.

**Test gate:** All existing `cargo nextest run -p ai` tests pass; new Anthropic-specific tests added (~25-30 new tests across wire/request/response). Manual smoke: a real Anthropic provider configured against `api.anthropic.com` with a valid key produces a streamed assistant turn, including at least one tool call + tool result, and the "Test connection" probe succeeds.

**Out of Phase 3a (deferred):**
- Ollama-native, Gemini, DeepSeek adapters — Phase 3b/c/d.
- Anthropic extended thinking (`thinking` content blocks) surfaced as `AgentReasoning`: included if implementation is straightforward, dropped to Phase 4 otherwise.
- Anthropic prompt caching (`cache_control` on system / tool blocks) — Phase 4 polish, since it requires UI for cache-strategy selection.
- Vision / PDF / audio input — Phase 4c (multimodal capability fields).
- `/v1/models` model-list importer button — Phase 4a.

---

## Design refinement

### Adapter layout: `adapters/anthropic/` submodule, not a single file

`OpenAiAdapter` (Phase 2) is ~140 lines because it delegates to existing `request.rs` / `response.rs` / `wire.rs` at the local_provider module root. For Anthropic, the parallel scaffolding doesn't exist — we need fresh wire types, a fresh translator, and a fresh decoder. Single-file would push the file over 700 lines, mixing four concerns. Use a submodule directory:

```
crates/ai/src/local_provider/adapters/
├── mod.rs          (existing — trait defs + select_adapter)
├── openai.rs       (existing — ~140 lines, OpenAi impl)
├── adapters_tests.rs (existing)
├── probe_tests.rs    (existing)
└── anthropic/
    ├── mod.rs              # AnthropicAdapter impl + ProviderAdapter trait wiring
    ├── wire.rs             # Serde types for Messages API
    ├── request.rs          # compose_anthropic_messages_request + history walker
    ├── request_tests.rs    # sibling tests for request.rs
    ├── response.rs         # AnthropicSseDecoder + StreamDecoder impl
    └── response_tests.rs   # sibling tests for response.rs
```

`adapters/mod.rs` registers `pub mod anthropic;` and re-exports `pub use anthropic::AnthropicAdapter`. No other public exports — the wire / request / response types stay crate-private.

### Anthropic Messages API endpoints

- **Chat (streaming):** `POST {base_url}/v1/messages` with `stream: true` in the body.
- **Chat (non-streaming):** same endpoint, `stream: false`. Used by the summarizer.
- **Probe:** `GET {base_url}/v1/models`. Released by Anthropic Nov 2024; returns `{"data": [...]}`. We don't parse — HTTP 2xx is the only success criterion.

Default `base_url` for Anthropic users: `https://api.anthropic.com`. The settings UI will keep `base_url` user-editable (matches openwarp pattern — supports proxy / enterprise gateways like Bedrock-front-ends or self-hosted Claude relays).

### Authentication headers

Anthropic uses **`x-api-key`**, not `Authorization: Bearer`. It also requires:
- `anthropic-version: 2023-06-01` (the API version pin; same value across all Claude 3.x and 4.x models).
- `content-type: application/json` on POST.
- `accept: text/event-stream` on streaming requests.

```rust
fn apply_anthropic_headers(rb: reqwest::RequestBuilder, key: Option<&str>) -> reqwest::RequestBuilder {
    let mut rb = rb.header("anthropic-version", "2023-06-01");
    if let Some(k) = key.filter(|s| !s.is_empty()) {
        rb = rb.header("x-api-key", k);
    }
    rb
}
```

The `Authorization: Bearer` style is **not** used — Anthropic's gateway returns a generic 401 with no body for Bearer auth, which would have surfaced as an opaque "HTTP 401" in the test-connection probe.

### Request body shape

```jsonc
{
  "model": "claude-sonnet-4-6",
  "max_tokens": 4096,                          // REQUIRED — see decision below
  "system": "...",                             // top-level, separate from messages
  "messages": [
    { "role": "user", "content": [{"type":"text","text":"..."}] },
    { "role": "assistant", "content": [
        {"type":"text","text":"..."},
        {"type":"tool_use","id":"toolu_01","name":"read_files","input":{...}}
      ]
    },
    { "role": "user", "content": [
        {"type":"tool_result","tool_use_id":"toolu_01","content":"..."}
      ]
    }
  ],
  "tools": [
    { "name":"read_files", "description":"...", "input_schema": {...} }
  ],
  "tool_choice": {"type":"auto"},
  "stream": true
}
```

**Key shape differences from OpenAI:**
1. `system` is a top-level field, **not** a message with `role:"system"`. We lift the synthesized system prompt out of the message list.
2. Each message's `content` is either a string OR an array of content blocks. We always use the array form to stay uniform across text and tool-use cases.
3. Anthropic alternates strictly between `user` and `assistant`. Consecutive same-role messages must be merged.
4. Tool calls live **inside** the assistant message's `content` as `{type:"tool_use", ...}` blocks — not as a separate `tool_calls` field.
5. Tool results are sent as `role:"user"` messages with `{type:"tool_result", tool_use_id, content}` blocks — not as a separate `role:"tool"`.
6. `max_tokens` is **required** by Anthropic (unlike OpenAI where it's optional). Decision below.
7. Tool definitions: `{name, description, input_schema}` — no wrapping `function: {...}` or `type:"function"`.

### `max_tokens` resolution

Anthropic's Messages API rejects requests without `max_tokens`. The `LocalProviderConfig` doesn't carry `max_output_tokens` (Phase 1's data model only has `context_window`). Strategy:

1. If `cfg.context_window` is `Some(n)` and `n >= 8192`, use `min(n / 4, 8192)` — quarter of the window, capped at 8K, which fits Sonnet's 8192-token output ceiling.
2. Otherwise default to `4096`.

This is conservative (most Claude 3.5/4 models support 8192 output tokens; Sonnet 4.6 supports 64K with the `output-128k-2025-02-19` beta header which we won't send in Phase 3a). Anthropic returns `stop_reason: "max_tokens"` if hit — the existing `map_stop_reason` logic surfaces this as `Reason::MaxTokenLimit`, matching the OpenAI `length` finish-reason behavior. A future Phase 4 task can add a `max_output_tokens` field to `AgentProviderModel` for explicit per-model tuning.

### SSE event types and the decoder state machine

Anthropic streams a structured event sequence:

```text
event: message_start          → MessageStart  { message: { id, role, model, usage:{input_tokens,output_tokens} } }
event: content_block_start    → ContentBlockStart { index, content_block: { type: "text" | "tool_use" | "thinking", ... } }
event: content_block_delta    → ContentBlockDelta { index, delta: { type: "text_delta" | "input_json_delta" | "thinking_delta", ... } }
event: content_block_stop     → ContentBlockStop { index }
event: message_delta          → MessageDelta { delta: { stop_reason, stop_sequence }, usage: { output_tokens } }
event: message_stop           → MessageStop
event: ping                   → Ping (keep-alive; ignore)
event: error                  → Error { error: { type, message } }
```

The SSE wire format prefixes each event with both `event: <name>` and `data: <json>` lines. Our existing SSE plumbing (`reqwest_eventsource`) parses both fields and surfaces each as a `MessageEvent` with `event: String` and `data: String`. The `StreamDecoder` trait's `feed(&mut self, data: &str)` takes the `data` field only — but Anthropic events are not distinguishable from `data` alone (the `event:` line carries the discriminator). **Resolution:** trait change. See below.

#### `StreamDecoder` trait extension

The current trait takes only `feed(&mut self, data: &str)`. OpenAI's stream doesn't have named events (every chunk is anonymous JSON), so this signature works. For Anthropic, the runner needs to pass the `event` name alongside the `data` payload, or the decoder has to introspect the JSON to derive the type (Anthropic chunks happen to include `type` as the first JSON field — so `feed(data)` *can* work without the event name).

**Decision:** extend the trait minimally to pass event metadata, with a default that preserves OpenAI's behavior:

```rust
pub trait StreamDecoder: Send {
    /// Feed an SSE message data line. The default impl delegates to
    /// `feed_event(None, data)` — OpenAI keeps its existing single-arg path.
    fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent> {
        self.feed_event(None, data)
    }
    /// Feed an SSE message with explicit `event:` discriminator. Anthropic
    /// uses this; OpenAI ignores the `event_name` argument.
    fn feed_event(&mut self, event_name: Option<&str>, data: &str) -> Vec<api::ResponseEvent>;
    fn finish(&mut self) -> Vec<api::ResponseEvent>;
    fn is_terminal(&self) -> bool;
    fn record_upstream_error(&mut self, msg: String);
}
```

The synthesize loop in `run.rs` switches to `decoder.feed_event(msg.event.as_deref().filter(|s| !s.is_empty()), &msg.data)`. `reqwest_eventsource::MessageEvent` carries `event: String` (defaults to `"message"` per the EventSource spec; we treat `"message"` as `None`).

`OpenAiSseAdapter::feed_event` ignores `event_name` and forwards to its existing `feed` body. `AnthropicSseDecoder` dispatches on `event_name`.

#### Decoder state mapping

```text
message_start          → emit Init + BeginTransaction + CreateTask (matches OpenAi)
content_block_start    → record the content-block-index → block-type mapping
                         (text/thinking: open agent_output/agent_reasoning;
                          tool_use: start a tool_buffer with id, name)
content_block_delta    → text_delta: append to agent_output
                         thinking_delta: append to agent_reasoning
                         input_json_delta: append to tool_buffer.arguments
content_block_stop     → emit the buffered tool_call event (if tool_use block)
message_delta          → capture stop_reason + output_tokens usage
message_stop           → flush any unemitted buffers, transition to Finishing
                         (the runner detects Finishing via is_terminal == true
                          on the [DONE]-equivalent — Anthropic ends with the
                          message_stop event itself, no separate [DONE] line)
error                  → record_upstream_error + transition to Errored
ping                   → no-op
```

**`message_stop` ≠ `[DONE]`:** Anthropic emits `message_stop` as the final logical event but the HTTP stream may end immediately after (no `[DONE]` line). We treat `message_stop` as the terminator — set `state = Done` so `is_terminal()` becomes true; the runner already flushes-and-closes when `is_terminal()` is hit. This matches the existing pattern in `synthesize_stream` (Phase 2) without changes.

#### Usage accounting

Anthropic emits `usage` in two places:
- `message_start.message.usage`: `{ input_tokens, output_tokens, cache_creation_input_tokens?, cache_read_input_tokens? }` — input tokens are final here; output_tokens is 0 or 1 at this point.
- `message_delta.usage`: `{ output_tokens }` — running total; final value on the last `message_delta`.

The decoder captures both, merges into a single `AnthropicUsage` struct, and emits `TokenUsage` on `finish()`:

```rust
TokenUsage {
    model_id: captured_model.unwrap_or("anthropic").to_string(),
    total_input: input_tokens,
    output: output_tokens,
    input_cache_read: cache_read_input_tokens.unwrap_or(0),
    input_cache_write: cache_creation_input_tokens.unwrap_or(0),
    cost_in_cents: 0.0,
}
```

This is more usage detail than OpenAI gives us — log it but don't surface specially. Phase 4 polish may render cache stats in the UI.

### Tool conversion

The OpenAI adapter consumes `LocalProviderInput.tasks` and synthesizes OpenAI `tool_calls` / `role:"tool"` pairs from the proto `Message::ToolCall` / `Message::ToolCallResult` variants (see `request.rs::push_history_messages`). For Anthropic the same proto inputs need to be folded into the alternating-role-with-content-blocks shape:

```text
proto Message::UserQuery       → {"role":"user", "content":[{"type":"text","text":...}]}
proto Message::AgentOutput     → {"role":"assistant", "content":[{"type":"text","text":...}]}
proto Message::ToolCall        → {"role":"assistant", "content":[{"type":"tool_use","id":...,"name":...,"input":{...}}]}
proto Message::ToolCallResult  → {"role":"user", "content":[{"type":"tool_result","tool_use_id":...,"content":...}]}
proto Message::AgentReasoning  → DROPPED (matches OpenAI behavior; reasoning isn't replayed)
```

**Merging rule:** Anthropic rejects consecutive same-role messages. After flattening the proto messages, run a fold pass that merges adjacent same-role entries by concatenating their `content` block arrays. This makes:

```text
[assistant_text, assistant_tool_use, user_tool_result, user_query]
```

become a 3-message body (the two assistant entries merge; user_tool_result and user_query stay separate? No — they're both user, so they merge too):

```text
[assistant: [text, tool_use], user: [tool_result, text]]
```

This is the canonical Anthropic shape and what their docs show in agentic tool-use examples.

**Orphan tool-call backfill** (the `backfill_orphaned_tool_calls` logic in OpenAI's `request.rs`): same idea — for each `tool_use` block emitted by the assistant, the *next* user message must contain a matching `tool_result` block. If `action_results` has the result, splice it in; otherwise insert a placeholder `tool_result` with `content: "(tool result not available)"`. Anthropic also requires this strictly (returns 400 otherwise).

**System prompt:** lift the synthesized system prompt out of the message list and into the top-level `system` field. If multiple system messages exist (compaction projection can synthesize a continue-prompt; that goes in messages as a user-role pair, not as system), join with `\n\n`. Phase 3a only ever has one system message — the synthesized prompt from `prompt::compose_system_prompt` — so concatenation is a guard rather than a real path.

**Tool definitions reshape:** `tools::tool_definitions(&local_tools)` today returns OpenAI shape (`{type:"function", function:{name, description, parameters}}`). We need a parallel `tools::tool_definitions_anthropic(&local_tools)` that emits `{name, description, input_schema}` — the JSON Schema body is identical; just unwrap the OpenAI envelope. ~20 lines.

### `validate()` and base URL flexibility

The existing `LocalProviderConfig::validate` rejects schemes other than http/https. Anthropic uses https — fine. **One nuance:** the existing `chat_completions_url()` always appends `chat/completions` regardless of api_type. We need an Anthropic-aware helper `messages_url()` that appends `v1/messages` (or `messages` if the base_url already includes `/v1`). Implementation:

```rust
impl LocalProviderConfig {
    pub fn messages_url(&self) -> Result<Url, LocalProviderConfigError> {
        let mut base = Url::parse(&self.base_url)?;
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        // Strip a trailing /v1 from path so we can always append v1/messages
        // cleanly; users pasting "https://api.anthropic.com/v1" should not
        // produce "https://api.anthropic.com/v1/v1/messages".
        let target = if base.path().ends_with("/v1/") { "messages" } else { "v1/messages" };
        base.join(target).map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))
    }
    // probe_url_anthropic: same idea but `v1/models`.
}
```

Symmetric helpers keep the adapter file thin.

### `select_adapter` flip

```rust
pub fn select_adapter(api_type: AgentProviderApiType)
    -> Result<Box<dyn ProviderAdapter>, AdapterError>
{
    use AgentProviderApiType::*;
    match api_type {
        OpenAi    => Ok(Box::new(OpenAiAdapter)),
        Anthropic => Ok(Box::new(anthropic::AnthropicAdapter)),    // flipped in Phase 3a
        OpenAiResp | Gemini | Ollama | DeepSeek => {
            Err(AdapterError::UnsupportedApiType(api_type))
        }
    }
}
```

### Settings UI: enable the Anthropic chip

`AgentProvidersWidget` shows an api-type dropdown (or chip row) per provider. Today Phase 2 reserved `Anthropic` in the enum but the UI either greys it out or simply lists it as a valid choice that doesn't dispatch correctly. Phase 3a:

1. Locate the widget's api-type renderer (somewhere in `app/src/settings_view/agent_providers_widget.rs`).
2. Confirm the Anthropic option is selectable. If a "Coming soon" / disabled state is rendered, remove that guard for `Anthropic` specifically; leave `Gemini` / `Ollama` / `DeepSeek` / `OpenAiResp` in their disabled state.
3. The settings UI doesn't need any other change — the api_type is already serialized into `AgentProvider.api_type`, which `snapshot_for_request` already passes to `LocalProviderConfig.api_type` (Phase 2 work).

If the Phase 2 widget didn't gate non-OpenAI options at all (they're just listed and rely on the dispatch-time error for feedback), this task is a no-op. Confirm during execution.

### Summarizer (non-streaming)

Same Messages API endpoint, `stream: false`. The non-streaming response shape:

```jsonc
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "content": [{"type":"text","text":"..."}],
  "model": "claude-...",
  "stop_reason": "end_turn",
  "usage": { "input_tokens": ..., "output_tokens": ... }
}
```

`parse_summarizer_response` walks the content blocks and concatenates `text` entries. Phase 3a-only summarizer responses won't include `tool_use` (we send `tools = None`), so a single text block is the normal case. The implementation mirrors OpenAI's: trim, error on empty.

### Hand-roll vs. `genai` crate

Per `design.md` §2.3 and §11 Risk 5: hand-roll first; revisit `genai` if Phase 3a exceeds ~1 week. The hand-roll is well-scoped (~700 lines, three discrete concerns, no dependency on Anthropic's mid-2026 API stability beyond the documented Messages API which has been stable since 2023). The estimated 6-hour budget is well inside the 1-week threshold. **Decision: hand-roll.** Revisit only if the SSE event-state machine takes more than half a day to stabilize.

---

## File map

**Files created:**
- `crates/ai/src/local_provider/adapters/anthropic/mod.rs` — adapter impl.
- `crates/ai/src/local_provider/adapters/anthropic/wire.rs` — serde types.
- `crates/ai/src/local_provider/adapters/anthropic/request.rs` — translator + tool reshape + merge pass.
- `crates/ai/src/local_provider/adapters/anthropic/request_tests.rs` — sibling unit tests.
- `crates/ai/src/local_provider/adapters/anthropic/response.rs` — SSE decoder.
- `crates/ai/src/local_provider/adapters/anthropic/response_tests.rs` — sibling unit tests.

**Files modified:**
- `crates/ai/src/local_provider/adapters/mod.rs` — `pub mod anthropic;`, re-export `AnthropicAdapter`, flip `select_adapter` arm. Trait gains `feed_event` with default; the existing `feed` becomes a default that forwards.
- `crates/ai/src/local_provider/response.rs` — `OpenAiSseAdapter`'s `StreamDecoder` impl gains `feed_event` (ignores `event_name`, forwards to `feed`).
- `crates/ai/src/local_provider/run.rs` — `synthesize_stream` calls `decoder.feed_event(msg.event.as_deref()...)` instead of `decoder.feed(&msg.data)`.
- `crates/ai/src/local_provider/config.rs` — add `messages_url()` and `anthropic_models_url()` helpers + tests.
- `crates/ai/src/local_provider/tools.rs` — add `tool_definitions_anthropic(&[LocalTool]) -> Vec<AnthropicToolDef>` (or move the schema-extraction logic into a shared helper and have each adapter wrap it). Trivial.
- `app/src/settings_view/agent_providers_widget.rs` — confirm `Anthropic` is selectable in the api-type dropdown / chip row; remove "disabled" guard if one exists.
- `crates/ai/src/local_provider/adapters/adapters_tests.rs` — update `select_adapter_returns_*` test to expect `Anthropic` to succeed; trim `Anthropic` from the `unimplemented_variant` loop.

**Cargo deps:** none added. `reqwest`, `serde`, `serde_json`, `uuid`, `thiserror` are all already pulled in.

---

## Stage A: Wire types + request composition

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git status --short                       # only .claude/, .omc/ untracked
git log --oneline -1                     # df0ec591 feat(ai/agent_providers): visualize Test connection probe state per card
cargo nextest run -p ai 2>&1 | tail -3   # all pass
```

If anything diverges, STOP and report.

### Task 1: Anthropic wire types

**File:** Create `crates/ai/src/local_provider/adapters/anthropic/wire.rs`.

- [ ] **Step 1.1: Request types**

```rust
//! Serde types for the Anthropic Messages API.
//!
//! Coverage matches the subset we send and receive. Anything Anthropic defines
//! that we don't read uses `#[serde(default)]` for forward compatibility.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- Request ----------

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<AnthropicToolChoice>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    pub content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicRole { User, Assistant }

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicToolChoice { Auto, Any, Tool { name: String } }
```

- [ ] **Step 1.2: Streaming event types**

The wire format is `{"type":"<event_type>", ...rest}`. Tag on `type`.

```rust
// ---------- Streaming events ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicStreamEvent {
    MessageStart { message: StreamMessageStart },
    ContentBlockStart { index: u32, content_block: StreamContentBlock },
    ContentBlockDelta { index: u32, delta: StreamContentDelta },
    ContentBlockStop { index: u32 },
    MessageDelta { delta: MessageDeltaPayload, #[serde(default)] usage: Option<MessageDeltaUsage> },
    MessageStop,
    Ping,
    Error { error: AnthropicErrorEnvelope },
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamMessageStart {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<MessageStartUsage>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct MessageStartUsage {
    #[serde(default)] pub input_tokens: u64,
    #[serde(default)] pub output_tokens: u64,
    #[serde(default)] pub cache_creation_input_tokens: u64,
    #[serde(default)] pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamContentBlock {
    Text { #[serde(default)] text: String },
    ToolUse { id: String, name: String, #[serde(default)] input: Value },
    Thinking { #[serde(default)] thinking: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamContentDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { #[serde(default)] signature: String },     // extended thinking; ignored
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageDeltaPayload {
    #[serde(default)] pub stop_reason: Option<String>,
    #[serde(default)] pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct MessageDeltaUsage {
    #[serde(default)] pub output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicErrorEnvelope {
    #[serde(default)] pub r#type: String,
    #[serde(default)] pub message: String,
}

// ---------- Non-streaming response (summarizer) ----------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AnthropicMessageResponse {
    #[serde(default)] pub id: Option<String>,
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub content: Vec<ResponseContentBlock>,
    #[serde(default)] pub stop_reason: Option<String>,
    /// Top-level error envelope returned by 4xx/5xx responses with a JSON body.
    #[serde(default)] pub error: Option<AnthropicErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentBlock {
    Text { #[serde(default)] text: String },
    ToolUse { #[serde(default)] id: String, #[serde(default)] name: String, #[serde(default)] input: Value },
    Thinking { #[serde(default)] thinking: String },
}
```

- [ ] **Step 1.3: Tests** — add `#[cfg(test)] mod tests { ... }` inline (small enough to not need a sibling file) covering: each request variant round-trips through serde_json; each stream event variant deserializes from a sample chunk taken verbatim from Anthropic's docs.

- [ ] **Step 1.4: Build + commit**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai 2>&1 | tail -3
```

Commit:

```
feat(ai/local_provider/adapters/anthropic): add wire types

Phase 3a stage A. Serde types for Anthropic Messages API request,
streaming events (message_start / content_block_* / message_delta /
message_stop / error / ping), and non-streaming response. No adapter
wiring yet.
```

### Task 2: Tool reshape + request translator

**Files:**
- Modify: `crates/ai/src/local_provider/tools.rs` — add `tool_definitions_anthropic(...)`.
- Create: `crates/ai/src/local_provider/adapters/anthropic/request.rs`.
- Create: `crates/ai/src/local_provider/adapters/anthropic/request_tests.rs`.

- [ ] **Step 2.1: `tool_definitions_anthropic`**

Today's `tool_definitions(&[LocalTool]) -> Vec<wire::ToolDefinition>` returns the OpenAI wrapper. Add a parallel function in `tools.rs`:

```rust
use crate::local_provider::adapters::anthropic::wire::AnthropicToolDef;

pub fn tool_definitions_anthropic(tools: &[LocalTool]) -> Vec<AnthropicToolDef> {
    tools.iter().map(|t| AnthropicToolDef {
        name: t.name().to_string(),
        description: t.description().to_string(),
        input_schema: t.input_schema_json(),
    }).collect()
}
```

If today's helper extracts the schema out of the OpenAI-wrapper construction, factor out a `LocalTool::input_schema_json() -> Value` accessor — both adapters reuse it. Otherwise inline.

- [ ] **Step 2.2: `compose_anthropic_messages_request`**

This is the largest single file in Phase 3a (~250 lines). Mirror the OpenAI translator's structure (`request.rs::compose_chat_completion_request`):

```rust
//! Translator: LocalProviderInput → AnthropicMessagesRequest.
//! Mirrors compose_chat_completion_request but emits Anthropic's
//! alternating-role-with-content-blocks shape:
//! - system prompt is lifted to the top-level `system` field;
//! - assistant tool calls become `tool_use` content blocks;
//! - tool results become `tool_result` blocks on user-role messages;
//! - adjacent same-role messages are merged after walking the proto history.

use warp_multi_agent_api as api;

use super::wire::*;
use crate::local_provider::{
    compaction,
    config::LocalProviderConfig,
    prompt,
    request::{push_history_messages_into, LocalProviderInput},  // OpenAI walker stays; we add a parallel anthropic walker
    tools::{self, tool_definitions_anthropic, LocalTool},
};

pub fn compose_anthropic_messages_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> AnthropicMessagesRequest {
    let local_tools = super::super::super::request::enabled_local_tools_pub(
        input.supported_tools.iter().copied(),
        cfg,
    );
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tool_definitions_anthropic(&local_tools))
    } else {
        None
    };
    let tool_choice = tools.as_ref().map(|_| AnthropicToolChoice::Auto);

    let system = Some(prompt::compose_system_prompt(
        &local_tools.iter().map(|t| t.description()).collect::<Vec<_>>(),
        cfg.context_window.filter(|n| *n > 0),
        local_tools.contains(&LocalTool::ApplyFileDiffs),
    ));

    let mut blocks: Vec<RoleAndBlocks> = Vec::new();
    // Compaction projection (same logic as OpenAI translator — synthesize a
    // user "Continue..." / assistant <summary> pair when state is set).
    // ... walk input.tasks emitting RoleAndBlocks entries one per proto msg ...
    // ... then merge adjacent same-role entries into AnthropicMessage ...
    // ... then backfill orphan tool_use → tool_result placeholders ...
    // ... then append a final user-role message with input.user_query if set ...

    let messages = merge_alternating(blocks);
    let backfilled = backfill_orphan_tool_uses(messages, &input.action_results);

    AnthropicMessagesRequest {
        model: cfg.model_id.clone(),
        max_tokens: resolve_max_tokens(cfg),
        system,
        messages: backfilled,
        tools,
        tool_choice,
        stream: true,
    }
}

fn resolve_max_tokens(cfg: &LocalProviderConfig) -> u32 {
    match cfg.context_window {
        Some(n) if n >= 8192 => (n / 4).min(8192),
        _ => 4096,
    }
}

struct RoleAndBlocks { role: AnthropicRole, blocks: Vec<AnthropicContentBlock> }

fn merge_alternating(entries: Vec<RoleAndBlocks>) -> Vec<AnthropicMessage> { /* ... */ }
fn backfill_orphan_tool_uses(
    msgs: Vec<AnthropicMessage>,
    action_results: &std::collections::HashMap<String, String>,
) -> Vec<AnthropicMessage> { /* ... */ }
```

**Helpers needed from `request.rs`:** `enabled_local_tools` (today is `fn`, promote to `pub(crate)`); the `summarize_tool_call` / `summarize_tool_result` renderers (already produce the strings we want; reuse via `pub(crate)`).

The `push_history_messages_into` rename / shared walker is an aspiration — actually simpler: write a self-contained Anthropic walker in this file rather than refactoring OpenAI's. The walker handles each `Some(M::*)` proto variant directly, emitting the right `RoleAndBlocks`. ~80 lines.

- [ ] **Step 2.3: Sibling tests file**

`request_tests.rs` — exhaustive coverage mirroring the OpenAI translator's tests:

- System prompt is in top-level `system` field, not in `messages`.
- Simple user query → single user-role message with one text block.
- User query → assistant reply → second user query merges correctly (three messages, alternating).
- Tool call → tool_use block on assistant message.
- Tool call + result → tool_use on assistant, tool_result on next user message, merged with subsequent text into same user message.
- Orphan tool_use → placeholder tool_result spliced.
- Reasoning messages are dropped (no `thinking` blocks in request body — those are streaming-only).
- `max_tokens` resolves correctly: `context_window: None` → 4096, `8192` → 2048 (window/4 = 2048; min(2048, 8192) = 2048), `200_000` → 8192 (cap).
- `tools` field absent when `supports_tools = false`.
- Tools advertised in Anthropic shape (`name`, `description`, `input_schema` — no `function:` wrapper).
- `merge_alternating` on `[user, user, assistant, user, user, user]` → `[user, assistant, user]` with content blocks accumulated.
- Compaction projection synthesizes `[user_continue, assistant_summary]` (Anthropic shape) and drops pre-tail history.

Aim for 12–18 tests in this file.

- [ ] **Step 2.4: Build + tests + commit**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/anthropic): request translator

Phase 3a stage A. compose_anthropic_messages_request walks the same
LocalProviderInput the OpenAI translator consumes and emits Messages
API shape: system lifted out of messages, tool calls as tool_use
content blocks, tool results as tool_result blocks on user-role
messages, adjacent same-role messages merged. Orphan tool_use blocks
get placeholder tool_results spliced so Anthropic's strict-ordering
validator doesn't 400. max_tokens resolved from context_window with
sane defaults (4096 default; quarter of window capped at 8192).
```

### Task 3: URL helpers on `LocalProviderConfig`

- [ ] **Step 3.1: Add `messages_url` + `anthropic_models_url`**

Already sketched in the design refinement section. Three tests each, mirroring the existing `chat_completions_url_*` tests.

- [ ] **Step 3.2: Commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3
```

Commit:

```
feat(ai/local_provider/config): messages_url + anthropic_models_url

Phase 3a stage A. Computes `{base_url}/v1/messages` and
`{base_url}/v1/models` with idempotent /v1 path handling so users
pasting "https://api.anthropic.com/v1" don't get double-/v1 URLs.
```

---

## Stage B: SSE decoder

### Task 4: Extend `StreamDecoder` trait + `OpenAiSseAdapter` forwarder

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/mod.rs` — extend `StreamDecoder` trait.
- Modify: `crates/ai/src/local_provider/response.rs` — add `feed_event` to the `OpenAiSseAdapter` `StreamDecoder` impl.
- Modify: `crates/ai/src/local_provider/run.rs` — `synthesize_stream` passes `msg.event` through.

- [ ] **Step 4.1: Trait change**

```rust
pub trait StreamDecoder: Send {
    /// Feed an SSE data line. Default forwards to `feed_event(None, data)`.
    fn feed(&mut self, data: &str) -> Vec<api::ResponseEvent> {
        self.feed_event(None, data)
    }
    fn feed_event(&mut self, event_name: Option<&str>, data: &str) -> Vec<api::ResponseEvent>;
    fn finish(&mut self) -> Vec<api::ResponseEvent>;
    fn is_terminal(&self) -> bool;
    fn record_upstream_error(&mut self, msg: String);
}
```

OpenAi's impl in `response.rs`:

```rust
impl crate::local_provider::adapters::StreamDecoder for OpenAiSseAdapter {
    fn feed_event(&mut self, _event_name: Option<&str>, data: &str) -> Vec<api::ResponseEvent> {
        Self::feed(self, data)
    }
    fn finish(&mut self) -> Vec<api::ResponseEvent> { Self::finish(self) }
    fn is_terminal(&self) -> bool { Self::is_terminal(self) }
    fn record_upstream_error(&mut self, msg: String) { Self::record_upstream_error(self, msg) }
}
```

The trait's default `feed` impl preserves the inherent `feed(&str)` behavior for callers that still go through the single-arg path. Existing OpenAi tests stay green.

- [ ] **Step 4.2: Wire `synthesize_stream` to forward `msg.event`**

```rust
Poll::Ready(Some(Ok(Event::Message(msg)))) => {
    debug_dump_response_chunk(&msg.data);
    let event_name = if msg.event.is_empty() || msg.event == "message" {
        None
    } else {
        Some(msg.event.as_str())
    };
    for ev in decoder.feed_event(event_name, &msg.data) {
        pending.push_back(ev);
    }
    if decoder.is_terminal() {
        for ev in decoder.finish() {
            pending.push_back(ev);
        }
        closed = true;
    }
    continue;
}
```

- [ ] **Step 4.3: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3   # unchanged count
```

Commit:

```
refactor(ai/local_provider/adapters): add feed_event to StreamDecoder

Phase 3a stage B. Extends StreamDecoder with feed_event(event_name,
data) so Anthropic's named SSE events can be dispatched on. Default
fn feed() forwards to feed_event(None, data), preserving the existing
OpenAi single-arg call path. synthesize_stream now threads the SSE
event-name through to the decoder.
```

### Task 5: `AnthropicSseDecoder`

**Files:**
- Create: `crates/ai/src/local_provider/adapters/anthropic/response.rs`.
- Create: `crates/ai/src/local_provider/adapters/anthropic/response_tests.rs`.

- [ ] **Step 5.1: Decoder skeleton**

Mirror `OpenAiSseAdapter`'s public surface verbatim — same `with_ids` constructor, same `skip_create_task`, same `is_terminal`, same `finish` emitting the closing transaction + `StreamFinished`. Internal differences:

- State enum: `Streaming → Finishing (on message_stop) → Done`; `Errored`.
- Per-index content-block table: `HashMap<u32, ContentBlockState>` where state is `{ kind: Text | ToolUse { id, name, args_acc } | Thinking, opened_message_id: Option<String> }`.
- Captured `stop_reason` from `MessageDelta` → mapped via `map_anthropic_stop_reason` to the same `Reason::*` variants OpenAi uses (`end_turn` / `tool_use` → `Done`; `max_tokens` → `MaxTokenLimit`; everything else → `Other`).
- Captured `AnthropicUsage` from `message_start` + `message_delta` → `TokenUsage` on `finish`.
- `event: "message"` (the default per the EventSource spec) shouldn't happen in Anthropic streams but if it does, treat as no-op (or `error`-prefix and warn).

```rust
pub struct AnthropicSseDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,
    blocks: HashMap<u32, BlockState>,
    captured_stop_reason: Option<String>,
    captured_model: Option<String>,
    captured_usage: AnthropicUsage,
}

impl crate::local_provider::adapters::StreamDecoder for AnthropicSseDecoder {
    fn feed_event(&mut self, event_name: Option<&str>, data: &str) -> Vec<api::ResponseEvent> {
        if matches!(self.state, State::Done | State::Errored) { return vec![]; }

        // Open Init/Begin/CreateTask on the first event of any kind.
        let mut out = self.ensure_prelude();

        // Parse to the typed enum. The tag-on-type strategy makes the
        // event_name argument redundant when the JSON is well-formed —
        // Anthropic embeds the type in both `event:` and the JSON itself.
        // Still pass event_name through for resilience (a server that
        // omits `type` in JSON but sets `event:` would otherwise fail).
        let parsed: Result<AnthropicStreamEvent, _> = match event_name {
            Some(name) if !has_type_field(data) => {
                // Synthesize type from event_name; rare path.
                serde_json::from_str(&inject_type(name, data))
            }
            _ => serde_json::from_str(data),
        };
        let Ok(event) = parsed else { /* set Errored + return InternalError reason on finish */ };

        match event {
            AnthropicStreamEvent::MessageStart { message } => self.on_message_start(message),
            AnthropicStreamEvent::ContentBlockStart { index, content_block } => self.on_block_start(index, content_block),
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => self.on_block_delta(index, delta, &mut out),
            AnthropicStreamEvent::ContentBlockStop { index } => self.on_block_stop(index, &mut out),
            AnthropicStreamEvent::MessageDelta { delta, usage } => self.on_message_delta(delta, usage),
            AnthropicStreamEvent::MessageStop => {
                self.flush_pending_blocks(&mut out);
                self.state = State::Done;
            }
            AnthropicStreamEvent::Ping => {} // no-op
            AnthropicStreamEvent::Error { error } => {
                self.upstream_error = Some(format!("{}: {}", error.r#type, error.message));
                self.state = State::Errored;
            }
        }
        out
    }
    /* finish / is_terminal / record_upstream_error mirror OpenAi */
}
```

The exact internals (`on_block_start`, `on_block_delta`, `on_block_stop`, `flush_pending_blocks`, `ensure_prelude`) are mechanical and mirror the OpenAi adapter's helpers — adapt the naming, swap "text/reasoning" buffer pairs for an indexed block map.

**Tool-call emission:** when `content_block_stop` fires for a `ToolUse` block, parse the accumulated `args_acc` (concatenated `partial_json` deltas) into JSON and emit a single `AddMessagesToTask` event with the proto `Message::ToolCall` variant. This is identical to OpenAi's `build_tool_call_event` path — reuse `tools::translate_openai_tool_call` (it parses JSON arguments by the v1 tool name; the resulting proto Message::ToolCall variant is wire-format-agnostic).

**Reasoning/thinking:** `Thinking` blocks and `ThinkingDelta` events surface as `AgentReasoning` proto messages, identical to OpenAi's `reasoning_content` path. Phase 3a includes this — it's a small extension, and Claude 4.x extended thinking output is non-trivial enough that hiding it hurts UX.

- [ ] **Step 5.2: Sibling tests file**

`response_tests.rs` — driving the decoder with hand-crafted event sequences (no HTTP, no SSE parser — feed JSON strings directly with `feed_event(Some("..."), &json)`):

- `message_start → content_block_start(text) → content_block_delta(text_delta, "Hello") → content_block_delta(text_delta, " world") → content_block_stop → message_delta(stop_reason=end_turn, usage) → message_stop`
  produces the canonical event sequence: Init, BeginTransaction, CreateTask, AddMessagesToTask(AgentOutput "Hello"), AppendToMessageContent(AgentOutput " world"), CommitTransaction, Finished(Done).
- Tool-use block: `content_block_start(tool_use, id, name) → content_block_delta(input_json_delta, '{"path":') → content_block_delta(input_json_delta, '"x"}') → content_block_stop` produces a single `ToolCall` AddMessagesToTask event.
- Thinking block: emits `AgentReasoning` events analogously to text.
- `error` event surfaces as `Finished{InternalError("overloaded_error: ...")}`.
- `ping` is silently consumed.
- Two simultaneous content blocks at different indices (index=0 text, index=1 tool_use) interleave their deltas correctly.
- `max_tokens` stop_reason → `Reason::MaxTokenLimit`.
- Premature EOF (no `message_stop`, decoder.finish() called) → Rollback + `Finished{InternalError("stream ended without finish_reason")}`.
- Usage captured from `message_start` + `message_delta` is correctly merged into the `TokenUsage` field of `Finished`.
- `with_ids` round-trips the ids in the Init event.
- `skip_create_task = true` (via the constructor analog) suppresses CreateTask.

Aim for 15+ tests.

- [ ] **Step 5.3: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/anthropic): SSE decoder

Phase 3a stage B. AnthropicSseDecoder consumes the Messages API
streaming event sequence (message_start / content_block_* /
message_delta / message_stop / ping / error) and emits the same
ResponseEvent shape OpenAiSseAdapter does — controller is unaware of
which adapter produced the events. Text blocks → AgentOutput,
thinking blocks → AgentReasoning, tool_use blocks → ToolCall.
message_stop is the terminator (no [DONE] equivalent).
```

---

## Stage C: Adapter impl + dispatch + live test

### Task 6: `AnthropicAdapter` impl

**Files:**
- Create: `crates/ai/src/local_provider/adapters/anthropic/mod.rs`.

- [ ] **Step 6.1: Impl + module wiring**

```rust
//! Anthropic Messages API adapter. Phase 3a.
//!
//! Wire-format differences from OpenAi handled here:
//! - x-api-key auth + anthropic-version header (not Bearer).
//! - Top-level system field, alternating user/assistant roles, content blocks.
//! - Streaming events are named (event: message_start, etc.); decoder
//!   dispatches on event_name via feed_event.

pub mod wire;
pub mod request;
pub mod response;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
#[cfg(test)]
#[path = "response_tests.rs"]
mod response_tests;

use super::{AdapterError, AgentProviderApiType, LocalProviderConfig, LocalProviderInput,
            ProviderAdapter, StreamDecoder, StreamIds, SummarizerError, SummarizerInput};

use request::compose_anthropic_messages_request;
use response::AnthropicSseDecoder;
use wire::{AnthropicMessagesRequest, AnthropicMessageResponse, ResponseContentBlock};

pub struct AnthropicAdapter;

const ANTHROPIC_VERSION: &str = "2023-06-01";

impl ProviderAdapter for AnthropicAdapter {
    fn api_type(&self) -> AgentProviderApiType { AgentProviderApiType::Anthropic }

    fn build_chat_request(
        &self, input: &LocalProviderInput, cfg: &LocalProviderConfig, http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.messages_url()?;
        let body = compose_anthropic_messages_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        let mut rb = http.post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header("anthropic-version", ANTHROPIC_VERSION)
            .body(body_json);
        if let Some(k) = cfg.api_key.as_deref().filter(|s| !s.is_empty()) {
            rb = rb.header("x-api-key", k);
        }
        Ok(rb)
    }

    fn create_stream_decoder(
        &self, ids: Option<StreamIds>, skip_create_task: bool,
    ) -> Box<dyn StreamDecoder> {
        let mut d = match ids {
            Some(ids) => AnthropicSseDecoder::with_ids(ids.conversation_id, ids.request_id, ids.run_id, ids.task_id),
            None => AnthropicSseDecoder::new(),
        };
        if skip_create_task { d.skip_create_task(); }
        Box::new(d)
    }

    fn build_summarizer_request(
        &self, input: &SummarizerInput, cfg: &LocalProviderConfig, http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.messages_url()?;
        let body = build_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        let mut rb = http.post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .header("anthropic-version", ANTHROPIC_VERSION)
            .body(body_json);
        if let Some(k) = cfg.api_key.as_deref().filter(|s| !s.is_empty()) {
            rb = rb.header("x-api-key", k);
        }
        Ok(rb)
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: AnthropicMessageResponse = serde_json::from_str(body)
            .map_err(|e| SummarizerError::DecodeResponse(format!("{e}: {}",
                crate::local_provider::run::first_chars(body, 200))))?;
        if let Some(err) = parsed.error {
            return Err(SummarizerError::UpstreamErrorEnvelope(format!("{}: {}", err.r#type, err.message)));
        }
        let text = parsed.content.into_iter().filter_map(|b| match b {
            ResponseContentBlock::Text { text } => Some(text),
            ResponseContentBlock::Thinking { thinking } => Some(thinking),
            ResponseContentBlock::ToolUse { .. } => None,
        }).collect::<Vec<_>>().join("\n").trim().to_string();
        if text.is_empty() { Err(SummarizerError::NoContent) } else { Ok(text) }
    }

    fn build_probe_request(
        &self, cfg: &LocalProviderConfig, http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.anthropic_models_url()?;
        let mut rb = http.get(url).header("anthropic-version", ANTHROPIC_VERSION);
        if let Some(k) = cfg.api_key.as_deref().filter(|s| !s.is_empty()) {
            rb = rb.header("x-api-key", k);
        }
        Ok(rb)
    }
}

fn build_summarizer_body(input: &SummarizerInput, cfg: &LocalProviderConfig) -> AnthropicMessagesRequest {
    // Translate the OpenAI-format ChatMessage list the compaction pipeline
    // produces into Anthropic shape. system messages lift; user/assistant
    // map to user/assistant content-block lists with a single text block.
    /* ~30 lines */
}
```

- [ ] **Step 6.2: Tests** — add 4–6 tests in `request_tests.rs` (already created):
  - Probe URL is `{base_url}/v1/models` with `x-api-key` and `anthropic-version` headers (no Bearer).
  - Probe URL idempotent for base ending in `/v1`.
  - Chat URL is `{base_url}/v1/messages`.
  - Build summarizer body strips tool advertisements (none should be in summarizer requests).
  - Auth header absent when `api_key = None`.
  - Default `anthropic-version` always present.

- [ ] **Step 6.3: Build + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/anthropic): implement AnthropicAdapter

Phase 3a stage C. Implements the ProviderAdapter trait. Auth via
x-api-key + anthropic-version header (not Bearer). Probe targets
{base_url}/v1/models (HTTP 2xx == success). Streaming chat targets
{base_url}/v1/messages. Summarizer reuses the same endpoint with
stream:false. select_adapter not yet flipped — Task 7 does that.
```

### Task 7: Flip `select_adapter` + enable UI chip

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/mod.rs`.
- Modify: `crates/ai/src/local_provider/adapters/adapters_tests.rs`.
- Modify: `app/src/settings_view/agent_providers_widget.rs` (if a disabled-guard exists for Anthropic).

- [ ] **Step 7.1: Module + dispatch**

```rust
pub mod anthropic;
pub use anthropic::AnthropicAdapter;

pub fn select_adapter(api_type: AgentProviderApiType)
    -> Result<Box<dyn ProviderAdapter>, AdapterError>
{
    use AgentProviderApiType::*;
    match api_type {
        OpenAi    => Ok(Box::new(OpenAiAdapter)),
        Anthropic => Ok(Box::new(AnthropicAdapter)),
        OpenAiResp | Gemini | Ollama | DeepSeek => Err(AdapterError::UnsupportedApiType(api_type)),
    }
}
```

- [ ] **Step 7.2: Update `adapters_tests.rs`**

```rust
#[test]
fn select_adapter_returns_openai_for_openai_api_type() { /* unchanged */ }

#[test]
fn select_adapter_returns_anthropic_for_anthropic_api_type() {
    let a = select_adapter(AgentProviderApiType::Anthropic).expect("ok");
    assert_eq!(a.api_type(), AgentProviderApiType::Anthropic);
}

#[test]
fn select_adapter_errors_for_each_unimplemented_variant() {
    for ty in [
        AgentProviderApiType::OpenAiResp,
        AgentProviderApiType::Gemini,
        AgentProviderApiType::Ollama,
        AgentProviderApiType::DeepSeek,
    ] { /* expect UnsupportedApiType */ }
}
```

- [ ] **Step 7.3: Settings UI**

Open `agent_providers_widget.rs`, locate the api-type chip / dropdown renderer. If `Anthropic` is currently marked disabled / "Coming soon", remove that guard. If the Phase 2 widget already lists all variants without gating (relying on the dispatch-time error for feedback), this step is a no-op — confirm and move on.

The probe-button code already routes through `select_adapter`, so flipping the match arm makes the probe work for Anthropic providers immediately.

- [ ] **Step 7.4: Build + tests + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo nextest run -p warp --lib 2>&1 | tail -3
cargo clippy -p ai -p warp --lib --tests -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters): flip select_adapter for Anthropic

Phase 3a stage C. Anthropic api_type now dispatches to
AnthropicAdapter instead of returning UnsupportedApiType. Settings UI
chip un-gated. Test connection probe and chat both work end-to-end
against api.anthropic.com with a real key.
```

### Task 8: Manual smoke test + documentation

- [ ] **Step 8.1: Live test against `api.anthropic.com`**

Documented in the commit body for Task 7 or 8 — requires a real Anthropic API key. Run the app, open Settings → AI → Custom AI Providers:

1. Add provider:
   - Name: `Anthropic`
   - API type: `Anthropic`
   - Base URL: `https://api.anthropic.com`
   - API key: a working `sk-ant-api03-...` key
   - Model id: `claude-sonnet-4-6` (one model row; default context window from Anthropic docs)
2. Click **Test connection** — expect green check.
3. Open a new conversation, pick `Anthropic / claude-sonnet-4-6` from the model picker.
4. Send: `Read the first 50 lines of Cargo.toml and summarize them.` — expect streaming assistant text, a tool call to `read_files`, a tool result rendered back, and a final summary.
5. Confirm the conversation persists across app restart; second turn continues correctly.
6. Edit the base URL to a wrong host, click Test connection — expect red failure with HTTP-level reason.
7. Remove the API key, click Test connection — expect red failure (401 or "missing x-api-key" from Anthropic).

- [ ] **Step 8.2: Update spec docs**

- README `Status` table: change Phase 3a row to ✅ shipped, with date.
- README "What landed" section: add a bullet under user-visible covering Anthropic native support.
- README "Architecture" section: add Anthropic adapter file paths.
- design.md §9: mark Phase 3a row ✅.

Two-line commit:

```
docs(specs/multi-local-llm): mark Phase 3a (Anthropic adapter) shipped
```

---

## Final verification

- [ ] **Verification 1: Sweeps**

```bash
echo "=== Anthropic submodule wired ==="
grep -n "pub mod anthropic" crates/ai/src/local_provider/adapters/mod.rs

echo "=== select_adapter flipped ==="
grep -nA 1 "Anthropic =>" crates/ai/src/local_provider/adapters/mod.rs   # expect: Ok(Box::new(AnthropicAdapter))

echo "=== Anthropic auth not Bearer ==="
grep -rn "bearer_auth\|x-api-key" crates/ai/src/local_provider/adapters/   # x-api-key appears in anthropic/mod.rs, not openai.rs

echo "=== feed_event in StreamDecoder ==="
grep -n "fn feed_event" crates/ai/src/local_provider/adapters/mod.rs

echo "=== Messages URL helpers ==="
grep -n "fn messages_url\|fn anthropic_models_url" crates/ai/src/local_provider/config.rs

echo "=== Tool definitions Anthropic ==="
grep -n "tool_definitions_anthropic" crates/ai/src/local_provider/tools.rs
```

- [ ] **Verification 2: Build + tests + clippy**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -5     # ~+30 tests
cargo nextest run -p warp --lib 2>&1 | tail -5
cargo clippy -p ai --all-targets --all-features -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

- [ ] **Verification 3: Manual smoke**

Per Task 8.1 above — real-key live test against `api.anthropic.com`.

- [ ] **Verification 4: Final reviewer + push**

Dispatch `oh-my-claudecode:code-reviewer` for the full Phase 3a diff. Stop before push; user reviews, then pushes manually.

---

## Risks & open questions

1. **`max_tokens` heuristic is too low for long-output tasks.** If users hit `stop_reason: "max_tokens"` frequently on Claude Sonnet 4.x, surface in the provider model UI (Phase 4b). Mitigation today: the default `4096` matches Anthropic's docs-recommended starting value; users with large `context_window` get the quarter-of-window formula.

2. **Strict role alternation might break existing conversations.** Local-provider conversations migrated from a Phase 1 OpenAI provider have tool_results stored as `Message::ToolCallResult` proto messages. The translator turns those into `tool_result` blocks on user-role messages — but if a conversation has a sequence like `assistant_text → assistant_tool_call → tool_result → assistant_text → assistant_text` (back-to-back assistant turns from a buggy multi-turn loop), the merge pass folds them into one assistant message with concatenated content blocks. Anthropic accepts that. The risk is in the *opposite* shape (`user → user → user` without separators), which **shouldn't** be reachable from valid local-provider history. Defensive: the merge pass concatenates rather than rejects.

3. **Extended thinking surfacing.** Anthropic emits `thinking` content blocks for Claude 4.x with `extended thinking enabled`. We render these as `AgentReasoning` — but we don't *request* extended thinking (no `thinking: { type: "enabled", budget_tokens: ... }` field in the request body). Phase 3a is fine without it; Phase 4 polish can add a per-model "enable extended thinking" toggle. The decoder handles `thinking_delta` events anyway so when the model emits them spontaneously (some 4.x preview models did) we don't lose them.

4. **Test-connection probe relies on `/v1/models` endpoint.** Available since Nov 2024; if a user configures a Bedrock-front-end or self-hosted Claude relay that doesn't implement `/v1/models`, the probe will fail. Probe success isn't required for chat to work; users can ignore a failing probe and the chat path still runs. We may want to document this in the probe failure message — defer to Phase 4 polish if it becomes a support issue.

5. **`feed_event` trait change is a Phase 3a commitment.** Existing OpenAi tests use `feed(&str)` directly; the default trait method preserves that — no test churn. Future adapters (Gemini, etc.) MAY also need event-name dispatch (Gemini's SSE format is similar to OpenAI's anonymous JSON; should be fine). If Phase 3c surfaces a third event-shape we hadn't anticipated, revisit the trait.

6. **Anthropic API-key safety.** Anthropic keys are valuable — exposed via misconfigured base_url-to-third-party-host is a real risk if the user pastes their key into a relay they don't trust. Out of scope for Phase 3a; settings UI doesn't warn on non-`api.anthropic.com` base URLs. Phase 4 may add a "non-canonical base URL" hint badge.

7. **Tool result content escaping.** Anthropic's `tool_result.content` accepts a string OR an array of content blocks. Phase 3a always sends a string (the rendered output from `summarize_tool_result`). Some images-from-tools paths (Phase 4c multimodal) will need the array form. Not a concern today.

8. **Live test requires a real key — gate by env var so CI doesn't fail.** Don't add the live test to `cargo nextest`; it's a manual gate per the README precedent (`v0.1.0`'s manual smoke). Phase 4 may add an integration test driving a mock Anthropic server (similar to `crates/ai/tests/local_provider_integration.rs` for OpenAI).

---

## Next plan (Phase 3b — Ollama-native adapter)

After Phase 3a ships green, Phase 3b targets the second-most-common BYOP user (Ollama). The native Ollama API at `/api/chat` has subtle differences from its OpenAI-compatible `/v1/chat/completions` endpoint — most notably better tool-call streaming semantics and a different message shape. Plan written after 3a is approved + executed.
