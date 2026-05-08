# Multi-Local-LLM — Phase 1b-3 (Settings UI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rebuild `AgentProvidersWidget` from a single-provider form (current Phase 1a-renamed shape) into a list view that lets the user add, edit, and remove multiple BYOP providers + their model rows. Minimum viable scope per `specs/multi-local-llm/design.md` §5: provider cards (name / base_url / api_key / api_type chips) + per-model rows (display name / model id / context window / tool_call toggle). The fancy openwarp features (models.dev quick-add chips, "Fetch from /models" button, expand/collapse model details, three-state multimodal capability chips, reasoning-effort UI) are deferred to Phases 4b/4c.

**Architecture:** Phase 1b-3 splits the widget out of `ai_page.rs` into its own file `app/src/settings_view/agent_providers_widget.rs` (matches openwarp's location and keeps `ai_page.rs` from growing further). The widget reads `agent_providers: Vec<AgentProvider>` from `AISettings` and the per-provider API keys from `AgentProviderSecrets`. UI mutations dispatch through a small set of new `AISettingsPageAction` variants that mutate `agent_providers` (immutable Vec) and `AgentProviderSecrets` (singleton) under `update`, then trigger `rebuild_current_page` so newly-added rows get fresh `EditorView` handles.

**Branch:** `multi-local-llm`. HEAD `5bae3032` (end of 1b-2). 19 commits ahead of `nmehta/local-llm-provider`.

**Spec references:**
- `specs/multi-local-llm/design.md` §5 (Settings UI), §8 (naming)
- openwarp source: `git show openwarp:app/src/settings_view/agent_providers_widget.rs` (1395 lines — we adopt the layout but skip Phase 4 features).

**Estimated scope:** ~5 tasks, ~5 atomic commits, ~600-800 lines net code. UI work — implementer should be ready to launch the app and visually verify after each task.

---

## File map

**Files created:**
- `app/src/settings_view/agent_providers_widget.rs` — the new list-view widget. ~500 lines target.

**Files modified:**
- `app/src/settings_view/ai_page.rs` — remove the existing inline `AgentProvidersWidget` struct + impls (lines ~6684 through end of `impl SettingsWidget for AgentProvidersWidget {}` block). Add new `AISettingsPageAction` variants for the dynamic list operations (see Task 2).
- `app/src/settings_view/mod.rs` — add `pub mod agent_providers_widget;` and re-export `AgentProvidersWidget` so `ai_page.rs:1515,1556` instantiation sites continue to compile unchanged.

**Files NOT touched in 1b-3 (deferred):**
- `app/src/ai/agent_providers/mod.rs` — existing logic stable; the widget consumes `build_byop_llm_infos` indirectly via the picker.
- `crates/ai/src/local_provider/agent_provider_secrets.rs` — public API set in 1b-2 (`get/set/remove/provider_ids`) is sufficient for the widget; no new methods needed.
- `app/src/ai/local_provider_config.rs` — picker injection logic from 1b-2 unchanged.
- Phase 4 features — explicitly out of scope.

**Out of Phase 1b-3 (tracked for Phase 4b/4c):**
- "Fetch from /models" button (4b).
- models.dev quick-add chip row (4b).
- Per-model multimodal toggles `image/pdf/audio` (4c).
- Per-model reasoning-effort UI (4c).
- Expand/collapse model row details (4c — comes with multimodal/reasoning fields).
- Catalog sync, capability inference (4b).

---

## Tasks

### Task 0: Pre-flight

- [ ] `git rev-parse --abbrev-ref HEAD` → `multi-local-llm`. `git status --short` clean apart from `.claude/scheduled_tasks.lock` + `.omc/`. `git log --oneline -1` → `5bae3032`.
- [ ] `cargo nextest run -p ai 2>&1 | tail -3` → `324 tests run: 324 passed`.
- [ ] `cargo build -p warp 2>&1 | tail -3` → `Finished`. (Sanity baseline for the UI work.)

### Task 1: Add `AISettingsPageAction` variants

**File:** `app/src/settings_view/ai_page.rs`

Find the existing `AISettingsPageAction` enum (grep for `pub enum AISettingsPageAction` — Phase 1a kept the variants `ToggleLocalProviderEnabled`, `ToggleLocalProviderSupportsTools`). Append these new variants:

```rust
    /// Add a fresh `AgentProvider` with a generated UUID and empty fields.
    /// Triggers a page rebuild so the new card gets EditorView handles.
    AddAgentProvider,

    /// Remove the provider at the given index. Also wipes its API key from
    /// `AgentProviderSecrets`.
    RemoveAgentProvider { provider_index: usize },

    /// Update the provider's display name. Fires on EditorView blur/Enter.
    UpdateAgentProviderName { provider_index: usize, name: String },

    /// Update the provider's base URL.
    UpdateAgentProviderBaseUrl { provider_index: usize, base_url: String },

    /// Update the provider's API key. Routes to `AgentProviderSecrets`,
    /// not to the settings TOML.
    UpdateAgentProviderApiKey { provider_index: usize, api_key: String },

    /// Set the provider's wire-protocol api_type (chip selector).
    UpdateAgentProviderApiType {
        provider_index: usize,
        api_type: crate::settings::AgentProviderApiType,
    },

    /// Append a fresh empty model row to the provider's models list.
    AddAgentProviderModel { provider_index: usize },

    /// Remove the model at `(provider_index, model_index)`.
    RemoveAgentProviderModel { provider_index: usize, model_index: usize },

    /// Update a single model field. Per-field actions keep the dispatch
    /// branches small; the alternative (one big `UpdateModel { field, value }`
    /// with a `ModelField` enum) trades clarity for less code.
    UpdateAgentProviderModelName {
        provider_index: usize,
        model_index: usize,
        name: String,
    },
    UpdateAgentProviderModelId {
        provider_index: usize,
        model_index: usize,
        id: String,
    },
    UpdateAgentProviderModelContextWindow {
        provider_index: usize,
        model_index: usize,
        context_window: u32,
    },
    ToggleAgentProviderModelToolCall {
        provider_index: usize,
        model_index: usize,
    },
```

Find the `AISettingsPageAction` dispatch site (a big `match action { ... }` block in the page view's update handler — grep for `AISettingsPageAction::ToggleLocalProviderEnabled`). Add handler arms for each new variant. Each handler:
1. Updates `AISettings::handle(ctx).update(ctx, |s, ctx| { ... s.agent_providers.set_value(new_vec, ctx) ... })`.
2. For `UpdateAgentProviderApiKey`, routes to `AgentProviderSecrets::handle(ctx).update(ctx, |secrets, ctx| { secrets.set(provider_id, api_key, ctx) })` instead of settings.
3. For `RemoveAgentProvider`, also calls `secrets.remove(provider_id, ctx)`.
4. For `Add*` and `Remove*`, calls `view.rebuild_current_page(ctx)` (or whatever the page-rebuild trigger is — find an existing usage in this file as the template).

**Test:** `cargo build -p warp` should succeed (the new variants are unused but enum exhaustivity is fine — no warnings). Commit:

```
feat(settings_view/ai_page): AISettingsPageAction variants for BYOP UI

Phase 1b-3 Task 1. Adds 12 new variants on AISettingsPageAction covering
the dynamic operations the rebuilt AgentProvidersWidget will dispatch:
add/remove provider, update name/base_url/api_key/api_type, add/remove
model, update model name/id/context_window, toggle tool_call. Handlers
mutate agent_providers Vec via AISettings + route api_key writes to
AgentProviderSecrets, then rebuild the page so new rows pick up fresh
EditorView handles.

The widget body that consumes these variants lands in Task 2.
```

### Task 2: Create the new widget file

**File:** `app/src/settings_view/agent_providers_widget.rs` (new)

Reference: `git show openwarp:app/src/settings_view/agent_providers_widget.rs`. Copy the layout, drop the Phase 4 features. The minimum-viable shape:

```
[Sub-header: "Custom AI Providers"            [+ Add Provider] ]
[short description: "Configure your own OpenAI-compatible LLM endpoints..."]

╭─ Provider 1 ─────────────────────────────────────────  [×] ╮
│ Name        ┃ [editor field, blur/Enter saves]              │
│ Base URL    ┃ [editor field]                                 │
│ API key     ┃ [editor field, masked]                         │
│ API type    ┃ ( OpenAI )  OpenAiResp  Gemini  Anthropic …   │
│                                                              │
│ Models                                          [+ Add Model] │
│ ┌──────────────────────────────────────────────────────────┐ │
│ │ Display name  ┃ Model ID         ┃ Ctx (tok)  ┃ Tools  ┃[×]│
│ │ [editor]      ┃ [editor]         ┃ [num]      ┃ [☑]    ┃   │
│ └──────────────────────────────────────────────────────────┘ │
╰──────────────────────────────────────────────────────────────╯

╭─ Provider 2 ─ … (collapsed not implemented; cards are always expanded)
```

Components needed (look for existing usages in `ai_page.rs` for the patterns):
- `EditorView` for text inputs (read existing `LocalProviderDisplayName` editor binding for the blur/Enter wiring).
- `EditorView` with masked-input options for API key.
- A chip selector for `AgentProviderApiType` — only `OpenAi` is enabled; the rest are visible but disabled with a tooltip "Available in Phase 3" (or just unstyled-disabled).
- A toggle switch for `tool_call`.
- Buttons `+ Add Provider`, `+ Add Model`, `[×]` (per-row delete).

State management:
- `AgentProvidersWidget` struct holds `Vec<ProviderCardHandles>` where each card has its own `EditorView` handles for name/base_url/api_key + a `Vec<ModelRowHandles>`. On `rebuild_current_page`, the widget is reconstructed from current `AISettings::as_ref(ctx).agent_providers.value()` so the handles stay synced with the data shape.
- `is_model_expanded` thread-local from openwarp's source is NOT needed for Phase 1b-3 (no expand/collapse).

**Layout primitives:** Use the `Container`, `Flex`, `Wrap`, `Text`, etc. imports from `warpui::elements` — copy import block from openwarp's source verbatim.

**Validation:** Provider with empty `base_url` or `models` shows a warning banner inside the card ("Configure base URL and at least one model to use this provider"). The picker injection (Phase 1b-2 Task 7's `build_byop_llm_infos`) already filters these out, so this is just user feedback.

Commit:

```
feat(settings_view/agent_providers_widget): multi-provider list view

Phase 1b-3 Task 2. Replaces the single-provider form (Phase 1a-renamed
LocalProviderWidget body) with a list of provider cards under a
"+ Add Provider" header. Each card has Name / Base URL / API key inputs,
an api_type chip selector (only OpenAI enabled in Phase 1b-2), and a
models table with per-row Display Name / Model ID / Context Window
inputs and a tool_call toggle.

The fancy openwarp features (models.dev quick-add, /models fetch, expand/
collapse model details, multimodal/reasoning UI) are deferred — Phase 4b
brings catalog integration; Phase 4c brings multimodal+reasoning.

Widget moved out of ai_page.rs into a sibling file matching openwarp's
layout. ai_page.rs gets ~250 lines smaller.
```

### Task 3: Remove old widget from `ai_page.rs`

**File:** `app/src/settings_view/ai_page.rs`

Remove:
- The struct `struct AgentProvidersWidget { … }` definition.
- The inherent `impl AgentProvidersWidget { … }` block.
- The `impl SettingsWidget for AgentProvidersWidget { … }` block.
- The handler arms for `ToggleLocalProviderEnabled` and `ToggleLocalProviderSupportsTools` if they're no longer reachable (the widget no longer dispatches them). Verify by searching for usages — if some flow still toggles the legacy `local_provider_enabled` setting, leave the arms in place.

Add:
```rust
use crate::settings_view::agent_providers_widget::AgentProvidersWidget;
```

at the top of `ai_page.rs` so lines 1515 and 1556 (the existing `widgets.push(Box::new(AgentProvidersWidget::new(ctx)));` sites) keep compiling.

**Test:** `cargo build -p warp 2>&1 | tail -10` must succeed. `cargo nextest run -p ai 2>&1 | tail -3` → still 324/324.

Commit:

```
refactor(settings_view/ai_page): remove inline AgentProvidersWidget

Phase 1b-3 Task 3. The widget body now lives in
agent_providers_widget.rs (Task 2). Existing instantiation sites at
ai_page.rs:1515,1556 keep compiling via a `use crate::settings_view::
agent_providers_widget::AgentProvidersWidget;` re-export.
```

### Task 4: Wire `pub mod` + final build

**File:** `app/src/settings_view/mod.rs`

Add `pub mod agent_providers_widget;` in alphabetical order with the existing pub-mod entries.

**Test:**
```
cargo build -p warp 2>&1 | tail -3                # Finished
cargo nextest run -p ai 2>&1 | tail -3            # 324/324
cargo clippy -p ai --all-targets --all-features -- -D warnings    # green
cargo clippy -p warp --lib --tests -- -D warnings                  # green
```

**Manual smoke test:**
1. `cargo run` to launch the app.
2. Open Settings → AI → Custom AI Providers.
3. Click `+ Add Provider`. Verify a new card appears with empty fields.
4. Type a name, base URL, paste an API key. Verify they persist after blur (re-open the page).
5. Click `+ Add Model` inside the card. Verify a new model row appears.
6. Type a model name + id + context window. Verify persistence.
7. Toggle the tool_call switch. Verify it sticks.
8. Click `[×]` on a model row. Verify it disappears.
9. Click `[×]` on the provider card. Verify the card AND the API key (in keychain — verify via Keychain Access) are removed.
10. Open a new conversation; verify the picker shows the migrated provider's models (post-migration the legacy single provider was migrated to a BYOP entry in 1b-2).

Commit:

```
feat(settings_view): wire agent_providers_widget module

Phase 1b-3 Task 4 — wraps up the UI rebuild. The new
AgentProvidersWidget renders the providers list and dispatches the
AISettingsPageAction variants from Task 1; ai_page.rs is ~250 lines
smaller. End of Phase 1b-3.

Manual smoke tested:
- Add/remove provider cards
- Add/remove model rows
- Edit name/base_url/api_key/context_window with persistence
- Toggle tool_call
- Verify keychain entry removed when provider deleted
- Verify picker reflects current provider list

Phase 1b-4 cleanup will follow: remove the legacy
agents.local_provider.* settings, the LocalProviderApiKey keychain
entry, and the obsolete LocalProvider* AISetting marker types.
```

### Task 5: Final verification + push

- [ ] `git log --oneline 5bae3032..HEAD` — verify 4 commits.
- [ ] `grep -rn "LocalProvider" --include="*.rs" app/src/settings_view/` — should return only the legacy AISetting marker types references that haven't been cleaned up yet (Phase 1b-4).
- [ ] `cargo clippy -p warp --lib --tests -- -D warnings` clean.
- [ ] Push: `git push origin multi-local-llm`.

---

## Risks & open questions

1. **WarpUI Element tree complexity.** The widget uses Container / Flex / Text / EditorView / SwitchStateHandle — the existing single-provider form (~250 lines in `ai_page.rs`) is the closest reference. Read it carefully before writing the new file.

2. **Dynamic list state management.** Adding a provider mid-page must trigger `rebuild_current_page` so new editor handles are minted. Find the existing `rebuild_current_page` call (e.g. for chip-add in another widget) as the template. **MouseStateHandle** must NOT be created inline during render (CLAUDE.md landmine — see WARP.md / CLAUDE.md). Hold per-card handles in the widget struct.

3. **API key masking.** EditorView has a "password mode" or similar — find an existing masked field in the codebase (e.g. an OpenAI key input under `ApiKeysWidget`) and mirror the pattern.

4. **Disabled api_type chips.** Showing all 6 variants but disabling the 5 non-OpenAI ones gives the user a forward-looking preview without inflating Phase 1b-3 scope. Tooltip wording: "Available in Phase 3" or just visually muted without a tooltip.

5. **Removing a provider while a conversation is using it.** Phase 1b-2 Task 7's picker re-injection runs on settings changes; the picker entry disappears but the conversation's stored LLMId becomes stale. Phase 1b-2 Task 6's `snapshot_for_request` returns `None` for missing provider, dispatch falls through to cloud-Warp which fails on the unrecognized model id. Acceptable transient state — the user gets prompted to re-pick. A nicer "provider deleted, pick another" inline banner is a Phase 4 polish.

6. **Settings test scaffolding.** No widget unit tests are planned for Phase 1b-3 — UI tests are integration territory and the existing `ApiKeysWidget` etc. don't have unit tests either. Manual smoke testing is the verification gate.

---

## Next plan (Phase 1b-4)

After 1b-3 ships:
- Remove legacy AISetting marker types: `LocalProviderEnabled`, `LocalProviderBaseUrl`, `LocalProviderModelId`, `LocalProviderDisplayName`, `LocalProviderContextWindow`, `LocalProviderSupportsTools`.
- Remove `agents.local_provider.*` TOML key declarations.
- Remove `LEGACY_PROVIDER_PLACEHOLDER_ID` and the V1 fallback path in `agent_provider_secrets.rs`. Switch the V2-corrupt-blob handler to `return HashMap::new()` (per the TODO comment landed at `22944c51`).
- Remove the legacy `LocalProviderApiKey` keychain entry.
- Remove the legacy `local_provider_*` widget instantiation paths (already removed in 1b-3, but verify no stragglers).
- Remove `is_local_llm_id` and the legacy `local:` LLMId branch from `local_provider_config.rs::snapshot_for_request` and the picker injection.
- Remove Task 6's legacy fallback inside `inject_local_provider_choice`.
- Remove the legacy `synthetic_llm_info` helper.

Phase 1b-4 should ONLY land after telemetry confirms the migration ran successfully on every active install. Until then, the legacy code paths are the safety net for users who skip the upgrade or whose migration hits an edge case.
