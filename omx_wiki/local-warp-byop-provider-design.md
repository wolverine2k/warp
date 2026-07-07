---
title: "Local-Warp BYOP Provider Design"
tags: ["architecture", "ai", "byop", "local-provider", "settings", "secrets", "design"]
created: 2026-07-07T07:05:14.249Z
updated: 2026-07-07T07:05:14.249Z
sources: []
links: []
category: architecture
confidence: medium
schemaVersion: 1
---

# Local-Warp BYOP Provider Design

# Local-Warp BYOP Provider Design

Captured: 2026-07-07.

## Product Intent

Local-Warp adds provider autonomy on top of Warp Agent Mode. The design supports user-configured providers and models while keeping secrets out of user-visible settings. The public README documents providers under `agents.warp_agent.providers`, OS keychain storage through `AgentProviderSecrets`, and BYOP model IDs of the form `byop:<provider_id>:<model_id>`.

## Data Model

`app/src/settings/ai.rs` defines the settings model:

- `AgentProvider` contains `id`, `name`, `kind`, `api_type`, `base_url`, `models`, `available_for_orchestration`, and `remote_secret_name`.
- `AgentProviderModel` contains `name`, `id`, `context_window`, `max_output_tokens`, `reasoning`, `tool_call`, and optional image/pdf/audio capability flags.
- `AgentProviderApiType` is re-exported from `ai::local_provider`.
- Raw API keys are not fields on `AgentProvider`; they belong in `AgentProviderSecrets`/secure storage or managed remote secret references.

Legacy `agents.local_provider.*` settings still exist for migration, but new multi-provider behavior should use `AISettings.agent_providers`.

## BYOP LLM IDs

`crates/ai/src/local_provider/llm_id.rs` owns BYOP ID encoding and decoding:

- Prefix: `byop:`.
- Encoding: `byop:<provider_id>:<model_id>`.
- Decoding splits only at the first separator after the provider ID, so model IDs may contain colons.
- `is_byop` identifies BYOP IDs.

Preserve this boundary. Future parsing code must not split model IDs on every colon.

## Provider Resolution

`app/src/ai/agent_providers/mod.rs` builds picker entries and resolves runtime provider configuration:

- `build_byop_llm_infos(app)` returns selectable BYOP models for configured providers with a base URL, models, and required credentials.
- `build_byop_orchestration_llm_infos(app)` further requires `available_for_orchestration`.
- Ollama can be unauthenticated; other providers require an API key or managed secret.
- `lookup_byop(&LLMId)` and child-resolution helpers decode the BYOP ID, find the provider in `AISettings.agent_providers`, load credentials from `AgentProviderSecrets`, and validate required fields.

Remote child resolution maps API type strings such as `open_ai`, `open_ai_resp`, `anthropic`, `gemini`, `ollama`, and `deep_seek` and returns managed secret names rather than raw keys.

## Request Dispatch

`app/src/ai/agent/api/impl.rs` routes local/BYOP requests. It builds local provider input from `RequestParams`, including model, supported tools, user query, task/conversation IDs, action results, synthetic user queries, compaction configuration/state, and attachments. It then calls `local_provider::run_chat_turn(input, cfg, cancellation_rx, reqwest::Client::new()).await` and wraps returned local events as a response stream.

## Provider Adapters

`crates/ai/src/local_provider/adapters/mod.rs` defines the adapter boundary:

- `ProviderAdapter` builds chat, summarizer, probe, and list-models requests and parses streaming or list-model responses.
- `StreamingFormat` supports SSE and newline-delimited JSON.
- `StreamDecoder` consumes stream chunks/events and records upstream errors.
- Concrete adapters exist for OpenAI-compatible, Anthropic, Ollama, Gemini, and DeepSeek. `OpenAiResp` is represented but currently unsupported by `select_adapter`.
- Ollama uses NDJSON; OpenAI-compatible, Anthropic, Gemini, and DeepSeek use SSE.

Provider-specific behavior should stay in adapters rather than leaking conditionals through orchestration code.

## Capabilities and Attachments

`crates/ai/src/capabilities.rs` resolves multimodal capability support with precedence: explicit user setting, models.dev catalog data, heuristic table, then false. `crates/ai/src/attachments.rs` defines runtime attachments with MIME metadata, bytes, display name, and optional thumbnails; bytes should not be serialized into conversation history.

## Invariants for Future Work

- Never store, log, or send raw BYOP API keys through settings, telemetry, wiki, or generated design docs.
- Keep Local-Warp visible copy and provider autonomy consistent with the README and `specs/multi-local-llm` design.
- Preserve `byop:<provider_id>:<model_id>` semantics exactly.
- Keep adapters responsible for provider-specific protocol differences.
- Keep capability resolution deterministic and explainable to users.
- When adding toggleable provider behavior, add discoverable Settings and Command Palette surfaces if the setting is user-facing.
