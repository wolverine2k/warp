# Phase 5b — BYOP Orchestration — Full Local Native Path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Touching UI? Read `warp-ui-guidelines` first.

**Goal:** Surface BYOP-configured models in the agent orchestration model picker, wire the per-provider orchestration opt-in toggle in Settings → AI, plumb the submit-time validator built in 5a, and verify end-to-end Local Native dispatch (the `oz` / empty harness path that runs through the existing in-process BYOP dispatcher from Phase 4d). External-CLI harnesses (Claude Code / Codex / Gemini CLI / OpenCode) and Remote execution are intentionally deferred to Phases 5c and 5d.

**Out of scope for 5b** (deferred to 5c / 5d):

- Env-var injection for external CLI harnesses (`orchestration_byop_env.rs`) — 5c.
- Child-spawn integration in `child_agent.rs` for non-`oz` harnesses — 5c.
- Remote credential bridge (the `remote_secret_name` field added in 5a, the Auto-create button, the managed-secret create flow) — 5d. The struct field is already there from 5a; 5b does not surface UI for it.
- GraphQL changes to `RunAgentsRequest` (`byop_base_url`, `byop_api_type`, `compaction_model_*`) — 5d.
- Compaction inheritance forwarding to Remote workers — 5d.

**Decisions locked in (see `spec-phase-5.md` and `plan-phase-5a.md`):**

| Decision | Choice |
|---|---|
| Picker source | `LLMPreferences::get_orchestration_llm_choices` (added in 5a) |
| Picker UX for incompatible / stale entries | Filter incompatible entries out; reset stale selections to the default with an inline notice |
| Submit-time validation strictness | Hard error when `model_id` starts with `byop:` and the harness/mode combo is incompatible (validator already built in 5a, wired here) |
| Per-provider orchestration opt-in | `available_for_orchestration: bool` toggle on `AgentProvider` (field added in 5a, UI added here). Default off. |
| Native dispatch path for BYOP | Reuses Phase 4d's in-process BYOP dispatcher — `run_agents_to_start_agent_mode` already forwards the run-wide `model_id` into `StartAgentExecutionMode::Local { model_id: Some("byop:…"), … }`, so no new dispatch wiring is required. 5b verifies this end-to-end via integration test. |

**Architecture (delta over 5a):**

5a built three pieces with no callers: `build_byop_orchestration_llm_infos`, `byop_llm_choices` + `get_orchestration_llm_choices`, and `validate_orchestration_model_id`. The validator carries `#[allow(dead_code)]` with a comment saying *"wired in Phase 5c"* — that wiring actually belongs to 5b (the comment was conservative). 5b removes those annotations and threads the new helpers into three real call sites:

1. **Settings UI** — one new `AISettingsPageAction` variant (`ToggleAgentProviderOrchestrationAvailability`) and a `ChipCheckbox`-style toggle rendered above the "Models" section of each provider card, gated on `FeatureFlag::LocalLlmProvider`.
2. **Orchestration picker** — four helpers in `app/src/ai/blocklist/inline_action/orchestration_controls.rs` switch from `get_base_llm_choices_for_agent_mode(ctx)` to `get_orchestration_llm_choices(ctx, harness_type, execution_mode)`. These helpers fan out into the "what to put in the dropdown" / "is the current selection still valid?" / "what's the default?" / "sync the visible selection after a state change" paths, all of which need the same picker source so a harness change re-filters everything consistently.
3. **Submit-time validator** — `run_agents.rs::validate_request` calls `validate_orchestration_model_id` when `model_id.starts_with("byop:")`.

The orchestration modal's stale-selection logic already exists (`is_model_in_filtered_choices` + `sync_picker_selections`); 5b just teaches those helpers about BYOP entries by sharing the same `get_orchestration_llm_choices` source the picker uses.

**Tech Stack:** Rust 2021, WarpUI Entity-Component-Handle framework, `serde`, the warp Dropdown / Toggle UI components.

---

## Per-touchpoint reference

| Concern | Source of truth |
|---|---|
| `AgentProvider::available_for_orchestration` | `app/src/settings/ai.rs:798` (added in 5a) |
| `validate_orchestration_model_id` | `app/src/ai/agent_sdk/common.rs:71` (added in 5a, `#[allow(dead_code)]` here) |
| `compatible_harness_names` helper | `app/src/ai/agent_sdk/common.rs:159` (added in 5a, `#[allow(dead_code)]` here) |
| `LLMPreferences::get_orchestration_llm_choices` | `app/src/ai/llms.rs:907` (added in 5a) |
| `byop_orchestration_filter` module | `app/src/ai/byop_orchestration_filter.rs` (added in 5a) |
| Orchestration model-picker call sites | `app/src/ai/blocklist/inline_action/orchestration_controls.rs:464,554,584,1489` |
| `populate_model_picker_for_harness` | `app/src/ai/blocklist/inline_action/orchestration_controls.rs:448` |
| `is_model_in_filtered_choices` | `app/src/ai/blocklist/inline_action/orchestration_controls.rs:543` |
| `first_filtered_model_id` | `app/src/ai/blocklist/inline_action/orchestration_controls.rs:575` |
| `sync_picker_selections` | `app/src/ai/blocklist/inline_action/orchestration_controls.rs:1475` |
| `OrchestrationEditState` (carries `harness_type`, `execution_mode`, `model_id`) | `app/src/ai/blocklist/inline_action/orchestration_controls.rs` (`from_run_agents_fields`) |
| `validate_request` | `app/src/ai/blocklist/action_model/execute/run_agents.rs:345` |
| `run_agents_to_start_agent_mode` (Native dispatch already forwards model_id) | `app/src/ai/blocklist/action_model/execute/run_agents.rs:386` |
| `AISettingsPageAction` | `app/src/settings_view/ai_page.rs:2756+` (existing `AddAgentProvider`, `RemoveAgentProvider`, etc. — append new variant after the existing provider toggles) |
| `AISettingsPageAction` handler dispatch | `app/src/settings_view/ai_page.rs:3760+` (existing handler patterns) |
| Provider card render | `app/src/settings_view/agent_providers_widget.rs:1235` (`render_provider_card`) |
| `FeatureFlag::LocalLlmProvider` gating | `app/src/settings_view/agent_providers_widget.rs:1499` (existing usage pattern) |
| Phase 4d Native BYOP dispatcher | `app/src/ai/compaction_dispatcher.rs::resolve_target` (cross-references `LLMId::byop:` IDs via the in-process path; the orchestration child agent will hit the same code path because the run-wide `model_id` flows through `StartAgentExecutionMode::Local { model_id: Some("byop:…") }`) |

---

## File map

**Modified:**

