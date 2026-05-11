# Multi-Local-LLM (BYOP) — Specs Index

Bring Your Own Provider for the Agent Mode dispatcher. This directory holds the design, phased implementation plans, and operational gating notes for the **multi-local-LLM** initiative on the `multi-local-llm` branch.

The work extends the single-local-provider scaffolding from `nmehta/local-llm-provider` (the `local:`-prefixed picker entry, OpenAI-compatible wire code, and compaction pipeline) into a `Vec<AgentProvider>` that lets a user configure **multiple endpoints simultaneously** (Ollama + LM Studio + a remote OpenAI-compatible host + …) and pick a specific *provider × model* per conversation. Cloud-Warp models continue to dispatch through the warp.dev path; the override is scoped to BYOP-flagged LLMIds (`byop:<provider_id>:<model_id>`) so picking a cloud model is unaffected.

## Status

# ✅ Phase 1 — COMPLETE

**Tagged `v0.1.0` on 2026-05-08.** All four sub-phases (1a / 1b-1 / 1b-2 / 1b-3) shipped end-to-end on `multi-local-llm` and are on `origin`. The branch is 31 commits ahead of `nmehta/local-llm-provider` (its forking point); end-to-end manual smoke testing on macOS confirmed BYOP picker entries route to local endpoints and cloud-Warp picks route to warp.dev.

**Phase 1b-4 (legacy cleanup)** is drafted and queued in [`plan-phase-1b-4-cleanup.md`](plan-phase-1b-4-cleanup.md). Execution is gated on telemetry confirming migration adoption ≥ 99% — see the plan's preamble for the gating checklist.

**Phase 2 (ProviderAdapter trait + Test connection button)** shipped on `multi-local-llm` (final commit `df0ec591`). Hoisted the OpenAI-compatible wire code behind a `ProviderAdapter` / `StreamDecoder` trait pair so Phase 3 can plug in native non-OpenAI adapters without touching `run.rs`. **No user-visible behavior change** for OpenAI; the existing test suite stays green and exercises the same paths through the trait. The per-card "Test connection" probe button is wired and visualizes Idle / Probing / Ok / Failed state.

**Phase 3a (Anthropic native adapter)** code is complete on `multi-local-llm` (final commit `061456b3`). First native non-OpenAI adapter and validates the Phase 2 trait shape. Hand-rolled against the Messages API (`/v1/messages`, `x-api-key` + `anthropic-version: 2023-06-01` auth, alternating user/assistant roles with content blocks, named SSE events `message_start` / `content_block_*` / `message_stop`). Includes a small `StreamDecoder` trait extension (`feed_event(event_name, data)`) so Anthropic's named events can be dispatched on without affecting OpenAi's anonymous-chunk path. **65 new unit tests** across wire types, request translator, URL helpers, SSE decoder, and adapter — plus the existing 341 tests stay green (`cargo nextest run -p ai` reports 426/426).

> **Verification gate:** live-test smoke against `api.anthropic.com` with a real `sk-ant-…` key is the remaining manual step per the plan (`plan-phase-3a.md` §Task 8.1). Once a turn streams successfully end-to-end (text + tool call + tool result + final text) the Phase 3a row flips to ✅ and the status note is removed.

**Phase 3b (Ollama-native adapter)** is drafted in [`plan-phase-3b.md`](plan-phase-3b.md). Targets `POST {base_url}/api/chat` with native tool-call streaming and `options.num_ctx` KV-cache control. **First adapter to use a non-SSE streaming format** — adds a small `StreamingFormat` enum + `streaming_format()` trait method (default SSE) and a parallel NDJSON drive loop in `synthesize_stream`. Existing Ollama users on the OpenAI-compat layer (`api_type = OpenAi` against `http://localhost:11434/v1`) keep working unchanged; selecting `Ollama` as the api_type opts into the native path.

**Future phases (3c / 3d / 4)** — native Gemini / DeepSeek adapters and polish features (`/models` fetch, models.dev catalog, multimodal, dedicated compaction model) — remain unscheduled and will get their own design + plan when started.

| Phase | Plan | Status |
|---|---|---|
| 1a — symbol-only rename (`LocalProviderKeyManager` → `AgentProviderSecrets`, `LocalProviderWidget` → `AgentProvidersWidget`) | [`plan-phase-1a.md`](plan-phase-1a.md) | ✅ shipped |
| 1b-1 — BYOP foundation: types, setting markers, `byop:` LLMId codec | [`plan-phase-1b-1-foundation.md`](plan-phase-1b-1-foundation.md) | ✅ shipped |
| 1b-2 — secrets HashMap + migration helper + dispatch routing + picker injection | [`plan-phase-1b-2-dispatch.md`](plan-phase-1b-2-dispatch.md) | ✅ shipped |
| 1b-3 — settings UI rebuild (`AgentProvidersWidget` list view) | [`plan-phase-1b-3-settings-ui.md`](plan-phase-1b-3-settings-ui.md) | ✅ shipped |
| 1b-4 — legacy `local:` cleanup | [`plan-phase-1b-4-cleanup.md`](plan-phase-1b-4-cleanup.md) | 📋 queued (gated on migration adoption) |
| 2 — `ProviderAdapter` trait + `OpenAiAdapter` + Test connection probe | [`plan-phase-2.md`](plan-phase-2.md) | ✅ shipped |
| 3a — Anthropic adapter (`AnthropicAdapter` + `AnthropicSseDecoder`) | [`plan-phase-3a.md`](plan-phase-3a.md) | 🧪 code complete — pending live smoke |
| 3b — Ollama-native adapter (`OllamaAdapter` + NDJSON drive loop + `streaming_format` trait method) | [`plan-phase-3b.md`](plan-phase-3b.md) | 📋 drafted |

