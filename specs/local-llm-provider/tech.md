# Tech Spec: Custom Local LLM Provider

**Issue:** none yet
**Companion:** [product.md](./product.md)

## Problem

Every Agent Mode turn is dispatched through `ServerApi::generate_multi_agent_output` (`app/src/server/server_api.rs:1091`), which always POSTs to `{ChannelState::server_root_url()}/ai/multi-agent` and decodes the response as a stream of `warp_multi_agent_api::ResponseEvent` protobuf messages over SSE. The model registry, request schema, and provider list are owned by Warp's closed-source backend and the closed-source proto crate `warp_multi_agent_api` (sourced from `github.com/warpdotdev/warp-proto-apis`).

This spec adds a parallel client-side dispatch path that, when a "local" model is selected, talks directly to a user-configured OpenAI-compatible HTTP endpoint and synthesizes `ResponseEvent`s from OpenAI SSE chunks. No backend changes; no proto changes.

## Relevant code

- `app/src/server/server_api.rs:1091-1183` — `generate_multi_agent_output`. Single dispatch chokepoint. Returns `AIOutputStream<ResponseEvent>` ≡ `BoxStream<Result<ResponseEvent, Arc<AIApiError>>>`. URL hardcoded at lines 1109-1118.
- `app/src/ai/agent/api/impl.rs:12-143` — orchestration wrapper that assembles the `Request`, calls `server_api.generate_multi_agent_output(&request)` at line 132, and applies `take_until(cancellation_rx)`. Right above the dispatch is the natural fork point.
- `app/src/ai/llms.rs:87-122` — `LLMProvider` and `LLMModelHost` enums. Already has the host-routing concept (`DirectApi`, `AwsBedrock`, `Unknown`); we extend it.
- `app/src/ai/llms.rs:289-340` — `AvailableLLMs` populated from server GraphQL. We inject one synthetic `LLMInfo` entry post-fetch when a local provider is configured.
- `app/src/server/server_api/ai.rs:1177,1198` — GraphQL queries that produce `ModelsByFeature`. Convert to `AvailableLLMs` via `TryFrom`. The injection step happens after this conversion.
- `crates/ai/src/api_keys.rs:19-25,52-94,176-212` — `ApiKeyManager` singleton, secure-storage pattern, `ApiKeys` struct. Pattern to mirror; we add a sibling manager rather than overload this one.
- `app/src/settings_view/ai_page.rs:5992-6072` — `create_api_key_editor!` macro for password-masked editor fields. Pattern to extend.
- `app/src/settings_view/mod.rs:188+` — `SettingsSection` / subpage routing. We add a `CustomProviders` subpage.
- `app/src/settings/ai.rs` (existing) and `app/src/settings/cloud_preferences.rs:20-30` — `define_settings_group!` macro for non-secret typed settings. Pattern to extend with `local_provider_*` fields.
- `app/src/ai/blocklist/controller/response_stream.rs:97,160,227-296` — the consumer side: matches `ResponseEvent::Type::{Init, ClientActions, Finished}`, applies retry policy, hands `ClientAction`s to the controller. The synthetic stream the local provider emits must satisfy this contract verbatim.
- `app/src/ai/agent/api/convert_from.rs:197-299` — converts inner `Message` variants (`AgentOutput`, `AgentReasoning`, `ToolCall`) to UI types. The shape we need our adapter to produce.
- `crates/warp_features/src/lib.rs` (`SoloUserByok` near line 803) — pattern for adding the new `LocalLlmProvider` feature flag.
- `crates/http_client/src/lib.rs:55-199` — shared HTTP client. We reuse it.

## Current state

`generate_multi_agent_output` is the only path. Every call site (`response_stream.rs:97,160`, `passive_suggestions/maa.rs`) goes through `agent::api::impl::generate_multi_agent_output` (the wrapper), which calls `server_api.generate_multi_agent_output(&request)` after setting up cancellation. The wrapper takes a `RequestParams` (model id, BYO keys, conversation context, tool list) and returns a stream of `Result<ResponseEvent, Arc<AIApiError>>`.

The model picker is driven by `AvailableLLMs::choices: Vec<LLMInfo>`, populated from GraphQL responses. Stale local IDs get cleared (`llms.rs:927-972`), but injected entries that the controller knows are present in `choices` survive that pass.

## Proposed changes

### Component overview

