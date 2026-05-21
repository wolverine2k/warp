# Phase 5 — BYOP in Agent Orchestration — Design Spec

**Date:** 2026-05-21
**Author:** nmehta
**Branch:** `multi-local-llm`
**Parent design:** `specs/multi-local-llm/design.md` (new §16)

---

## Goal

Surface BYOP-configured models in the agent orchestration model picker, validate compatibility against the selected harness + execution mode, and route the run-wide `model_id` plus credentials through both the Native (Warp/Oz) agent loop and the compatible external harness CLIs (Claude Code, Codex, OpenCode, Gemini CLI).

Today, orchestration's model picker is fed by `LLMPreferences::get_base_llm_choices_for_agent_mode`, which chains first-party server models with `custom_llm_choices`. `custom_llm_choices` only returns the older "Custom Inference Endpoints" entries (gated on `FeatureFlag::CustomInferenceEndpoints`) — it never reads the `AgentProviders` registry that Phase 1–4d built up. BYOP models are therefore silently absent from orchestration. Phase 5 closes that gap.

## Non-goals

- No new BYOP API types — Phase 5 uses the existing six from Phase 4 (OpenAI, OpenAIResp, Gemini, Anthropic, Ollama, DeepSeek).
- No changes to the external CLI binaries themselves. Phase 5 only sets env vars / CLI flags that those CLIs already honor.
- No new harness types. Phase 5 ships against the current set: Oz/Native, Claude Code, Codex, OpenCode, Gemini.
- No per-child model override — the run-wide `model_id` applies to all children. This matches the current orchestration UI.
- No silent fallback in orchestration. A misconfigured BYOP model is a hard error at submit (orchestration is more expensive to retry than a single conversation, so we'd rather block than run the wrong model).

---

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Integration shape | Add a dedicated `LLMPreferences::byop_llm_choices(ctx)` source and chain it **only inside `get_orchestration_llm_choices`** | Smallest delta that keeps Phase 5's blast radius scoped to orchestration. `custom_llm_choices` is unchanged, so the coding-model and CLI-agent pickers don't silently start showing BYOP entries. Reuses Phase 4d's `byop:<provider_id>:<model_id>` ID convention via `llm_id::encode()`. Approaches B (new trait abstraction) and C (parallel BYOP picker) were considered and rejected — see Architecture below. |
| Picker UX for incompatible entries | Filter incompatible BYOP entries out of the picker | Cleanest UX — user only sees valid choices. Picker re-filters when `harness_type` or `execution_mode` change. |
| External-harness scope | Native + matching external harnesses (Claude Code, Codex, OpenCode, Gemini) | Broader than the minimal "native-only" scope; requires API-type ↔ harness mapping and env-var injection, but unlocks the headline use case (run BYOP models inside Codex/Claude Code child agents). |
| API-type ↔ harness matrix | `Anthropic → {Native, Claude Code}`; `OpenAI/OpenAIResp/DeepSeek → {Native, Codex, OpenCode}` (OpenAIResp not on OpenCode); `Gemini → {Native, Gemini CLI}`; `Ollama → {Native only}` | OpenCode hasn't adopted OpenAI Responses API yet (excluded from `OpenAIResp`). Ollama's OpenAI-compat shim is intentionally excluded from Codex/OpenCode this phase to avoid a confusing dual-routing path. |
| Remote-mode reachability | BYOP entries with localhost / loopback / RFC1918 / `.local` base URLs are filtered out when `execution_mode = Remote` | A Warp worker can't reach a user's `http://localhost:11434`. Heuristic stays best-effort; users who self-host on private DNS can flag-override per provider. |
| Remote credential propagation | Per-BYOP-provider managed secret (`remote_secret_name`) resolved via the existing `RunAgentsRequest.auth_secret_name` machinery; Auto-create button writes the api_key into a personal managed secret named `byop-{provider_id}` | Reuses the existing managed-secret pipeline rather than inventing a new credential channel. Personal-owner default keeps team workspaces from accidentally exposing personal keys. |
| Per-provider orchestration opt-in | New `available_for_orchestration: bool` toggle on each BYOP provider (default off) | Users with half-configured providers (in-progress, broken, or experimental) can keep them around for the main conversation without polluting the orchestration picker. Explicit opt-in also gives a place to surface the Remote secret-name field. |
| Validation strictness | Hard error at submit when `model_id` starts with `byop:` and the harness/mode combo is incompatible | Three layers — picker filter (primary), submit-time guard in `validate_request`, dispatch-time `BYOPModelResolutionError`. No silent fallback to first-party defaults. |
| Compaction inheritance (Phase 4d) | Local children inherit `byop_compaction_model_*` settings live from `AISettings`. Remote children get `compaction_model_provider_id` / `compaction_model_id` forwarded as optional fields on `RunAgentsRequest` | Local path requires no new wiring (settings are global). Remote needs explicit forwarding because the worker host doesn't share the user's settings store. |

---

## Architecture

### Picker source: dedicated `byop_llm_choices`

Phase 5 does **not** modify `LLMPreferences::custom_llm_choices()` (which already feeds the coding-model and CLI-agent pickers). Instead it introduces a new sibling:

```rust
pub fn byop_llm_choices(&self, ctx: &AppContext) -> impl Iterator<Item = &LLMInfo>;
```

`byop_llm_choices` returns a freshly-built `Vec<LLMInfo>` synthesized from the `AgentProviders` registry (`app/src/settings/ai.rs`), gated on `FeatureFlag::LocalLlmProvider` (the same flag that gates BYOP for the main conversation). This new source is chained **only inside `get_orchestration_llm_choices`** (next section); existing pickers — `get_base_llm_choices_for_agent_mode`, `get_coding_llm_choices`, `get_cli_agent_llm_choices` — are unchanged. Exposing BYOP in those other pickers is out of scope for Phase 5 and would be a follow-up phase with its own opt-in.

Each BYOP `(provider, model)` pair becomes one `LLMInfo` with:
- `id = byop:<provider_id>:<model_id>` — encoded via existing `llm_id::encode()`. Matches Phase 4d's ID scheme so the rest of the pipeline (validation, dispatch, compaction) recognizes the ID without change.
- `display_name = "{provider.name} / {model.name}"` — matches the Phase 4d compaction dropdown format for consistency.
- `provider`, `description`, `disable_reason`, `tags` populated from provider/model metadata. Capabilities mirror the model's `tools` / `image` / `pdf` / `audio` toggles.

Rebuild trigger: cache the synthesized `Vec<LLMInfo>` on `LLMPreferences` and rebuild on the same hook the rest of `LLMPreferences` already uses for settings-driven invalidation (subscribe to `AgentProvidersChanged` if the existing event bus exposes one, otherwise rebuild lazily on each call — performance is fine because the registry is small and pickers open infrequently).

### Filter wrapper: `get_orchestration_llm_choices`

New helper on `LLMPreferences`:

```rust
pub fn get_orchestration_llm_choices(
    &self,
    ctx: &AppContext,
    harness_type: &str,
    execution_mode: &RunAgentsExecutionMode,
) -> impl Iterator<Item = &LLMInfo>;
```

Implementation chains `get_base_llm_choices_for_agent_mode(ctx)` with `byop_llm_choices(ctx)` and runs the result through three filter passes. The filters apply only to BYOP entries — first-party server models and legacy custom-endpoint entries pass through unchanged:

1. **Per-provider opt-in** — provider's `available_for_orchestration` toggle must be on (see Settings UI). Default off, so existing BYOP configs don't surface in orchestration until the user explicitly enables them.
2. **Harness compatibility** — `byop_harness_compatible(api_type, harness_type)` from the matrix above. Empty/Oz harness behaves as "Native".
3. **Execution-mode reachability** — when `execution_mode = Remote`, parse the provider's `base_url` and reject loopback, RFC1918 (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), `.local`, `.localhost`. Local-mode bypasses this filter.

The compatibility matrix and reachability heuristic live in a new helper module (`byop_orchestration_filter.rs`) so `llms.rs` doesn't grow further; `get_orchestration_llm_choices` is a thin method on `LLMPreferences` that calls into those helpers.

### Validation: `validate_orchestration_model_id`

New function alongside the existing `validate_agent_mode_base_model_id` in `app/src/ai/agent_sdk/common.rs`:

```rust
pub fn validate_orchestration_model_id(
    model_id: &str,
    harness_type: &str,
    execution_mode: &RunAgentsExecutionMode,
    ctx: &AppContext,
) -> anyhow::Result<LLMId>;
```

The existing `validate_agent_mode_base_model_id` is unchanged so per-conversation BYOP isn't affected. The new function consults `get_orchestration_llm_choices` and produces a structured error explaining the incompatibility:

- `"BYOP model '{provider}/{model}' (API type Anthropic) is not compatible with harness 'codex'. Use 'claude-code' or 'oz', or pick a different model."`
- `"BYOP model '{provider}/{model}' base URL 'http://localhost:11434' is not reachable from Remote execution. Pick Local mode or a publicly-accessible provider."`

### Env-var injection: `orchestration_byop_env`

New module `app/src/ai/orchestration_byop_env.rs`:

```rust
pub fn byop_env_for_harness(
    provider: &AgentProvider,
    model_id: &str,
    harness_type: &str,
) -> Vec<(String, String)>;
```

Returns the env-var bag a Local child harness needs to talk to the BYOP endpoint:

| Harness | Env vars set |
|---|---|
| `claude-code` | `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL` |
| `codex` | `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `OPENAI_MODEL` |
| `opencode` | `OPENAI_BASE_URL`, `OPENAI_API_KEY` |
| `gemini` | `GOOGLE_GENAI_USE_VERTEXAI=false`, `GOOGLE_API_KEY` |
| `oz` / empty (Native) | none — Native uses the existing in-process BYOP dispatcher |

Unsupported combinations (e.g., Ollama + Codex) return an empty vec — the validator will already have rejected at submit, so this is defence-in-depth.

The api_key value is read from the BYOP provider's stored secret (existing path). The returned `Vec` is consumed by the child-spawn site (`app/src/pane_group/child_agent.rs`) and threaded into `Command::envs(...)`. The api_key is redacted in any logs that capture the env-var bag.

### Remote credential bridge

Each BYOP provider gains an optional `remote_secret_name: String` in settings. When non-empty, it names a managed secret in the user's personal or team store containing the api_key. The orchestration submit path:

1. Reads the selected provider's `remote_secret_name`.
2. Sets `RunAgentsRequest.auth_secret_name = Some(remote_secret_name)` — the existing Remote channel.
3. Passes the BYOP provider's `base_url` and `api_type` alongside `model_id` via two new fields on `RunAgentsRequest`: `byop_base_url: Option<String>`, `byop_api_type: Option<String>` (populated only when `model_id` starts with `byop:`).
4. The worker host fetches the api_key from the named managed secret on receive, combines with `byop_base_url` and `byop_api_type` to reconstruct a runtime equivalent of `LocalProviderConfig`, then runs the child agent against that config.

### Compaction inheritance (Phase 4d)

**Local children** read `AISettings` directly — `CompactionDispatcher::resolve_target` (Phase 4d) already does the right thing. No new wiring.

**Remote children** need the compaction config forwarded. `RunAgentsRequest` gains two optional fields: `compaction_model_provider_id`, `compaction_model_id`. Populated on submit when the user has Phase 4d settings configured. The worker-side `CompactionDispatcher::resolve_target` is extended to accept these as an override; falls back to the conversation primary when absent. This matches the Phase 4d fallback semantics for the Local path.

### Why not Approach B (unified `AgentModeModelSource` trait)

We considered a new trait with impls for first-party, legacy custom endpoints, and BYOP, with `LLMPreferences` iterating over registered sources. Rejected because:
- The trait would refactor a hot, fragile area (`LLMPreferences` is the central model-routing entry point) for speculative future-proofing.
- Phase 5 doesn't introduce a third model source — only BYOP. The trait would have exactly two impls.
- It would lengthen the diff and the review surface without a concrete payoff. YAGNI.

### Why not Approach C (parallel BYOP-only orchestration path)

We considered a separate "BYOP" section in the orchestration modal with its own picker and dispatch path that bypasses `LLMPreferences::choices`. Rejected because:
- Two pickers means the user has to know which one to use — UX inconsistency.
- Two validation paths and two dispatchers means sustained duplication.
- It doesn't match how Phase 4d wired BYOP for the main conversation, which integrated cleanly with the existing code path.

---

## Settings UI

### New controls per BYOP provider card

In `app/src/settings_view/agent_providers_widget.rs`, gated on `FeatureFlag::LocalLlmProvider`:

1. **"Available for orchestration"** toggle.
   - Default: **off**.
   - When **off**, the provider's models stay available for the main conversation but never appear in orchestration's model picker.
   - When **on**, the models are eligible for the orchestration picker (subject to harness/mode filters).
   - Renders below the "Models" section, above the bottom-row buttons.

2. **"Remote managed secret"** field (visible only when toggle is on, *and* the base URL is not a private/localhost address).
   - Layout: a text input for the secret name, plus an **"Auto-create"** button.
   - Auto-create: calls `UpdateManager::create_managed_secret(name = "byop-{provider_id}", value = provider.api_key, owner = Personal)` and writes the returned name back into the field.
   - Helper text below the field: "Required for Remote orchestration. Skip if this provider is only used for Local."
   - Status indicator (red dot + tooltip) renders inline if a name is set but `UpdateManager::resolve_managed_secret(name)` returns not-found.

### New actions on `AISettingsPageAction`

- `ToggleAgentProviderOrchestrationAvailability { provider_index }`
- `SetAgentProviderRemoteSecretName { provider_index, name: String }`
- `AutoCreateAgentProviderManagedSecret { provider_index }`

### Orchestration modal — picker behaviour

`app/src/workspace/view/orchestration_launch_modal/view.rs`:

- Replace the picker's source iterator from `get_base_llm_choices_for_agent_mode(ctx)` to `get_orchestration_llm_choices(ctx, harness_type, execution_mode)`.
- Re-render the picker when `harness_type` or `execution_mode` changes. Existing `OrchestrationLaunchModalAction::SetHarnessType` / `SetExecutionMode` already trigger view rebuilds.
- If the previously-selected `model_id` is no longer in the filtered set (user switched harness after picking), reset the selection to the default and surface a dim inline notice: "Previous selection no longer compatible — pick again."
- Empty-state messaging: when the filtered set contains zero BYOP entries *and* the user has any BYOP providers configured, render a dim line: "No BYOP providers compatible with `{harness_type}` + `{execution_mode}`."

### Submit-time validation

`app/src/ai/blocklist/action_model/execute/run_agents.rs::validate_request`:

- After existing checks, if `request.model_id.starts_with("byop:")`, call `validate_orchestration_model_id(&request.model_id, &request.harness_type, &request.execution_mode, ctx)`.
- On error, return the structured message — the existing modal already surfaces `Err(String)` from `validate_request` in the launch result.

---

## Test plan

### Unit tests (`crates/ai/`) — 3 tests

1. `llm_id_byop_round_trip_with_orchestration_id` — `byop:<provider>:<model>` encodes/decodes losslessly via `llm_id::encode/decode`, identical to Phase 4d behavior.
2. `orchestration_byop_env_anthropic_emits_expected_vars` — `byop_env_for_harness(Anthropic provider, "claude-code")` returns `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL` with the expected values.
3. `orchestration_byop_env_unsupported_combination_returns_empty` — `byop_env_for_harness(Ollama provider, "codex")` returns an empty `Vec` (matrix says Ollama is Native-only).

### Unit tests (`app/`) — 9 tests

4. `byop_llm_choices_synthesizes_llm_info_per_model` — a provider with two models produces two `LLMInfo` entries from `byop_llm_choices` with distinct `byop:` IDs.
5. `byop_llm_choices_empty_when_feature_flag_off` — `FeatureFlag::LocalLlmProvider` disabled → `byop_llm_choices` returns an empty iterator.
6. `byop_entries_hidden_from_other_pickers` — `get_coding_llm_choices` and `get_cli_agent_llm_choices` do **not** include BYOP entries (Phase 5 scope check; guards against scope-creep).
7. `byop_entries_hidden_when_orchestration_toggle_off` — provider's `available_for_orchestration = false` → `get_orchestration_llm_choices` excludes the provider.
8. `picker_filter_matches_anthropic_byop_to_claude_code_only` — Anthropic BYOP visible with `harness_type = "claude-code"`, hidden with `"codex"`.
9. `picker_filter_excludes_localhost_byop_from_remote_mode` — OpenAI-API BYOP provider at `http://localhost:8080` is filtered out when `execution_mode = Remote` + `harness_type = "codex"`. (Ollama isn't used here because it's already Native-only per the matrix, which would mask the reachability test under the harness filter.)
10. `picker_filter_allows_public_byop_in_remote_mode` — same provider with `base_url = "https://my-llm.example.com"` is shown in Remote + Codex.
11. `validate_orchestration_model_id_rejects_byop_with_incompatible_harness` — submit-time validator catches a stale picker selection and returns a clear error.
12. `auto_create_managed_secret_writes_name_into_settings` — clicking Auto-create populates `remote_secret_name` and persists.

### Integration tests (`crates/ai/tests/`) — 2 tests

13. `orchestration_local_launches_codex_with_openai_env` — boot two mock HTTP servers (one matches the BYOP base URL). Submit Local orchestration with `harness_type = "codex"` and an OpenAI-API BYOP provider. Assert the child process is spawned with `OPENAI_BASE_URL` pointing at the mock and that subsequent traffic hits it (not the cloud server).
14. `orchestration_remote_payload_includes_compaction_model_fields` — Remote submit with Phase 4d compaction settings populated forwards `compaction_model_provider_id` and `compaction_model_id` in the `RunAgentsRequest` payload, and the worker-side `CompactionDispatcher::resolve_target` honors them.

### Manual smoke

- Configure an Ollama BYOP provider locally with `available_for_orchestration = on`. Pick `Native` harness, `Local` mode, launch 2 child agents. Confirm both make requests to the Ollama base URL and orchestration completes.
- Configure an Anthropic-API-compatible BYOP provider with a real key. Pick `Claude Code` harness, `Local` mode. Confirm `claude` CLI starts with `ANTHROPIC_BASE_URL` pointing at the BYOP endpoint and produces a response.
- Flip the "Available for orchestration" toggle off mid-session. Confirm the picker drops the entries immediately on next open.
- Stale-selection scenario: pick a BYOP entry, then switch the harness to an incompatible one. Submit. Confirm submit-time validation rejects with a clear message and the picker re-renders cleanly.
- Auto-create-managed-secret: click the button on a provider, confirm the field populates with `byop-{provider_id}`, verify the managed secret exists with the expected value via the existing managed-secret UI.
- Remote orchestration: configure a publicly-reachable BYOP provider with `remote_secret_name` set. Pick `Remote` mode + `Codex` harness. Confirm the worker runs the child and traffic lands on the BYOP endpoint.

---

## Risks

1. **External-CLI env-var drift.** Claude Code, Codex, Gemini CLI, OpenCode each evolve independently. An env-var name change in an upstream CLI silently breaks BYOP routing for that harness. Mitigation: pin env-var names in `orchestration_byop_env.rs` with inline comments citing the CLI version tested against, plus a release-note callout when CLIs ship breaking changes. Smoke tests in CI for the supported CLI versions would be follow-up work.
2. **Managed-secret leak surface.** Auto-create stores the BYOP api_key in the workspace managed-secret store. On a shared workspace, a careless click could expose a personal key. Mitigation: Auto-create defaults to `Personal` owner (not `Team`); the UI label explicitly says "Personal secret"; the action requires an explicit user click and is never auto-run on provider save or edit.
3. **Stale picker selection.** User picks a BYOP model, then changes harness/mode in the modal before submit. Mitigation: picker filter re-runs on field change and clears the selection if the current pick is no longer eligible, with an inline notice. Submit-time guard provides a second backstop.
4. **Reachability heuristic false positives and false negatives.** False positive: a user's BYOP provider on a Tailscale `.ts.net` address technically isn't an RFC1918 private IP, but the Warp worker can't reach it — Remote orchestration submit would succeed and then fail at dispatch. False negative: a publicly-resolvable hostname pointing at a private IP (e.g., `home.example.com → 192.168.1.10`) — the heuristic operates on the URL string, not the resolved address. Mitigation: best-effort string-based heuristic, documented as a known limitation. A configurable override (per-provider "treat as publicly reachable") is out of scope for Phase 5 and would be a small follow-up.
5. **Compaction provider deletion mid-orchestration.** A user deletes the compaction BYOP provider while orchestrated children are running. Local children read settings live and will fall back per Phase 4d's existing fallback. Remote children carry the resolved provider in their `RunAgentsRequest` envelope and complete with the snapshotted config. No new fallback paths required, but the spec test plan needs to keep the Phase 4d coverage green.
6. **Provider edit during orchestration.** Editing the `base_url` or `api_key` of an in-use BYOP provider mid-run affects Local children at the next request boundary (they re-read settings) and does *not* affect Remote children (snapshotted). Inconsistent behavior. Mitigation: a banner on the BYOP settings page when an orchestration run is active, warning that edits affect Local immediately and Remote at the next launch.

---

## File map

**Created**
- `app/src/ai/orchestration_byop_env.rs` — `byop_env_for_harness` plus the API-type ↔ harness compatibility matrix.
- `app/src/ai/orchestration_byop_env_tests.rs` — sibling unit tests.
- `app/src/ai/byop_orchestration_filter.rs` — `byop_harness_compatible(api_type, harness_type)` and the reachability heuristic. Pure helpers; `LLMPreferences::get_orchestration_llm_choices` (in `llms.rs`) calls into them.
- `app/src/ai/byop_orchestration_filter_tests.rs` — sibling unit tests.

**Modified**
- `app/src/settings/ai.rs` — add `available_for_orchestration: bool` (default false) and `remote_secret_name: String` (default empty) to `AgentProvider`. Serde defaults preserve existing settings files; no migration needed.
- `app/src/settings_view/agent_providers_widget.rs` — render the new toggle + Remote-secret field + Auto-create button.
- `app/src/settings_view/ai_page.rs` — new `AISettingsPageAction` variants: `ToggleAgentProviderOrchestrationAvailability`, `SetAgentProviderRemoteSecretName`, `AutoCreateAgentProviderManagedSecret`. Handlers update settings and (for Auto-create) call `UpdateManager::create_managed_secret`.
- `app/src/ai/llms.rs` — add `byop_llm_choices(ctx)` source method (BYOP-synthesized `LLMInfo`s); add `get_orchestration_llm_choices(ctx, harness_type, execution_mode)` that chains base + BYOP and applies filters; subscribe to `AgentProvidersChanged` for cache invalidation if available, otherwise rebuild lazily. `custom_llm_choices` is **not** modified.
- `app/src/ai/agent_sdk/common.rs` — add `validate_orchestration_model_id` alongside the existing `validate_agent_mode_base_model_id` (the latter is unchanged).
- `app/src/ai/blocklist/action_model/execute/run_agents.rs` — `validate_request` adds the BYOP guard; submit path populates the new `RunAgentsRequest` fields (`byop_base_url`, `byop_api_type`, `compaction_model_provider_id`, `compaction_model_id`, `auth_secret_name` from the provider's `remote_secret_name`).
- `app/src/pane_group/child_agent.rs` — thread the env-var bag from `orchestration_byop_env::byop_env_for_harness` into `Command::envs(...)` at Local child spawn.
- `app/src/workspace/view/orchestration_launch_modal/view.rs` — switch model-choice iterator to `get_orchestration_llm_choices`; rebuild on harness/mode change; reset stale selections; empty-state messaging.
- `crates/graphql/schema.graphql` + the `RunAgentsRequest` typed boundary — add `byop_base_url`, `byop_api_type`, `compaction_model_provider_id`, `compaction_model_id` as optional fields (Remote path).
- `app/src/ai/compaction_dispatcher.rs` — `resolve_target` accepts an optional override from `RunAgentsRequest` (Remote path); Local path unchanged.
- `crates/ai/tests/local_provider_integration.rs` — add the two new integration tests.
- `specs/multi-local-llm/README.md` — Phase 5 status row + bullets.
- `specs/multi-local-llm/design.md` — add §16 "Orchestration BYOP" with a back-reference to this spec.