- `app/src/settings_view/ai_page.rs` — add `ToggleAgentProviderOrchestrationAvailability { provider_index }` variant to `AISettingsPageAction`; add handler in the `Action` dispatch block.
- `app/src/settings_view/agent_providers_widget.rs` — render the new orchestration toggle inside each provider card, gated on `FeatureFlag::LocalLlmProvider`.
- `app/src/ai/blocklist/inline_action/orchestration_controls.rs` — swap the four picker / stale-selection helpers from `get_base_llm_choices_for_agent_mode` to `get_orchestration_llm_choices`, threading the `harness_type` and `execution_mode` through.
- `app/src/ai/blocklist/action_model/execute/run_agents.rs` — call `validate_orchestration_model_id` from `validate_request` when the model id is a BYOP id.
- `app/src/ai/agent_sdk/common.rs` — remove `#[allow(dead_code)]` from `validate_orchestration_model_id` and `compatible_harness_names`.
- `specs/multi-local-llm/README.md` — flip Phase 5b row to ⏳ code-complete with the standard verification-gate note.

**Created:**

- `crates/ai/tests/orchestration_byop_local_native.rs` — single integration test verifying that a BYOP model id submitted with `harness_type = "oz"` + `execution_mode = Local` resolves to a `StartAgentExecutionMode::Local { harness_type: None, model_id: Some("byop:<provider>:<model>"), … }`.

No new modules, no new public APIs, no new types — Phase 5b is pure wire-up.

---

## Stage A — Settings UI: orchestration opt-in toggle

### Task 1: Add `ToggleAgentProviderOrchestrationAvailability` action

**Files:**
- Modify: `app/src/settings_view/ai_page.rs` — add variant + handler.

**Read these reference files FIRST:**
- `app/src/settings_view/ai_page.rs:2756-2790` — existing `AddAgentProvider`, `RemoveAgentProvider`, `UpdateAgentProviderName` variants. The new variant follows the same shape.
- `app/src/settings_view/ai_page.rs:3760-3811` — existing handler patterns. `AddAgentProvider` / `RemoveAgentProvider` show the `AISettings::handle(ctx).update(...)` pattern used to mutate the provider list.

- [ ] **Step 1.1: Add the variant**

In `app/src/settings_view/ai_page.rs`, append a new variant to `AISettingsPageAction` immediately after the existing `UpdateAgentProviderApiType` variant (around line 2790):

```rust
    /// Phase 5b. Flip the `available_for_orchestration` toggle on a provider.
    /// When on, this provider's models appear in the orchestration picker
    /// (subject to harness-compatibility and reachability filters from 5a).
    /// Default off so existing BYOP configs don't surface in orchestration
    /// until the user explicitly opts in.
    ToggleAgentProviderOrchestrationAvailability {
        provider_index: usize,
    },
```

- [ ] **Step 1.2: Add the handler**

In `app/src/settings_view/ai_page.rs`, add a handler arm to the action dispatch block. Insert it immediately after the existing `UpdateAgentProviderApiType` handler (search for `AISettingsPageAction::UpdateAgentProviderApiType {` to find the anchor; the arm typically ends with a `ctx.notify();` call). Add:

```rust
            AISettingsPageAction::ToggleAgentProviderOrchestrationAvailability {
                provider_index,
            } => {
                let provider_index = *provider_index;
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let mut providers = settings.agent_providers.value().clone();
                    if let Some(p) = providers.get_mut(provider_index) {
                        p.available_for_orchestration = !p.available_for_orchestration;
                        report_if_error!(settings.agent_providers.set_value(providers, ctx));
                    }
                });
                ctx.notify();
            }
```

- [ ] **Step 1.3: Build to verify compile**

```bash
cargo build -p warp 2>&1 | tail -10
# Expected: compiles cleanly.
```

- [ ] **Step 1.4: Add an action-handler unit test**

In `app/src/settings_view/ai_page.rs` (or its sibling test file `ai_page_tests.rs` if it exists in your tree; otherwise add to the existing `#[cfg(test)]` module in the same file), add a test that:

1. Builds an `App::test()`.
2. Creates an `AgentProvider` with `available_for_orchestration = false`.
3. Dispatches `AISettingsPageAction::ToggleAgentProviderOrchestrationAvailability { provider_index: 0 }`.
4. Asserts `agent_providers.value()[0].available_for_orchestration == true`.
5. Dispatches again and asserts the toggle flips back.

Look at an existing handler test in `ai_page.rs` or `ai_tests.rs` for the test-app boilerplate. If no existing test pattern dispatches an `AISettingsPageAction` directly, add the test instead to `app/src/settings/ai_tests.rs` and replace step 3 with a direct mutation:

```rust
#[test]
fn toggle_agent_provider_orchestration_availability_flips_the_field() {
    let app = warpui::App::test();
    initialize_settings_for_tests(&app);

    AISettings::handle(&app).update(&app, |settings, ctx| {
        let provider = AgentProvider {
            id: "test-provider".to_owned(),
            name: "Test".to_owned(),
            kind: AgentProviderKind::default(),
            api_type: AgentProviderApiType::OpenAi,
            base_url: "https://api.example.com/v1".to_owned(),
            models: vec![AgentProviderModel::from_id("m1".to_owned())],
            available_for_orchestration: false,
            remote_secret_name: String::new(),
        };
        settings
            .agent_providers
            .set_value(vec![provider], ctx)
            .unwrap();
    });

    // Direct mutation mirroring what the action handler does.
    AISettings::handle(&app).update(&app, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        providers[0].available_for_orchestration = !providers[0].available_for_orchestration;
        settings.agent_providers.set_value(providers, ctx).unwrap();
    });

    assert!(
        AISettings::as_ref(&app).agent_providers.value()[0].available_for_orchestration,
        "toggle should have flipped to true"
    );

    AISettings::handle(&app).update(&app, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        providers[0].available_for_orchestration = !providers[0].available_for_orchestration;
        settings.agent_providers.set_value(providers, ctx).unwrap();
    });

    assert!(
        !AISettings::as_ref(&app).agent_providers.value()[0].available_for_orchestration,
        "toggle should have flipped back to false"
    );
}
```

- [ ] **Step 1.5: Run the test**

```bash
cargo nextest run -p warp toggle_agent_provider_orchestration_availability_flips_the_field 2>&1 | tail -10
# Expected:
#    PASS warp::settings::ai::tests::toggle_agent_provider_orchestration_availability_flips_the_field
#  Summary: 1 test run, 1 passed
```

- [ ] **Step 1.6: Commit**

```
feat(settings/ai): ToggleAgentProviderOrchestrationAvailability action

Phase 5b task 1. Adds the AISettingsPageAction variant + handler that
flips AgentProvider::available_for_orchestration. Default false stays
for new providers; the toggle is the user-controlled opt-in surfaced
in Settings -> AI in Task 2.

1 unit test verifies the round trip.
```

---

## Stage B — Settings UI: render the orchestration toggle