```
                                    ┌─────────────────────────────┐
                                    │ AgentMode picker            │
                                    │ (sees Local entry injected  │
                                    │ post-GraphQL)               │
                                    └──────────────┬──────────────┘
                                                   │ user selects "Local: <id>"
                                                   ▼
┌────────────────────────────┐    ┌────────────────────────────────┐
│ agent::api::impl           │───▶│ NEW: dispatch_router           │
│ generate_multi_agent_output│    │ (looks at LLMId prefix or      │
│ (wrapper)                  │    │ LLMInfo.host_configs)          │
└────────────────────────────┘    └─────┬──────────────────────────┘
                                        │                       │
                          Local host?  No│                  Yes  │
                                        │                       │
                                        ▼                       ▼
                       ┌────────────────────────┐   ┌─────────────────────────┐
                       │ ServerApi::             │   │ NEW:                    │
                       │ generate_multi_agent_   │   │ local_provider::run_    │
                       │ output (existing)       │   │ chat_turn(req, cfg, ck) │
                       └────────────────────────┘   └─────────────┬───────────┘
                                                                  │
                                                                  ▼ OpenAI SSE
                                                       ┌──────────────────────┐
                                                       │ NEW: openai_to_      │
                                                       │ response_event adapter│
                                                       └─────────────┬────────┘
                                                                     │
                                                                     ▼
                                                  Stream<Result<ResponseEvent, Arc<AIApiError>>>
                                                  (drop-in for the existing return type)
```

### 1. Feature flag

Add `LocalLlmProvider` to `crates/warp_features/src/lib.rs` next to `SoloUserByok`. Off in `RELEASE_FLAGS`/`PREVIEW_FLAGS`; on in `DOGFOOD_FLAGS` after the first round of internal testing. All UI surface and the dispatch fork are gated by `FeatureFlag::LocalLlmProvider.is_enabled()`.

### 2. Settings model

**Non-secret settings** — extend `AISettings` in `app/src/settings/ai.rs` via the existing `define_settings_group!` macro:

```rust
local_provider_enabled: SettingType { type: bool, default: false, ... toml_path: "agents.local_provider.enabled" },
local_provider_display_name: SettingType { type: String, default: "Local".to_string(), toml_path: "agents.local_provider.display_name" },
local_provider_base_url: SettingType { type: String, default: "".to_string(), toml_path: "agents.local_provider.base_url" },
local_provider_model_id: SettingType { type: String, default: "".to_string(), toml_path: "agents.local_provider.model_id" },
local_provider_supports_tools: SettingType { type: bool, default: true, toml_path: "agents.local_provider.supports_tools" },
```

All `sync_to_cloud: SyncToCloud::Never`, `private: false`. (The flag itself stays local — both because Warp Drive currently doesn't sync model config and because users may have an endpoint that's only reachable from one device.)

**Secret (API key)** — store in the OS keychain via the same `secure_storage` API used by `ApiKeyManager`. New singleton `LocalProviderKeyManager` in `crates/ai/src/local_provider_key.rs`:

```rust
const SECURE_STORAGE_KEY: &str = "LocalProviderApiKey";

pub struct LocalProviderKeyManager { key: Option<String> }

impl LocalProviderKeyManager {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self { /* loads JSON */ }
    pub fn key(&self) -> Option<&str>;
    pub fn set_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>);
}
```

Reasoning for a separate manager (vs. extending `ApiKeyManager`): clean blast radius, parallel module that can be deleted without touching the BYO-keys serialization, and `ApiKeys` is a wire-format mirror tied to `warp_multi_agent_api::request::settings::ApiKeys` — adding a non-wire field there confuses readers.

### 3. Provider config aggregation

New module `crates/ai/src/local_provider/config.rs`:

```rust
#[derive(Clone, Debug)]
pub struct LocalProviderConfig {
    pub display_name: String,
    pub base_url: String,
    pub model_id: String,
    pub api_key: Option<String>,
    pub supports_tools: bool,
}

impl LocalProviderConfig {
    pub fn from_app(ctx: &AppContext) -> Option<Self>;       // reads settings + LocalProviderKeyManager
    pub fn synthetic_llm_info(&self) -> LLMInfo;             // see (4)
    pub fn synthetic_llm_id(&self) -> LLMId;                 // formatted "local:<base_model_name>"
    pub fn validate(&self) -> Result<(), ConfigError>;
}
```

