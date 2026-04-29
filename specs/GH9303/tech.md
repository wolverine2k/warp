# Tech Spec: Custom Local LLM Provider

**Issue:** [warpdotdev/warp#9303](https://github.com/warpdotdev/warp/issues/9303)
**Companion:** [product.md](./product.md), [implementation-plan.md](./implementation-plan.md), [test-plan.md](./test-plan.md)

## Problem

Every Agent Mode turn is dispatched through `ServerApi::generate_multi_agent_output` (`app/src/server/server_api.rs:1091`), which always POSTs to `{ChannelState::server_root_url()}/ai/multi-agent` and decodes the response as a stream of `warp_multi_agent_api::ResponseEvent` protobuf messages over SSE. The model registry, request schema, and provider list are owned by Warp's closed-source backend; the wire types are open-source in `github.com/warpdotdev/warp-proto-apis`.

This spec adds a parallel client-side dispatch path that, when a "local" model is selected, talks directly to a user-configured OpenAI-compatible HTTP endpoint and synthesizes `ResponseEvent`s from OpenAI SSE chunks. **No backend changes and no proto changes are required for v1.** The proto crate is built via `prost_reflect_build`, which generates all messages and oneof variants as `pub` Rust types — confirmed by reading `apis/multi_agent/v1/gen/rust/build.rs` and the `.proto` sources directly. The constructibility concern flagged in earlier drafts is therefore obsolete; we can build any `ResponseEvent`/`ClientAction`/`Message` variant from outside the crate.

## Architectural choice

This spec implements **Path 1 (client-owned orchestration)** as defined in product.md §"Architectural choice". The two implications that touch every section below:

1. **We re-author the system prompt.** Warp's backend prepends a system prompt to every turn before sending to the LLM. That prompt is not in the OSS client. We ship a generic agent system prompt in `crates/ai/src/local_provider/prompt.rs`.
2. **We re-author tool schemas.** Warp's backend translates `Settings.supported_tools: repeated ToolType` into model-specific tool definitions before calling the LLM. That translation isn't in the OSS client either. We ship a JSON-schema table for the curated initial tool set in `crates/ai/src/local_provider/tools.rs`, AND the inverse — translating an OpenAI `tool_call` back into the proto's strongly-typed `Message::ToolCall.tool` oneof variant.

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
local_provider_context_window: SettingType { type: u32, default: 0, toml_path: "agents.local_provider.context_window" },  // 0 means "unset/auto"
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
    /// Optional context-window size in tokens, surfaced in the system prompt
    /// when populated. Read from the corresponding setting; `None` means
    /// "omit from prompt and let the model handle context limits itself".
    pub context_window: Option<u32>,
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

The actual signature of `app/src/ai/agent/api/impl.rs::generate_multi_agent_output` is:

```rust
pub async fn generate_multi_agent_output(
    server_api: Arc<ServerApi>,
    mut params: RequestParams,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<AIOutputStream<ResponseEvent>, Arc<AIApiError>>;
```

Note: **no `&AppContext` parameter, and the function is called from inside an async task** that doesn't own one. The router must therefore make its decisions purely from `params`. The fix is to extend `RequestParams` with an opt-in snapshot of the local-provider config, populated at every call site (which DOES have `AppContext`):

```rust
// in RequestParams (new field; default None)
pub local_provider_config: Option<LocalProviderConfig>,
```

Call sites that build `RequestParams` (`response_stream.rs:97`, `response_stream.rs:160`, `passive_suggestions/maa.rs`) snapshot `LocalProviderConfig::from_app(ctx)` while still on the `AppContext`-owning thread:

```rust
// at the call site, where `ctx: &mut AppContext` is in scope
let local_provider_config = if FeatureFlag::LocalLlmProvider.is_enabled() {
    LocalProviderConfig::from_app(ctx)
} else {
    None
};
let params = RequestParams { /* existing fields */, local_provider_config, .. };
generate_multi_agent_output(server_api, params, cancellation_rx).await
```

Inside the dispatch function, the router becomes:

```rust
pub async fn generate_multi_agent_output(
    server_api: Arc<ServerApi>,
    mut params: RequestParams,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<AIOutputStream<ResponseEvent>, Arc<AIApiError>> {
    if let (true, Some(cfg)) = (is_local_model_id(&params.model), params.local_provider_config.take()) {
        return local_provider::run_chat_turn(params, cfg, cancellation_rx, server_api.http_client()).await;
    }

    // existing path
    let request = build_request(params, /* ... */);
    let stream = server_api.generate_multi_agent_output(&request).await?;
    Ok(Box::pin(stream.take_until(async move { let _ = cancellation_rx.await; })))
}
```

`is_local_model_id` checks the `local:` prefix on `params.model`. The snapshot approach has three nice properties: (1) the dispatch function stays `AppContext`-free, preserving the existing async ownership model; (2) cancelling the local provider mid-config-change is safe — the in-flight request continues with the snapshot it captured; (3) tests can construct a `RequestParams` with any synthetic config, no `AppContext` mock needed. The cost is that every call site grows two lines, an acceptable trade.

The HTTP client is passed through explicitly via `server_api.http_client()` rather than reaching for a global so the local provider runs against the same retry/timeout/proxy config as the server path.

**Snapshot cost.** `LocalProviderConfig::from_app(ctx)` reads the `AISettings` singleton (cheap) plus a single `secure_storage::read_value` call to fetch the API key (cheap on the hot path because the OS keychain stays unlocked across calls within a session, but **not** zero — macOS Keychain Services and Linux Secret Service both involve IPC). For high-frequency call sites such as `passive_suggestions/maa.rs`, this adds ~1ms per build. v1 accepts that cost because passive-suggestion turns won't route to the local provider anyway (the model id won't match `local:*`). If profiling later shows the per-call cost is a regression, the fix is a `LocalProviderConfigCache` singleton that caches the snapshot and invalidates on the existing `ApiKeyManagerEvent::KeysUpdated` event plus an analogous event from `LocalProviderKeyManager`. Listed in follow-ups.

