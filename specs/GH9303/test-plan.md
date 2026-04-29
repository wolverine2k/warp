# Test Plan: Custom Local LLM Provider

**Issue:** [warpdotdev/warp#9303](https://github.com/warpdotdev/warp/issues/9303)
**Companion:** [product.md](./product.md), [tech.md](./tech.md), [implementation-plan.md](./implementation-plan.md)

## Pyramid

| Tier | Surface | Where |
|---|---|---|
| Unit | SSE adapter, request translator, config validation, key manager, picker injection | `crates/ai/src/local_provider/*_tests.rs` |
| Integration | One-process end-to-end against an in-test mock OpenAI server | `crates/integration/tests/local_provider_*.rs` |
| Component | Settings page rendering states | `app/src/settings_view/ai_page/custom_providers_tests.rs` |
| Manual | Real Ollama / LM Studio / vLLM / llama.cpp / NIM | Smoke matrix at the bottom |
| Network audit | Confirms no `*.warp.dev` traffic during a local turn | mitmproxy run, tracked in test plan §6 |

Each tier owns a different question:

- **Unit tests** answer "does the parser/translator behave correctly under fixture inputs?"
- **Integration tests** answer "does the full client-to-mock-server round-trip work?"
- **Manual matrix** answers "does it actually work against real local model servers in the wild?"
- **Network audit** answers "is the privacy claim true?"

## 1. Unit tests — SSE → ResponseEvent adapter

**File:** `crates/ai/src/local_provider/response_tests.rs`

Each test is a fixture in (`input_sse_chunks`, `expected_response_events`) form, run through `OpenAiSseAdapter::feed(...)`. Fixtures are checked-in `.txt` files under `crates/ai/src/local_provider/fixtures/sse/`.

| # | Fixture name | Asserts |
|---|---|---|
| 1.1 | `text_only_short` | `Init`, one `AppendToMessageContent(AgentOutput, "hi")`, `Finished{Done}` |
| 1.2 | `text_only_multi_chunk` | one `AppendToMessageContent` per chunk, in order |
| 1.3 | `text_with_done_marker` | `[DONE]` after a chunk with `finish_reason="stop"` collapses to a single `Finished{Done}` |
| 1.4 | `text_done_without_finish_reason` | `[DONE]` alone (no `finish_reason` ever) ⇒ `Finished{Done}` |
| 1.5 | `text_finish_reason_without_done` | `finish_reason` then stream EOF ⇒ `Finished{Done}` |
| 1.6 | `text_finish_length` | `finish_reason="length"` ⇒ `Finished{MaxTokenLimit}` |
| 1.7 | `text_finish_content_filter` | `finish_reason="content_filter"` ⇒ `Finished{Other}` (with diagnostic in `error_message`) |
| 1.8 | `tool_call_single_complete` | one tool call streamed in 4 fragments, complete on `finish_reason="tool_calls"` ⇒ `AddMessagesToTask([ToolCall])` then `Finished{Done}` |
| 1.9 | `tool_call_streamed_args` | `arguments` arrives as 6 string fragments concatenated; assert final ToolCall has full JSON |
| 1.10 | `tool_call_args_whole` | `arguments` arrives as a complete string in one chunk; assert ToolCall is emitted on first sight of complete args |
| 1.11 | `tool_call_two_interleaved` | indices 0 and 1 fragments interleave; both ToolCalls emit in correct order with correct args |
| 1.12 | `text_then_tool` | text deltas precede a tool call; assert event ordering matches input ordering |
| 1.13 | `tool_then_text` | tool fragments then text delta in same response (rare but valid); assert ordering |
| 1.14 | `reasoning_inline_think_tags` | `<think>...</think>` inside `delta.content` ⇒ `AppendToMessageContent(AgentReasoning)` for the inner text and `AppendToMessageContent(AgentOutput)` for surrounding text |
| 1.15 | `reasoning_dedicated_field` | `delta.reasoning_content="..."` ⇒ `AppendToMessageContent(AgentReasoning)` |
| 1.16 | `reasoning_openai_o1_summary` | OpenAI `delta.reasoning` (when/if observed) ⇒ same |
| 1.17 | `reasoning_then_text` | reasoning chunks followed by visible content; assert two distinct event streams in correct order |
| 1.18 | `malformed_json_chunk` | one chunk fails JSON parse; assert `Finished{Other}` is emitted with a stable error message |
| 1.19 | `premature_disconnect` | stream EOF before `finish_reason` or `[DONE]` ⇒ `Finished{InternalError}` |
| 1.20 | `empty_choices` | `choices: []` chunk ⇒ skipped silently; subsequent chunks process normally |
| 1.21 | `server_error_event` | mid-stream chunk has `error: {message: "...", type: "..."}` ⇒ `Finished{Other}` carrying the message |
| 1.22 | `huge_text_burst` | 50KB of text in one chunk ⇒ single `AppendToMessageContent` (no truncation) |
| 1.23 | `unicode_and_emoji` | UTF-8 multi-byte content streams correctly across chunk boundaries (split a codepoint across chunks intentionally) |
| 1.24 | `cancellation_mid_text` | adapter is dropped after 3 chunks; assert no panic, no further events emitted |
| 1.25 | `init_event_uniqueness` | running the adapter twice produces two distinct `Init.request_id`s |

**Coverage rationale:** the adapter is pure & deterministic, so high coverage is cheap and the fixtures double as living documentation of the supported wire dialect.

## 2. Unit tests — request translator

**File:** `crates/ai/src/local_provider/request_tests.rs`

| # | Test | Asserts |
|---|---|---|
| 2.1 | `model_id_uses_user_string_not_synthetic` | `messages` body's `"model"` is `cfg.model_id` (e.g. `"llama3.1"`), not the LLMId (`"local:llama3.1"`) |
| 2.2 | `system_prompt_present` | first message has `role="system"` and content matches the existing prompt builder's output |
| 2.3 | `history_preserved_order` | given a 4-turn fixture history, `messages` array has `system, user, assistant, user, assistant, user` in that order |
| 2.4 | `tool_results_use_role_tool` | a prior tool result becomes a `{role:"tool", tool_call_id:..., content:...}` entry |
| 2.5 | `tools_present_when_supported` | `cfg.supports_tools=true` ⇒ `tools` array present, mapped from Warp's SupportedTools |
| 2.6 | `tools_absent_when_unsupported` | `cfg.supports_tools=false` ⇒ no `tools` field |
| 2.7 | `auth_header_when_key_set` | `Authorization: Bearer xxx` present iff key set |
| 2.8 | `auth_header_absent_when_no_key` | header absent for empty/None key |
| 2.9 | `stream_true` | body contains `"stream": true` |
| 2.10 | `temperature_omitted` | body has no `temperature` field |
| 2.11 | `image_attachment_stripped` | a fixture with image attachments produces messages without image content (since `vision_supported=false`) |
| 2.12 | `tool_choice_auto` | `"tool_choice": "auto"` present when tools present |

## 3. Unit tests — config & key manager

**File:** `crates/ai/src/local_provider/config_tests.rs` and `crates/ai/src/local_provider_key_tests.rs`

| # | Test | Asserts |
|---|---|---|
| 3.1 | `from_app_disabled` | `local_provider_enabled=false` ⇒ `from_app` returns `None` |
| 3.2 | `from_app_empty_url` | empty base URL ⇒ `None` |
| 3.3 | `from_app_invalid_url` | non-URL string ⇒ `None`, with a `log::warn!` |
| 3.4 | `from_app_empty_model_id` | empty model id ⇒ `None` |
| 3.5 | `from_app_happy` | populated settings + key in storage ⇒ fully-populated `Some(LocalProviderConfig)` |
| 3.6 | `from_app_no_key` | populated settings, no key in storage ⇒ `Some(...)` with `api_key=None` |
| 3.7 | `key_manager_set_get_round_trip` | `set_key(Some(s))` then `key()` returns `Some(s)` |
| 3.8 | `key_manager_set_none_clears` | `set_key(None)` followed by reload reads back `None`; secure_storage mock recorded a delete |
| 3.9 | `key_manager_persists_across_reload` | write key, drop manager, recreate, still readable |
| 3.10 | `synthetic_llm_id_format` | `synthetic_llm_id` is exactly `"local:<model_id>"` for given model id |
| 3.11 | `synthetic_llm_info_minimal` | produced `LLMInfo` has `provider=Unknown`, `host_configs[Local].enabled=true`, `request_multiplier=0` |

## 3.5. Unit tests — system prompt

**File:** `crates/ai/src/local_provider/prompt_tests.rs`

| # | Test | Asserts |
|---|---|---|
| 3.5.1 | `prompt_includes_role_framing` | rendered prompt mentions "Warp", "terminal", "coding" or equivalent role-framing keywords |
| 3.5.2 | `prompt_lists_only_supported_tools` | given `supported=[ReadFiles, Grep]`, the rendered prompt mentions those two tools and does NOT mention `apply_file_diffs` etc. |
| 3.5.3 | `prompt_includes_v4a_diff_instructions_iff_apply_diffs` | diff format guidance present iff `ApplyFileDiffs` is in supported set |
| 3.5.4 | `prompt_includes_context_window_when_set` | `context_window=Some(8192)` ⇒ "approximately 8192 tokens" appears |
| 3.5.5 | `prompt_omits_context_window_when_none` | `context_window=None` ⇒ no "approximately N tokens" line |
| 3.5.6 | `prompt_is_stable_across_runs` | same inputs ⇒ identical output (deterministic; no `format!()` of timestamps etc.) |
| 3.5.7 | `prompt_passes_through_jinja_substitution_safely` | tool names containing `{` and `}` (defensive) don't corrupt the template |

## 3.6. Unit tests — tool definitions & translation

**File:** `crates/ai/src/local_provider/tools_tests.rs`

The hardest-to-get-right area; deserves heavy coverage. Each of the 5 v1 tools (`read_files`, `apply_file_diffs`, `run_shell_command`, `grep`, `file_glob`) gets the same 5-test pattern:

| Pattern test | Asserts |
|---|---|
| `<tool>_schema_is_valid_json_schema` | `serde_json::from_str::<Value>(json_schema)` succeeds and root has `type:"object"` and `properties` |
| `<tool>_schema_describes_required_fields` | each required field listed in `required` array appears in `properties` |
| `<tool>_parse_args_minimal_valid` | minimum-required JSON parses to the right `Message::ToolCall.tool::*` variant |
| `<tool>_parse_args_full_valid` | all-fields JSON populates every typed field on the proto variant |
| `<tool>_parse_args_missing_required` | required field absent ⇒ `ToolParseError::MissingField(<name>)` |
| `<tool>_parse_args_wrong_type` | string where number expected ⇒ `ToolParseError::TypeMismatch` |
| `<tool>_parse_args_extra_fields_ignored` | hallucinated extra field ⇒ accepted, ignored, no error (LLMs hallucinate, we tolerate) |
| `<tool>_parse_args_gibberish_json` | non-JSON string ⇒ `ToolParseError::InvalidJson` |

5 tools × 8 patterns = **40 tests minimum** in this file. Each individual test is small (5–15 lines) so the volume is manageable.

Plus per-tool semantic tests:

| # | Test | Asserts |
|---|---|---|
| 3.6.A | `apply_file_diffs_v4a_diff_parses` | a real V4A-format diff string from the prompt gets translated into the typed `ApplyFileDiffsArgs` with each hunk's `search`/`replace` populated |
| 3.6.B | `run_shell_command_purpose_required` | the user-friendly `purpose` field maps to whichever proto field carries the model's rationale; missing ⇒ defaults to empty string, not error |
| 3.6.C | `read_files_paths_array` | `paths` JSON array maps to the proto's `repeated string paths` |
| 3.6.D | `grep_pattern_passes_through_unchanged` | regex-special characters in `pattern` are not escaped/altered |
| 3.6.E | `file_glob_pattern_relative_path_ok` | both relative (`*.rs`) and absolute (`/abs/**`) globs accepted |

And the round-trip / outbound:

| # | Test | Asserts |
|---|---|---|
| 3.6.F | `tool_definitions_filters_by_supported_tools` | `tool_definitions(&[ReadFiles])` returns exactly 1 OpenAI tool def, with `function.name=="read_files"` |
| 3.6.G | `tool_definitions_empty_when_no_supported` | `tool_definitions(&[])` returns empty Vec |
| 3.6.H | `tool_definitions_unknown_tool_skipped` | unsupported `ToolType::ComputerUse` is silently omitted (not error) |

## 4. Unit tests — picker injection

**File:** `app/src/ai/llms_local_injection_tests.rs`

| # | Test | Asserts |
|---|---|---|
| 4.1 | `inject_when_config_some` | `AvailableLLMs::with_local_injected(...)` adds the synthetic entry at the end |
| 4.2 | `no_inject_when_none` | None config ⇒ list unchanged |
| 4.3 | `cleanup_does_not_strip_local_id` | the existing prefs-cleanup pass at `app/src/ai/llms.rs:927-972` keeps the injected entry |
| 4.4 | `picker_subtext_shows_endpoint_host` | `model_menu_items` rendering snapshot for a `local:*` entry includes the URL host as subtext |
| 4.5 | `picker_no_credit_label_for_local` | snapshot has no `Credits:` annotation |

## 5. Integration tests

**Files:** `crates/integration/tests/local_provider_*.rs`

Each test boots a one-process mock OpenAI server using `tokio::net::TcpListener` + a hand-rolled chunked-encoding handler (kept under ~150 lines, deliberately *not* using `axum`/`hyper` for test isolation).

### 5.1 `local_provider_basic.rs`

Configure a local provider, send a single text turn, assert:
- mock server received exactly one POST to `/v1/chat/completions`
- request body has `stream:true`, `model:"test-model"`, `messages` non-empty
- response stream produced visible content matching the mock's emitted chunks
- conversation persists with the assistant turn recorded

### 5.2 `local_provider_dispatch.rs`

Configure a local provider AND have a regular Warp model also available. With the local model selected, send a turn. Assert:
- mock local server received the request
- mock Warp server (the existing test fixture) received zero AI requests
- swap selection back to a Warp model and resend; now the Warp mock receives traffic and the local mock does not

### 5.3 `local_provider_tool_call.rs`

Mock server emits a `read_file` tool call in turn 1. Assert:
- Warp's existing tool runner is invoked (intercept via the existing test infra)
- after the tool runs, turn 2 is initiated automatically and the mock receives a follow-up POST whose `messages` array ends with `{role:"tool", tool_call_id, content: <file bytes>}`
- turn 2's text response renders into the conversation

### 5.4 `local_provider_cancellation.rs`

Send a turn whose mock response is paced (1 chunk per 100ms over 5s). After 300ms, simulate the user pressing stop. Assert:
- HTTP connection closes
- mock server's connection-close handler fires
- conversation has a partial assistant turn (whatever streamed before cancel)
- no `Finished{InternalError}` toast appears (clean cancel, not error)

### 5.5 `local_provider_unreachable.rs`

Configure a base URL pointing at a closed port. Send a turn. Assert:
- `Finished{Other}` arrives within the connect-timeout
- error toast text contains the configured display name
- conversation can be retried (the input bar is not stuck)

### 5.6 `local_provider_4xx_5xx.rs`

For each status in `[401, 403, 404, 422, 500, 503]`:
- mock server returns that status with a JSON error body
- assert the error toast contains the status code
- 401 specifically should also include the "check your API key" hint

### 5.7 `local_provider_settings_round_trip.rs`

Open settings, fill the form via the UI test harness, save, close, reopen. Assert:
- non-secret fields read back from the TOML file
- API key reads back from the secure-storage mock
- Test connection button calls the mock and renders the model's reply

### 5.8 `local_provider_disabled_with_stale_id.rs`

Configure a local provider, select its model, then disable `local_provider_enabled`. Send a new turn. Assert:
- the local mock receives nothing
- the next turn is sent to the Warp default model
- a one-time toast appears saying "Local provider is no longer configured; falling back to <default>"

### 5.9 `local_provider_transaction_lifecycle.rs`

Mock server emits a normal text-only response. Assert the synthesized event stream contains, in order: `Init`, `BeginTransaction`, ≥1 `AppendToMessageContent`, `CommitTransaction`, `Finished{Done}`. Then run a second test where the mock simulates a mid-stream HTTP failure; assert the stream contains `Init`, `BeginTransaction`, ≥0 `AppendToMessageContent`, `RollbackTransaction`, `Finished{Other}`. Both cases assert the conversation state on the controller side after the stream completes — clean turn for commit, no orphaned half-message for rollback.

### 5.10 `local_provider_tool_arg_round_trip.rs`

For each of the 5 v1 tools, drive a full integration scenario: mock server emits a tool call with realistic arguments → tools.rs parses → controller invokes the existing tool runner → tool result feeds back into the next turn → assert the next turn's request body contains a `{role:"tool", tool_call_id, content}` entry with the expected payload. This is the end-to-end proof that the strongly-typed proto translation closes the loop.

### 5.11 `local_provider_malformed_tool_args.rs`

Mock server emits a tool call with malformed JSON arguments (e.g. `{"paths": [`). Assert:
- the adapter does NOT crash or drop the turn
- the conversation contains a synthetic assistant text message describing the parse failure (per tech.md §Risks "ToolParseError" mitigation)
- the user can ask the model to retry without a state reset

## 6. Network audit (manual but reproducible)

Goal: prove the privacy claim from product.md §Goals.

Procedure:
1. Configure local provider (Ollama on `127.0.0.1:11434`).
2. Run `mitmproxy --mode regular -p 8888` and configure Warp's HTTPS proxy to `localhost:8888` via `HTTPS_PROXY` env var.
3. Open Agent Mode, select the `local:*` model.
4. Send a chat turn. Wait for completion.
5. Inspect the mitmproxy log:
   - Expected: zero requests to any `*.warp.dev` host **for the LLM call itself**.
   - Allowed (with caveat): non-AI traffic (telemetry pings, version-checks). Document each one in the audit report.
6. Repeat with the local provider disabled to baseline what "normal" Warp traffic looks like.

Reproducibility: the `mitmproxy --save-stream-file` flag captures all flows; the test plan ships a `script/audit_local_provider.sh` (post-phase-8) that runs this and diffs against an expected allow-list.

## 7. Manual smoke matrix

Run before promoting flag from `DOGFOOD_FLAGS` to `PREVIEW_FLAGS`. One row per environment.

| # | Server | OS | Model | Tool calls? | API key? | Pass criteria |
|---|---|---|---|---|---|---|
| M1 | Ollama 0.4 | macOS | `llama3.1:8b-instruct` | ✓ | — | text + tool call cycle work; cancel works |
| M2 | Ollama 0.4 | Linux | `qwen2.5-coder:7b` | ✓ | — | reasoning content from `<think>` tags renders separately |
| M3 | LM Studio 0.3 | macOS | `bartowski/Llama-3.1-8B-Instruct-GGUF` | ✓ | — | streamed tool-call args concatenate correctly |
| M4 | vLLM 0.6 | Linux GPU box | `meta-llama/Llama-3.1-70B-Instruct` | ✓ | — | high-throughput stream renders without UI lag |
| M5 | llama.cpp `server` | macOS | any GGUF | ✗ (text only) | — | tools-disabled path produces text-only assistant turn |
| M6 | NVIDIA NIM | macOS | `meta/llama-3.1-70b-instruct` | ✓ | ✓ (NIM key) | HTTPS + bearer auth work; conversation persists |
| M7 | OpenRouter direct | macOS | `anthropic/claude-3.5-sonnet` | ✓ | ✓ | sanity check that an OpenAI-compatible gateway in front of Claude works |
| M8 | Closed port | macOS | — | — | — | error toast surfaces, no crash |

Each row's evidence: a screenshot of the conversation + the network audit log + a 30-second screen recording of cancellation.

## 8. Performance & resource

Out-of-scope for v1, but smoke-check:

- Send 50 consecutive turns to a local model; confirm no memory growth in the client process beyond conversation-size baseline (use `Activity Monitor` / `ps aux`).
- Confirm no thread leak after cancellation (compare thread count before and after 20 cancel cycles).
- Confirm long contexts (~30K tokens of history) don't crash the UI when the model rejects them — error path renders.

## 9. Regression coverage

Add to the existing presubmit-run nextest list:

- All unit tests in `crates/ai/src/local_provider/*`
- Integration tests `local_provider_basic`, `local_provider_dispatch`, `local_provider_tool_call`, `local_provider_unreachable`, `local_provider_settings_round_trip`

Skip from default test run (too slow / require external resources):

- Manual smoke matrix (run pre-promotion)
- Network audit (run pre-promotion)

## 10. Test data hygiene

- All fixtures under `crates/ai/src/local_provider/fixtures/sse/` are committed real SSE captures (not handwritten — captured from real Ollama/LM Studio/vLLM and trimmed). The capture script `script/capture_sse_fixture.sh` is committed for reproducibility.
- No real API keys in fixtures or test code; the secure_storage mock returns synthetic keys.
- Mock server responses live as `.json` and `.sse` files next to their tests, not inline string literals — keeps tests readable and lets non-Rust contributors edit fixtures.

## Open testing questions

1. **i18n for error toasts.** If this repo uses fluent/gettext, the error strings need translation entries; if not, inline strings suffice. Confirm in phase 6.
2. **Snapshot test framework.** What's the existing UI snapshot tool here (insta? a custom one)? Phase 4/6 tests will use the same.
3. **Real-network manual tests in CI.** Should phase 8's manual matrix be partially automated (e.g., a self-hosted runner with Ollama pre-installed)? Out of scope for v1 but worth a follow-up.