`from_app` returns `None` if `local_provider_enabled` is false, the base URL is empty/unparseable, or the model id is empty. This is the single source of truth — the picker injection, the dispatch router, and the request builder all consume `LocalProviderConfig`.

### 4. Picker injection

Extend `LLMModelHost` (`app/src/ai/llms.rs:108`) with a new variant **at the end** to keep wire deserialization happy:

```rust
pub enum LLMModelHost {
    DirectApi,
    AwsBedrock,
    Local,
    #[serde(other)]
    Unknown,
}
```

Add a post-fetch hook in the place that builds `AvailableLLMs` from `ModelsByFeature` (in `app/src/server/server_api/ai.rs` near the `TryFrom<ModelsByFeature> for AvailableLLMs` impls around line 2014). After producing the server-side list, call:

```rust
if let Some(cfg) = LocalProviderConfig::from_app(ctx) {
    available.choices.push(cfg.synthetic_llm_info());
}
```

`synthetic_llm_info()` produces:

```rust
LLMInfo {
    display_name: format!("{}: {}", cfg.display_name, cfg.model_id),
    base_model_name: cfg.model_id.clone(),
    id: cfg.synthetic_llm_id(),                          // e.g. "local:llama3.1"
    reasoning_level: None,
    usage_metadata: LLMUsageMetadata { request_multiplier: 0, credit_multiplier: None },
    description: Some("Custom local provider".into()),
    disable_reason: None,
    vision_supported: false,
    spec: None,
    provider: LLMProvider::Unknown,
    host_configs: HashMap::from([(LLMModelHost::Local, RoutingHostConfig {
        enabled: true,
        model_routing_host: LLMModelHost::Local,
    })]),
    discount_percentage: None,
}
```

The cleanup pass at `llms.rs:927-972` already keeps IDs that are present in `choices`, so the local model survives across reloads.

The picker rendering (`execution_profiles/model_menu_items.rs:147`) gets a small tweak: when `provider == Unknown && host_configs contains Local`, suppress the credit/cost label and show `<endpoint host>` as a subtext hint instead.

### 5. Dispatch router

Refactor `app/src/ai/agent/api/impl.rs::generate_multi_agent_output` (the wrapper that today directly calls `server_api.generate_multi_agent_output`) into a small router:

```rust
pub async fn generate_multi_agent_output(
    server_api: &ServerApi,
    params: RequestParams,
    cancel_rx: oneshot::Receiver<()>,
    ctx: &mut AppContext,
) -> Result<AIOutputStream<ResponseEvent>, Arc<AIApiError>> {
    if FeatureFlag::LocalLlmProvider.is_enabled()
        && is_local_model_id(&params.model)
        && let Some(cfg) = LocalProviderConfig::from_app(ctx)
    {
        let stream = local_provider::run_chat_turn(params, cfg, cancel_rx, ctx).await?;
        return Ok(stream);
    }

    // existing path
    let request = build_request(params, ...);
    let stream = server_api.generate_multi_agent_output(&request).await?;
    Ok(Box::pin(stream.take_until(async move { let _ = cancel_rx.await; })))
}
```

`is_local_model_id` checks the `local:` prefix. We don't read `host_configs` here because the serialized cache might carry stale data; the prefix-on-LLMId is unambiguous and round-trips through preferences cleanly.

### 6. Local provider client

New crate module `crates/ai/src/local_provider/` with three files:

**`mod.rs`** — public entry point:
```rust
pub async fn run_chat_turn(
    params: RequestParams,
    cfg: LocalProviderConfig,
    cancel_rx: oneshot::Receiver<()>,
    ctx: &mut AppContext,
) -> Result<AIOutputStream<ResponseEvent>, Arc<AIApiError>>;
```

Steps:
1. Translate `params` → OpenAI `ChatCompletionRequest` (see `request.rs`).
2. POST to `{base_url}/chat/completions` with `Authorization: Bearer <key>` if set, `Accept: text/event-stream`, body with `"stream": true`.
3. Use the shared `http_client::Client` (returned by `ServerApi::http_client()`).
4. Pipe `.eventsource()` through the OpenAI-SSE → `ResponseEvent` adapter.
5. Wrap the resulting stream in `take_until(cancel_rx)` so cancellation matches the existing behavior.
6. Map all transport/decode failures into the existing `AIApiError` variants (`Transport`, `Stream`, `ErrorStatus`, `Deserialization`).

