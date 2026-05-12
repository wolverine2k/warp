# Multi-Local-LLM — Phase 3d (DeepSeek-native Adapter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a native `DeepSeekAdapter` targeting `POST {base_url}/chat/completions` and flip `select_adapter` for `AgentProviderApiType::DeepSeek` from `Err(UnsupportedApiType)` to a real impl. **Final native adapter of Phase 3.** Differs from `OpenAi` in exactly one way that matters: the **`reasoning_content` channel** that `deepseek-reasoner` emits as an additional field alongside `content` on assistant messages — the decoder surfaces it as `AgentReasoning` proto messages so the Warp UI can render the chain-of-thought blocks distinctly from final-answer text.

**Architecture:** Four logical stages, atomic in one PR (split into 3d-i / 3d-ii / 3d-iii / 3d-iv if review prefers):

- **Stage A (Tasks 1–2)** — Wire types (clone OpenAI's request types + add `reasoning_content` field on inbound assistant messages and inbound `delta` chunks) and request translator (mirrors the OpenAI translator's behavior; **drops `AgentReasoning` from outbound history** — DeepSeek's API rejects `reasoning_content` on inbound `messages` with HTTP 400).
- **Stage B (Task 3)** — `DeepSeekSseDecoder` consumes anonymous SSE `data:` chunks (same wire framing as OpenAI). New state machine: opens a shared `AgentReasoning` message on first `delta.reasoning_content`, finalizes it and opens a shared `AgentOutput` on first `delta.content`. `[DONE]` is the terminator (same as OpenAI).
- **Stage C (Tasks 4–5)** — `DeepSeekAdapter` impl + flip `select_adapter`. **No new URL helpers** — DeepSeek's endpoints (`/chat/completions` for chat, `/models` for the probe) match OpenAI's exactly, so the adapter reuses `cfg.chat_completions_url()` and `cfg.models_list_url()` directly.
- **Stage D (Task 6)** — Manual smoke against `api.deepseek.com` with a real DeepSeek API key + spec doc updates.

**Branch:** `multi-local-llm`. Forks from `08f57d6b` (Phase 3c plan-commit tip — the latest commit on origin after Phase 3c shipped). Estimated ~600 lines net code (~150 wire types + 200 translator + 220 decoder + 80 adapter glue — DeepSeek's body is OpenAI-compatible so the translator is mostly a thin clone), ~4 hours of subagent-driven work. **Smaller than 3c** because URL helpers are reused and the wire types are mostly cloned from OpenAI.

**Spec references:**
- `specs/multi-local-llm/design.md` §9 (Phase 3d row — gets the "code complete" status flip in Task 6).
- `specs/multi-local-llm/plan-phase-3c.md` §"Next plan (Phase 3d — DeepSeek adapter)" — superseded by this document. **Note:** that paragraph said *"the model expects the prior turn's `reasoning_content` to be round-tripped back in the history"* — that's incorrect. DeepSeek's official docs state the API returns HTTP 400 if `reasoning_content` appears on inbound `messages`. This plan reflects the correct behavior: emit `AgentReasoning` from the decoder, drop it from the translator's outbound history.
- DeepSeek API docs: <https://api-docs.deepseek.com/guides/reasoning_model>.
- Reasoning-model usage notes (DeepSeek): <https://api-docs.deepseek.com/guides/reasoning_model#api-parameters>.

**Test gate:** All existing `cargo nextest run -p ai` tests pass (567/567 baseline at HEAD `08f57d6b`); new DeepSeek-specific tests added (~55). Manual smoke: a single DeepSeek provider configured with `base_url = https://api.deepseek.com`, api_type `DeepSeek`, model `deepseek-reasoner`, runs a turn that streams reasoning text (rendered as `AgentReasoning`), then final-answer text (rendered as `AgentOutput`), and emits a `Finished` event. "Test connection" probe succeeds.

**Out of Phase 3d (deferred to Phase 4):**
- `/models` fetch button (Phase 4a).
- models.dev catalog sync + quick-add chips (Phase 4b).
- Multimodal capabilities (image / pdf / audio per model) — Phase 4c.
- Dedicated compaction model routing — Phase 4d.
- `tool_choice` knob exposure (DeepSeek-Chat supports tools; DeepSeek-Reasoner doesn't — surfacing this per-model is a Phase 4 polish item).
- Token-usage cache-hit fields (`prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`) surfaced into the UI — Phase 4 polish.

---

## Design refinement

### Endpoint + auth

- **Chat:** `POST {base_url}/chat/completions`. **Identical to OpenAI** — DeepSeek's wire shape is intentionally OpenAI-compatible. Reuses `cfg.chat_completions_url()`.
- **Probe:** `GET {base_url}/models`. Same as OpenAI. Reuses `cfg.models_list_url()`.
- **Auth:** `Authorization: Bearer <sk-...>` request header. Same as OpenAI's `apply_openai_headers` pattern — DeepSeek uses Bearer auth, not a custom header.
- **Default `base_url`:** `https://api.deepseek.com` (no `/v1` prefix needed — DeepSeek's path layout doesn't use one, but the OpenAI-shaped `chat_completions_url` will still join `/chat/completions` correctly because `Url::join` operates on the path-as-given).

  **Important caveat:** DeepSeek also accepts `https://api.deepseek.com/v1` as a base URL. The OpenAI URL helpers (`chat_completions_url`, `models_list_url`) handle the trailing-`/v1` case correctly because they do path-join, not version-prepend. **Recommendation:** the smoke test uses the bare `https://api.deepseek.com` form so we exercise the non-`/v1` path.

### Streaming format: SSE (anonymous chunks)

**Same SSE shape as OpenAI**: each `data: {...}` line is a partial `ChatCompletionChunk` JSON object; no `event:` discriminator. The stream ends with a literal `data: [DONE]\n\n` line (same as OpenAI — NOT like Gemini which uses `finishReason` inside the last chunk).

The decoder transitions to `State::Done` on the `[DONE]` line. `streaming_format()` inherits the SSE default. **No `run.rs` changes.** The existing `synthesize_sse_stream` drive loop handles it without modification.

### Request body shape (OpenAI-compatible, minus reasoning)

```jsonc
{
  "model": "deepseek-reasoner",
  "stream": true,
  "messages": [
    {"role": "system",    "content": "..."},
    {"role": "user",      "content": "..."},
    {"role": "assistant", "content": "The answer is..."},
    {"role": "tool",      "content": "...tool result text...",
     "tool_call_id": "call_abc"}
  ],
  "tools": [
    {"type": "function",
     "function": {"name": "read_files", "description": "...", "parameters": {...}}}
  ]
}
```

**Differences from OpenAI's body:**

1. **`reasoning_content` is FORBIDDEN on inbound `messages`.** DeepSeek's API returns HTTP 400 with `Error code: 400 - {'error': {'message': 'The last message of deepseek-reasoner must be a user message, or an assistant message with prefix mode on (refer to https://api-docs.deepseek.com/guides/chat_prefix_completion).', 'type': 'invalid_request_error', 'param': None, 'code': 'invalid_request_error'}}` (or similar) when `reasoning_content` appears in the request. **The translator MUST drop `AgentReasoning` proto messages from outbound history** — same behavior as every other adapter (none of the other native adapters round-trip reasoning either).
2. **`max_tokens`, `temperature`, `top_p` etc.** are accepted but not required. Phase 3d omits all of them (matches the OpenAI translator's current behavior; Phase 4 polish can expose them per-model).
3. **`tool_choice` defaults to `"auto"`.** Phase 3d does not set it explicitly (matches OpenAI's translator behavior).
4. **Tools array shape is identical to OpenAI's** (`{type: "function", function: {name, description, parameters}}`).
5. **Tool-call IDs are emitted by the server** in the streaming response (same as OpenAI). The translator round-trips them back on assistant messages with tool_calls and on subsequent `role: "tool"` messages via `tool_call_id`.
6. **Some DeepSeek models don't support tools.** `deepseek-reasoner` does NOT support function calls (tools); `deepseek-chat` does. Phase 3d emits tools unconditionally based on `cfg.supports_tools` — the user is responsible for setting `supports_tools = false` for `deepseek-reasoner` providers. Phase 4 polish can add per-model capability gating in the settings UI.

### Streaming response shape (with reasoning)

Each `data:` line is a `ChatCompletionChunk`:

```jsonc
// First chunks emit reasoning_content only (deepseek-reasoner):
data: {"id":"...","object":"chat.completion.chunk","model":"deepseek-reasoner",
  "choices":[{"index":0,
              "delta":{"role":"assistant","reasoning_content":"Let me think..."},
              "finish_reason":null}]}

data: {"id":"...","object":"chat.completion.chunk","model":"deepseek-reasoner",
  "choices":[{"index":0,
              "delta":{"reasoning_content":" about this."},
              "finish_reason":null}]}

// Then content begins streaming (reasoning is now complete):
data: {"id":"...","object":"chat.completion.chunk","model":"deepseek-reasoner",
  "choices":[{"index":0,
              "delta":{"content":"The answer is "},
              "finish_reason":null}]}

data: {"id":"...","object":"chat.completion.chunk","model":"deepseek-reasoner",
  "choices":[{"index":0,
              "delta":{"content":"42."},
              "finish_reason":null}]}

// Tool call (deepseek-chat only — deepseek-reasoner has no tool calls):
data: {"id":"...","model":"deepseek-chat",
  "choices":[{"index":0,
              "delta":{"tool_calls":[
                {"index":0,
                 "id":"call_abc",
                 "type":"function",
                 "function":{"name":"read_files",
                             "arguments":"{\"paths\":[\""}}]},
              "finish_reason":null}]}

// Tool call arguments continue (streaming):
data: {"id":"...","choices":[{"index":0,
  "delta":{"tool_calls":[{"index":0,"function":{"arguments":"Cargo.toml\"]}"}}]},
  "finish_reason":null}]}

// Final chunk with finish_reason + usage:
data: {"id":"...","choices":[{"index":0,
  "delta":{},"finish_reason":"stop"}],
  "usage":{"prompt_tokens":50,"completion_tokens":120,"total_tokens":170,
           "prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50,
           "completion_tokens_details":{"reasoning_tokens":80}}}

// Terminator:
data: [DONE]
```

**Key wire-format quirks specific to DeepSeek:**

- `delta.reasoning_content` is a STRING that streams incrementally — same pattern as `delta.content`. Both fields can appear on the same chunk in theory, but in practice `deepseek-reasoner` emits all reasoning chunks first, then all content chunks. **The decoder's state machine assumes reasoning-precedes-content but tolerates interleaving** (each delta is dispatched on its fields).
- `delta.content` and `delta.reasoning_content` are mutually independent — the decoder must check both per chunk.
- `usage.completion_tokens_details.reasoning_tokens` reports the token cost of the reasoning channel. Phase 3d folds this into `captured_output_tokens` along with the regular `completion_tokens` (matches OpenAI's accounting). Phase 4 polish can split them in the UI.
- `usage.prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` are DeepSeek-specific cache-hit counters. Phase 3d ignores them (deserialized via `#[serde(default)]` but not surfaced). Phase 4 polish can wire them into the UI.
- **`tool_calls` streaming is identical to OpenAI's** — the same `id` / `type` / `function: {name, arguments}` shape, with `arguments` arriving as fragments. Tool calls are NEVER emitted on `deepseek-reasoner` (the model has no tool-use channel).
- **Error envelope on 4xx** matches OpenAI's `{"error": {"message": "...", "type": "...", "code": "..."}}` shape. Reuse `OpenAi`'s error-handling pattern in the decoder.

### Decoder design

Architecturally similar to `OpenAiSseAdapter` but with an additional reasoning-channel path. The state machine:

```rust
pub struct DeepSeekSseDecoder {
    state: State,
    task_id: String,
    conversation_id: String,
    request_id: String,
    run_id: String,
    upstream_error: Option<String>,
    sent_init: bool,
    sent_begin: bool,
    sent_create_task: bool,

    /// One shared open AgentReasoning message across the turn — opens on
    /// first `delta.reasoning_content`, gets appends on subsequent
    /// reasoning chunks. Never closed explicitly; the controller groups
    /// it with the next AgentOutput at render time.
    reasoning_message_id: Option<String>,

    /// One shared open AgentOutput message across the turn — opens on
    /// first `delta.content`, gets appends on subsequent text chunks.
    text_message_id: Option<String>,

    /// Pending tool-call accumulator (matches OpenAI's pattern — args
    /// arrive in fragments, finalize on subsequent chunk index change or
    /// stream end). Reuses the same per-tool-call buffer state OpenAI's
    /// decoder uses.
    pending_tool_calls: Vec<PendingToolCall>,

    captured_finish_reason: Option<String>,
    captured_model: Option<String>,
    captured_input_tokens: u64,
    captured_output_tokens: u64,
}

enum State { Streaming, Done, Errored }
```

**Per-chunk dispatch** (called via the default `feed` path — DeepSeek's SSE has no `event:` names):

```rust
fn feed_event(&mut self, _event_name: Option<&str>, data: &str)
    -> Vec<api::ResponseEvent>
{
    if matches!(self.state, State::Done | State::Errored) { return vec![]; }
    let trimmed = data.trim();
    if trimmed.is_empty() { return vec![]; }

    // [DONE] terminator — same as OpenAI.
    if trimmed == "[DONE]" {
        self.state = State::Done;
        return vec![];
    }

    let mut out = self.ensure_prelude();
    let chunk: DeepSeekChatChunk = match serde_json::from_str(trimmed) {
        Ok(c) => c,
        Err(e) => {
            self.state = State::Errored;
            self.upstream_error.get_or_insert_with(|| format!("malformed DeepSeek chunk: {e}"));
            return out;
        }
    };

    // Top-level error envelope (rare; some DeepSeek versions emit this
    // mid-stream as a JSON envelope rather than an HTTP 4xx).
    if let Some(err) = chunk.error {
        self.upstream_error = Some(format!("{}: {}", err.kind, err.message));
        self.state = State::Errored;
        return out;
    }

    if self.captured_model.is_none() {
        if let Some(m) = chunk.model.filter(|s| !s.is_empty()) {
            self.captured_model = Some(m);
        }
    }

    if let Some(usage) = chunk.usage {
        self.captured_input_tokens = usage.prompt_tokens.max(self.captured_input_tokens);
        self.captured_output_tokens = usage.completion_tokens.max(self.captured_output_tokens);
    }

    if let Some(choice) = chunk.choices.into_iter().next() {
        if let Some(delta) = choice.delta {
            // Reasoning channel — open or append AgentReasoning.
            if let Some(reasoning) = delta.reasoning_content {
                if !reasoning.is_empty() {
                    self.append_reasoning(&reasoning, &mut out);
                }
            }
            // Content channel — open or append AgentOutput.
            if let Some(content) = delta.content {
                if !content.is_empty() {
                    self.append_text(&content, &mut out);
                }
            }
            // Tool calls — accumulate fragments by index.
            if let Some(tool_calls) = delta.tool_calls {
                for tc in tool_calls {
                    self.absorb_tool_call_fragment(tc, &mut out);
                }
            }
        }
        if let Some(reason) = choice.finish_reason {
            self.captured_finish_reason = Some(reason);
            // Don't transition to Done here — wait for [DONE]. Matches
            // OpenAI's pattern: finish_reason and [DONE] arrive in
            // consecutive chunks.
        }
    }

    out
}
```

**`append_reasoning`**: identical pattern to `append_text` but emits `MessageKind::AgentReasoning` instead of `MessageKind::AgentOutput`. The shared `build_kind_message` helper in `proto_helpers.rs` already supports both. The reasoning message id is stored in `reasoning_message_id` (separate from `text_message_id`).

**`absorb_tool_call_fragment`**: mirrors `OpenAiSseAdapter`'s tool-call accumulator. Each fragment carries `index` (per-call slot), `id` (sometimes), `type` (sometimes), and `function: {name, arguments}` where `arguments` is a string fragment. Accumulate by index; emit `AddMessages{ToolCall}` when the call completes (heuristic: any chunk where the next index increments, OR on `finish_reason: "tool_calls"`, OR on `finish`).

**`finish`**: on `[DONE]` or stream EOF:
- Flush any pending tool-call accumulator entries.
- If `state == Done && captured_finish_reason.is_some()` → emit `CommitTransaction` + `Finished { reason: map_deepseek_finish_reason(...), token_usage }`.
- Otherwise → emit `RollbackTransaction` + `Finished { reason: InternalError(message), token_usage }` where `message` is from `upstream_error` or `"stream ended without finish_reason"`.

**`map_deepseek_finish_reason`**:

```rust
fn map_deepseek_finish_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "stop" => Reason::Done(Done {}),
        "length" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        // "tool_calls" means the model stopped to invoke a tool — surface
        // as Done; the controller continues the turn loop on tool results.
        "tool_calls" => Reason::Done(Done {}),
        // "content_filter" and "insufficient_system_resource" surface as
        // Other. Phase 4 polish can split content_filter into a Refused
        // variant the UI distinguishes.
        _ => Reason::Other(Other {}),
    }
}
```

### Translator design

The translator is **structurally identical to OpenAI's**. The proto-message walk:

- `UserQuery` → `role: "user"` + text content.
- `AgentOutput` → `role: "assistant"` + text content.
- `AgentReasoning` → **DROPPED**. DeepSeek's API rejects `reasoning_content` on inbound messages with HTTP 400. The translator must not emit reasoning back to the server. (Matches every other adapter's translator — none round-trip reasoning.)
- `ToolCall` → `role: "assistant"` + `tool_calls: [{id, type:"function", function: {name, arguments: <stringified JSON>}}]`. Same as OpenAI.
- `ToolCallResult` → `role: "tool"` + content (rendered text) + `tool_call_id`. Same as OpenAI.

System prompt is composed via `prompt::compose_system_prompt` and emitted as a `role: "system"` message at the start of `messages` (same as OpenAI; **NOT** lifted like Anthropic / Gemini do).

Tools envelope: same as OpenAI (`tools: [{type: "function", function: {name, description, parameters}}]`).

Compaction projection, synthetic user-query anchoring, and final user_query append: identical-in-structure to OpenAI's translator. Mirror the existing pattern.

### URL helpers

**Reuse OpenAI's existing helpers.** `cfg.chat_completions_url()` and `cfg.models_list_url()` produce the correct URLs for DeepSeek's `https://api.deepseek.com` base. No new helpers needed.

This is explicit in the file map: `config.rs` is **NOT modified** in Phase 3d.

### Adapter file structure

Mirrors the prior adapter directories:

```
crates/ai/src/local_provider/adapters/deepseek/
├── mod.rs              # DeepSeekAdapter + ProviderAdapter trait impl
├── wire.rs             # Serde types for /chat/completions (extends OpenAI's shape)
├── request.rs          # compose_deepseek_chat_request
├── request_tests.rs    # sibling tests
├── response.rs         # DeepSeekSseDecoder
└── response_tests.rs   # sibling tests
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
        Gemini    => Ok(Box::new(GeminiAdapter)),
        DeepSeek  => Ok(Box::new(DeepSeekAdapter)),       // flipped in Phase 3d
        OpenAiResp => Err(AdapterError::UnsupportedApiType(api_type)),
    }
}
```

After Phase 3d, only `OpenAiResp` remains in the error arm (a Phase 4 polish target).

The file-level doc comment on `adapters/mod.rs` (currently says *"Phase 2 added `OpenAi`; Phase 3a added `Anthropic`; Phase 3b added `Ollama`; Phase 3c added `Gemini`. `DeepSeek` remains a Phase 3d impl; `OpenAiResp` is Phase 4 polish."*) gets updated to: *"Phase 2 added `OpenAi`; Phase 3a added `Anthropic`; Phase 3b added `Ollama`; Phase 3c added `Gemini`; Phase 3d added `DeepSeek`. `OpenAiResp` is Phase 4 polish."*

Same update needed on the `ProviderAdapter` trait-level doc comment (around `mod.rs:114-116`).

### Settings UI

Same situation as Phases 3a / 3b / 3c — the widget already renders every `AgentProviderApiType` variant as a clickable chip via `EnumIter` without per-variant gating. **No UI change needed.** Selecting `DeepSeek` now dispatches correctly instead of erroring with `UnsupportedApiType`.

---

## File map

**Files created:**
- `crates/ai/src/local_provider/adapters/deepseek/mod.rs` — adapter impl.
- `crates/ai/src/local_provider/adapters/deepseek/wire.rs` — serde types.
- `crates/ai/src/local_provider/adapters/deepseek/request.rs` — translator.
- `crates/ai/src/local_provider/adapters/deepseek/request_tests.rs` — sibling tests.
- `crates/ai/src/local_provider/adapters/deepseek/response.rs` — SSE decoder.
- `crates/ai/src/local_provider/adapters/deepseek/response_tests.rs` — sibling tests.

**Files modified:**
- `crates/ai/src/local_provider/adapters/mod.rs` — register `pub mod deepseek;`, re-export `DeepSeekAdapter`, flip `select_adapter(DeepSeek)`, update file-level doc comment + trait-level doc + `select_adapter` rustdoc.
- `crates/ai/src/local_provider/adapters/adapters_tests.rs` — add `select_adapter_returns_deepseek_for_deepseek_api_type`; remove `DeepSeek` from the unimplemented-variants loop (which leaves only `OpenAiResp`).

**Files unchanged (importantly):**
- `crates/ai/src/local_provider/run.rs` — DeepSeek uses SSE; the existing `synthesize_sse_stream` drives it without modification.
- `crates/ai/src/local_provider/config.rs` — URL helpers reused from OpenAI (`chat_completions_url`, `models_list_url`).
- `crates/ai/src/local_provider/adapters/proto_helpers.rs` — DeepSeek reuses `build_kind_message`, `MessageKind::{AgentOutput, AgentReasoning}`, `build_client_action_event`, `client_action_*`, `build_tool_call_event`, `internal_error_reason`.

**Cargo deps:** none added.

---

## Stage A: Wire types + request translator

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git log --oneline -1                     # 08f57d6b docs(specs/multi-local-llm): add plan-phase-3c.md
cargo nextest run -p ai 2>&1 | tail -3   # 567 / 567 passed
```

If anything diverges, STOP and report.

### Task 1: DeepSeek wire types

**File:** Create `crates/ai/src/local_provider/adapters/deepseek/wire.rs`.

DeepSeek's wire types are **mostly clones of OpenAI's** (defined in `crates/ai/src/local_provider/wire.rs`) with one addition: `reasoning_content` on inbound assistant messages and inbound delta chunks. We intentionally define them as a separate type set rather than re-using OpenAI's serde types directly — same precedent as Ollama (which also has OpenAI-shaped wire types but in its own module). This keeps the adapters cleanly partitioned.

**Read `crates/ai/src/local_provider/wire.rs` first** to see the OpenAI request/response types you're cloning. Then read `crates/ai/src/local_provider/adapters/ollama/wire.rs` and `crates/ai/src/local_provider/adapters/gemini/wire.rs` to internalize the sibling-file structure (inline `#[cfg(test)] mod tests { ... }` at the bottom).

- [ ] **Step 1.1: Request types (mirror OpenAI's)**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekChatRequest {
    pub model: String,
    pub stream: bool,
    pub messages: Vec<DeepSeekChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<DeepSeekToolDef>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekChatMessage {
    pub role: DeepSeekRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Required on assistant messages that carry tool_calls; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeepSeekOutboundToolCall>>,
    /// Required on role:"tool" messages; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeepSeekRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekOutboundToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: DeepSeekOutboundToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekOutboundToolCallFunction {
    pub name: String,
    /// Stringified JSON — same as OpenAI's convention. NOT a Value
    /// object. The translator stringifies the typed proto args before
    /// emitting.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekToolDef {
    #[serde(rename = "type")]
    pub kind: &'static str, // "function"
    pub function: DeepSeekToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
```

- [ ] **Step 1.2: Streaming response types (with `reasoning_content`)**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekChatChunk {
    #[serde(default)] pub id: Option<String>,
    #[serde(default)] pub object: Option<String>,
    #[serde(default)] pub created: Option<u64>,
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub choices: Vec<DeepSeekStreamChoice>,
    #[serde(default)] pub usage: Option<DeepSeekUsage>,
    /// Top-level error envelope (rare; some DeepSeek versions emit this
    /// mid-stream).
    #[serde(default)] pub error: Option<DeepSeekErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamChoice {
    #[serde(default)] pub index: Option<u32>,
    #[serde(default)] pub delta: Option<DeepSeekStreamDelta>,
    #[serde(default)] pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamDelta {
    #[serde(default)] pub role: Option<String>,
    #[serde(default)] pub content: Option<String>,
    /// THE Phase-3d-specific field. Streams the reasoning channel
    /// alongside `content`. Present only on `deepseek-reasoner` (other
    /// DeepSeek models always have this as None / absent).
    #[serde(default)] pub reasoning_content: Option<String>,
    #[serde(default)] pub tool_calls: Option<Vec<DeepSeekStreamToolCall>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamToolCall {
    /// The per-call slot index — needed because fragments arrive in
    /// pieces and the decoder needs to know which open call each
    /// fragment belongs to. Required.
    pub index: u32,
    #[serde(default)] pub id: Option<String>,
    #[serde(default, rename = "type")] pub kind: Option<String>,
    #[serde(default)] pub function: Option<DeepSeekStreamToolCallFunction>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekStreamToolCallFunction {
    #[serde(default)] pub name: Option<String>,
    /// Fragment of the stringified-JSON arguments. Accumulate across
    /// chunks until the call completes.
    #[serde(default)] pub arguments: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct DeepSeekUsage {
    #[serde(default)] pub prompt_tokens: u64,
    #[serde(default)] pub completion_tokens: u64,
    #[serde(default)] pub total_tokens: u64,
    /// DeepSeek-specific: tokens served from the API's prompt-cache.
    /// Phase 3d ignores; Phase 4 polish can surface in UI.
    #[serde(default)] pub prompt_cache_hit_tokens: u64,
    #[serde(default)] pub prompt_cache_miss_tokens: u64,
    /// DeepSeek-specific: tokens spent on reasoning vs. final answer.
    /// Phase 3d ignores; folded into `completion_tokens` already.
    #[serde(default)] pub completion_tokens_details: Option<DeepSeekCompletionDetails>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct DeepSeekCompletionDetails {
    #[serde(default)] pub reasoning_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekErrorEnvelope {
    #[serde(default)] pub message: String,
    #[serde(default, rename = "type")] pub kind: String,
    #[serde(default)] pub code: Option<String>,
}
```

- [ ] **Step 1.3: Non-streaming response (summarizer path)**

```rust
/// Non-streaming `/chat/completions` response — used by the summarizer
/// path. Has the same `choices[].message.content` shape OpenAI uses,
/// but `message` may also carry `reasoning_content` (which the
/// summarizer ignores — we only extract `content`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekChatResponse {
    #[serde(default)] pub id: Option<String>,
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub choices: Vec<DeepSeekResponseChoice>,
    #[serde(default)] pub usage: Option<DeepSeekUsage>,
    #[serde(default)] pub error: Option<DeepSeekErrorEnvelope>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekResponseChoice {
    #[serde(default)] pub index: Option<u32>,
    #[serde(default)] pub message: Option<DeepSeekResponseMessage>,
    #[serde(default)] pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeepSeekResponseMessage {
    #[serde(default)] pub role: Option<String>,
    #[serde(default)] pub content: Option<String>,
    /// Present on `deepseek-reasoner` non-streaming responses. The
    /// summarizer ignores this and reads only `content`.
    #[serde(default)] pub reasoning_content: Option<String>,
}
```

- [ ] **Step 1.4: Inline tests + commit**

Tests (~14) cover, following the structure of `gemini/wire.rs` inline tests:

Request serialization:
- `serializes_minimal_text_only_request` — verifies `role: "user"`, `stream: true`, `content` field present.
- `serializes_system_user_assistant_sequence` — confirms role-string lowercase, `messages` array shape.
- `serializes_assistant_with_tool_calls` — `tool_calls[].id`, `type: "function"`, `function.arguments` is a string.
- `serializes_role_tool_with_tool_call_id` — `role: "tool"` + `tool_call_id` field present.
- `omits_tools_when_none`.
- `serializes_tools_array_with_function_wrapper`.
- `tool_call_arguments_serialize_as_string_not_object` — explicitly assert `arguments` JSON value is a String, NOT an Object.

Streaming response deserialization (samples from DeepSeek's docs):
- `deserializes_reasoning_delta_chunk` — `delta.reasoning_content` is `Some(...)`, `content` is `None`.
- `deserializes_content_delta_chunk` — `delta.content` is `Some(...)`, `reasoning_content` is `None`.
- `deserializes_tool_call_fragment_chunk` — `delta.tool_calls[0]` has `index: 0` and partial `function.arguments` fragment.
- `deserializes_final_chunk_with_finish_reason_and_usage` — `finish_reason: "stop"`, usage fields parsed.
- `deserializes_usage_with_reasoning_tokens` — `completion_tokens_details.reasoning_tokens` parsed.
- `deserializes_error_envelope` — `error.message`, `error.kind` parsed.
- `deserializes_chunk_with_role_only_delta` — first chunk where `delta.role: "assistant"` and no other fields; must not error.

Commit:

```
feat(ai/local_provider/adapters/deepseek): add wire types

Phase 3d stage A. Serde types for the OpenAI-compatible /chat/completions
request shape (DeepSeekChatRequest with messages[] / tools[]; assistant
tool_calls with stringified-JSON arguments; role:"tool" with tool_call_id)
plus the streaming response types extended for DeepSeek's
reasoning_content channel (delta.reasoning_content Option<String> on
each chunk). Usage fields include DeepSeek-specific prompt_cache_hit/miss
tokens and completion_tokens_details.reasoning_tokens; Phase 3d
deserializes them but doesn't surface them yet.

The wire types are intentionally a parallel definition (not a reuse of
OpenAI's serde types) so the adapters stay cleanly partitioned — same
precedent as Ollama's wire module.
```

### Task 2: Request translator

**Files:**
- Create `crates/ai/src/local_provider/adapters/deepseek/request.rs`.
- Create `crates/ai/src/local_provider/adapters/deepseek/request_tests.rs`.

**Read these reference files FIRST:**

1. `crates/ai/src/local_provider/adapters/ollama/request.rs` — your closest structural sibling. Ollama's translator emits an OpenAI-shaped body (system as role:"system", role:"tool" messages, tool_calls with stringified arguments). DeepSeek's translator is structurally identical to Ollama's; the ONLY differences are (a) tool_call arguments are stringified-JSON (matching OpenAI's convention, NOT Ollama's object form), and (b) the body uses `DeepSeekChatRequest` / `DeepSeekChatMessage` types instead of Ollama's `OllamaChatRequest` / `OllamaChatMessage`.
2. `crates/ai/src/local_provider/request.rs` — defines `LocalProviderInput`, `enabled_local_tools`, `summarize_tool_call_input`, `summarize_tool_result`. Your translator imports these.

- [ ] **Step 2.1: `compose_deepseek_chat_request`**

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

pub fn compose_deepseek_chat_request(
    input: &LocalProviderInput,
    cfg: &LocalProviderConfig,
) -> DeepSeekChatRequest {
    let local_tools = enabled_local_tools(input.supported_tools.iter().copied(), cfg);
    let tools = if cfg.supports_tools && !local_tools.is_empty() {
        Some(tool_definitions_deepseek(&local_tools))
    } else {
        None
    };

    // System prompt as a role:"system" message (DeepSeek follows OpenAI's
    // convention — system lives in `messages[0]`, not lifted to top-level).
    let descriptions: Vec<&str> = local_tools.iter().map(|t| t.description()).collect();
    let apply_diffs_enabled = local_tools.contains(&LocalTool::ApplyFileDiffs);
    let system_prompt = prompt::compose_system_prompt(
        &descriptions,
        cfg.context_window.filter(|n| *n > 0),
        apply_diffs_enabled,
    );

    let mut messages: Vec<DeepSeekChatMessage> = vec![DeepSeekChatMessage {
        role: DeepSeekRole::System,
        content: Some(system_prompt),
        tool_calls: None,
        tool_call_id: None,
    }];

    // === Compaction projection + history walk + synthetic anchor +
    //     final user_query append: MIRROR the structure of
    //     crates/ai/src/local_provider/adapters/ollama/request.rs::compose_ollama_chat_request
    //     verbatim, substituting DeepSeek types for Ollama types and
    //     replacing per-message body construction with push_proto_message
    //     below. ===

    DeepSeekChatRequest {
        model: cfg.model_id.clone(),
        stream: true,
        messages,
        tools,
    }
}

fn tool_definitions_deepseek(enabled: &[LocalTool]) -> Vec<DeepSeekToolDef> {
    enabled.iter().filter_map(|t| {
        crate::local_provider::tools::schema_for(*t).map(|parameters| DeepSeekToolDef {
            kind: "function",
            function: DeepSeekToolFunction {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters,
            },
        })
    }).collect()
}
```

**History walking** — `push_proto_message`:

```rust
fn push_proto_message(out: &mut Vec<DeepSeekChatMessage>, proto_msg: &api::Message) {
    use api::message::Message as M;
    match proto_msg.message.as_ref() {
        Some(M::UserQuery(q)) => out.push(DeepSeekChatMessage {
            role: DeepSeekRole::User,
            content: Some(q.query.clone()),
            tool_calls: None,
            tool_call_id: None,
        }),
        Some(M::AgentOutput(a)) => out.push(DeepSeekChatMessage {
            role: DeepSeekRole::Assistant,
            content: Some(a.text.clone()),
            tool_calls: None,
            tool_call_id: None,
        }),
        Some(M::ToolCall(call)) => {
            if let Some((name, args)) = summarize_tool_call_input(call) {
                let args_string = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                out.push(DeepSeekChatMessage {
                    role: DeepSeekRole::Assistant,
                    content: None,
                    tool_calls: Some(vec![DeepSeekOutboundToolCall {
                        id: call.tool_call_id.clone(),
                        kind: "function",
                        function: DeepSeekOutboundToolCallFunction {
                            name,
                            arguments: args_string,
                        },
                    }]),
                    tool_call_id: None,
                });
            }
        }
        Some(M::ToolCallResult(result)) => out.push(DeepSeekChatMessage {
            role: DeepSeekRole::Tool,
            content: Some(summarize_tool_result(result)),
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id.clone()),
        }),
        // AgentReasoning is DROPPED from outbound history — DeepSeek's API
        // returns HTTP 400 if reasoning_content appears in inbound
        // messages. The decoder still emits AgentReasoning into the proto
        // stream for the UI; the translator just never sends it back.
        Some(M::AgentReasoning(_)) | Some(_) | None => {}
    }
}
```

**Key shape detail** — confirmed against the DeepSeek docs:
- `tool_calls[].function.arguments` is a **stringified-JSON STRING** (`serde_json::to_string(&args)` produces this), NOT a JSON object. This matches OpenAI's wire convention and the `DeepSeekOutboundToolCallFunction.arguments: String` field type from Task 1.
- `role: "tool"` messages set `tool_call_id` (required); `content` carries the rendered tool result string.

**Compaction projection**: identical pattern to Ollama / OpenAI translators. Open `crates/ai/src/local_provider/adapters/ollama/request.rs::compose_ollama_chat_request` side-by-side and mirror its compaction-projection logic (synthesizes a `user "Continue from prior summary."` + `assistant "<summary>"` pair, then resumes walking from `tail_start_id`).

**Synthetic user-query anchoring**: mirror Ollama / OpenAI.

**Final user_query append**: mirror Ollama / OpenAI.

**Tool-call ordering / orphan handling**: DeepSeek is strict about tool-call/tool-result pairing — every assistant `tool_calls` entry MUST be followed by a `role: "tool"` message with the matching `tool_call_id`. Mirror Ollama's orphan-backfill logic if it has one (read the file); otherwise document that the controller handles orphan cleanup upstream.

- [ ] **Step 2.2: Sibling tests (~16) — `request_tests.rs`**

Test list:

1. `system_prompt_becomes_first_message_with_role_system` — confirm `messages[0].role == DeepSeekRole::System`.
2. `user_query_becomes_role_user_message`.
3. `agent_output_becomes_role_assistant_message`.
4. `agent_reasoning_is_dropped_from_outbound_history` — critical: a `LocalProviderInput` with proto `AgentReasoning` messages must NOT produce any wire message containing `reasoning_content`. Assert by serializing the body to JSON and grep-asserting no `reasoning_content` key.
5. `tool_call_proto_becomes_assistant_with_stringified_arguments` — verify `tool_calls[0].function.arguments` is a JSON STRING (not an object).
6. `tool_call_carries_tool_call_id_from_proto` — assert round-trip of `tool_call_id`.
7. `tool_result_becomes_role_tool_with_tool_call_id`.
8. `tools_envelope_uses_function_type_wrapper`.
9. `tools_omitted_when_supports_tools_false`.
10. `tools_omitted_when_enabled_tools_empty`.
11. `stream_is_always_true`.
12. `compaction_projection_synthesizes_user_assistant_summary_pair` — mirrors Ollama's equivalent test.
13. `synthetic_user_query_anchoring_works`.
14. `multi_turn_round_trip_with_text_and_tool_call` — multi-message history serializes cleanly.
15. `model_id_threads_from_cfg` — verify `request.model == cfg.model_id`.
16. `reasoning_content_never_appears_in_serialized_body` — paranoia test: build an input with both `AgentReasoning` and `AgentOutput` messages, serialize body, grep the JSON for `reasoning_content` — must NOT be present.

- [ ] **Step 2.3: Wire the module**

Edit `crates/ai/src/local_provider/adapters/deepseek/mod.rs` to add:

```rust
pub mod request;
pub mod wire;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
```

Don't add `pub mod response;` yet — that's Task 3. Don't add a `DeepSeekAdapter` struct — that's Task 4.

- [ ] **Step 2.4: Build + tests + commit**

Baseline before this task: 567 tests pass plus ~14 from Task 1 = ~581. After Task 2: 581 + 16 = ~597.

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/deepseek): request translator

Phase 3d stage A. compose_deepseek_chat_request walks LocalProviderInput
and emits OpenAI-compatible /chat/completions body shape: system as
role:"system" message at messages[0], user/assistant text messages,
assistant tool_calls with stringified-JSON arguments, role:"tool"
messages with tool_call_id. Compaction projection + synthetic anchor
+ final user_query mirror the Ollama / OpenAI translators.

CRITICAL: AgentReasoning proto messages are DROPPED from outbound
history. DeepSeek's API returns HTTP 400 when reasoning_content appears
on inbound messages — the reasoning channel is response-only. The
decoder (Task 3) still emits AgentReasoning for the UI; the translator
just doesn't echo it back.
```

---

## Stage B: SSE decoder

### Task 3: `DeepSeekSseDecoder`

**Files:**
- Create `crates/ai/src/local_provider/adapters/deepseek/response.rs`.
- Create `crates/ai/src/local_provider/adapters/deepseek/response_tests.rs`.

**Read these reference files FIRST:**

1. `crates/ai/src/local_provider/adapters/openai.rs` — DeepSeek's wire shape is closer to OpenAI's than any other adapter. The OpenAI decoder is **inline** in this file (rather than split into request/response submodules — Phase 2 predates the submodule pattern). Read the `OpenAiSseAdapter` struct, its `StreamDecoder` impl, and especially its tool-call-fragment accumulator. Your decoder's tool-call code is a near-clone.
2. `crates/ai/src/local_provider/adapters/gemini/response.rs` — your structural sibling for the SSE state machine (Init/Begin/CreateTask prelude, shared text-message id, finish path). Gemini doesn't have reasoning, so your decoder ADDS the reasoning-channel path.
3. `crates/ai/src/local_provider/adapters/anthropic/response.rs` — read briefly for how `AgentReasoning` is emitted (Anthropic's `thinking_delta` path). The `MessageKind::AgentReasoning` proto event shape is established there.
4. `crates/ai/src/local_provider/adapters/proto_helpers.rs` — your decoder uses `build_kind_message(message_id, MessageKind::{AgentOutput, AgentReasoning}, text)`, `build_client_action_event`, `client_action_begin/commit/rollback/create_task`, `build_tool_call_event`, `internal_error_reason`.

- [ ] **Step 3.1: Decoder impl**

Public surface mirrors `GeminiSseDecoder` / `OllamaDecoder`:

```rust
pub struct DeepSeekSseDecoder { ... }

impl DeepSeekSseDecoder {
    pub fn new() -> Self { ... }
    pub fn with_ids(conversation_id: String, request_id: String, run_id: String, task_id: String) -> Self { ... }
    pub fn skip_create_task(&mut self) { ... }
}

impl crate::local_provider::adapters::StreamDecoder for DeepSeekSseDecoder {
    fn feed_event(&mut self, _event_name: Option<&str>, data: &str) -> Vec<api::ResponseEvent> { ... }
    fn finish(&mut self) -> Vec<api::ResponseEvent> { ... }
    fn is_terminal(&self) -> bool { matches!(self.state, State::Done | State::Errored) }
    fn record_upstream_error(&mut self, msg: String) {
        self.upstream_error.get_or_insert(msg);
    }
}
```

Internal state per the design refinement above. Private helpers:

- `ensure_prelude(&mut self) -> Vec<ResponseEvent>` — lazily emits Init + BeginTransaction + CreateTask on first non-empty feed. Identical to `GeminiSseDecoder::ensure_prelude`.
- `append_text(&mut self, text: &str, out: &mut Vec<ResponseEvent>)` — opens or appends the shared `AgentOutput`. Identical to `GeminiSseDecoder::append_text`.
- `append_reasoning(&mut self, text: &str, out: &mut Vec<ResponseEvent>)` — opens or appends the shared `AgentReasoning`. **New** for Phase 3d. Uses `MessageKind::AgentReasoning` from `proto_helpers`.
- `absorb_tool_call_fragment(&mut self, tc: DeepSeekStreamToolCall, out: &mut Vec<ResponseEvent>)` — accumulates tool-call fragments by `index`. When the call is complete (heuristic: subsequent chunk arrives with a different index, OR `finish_reason: "tool_calls"` appears, OR stream ends), emit one `AddMessages{ToolCall}` via `proto_helpers::build_tool_call_event`. **Read OpenAi's existing fragment accumulator** at `crates/ai/src/local_provider/adapters/openai.rs` for the canonical implementation — clone its structure.

**Per-chunk dispatch:**

```rust
fn feed_event(&mut self, _event_name: Option<&str>, data: &str)
    -> Vec<api::ResponseEvent>
{
    if matches!(self.state, State::Done | State::Errored) { return vec![]; }
    let trimmed = data.trim();
    if trimmed.is_empty() { return vec![]; }

    // [DONE] terminator (same as OpenAI).
    if trimmed == "[DONE]" {
        self.state = State::Done;
        return vec![];
    }

    let mut out = self.ensure_prelude();
    let chunk: DeepSeekChatChunk = match serde_json::from_str(trimmed) {
        Ok(c) => c,
        Err(e) => {
            self.state = State::Errored;
            self.upstream_error.get_or_insert_with(|| format!("malformed DeepSeek chunk: {e}"));
            return out;
        }
    };

    if let Some(err) = chunk.error {
        let kind = if err.kind.is_empty() { "error".to_string() } else { err.kind };
        self.upstream_error = Some(format!("{}: {}", kind, err.message));
        self.state = State::Errored;
        return out;
    }

    if self.captured_model.is_none() {
        if let Some(m) = chunk.model.filter(|s| !s.is_empty()) {
            self.captured_model = Some(m);
        }
    }

    if let Some(usage) = chunk.usage {
        self.captured_input_tokens = usage.prompt_tokens.max(self.captured_input_tokens);
        self.captured_output_tokens = usage.completion_tokens.max(self.captured_output_tokens);
    }

    if let Some(choice) = chunk.choices.into_iter().next() {
        if let Some(delta) = choice.delta {
            if let Some(reasoning) = delta.reasoning_content {
                if !reasoning.is_empty() {
                    self.append_reasoning(&reasoning, &mut out);
                }
            }
            if let Some(content) = delta.content {
                if !content.is_empty() {
                    self.append_text(&content, &mut out);
                }
            }
            if let Some(tool_calls) = delta.tool_calls {
                for tc in tool_calls {
                    self.absorb_tool_call_fragment(tc, &mut out);
                }
            }
        }
        if let Some(reason) = choice.finish_reason {
            self.captured_finish_reason = Some(reason);
            // Don't transition to Done here — [DONE] comes in a separate
            // SSE event afterward (same as OpenAI's two-step pattern).
        }
    }

    out
}
```

**`finish`** — when called by the runner (after `[DONE]` or stream EOF):

- Flush any pending tool-call accumulator entries by emitting their `AddMessages{ToolCall}` events (OpenAI's decoder does this; mirror it).
- Healthy path (`state == Done && captured_finish_reason.is_some()`) → emit `CommitTransaction` + `Finished { reason: map_deepseek_finish_reason(...), token_usage }`.
- Sad paths (premature EOF / Errored) → emit `RollbackTransaction` + `Finished { reason: InternalError(...), token_usage }`.
- `TokenUsage` carries `captured_input_tokens` + `captured_output_tokens`. Model id falls back to `captured_model` then `"deepseek"`.

**`map_deepseek_finish_reason`:**

```rust
fn map_deepseek_finish_reason(reason: &str) -> api::response_event::stream_finished::Reason {
    use api::response_event::stream_finished::*;
    match reason {
        "stop" | "tool_calls" => Reason::Done(Done {}),
        "length" => Reason::MaxTokenLimit(ReachedMaxTokenLimit {}),
        _ => Reason::Other(Other {}),
    }
}
```

(String match — same precedent as `map_gemini_finish_reason` and `map_ollama_done_reason`. The `_ =>` wildcard is acceptable here per the established string-discriminator pattern.)

- [ ] **Step 3.2: Sibling tests (~22) — `response_tests.rs`**

Use the test-helper patterns from `gemini/response_tests.rs` (helpers like `decoder()`, `feed()`, `extract_init/action/finished`, `drive_to_done`, `make_text_chunk(text)`, etc.).

Test list:

1. `prelude_emitted_on_first_non_empty_feed`.
2. `with_ids_round_trips_into_init`.
3. `skip_create_task_suppresses_create_task`.
4. `simple_text_streaming_builds_canonical_event_sequence` — feed text chunks "Hello", " world"; then `finish_reason: "stop"` + `[DONE]`; assert Init + Begin + Create + AddMessages(text "Hello") + Append(" world") + Commit + Finished{Done}.
5. `reasoning_streaming_emits_agent_reasoning_message` — feed two `delta.reasoning_content` chunks ("Let me ", "think..."); assert the events include an `AddMessages` containing a `Message::AgentReasoning("Let me ")` + `AppendToMessage` with "think...".
6. `reasoning_then_content_emits_distinct_messages` — feed reasoning chunks first, then content chunks; assert two distinct shared messages were opened (one AgentReasoning, one AgentOutput) — NOT a single message with mixed content.
7. `interleaved_reasoning_and_content_still_dispatches_correctly` — chunk with BOTH `reasoning_content` and `content` non-empty; assert both messages get appropriate appends.
8. `empty_reasoning_content_silently_skipped` — `delta.reasoning_content: Some("")` does NOT emit an event.
9. `empty_content_silently_skipped` — `delta.content: Some("")` does NOT emit an event.
10. `tool_call_fragments_accumulate_and_emit_on_completion` — feed two tool_call fragments for index 0 (name + arg fragment 1, arg fragment 2); then a `finish_reason: "tool_calls"` chunk; assert one `AddMessages{ToolCall}` event with the concatenated arguments parsed correctly.
11. `multiple_tool_calls_emit_separately` — feed tool_call fragments for index 0 and index 1; assert two distinct `AddMessages{ToolCall}` events.
12. `finish_reason_stop_maps_to_done`.
13. `finish_reason_tool_calls_maps_to_done` — DeepSeek emits `"tool_calls"` when stopping mid-stream to invoke a tool; map to `Done` so the controller continues the turn loop.
14. `finish_reason_length_maps_to_max_token_limit`.
15. `finish_reason_content_filter_maps_to_other`.
16. `finish_reason_unknown_maps_to_other`.
17. `top_level_error_field_surfaces_as_internal_error`.
18. `malformed_json_chunk_transitions_to_errored`.
19. `premature_eof_without_done_emits_rollback`.
20. `record_upstream_error_surfaces_in_finish`.
21. `usage_with_reasoning_tokens_still_folds_into_completion_tokens` — assert `usage.completion_tokens` is what shows up in `TokenUsage.output_tokens`, ignoring the `completion_tokens_details.reasoning_tokens` split.
22. `terminal_state_safety_post_done_feeds_are_no_ops`.

- [ ] **Step 3.3: Wire the module**

Extend `crates/ai/src/local_provider/adapters/deepseek/mod.rs`:

```rust
pub mod request;
pub mod response;
pub mod wire;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
#[cfg(test)]
#[path = "response_tests.rs"]
mod response_tests;
```

Still no `DeepSeekAdapter` struct — that's Task 4.

- [ ] **Step 3.4: Build + tests + clippy + commit**

Baseline: ~597 tests. After: 597 + 22 = ~619.

```bash
cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/deepseek): SSE decoder

Phase 3d stage B. DeepSeekSseDecoder consumes the /chat/completions
SSE stream (anonymous data: chunks, same wire framing as OpenAI;
[DONE] is the terminator). Adds a reasoning-channel path:
delta.reasoning_content opens / appends a shared AgentReasoning
message that streams alongside the AgentOutput text channel. Tool-call
fragment accumulator mirrors OpenAiSseAdapter's by-index pattern.
finish_reason mapping covers stop/tool_calls/length/content_filter/
insufficient_system_resource via the same Reason variants as the
other adapters.

usage.completion_tokens_details.reasoning_tokens is deserialized but
ignored in Phase 3d — reasoning + answer tokens fold into a single
output_tokens counter (Phase 4 polish can split them in the UI).
```

---

## Stage C: Adapter impl + dispatch flip

### Task 4: `DeepSeekAdapter` impl

**Files:**
- Extend `crates/ai/src/local_provider/adapters/deepseek/mod.rs`.
- Adapter-glue tests appended to `crates/ai/src/local_provider/adapters/deepseek/request_tests.rs` (same pattern as Gemini Phase 3c).

**Read these reference files FIRST:**

1. `crates/ai/src/local_provider/adapters/openai.rs` — the canonical reference. `OpenAiAdapter`'s `build_chat_request` + `build_summarizer_request` + `parse_summarizer_response` + `build_probe_request` is what you're cloning, just substituting DeepSeek types.
2. `crates/ai/src/local_provider/adapters/gemini/mod.rs` — your sibling-pattern reference (recently shipped). Compare the `apply_*_headers` helper structure.

- [ ] **Step 4.1: `mod.rs` — adapter glue**

```rust
//! DeepSeek native protocol adapter. Phase 3d.
//!
//! Submodule layout mirrors Phase 3a/3b/3c:
//! - `wire`: serde types for /chat/completions (OpenAI-shaped with
//!   reasoning_content extensions on inbound types).
//! - `request`: translator from `LocalProviderInput` to a
//!   `DeepSeekChatRequest`.
//! - `response`: SSE stream decoder (`DeepSeekSseDecoder`).
//!
//! DeepSeek's wire format is intentionally OpenAI-compatible — the only
//! semantic divergence is the `reasoning_content` channel on assistant
//! messages (deepseek-reasoner model only). Phase 3d handles it on the
//! response side (decoder emits AgentReasoning proto messages) but NOT
//! on the request side: the API returns HTTP 400 if reasoning_content
//! appears on inbound messages, so the translator drops AgentReasoning
//! from outbound history.

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

use request::compose_deepseek_chat_request;
use response::DeepSeekSseDecoder;
use wire::{DeepSeekChatRequest, DeepSeekChatResponse};

pub struct DeepSeekAdapter;

impl ProviderAdapter for DeepSeekAdapter {
    fn api_type(&self) -> AgentProviderApiType {
        AgentProviderApiType::DeepSeek
    }

    // streaming_format() inherits the SSE default — no override needed.

    fn build_chat_request(
        &self,
        input: &LocalProviderInput,
        cfg: &LocalProviderConfig,
        http: &reqwest::Client,
    ) -> Result<reqwest::RequestBuilder, AdapterError> {
        cfg.validate()?;
        let url = cfg.chat_completions_url()?;
        let body = compose_deepseek_chat_request(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_deepseek_headers(
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
            Some(ids) => DeepSeekSseDecoder::with_ids(
                ids.conversation_id,
                ids.request_id,
                ids.run_id,
                ids.task_id,
            ),
            None => DeepSeekSseDecoder::new(),
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
        let url = cfg.chat_completions_url()?;
        let body = build_deepseek_summarizer_body(input, cfg);
        let body_json = serde_json::to_string(&body)?;
        Ok(apply_deepseek_headers(
            http.post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body_json),
            cfg.api_key.as_deref(),
        ))
    }

    fn parse_summarizer_response(&self, body: &str) -> Result<String, SummarizerError> {
        let parsed: DeepSeekChatResponse = serde_json::from_str(body).map_err(|e| {
            SummarizerError::DecodeResponse(format!(
                "{e}: {}",
                crate::local_provider::run::first_chars(body, 200)
            ))
        })?;
        if let Some(err) = parsed.error {
            let kind = if err.kind.is_empty() { "error".to_string() } else { err.kind };
            return Err(SummarizerError::UpstreamErrorEnvelope(format!(
                "{}: {}",
                kind, err.message
            )));
        }
        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .and_then(|m| m.content)
            .unwrap_or_default();
        let trimmed = text.trim();
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
        let url = cfg.models_list_url()?;
        Ok(apply_deepseek_headers(http.get(url), cfg.api_key.as_deref()))
    }
}

fn apply_deepseek_headers(
    rb: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    match api_key.filter(|k| !k.is_empty()) {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    }
}

/// Translate the OpenAI-shaped `SummarizerInput.messages` list into a
/// non-streaming DeepSeek /chat/completions body. Same shape as OpenAI's
/// summarizer body — system / user / assistant messages, no tools,
/// stream: false.
fn build_deepseek_summarizer_body(
    input: &SummarizerInput,
    cfg: &LocalProviderConfig,
) -> DeepSeekChatRequest {
    use crate::local_provider::wire::Role;
    use wire::{DeepSeekChatMessage, DeepSeekRole};
    let messages: Vec<DeepSeekChatMessage> = input
        .messages
        .iter()
        .filter_map(|msg| {
            let role = match msg.role {
                Role::System => DeepSeekRole::System,
                Role::User => DeepSeekRole::User,
                Role::Assistant => DeepSeekRole::Assistant,
                Role::Tool => return None, // compaction never emits Tool
            };
            Some(DeepSeekChatMessage {
                role,
                content: msg.content.clone(),
                tool_calls: None,
                tool_call_id: None,
            })
        })
        .collect();
    DeepSeekChatRequest {
        model: cfg.model_id.clone(),
        stream: false,
        messages,
        tools: None,
    }
}
```

- [ ] **Step 4.2: Adapter tests (~9) — append to `request_tests.rs`**

Test list (mirrors Gemini's Phase 3c adapter tests):

1. `deepseek_adapter_chat_request_url_and_bearer_header` — `cfg.base_url = "https://api.deepseek.com"`, `cfg.model_id = "deepseek-reasoner"`, `cfg.api_key = Some("sk-TEST")`. Call `DeepSeekAdapter.build_chat_request(...).build()`. Assert: method POST, URL is `https://api.deepseek.com/chat/completions`, header `authorization: Bearer sk-TEST`, header `accept: text/event-stream`.
2. `deepseek_adapter_chat_request_omits_bearer_when_key_absent`.
3. `deepseek_adapter_chat_request_omits_bearer_when_key_empty_string`.
4. `deepseek_adapter_summarizer_request_uses_chat_completions_url_with_application_json_accept`.
5. `deepseek_adapter_summarizer_body_emits_role_system_message`.
6. `deepseek_adapter_probe_request_targets_models_endpoint_with_bearer`.
7. `deepseek_adapter_parse_summarizer_response_extracts_content_from_first_choice`.
8. `deepseek_adapter_parse_summarizer_response_empty_yields_no_content_error`.
9. `deepseek_adapter_parse_summarizer_response_top_level_error_yields_upstream_envelope`.

Test helper pattern: define `fn deepseek_cfg()` and `fn http_client()` at the start of the adapter-tests section, mirroring `gemini_cfg()` in Gemini's tests:

```rust
fn deepseek_cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "DeepSeek".into(),
        base_url: "https://api.deepseek.com".into(),
        model_id: "deepseek-reasoner".into(),
        api_key: Some("sk-TEST".into()),
        supports_tools: false,    // deepseek-reasoner has no tool support
        context_window: None,
        api_type: AgentProviderApiType::DeepSeek,
    }
}

fn http_client() -> reqwest::Client {
    crate::local_provider::adapters::ensure_rustls_provider();
    reqwest::Client::new()
}
```

- [ ] **Step 4.3: Build + tests + clippy + commit**

Baseline: ~619. After: 619 + 9 = ~628.

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
cargo clippy -p ai --all-targets -- -D warnings
```

Commit:

```
feat(ai/local_provider/adapters/deepseek): adapter glue

Phase 3d stage C. DeepSeekAdapter implements ProviderAdapter: chat
targets {base_url}/chat/completions (reuses OpenAI's
chat_completions_url helper) with Bearer auth and text/event-stream
Accept; summarizer hits the same URL with stream:false body and
application/json Accept; probe targets {base_url}/models. Reuses
OpenAI's URL helpers — no new config.rs functions needed.
streaming_format() inherits the SSE default.

Summarizer body builder mirrors OpenAI's: system / user / assistant
messages, no tools. role:Tool messages from compaction (never emitted
in practice) are silently dropped.
```

### Task 5: Flip `select_adapter`

**Files:**
- Modify `crates/ai/src/local_provider/adapters/mod.rs`.
- Modify `crates/ai/src/local_provider/adapters/adapters_tests.rs`.

- [ ] **Step 5.1: `adapters/mod.rs` edits**

**Edit 1 — file-level doc comment (lines 1-4):**

Current text (verify by reading the file):

```rust
//! Provider adapter trait — abstracts request composition and stream decoding
//! over wire-protocol variants. Phase 2 added `OpenAi`; Phase 3a added
//! `Anthropic`; Phase 3b added `Ollama`; Phase 3c added `Gemini`. `DeepSeek`
//! remains a Phase 3d impl; `OpenAiResp` is Phase 4 polish.
```

Replace with:

```rust
//! Provider adapter trait — abstracts request composition and stream decoding
//! over wire-protocol variants. Phase 2 added `OpenAi`; Phase 3a added
//! `Anthropic`; Phase 3b added `Ollama`; Phase 3c added `Gemini`; Phase 3d
//! added `DeepSeek`. `OpenAiResp` remains Phase 4 polish.
```

**Edit 2 — `pub mod deepseek;` + re-export:**

After the existing `pub mod gemini;` line, add `pub mod deepseek;` (alphabetical placement actually means: anthropic, deepseek, gemini, ollama, openai). Verify alphabetical ordering and place accordingly.

After the existing `pub use gemini::GeminiAdapter;`, add `pub use deepseek::DeepSeekAdapter;` (again alphabetical).

**Edit 3 — `ProviderAdapter` trait-level doc comment:**

Find the trait-doc paragraph (around `mod.rs:114-116`). Current:

```rust
/// Wire-protocol adapter. Stateless; one instance per `AgentProviderApiType`.
/// Phase 2 shipped `OpenAiAdapter`; Phase 3a added Anthropic; Phase 3b added
/// Ollama-native; Phase 3c added Gemini. DeepSeek remains Phase 3d work.
```

Replace with:

```rust
/// Wire-protocol adapter. Stateless; one instance per `AgentProviderApiType`.
/// Phase 2 shipped `OpenAiAdapter`; Phase 3a added Anthropic; Phase 3b added
/// Ollama-native; Phase 3c added Gemini; Phase 3d added DeepSeek.
```

**Edit 4 — `streaming_format` rustdoc:**

If the current rustdoc still says "Future SSE-based adapters (DeepSeek) inherit the default", update it. After Phase 3d, no future adapters remain — change to:

```rust
/// What wire framing does this adapter's chat stream use? Defaults to
/// SSE — `OllamaAdapter` overrides to `NewlineDelimitedJson`. All other
/// adapters (OpenAi, Anthropic, Gemini, DeepSeek) inherit the default.
fn streaming_format(&self) -> StreamingFormat {
    StreamingFormat::ServerSentEvents
}
```

**Edit 5 — `select_adapter` rustdoc:**

Current:

```rust
/// Pick an adapter for the given wire-protocol variant. Phase 2 added
/// `OpenAiAdapter`; Phase 3a/3b/3c flipped `Anthropic`, `Ollama`, and
/// `Gemini` to real impls. The two remaining variants (`OpenAiResp`,
/// `DeepSeek`) surface a structured `UnsupportedApiType` error until
/// their respective Phase 3d/4 sub-phases land. The match is intentionally
/// exhaustive (no `_ =>` arm) so adding/removing a variant triggers a
/// compile error at this dispatch site per repo convention.
```

Replace with:

```rust
/// Pick an adapter for the given wire-protocol variant. Phase 2 added
/// `OpenAiAdapter`; Phase 3a/3b/3c/3d flipped `Anthropic`, `Ollama`,
/// `Gemini`, and `DeepSeek` to real impls. The one remaining variant
/// (`OpenAiResp`) surfaces a structured `UnsupportedApiType` error
/// pending Phase 4 polish. The match is intentionally exhaustive (no
/// `_ =>` arm) so adding/removing a variant triggers a compile error at
/// this dispatch site per repo convention.
```

**Edit 6 — `select_adapter` body:**

Current:

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

Replace with:

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
        DeepSeek => Ok(Box::new(DeepSeekAdapter)),
        OpenAiResp => Err(AdapterError::UnsupportedApiType(api_type)),
    }
}
```

- [ ] **Step 5.2: `adapters_tests.rs` edits**

After the existing `select_adapter_returns_gemini_for_gemini_api_type` test, add:

```rust
#[test]
fn select_adapter_returns_deepseek_for_deepseek_api_type() {
    let a = select_adapter(AgentProviderApiType::DeepSeek).expect("ok");
    assert_eq!(a.api_type(), AgentProviderApiType::DeepSeek);
}
```

Then find the `select_adapter_errors_for_each_unimplemented_variant` test. After Phase 3c, its iteration array was `[OpenAiResp, DeepSeek]`. Remove `DeepSeek`. The result should be `[OpenAiResp]`. Since the loop now has only one variant, you may also consider replacing the loop with a single-call test for cleanliness — BUT, to preserve the "this catches additions of new unimplemented variants" intent, keep the loop form. Final result:

```rust
#[test]
fn select_adapter_errors_for_each_unimplemented_variant() {
    for ty in [AgentProviderApiType::OpenAiResp] {
        match select_adapter(ty) {
            Ok(_) => panic!("expected UnsupportedApiType for {ty:?}"),
            Err(AdapterError::UnsupportedApiType(got)) => assert_eq!(got, ty),
            Err(other) => panic!("wrong variant for {ty:?}: {other:?}"),
        }
    }
}
```

- [ ] **Step 5.3: Build + tests + clippy + commit**

Baseline: ~628. After: 628 + 1 = ~629.

```bash
cargo build -p ai && cargo nextest run -p ai 2>&1 | tail -3
cargo build -p warp
cargo clippy -p ai --all-targets -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

The `warp` lib build must still pass — `DeepSeekAdapter` is now reachable through `select_adapter`.

Commit:

```
feat(ai/local_provider/adapters): flip select_adapter for DeepSeek

Phase 3d stage C. DeepSeek api_type now dispatches to DeepSeekAdapter
instead of returning UnsupportedApiType. Updates the adapters/mod.rs
file-level doc, ProviderAdapter trait-level doc, streaming_format
rustdoc, and select_adapter rustdoc to reflect that all five native
adapters (OpenAi, Anthropic, Ollama, Gemini, DeepSeek) ship; only
OpenAiResp remains in the unimplemented arm.

adapters_tests gains a green-path test for DeepSeek; the unimplemented-
variant loop now iterates only OpenAiResp.
```

---

## Stage D: Manual smoke + docs

### Task 6: Live test + spec docs

- [ ] **Step 6.1: Live test against `api.deepseek.com`**

Documented checklist — requires a DeepSeek API key from <https://platform.deepseek.com/api_keys>:

1. Create or reuse an API key.
2. In the app, Settings → AI → Custom AI Providers, add provider:
   - Name: `DeepSeek (reasoner)`
   - API type: `DeepSeek`
   - Base URL: `https://api.deepseek.com`
   - API key: paste the `sk-…` value.
   - Model id: `deepseek-reasoner`
   - Tool call: **disabled** (`deepseek-reasoner` has no tool-use channel).
3. Click **Test connection** — expect green check (200 from `/models`).
4. Open a new conversation, pick `DeepSeek (reasoner) / deepseek-reasoner`.
5. Send: `What is 17 * 23? Show your reasoning.` — expect:
   - Streamed `AgentReasoning` text showing the model's chain-of-thought.
   - Then streamed `AgentOutput` text with the final answer.
   - A `Finished{Done}` event closing the turn.
6. Send a multi-turn follow-up that re-references the prior result; confirm the model continues correctly **without** echoing the prior turn's reasoning back (the translator dropped it on the way out).
7. **Tool-use test** — add a second provider entry with model `deepseek-chat` and tool_call **enabled**. Send: `Read the top 5 lines of Cargo.toml and tell me what version the workspace is on.` — expect streamed assistant text, a `read_files` tool call, the tool result rendered back, and a final summary. NO `AgentReasoning` content (deepseek-chat doesn't reason).
8. Set the API key to an invalid value, click Test connection — expect red failure with a 401 message.

- [ ] **Step 6.2: Update spec docs**

- `specs/multi-local-llm/README.md`:
  - Phase 3d row in the table: 🧪 code complete (then ✅ shipped once live smoke passes) with date.
  - Status preamble: add a Phase 3d paragraph mirroring the Phase 3a/3b/3c pattern (commit ref, key facts, test count, verification gate).
  - "What landed" → User-visible: add a Phase 3d bullet mentioning DeepSeek as a real api_type, with `reasoning_content` rendered as distinct `AgentReasoning` blocks in the UI.
  - "What landed" → Architecture: add a Phase 3d bullet mentioning `DeepSeekAdapter` + `DeepSeekSseDecoder`, the `reasoning_content` decoder path, and the **dropped-from-history** behavior.
  - `AgentProviderApiType` enum-active list: gains DeepSeek; reserved list becomes empty (just `OpenAiResp` for future Phase 4 polish).
  - "Future phases" section: drop the Phase 3d entry; remaining future is 4a–d.
  - "Reading order → Source" list: add `crates/ai/src/local_provider/adapters/deepseek/{mod,request,response,wire}.rs`.
- `specs/multi-local-llm/design.md` §9 phase table: mark Phase 3d row 🧪 code complete / ✅ shipped with a fuller description matching the 3a/3b/3c rows.

- [ ] **Step 6.3: Commit**

```
docs(specs/multi-local-llm): record Phase 3d code-complete status
```

Or, after live smoke passes:

```
docs(specs/multi-local-llm): mark Phase 3d (DeepSeek adapter) shipped
```

---

## Final verification

- [ ] **Verification 1: Sweeps**

```bash
echo "=== DeepSeek submodule wired ==="
grep -n "pub mod deepseek" crates/ai/src/local_provider/adapters/mod.rs

echo "=== select_adapter flipped for DeepSeek ==="
grep -nA 1 "DeepSeek =>" crates/ai/src/local_provider/adapters/mod.rs

echo "=== DeepSeekAdapter inherits SSE default ==="
grep -n "streaming_format" crates/ai/src/local_provider/adapters/deepseek/mod.rs || echo "(none — inherits default — expected)"

echo "=== No new URL helpers ==="
git diff 08f57d6b -- crates/ai/src/local_provider/config.rs   # should be empty

echo "=== run.rs unchanged ==="
git diff 08f57d6b -- crates/ai/src/local_provider/run.rs   # should be empty

echo "=== AgentReasoning never serialized in outbound body ==="
# Sanity: search for any place the translator might emit reasoning_content
grep -nE "reasoning_content" crates/ai/src/local_provider/adapters/deepseek/request.rs && echo "FOUND — REVIEW" || echo "(none — outbound translator never emits reasoning_content — expected)"
```

- [ ] **Verification 2: Build + tests + clippy**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -5     # ~629 tests (567 + 14 wire + 16 translator + 22 decoder + 9 adapter + 1 dispatch flip)
cargo nextest run -p warp --lib 2>&1 | tail -5
cargo clippy -p ai --all-targets --all-features -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

- [ ] **Verification 3: Manual smoke**

Per Task 6.1 — real DeepSeek API key.

- [ ] **Verification 4: Final reviewer + push**

Dispatch `oh-my-claudecode:code-reviewer` for the full Phase 3d diff (`08f57d6b..HEAD`). Stop before push; user reviews, then pushes manually.

---

## Risks & open questions

1. **`reasoning_content` round-trip rule** — DeepSeek's API EXPLICITLY rejects `reasoning_content` on inbound `messages` (HTTP 400). The translator's correctness depends on this rule being enforced. **Mitigation:** Task 2 includes a paranoia test (`reasoning_content_never_appears_in_serialized_body`) that serializes a body built from an input with `AgentReasoning` proto messages and grep-asserts the JSON body does NOT contain `reasoning_content`. If this test ever passes when reasoning_content is present, the translator has regressed and the live API will reject the request.

2. **Tool-call fragment accumulator correctness** — DeepSeek's tool-call streaming follows OpenAI's by-index pattern. The decoder's accumulator must (a) recognize index changes as call boundaries, (b) handle `id` and `type` arriving only on the first fragment of a call (not all fragments), (c) emit accumulated calls on `finish_reason: "tool_calls"` even when the next chunk's index hasn't incremented. **Mitigation:** Reuse `OpenAiSseAdapter`'s accumulator logic verbatim — it's been live for many turns and the test coverage there is already robust.

3. **`deepseek-reasoner` has no tool support** — selecting `deepseek-reasoner` with `supports_tools = true` makes the adapter emit a `tools` array, which DeepSeek silently ignores (no error). User experience: tool-using prompts won't actually invoke tools, but the conversation will proceed. Phase 4 polish can add per-model capability gating in the settings UI to warn the user. Phase 3d documents this in the live-smoke checklist (Step 2 sets tool_call disabled).

4. **`completion_tokens_details.reasoning_tokens` is hidden from the UI** — Phase 3d folds reasoning + final-answer tokens into a single `output_tokens` counter. Users can't see how much of their token budget went to reasoning vs. the final answer. **Risk:** Low — the count is still ACCURATE (just unsegmented). Phase 4 polish can split them.

5. **`prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` ignored** — DeepSeek's API charges differently for cache-hit vs. cache-miss prompt tokens. Phase 3d's `TokenUsage` carries `input_tokens` and `output_tokens` aggregated; the cache split is dropped. **Risk:** Low — DeepSeek pricing is still calculable from total prompt tokens. Phase 4 polish can surface the split in the UI cost-attribution.

6. **No live test in CI** — Same gate as Phases 3a/3b/3c — manual smoke against the real API. A future integration test using a mock SSE server (similar to `local_provider_integration.rs`) would unblock CI coverage; deferred to Phase 4.

7. **`finish_reason: "tool_calls"` semantics** — DeepSeek emits this when the model stops mid-stream to invoke a tool. The decoder maps it to `Reason::Done` so the controller continues the turn loop on tool results. **Verify** this matches OpenAI's mapping (it should — OpenAI uses the same convention).

8. **`stop_sequences` field** — DeepSeek accepts a `stop` array of stop sequences. Phase 3d doesn't emit it (matches OpenAI translator's behavior). If a user has a legitimate use case for custom stop sequences, they'd be unable to configure them. **Risk:** Very low — no known user demand. Phase 4 polish if needed.

9. **DeepSeek's prompt-cache behavior** — DeepSeek caches the prompt prefix on the server side. The translator doesn't need to know about this (it's transparent at the API level), but the cache-hit token count surfaced in `usage.prompt_cache_hit_tokens` indicates how much of the prompt was served from cache. **No action needed for Phase 3d.**

---

## Next plan (Phase 4a — `/models` fetch button)

After Phase 3d ships green, Phase 4a targets the per-provider `/models` fetch button. Adds a one-click "Fetch available models" action in the settings card that calls the adapter's `build_probe_request`-style endpoint (`GET /v1/models` for OpenAi/DeepSeek/Anthropic, `GET /api/tags` for Ollama, `GET /v1beta/models` for Gemini), parses the response, and populates the models table with the returned list. Each provider's response shape is different — the adapter trait gains a `parse_models_list(body: &str) -> Result<Vec<RemoteModel>, ProbeError>` method to abstract the parsing. Plan written after Phase 3d is approved + executed.
