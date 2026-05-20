# Phase 4d — Dedicated Compaction Model — Design Spec

**Date:** 2026-05-20
**Author:** nmehta
**Branch:** `multi-local-llm`
**Parent design:** `specs/multi-local-llm/design.md` §15

---

## Goal

Let the user nominate a separate model for conversation compaction (summarization) so the primary agent model stays focused on agent work while a cheaper/faster model handles summarization. Common case: Claude Sonnet for the agent, Claude Haiku or a local Ollama model for compaction.

## Non-goals

- No per-conversation compaction model override (global setting only).
- No cloud-Warp model support in the compaction model picker (BYOP models only — cloud compaction uses the existing warp.dev path).
- No new compaction algorithm changes — the existing head/tail summarization pipeline is unchanged.
- No compaction-specific token tracking or cost reporting.

---

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Setting shape | Existing two-field (`byop_compaction_model_provider_id` + `byop_compaction_model_id`) | Fields already declared in `ai.rs`; reconstruct LLMId at read time via `llm_id::encode()`. No setting migration needed. |
| UI placement | "Summarization model" dropdown in the BYOP compaction settings section | Scoped to BYOP users. Doesn't touch the cloud model pickers. Lives next to auto/prune/tail_turns controls. |
| Fallback behavior | Silent runtime fallback to conversation primary + inline settings warning | Log a warning and fall back at dispatch time. Settings page shows an inline warning when the configured model can't resolve. |
| Context window threading | Split: primary for overflow trigger, summarizer for budget | `is_overflow` uses `primary_cfg.context_window` (determines WHEN compaction triggers — the agent model's context is full). `usable()` and `preserve_recent_budget()` use `summarizer_cfg.context_window` (determines HOW MUCH history the summarizer can ingest). `run_summarizer_turn` uses `summarizer_cfg` for the actual call. |
| Implementation approach | CompactionDispatcher — dedicated module owning the full compaction lifecycle | Encapsulates setting resolution, fallback, config snapshot, and dispatch in one place. Existing `local_provider_compaction.rs` functions become thin delegations. |

---

## Architecture

### New type: `CompactionTarget` (crates/ai/)

Added to `crates/ai/src/local_provider/compaction/config.rs`:

```rust
pub struct CompactionTarget {
    /// The conversation's primary model. Used for overflow detection
    /// (is_overflow checks against this model's context_window to
    /// decide WHEN compaction triggers).
    pub primary_cfg: LocalProviderConfig,
    /// The model that runs the summarizer call. When the user has
    /// configured a dedicated compaction model, this differs from
    /// primary_cfg. When unset, same as primary_cfg.
    /// Budget computation (usable, preserve_recent_budget) uses this
    /// model's context_window to decide HOW MUCH history the summarizer
    /// can ingest.
    pub summarizer_cfg: LocalProviderConfig,
}

impl CompactionTarget {
    /// No dedicated compaction model — use the conversation's primary
    /// model for everything. Zero behavior change vs. pre-4d.
    pub fn same_model(cfg: LocalProviderConfig) -> Self {
        Self {
            summarizer_cfg: cfg.clone(),
            primary_cfg: cfg,
        }
    }
}
```

### Refactored: `try_compact` (crates/ai/)

`auto.rs::try_compact` signature changes from `cfg: &LocalProviderConfig` to `target: &CompactionTarget`:

- `is_overflow(compaction_cfg, tokens, model)` — `model` derived from `target.primary_cfg.context_window` (determines WHEN to compact).
- `usable(compaction_cfg, model)` and `preserve_recent_budget(usable)` — `model` derived from `target.summarizer_cfg.context_window` (determines HOW to budget the head/tail split for the summarizer's ingest capacity).
- `run_summarizer_turn(input, cfg, http)` — `cfg` is `&target.summarizer_cfg` (the actual summarizer call).

All existing callers migrate to `CompactionTarget::same_model(cfg)` as a no-behavior-change refactor.

### New module: `CompactionDispatcher` (app/)

`app/src/ai/compaction_dispatcher.rs`:

**`resolve_target(ctx, primary_cfg) -> CompactionTarget`:**
1. Read `byop_compaction_model_provider_id` and `byop_compaction_model_id` from `AISettings`.
2. If both are non-empty, encode to `byop:<provider_id>:<model_id>` via `llm_id::encode()`.
3. Call `snapshot_for_request(ctx, &llm_id)`.
4. On `Some(summarizer_cfg)` → return `CompactionTarget { primary_cfg, summarizer_cfg }`.
5. On `None` → log warning, return `CompactionTarget::same_model(primary_cfg)`.

**`dispatch_auto(controller, conversation_id, finished_token_usage, ctx)`:**
Replaces the body of `local_provider_compaction::dispatch_auto_compaction`. Snapshots the primary config, calls `resolve_target`, builds the `CompactionTarget`, snapshots messages + compaction state from the conversation, spawns the async `try_compact` call, and commits the result back to the live conversation.

**`dispatch_manual(controller, conversation_id, ctx) -> bool`:**
Replaces the body of `local_provider_compaction::dispatch_manual_compaction`. Same pattern as `dispatch_auto` but with `manual=true`.

**`compaction_model_available(ctx) -> bool`:**
Reads the setting fields; if both non-empty, attempts `snapshot_for_request`. Returns true if resolution succeeds, false otherwise. Used by the settings UI for the inline warning.

### Existing module changes: `local_provider_compaction.rs`

`dispatch_auto_compaction` and `dispatch_manual_compaction` become thin one-line delegations to `CompactionDispatcher::dispatch_auto` / `CompactionDispatcher::dispatch_manual`. Their signatures are unchanged so call sites (`controller.rs:2949` and `slash_command.rs:82`) don't need updating.

---

## Settings UI

### Dropdown: "Summarization model"

**Location:** Inside the BYOP compaction settings section of the AI settings page (where `byop_compaction_auto`, `prune`, `tail_turns` controls render).

**Entries:**
1. **"Use conversation model"** (default) — maps to empty `provider_id` + empty `model_id`.
2. One entry per `(provider, model)` pair from `agents.warp_agent.providers` — labelled `"{provider.name} / {model.name}"`, matching the picker format.

**Action:** New `AISettingsPageAction::SetCompactionModel { provider_id: String, model_id: String }` — writes both `byop_compaction_model_provider_id` and `byop_compaction_model_id`. Empty strings = clear (revert to default).

**Render condition:** Visible when `FeatureFlag::LocalLlmProvider.is_enabled()` and at least one BYOP provider is configured.

### Inline warning

When `byop_compaction_model_provider_id` is non-empty but `CompactionDispatcher::compaction_model_available(ctx)` returns false, render a dim warning label below the dropdown:

> "Configured summarization model is unavailable — compaction will use the conversation model."

---

## Test plan

### Unit tests (crates/ai/) — 4 tests

1. `compaction_target_same_model_has_identical_configs` — `CompactionTarget::same_model(cfg)` produces identical `primary_cfg` and `summarizer_cfg`.
2. `try_compact_uses_primary_for_overflow_detection` — `primary_cfg` has a tiny context window (triggers overflow), `summarizer_cfg` has a large one. Verify overflow fires based on primary.
3. `try_compact_skipped_when_auto_disabled_with_target` — existing behavior preserved with `CompactionTarget`.
4. `try_compact_skipped_when_below_overflow_with_target` — existing behavior preserved with `CompactionTarget`.

### Unit tests (app/) — 6 tests

5. `resolve_target_empty_settings_returns_same_model` — empty `provider_id` + `model_id` → `CompactionTarget::same_model`.
6. `resolve_target_valid_setting_returns_split_target` — valid provider/model → `summarizer_cfg` differs from `primary_cfg`.
7. `resolve_target_missing_provider_falls_back_to_same_model` — deleted provider → `CompactionTarget::same_model` + log warning.
8. `compaction_model_available_false_when_provider_deleted` — deleted provider → returns false.
9. `set_compaction_model_action_writes_both_fields` — action sets both settings fields.
10. `set_compaction_model_empty_clears_to_default` — empty strings → both fields cleared.

### Integration test — 1 test

11. Boot two mock HTTP servers. Configure one as the agent provider and one as the compaction model. Trigger compaction. Assert the summarizer request hit the compaction server, not the agent server.

### Manual smoke

- Configure two BYOP providers (e.g., Ollama for agent, Anthropic for compaction). Send a long conversation. Confirm compaction requests hit the Anthropic endpoint while agent requests hit Ollama.
- Delete the compaction provider. Confirm the settings warning appears and compaction falls back to the agent model.

---

## Risks

1. **Context-window mismatch.** A compaction model with a much smaller context window than the primary could produce poor summaries if the head is large. Mitigation: `usable()` and `preserve_recent_budget()` use the summarizer model's window, so the head/tail split adapts to the summarizer's capacity.
2. **Silent fallback hides config errors.** A user who set an invalid compaction model gets correct behavior (primary used) but limited signal. Mitigation: inline settings warning surfaces the problem at settings-page open time.
3. **Cross-provider auth.** Cloud agent + BYOP compaction requires keys in separate stores. Mitigation: `snapshot_for_request` is self-contained per call — each resolution reads its own provider's key.
4. **Summarizer adapter compatibility.** The summarizer path (`build_summarizer_request` + `parse_summarizer_response`) is implemented per adapter. All five active adapters already have summarizer support. No new adapter work needed.
5. **Race between model deletion and in-flight compaction.** If the user deletes the compaction provider while a summarizer call is in-flight, the call uses the already-snapshotted config and completes normally. The next compaction triggers fallback. No crash.

---

## File map

**Created:**
- `app/src/ai/compaction_dispatcher.rs` — `CompactionDispatcher` struct with `resolve_target`, `dispatch_auto`, `dispatch_manual`, `compaction_model_available`.
- `app/src/ai/compaction_dispatcher_tests.rs` — sibling unit tests.

**Modified:**
- `crates/ai/src/local_provider/compaction/config.rs` — add `CompactionTarget` type.
- `crates/ai/src/local_provider/compaction/auto.rs` — `try_compact` takes `&CompactionTarget` instead of `&LocalProviderConfig`.
- `crates/ai/src/local_provider/compaction/mod.rs` — re-export `CompactionTarget`.
- `app/src/ai/local_provider_compaction.rs` — thin delegation to `CompactionDispatcher`.
- `app/src/ai/mod.rs` — add `pub mod compaction_dispatcher;`.
- `app/src/settings_view/ai_page.rs` (or wherever BYOP compaction settings render) — add `SetCompactionModel` action + dropdown + inline warning.
- `crates/ai/tests/local_provider_integration.rs` — extend or add cross-model compaction test.
- `specs/multi-local-llm/README.md` — Phase 4d status row + bullets.
- `specs/multi-local-llm/design.md` — flip §15 / §9 row.