**`request.rs`** — request translation. Maps the in-memory `RequestParams` to OpenAI-format JSON.

Mapping rules:
- `model` ← `cfg.model_id` (NOT the synthetic `LLMId`).
- `messages` ← walk the existing conversation history and prior tool turns. System prompt synthesized from Warp's existing prompt builder (the same builder feeds `metadata.logging_dict` today; we extract it into a reusable `compose_system_prompt(params)`). Roles map: user → `user`, assistant text → `assistant`, tool result → `tool` with `tool_call_id`.
- `tools` ← only if `cfg.supports_tools`. Translate Warp's `SupportedTools` enum into OpenAI tool definitions. The translation table lives in `request.rs::tool_definitions()` and is unit-tested. Definitions are static (no per-call schema rewrites needed).
- `tool_choice` ← `"auto"`.
- `stream` ← `true`.
- `temperature`, `max_tokens` ← omitted; let the server decide.

**`response.rs`** — SSE → `ResponseEvent` adapter. The hard part. Output contract: emit exactly `Init` first, then one or more `ClientActions`, then `Finished`. The state machine:

```
state = StreamStart
emit Init { request_id: uuid::new_v4().to_string() }

for chunk in sse_stream {
    if chunk == "[DONE]" { state = StreamDone; break; }
    let ChatCompletionChunk { choices: [c], .. } = parse(chunk);

    // visible content
    if let Some(text) = c.delta.content {
        emit ClientActions { actions: [Action::AppendToMessageContent {
            message_kind: MessageKind::AgentOutput, content: text,
        }] };
    }
    // reasoning content (DeepSeek/Qwen `delta.reasoning_content`,
    // OpenAI `delta.reasoning`, or inline <think>...</think> tags)
    if let Some(reasoning) = extract_reasoning(c.delta) {
        emit ClientActions { actions: [Action::AppendToMessageContent {
            message_kind: MessageKind::AgentReasoning, content: reasoning,
        }] };
    }
    // tool calls (streamed in fragments — accumulate by index)
    for tc_delta in c.delta.tool_calls.unwrap_or_default() {
        accumulator.append(tc_delta);
        if accumulator.is_complete(tc_delta.index) {
            emit ClientActions { actions: [Action::AddMessagesToTask {
                messages: [ Message::ToolCall(accumulator.take(tc_delta.index)) ]
            }] };
        }
    }
    // finish
    if let Some(reason) = c.finish_reason {
        state = StreamDone;
        let mapped = match reason {
            "stop" => StreamFinishedReason::Done,
            "length" => StreamFinishedReason::MaxTokenLimit,
            "tool_calls" => StreamFinishedReason::Done,
            "content_filter" => StreamFinishedReason::Other,
            _ => StreamFinishedReason::Other,
        };
        emit Finished { reason: mapped };
        break;
    }
}

if state != StreamDone {
    emit Finished { reason: StreamFinishedReason::InternalError };
}
```

The actual proto names (`Action::AppendToMessageContent`, `Message::ToolCall`, etc.) come from the closed-source `warp_multi_agent_api` crate. The adapter relies on those types being public; if any are private we expose a small free-standing builder API in our codebase that constructs them via the public proto methods used by `convert_from.rs:197-299` today.

Tool-call fragment accumulation is the one piece of subtle state: OpenAI streams the `function.arguments` JSON as a string of partial deltas keyed by `index`. The accumulator (`response.rs::ToolCallBuffer`) joins them and emits the complete call on first `finish_reason` or first sight of a new index.

### 7. Settings UI

New file `app/src/settings_view/ai_page/custom_providers.rs` rendering a `CustomProvidersPage`:

- Reuses the `create_api_key_editor!` macro for the API-key field (password-masked).
- Plain `EditorView` (non-masked) for base URL, model id, display name.
- Checkboxes for `enabled` and `supports_tools` (uses existing `Checkbox` widget).
- A **Test connection** button that calls `local_provider::run_test_completion(cfg)`, a non-streaming variant that POSTs `messages: [{role: "user", content: "ping"}]` with `stream: false` and renders the model's reply (or the HTTP error) inline.

Wire into the AI subpage tree: add `AISubpage::CustomProviders` to `app/src/settings_view/ai_page.rs:93`. Page is hidden behind `FeatureFlag::LocalLlmProvider.is_enabled()`.