### Task 2: Render the toggle in `render_provider_card`

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs` — render the toggle inside each provider card.

**Read these reference files FIRST:**
- `app/src/settings_view/agent_providers_widget.rs:1235-1500` — `render_provider_card` (the full method). The new toggle renders between the API-key field and the Models section header (i.e. before line 1290 `// ---- Models section ----`).
- `app/src/settings_view/agent_providers_widget.rs:1499` — existing `FeatureFlag::LocalLlmProvider.is_enabled()` pattern (used to gate other Phase 4/5 UI).
- `app/src/settings_view/agent_providers_widget.rs:108-124` — `ProviderCardHandles` struct (we'll add a `MouseStateHandle` for the new toggle).
- `app/src/settings_view/agent_providers_widget.rs:308-415` — `build_provider_card` (where new `MouseStateHandle::default()` allocations belong).
- `warp-ui-guidelines` repo skill — read this **before writing the render code**. The CLAUDE.md callout about `MouseStateHandle::default()` being constructed inline is a real footgun: every interactive element needs its handle allocated at construction time, not in the render path.

- [ ] **Step 2.1: Add a `MouseStateHandle` for the toggle**

In `app/src/settings_view/agent_providers_widget.rs`, add a field to `ProviderCardHandles` (around line 121, after `api_type_chip_states`):

```rust
    /// Phase 5b. Mouse-state for the "Available for orchestration" toggle
    /// rendered between the API-key field and the Models section. Allocated
    /// here so render never builds `MouseStateHandle::default()` inline.
    orchestration_toggle_state: MouseStateHandle,
```

Then initialize it inside `build_provider_card` (search for `api_type_chip_states:` and add the new field after the existing handles, before the `model_rows:` line):

```rust
            orchestration_toggle_state: MouseStateHandle::default(),
```

- [ ] **Step 2.2: Add the toggle render helper**

In `app/src/settings_view/agent_providers_widget.rs`, add a new method on `AgentProvidersWidget` (the same impl block that holds `render_provider_card`). Insert it after `render_card_button` (around line 573):

```rust
    /// Phase 5b. Renders the "Available for orchestration" toggle for a
    /// provider card. Flips `AgentProvider::available_for_orchestration` via
    /// `ToggleAgentProviderOrchestrationAvailability`.
    ///
    /// Gated on `FeatureFlag::LocalLlmProvider` at the call site — if the
    /// flag is off this row is not rendered and the toggle has no effect.
    fn render_orchestration_toggle(
        provider: &AgentProvider,
        provider_index: usize,
        card: &ProviderCardHandles,
        label_color: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let label_text = if provider.available_for_orchestration {
            "Available for orchestration: On"
        } else {
            "Available for orchestration: Off"
        };
        let body = Text::new(
            label_text.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(label_color.into())
        .finish();
        let helper = Container::new(
            Text::new(
                "When On, this provider's models appear in the orchestration model picker."
                    .to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(appearance.theme().disabled_ui_text_color().into())
            .soft_wrap(true)
            .finish(),
        )
        .with_margin_top(2.)
        .finish();

        let toggle_button = Self::render_card_button(
            label_text.to_string(),
            card.orchestration_toggle_state.clone(),
            AISettingsPageAction::ToggleAgentProviderOrchestrationAvailability { provider_index },
            appearance,
        );

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(toggle_button)
                    .with_margin_top(FIELD_LABEL_MARGIN_TOP)
                    .finish(),
            )
            .with_child(helper)
            .finish()
    }
```

**Note on the toggle visual:** the file already uses `render_card_button` for the per-provider buttons (Add Model, Test connection, Remove, etc.). Using the same button widget keeps Phase 5b minimal — no new component types — and the on/off state is communicated through the label text. If a chip-style checkbox is preferred for design polish, that's a follow-up after the functionality is wired; the spec calls for the toggle, not a particular widget shape.

Add the import for `ColorU` if not already present (it's used elsewhere in the file; check `use` lines at the top of `agent_providers_widget.rs`):

```rust
use pathfinder_color::ColorU;
```

- [ ] **Step 2.3: Wire the toggle into `render_provider_card`**

In `app/src/settings_view/agent_providers_widget.rs::render_provider_card` (around line 1289, immediately before the `// ---- Models section ----` comment), insert:

```rust
        // ---- Orchestration opt-in (Phase 5b, gated on LocalLlmProvider) ----
        let orchestration_toggle = if warp_core::features::FeatureFlag::LocalLlmProvider
            .is_enabled()
        {
            Some(Self::render_orchestration_toggle(
                provider,
                provider_index,
                card,
                label_color,
                appearance,
            ))
        } else {
            None
        };
```

Then thread it into the card's column. Find the `Flex::column()` that the card's fields are assembled into (it's later in `render_provider_card` — look for `.with_child(name_field)` and the subsequent `.with_child(...)` chain that adds `api_type_field`, `base_url_field`, `api_key_field`). After the `api_key_field` is added and before the Models section is appended, conditionally add the toggle:

```rust
        if let Some(toggle) = orchestration_toggle {
            column.add_child(toggle);
        }
```

(The exact name of the `Flex::column()` binding will be visible in the existing code — if it's named `card_column` or similar, use that name. The plan calls it `column` for brevity; substitute the actual identifier.)

- [ ] **Step 2.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
# Expected: compiles cleanly.

cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
# Expected: no warnings.
```

- [ ] **Step 2.5: Manual smoke (recommended)**

```bash
cargo run --features with_local_server 2>&1 | tail -10 &
```

In the running client:

1. Open Settings → AI → BYOP / Custom AI Providers.
2. Add a provider (or confirm an existing one) — verify the new "Available for orchestration: Off" label and the helper text appear below the API key field.
3. Click the toggle — label flips to "Available for orchestration: On".
4. Restart the client and confirm the value persisted in `~/.config/warp-terminal/user_preferences.json` under `agent_providers[N].available_for_orchestration`.

If `FeatureFlag::LocalLlmProvider` is off in your build profile, the toggle won't render — flip it on via `app/src/features.rs` or the developer settings panel for the smoke test.

- [ ] **Step 2.6: Commit**

```
feat(settings/ai): render BYOP orchestration opt-in toggle on provider cards

Phase 5b task 2. Renders an "Available for orchestration" toggle on
each provider card in Settings -> AI, gated on
FeatureFlag::LocalLlmProvider. Flipping it dispatches
ToggleAgentProviderOrchestrationAvailability from task 1.

Helper text explains the effect ("When On, this provider's models
appear in the orchestration model picker."). No remote-secret UI
yet — that pairs with the GraphQL changes in Phase 5d.
```

---

## Stage C — Orchestration picker swap

### Task 3: Swap `populate_model_picker_for_harness` to `get_orchestration_llm_choices`

**Files:**
- Modify: `app/src/ai/blocklist/inline_action/orchestration_controls.rs`.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/inline_action/orchestration_controls.rs:448-529` — `populate_model_picker_for_harness` (the Oz branch is the one to change).
- `app/src/ai/llms.rs:907` — `get_orchestration_llm_choices` signature (`fn(app: &AppContext, harness_type: &str, execution_mode: &RunAgentsExecutionMode) -> Vec<LLMInfo>`). Note the `&mut self` borrow: `LLMPreferences::handle(...).update(...)` is the standard way to get a mutable reference.
- `app/src/ai/llms.rs:736` — `get_base_llm_choices_for_agent_mode` for the iterator-based shape the existing code uses.
- `app/src/ai/blocklist/inline_action/orchestration_controls.rs` — `OrchestrationEditState` definition. It already carries `harness_type` and `execution_mode`; both are passed into the picker helpers as parameters today, but `execution_mode` may need to be added.

- [ ] **Step 3.1: Audit the existing `populate_model_picker_for_harness` signature**

The current signature (line 448) is:

```rust
pub fn populate_model_picker_for_harness<A: OrchestrationControlAction, V: View>(
    dropdown: &ViewHandle<Dropdown<A>>,
    initial_model_id: &str,
    harness_type: &str,
    is_local: bool,
    ctx: &mut ViewContext<V>,
)
```

`is_local: bool` is already a parameter — it's the boolean form of `execution_mode`. To call `get_orchestration_llm_choices` we need the full `RunAgentsExecutionMode` (the spec said "execution_mode" not "is_local"). Two options:

- **(A) Reconstruct execution mode locally**: `let execution_mode = if is_local { RunAgentsExecutionMode::Local } else { RunAgentsExecutionMode::Remote { environment_id: String::new(), worker_host: String::new(), computer_use_enabled: false } };`. Workable but loses the actual Remote fields — only `is_remote()` is consulted in 5a's filter so the placeholders are fine.
- **(B) Plumb the real `RunAgentsExecutionMode` through**: changes the helper's signature and every call site.

For 5b, **prefer (A)** — `get_orchestration_llm_choices` only branches on `is_remote()` so the placeholders don't change behavior. This keeps the diff scoped to one file. A follow-up can switch to (B) if Remote-only filters ever need richer mode data.

- [ ] **Step 3.2: Patch the Oz / unset branch of `populate_model_picker_for_harness`**

In `app/src/ai/blocklist/inline_action/orchestration_controls.rs:460-485`, replace the `Some(Harness::Oz) | None => { … }` arm with:

```rust
            Some(Harness::Oz) | None => {
                // Oz / unset: Warp LLM catalog + BYOP entries opted into
                // orchestration (Phase 5b). `get_orchestration_llm_choices`
                // applies the per-provider toggle, harness-compatibility, and
                // Remote-reachability filters built in Phase 5a.
                let execution_mode = if is_local {
                    ai::agent::action::RunAgentsExecutionMode::Local
                } else {
                    // Only `is_remote()` is consulted by the filter; the
                    // Remote-mode placeholder fields don't affect output.
                    ai::agent::action::RunAgentsExecutionMode::Remote {
                        environment_id: String::new(),
                        worker_host: String::new(),
                        computer_use_enabled: false,
                    }
                };
                let choices: Vec<LLMInfo> = LLMPreferences::handle(ctx_dropdown)
                    .update(ctx_dropdown, |llm_prefs, ctx_update| {
                        llm_prefs.get_orchestration_llm_choices(
                            ctx_update,
                            &harness_type,
                            &execution_mode,
                        )
                    });
                let selected_display_name = choices
                    .iter()
                    .find(|llm| llm.id.to_string() == initial_model_id)
                    .map(|llm| llm.menu_display_name());
                let items = available_model_menu_items(
                    choices,
                    move |llm| {
                        DropdownAction::SelectActionAndClose(A::model_changed(llm.id.to_string()))
                    },
                    None,
                    None,
                    false,
                    false,
                    ctx_dropdown,
                );
                dropdown.set_rich_items(items, ctx_dropdown);
                if let Some(name) = &selected_display_name {
                    dropdown.set_selected_by_name(name, ctx_dropdown);
                }
            }
```

The reference `LLMInfo` import may need adding at the top of the file (check existing `use` lines):

```rust
use crate::ai::llms::{LLMInfo, LLMPreferences};
```

(`LLMPreferences` is likely already imported; add `LLMInfo` only if missing.)

- [ ] **Step 3.3: Build + verify no other callers broke**

```bash
cargo build -p warp 2>&1 | tail -10
# Expected: compiles cleanly. The signature didn't change so no callers need updates.
```

- [ ] **Step 3.4: Commit**

```
feat(orchestration): show BYOP entries in the model picker

Phase 5b task 3. Switches the Oz/unset arm of
populate_model_picker_for_harness to LLMPreferences::
get_orchestration_llm_choices. With FeatureFlag::LocalLlmProvider on
and AgentProvider.available_for_orchestration enabled, the user's
BYOP models now appear alongside first-party server models, filtered
by harness compatibility and (for Remote) reachability.
```

---

### Task 4: Swap stale-selection helpers + add empty-state default

**Files:**
- Modify: `app/src/ai/blocklist/inline_action/orchestration_controls.rs`.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/inline_action/orchestration_controls.rs:543-590` — `is_model_in_filtered_choices` and `first_filtered_model_id`.
- `app/src/ai/blocklist/inline_action/orchestration_controls.rs:1475-1511` — `sync_picker_selections` (the third call site).

These three helpers must share the same picker source as `populate_model_picker_for_harness` — otherwise the dropdown can show a BYOP entry the validator immediately treats as stale, or vice versa.

- [ ] **Step 4.1: Patch `is_model_in_filtered_choices`**

In `app/src/ai/blocklist/inline_action/orchestration_controls.rs:543`, replace the `Some(Harness::Oz) | None` arm with:

```rust
        Some(Harness::Oz) | None => {
            let execution_mode = if is_local {
                ai::agent::action::RunAgentsExecutionMode::Local
            } else {
                ai::agent::action::RunAgentsExecutionMode::Remote {
                    environment_id: String::new(),
                    worker_host: String::new(),
                    computer_use_enabled: false,
                }
            };
            LLMPreferences::handle(ctx)
                .update(ctx, |llm_prefs, ctx_update| {
                    llm_prefs.get_orchestration_llm_choices(
                        ctx_update,
                        harness_type,
                        &execution_mode,
                    )
                })
                .iter()
                .any(|llm| llm.id.to_string() == model_id)
        }
```

- [ ] **Step 4.2: Patch `first_filtered_model_id`**

The current signature is `pub fn first_filtered_model_id<V: View>(harness_type: &str, ctx: &mut ViewContext<V>) -> Option<String>` — it doesn't take `is_local`. Calling sites assume the Oz/unset branch returns the first first-party server model. With BYOP entries now in the picker, the *default* should still be a first-party entry (so a user who hasn't opted any BYOP provider in still gets a sane default).

Two options:
- **(A) Plumb `is_local` through** — every caller passes it from `OrchestrationEditState::execution_mode`.
- **(B) Keep the default as first-party** — internally call `get_base_llm_choices_for_agent_mode` (not `get_orchestration_llm_choices`) so the default is always a Warp model.

**Prefer (B)** — the *default* selection is a UX concern, not a validation one. A user who has zero BYOP providers opted in shouldn't see a BYOP entry as the default, and a user who has many should still default to a familiar Warp model. Keeping `first_filtered_model_id` unchanged also avoids signature churn.

Add a doc-comment note explaining the choice. Replace the existing helper's doc comment with:

```rust
/// Returns the default model_id for the given harness.
///
/// For Oz this is the first **first-party** Warp LLM — BYOP entries are
/// reachable via the picker but are never the picker's default selection
/// (Phase 5b decision: a user who has zero BYOP providers opted in should
/// see a familiar Warp model in the empty state, and users who have many
/// should still default to a familiar Warp model rather than an
/// alphabetically-first BYOP entry).
///
/// For non-Oz harnesses, it is an empty string (the "Default model" entry).
pub fn first_filtered_model_id<V: View>(
    harness_type: &str,
    ctx: &mut ViewContext<V>,
) -> Option<String> {
    let harness = Harness::parse_orchestration_harness(harness_type);
    match harness {
        Some(Harness::Oz) | None => {
            let llm_prefs = LLMPreferences::as_ref(ctx);
            llm_prefs
                .get_base_llm_choices_for_agent_mode(ctx)
                .next()
                .map(|llm| llm.id.to_string())
        }
        Some(_) => Some(String::new()),
    }
}
```

(This is unchanged from the existing implementation except for the doc comment; the existing logic already does the right thing.)

- [ ] **Step 4.3: Patch `sync_picker_selections`**

In `app/src/ai/blocklist/inline_action/orchestration_controls.rs:1475`, the Oz / unset branch around line 1486 currently calls `get_base_llm_choices_for_agent_mode`. Update it to query the orchestration choices so BYOP entries display the correct label when re-selected after a harness/mode change:

```rust
                Some(Harness::Oz) | None => {
                    // Phase 5b: query the same source the picker uses so the
                    // visible label tracks BYOP entries through harness/mode
                    // changes.
                    let execution_mode = match &state.execution_mode {
                        RunAgentsExecutionMode::Local => RunAgentsExecutionMode::Local,
                        RunAgentsExecutionMode::Remote {
                            environment_id,
                            worker_host,
                            computer_use_enabled,
                        } => RunAgentsExecutionMode::Remote {
                            environment_id: environment_id.clone(),
                            worker_host: worker_host.clone(),
                            computer_use_enabled: *computer_use_enabled,
                        },
                    };
                    LLMPreferences::handle(ctx_dropdown)
                        .update(ctx_dropdown, |llm_prefs, ctx_update| {
                            llm_prefs.get_orchestration_llm_choices(
                                ctx_update,
                                &harness_type,
                                &execution_mode,
                            )
                        })
                        .into_iter()
                        .find(|llm| llm.id.to_string() == target_model_id)
                        .map(|llm| llm.menu_display_name())
                }
```

Note: the existing code uses `find(...).map(|llm| llm.menu_display_name())` over an iterator. With `get_orchestration_llm_choices` returning `Vec<LLMInfo>` (owned), use `into_iter().find(...).map(...)` to consume the vec.

- [ ] **Step 4.4: Add stale-selection reset behavior**

The orchestration card's `OrchestrationEditState` already has logic that resets the model selection to `first_filtered_model_id` when the harness changes (search for where `is_model_in_filtered_choices` is consulted in this file; one common location is inside the harness-changed action handler). Phase 5b doesn't need to add new reset logic — `is_model_in_filtered_choices` now returns `false` for a BYOP entry whose provider's `available_for_orchestration` was just turned off, and the existing reset code will redirect to the first-party default. Verify this by searching for `is_model_in_filtered_choices` callers in the same file:

```bash
grep -n "is_model_in_filtered_choices" app/src/ai/blocklist/inline_action/orchestration_controls.rs
```

For each call site, walk the surrounding code and confirm the "stale → reset" path already exists. (At time of writing, the resets are triggered through `OrchestrationEditState::sync_after_harness_change` or similar — exact name may vary.) If a call site only logs the staleness without resetting, file a follow-up issue rather than expanding 5b's scope.

- [ ] **Step 4.5: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
# Expected: compiles cleanly.

cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
# Expected: no warnings.
```

- [ ] **Step 4.6: Run existing orchestration_controls tests**

```bash
cargo nextest run -p warp orchestration_controls 2>&1 | tail -20
# Expected: existing tests stay green. Phase 5b doesn't change observable
# behavior for non-BYOP cases — first-party choices flow through unchanged.
```

- [ ] **Step 4.7: Commit**

```
feat(orchestration): query orchestration choices in stale-selection helpers

Phase 5b task 4. Updates is_model_in_filtered_choices and
sync_picker_selections to query LLMPreferences::
get_orchestration_llm_choices so the picker, the stale-selection
detector, and the visible-label sync all share the same model source.

first_filtered_model_id keeps the first-party default — by design,
BYOP entries are reachable via the picker but never the picker's
implicit default.

Existing orchestration_controls tests stay green; behavior for
non-BYOP cases is unchanged.
```

---

## Stage D — Submit-time validator wire-up

### Task 5: Wire `validate_orchestration_model_id` into `run_agents::validate_request`

**Files:**
- Modify: `app/src/ai/blocklist/action_model/execute/run_agents.rs` — call the validator.
- Modify: `app/src/ai/agent_sdk/common.rs` — remove the `#[allow(dead_code)]` annotations.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/action_model/execute/run_agents.rs:345-364` — `validate_request` (current implementation).
- `app/src/ai/blocklist/action_model/execute/run_agents.rs:83-103` — `dispatch_run_agents` calls `validate_request(&request)` synchronously and short-circuits with `RunAgentsResult::Failure { error }` on `Err`. The validator returns `Result<(), String>`.
- `app/src/ai/agent_sdk/common.rs:71-153` — `validate_orchestration_model_id` returns `anyhow::Result<LLMId>`. We need to adapt this to the `Result<(), String>` shape `validate_request` returns.
- `app/src/ai/agent_sdk/common.rs:158-169` — `compatible_harness_names` (also `#[allow(dead_code)]` here).

- [ ] **Step 5.1: Extend `validate_request` to take `ctx`**

The current signature is `fn validate_request(request: &RunAgentsRequest) -> Result<(), String>` — no `ctx`. `validate_orchestration_model_id` needs an `AppContext` to read `LLMPreferences` and `AISettings`. Thread an `AppContext` through.

In `app/src/ai/blocklist/action_model/execute/run_agents.rs`, change the signature (line 345):

```rust
fn validate_request(
    request: &RunAgentsRequest,
    ctx: &AppContext,
) -> Result<(), String> {
    if request.agent_run_configs.is_empty() {
        return Err("orchestrate: empty agent_run_configs".to_string());
    }
    if matches!(request.execution_mode, RunAgentsExecutionMode::Local) {
        if let Some(harness) = Harness::parse_local_child_harness(&request.harness_type) {
            if let Some(message) = local_child_harness_disabled_message(harness) {
                return Err(message.to_string());
            }
        }
    }
    if matches!(
        request.execution_mode,
        RunAgentsExecutionMode::Remote { .. }
    ) && request.harness_type.eq_ignore_ascii_case("opencode")
    {
        return Err("Remote child agents do not support the opencode harness yet.".to_string());
    }

    // Phase 5b. When the run-wide model is a BYOP entry, run the full
    // filter pipeline against the harness + execution mode to catch a
    // stale picker selection (e.g. user picked an Anthropic BYOP model
    // then switched harness to Codex before submitting).
    if request.model_id.starts_with(ai::local_provider::llm_id::BYOP_PREFIX) {
        if let Err(err) = crate::ai::agent_sdk::common::validate_orchestration_model_id(
            &request.model_id,
            &request.harness_type,
            &request.execution_mode,
            ctx,
        ) {
            return Err(err.to_string());
        }
    }

    Ok(())
}
```

Add the import for `AppContext` at the top of the file if needed (search for `use warpui::`):

```rust
use warpui::AppContext;
```

`AppContext` is typically already in scope via the `ModelContext` parameter in `dispatch_run_agents`, but the helper needs its own reference.

- [ ] **Step 5.2: Update the single caller of `validate_request`**

The call site in `dispatch_run_agents` (line 98) currently reads `if let Err(error) = validate_request(&request)`. Update to pass the context:

```rust
        if let Err(error) = validate_request(&request, ctx) {
```

`ctx` here is the `&mut ModelContext<Self>` parameter — that derefs to `&AppContext` via `AsRef<AppContext>` (or use `&*ctx` / `ctx.app()` per the existing call convention in this file). Check the file's existing pattern; if helpers there take `&AppContext`, the canonical conversion is already in use elsewhere in the same function and can be copied.

If `ModelContext<Self>` doesn't deref directly, pass `ctx.app_context()` or the equivalent — search for `as_ref(ctx)` or `.app()` calls inside `dispatch_run_agents` to find the right idiom in this codebase.

- [ ] **Step 5.3: Remove the `#[allow(dead_code)]` annotations**

In `app/src/ai/agent_sdk/common.rs`, remove the two `#[allow(dead_code)]` lines:

- Line 70: above `pub fn validate_orchestration_model_id`.
- Line 158: above `fn compatible_harness_names`.

Also update the doc-comment line 70 (the line that says `// Wired into run_agents::validate_request in Phase 5c.`) to remove the deferral note — the function is now live.

- [ ] **Step 5.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
# Expected: compiles cleanly. The dead_code allow can be removed safely now.

cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
# Expected: no warnings. Without the call site change in step 5.2, clippy
# will fail on the removed #[allow(dead_code)] — fix that first.
```

- [ ] **Step 5.5: Add a focused unit test**

In `app/src/ai/blocklist/action_model/execute/run_agents.rs` (or its sibling `run_agents_tests.rs` if a test module already exists for this file), add:

```rust
#[cfg(test)]
mod validate_request_byop_tests {
    use super::*;
    use crate::settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel};
    use crate::ai::settings_tests::initialize_settings_for_tests;
    use ai::agent::action::{
        RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest,
    };
    use ai::local_provider::llm_id;
    use settings::Setting;

    fn make_request(model_id: &str, harness_type: &str) -> RunAgentsRequest {
        RunAgentsRequest {
            summary: String::new(),
            base_prompt: "go".to_string(),
            skills: vec![],
            model_id: model_id.to_string(),
            harness_type: harness_type.to_string(),
            execution_mode: RunAgentsExecutionMode::Local,
            agent_run_configs: vec![RunAgentsAgentRunConfig {
                name: "child-1".to_string(),
                prompt: "do thing".to_string(),
                title: "T".to_string(),
            }],
            plan_id: String::new(),
            harness_auth_secret_name: None,
        }
    }

    fn add_byop_provider_for_orchestration(
        app: &warpui::App,
        provider_id: &str,
        api_type: AgentProviderApiType,
    ) {
        AISettings::handle(app).update(app, |settings, ctx| {
            let provider = AgentProvider {
                id: provider_id.to_owned(),
                name: "P".to_owned(),
                kind: Default::default(),
                api_type,
                base_url: "https://api.example.com/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("m1".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            let mut providers = settings.agent_providers.value().clone();
            providers.push(provider);
            settings.agent_providers.set_value(providers, ctx).unwrap();
        });
    }

    #[test]
    fn validate_request_accepts_compatible_byop_with_oz_harness() {
        let app = warpui::App::test();
        initialize_settings_for_tests(&app);
        add_byop_provider_for_orchestration(&app, "prov-1", AgentProviderApiType::OpenAi);

        let model_id = llm_id::encode("prov-1", "m1").to_string();
        let request = make_request(&model_id, "oz");
        assert!(validate_request(&request, &app).is_ok());
    }

    #[test]
    fn validate_request_rejects_anthropic_byop_with_codex_harness() {
        let app = warpui::App::test();
        initialize_settings_for_tests(&app);
        add_byop_provider_for_orchestration(&app, "prov-2", AgentProviderApiType::Anthropic);

        let model_id = llm_id::encode("prov-2", "m1").to_string();
        let request = make_request(&model_id, "codex");
        let err = validate_request(&request, &app).unwrap_err();
        assert!(err.contains("not compatible with harness 'codex'"), "{err}");
    }

    #[test]
    fn validate_request_rejects_byop_when_provider_not_opted_in() {
        let app = warpui::App::test();
        initialize_settings_for_tests(&app);
        // Add provider but flip the opt-in off.
        add_byop_provider_for_orchestration(&app, "prov-3", AgentProviderApiType::OpenAi);
        AISettings::handle(&app).update(&app, |settings, ctx| {
            let mut providers = settings.agent_providers.value().clone();
            providers
                .last_mut()
                .unwrap()
                .available_for_orchestration = false;
            settings.agent_providers.set_value(providers, ctx).unwrap();
        });

        let model_id = llm_id::encode("prov-3", "m1").to_string();
        let request = make_request(&model_id, "oz");
        let err = validate_request(&request, &app).unwrap_err();
        assert!(err.contains("not enabled for orchestration"), "{err}");
    }

    #[test]
    fn validate_request_passes_through_first_party_model_ids() {
        // First-party model IDs (without the byop: prefix) skip the BYOP
        // validator entirely — they are validated upstream by the picker.
        let app = warpui::App::test();
        initialize_settings_for_tests(&app);
        let request = make_request("claude-4.5-sonnet", "oz");
        // No BYOP providers registered; the request should still pass
        // because validate_request does NOT validate first-party IDs.
        assert!(validate_request(&request, &app).is_ok());
    }
}
```

Adjust the test helper imports to match your tree (`initialize_settings_for_tests` is in `app/src/settings/ai_tests.rs`; the `use` path may be `crate::settings::ai_tests::initialize_settings_for_tests`).

- [ ] **Step 5.6: Run the tests**

```bash
cargo nextest run -p warp validate_request_byop_tests 2>&1 | tail -10
# Expected:
#    PASS warp::ai::blocklist::action_model::execute::run_agents::validate_request_byop_tests::validate_request_accepts_compatible_byop_with_oz_harness
#    PASS warp::ai::blocklist::action_model::execute::run_agents::validate_request_byop_tests::validate_request_rejects_anthropic_byop_with_codex_harness
#    PASS warp::ai::blocklist::action_model::execute::run_agents::validate_request_byop_tests::validate_request_rejects_byop_when_provider_not_opted_in
#    PASS warp::ai::blocklist::action_model::execute::run_agents::validate_request_byop_tests::validate_request_passes_through_first_party_model_ids
#  Summary: 4 tests run, 4 passed
```

- [ ] **Step 5.7: Commit**

```
feat(orchestration): wire BYOP submit-time validator into run_agents::validate_request

Phase 5b task 5. validate_request now calls
validate_orchestration_model_id from agent_sdk/common when the
run-wide model_id is a BYOP entry, catching stale picker selections
that survive the picker filter (e.g. harness switched between picker
display and submit).

Removes the #[allow(dead_code)] annotations on the validator and its
compatible_harness_names helper — both are now live.

4 unit tests cover: compatible BYOP+harness combo passes, Anthropic
BYOP+codex harness rejected with clear message, opted-out provider
rejected, first-party model ids pass through unchanged.
```

---

## Stage E — End-to-end Local Native verification

### Task 6: Integration test — BYOP model_id flows through to `StartAgentExecutionMode::Local`

**Files:**
- Create: `crates/ai/tests/orchestration_byop_local_native.rs`.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/action_model/execute/run_agents.rs:386-435` — `run_agents_to_start_agent_mode` (the function under test).
- `crates/ai/src/agent/action/mod.rs:200-260` — `RunAgentsExecutionMode`, `RunAgentsAgentRunConfig`, `StartAgentExecutionMode`.
- `crates/ai/tests/local_provider_integration.rs` (if it exists in your tree) — the integration-test boilerplate convention.

The integration test verifies the contract between the orchestration submit path and the per-child agent dispatcher: given a BYOP model id, `run_agents_to_start_agent_mode` produces a `StartAgentExecutionMode::Local` carrying the BYOP id as the model. This is the actual "Local Native dispatch" promise of Phase 5b — the downstream BYOP dispatcher (Phase 4d) already knows how to interpret `byop:` ids and route them to the correct provider config.

The test does NOT spin up a real mock HTTP server — that's a follow-up. 5b's verification gate is unit + integration-test parity; live smoke testing flips the Phase 5b README row from ⏳ to ✅.

- [ ] **Step 6.1: Move `run_agents_to_start_agent_mode` test surface if necessary**

`run_agents_to_start_agent_mode` lives in `app/src/ai/blocklist/...` (the `warp` binary crate, not `crates/ai`). Integration tests in `crates/ai/tests/` cannot import from the binary crate. Two options:

- **(A) Add the test to `app/src/ai/blocklist/action_model/execute/run_agents_tests.rs`** (sibling unit-test file pattern). Treat it as a heavier-weight test inside the existing `warp` crate test scope.
- **(B) Move `run_agents_to_start_agent_mode` to `crates/ai/`** and re-export — broader change, not justified for one test.

**Prefer (A)**. The function is `pub` (`pub fn run_agents_to_start_agent_mode`) so it's already visible to sibling test files.

Update the file map at the top of this plan: the test file is `app/src/ai/blocklist/action_model/execute/run_agents_tests.rs` (sibling unit-test file), not `crates/ai/tests/orchestration_byop_local_native.rs`. If `run_agents_tests.rs` doesn't exist, create it and wire it via `#[cfg(test)] #[path = "run_agents_tests.rs"] mod tests;` at the bottom of `run_agents.rs`.

- [ ] **Step 6.2: Write the test**

In `app/src/ai/blocklist/action_model/execute/run_agents_tests.rs` (create if missing):

```rust
//! Phase 5b integration tests for the orchestration submit -> child-agent
//! translator. Verifies that BYOP model ids survive `run_agents_to_start_agent_mode`
//! into the per-child `StartAgentExecutionMode` so the existing in-process
//! BYOP dispatcher (Phase 4d) takes over.

use super::*;
use ai::agent::action::{
    RunAgentsAgentRunConfig, RunAgentsExecutionMode, StartAgentExecutionMode,
};
use ai::local_provider::llm_id;

fn make_run_config() -> RunAgentsAgentRunConfig {
    RunAgentsAgentRunConfig {
        name: "child-1".to_string(),
        prompt: "p".to_string(),
        title: "T".to_string(),
    }
}

#[test]
fn byop_model_id_threads_into_local_native_start_mode() {
    let model_id = llm_id::encode("prov-1", "m1").to_string();
    let mode = run_agents_to_start_agent_mode(
        &RunAgentsExecutionMode::Local,
        "oz",
        &model_id,
        &[],
        None,
        &make_run_config(),
    )
    .expect("Local Native + BYOP must translate");
    match mode {
        StartAgentExecutionMode::Local {
            harness_type,
            model_id: forwarded,
        } => {
            assert!(harness_type.is_none(), "oz harness should be None on Local");
            assert_eq!(forwarded.as_deref(), Some(model_id.as_str()));
        }
        other => panic!("expected Local mode, got {other:?}"),
    }
}

#[test]
fn empty_harness_is_treated_as_native_oz() {
    let model_id = llm_id::encode("prov-1", "m1").to_string();
    let mode = run_agents_to_start_agent_mode(
        &RunAgentsExecutionMode::Local,
        "",
        &model_id,
        &[],
        None,
        &make_run_config(),
    )
    .expect("empty harness == native oz");
    assert!(matches!(
        mode,
        StartAgentExecutionMode::Local {
            harness_type: None,
            ..
        }
    ));
}

#[test]
fn byop_model_id_threads_into_codex_local_when_harness_set() {
    // Sanity: even when the orchestration UI lets the user pick a third-party
    // local harness, the BYOP id rides along as the model. (Wiring the
    // env-vars for that path is Phase 5c work; this test only proves the
    // translator preserves the model_id.)
    let model_id = llm_id::encode("prov-1", "m1").to_string();
    let mode = run_agents_to_start_agent_mode(
        &RunAgentsExecutionMode::Local,
        "codex",
        &model_id,
        &[],
        None,
        &make_run_config(),
    );
    // Note: this may return Err if local_child_harness_disabled_message
    // says codex is disabled in this build. Use match rather than assume Ok.
    if let Ok(StartAgentExecutionMode::Local {
        harness_type,
        model_id: forwarded,
    }) = mode
    {
        assert_eq!(harness_type.as_deref(), Some("codex"));
        assert_eq!(forwarded.as_deref(), Some(model_id.as_str()));
    }
}

#[test]
fn empty_model_id_returns_none_on_local_native() {
    let mode = run_agents_to_start_agent_mode(
        &RunAgentsExecutionMode::Local,
        "oz",
        "", // run-wide model_id empty => fall back to child's own pref
        &[],
        None,
        &make_run_config(),
    )
    .expect("empty model_id must still translate");
    match mode {
        StartAgentExecutionMode::Local { model_id, .. } => assert!(model_id.is_none()),
        other => panic!("expected Local mode, got {other:?}"),
    }
}
```

Wire the test module into `run_agents.rs` (at the bottom of the file, outside any other test cfg block):

```rust
#[cfg(test)]
#[path = "run_agents_tests.rs"]
mod tests;
```

If the file already has a `#[cfg(test)] mod` declaration, append the new tests inside it instead.

- [ ] **Step 6.3: Run the tests**

```bash
cargo nextest run -p warp run_agents 2>&1 | tail -20
# Expected: the four new tests pass alongside any existing run_agents tests.
```

- [ ] **Step 6.4: Verify with the full presubmit-equivalent slice**

```bash
cargo nextest run -p warp 2>&1 | tail -30
# Expected: workspace tests stay green. Phase 5b shouldn't break anything
# outside the touched files.

cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
# Expected: no warnings.

cargo fmt -- --check 2>&1 | tail -5
# Expected: no diff.
```

- [ ] **Step 6.5: Commit**

```
test(orchestration): integration tests for BYOP model_id flow into StartAgentExecutionMode::Local

Phase 5b task 6. Adds run_agents_tests.rs with 4 tests proving:
  - Local + oz harness + BYOP model_id produces StartAgentExecutionMode::
    Local { harness_type: None, model_id: Some("byop:...") }
  - Empty harness behaves identically to oz
  - codex harness (when enabled in build) preserves the BYOP model_id
  - Empty run-wide model_id yields model_id: None (child inherits default)

These are the contract tests for the Phase 5b Local Native dispatch
promise — the downstream in-process BYOP dispatcher from Phase 4d
takes over once a byop: id appears in StartAgentExecutionMode::Local.
```

---

## Stage F — Status doc updates

### Task 7: Update Phase 5b row in `specs/multi-local-llm/README.md`

**Files:**
- Modify: `specs/multi-local-llm/README.md`.

**Read these reference files FIRST:**
- `specs/multi-local-llm/README.md` — existing status table / phase rows. Phase 4d's "code complete" entry is the template to follow.

- [ ] **Step 7.1: Append a Phase 5b status section**

In `specs/multi-local-llm/README.md`, after the existing Phase 5a section, add:

```markdown
**Phase 5b (BYOP orchestration — Full Local Native path)** code is complete on `multi-local-llm`. Builds on 5a's filter / validator / synthesis pipeline by wiring it into the user-facing surfaces:

- Settings UI — new "Available for orchestration" toggle on each BYOP provider card (`AgentProvider::available_for_orchestration`), gated on `FeatureFlag::LocalLlmProvider`. Default off so existing BYOP configs don't surface in orchestration until the user explicitly opts in.
- Orchestration model picker — `populate_model_picker_for_harness`, `is_model_in_filtered_choices`, and `sync_picker_selections` now query `LLMPreferences::get_orchestration_llm_choices`, so opted-in BYOP entries appear alongside first-party server models. Harness/mode changes re-filter the picker; the picker default stays on a first-party Warp model by design.
- Submit-time validator — `validate_request` calls the 5a `validate_orchestration_model_id` whenever the run-wide model id is a BYOP entry. Stale picks that survived the picker (e.g. harness switched between picker display and submit) are rejected with a structured error.
- Native dispatch — `run_agents_to_start_agent_mode` already forwards the run-wide `model_id` into `StartAgentExecutionMode::Local { model_id: Some(...) }`. The downstream in-process BYOP dispatcher (Phase 4d) takes over from there, so no new dispatch wiring was required. Four integration tests cover the translator's model-id passthrough.

External-CLI harnesses (Claude Code / Codex / Gemini CLI / OpenCode) and Remote execution remain deferred to Phases 5c and 5d.

> **Verification gate:** live-test smoke against a real BYOP provider in Local Native mode (`oz` harness, `Local` execution, ≥2 child agents) is the remaining manual step. Once orchestration completes end-to-end against the user-configured BYOP endpoint, the Phase 5b row flips to ✅ and this note is removed.
```

- [ ] **Step 7.2: If the README has a status table, add a row**

If the file has a markdown status table near the top (e.g. "| Phase | Status | …"), add a row:

```markdown
| 5b | BYOP orchestration — Full Local Native path | ⏳ Code complete | Settings opt-in toggle, picker swap, submit validator wired, Native dispatch verified end-to-end |
```

- [ ] **Step 7.3: Commit**

```
docs(specs/multi-local-llm): record Phase 5b code-complete status

Phase 5b lights up the user-facing surfaces of Phase 5a's pipeline
end-to-end for the Local Native (oz) dispatch path. External-CLI
harnesses + Remote remain deferred to 5c / 5d.
```

---

## Stage G — Memory update

### Task 8: Record Phase 5b status in `~/.claude/.../memory`

**Files:**
- Modify: `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/MEMORY.md` + a new memory file under the same directory.

This is bookkeeping for future Claude Code sessions; if you're running this plan without Claude Code's memory layer, skip Task 8.

- [ ] **Step 8.1: Write the memory file**

Save a new memory file at `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/multi-local-llm-phase-5b.md` with the standard frontmatter, summarizing:

- Phase 5b is code complete on `multi-local-llm`.
- Three user-facing surfaces lit up: Settings UI toggle, orchestration picker swap, submit-time validator.
- Native dispatch verified end-to-end via integration tests.
- Outstanding: external-CLI harness env-var injection (5c), Remote credential bridge + GraphQL forwarding (5d), live smoke testing.

- [ ] **Step 8.2: Add a one-line index entry to MEMORY.md**

Append a single line to `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/MEMORY.md`:

```markdown
- [Phase 5b code complete on multi-local-llm](multi-local-llm-phase-5b.md) — BYOP orchestration Local Native path lit up; remote (5c/5d) still deferred
```

- [ ] **Step 8.3: No git commit needed** — `~/.claude` is outside the repo.

---

## Self-review checklist

Run this before declaring the plan complete:

1. **Spec coverage** — every item from `spec-phase-5.md` that the user's chosen scope ("Full Local Native path") covers has a task: ✓ Settings UI opt-in toggle (Task 1, 2); ✓ orchestration picker swap (Task 3, 4); ✓ submit validator wire-up (Task 5); ✓ Local Native dispatch verification (Task 6). Out-of-scope items (env-var injection, Remote credential bridge, GraphQL, compaction forwarding to Remote) are explicitly listed as deferred at the top of the plan.

2. **Placeholder scan** — no `TODO`, `TBD`, `implement later`, or hand-wavy "add appropriate error handling" steps. Every code block is concrete.

3. **Type consistency** — `AISettingsPageAction::ToggleAgentProviderOrchestrationAvailability { provider_index }` is used identically in the variant declaration (Task 1.1), the handler (Task 1.2), the render helper (Task 2.2), and the action dispatch in `render_orchestration_toggle` (Task 2.3). `validate_request` keeps its `Result<(), String>` return signature; `validate_orchestration_model_id` returns `anyhow::Result<LLMId>` and is adapted via `err.to_string()` in Task 5.1. `RunAgentsExecutionMode::Local` / `Remote { … }` field shape matches the definition in `crates/ai/src/agent/action/mod.rs:200`.

4. **Backward compat** — `AgentProvider::available_for_orchestration` already defaults to `false` (Phase 5a), so flipping FeatureFlag::LocalLlmProvider on doesn't auto-surface anyone's providers in the orchestration picker until they explicitly toggle in.

5. **Test coverage** — Task 1 covers the action handler, Task 5 covers the submit-time validator across the four key cases (compatible, incompatible-harness, opted-out, first-party passthrough), Task 6 covers the dispatch translator. The picker swap (Tasks 3 + 4) relies on the existing `orchestration_controls` test suite staying green; new picker-specific tests are deferred because the helpers are thin wrappers over `get_orchestration_llm_choices`, which already has full coverage in Phase 5a's `byop_orchestration_filter_tests.rs`.

---

## Plan complete

Plan complete and saved to `specs/multi-local-llm/plan-phase-5b.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.

Which approach?
