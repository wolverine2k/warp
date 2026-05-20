# Phase 4d — Dedicated Compaction Model — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Touching UI? Read `warp-ui-guidelines` first.

**Goal:** Let the user nominate a separate model for conversation compaction (summarization) so the primary agent model stays focused on agent work while a cheaper/faster model handles summarization. Adds a `CompactionDispatcher` module that owns setting resolution, fallback, and dispatch; a `CompactionTarget` type that separates overflow detection (primary model) from summarizer budget/call (compaction model); a "Summarization model" dropdown in the BYOP settings section; and an inline warning when the configured model is unavailable.

**Decisions locked in (see spec-phase-4d.md):**

| Decision | Choice |
|---|---|
| Setting shape | Existing two-field (`byop_compaction_model_provider_id` + `byop_compaction_model_id`), reconstruct LLMId at read time |
| UI placement | "Summarization model" dropdown in the BYOP compaction settings section (inside `AgentProvidersWidget`) |
| Fallback | Silent runtime fallback to conversation primary + inline settings warning when model unavailable |
| Context window | `is_overflow` uses primary model's window; `usable()` + `preserve_recent_budget()` use summarizer model's window |
| Approach | CompactionDispatcher — dedicated module owning the full lifecycle |

**Architecture:** A new `CompactionTarget { primary_cfg, summarizer_cfg }` type in `crates/ai/` carries both configs. `try_compact` is refactored to accept `&CompactionTarget` instead of `&LocalProviderConfig`. A new `CompactionDispatcher` in `app/` reads the two-field setting, resolves the compaction model via `snapshot_for_request`, builds the `CompactionTarget`, and owns the `ctx.spawn(...)` async call. The existing `local_provider_compaction.rs` functions become thin delegations.

**Tech Stack:** Rust 2021, WarpUI Entity-Component-Handle framework, `serde` / `serde_json`.

---

## Per-touchpoint reference

| Concern | Source of truth |
|---|---|
| Compaction pipeline (auto.rs) | `crates/ai/src/local_provider/compaction/auto.rs:63` — `try_compact` |
| CompactionConfig type | `crates/ai/src/local_provider/compaction/config.rs` |
| Compaction re-exports | `crates/ai/src/local_provider/compaction/mod.rs:32-36` |
| Overflow detection | `crates/ai/src/local_provider/compaction/overflow.rs` — `is_overflow`, `usable`, `ModelLimit` |
| Summarizer runner | `crates/ai/src/local_provider/run.rs:603` — `run_summarizer_turn` |
| App-side dispatch glue | `app/src/ai/local_provider_compaction.rs` — `dispatch_auto_compaction`, `dispatch_manual_compaction` |
| Dispatch call sites | `app/src/ai/blocklist/controller.rs:2949`, `app/src/ai/blocklist/controller/slash_command.rs:82` |
| Settings fields (already declared) | `app/src/settings/ai.rs:1904-1921` — `byop_compaction_model_provider_id`, `byop_compaction_model_id` |
| LLMId encode | `crates/ai/src/local_provider/llm_id.rs:24` — `pub fn encode(provider_id, model_id) -> LLMId` |
| Config snapshot | `app/src/ai/local_provider_config.rs:91` — `snapshot_for_request(ctx, &llm_id)` |
| Action enum | `app/src/settings_view/ai_page.rs:2245` — `AISettingsPageAction` |
| Providers widget | `app/src/settings_view/agent_providers_widget.rs` |
| Widget insertion point | `app/src/settings_view/ai_page.rs:1552` — `AgentProvidersWidget::new(ctx)` |

---

## File map

**Created:**
- `app/src/ai/compaction_dispatcher.rs` — `CompactionDispatcher` with `resolve_target`, `dispatch_auto`, `dispatch_manual`, `compaction_model_available`.
- `app/src/ai/compaction_dispatcher_tests.rs` — sibling unit tests.

**Modified:**
- `crates/ai/src/local_provider/compaction/config.rs` — add `CompactionTarget` type.
- `crates/ai/src/local_provider/compaction/auto.rs` — `try_compact` takes `&CompactionTarget` instead of `&LocalProviderConfig`.
- `crates/ai/src/local_provider/compaction/mod.rs` — re-export `CompactionTarget`.
- `app/src/ai/mod.rs` — add `pub mod compaction_dispatcher;`.
- `app/src/ai/local_provider_compaction.rs` — thin delegation to `CompactionDispatcher`.
- `app/src/settings_view/ai_page.rs` — add `SetCompactionModel` action variant + handler.
- `app/src/settings_view/agent_providers_widget.rs` — add "Summarization model" dropdown + inline warning.
- `crates/ai/tests/local_provider_integration.rs` — extend with cross-model compaction test.
- `specs/multi-local-llm/README.md` — Phase 4d status row + bullets.
- `specs/multi-local-llm/design.md` — flip §15 / §9 row.

---

## Stage A — Data layer (`crates/ai/`)

### Task 1: `CompactionTarget` type + `try_compact` refactor

