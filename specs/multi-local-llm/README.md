# Multi-Local-LLM (BYOP) — Specs Index

Bring Your Own Provider for the Agent Mode dispatcher. This directory holds the design, phased implementation plans, and operational gating notes for the **multi-local-LLM** initiative on the `multi-local-llm` branch.

The work extends the single-local-provider scaffolding from `nmehta/local-llm-provider` (the `local:`-prefixed picker entry, OpenAI-compatible wire code, and compaction pipeline) into a `Vec<AgentProvider>` that lets a user configure **multiple endpoints simultaneously** (Ollama + LM Studio + a remote OpenAI-compatible host + …) and pick a specific *provider × model* per conversation. Cloud-Warp models continue to dispatch through the warp.dev path; the override is scoped to BYOP-flagged LLMIds (`byop:<provider_id>:<model_id>`) so picking a cloud model is unaffected.

## Status

# ✅ Phase 1 — COMPLETE

**Tagged `v0.1.0` on 2026-05-08.** All four sub-phases (1a / 1b-1 / 1b-2 / 1b-3) shipped end-to-end on `multi-local-llm` and are on `origin`. The branch is 31 commits ahead of `nmehta/local-llm-provider` (its forking point); end-to-end manual smoke testing on macOS confirmed BYOP picker entries route to local endpoints and cloud-Warp picks route to warp.dev.

**Phase 1b-4 (legacy cleanup)** is drafted and queued in [`plan-phase-1b-4-cleanup.md`](plan-phase-1b-4-cleanup.md). Execution is gated on telemetry confirming migration adoption ≥ 99% — see the plan's preamble for the gating checklist.

**Phase 2 (ProviderAdapter trait + Test connection button)** is drafted in [`plan-phase-2.md`](plan-phase-2.md). Hoists the OpenAI-compatible wire code behind a `ProviderAdapter` / `StreamDecoder` trait pair so Phase 3 can plug in native non-OpenAI adapters without touching `run.rs`. **No user-visible behavior change** for OpenAI; the existing test suite stays green and exercises the same paths through the trait.

**Future phases (3 / 4)** — native non-OpenAI adapters and polish features (`/models` fetch, models.dev catalog, multimodal, dedicated compaction model) — remain unscheduled and will get their own design + plan when started.

| Phase | Plan | Status |
|---|---|---|
| 1a — symbol-only rename (`LocalProviderKeyManager` → `AgentProviderSecrets`, `LocalProviderWidget` → `AgentProvidersWidget`) | [`plan-phase-1a.md`](plan-phase-1a.md) | ✅ shipped |
| 1b-1 — BYOP foundation: types, setting markers, `byop:` LLMId codec | [`plan-phase-1b-1-foundation.md`](plan-phase-1b-1-foundation.md) | ✅ shipped |
| 1b-2 — secrets HashMap + migration helper + dispatch routing + picker injection | [`plan-phase-1b-2-dispatch.md`](plan-phase-1b-2-dispatch.md) | ✅ shipped |
| 1b-3 — settings UI rebuild (`AgentProvidersWidget` list view) | [`plan-phase-1b-3-settings-ui.md`](plan-phase-1b-3-settings-ui.md) | ✅ shipped |
| 1b-4 — legacy `local:` cleanup | [`plan-phase-1b-4-cleanup.md`](plan-phase-1b-4-cleanup.md) | 📋 queued (gated on migration adoption) |
| 2 — `ProviderAdapter` trait + `OpenAiAdapter` + Test connection probe | [`plan-phase-2.md`](plan-phase-2.md) | 🚧 in progress |

The full design — data model, dispatch flow, migration strategy, risks — is in [`design.md`](design.md).

## What landed

**User-visible:**
- New **Custom AI Providers** panel in Settings → AI. Add/remove provider cards (name, base URL, API key, OpenAI-compatible API type chip, models table). Per-model display name, model id, context window, and tool-calling toggle.
- Picker shows an entry per `(provider, model)` pair labelled `<provider> / <model>`. Selecting it routes the conversation through the user's endpoint.
- Existing single-provider users have their config auto-migrated on first launch; the migrated provider appears as one card with one model and the API key intact.

**Architecture:**
- Type system in `app/src/settings/ai.rs`: `AgentProvider`, `AgentProviderModel`, `AgentProviderKind` (`OpenAiCompatible` only today), `AgentProviderApiType` (`OpenAi` active; `OpenAiResp`, `Gemini`, `Anthropic`, `Ollama`, `DeepSeek` enum variants reserved for Phase 3).
- Persistence: `Vec<AgentProvider>` under `agents.warp_agent.providers` (TOML); `HashMap<provider_id, api_key>` in OS keychain blob `AgentProviderSecrets`.
- LLMId codec in `crates/ai/src/local_provider/llm_id.rs` (`byop:<uuid>:<model_id>` with first-colon-after-prefix splitting so vendor:model:variant style names round-trip).
- Dispatch in `app/src/ai/local_provider_config.rs::snapshot_for_request` branches on prefix:
  - `byop:` → `agent_providers::lookup_byop` → local provider runtime.
  - `local:` (legacy) → `snapshot_from_app` legacy path (removed in 1b-4).
  - Anything else → cloud-Warp path (untouched).
- Migration in `app/src/ai/agent_providers/migration.rs`: idempotent, runs once on app boot after singleton registration. Synthesizes a single `AgentProvider` from the legacy `agents.local_provider.*` settings, moves the API key from the `__legacy__` placeholder id to a fresh UUID, sets the marker.

## Future phases (per [`design.md`](design.md) §9)

Each gets its own design + plan when started:

- **Phase 2** — `ProviderAdapter` trait so per-protocol codecs are pluggable.
- **Phase 3a–d** — native adapters for Anthropic / Ollama / Gemini / DeepSeek.
- **Phase 4a–d** — `/models` fetch button, models.dev catalog sync, multimodal capability resolution (image / pdf / audio), dedicated compaction model.

## Operational notes

- The legacy `LocalLlmProvider` feature flag continues to gate the entire feature; renaming it is intentionally not part of any phase to avoid churn in flag rollout configs.
- The `agents.warp_agent.migration.legacy_local_provider_migrated` setting marker prevents re-running migration on subsequent launches. After Phase 1b-4 it stays as `#[allow(dead_code)]` for telemetry/forensics.
- Tag [`v0.1.0`](https://github.com/wolverine2k/warp/releases/tag/v0.1.0) marks the end of Phase 1b-3 (post-dispatch-scoping fix); tag `v0.2.0` will mark the end of Phase 1b-4 cleanup.

## Reading order for new contributors

1. [`design.md`](design.md) — architecture and the 4-stage roadmap.
2. The per-phase plan files in execution order: 1a → 1b-1 → 1b-2 → 1b-3 → 1b-4.
3. Source: `crates/ai/src/local_provider/{agent_provider_secrets,llm_id}.rs` and `app/src/ai/agent_providers/{mod,migration}.rs` for the dispatch path; `app/src/settings_view/agent_providers_widget.rs` for the UI.

## Reference comparison

The data model and naming are adopted verbatim from the [`openwarp`](https://github.com/wolverine2k/warp/tree/openwarp) branch's BYOP design (with all Chinese comments translated to English during the port). This makes a future merge with openwarp's other features (models.dev catalog, native non-OpenAI adapters, multimodal) conflict-light.