The full design — data model, dispatch flow, migration strategy, risks — is in [`design.md`](design.md).

## What landed

**User-visible:**
- New **Custom AI Providers** panel in Settings → AI. Add/remove provider cards (name, base URL, API key, API type chip, models table). Per-model display name, model id, context window, and tool-calling toggle.
- Picker shows an entry per `(provider, model)` pair labelled `<provider> / <model>`. Selecting it routes the conversation through the user's endpoint.
- Existing single-provider users have their config auto-migrated on first launch; the migrated provider appears as one card with one model and the API key intact.
- **Phase 2:** per-card **Test connection** button that probes the provider endpoint and surfaces Idle / Probing / Ok / Failed state inline.
- **Phase 3a (pending live smoke):** **Anthropic** is now a real api_type — selecting it routes the conversation to `{base_url}/v1/messages` with native `x-api-key` auth, streamed `message_start` / `content_block_delta` / `message_stop` events, and tool use as `tool_use` content blocks on the assistant message. The Test connection button probes `{base_url}/v1/models`.

**Architecture:**
- Type system in `app/src/settings/ai.rs`: `AgentProvider`, `AgentProviderModel`, `AgentProviderKind` (`OpenAiCompatible` only today), `AgentProviderApiType` (`OpenAi`, `Anthropic` active; `OpenAiResp`, `Gemini`, `Ollama`, `DeepSeek` enum variants reserved for Phase 3b–d).
- Persistence: `Vec<AgentProvider>` under `agents.warp_agent.providers` (TOML); `HashMap<provider_id, api_key>` in OS keychain blob `AgentProviderSecrets`.
- LLMId codec in `crates/ai/src/local_provider/llm_id.rs` (`byop:<uuid>:<model_id>` with first-colon-after-prefix splitting so vendor:model:variant style names round-trip).
- Dispatch in `app/src/ai/local_provider_config.rs::snapshot_for_request` branches on prefix:
  - `byop:` → `agent_providers::lookup_byop` → local provider runtime.
  - `local:` (legacy) → `snapshot_from_app` legacy path (removed in 1b-4).
  - Anything else → cloud-Warp path (untouched).
- Migration in `app/src/ai/agent_providers/migration.rs`: idempotent, runs once on app boot after singleton registration. Synthesizes a single `AgentProvider` from the legacy `agents.local_provider.*` settings, moves the API key from the `__legacy__` placeholder id to a fresh UUID, sets the marker.
- **Phase 2:** `ProviderAdapter` trait (`crates/ai/src/local_provider/adapters/mod.rs`) abstracts wire-protocol differences; `OpenAiAdapter` is the canonical impl. `StreamDecoder` trait split out so per-turn stream state stays addressable.
- **Phase 3a:** `AnthropicAdapter` + `AnthropicSseDecoder` (`crates/ai/src/local_provider/adapters/anthropic/{mod,request,response,wire}.rs`). Translator lifts the synthesized system prompt to Anthropic's top-level `system` field, merges adjacent same-role messages, splices missing `tool_result` blocks. Decoder maps the named event family to the same `ResponseEvent` shape `OpenAiSseAdapter` emits. `StreamDecoder` trait gained `feed_event(event_name, data)` to carry the SSE `event:` discriminator through.

## Future phases (per [`design.md`](design.md) §9)

Each gets its own design + plan when started:

- **Phase 3b** — native Ollama adapter (`/api/chat`, NDJSON streaming, native tool-call streaming, `options.num_ctx` knob). Plan drafted at [`plan-phase-3b.md`](plan-phase-3b.md).
- **Phase 3c** — native Gemini adapter (`POST /v1beta/models/{model}:streamGenerateContent?alt=sse`, `x-goog-api-key` auth, content-parts message shape).
- **Phase 3d** — native DeepSeek adapter (reasoning-content surfacing for `deepseek-reasoner`).
- **Phase 4a–d** — `/models` fetch button, models.dev catalog sync, multimodal capability resolution (image / pdf / audio), dedicated compaction model.

## Operational notes

- The legacy `LocalLlmProvider` feature flag continues to gate the entire feature; renaming it is intentionally not part of any phase to avoid churn in flag rollout configs.
- The `agents.warp_agent.migration.legacy_local_provider_migrated` setting marker prevents re-running migration on subsequent launches. After Phase 1b-4 it stays as `#[allow(dead_code)]` for telemetry/forensics.
- Tag [`v0.1.0`](https://github.com/wolverine2k/warp/releases/tag/v0.1.0) marks the end of Phase 1b-3 (post-dispatch-scoping fix); tag `v0.2.0` will mark the end of Phase 1b-4 cleanup.

## Reading order for new contributors

1. [`design.md`](design.md) — architecture and the 4-stage roadmap.
2. The per-phase plan files in execution order: 1a → 1b-1 → 1b-2 → 1b-3 → 1b-4 → 2 → 3a.
3. Source:
   - Dispatch path: `crates/ai/src/local_provider/{agent_provider_secrets,llm_id}.rs` and `app/src/ai/agent_providers/{mod,migration}.rs`.
   - UI: `app/src/settings_view/agent_providers_widget.rs`.
   - Adapter trait + selector: `crates/ai/src/local_provider/adapters/mod.rs`.
   - OpenAi adapter: `crates/ai/src/local_provider/adapters/openai.rs`.
   - Anthropic adapter: `crates/ai/src/local_provider/adapters/anthropic/{mod,request,response,wire}.rs`.

## Reference comparison

The data model and naming are adopted verbatim from the [`openwarp`](https://github.com/wolverine2k/warp/tree/openwarp) branch's BYOP design (with all Chinese comments translated to English during the port). This makes a future merge with openwarp's other features (models.dev catalog, native non-OpenAI adapters, multimodal) conflict-light.