**Files:**
- Modify: `crates/ai/src/local_provider/compaction/config.rs` — add `CompactionTarget`.
- Modify: `crates/ai/src/local_provider/compaction/auto.rs` — change `try_compact` signature.
- Modify: `crates/ai/src/local_provider/compaction/mod.rs` — re-export `CompactionTarget`.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/compaction/config.rs` (full file, ~53 lines) — current `CompactionConfig` shape.
- `crates/ai/src/local_provider/compaction/auto.rs:63-143` — current `try_compact` body.
- `crates/ai/src/local_provider/compaction/mod.rs:32-36` — re-exports.
- `crates/ai/src/local_provider/config.rs` — `LocalProviderConfig` shape (has `Clone`).

- [ ] **Step 1.1: Add `CompactionTarget` to `config.rs`**

Append after the `CompactionConfig` impl block:

```rust
/// Phase 4d. Carries both the conversation's primary model config and the
/// (potentially different) summarizer model config. Overflow detection uses
/// `primary_cfg`; budget computation and the actual summarizer call use
/// `summarizer_cfg`.
#[derive(Debug, Clone)]
pub struct CompactionTarget {
    pub primary_cfg: LocalProviderConfig,
    pub summarizer_cfg: LocalProviderConfig,
}

impl CompactionTarget {
    pub fn same_model(cfg: LocalProviderConfig) -> Self {
        Self {
            summarizer_cfg: cfg.clone(),
            primary_cfg: cfg,
        }
    }
}
```

Add the import at the top of `config.rs`:

```rust
use super::super::config::LocalProviderConfig;
```

- [ ] **Step 1.2: Re-export `CompactionTarget` from `mod.rs`**

In `crates/ai/src/local_provider/compaction/mod.rs`, add to the re-export line:

```rust
pub use config::{CompactionConfig, CompactionTarget};
```

(Replace the existing `pub use config::CompactionConfig;` line.)

- [ ] **Step 1.3: Refactor `try_compact` to accept `&CompactionTarget`**

In `auto.rs`, change the signature from:

```rust
pub async fn try_compact(
    messages: &[api::Message],
    state: &mut CompactionState,
    cfg: &LocalProviderConfig,
    compaction_cfg: &CompactionConfig,
    tokens: TokenCounts,
    manual: bool,
    http: &reqwest::Client,
) -> Result<AutoCompactionOutcome, AutoCompactionError> {
```

to:

```rust
pub async fn try_compact(
    messages: &[api::Message],
    state: &mut CompactionState,
    target: &CompactionTarget,
    compaction_cfg: &CompactionConfig,
    tokens: TokenCounts,
    manual: bool,
    http: &reqwest::Client,
) -> Result<AutoCompactionOutcome, AutoCompactionError> {
```

Update the three internal references to `cfg`:

1. Overflow detection — uses **primary** (WHEN to compact):
```rust
    let model = ModelLimit::from_context_window(target.primary_cfg.context_window.map(|n| n as usize));
    if !manual && !is_overflow(compaction_cfg, tokens, model) {
        return Ok(AutoCompactionOutcome::Skipped);
    }
```

2. Budget computation — uses **summarizer** (HOW MUCH to ingest):
```rust
    let summarizer_model = ModelLimit::from_context_window(target.summarizer_cfg.context_window.map(|n| n as usize));
    let usable_tokens = usable(compaction_cfg, summarizer_model);
    let preserve_budget = compaction_cfg.preserve_recent_budget(usable_tokens);
```

3. Summarizer call — uses **summarizer**:
```rust
    let summary = run_summarizer_turn(
        SummarizerInput {
            messages: summarizer_messages,
        },
        &target.summarizer_cfg,
        http,
    )
    .await?;
```

Add the import at the top of `auto.rs`:

```rust
use crate::local_provider::compaction::config::CompactionTarget;
```

- [ ] **Step 1.4: Update existing `try_compact` callers in `auto.rs` tests**

The two test functions in `auto.rs` (at the bottom of the file) call `try_compact` with `&cfg()`. Change each to `&CompactionTarget::same_model(cfg())`:

```rust
    // In skipped_when_auto_disabled:
    let r = try_compact(
        &messages,
        &mut state,
        &CompactionTarget::same_model(cfg()),
        &compaction_cfg,
        // ... rest unchanged
    )
    .await
    .expect("ok");
```

```rust
    // In skipped_when_below_overflow_threshold:
    let r = try_compact(
        &messages,
        &mut state,
        &CompactionTarget::same_model(large_window_cfg),
        &compaction_cfg,
        // ... rest unchanged
    )
    .await
    .expect("ok");
```

Add the import in the test module:

```rust
    use crate::local_provider::compaction::config::CompactionTarget;
```

- [ ] **Step 1.5: Add 2 unit tests on `CompactionTarget`**

Add to the existing test module in `auto.rs` (or create a sibling `config_tests.rs` if preferred — match the existing pattern):

```rust
#[test]
fn compaction_target_same_model_has_identical_configs() {
    let cfg = LocalProviderConfig {
        display_name: "Test".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "test-model".into(),
        api_key: None,
        supports_tools: true,
        context_window: Some(128_000),
        api_type: crate::local_provider::AgentProviderApiType::OpenAi,
    };
    let target = CompactionTarget::same_model(cfg.clone());
    assert_eq!(target.primary_cfg, cfg);
    assert_eq!(target.summarizer_cfg, cfg);
}

#[test]
fn compaction_target_split_has_different_configs() {
    let primary = LocalProviderConfig {
        display_name: "Primary".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "big-model".into(),
        api_key: None,
        supports_tools: true,
        context_window: Some(128_000),
        api_type: crate::local_provider::AgentProviderApiType::OpenAi,
    };
    let summarizer = LocalProviderConfig {
        display_name: "Summarizer".into(),
        base_url: "http://127.0.0.1:2/v1".into(),
        model_id: "small-model".into(),
        api_key: None,
        supports_tools: false,
        context_window: Some(32_000),
        api_type: crate::local_provider::AgentProviderApiType::Ollama,
    };
    let target = CompactionTarget {
        primary_cfg: primary.clone(),
        summarizer_cfg: summarizer.clone(),
    };
    assert_eq!(target.primary_cfg.model_id, "big-model");
    assert_eq!(target.summarizer_cfg.model_id, "small-model");
    assert_ne!(target.primary_cfg, target.summarizer_cfg);
}
```

- [ ] **Step 1.6: Build + test + clippy**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai compaction 2>&1 | tail -10
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

Expect: existing tests pass with `CompactionTarget::same_model` migration. 2 new tests on `CompactionTarget`. Some callers in `app/` will have compile errors — those are fixed in Task 2.

- [ ] **Step 1.7: Commit**

```
feat(ai/compaction): CompactionTarget type + try_compact refactor

Phase 4d task 1. Adds CompactionTarget { primary_cfg, summarizer_cfg }
to separate overflow detection (primary model's context_window) from
summarizer budget computation and dispatch (summarizer model's
context_window). try_compact now takes &CompactionTarget instead of
&LocalProviderConfig. Existing callers migrate via
CompactionTarget::same_model() — zero behavior change.

2 new unit tests on CompactionTarget construction.
```

---

## Stage B — CompactionDispatcher (`app/`)

### Task 2: `CompactionDispatcher` module + `resolve_target`

**Files:**
- Create: `app/src/ai/compaction_dispatcher.rs`
- Modify: `app/src/ai/mod.rs` — add `pub mod compaction_dispatcher;`

**Read these reference files FIRST:**
- `app/src/ai/local_provider_compaction.rs` (full file, ~283 lines) — the existing dispatch glue being replaced.
- `app/src/ai/local_provider_config.rs:91-138` — `snapshot_for_request` (the resolver).
- `app/src/settings/ai.rs:1904-1921` — the two setting fields.
- `crates/ai/src/local_provider/llm_id.rs:24` — `encode` function.

- [ ] **Step 2.1: Create `compaction_dispatcher.rs` with `resolve_target`**

```rust
use ai::local_provider::compaction::config::CompactionTarget;
use ai::local_provider::config::LocalProviderConfig;
use warpui::AppContext;

use crate::settings::AISettings;

pub struct CompactionDispatcher;

impl CompactionDispatcher {
    pub fn resolve_target(
        ctx: &AppContext,
        primary_cfg: LocalProviderConfig,
    ) -> CompactionTarget {
        let s = AISettings::as_ref(ctx);
        let provider_id = s.byop_compaction_model_provider_id.to_string();
        let model_id = s.byop_compaction_model_id.to_string();

        if provider_id.trim().is_empty() || model_id.trim().is_empty() {
            return CompactionTarget::same_model(primary_cfg);
        }

        let llm_id = ai::local_provider::llm_id::encode(&provider_id, &model_id);
        match crate::ai::local_provider_config::snapshot_for_request(ctx, &llm_id) {
            Some(summarizer_cfg) => CompactionTarget {
                primary_cfg,
                summarizer_cfg,
            },
            None => {
                log::warn!(
                    "[compaction-dispatcher] configured compaction model \
                     byop:{provider_id}:{model_id} unavailable, falling back to primary"
                );
                CompactionTarget::same_model(primary_cfg)
            }
        }
    }

    pub fn compaction_model_available(ctx: &AppContext) -> bool {
        let s = AISettings::as_ref(ctx);
        let provider_id = s.byop_compaction_model_provider_id.to_string();
        let model_id = s.byop_compaction_model_id.to_string();

        if provider_id.trim().is_empty() || model_id.trim().is_empty() {
            return true;
        }

        let llm_id = ai::local_provider::llm_id::encode(&provider_id, &model_id);
        crate::ai::local_provider_config::snapshot_for_request(ctx, &llm_id).is_some()
    }
}
```

- [ ] **Step 2.2: Wire module in `app/src/ai/mod.rs`**

Add alongside the existing `pub mod local_provider_compaction;`:

```rust
pub mod compaction_dispatcher;
```

- [ ] **Step 2.3: Build to verify `resolve_target` compiles**

```bash
cargo build -p warp 2>&1 | tail -10
```

Expect: compiles (no callers yet, but the module is wired).

- [ ] **Step 2.4: Commit**

```
feat(ai): CompactionDispatcher with resolve_target

Phase 4d task 2. Adds CompactionDispatcher::resolve_target(ctx,
primary_cfg) — reads byop_compaction_model_provider_id +
byop_compaction_model_id from AISettings, encodes to a byop: LLMId,
resolves via snapshot_for_request. Falls back to
CompactionTarget::same_model when the setting is empty or the model
is unavailable. Also adds compaction_model_available() for the
settings-UI inline warning (Task 7).
```

---

### Task 3: `dispatch_auto` + `dispatch_manual` on `CompactionDispatcher`

**Files:**
- Modify: `app/src/ai/compaction_dispatcher.rs` — add `dispatch_auto`, `dispatch_manual`.
- Modify: `app/src/ai/local_provider_compaction.rs` — delegate to `CompactionDispatcher`.

**Read these reference files FIRST:**
- `app/src/ai/local_provider_compaction.rs` (full file) — the existing dispatch bodies to move.
- `app/src/ai/compaction_dispatcher.rs` (just written in Task 2).

- [ ] **Step 3.1: Add `dispatch_auto` to `CompactionDispatcher`**

Add these imports to `compaction_dispatcher.rs`:

```rust
use ai::local_provider::compaction::{
    try_compact, AutoCompactionOutcome, CompletedCompaction, TokenCounts,
};
use warp_multi_agent_api::{self as api, response_event::stream_finished::TokenUsage};
use warpui::{ModelContext, SingletonEntity};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;
use crate::ai::blocklist::BlocklistAIController;
```

Add the method:

```rust
impl CompactionDispatcher {
    // ... existing resolve_target + compaction_model_available ...

    pub fn dispatch_auto(
        _controller: &mut BlocklistAIController,
        conversation_id: AIConversationId,
        finished_token_usage: &[TokenUsage],
        ctx: &mut ModelContext<BlocklistAIController>,
    ) {
        let Some(primary_cfg) = crate::ai::local_provider_config::snapshot_from_app(ctx) else {
            return;
        };
        let compaction_cfg = crate::ai::local_provider_config::compaction_config_from_app(ctx);
        if !compaction_cfg.auto {
            return;
        }

        let was_local_turn = finished_token_usage
            .iter()
            .any(|u| u.model_id == primary_cfg.model_id || u.model_id == "local");
        if !was_local_turn {
            return;
        }

        let tokens = aggregate_token_counts(finished_token_usage, &primary_cfg.model_id);
        let target = Self::resolve_target(ctx, primary_cfg);

        let history_model = BlocklistAIHistoryModel::handle(ctx);
        let snapshot: Option<(
            Vec<api::Message>,
            ai::local_provider::compaction::CompactionState,
        )> = {
            let history = history_model.as_ref(ctx);
            history.conversation(&conversation_id).map(|conv| {
                let messages: Vec<api::Message> = conv
                    .all_linearized_messages()
                    .iter()
                    .map(|m| (*m).clone())
                    .collect();
                (messages, conv.compaction_state().clone())
            })
        };
        let Some((messages, state_snapshot)) = snapshot else {
            return;
        };

        log::info!(
            "[compaction-dispatcher] auto: conversation={} messages={} \
             primary_model={} summarizer_model={} tokens.count={}",
            conversation_id,
            messages.len(),
            target.primary_cfg.model_id,
            target.summarizer_cfg.model_id,
            tokens.count(),
        );

        let http = reqwest::Client::new();
        ctx.spawn(
            async move {
                let mut state = state_snapshot;
                let outcome = try_compact(
                    &messages,
                    &mut state,
                    &target,
                    &compaction_cfg,
                    tokens,
                    false,
                    &http,
                )
                .await;
                outcome.map(|o| (o, state))
            },
            move |_me, result, ctx| match result {
                Ok((AutoCompactionOutcome::Compacted(_), state)) => {
                    let Some(latest) = state.completed().last().cloned() else {
                        log::warn!(
                            "[compaction-dispatcher] Compacted outcome but state.completed empty?"
                        );
                        return;
                    };
                    let history_model = BlocklistAIHistoryModel::handle(ctx);
                    history_model.update(ctx, |history_model, _ctx| {
                        let Some(conv) = history_model.conversation_mut(&conversation_id) else {
                            log::warn!(
                                "[compaction-dispatcher] conversation gone before commit: \
                                 {conversation_id}"
                            );
                            return;
                        };
                        let cc = CompletedCompaction {
                            user_msg_id: latest.user_msg_id,
                            assistant_msg_id: latest.assistant_msg_id,
                            tail_start_id: latest.tail_start_id,
                            summary_text: latest.summary_text,
                            auto: latest.auto,
                            overflow: latest.overflow,
                        };
                        conv.compaction_state_mut().push_completed(cc);
                        log::info!(
                            "[compaction-dispatcher] committed summary onto \
                             live conversation {conversation_id}"
                        );
                    });
                }
                Ok((AutoCompactionOutcome::Skipped, _)) => {}
                Err(e) => {
                    log::warn!("[compaction-dispatcher] summarizer call failed: {e}");
                }
            },
        );
    }
}
```

- [ ] **Step 3.2: Add `dispatch_manual` to `CompactionDispatcher`**

```rust
impl CompactionDispatcher {
    // ... existing methods ...

    pub fn dispatch_manual(
        _controller: &mut BlocklistAIController,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<BlocklistAIController>,
    ) -> bool {
        let Some(primary_cfg) = crate::ai::local_provider_config::snapshot_from_app(ctx) else {
            return false;
        };
        let compaction_cfg = crate::ai::local_provider_config::compaction_config_from_app(ctx);

        let history_model = BlocklistAIHistoryModel::handle(ctx);
        let snapshot: Option<(
            Vec<api::Message>,
            ai::local_provider::compaction::CompactionState,
            bool,
        )> = {
            let history = history_model.as_ref(ctx);
            history.conversation(&conversation_id).map(|conv| {
                let messages: Vec<api::Message> = conv
                    .all_linearized_messages()
                    .iter()
                    .map(|m| (*m).clone())
                    .collect();
                let was_local = conv
                    .token_usage()
                    .iter()
                    .any(|u| u.model_id == primary_cfg.model_id || u.model_id == "local");
                (messages, conv.compaction_state().clone(), was_local)
            })
        };
        let Some((messages, state_snapshot, was_local)) = snapshot else {
            return false;
        };
        if !was_local {
            return false;
        }
        if messages.is_empty() {
            log::info!(
                "[compaction-dispatcher] manual: conversation has no history; nothing to compact"
            );
            return false;
        }

        let target = Self::resolve_target(ctx, primary_cfg);

        log::info!(
            "[compaction-dispatcher] manual: conversation={} messages={} \
             primary_model={} summarizer_model={}",
            conversation_id,
            messages.len(),
            target.primary_cfg.model_id,
            target.summarizer_cfg.model_id,
        );

        let http = reqwest::Client::new();
        ctx.spawn(
            async move {
                let mut state = state_snapshot;
                let outcome = try_compact(
                    &messages,
                    &mut state,
                    &target,
                    &compaction_cfg,
                    TokenCounts::default(),
                    true,
                    &http,
                )
                .await;
                outcome.map(|o| (o, state))
            },
            move |_me, result, ctx| match result {
                Ok((AutoCompactionOutcome::Compacted(_), state)) => {
                    let Some(latest) = state.completed().last().cloned() else {
                        log::warn!(
                            "[compaction-dispatcher] manual: Compacted but state.completed empty?"
                        );
                        return;
                    };
                    let history_model = BlocklistAIHistoryModel::handle(ctx);
                    history_model.update(ctx, |history_model, _ctx| {
                        let Some(conv) = history_model.conversation_mut(&conversation_id) else {
                            log::warn!(
                                "[compaction-dispatcher] manual: conversation gone: \
                                 {conversation_id}"
                            );
                            return;
                        };
                        let cc = CompletedCompaction {
                            user_msg_id: latest.user_msg_id,
                            assistant_msg_id: latest.assistant_msg_id,
                            tail_start_id: latest.tail_start_id,
                            summary_text: latest.summary_text,
                            auto: latest.auto,
                            overflow: latest.overflow,
                        };
                        conv.compaction_state_mut().push_completed(cc);
                        log::info!(
                            "[compaction-dispatcher] manual: committed summary onto \
                             live conversation {conversation_id}"
                        );
                    });
                }
                Ok((AutoCompactionOutcome::Skipped, _)) => {}
                Err(e) => log::warn!("[compaction-dispatcher] manual: summarizer failed: {e}"),
            },
        );
        true
    }
}
```

- [ ] **Step 3.3: Add `aggregate_token_counts` as a free function**

Move the existing `aggregate_token_counts` from `local_provider_compaction.rs` into `compaction_dispatcher.rs` (or keep it in both — simpler to move it since the old module is becoming a thin wrapper):

```rust
fn aggregate_token_counts(usage: &[TokenUsage], local_model_id: &str) -> TokenCounts {
    let mut total = TokenCounts::default();
    for u in usage {
        if u.model_id != local_model_id && u.model_id != "local" {
            continue;
        }
        total.input = total.input.saturating_add(u.total_input as usize);
        total.output = total.output.saturating_add(u.output as usize);
        total.cache_read = total.cache_read.saturating_add(u.input_cache_read as usize);
        total.cache_write = total
            .cache_write
            .saturating_add(u.input_cache_write as usize);
    }
    total
}
```

- [ ] **Step 3.4: Convert `local_provider_compaction.rs` to thin delegation**

Replace the bodies of both functions in `app/src/ai/local_provider_compaction.rs`:

```rust
pub fn dispatch_auto_compaction(
    controller: &mut BlocklistAIController,
    conversation_id: AIConversationId,
    finished_token_usage: &[TokenUsage],
    ctx: &mut ModelContext<BlocklistAIController>,
) {
    crate::ai::compaction_dispatcher::CompactionDispatcher::dispatch_auto(
        controller,
        conversation_id,
        finished_token_usage,
        ctx,
    );
}

pub fn dispatch_manual_compaction(
    controller: &mut BlocklistAIController,
    conversation_id: AIConversationId,
    ctx: &mut ModelContext<BlocklistAIController>,
) -> bool {
    crate::ai::compaction_dispatcher::CompactionDispatcher::dispatch_manual(
        controller,
        conversation_id,
        ctx,
    )
}
```

Remove the old `aggregate_token_counts` from this file (it's now in `compaction_dispatcher.rs`). Remove unused imports.

- [ ] **Step 3.5: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
```

Expect: clean compile. The call sites in `controller.rs:2949` and `slash_command.rs:82` are unchanged — they still call `local_provider_compaction::dispatch_auto_compaction` / `dispatch_manual_compaction` which now delegate.

- [ ] **Step 3.6: Commit**

```
feat(ai): CompactionDispatcher dispatch_auto + dispatch_manual

Phase 4d task 3. Moves the dispatch bodies from
local_provider_compaction.rs into CompactionDispatcher, adding
resolve_target() at the config-snapshot step so a dedicated
compaction model (when configured) is used for the summarizer call.
The existing local_provider_compaction functions become thin
delegations — call sites in controller.rs and slash_command.rs are
unchanged.

Log messages now include both primary_model and summarizer_model
for debuggability.
```

---

## Stage C — Unit tests for CompactionDispatcher

### Task 4: CompactionDispatcher unit tests

**Files:**
- Create: `app/src/ai/compaction_dispatcher_tests.rs`
- Modify: `app/src/ai/compaction_dispatcher.rs` — add `#[cfg(test)]` module link.

**Read these reference files FIRST:**
- `app/src/ai/compaction_dispatcher.rs` (just written in Tasks 2-3).
- Existing test helpers: grep for `test_app\|TestAppContext\|register_singletons` in `app/src/` to find the test setup pattern for `AISettings` access.

**Note:** The `resolve_target` and `compaction_model_available` methods need an `AppContext` with `AISettings` registered. The exact test helper pattern depends on how this codebase sets up `AppContext` in tests — the implementer MUST read the existing test patterns before writing these tests. The tests below are pseudocode-level; adapt to the actual test helper.

- [ ] **Step 4.1: Wire the test module**

At the bottom of `compaction_dispatcher.rs`:

```rust
#[cfg(test)]
#[path = "compaction_dispatcher_tests.rs"]
mod tests;
```

- [ ] **Step 4.2: Write 4 unit tests**

Create `app/src/ai/compaction_dispatcher_tests.rs`:

```rust
use super::*;

// Test 1: Empty settings → same_model
// When byop_compaction_model_provider_id and byop_compaction_model_id are
// both empty (the default), resolve_target should return
// CompactionTarget::same_model.
//
// Requires: AppContext with AISettings registered (default values).
// Assert: target.primary_cfg == target.summarizer_cfg == primary_cfg arg.

// Test 2: Valid setting → split target
// When both fields are set to a valid provider+model that exists in
// agents.warp_agent.providers, resolve_target returns a CompactionTarget
// where summarizer_cfg differs from primary_cfg.
//
// Requires: AppContext with AISettings + AgentProviderSecrets registered,
// at least one provider configured in settings, byop_compaction_model_*
// fields pointing at that provider.
// Assert: target.primary_cfg.model_id == primary arg's model_id,
//         target.summarizer_cfg.model_id == configured compaction model's id.

// Test 3: Missing provider → fallback to same_model
// When the setting fields point to a non-existent provider, resolve_target
// falls back to CompactionTarget::same_model.
//
// Requires: AppContext with AISettings registered, byop_compaction_model_*
// set to "nonexistent-uuid" / "nonexistent-model".
// Assert: target.primary_cfg == target.summarizer_cfg.

// Test 4: compaction_model_available returns false for missing provider
// When the settings point to a deleted provider, the availability check
// returns false.
//
// Requires: same setup as Test 3.
// Assert: CompactionDispatcher::compaction_model_available(ctx) == false.
```

**Implementation note:** The exact test bodies depend on the AppContext test helper pattern used in this codebase. The implementer should:
1. `grep -rn "fn test_app\|TestAppBuilder\|register_singletons" app/src/` to find the pattern.
2. Mirror that setup to register `AISettings` with custom `byop_compaction_model_*` values.
3. For Test 2, also register `AgentProviderSecrets` and configure a provider.

- [ ] **Step 4.3: Build + test**

```bash
cargo nextest run -p warp compaction_dispatcher 2>&1 | tail -10
```

- [ ] **Step 4.4: Commit**

```
test(ai/compaction_dispatcher): unit tests for resolve_target + availability

Phase 4d task 4. Tests empty-settings fallback, valid-setting split,
missing-provider fallback, and compaction_model_available for the
deleted-provider case.
```

---

## Stage D — Settings UI

### Task 5: `SetCompactionModel` action variant + handler

**Files:**
- Modify: `app/src/settings_view/ai_page.rs` — add `SetCompactionModel` variant to `AISettingsPageAction` + handler.

**Read these reference files FIRST:**
- `app/src/settings_view/ai_page.rs:2245-2410` — `AISettingsPageAction` enum.
- `app/src/settings_view/ai_page.rs:3299+` — action handler `match` arms for BYOP actions.

- [ ] **Step 5.1: Add the action variant**

In the `AISettingsPageAction` enum, after the BYOP multi-provider actions section (around line 2410), add:

```rust
    // ----- Phase 4d: Dedicated compaction model -----
    SetCompactionModel {
        provider_id: String,
        model_id: String,
    },
```

- [ ] **Step 5.2: Add the handler arm**

In the `handle_action` match block (after the BYOP action handlers), add:

```rust
AISettingsPageAction::SetCompactionModel {
    provider_id,
    model_id,
} => {
    let settings = AISettings::handle(ctx);
    settings.update(ctx, |settings, ctx| {
        settings
            .byop_compaction_model_provider_id
            .set(provider_id.clone(), ctx);
        settings
            .byop_compaction_model_id
            .set(model_id.clone(), ctx);
    });
}
```

- [ ] **Step 5.3: Add 2 unit tests for the action handler**

These tests verify the settings are written correctly. Place them in the appropriate test file for `AISettingsPageAction` handlers (grep for existing action-handler tests in `ai_page.rs` or a sibling test module):

1. `set_compaction_model_action_writes_both_fields` — dispatch `SetCompactionModel { provider_id: "abc", model_id: "small-model" }`, read back both settings, assert they match.
2. `set_compaction_model_empty_clears_to_default` — dispatch `SetCompactionModel { provider_id: "", model_id: "" }`, read back both settings, assert they're empty strings.

The implementer must follow the existing action-handler test pattern in this file.

- [ ] **Step 5.4: Build + test**

```bash
cargo build -p warp 2>&1 | tail -10
cargo nextest run -p warp set_compaction_model 2>&1 | tail -10
```

- [ ] **Step 5.5: Commit**

```
feat(settings): SetCompactionModel action + handler + tests

Phase 4d task 5. Adds AISettingsPageAction::SetCompactionModel {
provider_id, model_id } that writes both byop_compaction_model_*
settings. Empty strings clear to default (use conversation model).
2 unit tests verify write + clear paths.
```

---

### Task 6: "Summarization model" dropdown in AgentProvidersWidget

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs` — add the dropdown + inline warning.

**Read these reference files FIRST:**
- `app/src/settings_view/agent_providers_widget.rs` — full file. Understand the render structure, the `Dropdown<AISettingsPageAction>` pattern from existing dropdowns, and where to add the new one.
- `app/src/settings_view/ai_page.rs:476-477` — existing `base_model_dropdown` / `coding_model_dropdown` as pattern reference for `Dropdown<AISettingsPageAction>` usage.
- `warp-ui-guidelines` skill — read before writing any view code.

- [ ] **Step 6.1: Add the dropdown ViewHandle field**

On `AgentProvidersWidget` (or `AISettingsPageView` if that's where dropdowns are centralized), add:

```rust
compaction_model_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
```

Initialize in the constructor by building the entries list:

```rust
fn build_compaction_model_entries(ctx: &AppContext) -> Vec<DropdownEntry<AISettingsPageAction>> {
    let mut entries = vec![DropdownEntry {
        label: "Use conversation model".into(),
        action: AISettingsPageAction::SetCompactionModel {
            provider_id: String::new(),
            model_id: String::new(),
        },
    }];

    let s = AISettings::as_ref(ctx);
    let providers: Vec<AgentProvider> = s.agent_providers.value().clone();
    for provider in &providers {
        for model in &provider.models {
            let label = if provider.name.is_empty() {
                model.name.clone()
            } else {
                format!("{} / {}", provider.name, model.name)
            };
            entries.push(DropdownEntry {
                label,
                action: AISettingsPageAction::SetCompactionModel {
                    provider_id: provider.id.clone(),
                    model_id: model.id.clone(),
                },
            });
        }
    }
    entries
}
```

**Note:** The exact `DropdownEntry` type and `Dropdown::new(...)` constructor may differ — the implementer MUST read the existing dropdown patterns in this file and match them exactly.

- [ ] **Step 6.2: Render the dropdown in the providers widget**

After the existing compaction settings (or at the bottom of the providers widget section), render the dropdown with a label "Summarization model". Only show when `FeatureFlag::LocalLlmProvider.is_enabled()` and at least one provider is configured.

- [ ] **Step 6.3: Add inline warning**

Below the dropdown, conditionally render a dim warning label:

```rust
let s = AISettings::as_ref(ctx);
let has_configured_model = !s.byop_compaction_model_provider_id.to_string().trim().is_empty();
if has_configured_model
    && !crate::ai::compaction_dispatcher::CompactionDispatcher::compaction_model_available(ctx)
{
    // Render dim warning label:
    // "Configured summarization model is unavailable — compaction will use the conversation model."
}
```

- [ ] **Step 6.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 6.5: Commit**

```
feat(settings): "Summarization model" dropdown in BYOP compaction section

Phase 4d task 6. Adds a dropdown to the AgentProvidersWidget that
lists all configured BYOP provider×model pairs plus a "Use
conversation model" default. Dispatches SetCompactionModel to write
both byop_compaction_model_* settings. Shows an inline warning when
the configured model is unavailable.
```

---

## Stage E — Integration test

### Task 7: Cross-model compaction integration test

**Files:**
- Modify: `crates/ai/tests/local_provider_integration.rs` — add a new test.

**Read these reference files FIRST:**
- `crates/ai/tests/local_provider_integration.rs` — full file. Find the existing `auto_compaction_round_trip` test and understand the mock-server setup pattern.
- `crates/ai/src/local_provider/compaction/auto.rs:63-143` — the `try_compact` function being tested.

- [ ] **Step 7.1: Add `cross_model_compaction_routes_summarizer_to_separate_server`**

Boot two mock HTTP servers (agent on port A, summarizer on port B). Build a `CompactionTarget` with `primary_cfg` pointing at server A and `summarizer_cfg` pointing at server B. Trigger `try_compact` with a conversation that overflows. Assert:
- The summarizer request was received by server B (not A).
- Server A received zero requests.
- The resulting `CompletedCompaction` has a valid summary.

Follow the exact mock-server setup pattern from the existing `auto_compaction_round_trip` test.

- [ ] **Step 7.2: Run the test**

```bash
cargo nextest run -p ai cross_model_compaction 2>&1 | tail -10
```

- [ ] **Step 7.3: Commit**

```
test(ai/integration): cross-model compaction routes summarizer to separate server

Phase 4d task 7. Boots two mock HTTP servers — one for the agent
model, one for the compaction model. Triggers try_compact via a
CompactionTarget with split configs. Asserts the summarizer request
hit the compaction server, not the agent server.
```

---

## Stage F — Docs

### Task 8: Spec docs + status flip

**Files:**
- Modify: `specs/multi-local-llm/README.md` — append Phase 4d status paragraph, table row, user-visible bullet, architecture bullet.
- Modify: `specs/multi-local-llm/design.md` — flip §9 row + §15 status.

- [ ] **Step 8.1: Update README.md**

Status paragraph:

```markdown
**Phase 4d (dedicated compaction model)** code is complete on `multi-local-llm` (final commit `<TBD>`). Adds a `CompactionDispatcher` module that owns setting resolution, fallback, and dispatch for the compaction pipeline. A new `CompactionTarget { primary_cfg, summarizer_cfg }` type separates overflow detection (primary model's context_window) from summarizer budget and dispatch (compaction model's context_window). The "Summarization model" dropdown in the BYOP compaction settings section shows all configured provider×model pairs plus a "Use conversation model" default. When the configured model is unavailable, dispatch silently falls back to the conversation's primary model, and the settings page shows an inline warning. ~11 new unit tests across CompactionTarget (2), CompactionDispatcher (4), settings action (2), dropdown (2), integration (1).

> **Verification gate:** manual smoke with two BYOP providers — agent on provider A, compaction on provider B. Confirm summarizer requests hit provider B. Delete provider B, confirm settings warning and fallback to provider A.
```

Status table row:

```markdown
| 4d — Dedicated compaction model | [`plan-phase-4d.md`](plan-phase-4d.md) | 🧪 code complete — pending live smoke |
```

User-visible bullet:

```markdown
- **Phase 4d (dedicated compaction model):** New "Summarization model" dropdown in Settings → AI (BYOP section) lets users route conversation compaction to a cheaper/faster model while the primary agent model handles agent work. Falls back to the conversation model when the configured compaction model is unavailable.
```

Architecture bullet:

```markdown
- **Phase 4d:** New `CompactionTarget { primary_cfg, summarizer_cfg }` in `crates/ai/src/local_provider/compaction/config.rs` separates overflow detection from summarizer dispatch. New `CompactionDispatcher` at `app/src/ai/compaction_dispatcher.rs` reads `byop_compaction_model_provider_id` + `byop_compaction_model_id` from AISettings, resolves via `snapshot_for_request`, and builds the target. Existing `local_provider_compaction.rs` functions delegate to the dispatcher. Settings UI gains a "Summarization model" dropdown + inline availability warning in the BYOP compaction section.
```

- [ ] **Step 8.2: Update design.md §9 row**

Change the Phase 4d row from "dedicated compaction model — future" to "🧪 code complete — pending live smoke".

Update §15 header to note implementation is complete.

- [ ] **Step 8.3: Commit**

```
docs(specs/multi-local-llm): record Phase 4d code-complete status
```

---

## Final verification

- [ ] **Verification 1: Backward compat** — when `byop_compaction_model_provider_id` and `byop_compaction_model_id` are both empty (the default), behavior is identical to pre-4d: `resolve_target` returns `CompactionTarget::same_model`, `try_compact` uses the same config for overflow + summarizer, and existing tests pass unchanged.
- [ ] **Verification 2: Build + tests + clippy** — `cargo build -p ai && cargo build -p warp` clean. `cargo nextest run -p ai compaction` and `cargo nextest run -p warp compaction_dispatcher` show new tests passing. `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` clean.
- [ ] **Verification 3: Manual smoke** — two BYOP providers configured (one agent, one compaction). Verify summarizer traffic hits the compaction provider. Delete the compaction provider, verify settings warning and fallback.

---

## Risks & mitigations

1. **Test helper availability.** `AppContext`-based tests in `app/` may require specific test helper patterns for registering `AISettings` + `AgentProviderSecrets`. The implementer must grep for existing patterns before writing Task 4 tests. If no suitable helper exists, the `resolve_target` logic can be tested by extracting the pure logic into a helper function that takes the setting values directly (no `AppContext`).
2. **Dropdown pattern mismatch.** The `Dropdown<AISettingsPageAction>` constructor may differ from what's shown. The implementer must read the existing dropdown patterns in `agent_providers_widget.rs` and `ai_page.rs` exactly.
3. **`snapshot_from_app` vs `snapshot_for_request`.** The auto dispatch uses `snapshot_from_app` to get the primary config (same as before), then `resolve_target` uses `snapshot_for_request` for the compaction model. If `snapshot_from_app` returns `None` (local provider not configured), dispatch short-circuits — this is correct since there's nothing to compact.