### 8. Cancellation, errors, retries

- Cancellation: same `oneshot::Receiver<()>` plumbing as today; the local stream is wrapped in `take_until` exactly like the server stream (`agent/api/impl.rs:135`).
- Errors: produce `Arc<AIApiError>` items. Mapping:
  - `reqwest::Error` → `AIApiError::Transport`
  - HTTP non-2xx → `AIApiError::ErrorStatus(status, body_first_200_chars)`
  - JSON parse failure on a chunk → `AIApiError::Deserialization`
  - Mid-stream IO failure → `AIApiError::Stream { stream_type: "local-provider", source }`
  - `serde_json::Error` translating tool-call deltas → `AIApiError::Deserialization` plus a synthesized `Finished { Other }`
- Retries: the existing controller (`response_stream.rs:274-296`) retries on certain `AIApiError` variants up to 3 times. We piggyback that — by producing the same error variants we get retries for free on the local path. (We may want to opt-out for `ErrorStatus(401)` since retrying a bad key is wasted; that lives in a follow-up.)

### 9. Network audit gates

A small unit test in `crates/ai/src/local_provider/mod_tests.rs` constructs a `RequestParams` with a `local:` model id and asserts the resulting `Request` to `ServerApi::generate_multi_agent_output` is **never built**. This protects us against accidentally regressing the "no warp.dev traffic for the LLM call" guarantee.

## End-to-end flow (happy path)

1. Feature flag on, settings populated, Ollama running on `localhost:11434`, model `llama3.1` selected.
2. User sends `Summarize file foo.txt`.
3. `agent::controller` builds a `RequestParams` and calls `agent::api::impl::generate_multi_agent_output`.
4. Router sees `params.model.as_str().starts_with("local:")` and routes to `local_provider::run_chat_turn`.
5. `request.rs` builds an OpenAI `ChatCompletionRequest`: system prompt + history + `tools` (because `supports_tools=true`) + `model="llama3.1"` + `stream=true`.
6. `http_client::Client::post("http://localhost:11434/v1/chat/completions")` with `Authorization` header (if any), body JSON-encoded.
7. SSE stream returned. Adapter emits `Init` first.
8. First chunk: `delta.content="I'll need to read foo.txt."` → emit `ClientActions { AppendToMessageContent(AgentOutput) }`.
9. Next chunks: `delta.tool_calls[0]` arrives in 4 fragments. Accumulator joins them; when `finish_reason="tool_calls"` lands, emit `ClientActions { AddMessagesToTask([ToolCall]) }` then `Finished { Done }`.
10. Controller (unchanged) executes the tool call via Warp's existing tool runner, sends a follow-up turn with the tool result back through the same router. Subsequent turns work the same way.
11. Cancellation, retries, error toasts, conversation persistence, and the picker UI all behave exactly as in the server path because we satisfy the `ResponseEvent` contract.

## Risks and mitigations

**Risk:** `warp_multi_agent_api` proto types (`ResponseEvent`, `Action`, `Message::ToolCall`) might be `pub(crate)` or only constructible via builders that assume server-origin invariants.
**Mitigation:** First task of phase 1 is to verify constructibility from outside the crate by writing a "build a fake `Init+Finished` stream" smoke test in `crates/ai/src/local_provider/mod_tests.rs`. If a needed type is sealed, file an upstream change in `warp-proto-apis` to expose a builder; until then, the dependent code can land behind `#[cfg(feature = "local_llm")]` rather than the runtime flag.

**Risk:** OpenAI tool-call streaming format inconsistencies across servers (Ollama vs vLLM vs LM Studio sometimes emit `function.arguments` whole vs in fragments, or non-string `arguments`).
**Mitigation:** The accumulator handles both whole-arg and fragment-streamed cases. Add server-specific fixtures in `response_tests.rs` covering Ollama 0.4, LM Studio 0.3, vLLM 0.6, llama.cpp `server`. Document the supported matrix in `product.md`.

