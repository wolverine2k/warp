# Implementation Plan: Custom Local LLM Provider

**Issue:** [warpdotdev/warp#9303](https://github.com/warpdotdev/warp/issues/9303)
**Companion:** [product.md](./product.md), [tech.md](./tech.md), [test-plan.md](./test-plan.md)
**Branch:** `nmehta/local-llm-provider`

## Reality check from issue #9303

Before sequencing work: the comment thread on issue #9303 (16 comments) confirms two things that materially shape this plan:

1. **The system prompt and tool JSON schemas are server-only** in Warp's stack and must be authored fresh in the OSS client. There is no `compose_system_prompt` to extract — `Aeromix`'s LM-Studio attempt and the Opus analysis both confirm this. Adjustments below absorb that reality (new Phase 2.5).
2. **`prost_reflect_build` produces fully-public Rust types**, verified directly against `apis/multi_agent/v1/gen/rust/build.rs` in the proto repo. The "constructibility probe" called out as the risk gate in earlier drafts is no longer needed; it is replaced by a much smaller "verify Rust struct shapes match proto" check inside Phase 1.

The issue is `triaged` / `enhancement` / `duplicate` but **not yet `ready-to-spec`**. Phase 0 below covers getting the readiness label before any code lands.

## Sequencing principle

Each phase produces a **landable, reverted-if-needed** commit, gated by `FeatureFlag::LocalLlmProvider`. The flag stays off in `RELEASE_FLAGS`/`PREVIEW_FLAGS` until phase 8. Reviewers see one phase per PR, not one giant 30-file change.

Phases 1–3 are **library-only**; they don't touch any UI or dispatch path. If the proto-constructibility risk (tech.md §Risks) bites, only phases 1–3 need rework — phases 4+ are unblocked once the adapter compiles.

## Phase 0 — Prep (no code)

- [ ] Comment on issue [#9303](https://github.com/warpdotdev/warp/issues/9303) summarizing the Path 1 spec at this branch, requesting the `ready-to-spec` label, and explicitly acknowledging that this is one of the three paths the thread already discussed (so reviewers see we engaged with the architecture conversation).
- [ ] Wait for `@oss-maintainers` to apply `ready-to-spec`. **Do not start phases 1+ before the label.** Per CONTRIBUTING.md, feature work requires the label.
- [ ] On labeling: rename `specs/local-llm-provider/` to `specs/GH9303/` to match repo convention; open the spec PR per CONTRIBUTING.md §"Opening a Spec PR".
- [ ] Add the feature flag entry stub: `FeatureFlag::LocalLlmProvider` in `crates/warp_features/src/lib.rs`. Land this on its own as a **typo-trivial PR** so subsequent phases compile against a real flag.

**Exit criteria:** issue is `ready-to-spec`, spec PR open, flag exists, branch tracks `master` cleanly.

## Phase 1 — SSE → ResponseEvent adapter (pure library)

**Goal:** convert OpenAI Chat Completions SSE chunks into a `Stream<Result<ResponseEvent, AdapterError>>`. No HTTP, no settings, no UI. Maximum testability.

**Deliverables:**
- New crate module `crates/ai/src/local_provider/` with:
  - `mod.rs` — re-exports.
  - `wire.rs` — `serde` types for the OpenAI Chat Completion streaming envelope (`ChatCompletionChunk`, `ChoiceDelta`, `ToolCallDelta`, `ToolCallFragment`).
  - `response.rs` — the adapter state machine (`OpenAiSseAdapter`), `ToolCallBuffer` for fragment accumulation, and `BeginTransaction`/`CommitTransaction`/`RollbackTransaction` framing per tech.md §6.6.
  - `response_tests.rs` — fixture-driven tests (see test-plan.md §1).
- Shape check (replaces the obsolete constructibility probe): a 20-line smoke test that constructs `ResponseEvent::Type::Init(StreamInit{...})`, `ClientAction::Action::AppendToMessageContent(...)`, and `ResponseEvent::Type::Finished(StreamFinished{...})` from the local crate and asserts they encode/decode via `prost::Message::encode` round-trip. Cheap; protects against future proto field renames.

**Files touched:**
- `crates/ai/src/local_provider/mod.rs` (new)
- `crates/ai/src/local_provider/wire.rs` (new)
- `crates/ai/src/local_provider/response.rs` (new)
- `crates/ai/src/local_provider/response_tests.rs` (new)
- `crates/ai/src/lib.rs` (one line: `pub mod local_provider;`)
- `crates/ai/Cargo.toml` (add `serde_json` if not already a direct dep — it likely is)

**Tests:** ~25 unit tests covering the matrix in tech.md §6 + test-plan.md §1.

**No flag wiring yet.** This module is reachable only from tests.

**Out of scope this phase:** any dispatch, any HTTP, any settings.

## Phase 2 — Request translator + HTTP runner (still no UI)

**Goal:** given an in-memory `RequestParams` and a `LocalProviderConfig`, produce a stream of `ResponseEvent`s by talking to a real (or test-fixture) HTTP endpoint.

**Deliverables:**
- `crates/ai/src/local_provider/config.rs` — `LocalProviderConfig` struct + `validate()`. Stand-alone; not yet wired to settings.
- `crates/ai/src/local_provider/request.rs` — `compose_chat_completion_request(params, cfg) -> ChatCompletionRequest` + `tool_definitions()` translation table.
- `crates/ai/src/local_provider/mod.rs` — `pub async fn run_chat_turn(http, params, cfg, cancel) -> AIOutputStream<ResponseEvent>`.
- `crates/ai/src/local_provider/run_test.rs` — non-streaming `run_test_completion(http, cfg)` for the Settings → Test connection button.
- New integration test under `crates/integration/tests/local_provider_basic.rs` that boots a `tokio::net::TcpListener`-backed mock server in-process and exercises one streaming turn.

**Files touched:**
- 4 new files under `crates/ai/src/local_provider/`
- `crates/integration/tests/local_provider_basic.rs` (new)
- `crates/integration/Cargo.toml` (add `tokio-tungstenite`/`hyper` test-only dep if needed; reuse what's already available)

**Tests:** request-translator unit tests (assert system prompt, message order, tools omitted/included, model id is the user's not the synthetic one, Authorization header presence). One integration smoke for the runner.

**Critical correction from earlier draft:** the assumption that `compose_system_prompt` could be extracted from existing code is **wrong** — issue #9303 comments and inspection of the OSS source confirm Warp's system prompt is server-only and never reaches the OSS client. Accordingly, request translation in Phase 2 imports `compose_system_prompt` from the new module produced in Phase 2.5 below. Phase 2 stubs it as `compose_system_prompt(_params) -> String { "TODO: filled in Phase 2.5".to_string() }` so request-shape tests can land independently of prompt content.

**Out of scope this phase:** system-prompt content (Phase 2.5), tool schemas (Phase 2.5), dispatch routing (Phase 5), picker injection (Phase 4), settings UI (Phase 6), secure storage (Phase 3).

## Phase 2.5 — System prompt + tool schemas (content authoring)

**Goal:** ship the hand-authored content described in tech.md §6.4 and §6.5. This is the single biggest determinant of user-perceived quality and dwarfs the SSE adapter in calendar-time, even if it's smaller in line count.

**Deliverables:**
- `crates/ai/src/local_provider/prompt.rs`:
  - `const TEMPLATE: &str = include_str!("system_prompt.md");` (checked-in plain text, code-reviewed like any source file).
  - `pub fn compose_system_prompt(supported: &[ToolType], context_window: Option<u32>) -> String` with `{tools}` and `{context_window}` substitution.
- `crates/ai/src/local_provider/system_prompt.md` — the actual prompt body, sectioned per tech.md §6.4 (role framing, tools, output format, diff format, safety guardrails, context-window hint).
- `crates/ai/src/local_provider/tools.rs`:
  - `ToolDef` struct + a static registry of the 5 v1 tools (`read_files`, `apply_file_diffs`, `run_shell_command`, `grep`, `file_glob_v2`).
  - For each tool: literal JSON-schema string + `parse_args` returning the typed `Message::ToolCall.tool::*` variant.
  - `pub fn tool_definitions(supported: &[ToolType]) -> Vec<OpenAiToolDefinition>`.
  - `pub fn translate_openai_tool_call(call: &OpenAiToolCall) -> Result<Message::ToolCall, ToolParseError>`.
- 5 sets of `parse_args` tests per tool: minimal-valid, all-fields, missing-required, wrong-type, gibberish. ~25 unit tests minimum.
- Integration test in `crates/integration/tests/local_provider_tools.rs`: feed a fixture of model-emitted JSON for each of the 5 tools and assert the resulting proto variant has the expected fields.

**Files touched:**
- 4 new files under `crates/ai/src/local_provider/`
- 1 new integration test file
- No production wiring yet — Phase 2's `run_chat_turn` upgrades to call into this module

**This phase has the highest content-iteration risk.** Budget time for at least one round of "model emits weird JSON; refine schema and prompt; re-test" per tool. The 5-tool scope is intentionally tight to make this tractable. Add follow-up tools in post-launch PRs.

**Out of scope:** any tool not in the v1 list. Adding MCP, computer-use, web-search, etc. each become separate post-launch PRs once the framework is shaken out.

## Phase 3 — Settings model (storage only, no UI)

**Goal:** wire the new settings keys and `LocalProviderKeyManager` so the runtime can read a `LocalProviderConfig` from `AppContext`.

**Deliverables:**
- New non-secret settings in `app/src/settings/ai.rs` per tech.md §2.
- `crates/ai/src/local_provider_key.rs` — singleton modeled on `crates/ai/src/api_keys.rs`.
- App-level wiring: register `LocalProviderKeyManager` as a singleton in the same place `ApiKeyManager` is registered (`app/src/main.rs` or wherever the AI singletons live — TBD on first read).
- `LocalProviderConfig::from_app(ctx) -> Option<Self>` reads both surfaces.

**Tests:**
- Unit test on `LocalProviderKeyManager::set_key`/`set_key(None)` round-trip via a `secure_storage` mock.
- Unit test on `LocalProviderConfig::from_app` returning `None` when disabled, malformed URL, empty model id, or empty base URL.

**Files touched:**
- `app/src/settings/ai.rs` (extend)
- `crates/ai/src/local_provider_key.rs` (new)
- `crates/ai/src/lib.rs` (one line)
- `app/src/main.rs` or singleton registration site (one line)

**No UI yet** — settings are only writable from code/tests at this point.

## Phase 4 — Picker injection

**Goal:** make a `local:*` `LLMInfo` show up in the Agent Mode picker when settings are populated.

**Deliverables:**
- Extend `LLMModelHost` with `Local` (append before `#[serde(other)] Unknown`).
- Add the post-fetch hook in `app/src/server/server_api/ai.rs` near the `TryFrom<ModelsByFeature>` conversion.
- Tweak `app/src/ai/execution_profiles/model_menu_items.rs:147` to suppress credit/cost label and show `<endpoint host>` subtext when `provider==Unknown && host_configs.contains(Local)`.
- Verify the cleanup pass at `app/src/ai/llms.rs:927-972` does not strip the local entry.

**Tests:**
- Unit test on the `synthetic_llm_info()` shape.
- Unit test on the post-fetch injection: feed an `AvailableLLMs` from a fixture + a populated `LocalProviderConfig`, assert the local entry is present.
- Unit test that with `LocalProviderConfig::from_app` returning `None` the entry is absent.
- Snapshot test on `available_model_menu_items` rendering the local entry without a credit label.

**Files touched:**
- `app/src/ai/llms.rs` (enum extension)
- `app/src/server/server_api/ai.rs` (injection)
- `app/src/ai/execution_profiles/model_menu_items.rs` (label tweak)

**Risk gate:** if the `LLMModelHost` enum is consumed by code we don't own (server-side wire types), confirm the new variant doesn't break upstream. The `#[serde(other)] Unknown` arm shields read paths; for write paths the only emitter is our new injection site, which is internal.

## Phase 5 — Dispatch fork

**Goal:** when a `local:*` model is selected and the flag is on, traffic goes to `local_provider::run_chat_turn` instead of `ServerApi::generate_multi_agent_output`.

**Deliverables:**
- Refactor `app/src/ai/agent/api/impl.rs::generate_multi_agent_output` (the wrapper) into a router per tech.md §5.
- Add the `is_local_model_id` helper.
- Wire `AppContext` access through the wrapper if it isn't already (the wrapper signature may need to take `&AppContext` to read `LocalProviderConfig::from_app`; if a thread-safety constraint blocks that, snapshot the config into `RequestParams` at the call site).

**Tests:**
- Integration test `tests/local_provider_dispatch.rs`: configure a local provider, send a chat turn, assert the mock local server received the request and the mock Warp server got zero traffic.
- Unit test on the router branch logic: with `local_provider_enabled=false` but a `local:*` LLMId still selected (stale state), assert we route to the server path with a fallback default model. (This safety net handles the case where the user disabled the provider but their saved profile still references the local id.)

**Files touched:**
- `app/src/ai/agent/api/impl.rs`
- `crates/integration/tests/local_provider_dispatch.rs` (new)

**This is the highest-blast-radius phase.** Pair-review with the AI team SME (Oz will route).

## Phase 6 — Settings UI

**Goal:** the user can configure the provider through the GUI.

**Deliverables:**
- New file `app/src/settings_view/ai_page/custom_providers.rs` with `CustomProvidersPage`.
- New `AISubpage::CustomProviders` variant, hidden behind the feature flag.
- The **Test connection** button calling `local_provider::run_test_completion`.
- Inline validation: invalid URL, empty model id, etc.

**Tests:**
- Component-level test rendering the page in three states: empty, populated, disabled. Snapshot the layout.
- Integration test: open settings, fill the form, save, reopen, assert values round-trip.
- Manual smoke: click Test connection against a real Ollama, against an invalid URL, against an unreachable port — confirm the three error paths render as designed.

**Files touched:**
- `app/src/settings_view/ai_page/custom_providers.rs` (new)
- `app/src/settings_view/ai_page.rs` (add subpage)
- `app/src/settings_view/mod.rs` (subpage routing)
- (UI strings) `resources/translations/*` if applicable — need to confirm this repo's i18n story; otherwise inline.

## Phase 7 — Tool-call round trip

**Goal:** an end-to-end tool-call cycle works: model emits tool call → Warp runs the tool → result is sent back → model continues.

**Deliverables:**
- Verify the existing tool runner (`agent::controller`) accepts `ClientAction::AddMessagesToTask` events from any source. The investigation done for the spec suggests it does (it's source-agnostic), but this phase is the proof.
- If a follow-up turn synthesis path is server-only (e.g., the controller currently calls `ServerApi` directly to send a tool result), refactor that call site to route through `agent::api::impl` so the dispatch fork applies.
- Integration test that exercises a two-turn cycle: turn 1 emits a `read_file` tool call; controller runs it; turn 2 begins automatically with the file contents in `messages` of role `tool`; mock server returns final answer.

**Files touched:**
- Likely `app/src/ai/blocklist/controller.rs` (if a tool-result send-back path is found that bypasses the wrapper) — TBD pending phase 5 review feedback.
- `crates/integration/tests/local_provider_tool_call.rs` (new)

**This is where the design proves out.** If the proto's `Message::ToolCall` constructor is unworkable from outside the proto crate, this phase exposes it; we then either (a) push a constructor into `warp-proto-apis` upstream or (b) build a thin builder shim in `crates/ai/src/local_provider/proto_builder.rs` that uses whatever public API is available.

## Phase 8 — Stabilization & flag promotion

**Goal:** sanding, documentation, dogfood rollout.

**Deliverables:**
- Update `WARP.md`'s feature-flags section if there's a feature-flag inventory.
- Document the feature in `docs.warp.dev` (out-of-tree; coordinate with docs team).
- Add `FeatureFlag::LocalLlmProvider` to `DOGFOOD_FLAGS` once internal smoke is clean.
- Run the manual smoke matrix from product.md §Validation against Ollama, LM Studio, vLLM, llama.cpp, NIM.
- Run mitmproxy network audit; document any non-AI Warp traffic that still leaves the box during a local-provider turn (telemetry, version-check, etc.) so the privacy story is honest.
- File the follow-up tickets enumerated in tech.md §Follow-ups.

**Promotion gate:** at least 2 weeks of dogfood-only usage from the AI team plus the maintainer of the spec, no regressions filed against `Local` requests, before promotion to `PREVIEW_FLAGS`.

## Open coordination items

1. **Issue filing** — this spec presumes an issue will be created. The issue body should link this PR's spec folder and ask for `ready-to-spec` from `@oss-maintainers`.
2. **Proto exposure (`warp-proto-apis`)** — phase 1's probe test gates everything. If it fails, file an upstream PR to expose the necessary builders **before** continuing.
3. **AI team review** — phases 5 and 7 cross AI-runtime ownership; loop in the AI SME early via Oz auto-routing.
4. **Settings UI review** — phase 6 deserves a UX pass; the Settings → AI tree is sensitive territory.
5. **Privacy copy** — the settings page needs final copy approval from anyone owning the user-facing trust story (the "this endpoint will receive your full conversation directly" line).

## Estimated effort

| Phase | Engineer-days (point estimate) | Risk |
|---|---|---|
| 0 — Prep | 0.25 | low (calendar-bound on label) |
| 1 — Adapter | 2 | low (proto constructibility resolved) |
| 2 — Request + HTTP (with stub prompt) | 1.5 | low |
| 2.5 — System prompt + tool schemas | 5–6 | **high** (content iteration; biggest quality lever; ApplyFileDiffs alone is non-trivial) |
| 3 — Settings storage | 1 | low |
| 4 — Picker injection | 1 | low |
| 5 — Dispatch fork | 1.5 | medium (cross-cutting + reviewer load) |
| 6 — Settings UI | 2 | medium (UX iteration) |
| 7 — Tool-call cycle | 2 | medium (now that tools.rs ships parsers, this is mostly wiring) |
| 8 — Stabilization | 2+ | low (calendar-bound) |
| **Total** | **~18–19 days** of focused work plus a multi-week dogfood window | |

The `~18–19 days` is a 95th-percentile estimate for a single engineer working with reviewer round-trips. Realistic calendar time, accounting for label-wait, Oz/SME review cycles, and dogfood feedback loops, is closer to 6–10 weeks end-to-end.
