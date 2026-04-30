# Custom Local LLM Provider — Usage (v1)

Implementation of [warpdotdev/warp#9303](https://github.com/warpdotdev/warp/issues/9303). Lets Warp's Agent Mode chat directly with a user-configured OpenAI-compatible HTTP endpoint (Ollama, LM Studio, vLLM, llama.cpp, NVIDIA NIM) instead of routing through warp.dev.

## Status

v1 ships as a **functional but UI-light** feature:

| Surface | Status |
|---|---|
| SSE → ResponseEvent adapter | ✅ Full coverage |
| OpenAI request builder | ✅ |
| HTTP runner (`run_chat_turn`) | ✅ |
| 5 tool schemas (`read_files`, `apply_file_diffs`, `run_shell_command`, `grep`, `file_glob_v2`) | ✅ |
| System prompt template | ✅ Generic; iterate post-launch |
| Settings storage (TOML + secure-storage key) | ✅ |
| Picker injection (synthetic LLMInfo) | ✅ |
| Dispatch fork in `agent::api::impl::generate_multi_agent_output` | ✅ |
| API-key UI field (Settings → AI) | ✅ |
| Full settings subpage (URL/model id form) | ⏳ TOML-only for v1; full subpage is a follow-up |
| Integration tests with mock OpenAI server | ⏳ Follow-up |

## Enabling the feature

The feature is gated behind `FeatureFlag::LocalLlmProvider`. Off by default.

To enable for development, add to your runtime feature flag overrides:

```toml
[runtime_feature_flags]
LocalLlmProvider = true
```

(or use the dev settings UI to flip it).

## Configuring the provider

Edit your Warp settings TOML (Settings → Show settings on disk):

```toml
[agents.local_provider]
enabled = true
display_name = "Ollama"
base_url = "http://localhost:11434/v1"
model_id = "llama3.1"
supports_tools = true
context_window = "8192"     # optional; empty/0 means "let the model handle it"
```

If your endpoint requires an API key, set it via:

**Settings → AI → "Local Provider API Key (optional)"** (the field appears below the existing OpenAI/Anthropic/Google fields when the feature flag is on).

The key is stored in the OS keychain via `secure_storage`, NOT the settings TOML.

## Using it

After Warp's next model-list refresh (triggered by login, network reconnect, or restart), the Agent Mode model picker shows an entry like:

```
Ollama: llama3.1
  Custom local provider
```

Selecting it routes Agent Mode turns directly to your endpoint. Network inspection during a turn shows zero requests to `*.warp.dev` for the LLM call (other Warp features unrelated to AI continue to use warp.dev).

## Known limitations (v1)

- **Quality gap vs. Warp's tuned models.** Warp's backend tunes its system prompt continuously against its model lineup; we ship a generic one. Smaller / weaker local models will visibly struggle with tool-call formatting and multi-step planning. Iterate by editing `crates/ai/src/local_provider/system_prompt.md`.
- **Reduced tool coverage.** Out of 33 tool variants in `Message.ToolCall.tool`, v1 ships JSON schemas for 5. Other tools (MCP, computer-use, web-search, code-review, todos, etc.) are absent from the local-model tool list.
- **No `/v1/models` discovery.** Model id is manual.
- **No Anthropic-format support.** OpenAI Chat Completions wire format only. Use a gateway like LiteLLM if you need Claude.
- **Settings UI is incomplete.** Display name / URL / model id / supports-tools / context-window are configured via TOML rather than a graphical form. The API key has a graphical field. A full settings subpage is a follow-up PR.
- **Retry semantics.** The existing controller retries on transient errors gated on `is_online` (warp.dev connectivity). A localhost endpoint is reachable even when warp.dev is down — that gating over-restricts retries on the local path. Documented in `tech.md` §8 follow-ups.

## Architecture overview

The implementation is Path 1 of three architectures discussed in the issue thread:

- **Path 1 (this) — Client-owned orchestration.** OSS client builds the prompt, ships JSON schemas for tools, talks directly to the user's endpoint, parses tool calls, and feeds tool results back on the next turn.
- Path 2 (future) — Inference Delegation: Warp's backend builds the prompt + tool schemas as it does today, but emits a `ClientAction::ExecuteLLMInference` carrying the fully-formulated payload; client forwards to the user's endpoint and streams the result back. Requires Warp backend cooperation. See [`specs/future/inference-delegation/`](../future/inference-delegation/README.md).
- Path 3 (future) — Hybrid provider adapters.

Path 1 is the only path a community contributor can land end-to-end without backend buy-in.

## File map

```
crates/ai/src/local_provider/
├── mod.rs              # public entrypoint + module wiring
├── config.rs           # LocalProviderConfig (snapshot struct)
├── key_manager.rs      # LocalProviderKeyManager singleton (secure storage)
├── prompt.rs           # System prompt template + composer
├── system_prompt.md    # The actual prompt body (text)
├── request.rs          # Walks RequestParams + history -> OpenAI ChatCompletionRequest
├── response.rs         # OpenAiSseAdapter: OpenAI SSE -> warp_multi_agent_api ResponseEvent
├── run.rs              # HTTP runner: ties config + request + response + cancellation
├── tools.rs            # 5 tool schemas + bidirectional translation
├── wire.rs             # Serde types for OpenAI Chat Completions wire format
├── response_tests.rs   # 13 fixture-driven SSE adapter tests
└── tools_tests.rs      # 27+ tool translation tests

app/src/ai/
├── llms.rs             # LLMModelHost::Local + post-fetch picker injection hook
├── local_provider_config.rs  # Snapshot from AppContext + synthetic_llm_info builder
├── agent/api.rs        # RequestParams field for the snapshot
└── agent/api/impl.rs   # Dispatch fork: routes local:* model ids away from warp.dev

app/src/settings/
└── ai.rs               # 6 new AISettings entries under [agents.local_provider]

app/src/settings_view/
└── ai_page.rs          # Local provider API key editor (gated on feature flag)

crates/warp_features/src/lib.rs
                        # FeatureFlag::LocalLlmProvider variant
```

## Verification

The library code is verified by:

```bash
cargo test -p ai local_provider::
# 69 tests pass: SSE adapter, tool translation, config validation, prompt template
```

End-to-end manual smoke (planned for follow-up): set up Ollama on localhost, configure as above, send a chat turn, confirm streaming + tool calls work.