**Synthetic conversation_id assumption.** The adapter sets `conversation_id = "local:{uuid}"` in `StreamInit` (§6 below). The existing controller round-trips this string into request metadata on subsequent turns (`Request.Metadata.conversation_id`) — verified by reading `app/src/ai/blocklist/controller/conversation.rs`'s persistence path, which treats the field as an opaque string. There is no client-side format validation. The `local:` prefix is therefore safe, distinguishes local conversations from server-issued IDs in logs/db rows, and round-trips correctly. Test §5.12 in test-plan.md exercises a two-turn local conversation and asserts the second turn carries the same `local:{uuid}` value the first turn returned. **No backend traffic is generated for that round-trip** — the value never leaves the client.

### 6. Local provider client

New crate module `crates/ai/src/local_provider/` with three files:

**`mod.rs`** — public entry point:
```rust
pub async fn run_chat_turn(
    params: RequestParams,
    cfg: LocalProviderConfig,
    cancel_rx: oneshot::Receiver<()>,
    http: http_client::Client,                 // shared client from ServerApi::http_client()
) -> Result<AIOutputStream<ResponseEvent>, Arc<AIApiError>>;
```

The function is `AppContext`-free by design (per §5 — config was already snapshotted into `cfg` at the call site, and the HTTP client is passed in so the local path inherits the same retry/timeout/proxy config the server path uses). This is intentional and aligns with B3's resolution.

Steps:
1. Translate `params` → OpenAI `ChatCompletionRequest` (see `request.rs`).
2. POST to `{base_url}/chat/completions` with `Authorization: Bearer <key>` if set, `Accept: text/event-stream`, body with `"stream": true`.
3. Use the shared `http_client::Client` (returned by `ServerApi::http_client()`).
4. Pipe `.eventsource()` through the OpenAI-SSE → `ResponseEvent` adapter.
5. Wrap the resulting stream in `take_until(cancel_rx)` so cancellation matches the existing behavior.
6. Map all transport/decode failures into the existing `AIApiError` variants (`Transport`, `Stream`, `ErrorStatus`, `Deserialization`).