**Risk:** Reasoning content extraction is heterogeneous (`<think>` tags inline in `delta.content` vs. a separate `delta.reasoning_content` field vs OpenAI o1's `reasoning_summary`).
**Mitigation:** `extract_reasoning` is a small dispatcher with provider-agnostic heuristics: prefer explicit `reasoning_content` field if present; otherwise scan `delta.content` for `<think>...</think>` and split. Unit-test each path.

**Risk:** Bypassing `warp.dev` means we lose the server-side rate-limit and prompt-injection guards.
**Mitigation:** Document in product.md (already covered). The endpoint is the user's, so the threat model is "user trusts their own endpoint". For shared/team configs that's a future concern.

**Risk:** Conversation history replay assumes a specific message-shape that Warp's server tolerates. A local model may reject a system prompt that's longer than its context.
**Mitigation:** No client-side context check in v1. The HTTP error path renders the model's rejection. Add a follow-up to surface a "history too long, consider /clear" hint when we see `context_length_exceeded` style errors in the response body.

**Risk:** A user sets `base_url=https://api.openai.com/v1` and sends their OpenAI key — at that point we're a thin OpenAI client, sidestepping Warp's billing and routing.
**Mitigation:** That's intentional; users opting into a custom provider take the consequences. Settings copy says "this endpoint will receive your full conversation directly". No safeguard.

**Risk:** Adding `LLMModelHost::Local` mid-enum could break older serialized caches.
**Mitigation:** Append at the end (before `#[serde(other)] Unknown`). The existing `#[serde(other)]` arm catches cache values from a future-newer-variant; the new variant only appears after a binary that knows about it writes it.

## Testing and validation

| Invariant from product.md | Validation |
|---|---|
| 1, 9 (picker entry shows / hides) | Unit test on `synthetic_llm_info()` + integration assertion that `AvailableLLMs::choices` contains a `local:*` ID iff `LocalProviderConfig::from_app` returns `Some`. |
| 2, 3 (text streams; no warp.dev traffic) | Integration test `tests/local_provider_chat.rs` under `crates/integration/` boots a 50-line mock OpenAI server, sends a turn, asserts content received and asserts the test harness's outbound-HTTP recorder shows zero requests to any `*.warp.dev` host. |
| 4 (tool calls execute) | Integration test that the mock server emits a streamed tool-call for `read_file`; assert the Warp tool runner is invoked with the right args and a follow-up turn lands. |
| 5 (reasoning rendering) | Unit test on the SSE adapter: feed a fixture with `<think>...</think>` and a fixture with `delta.reasoning_content`; assert two `AppendToMessageContent(AgentReasoning)` events are emitted. |
| 6 (endpoint down → graceful error) | Integration test pointing to a closed port; assert the `Finished { reason: Other }` arrives and the error-toast text contains the configured display name. |
| 7 (Authorization header) | Unit test on `request.rs::build_http_request` asserts the header is present iff the key is set, and absent otherwise. |
| 8 (no tools when disabled) | Same unit test; `tools` field present iff `supports_tools=true`. |
| 10 (keychain cleared on key removal) | Unit test on `LocalProviderKeyManager::set_key(None)` reads back `None` and the `secure_storage` mock records a delete. |

Plus:

- **OpenAI-SSE adapter coverage**: 25+ fixtures in `response_tests.rs` covering text-only, text+tool, tool-only, multi-tool-interleaved, malformed JSON mid-stream, premature disconnect, [DONE] without `finish_reason`, `finish_reason` without [DONE], reasoning variants, empty `choices`, server-sent error event.
- **Network audit unit test** mentioned in §9.
- **Manual smoke matrix**: Ollama 0.4 (Mac and Linux), LM Studio 0.3 (Mac), vLLM 0.6 (Linux), llama.cpp server (Mac), and an NVIDIA NIM endpoint over HTTPS with bearer auth. Each: text turn + tool turn + cancel mid-stream + invalid key.
- **Network audit (manual)**: mitmproxy/Charles run while exercising a turn, confirms zero requests to `*.warp.dev` (other than non-AI features like telemetry, which are out of scope but should be audited and documented).

## Follow-ups (out of scope for v1)

- Multiple custom providers.
- `/v1/models` discovery for the model id field (autocomplete from the endpoint).
- Vision / image input.
- Anthropic-format wire support for direct Claude calls.
- Auto-detect running local servers (Ollama/LM Studio default ports).
- Surface `context_length_exceeded` errors as a "consider /clear" hint.
- Re-export proto builders from the `warp-proto-apis` repo if the constructibility-risk path requires it.
- Telemetry counter (`local_provider_turn`, `local_provider_test_connection_*`).