**`request.rs`** — request translation. Maps the in-memory `RequestParams` (plus any existing conversation history) to OpenAI-format JSON.

Mapping rules:
- `model` ← `cfg.model_id` (NOT the synthetic `LLMId`).
- `messages` ← `[ {role: "system", content: compose_system_prompt(...)} ]` followed by a walk of the existing conversation. Roles map: user query → `user`; `Message::AgentOutput` → `assistant`; `Message::ToolCall` → assistant message with `tool_calls` array; `Message::ToolCallResult` (incoming as `Input.UserInputs.tool_call_result` on continuation turns) → `{role:"tool", tool_call_id, content}`. `Message::AgentReasoning` is intentionally NOT replayed in history (matches OpenAI's `reasoning` behavior — only the final assistant text persists across turns).
- `tools` ← only if `cfg.supports_tools`. Pulled from `tools::tool_definitions()` (see §6.5 below).
- `tool_choice` ← `"auto"`.
- `stream` ← `true`.
- `temperature`, `max_tokens` ← omitted; let the server decide.

### 6.4 System prompt — authored, not extracted

**Critical:** the comment on issue #9303 from `Aeromix` and the corroborating Opus analysis confirm that Warp's system prompt is constructed server-side and **never reaches the OSS client**. There is no `compose_system_prompt` to extract.

Therefore `crates/ai/src/local_provider/prompt.rs` ships a hand-authored, model-agnostic system prompt with the following constituents:

1. **Role framing** — "You are a coding assistant operating inside the Warp terminal..."
2. **Available tools** — short prose description of the tools the model can call (must match the schemas in `tools.rs`). Reads from the same registry so prompt and schemas can never drift.
3. **Output format guidance** — "If the user asks you to take an action that requires a tool, emit a tool call. Otherwise, respond in plain Markdown."
4. **Diff format** — when `apply_file_diffs` is in the tool set, the prompt instructs the model to emit **search/replace blocks** (the simpler `FileDiff { file_path, search, replace }` shape from the proto's `ApplyFileDiffsArgs`, not the more complex V4A hunk format also supported by that message). Search/replace is dramatically easier for smaller local models to produce reliably; V4A is left for a follow-up gated on `supports_v4a_file_diffs`. The prompt shows one worked example so the model's output is predictable.
5. **Safety guardrails** — "Do not run destructive commands without confirmation"; "When uncertain, ask"; minimal versions of Warp's published guidance.
6. **Context-window hint** — `"You have approximately N tokens of context"` if `cfg.context_window` is set; omitted otherwise.

The prompt is a `const TEMPLATE: &str = "..."` with `{tools}` and `{context_window}` substitution slots, rendered at request build time. It's checked-in plain text, code-reviewed like any other source file. **It is the single largest reason the quality gap in product.md exists; iterating on it is expected post-launch.**

### 6.5 Tool schemas and tool-call translation — bidirectional

The proto's `Message::ToolCall.tool` oneof at `apis/multi_agent/v1/task.proto:357-880` has 33 strongly-typed variants today (fields 2-34, including recent additions `AskUserQuestion`, `StartAgentV2`, `UploadFileArtifact`). Each variant has its own structured Rust type with named fields — there is no generic `(name, args_json)` pair anywhere in the wire protocol. v1 covers 5; the remaining ~28 are out of scope and silently absent from local-model tool listings.

Implication: the local-provider adapter must implement TWO mappings, not one:

- **Outbound (request → OpenAI `tools` field):** Warp tool variant → OpenAI tool definition (`{type:"function", function:{name, description, parameters: <JSON schema>}}`).
- **Inbound (OpenAI tool_call → proto):** OpenAI's `{name, arguments: <JSON string>}` → the matching `Message::ToolCall.tool::*` Rust enum variant with all its typed fields populated.

Both live in `crates/ai/src/local_provider/tools.rs`:

```rust
pub struct ToolDef {
    name: &'static str,
    description: &'static str,
    json_schema: &'static str,                                       // raw JSON
    parse_args: fn(&str) -> Result<task_proto::message::tool_call::Tool, ToolParseError>,
}

pub fn tool_definitions(supported: &[ToolType]) -> Vec<OpenAiToolDefinition>;
pub fn translate_openai_tool_call(call: &OpenAiToolCall) -> Result<task_proto::message::ToolCall, ToolParseError>;
```

**v1 ships exactly five `ToolDef`s** (the curated set from product.md): `read_files`, `apply_file_diffs`, `run_shell_command`, `grep`, `file_glob_v2`. Each:

1. Has a literal JSON-schema string ([draft-07](https://json-schema.org/draft-07/schema#) compatible — what every OpenAI-compatible server accepts).
2. Has a `parse_args` function that takes the OpenAI-emitted `arguments` string (model-produced JSON) and produces the strongly-typed proto variant.
3. Is fully unit-tested with both happy-path and malformed-input fixtures (see test-plan.md §1.5).

**Schemas are not auto-generated from the proto** because the proto fields (`run_shell_command_id`, `wait_until_complete_value`, etc.) carry server-side semantics the model shouldn't see. The hand-curated schemas expose the minimal user-friendly surface (`command`, `cwd`, `purpose` for `run_shell_command`) and the `parse_args` step fills server-required defaults.

Tools NOT shipped in v1 (MCP, computer-use, web-search, code-review, todos, etc.) are simply absent from `tool_definitions()`, so the local model never knows they exist. Existing UI code that handles them (which is server-action-driven) won't fire on local turns. No code path requires them to be present.

### 6.6 Transactions

The proto's `ClientAction` oneof includes `BeginTransaction` / `CommitTransaction` / `RollbackTransaction` (`response.proto:284-290`). Warp's existing controller logic uses these on the server path so failed mid-stream actions are atomically rolled back.

The local-provider adapter wraps each turn:

```
emit Init
emit ClientActions { BeginTransaction }
... incremental AppendToMessageContent / AddMessagesToTask events ...
on success → emit ClientActions { CommitTransaction } → Finished{Done}
on stream error → emit ClientActions { RollbackTransaction } → Finished{Other}
```

This makes partial-failure cleanup automatic, matches server-path semantics, and gives users a clean retry experience without orphaned half-rendered turns.

**Edge cases to verify against the controller's transaction state machine** (`app/src/ai/blocklist/controller.rs` transaction handling around the conversation auto-resume site at `controller.rs:~2485`):

- A new local turn must not begin if the controller already has an open transaction from a prior server-path stream (e.g., a crash mid-stream that never emitted Commit/Rollback). The wrapper checks `controller.has_open_transaction()` before dispatching; if true, it emits `RollbackTransaction` first to drain the prior state.
- Emitting `CommitTransaction` followed by additional `ClientActions` is a controller-state violation; the adapter must order them strictly: incremental updates → Commit OR Rollback → Finished, with no further `ClientActions` between Commit/Rollback and Finished.
- A user-initiated cancel mid-stream must result in `RollbackTransaction` (not Commit), so that the partial assistant turn is discarded rather than persisted.
- These ordering invariants are tested in test-plan.md §5.9.

**`response.rs`** — SSE → `ResponseEvent` adapter. The hard part. Output contract: emit exactly one `Init`, then one `ClientActions{BeginTransaction}`, then ≥0 `ClientActions{...}`, then `ClientActions{CommitTransaction}` or `ClientActions{RollbackTransaction}`, then exactly one `Finished`. The state machine in `Rust` pseudocode using the actual proto types:

```rust
use warp_multi_agent_api::{
    response_event::{self, stream_finished, StreamInit, StreamFinished, ClientActions},
    client_action::{self, Action, AddMessagesToTask, AppendToMessageContent,
                    BeginTransaction, CommitTransaction, RollbackTransaction},
    message::{self, Message, AgentOutput, AgentReasoning, tool_call},
    ResponseEvent, ClientAction,
};
use uuid::Uuid;

let conversation_id = format!("local:{}", Uuid::new_v4());
let request_id = Uuid::new_v4().to_string();
let run_id = Uuid::new_v4().to_string();

emit ResponseEvent { r#type: Some(response_event::Type::Init(StreamInit {
    conversation_id, request_id, run_id,
})) };
emit ResponseEvent { r#type: Some(response_event::Type::ClientActions(ClientActions {
    actions: vec![ClientAction { action: Some(client_action::Action::BeginTransaction(BeginTransaction {})) }],
})) };

for chunk in sse_stream {
    if chunk == "[DONE]" { break; }
    let ChatCompletionChunk { choices, .. } = parse(chunk)?;
    let Some(c) = choices.first() else { continue };           // empty `choices` is silent

    // visible content
    if let Some(text) = c.delta.content.clone() {
        emit_append_to_message_content(MessageKind::AgentOutput, text);
    }
    // reasoning content (`delta.reasoning_content` for DeepSeek/Qwen,
    // `delta.reasoning` for OpenAI, or inline <think>...</think>)
    if let Some(reasoning) = extract_reasoning(&c.delta) {
        emit_append_to_message_content(MessageKind::AgentReasoning, reasoning);
    }
    // tool calls (fragments accumulated by index)
    for tc_delta in c.delta.tool_calls.iter().flatten() {
        accumulator.append(tc_delta);
        if accumulator.is_complete(tc_delta.index) {
            let tool_call: message::ToolCall = tools::translate_openai_tool_call(&accumulator.take(tc_delta.index))?;
            emit ResponseEvent { r#type: Some(response_event::Type::ClientActions(ClientActions {
                actions: vec![ClientAction { action: Some(Action::AddMessagesToTask(AddMessagesToTask {
                    task_id: current_task_id.clone(),
                    messages: vec![Message { message: Some(message::Message::ToolCall(tool_call)) }],
                })) }],
            })) };
        }
    }
    if let Some(reason) = &c.finish_reason {
        finish_reason = Some(map_finish_reason(reason));   // see helper below
        break;
    }
}

// Commit or rollback, then Finished
let (closing_action, finish_inner) = match (state, finish_reason) {
    (Healthy, Some(reason)) => (CommitTransaction {}, reason),
    _                       => (RollbackTransaction {}, stream_finished::Reason::Other(stream_finished::Other {})),
};
emit ResponseEvent { r#type: Some(response_event::Type::ClientActions(ClientActions {
    actions: vec![ClientAction { action: Some(closing_action.into()) }],   // .into() picks Commit vs Rollback variant
})) };
emit ResponseEvent { r#type: Some(response_event::Type::Finished(StreamFinished {
    reason: Some(finish_inner),
    .. Default::default()
})) };
```

`map_finish_reason(&str) -> stream_finished::Reason`:
- `"stop"`, `"tool_calls"` → `Reason::Done(stream_finished::Done {})`
- `"length"` → `Reason::MaxTokenLimit(stream_finished::ReachedMaxTokenLimit {})`
- `"content_filter"`, anything else → `Reason::Other(stream_finished::Other {})`
- Stream EOF without a `finish_reason` → `Reason::InternalError(stream_finished::InternalError { message: "stream ended without finish_reason".into() })`

`emit_append_to_message_content(kind, text)` constructs an `AppendToMessageContent { task_id, message: Message{...}, mask: FieldMask{ paths: vec!["content"] } }` per `response.proto:266-273` — the FieldMask names the string field on the inner `Message` whose content is appended. For `AgentOutput`, the mask path is `agent_output.content`; for `AgentReasoning`, it's `agent_reasoning.reasoning`. The exact field-mask paths are validated against the controller in test-plan.md §5.9.

The proto types are constructible from outside the crate (resolved risk; see top of file). All variants used here are confirmed against `apis/multi_agent/v1/response.proto:17-211` and `apis/multi_agent/v1/task.proto:164-330`.

Tool-call argument fragments are buffered until either (a) `finish_reason` arrives or (b) a fragment with a higher `index` arrives, signaling the previous index is complete. This matches OpenAI's documented streaming behavior and is what every OpenAI-compatible server emits.

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
- Retries: the existing controller (`response_stream.rs` retry block, ~lines 269-296) retries on certain `AIApiError` variants up to 3 times, **gated on `is_online` (a global Warp connectivity check) AND `!has_received_client_actions`**. This semantics is wrong for local turns: a `localhost` endpoint is reachable even when the user is offline, and Warp's connectivity check has nothing to say about it. v1 accepts this mismatch (the controller will retry only when *both* warp.dev and the local endpoint look healthy, which over-restricts retry coverage but never produces incorrect behavior). The proper fix is a follow-up that adds a per-provider reachability check; documented in product.md Open Questions / Follow-ups.
- We may want to opt-out of retries for `ErrorStatus(401)` since retrying a bad key is wasted; that lives in a follow-up.

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

**Risk (RESOLVED, kept for traceability):** `warp_multi_agent_api` proto types might be sealed.
**Resolution:** Verified directly against the proto repo at `/Users/nmehta/Documents/code/github/warp-proto-apis` (HEAD `aa2f9cd`). The Rust crate is generated by `prost_reflect_build` (see `apis/multi_agent/v1/gen/rust/build.rs`); `prost-build` produces all messages, oneof variants, and enums as `pub` Rust types by convention. `ResponseEvent`, `ClientAction`, `Message`, `Message::ToolCall`, etc. are all constructible from outside the crate via direct struct literals and `Default::default()`. No upstream change required.

**Risk (NEW, primary):** Quality of the hand-authored system prompt and tool schemas determines the local-provider experience. A bad prompt/schema combination produces models that emit unparseable tool calls, run away on autonomy, or refuse to call tools at all.
**Mitigation:** §6.4 commits to checking in the prompt as plain text and code-reviewing iteratively. §6.5 commits to a tight initial tool set (5 tools) so each schema can be exhaustively round-trip-tested against real model output (test-plan.md §5.9). Phase 8 gates promotion on the manual smoke matrix passing for at least three different model families. Quality gap is documented in product.md so user expectations are calibrated.

**Risk (NEW):** OpenAI tool-call argument JSON deserialization into the strongly-typed `Message::ToolCall.tool` oneof can fail in dozens of subtle ways: missing required fields, wrong types, hallucinated extra fields, malformed JSON, mixed-content arguments.
**Mitigation:** Each `ToolDef::parse_args` returns `Result<_, ToolParseError>`. On parse failure the adapter emits a synthetic assistant text message ("I tried to call `<tool>` but the arguments were malformed: …") instead of dropping the turn, so the user sees the model's intent and the model gets a chance to retry on the next turn. Every `parse_args` has fuzz-style tests with gibberish inputs.

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
- Telemetry counter (`local_provider_turn`, `local_provider_test_connection_*`).
- Expanded tool-schema set (MCP, computer-use, web-search, code-review, todos) — each new schema needs round-trip parse tests.
- User-overridable system prompt (advanced setting).
- **Upstream filing for Path 2 (Inference Delegation):** propose adding a `ClientAction::ExecuteLLMInference` variant to `warp-proto-apis/apis/multi_agent/v1/response.proto`. Server emits the fully-formulated OpenAI payload; client forwards to the user's endpoint and streams response back to the server which continues its existing agent loop. This preserves Warp's tuned prompt + tool schemas and is the only path to true parity. **This is a Warp-team decision** — the upstream issue should describe the proto extension and request green-light before any contributor work. It coexists cleanly with Path 1; both can be present (Path 1 for offline / no-warp.dev users; Path 2 for users who want Warp-quality prompts + local inference).
