# Phase 5d — BYOP Orchestration — Remote Credential Bridge — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Touching UI? Read `warp-ui-guidelines` first.

**Goal:** Close the client-side surface for Remote BYOP orchestration. Add the Settings UI to surface the `remote_secret_name` field (Phase 5a added the data field but no UI), wire the Auto-create-managed-secret button, extend the orchestration wire shape (`AgentConfigSnapshot`) with BYOP routing + compaction-inheritance fields, and populate those fields at submit time when the user picks a BYOP model with Remote execution.

**Scoping decision (recorded during planning):** Worker-side BYOP routing — the part that *actually* makes the remote child agent talk to the user's BYOP endpoint at runtime — depends on warp.dev server / Namespace infrastructure that this repo cannot ship from. Phase 5d's client side is the complete, mergeable unit: the wire fields land on `AgentConfigSnapshot`, the submit path populates them, and the Settings UI lets users configure the managed secret. End-to-end Remote BYOP runs are gated on a separate server-team task to honor the new fields on the receiving side. Worker-side support is tracked as out of scope; the client-side bridge is correct and complete regardless.

**Out of scope for 5d:**

- Self-hosted-worker (`crates/remote_server/`) BYOP runtime support. If we later confirm that path runs orchestration children, a follow-up adds the receive-side handling there.
- Warp-managed worker (Namespace) BYOP runtime support. Server-team owns.
- Banner warnings about provider edits during in-flight orchestration runs (spec calls this out as a 5d-class concern under "Risks → Provider edit during orchestration"; it's a follow-up — not blocking Remote bridge correctness).
- Gemini CLI enablement as a local child harness. Stays disabled; orthogonal to Remote.

**Decisions locked in (from `spec-phase-5.md` plus discoveries during 5c execution):**

| Decision | Choice |
|---|---|
| Wire-boundary struct for BYOP routing | Extend the existing `AgentConfigSnapshot` (`app/src/ai/ambient_agents/task.rs:29`) — this is the struct that already crosses the orchestration RPC boundary (`SpawnAgentRequest.config`). Don't introduce a new envelope. |
| New `AgentConfigSnapshot` fields | `byop_base_url: Option<String>`, `byop_api_type: Option<String>`, `compaction_model_provider_id: Option<String>`, `compaction_model_id: Option<String>` — all `#[serde(default, skip_serializing_if = "Option::is_none")]` so the wire shape stays backward-compatible for non-BYOP and non-Phase-4d users. |
| BYOP managed-secret routing | Extend `HarnessAuthSecretsConfig` with `byop_auth_secret_name: Option<String>`. Existing `claude_auth_secret_name` / `codex_auth_secret_name` keep their semantics; the new field is the channel for BYOP credentials and is populated from `AgentProvider.remote_secret_name`. |
| Settings UI placement | New "Remote managed secret" field + "Auto-create" button rendered on each BYOP provider card, visible only when the Phase 5b "Available for orchestration" toggle is ON **and** the provider's `base_url` is publicly reachable (Phase 5a's `base_url_reachable_from_remote` heuristic). Localhost / RFC1918 / `.local` providers skip the field entirely — they're Local-only by reachability. |
| Auto-create behavior | Click → `UpdateManager::create_managed_secret(name = "byop-{provider_id}", value = provider.api_key, owner = Personal)`. The returned name is written back into `remote_secret_name`. Async + error-toast on failure. Personal owner default so a careless team-workspace user doesn't expose a personal key. |
| Compaction inheritance | When the user has Phase 4d `byop_compaction_model_provider_id` + `byop_compaction_model_id` set AND `execution_mode = Remote`, populate the two new `compaction_model_*` fields on `AgentConfigSnapshot`. Falls back to absent (server picks its own) when settings are empty. |
| Backward compat | All four new `AgentConfigSnapshot` fields are `Option<T>` with `serde(default)`. Pre-5d clients deserialize new payloads cleanly (unknown fields are silently dropped only if the deserializer is configured for it — but the four `Option<T>` are always set to `None` on old payloads). Pre-5d servers receiving new payloads ignore unknown fields per the JSON convention. |
| Phase 5c follow-ups | Fold the two non-blocking items from Phase 5c's whole-branch review: (a) `lookup_byop` empty-key gap, (b) DRY refactor sharing logic between `lookup_byop` and `resolve_byop_for_local_child`. These ship as the first task here so the BYOP lookup is consistent across all callers before Phase 5d's submit-path forwarding lands. |

**Architecture (delta over 5c):**

Phase 5c built the supply chain for the env-var bag. Phase 5d builds a sibling supply chain for the Remote wire bridge:

1. **Pre-cleanup (Task 1):** Extract a shared `resolve_byop_inner` from `lookup_byop` + `resolve_byop_for_local_child`. Both gain the empty-key guard. This addresses Phase 5c's review follow-ups before adding a third caller in Task 6.
2. **Settings UI (Tasks 2 + 3):** Three new `AISettingsPageAction` variants + render the new "Remote managed secret" field + Auto-create button.
3. **Wire shape (Task 4):** Extend `AgentConfigSnapshot` with four new optional fields + extend `HarnessAuthSecretsConfig` with `byop_auth_secret_name`. Pure data-layer additions, backward-compatible serde.
4. **Submit-path forwarding (Tasks 5 + 6):** In `launch_remote_child`, resolve the BYOP entry via the shared helper, populate the new wire fields, and route the api_key through the new `byop_auth_secret_name` channel. Compaction inheritance is forwarded from `AISettings` when applicable.
5. **Tests (Task 7):** Round-trip serialization tests for the new fields, populate-on-submit tests for the Remote path.
6. **Docs (Tasks 8 + 9):** README + memory.

**Tech Stack:** Rust 2021, `serde`, `warpui`, `tokio`, the existing `UpdateManager` managed-secret API surface.

---

## Per-touchpoint reference

| Concern | Source of truth |
|---|---|
| `AgentProvider.remote_secret_name` (Phase 5a) | `app/src/settings/ai.rs:812` |
| `AgentConfigSnapshot` (wire struct) | `app/src/ai/ambient_agents/task.rs:29` |
| `HarnessAuthSecretsConfig` (wire struct) | `app/src/ai/ambient_agents/task.rs:146` |
| `launch_remote_child` (submit path) | `app/src/pane_group/pane/terminal_pane.rs:2047` |
| `harness_auth_secrets` assembly (current mapping) | `app/src/pane_group/pane/terminal_pane.rs:2167-2180` |
| `RemoteLaunchFields.auth_secret_name` | `app/src/pane_group/pane/terminal_pane.rs:2031` |
| `lookup_byop` (Phase 1b-2) | `app/src/ai/agent_providers/mod.rs:~219` |
| `resolve_byop_for_local_child` (Phase 5c) | `app/src/ai/agent_providers/mod.rs:~240` |
| `byop_compaction_model_provider_id` + `byop_compaction_model_id` settings (Phase 4d) | `app/src/settings/ai.rs` (search for `byop_compaction_model_provider_id`) |
| `base_url_reachable_from_remote` (Phase 5a) | `app/src/ai/byop_orchestration_filter.rs` |
| `UpdateManager::create_managed_secret` | `app/src/server/server_api/managed_secrets.rs:87` |
| `AISettingsPageAction` enum | `app/src/settings_view/ai_page.rs:~2756+` |
| `AgentProvidersWidget::render_provider_card` (place to render the new field) | `app/src/settings_view/agent_providers_widget.rs:~1235` |
| `render_orchestration_toggle` helper (Phase 5b — anchor for placement) | `app/src/settings_view/agent_providers_widget.rs:~575` |

---

## File map

**Created:** none. All changes are extensions to existing files.

**Modified:**

- `app/src/ai/agent_providers/mod.rs` — extract `resolve_byop_inner` helper; update `lookup_byop` + `resolve_byop_for_local_child` to use it; add empty-key guard to `lookup_byop`.
- `app/src/ai/agent_providers/mod_tests.rs` — add test confirming `lookup_byop` rejects empty api_key (closes Phase 5c follow-up).
- `app/src/ai/ambient_agents/task.rs` — extend `AgentConfigSnapshot` with 4 new fields; extend `HarnessAuthSecretsConfig` with `byop_auth_secret_name`.
- `app/src/settings_view/ai_page.rs` — add 2 new `AISettingsPageAction` variants (`SetAgentProviderRemoteSecretName`, `AutoCreateAgentProviderManagedSecret`) + handlers.
- `app/src/settings_view/agent_providers_widget.rs` — render the new "Remote managed secret" field + "Auto-create" button on each provider card (gated on Phase 5b toggle + Phase 5a reachability heuristic).
- `app/src/pane_group/pane/terminal_pane.rs` — in `launch_remote_child` (line 2047), resolve BYOP entry when `model_id` starts with `byop:`, populate the four new wire fields, route api_key through the new `byop_auth_secret_name`.
- `app/src/pane_group/pane/local_harness_launch_tests.rs` — none. The new wire-shape tests live with the struct in `task.rs`.
- `app/src/ai/ambient_agents/task_tests.rs` (or sibling) — add serde round-trip tests for the new fields.
- `specs/multi-local-llm/README.md` — flip Phase 5d row to 🧪 code-complete.

---

## Stage A — Phase 5c review follow-ups (pre-cleanup)

### Task 1: DRY `lookup_byop` + `resolve_byop_for_local_child` and close the empty-key gap

**Files:**
- Modify: `app/src/ai/agent_providers/mod.rs`.
- Modify: `app/src/ai/agent_providers/mod_tests.rs`.

**Read these reference files FIRST:**
- `app/src/ai/agent_providers/mod.rs:~219-227` — current `lookup_byop` (Phase 1b-2; takes `&LLMId`, returns `Option<(AgentProvider, String /*api_key*/, String /*model_id*/)>`).
- `app/src/ai/agent_providers/mod.rs:~240-260` — current `resolve_byop_for_local_child` (Phase 5c; takes `&str`, returns `Option<(AgentProvider, String, String)>`, has the empty-key guard).
- Phase 5c review (commit `cdd74788`+): both reviewers flagged the duplication and the latent `lookup_byop` empty-key gap.

- [ ] **Step 1.1: Extract `resolve_byop_inner`**

Add a private helper above `lookup_byop`:

```rust
/// Shared BYOP resolution: decode → look up provider → fetch + validate
/// api_key. Returns `None` for non-BYOP ids, malformed ids, missing
/// providers, missing keys, or empty keys.
///
/// Both `lookup_byop` and `resolve_byop_for_local_child` delegate to this
/// helper. The only difference between the two public entry points is the
/// input type — `lookup_byop` accepts `&ai::LLMId` (already-decoded type)
/// while `resolve_byop_for_local_child` accepts `&str` (raw model_id from
/// the orchestration submit path).
fn resolve_byop_inner(
    app: &AppContext,
    llm_id: &ai::LLMId,
) -> Option<(AgentProvider, String, String)> {
    let (provider_id, model_id) = llm_id::decode(llm_id)?;

    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let provider = providers.into_iter().find(|p| p.id == provider_id)?;

    let api_key = AgentProviderSecrets::as_ref(app)
        .get(&provider.id)?
        .to_string();
    if api_key.is_empty() {
        return None;
    }

    Some((provider, api_key, model_id))
}
```

- [ ] **Step 1.2: Update `lookup_byop` to delegate**

Replace the body of `lookup_byop`:

```rust
pub fn lookup_byop(
    app: &AppContext,
    llm_id: &ai::LLMId,
) -> Option<(AgentProvider, String, String)> {
    resolve_byop_inner(app, llm_id)
}
```

This adds the empty-key guard that `lookup_byop` was missing (it previously accepted `Some("")` as the api_key and forwarded it downstream).

- [ ] **Step 1.3: Update `resolve_byop_for_local_child` to delegate**

```rust
pub fn resolve_byop_for_local_child(
    app: &AppContext,
    model_id: &str,
) -> Option<(AgentProvider, String, String)> {
    let llm_id: ai::LLMId = model_id.into();
    resolve_byop_inner(app, &llm_id)
}
```

- [ ] **Step 1.4: Add a test for `lookup_byop`'s empty-key rejection**

In `app/src/ai/agent_providers/mod_tests.rs`, mirror the `resolve_byop_for_local_child_returns_none_when_api_key_is_empty_string` test pattern but call `lookup_byop` instead. The function signature differs (takes `&LLMId`), so:

```rust
#[test]
fn lookup_byop_returns_none_when_api_key_is_empty_string() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-empty-key".to_owned(),
                name: "P".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.example.com/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("m1".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings.agent_providers.set_value(vec![provider], ctx).unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-empty-key", String::new(), ctx);
        });

        let llm_id = llm_id::encode("prov-empty-key", "m1");
        let resolved = app.read(|ctx| super::lookup_byop(ctx, &llm_id));
        assert!(resolved.is_none(), "empty api_key must produce None");
    });
}
```

- [ ] **Step 1.5: Run all `lookup_byop` and `resolve_byop_for_local_child` tests**

```bash
cargo nextest run -p warp 'lookup_byop|resolve_byop_for_local_child' 2>&1 | tail -15
```

Existing tests should stay green. The new test passes. (Note: nextest treats the filter as a substring across all test name segments, not a regex — passing `lookup_byop` and `resolve_byop_for_local_child` as separate runs is safer if the combined filter doesn't match.)

- [ ] **Step 1.6: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 1.7: Commit**

```
refactor(ai/agent_providers): share resolve_byop_inner + close lookup_byop empty-key gap

Phase 5d task 1 (Phase 5c review follow-up). Extracts a private
resolve_byop_inner helper containing the decode → lookup → fetch +
validate api_key chain. lookup_byop and resolve_byop_for_local_child
both delegate.

Side effect: lookup_byop gains the same empty-key guard that
resolve_byop_for_local_child had. Pre-5d lookup_byop could return
Some(provider, "", model_id), forwarding an empty Authorization
header to the upstream. This was a latent bug in callers like
local_provider_config; the new guard closes it.

1 new test for lookup_byop's empty-key rejection.
```

---

## Stage B — Settings UI: Remote managed-secret field + Auto-create

### Task 2: Add `AISettingsPageAction` variants + handlers

**Files:**
- Modify: `app/src/settings_view/ai_page.rs`.

**Read these reference files FIRST:**
- `app/src/settings_view/ai_page.rs:~2756-2800` — existing `AddAgentProvider` / `UpdateAgentProviderName` / `ToggleAgentProviderOrchestrationAvailability` variants and handlers.
- `app/src/server/server_api/managed_secrets.rs:87` — `create_managed_secret(owner, name, secret_type, encrypted_value, description)` signature. Returns `Result<ManagedSecret>`.

- [ ] **Step 2.1: Add two new variants**

After `ToggleAgentProviderOrchestrationAvailability`, add:

```rust
    /// Phase 5d. Set or clear the BYOP provider's remote managed-secret name.
    /// Empty string means "not configured for Remote orchestration"; the
    /// provider stays Local-only.
    SetAgentProviderRemoteSecretName {
        provider_index: usize,
        name: String,
    },

    /// Phase 5d. Auto-create a personal-owner managed secret named
    /// `byop-{provider_id}` with the provider's current api_key as the value,
    /// and write the returned secret name into the provider's
    /// `remote_secret_name`. Asynchronous; surfaces a toast on success/error.
    AutoCreateAgentProviderManagedSecret {
        provider_index: usize,
    },
```

- [ ] **Step 2.2: Add the `SetAgentProviderRemoteSecretName` handler**

```rust
            AISettingsPageAction::SetAgentProviderRemoteSecretName {
                provider_index,
                name,
            } => {
                let provider_index = *provider_index;
                let name = name.clone();
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let mut providers = settings.agent_providers.value().clone();
                    if let Some(p) = providers.get_mut(provider_index) {
                        p.remote_secret_name = name;
                        report_if_error!(settings.agent_providers.set_value(providers, ctx));
                    }
                });
                ctx.notify();
            }
```

- [ ] **Step 2.3: Add the `AutoCreateAgentProviderManagedSecret` handler**

```rust
            AISettingsPageAction::AutoCreateAgentProviderManagedSecret { provider_index } => {
                let provider_index = *provider_index;
                let Some(provider) = AISettings::as_ref(ctx)
                    .agent_providers
                    .value()
                    .get(provider_index)
                    .cloned()
                else {
                    return;
                };
                let provider_id = provider.id.clone();
                let secret_name = format!("byop-{provider_id}");
                let api_key = ::ai::local_provider::AgentProviderSecrets::as_ref(ctx)
                    .get(&provider.id)
                    .map(str::to_owned)
                    .unwrap_or_default();
                if api_key.trim().is_empty() {
                    log::warn!(
                        "AutoCreate managed secret skipped for provider {provider_id}: no api_key configured"
                    );
                    return;
                }

                let ai_client = ServerApiProvider::handle(ctx).as_ref(ctx).get_ai_client();
                let view_handle = ctx.handle();
                ctx.spawn(
                    async move {
                        // Personal owner per the spec. ManagedSecretType::GenericApiKey
                        // is the default for BYOP-style keys; adjust to the enum variant
                        // your server expects.
                        ai_client
                            .create_managed_secret(
                                ::ai::secrets::SecretOwner::CurrentUser,
                                secret_name.clone(),
                                ::ai::secrets::ManagedSecretType::GenericApiKey,
                                api_key,
                                Some(format!("BYOP key for provider {provider_id}")),
                            )
                            .await
                            .map(|secret| (secret_name, secret))
                    },
                    move |view, result, ctx| {
                        let provider_index = provider_index; // capture
                        match result {
                            Ok((name, _secret)) => {
                                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                                    let mut providers = settings.agent_providers.value().clone();
                                    if let Some(p) = providers.get_mut(provider_index) {
                                        p.remote_secret_name = name;
                                        report_if_error!(
                                            settings.agent_providers.set_value(providers, ctx)
                                        );
                                    }
                                });
                                ctx.notify();
                            }
                            Err(error) => {
                                log::error!(
                                    "AutoCreate managed secret failed for provider {}: {error}",
                                    provider_id
                                );
                                // TODO(phase-5d-polish): surface a toast/snackbar
                                // here. For 5d-bridge, log-only is acceptable;
                                // the inline status indicator on the field
                                // (Task 3) surfaces the missing/failed state.
                                let _ = view;
                            }
                        }
                    },
                );
            }
```

The exact `SecretOwner::CurrentUser` / `ManagedSecretType::GenericApiKey` enum variants depend on the in-repo `ai::secrets` module. Verify the actual names by grep'ing `app/src/server/server_api/managed_secrets.rs` and adjust.

- [ ] **Step 2.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 2.5: Commit**

```
feat(settings/ai): SetAgentProviderRemoteSecretName + AutoCreateAgentProviderManagedSecret actions

Phase 5d task 2. Two new AISettingsPageAction variants:

- SetAgentProviderRemoteSecretName persists the user's typed
  secret name into AgentProvider.remote_secret_name. Wired by the
  EditorView blur/Enter event in Task 3.

- AutoCreateAgentProviderManagedSecret kicks off an async
  UpdateManager::create_managed_secret call with name
  "byop-{provider_id}", api_key from AgentProviderSecrets, and
  Personal owner. On success, writes the returned name back into
  remote_secret_name. On failure, logs and lets the inline status
  indicator (Task 3) surface the missing/failed state.
```

---

### Task 3: Render the new "Remote managed secret" field + Auto-create button

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs`.

**Read these reference files FIRST:**
- `app/src/settings_view/agent_providers_widget.rs:108-125` — `ProviderCardHandles` struct (add 1 new `EditorView` handle + 1 new `MouseStateHandle`).
- `app/src/settings_view/agent_providers_widget.rs:~575` — Phase 5b's `render_orchestration_toggle` helper; place the new field after the toggle.
- `app/src/settings_view/agent_providers_widget.rs:~1235` — `render_provider_card` orchestration site.
- `app/src/ai/byop_orchestration_filter.rs::base_url_reachable_from_remote` — visibility predicate.

- [ ] **Step 3.1: Add new handles to `ProviderCardHandles`**

```rust
    /// Phase 5d. EditorView for the "Remote managed secret" field.
    /// Visible only when available_for_orchestration is on AND the
    /// provider's base_url is publicly reachable.
    remote_secret_name_editor: ViewHandle<EditorView>,

    /// Phase 5d. Mouse-state for the "Auto-create" button.
    auto_create_secret_button_state: MouseStateHandle,
```

Initialize both in `build_provider_card`:

```rust
            remote_secret_name_editor: ctx.add_typed_action_view(|ctx| {
                let appearance = Appearance::handle(ctx).as_ref(ctx);
                let options = single_line_editor_options(appearance, false);
                let mut editor = EditorView::single_line(options, ctx);
                editor.set_placeholder_text("byop-{provider_id}", ctx);
                editor.set_text(provider.remote_secret_name.clone(), ctx);
                editor
            }),
            auto_create_secret_button_state: MouseStateHandle::default(),
```

Subscribe to the editor's blur/Enter event to dispatch `SetAgentProviderRemoteSecretName`. Mirror the existing pattern used by `name_editor` / `base_url_editor` (search for `EditorEvent::Blurred | EditorEvent::Enter` in this file).

- [ ] **Step 3.2: Add a `render_remote_secret_field` helper**

After `render_orchestration_toggle`, add:

```rust
    /// Phase 5d. Renders the "Remote managed secret" field + Auto-create
    /// button. Visibility is gated by:
    ///   - FeatureFlag::LocalLlmProvider (delegated to call site).
    ///   - provider.available_for_orchestration == true (Phase 5b toggle).
    ///   - base_url_reachable_from_remote(&provider.base_url) — providers on
    ///     localhost / RFC1918 / `.local` are Local-only by reachability.
    fn render_remote_secret_field(
        provider: &AgentProvider,
        provider_index: usize,
        card: &ProviderCardHandles,
        label_color: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let editor_view = ChildView::new(&card.remote_secret_name_editor).finish();
        let field = field_block("Remote managed secret", editor_view, label_color, appearance);

        let auto_create_button = Self::render_card_button(
            "Auto-create".to_string(),
            card.auto_create_secret_button_state.clone(),
            AISettingsPageAction::AutoCreateAgentProviderManagedSecret { provider_index },
            appearance,
        );

        let helper = Container::new(
            Text::new(
                "Required for Remote orchestration. Skip if this provider is only used for Local."
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

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(field)
            .with_child(
                Container::new(auto_create_button)
                    .with_margin_top(4.)
                    .finish(),
            )
            .with_child(helper)
            .finish()
    }
```

- [ ] **Step 3.3: Wire the field into `render_provider_card`**

Right after the existing `orchestration_toggle` block (Phase 5b), add a conditional render:

```rust
        // ---- Remote managed secret (Phase 5d, gated on toggle + reachability) ----
        let remote_secret_field = if warp_core::features::FeatureFlag::LocalLlmProvider.is_enabled()
            && provider.available_for_orchestration
            && crate::ai::byop_orchestration_filter::base_url_reachable_from_remote(
                &provider.base_url,
            )
        {
            Some(Self::render_remote_secret_field(
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

Then thread it into `card_column`:

```rust
        if let Some(field) = remote_secret_field {
            card_column.add_child(field);
        }
```

- [ ] **Step 3.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 3.5: Commit**

```
feat(settings/ai): render BYOP Remote managed-secret field + Auto-create button

Phase 5d task 3. Renders a "Remote managed secret" text field + an
"Auto-create" button on each BYOP provider card. Visible only when
the Phase 5b "Available for orchestration" toggle is on AND the
provider's base_url passes Phase 5a's
base_url_reachable_from_remote heuristic. Localhost / RFC1918 /
`.local` providers see no field — they're Local-only by reachability.

Editor blur/Enter dispatches SetAgentProviderRemoteSecretName.
Auto-create button dispatches AutoCreateAgentProviderManagedSecret
which kicks off the async UpdateManager call.
```

---

## Stage C — Wire-shape extensions

### Task 4: Extend `AgentConfigSnapshot` + `HarnessAuthSecretsConfig`

**Files:**
- Modify: `app/src/ai/ambient_agents/task.rs`.
- Modify: `app/src/ai/ambient_agents/task_tests.rs` (or add sibling if missing).

**Read these reference files FIRST:**
- `app/src/ai/ambient_agents/task.rs:29-67` — `AgentConfigSnapshot` struct + `is_empty` method.
- `app/src/ai/ambient_agents/task.rs:146-153` — `HarnessAuthSecretsConfig` struct.

- [ ] **Step 4.1: Add 4 fields to `AgentConfigSnapshot`**

After `harness_auth_secrets` at line ~66, add:

```rust
    /// Phase 5d. BYOP endpoint URL forwarded to the Remote worker so it can
    /// route the child agent's LLM calls. Populated only when the run-wide
    /// model_id starts with `byop:` and execution_mode is Remote. Server
    /// implementations that don't yet honor this field ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byop_base_url: Option<String>,

    /// Phase 5d. BYOP API type (the wire protocol the endpoint speaks).
    /// One of `"open_ai"`, `"open_ai_resp"`, `"anthropic"`, `"gemini"`,
    /// `"ollama"`, `"deep_seek"` — the canonical names from
    /// `AgentProviderApiType`. Populated alongside `byop_base_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byop_api_type: Option<String>,

    /// Phase 5d. Forwarded `byop_compaction_model_provider_id` from Phase 4d
    /// settings, so Remote workers can route conversation compaction to a
    /// distinct provider. Populated when the user has Phase 4d configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_model_provider_id: Option<String>,

    /// Phase 5d. Forwarded `byop_compaction_model_id` from Phase 4d settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_model_id: Option<String>,
```

- [ ] **Step 4.2: Update the `is_empty` method**

Extend `is_empty` to also check the new fields:

```rust
    pub fn is_empty(&self) -> bool {
        let Self {
            name,
            environment_id,
            model_id,
            base_prompt,
            mcp_servers,
            profile_id,
            worker_host,
            skill_spec,
            computer_use_enabled,
            harness,
            harness_auth_secrets,
            byop_base_url,
            byop_api_type,
            compaction_model_provider_id,
            compaction_model_id,
        } = self;

        name.is_none()
            && environment_id.is_none()
            && model_id.is_none()
            && base_prompt.is_none()
            && mcp_servers.is_none()
            && profile_id.is_none()
            && worker_host.is_none()
            && skill_spec.is_none()
            && computer_use_enabled.is_none()
            && harness.is_none()
            && harness_auth_secrets.is_none()
            && byop_base_url.is_none()
            && byop_api_type.is_none()
            && compaction_model_provider_id.is_none()
            && compaction_model_id.is_none()
    }
```

- [ ] **Step 4.3: Add `byop_auth_secret_name` to `HarnessAuthSecretsConfig`**

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessAuthSecretsConfig {
    /// Name of a managed secret for Claude Code harness authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_auth_secret_name: Option<String>,
    /// Name of a managed secret for Codex harness authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_auth_secret_name: Option<String>,
    /// Phase 5d. Name of a managed secret containing the BYOP provider's
    /// api_key, for Remote BYOP orchestration. Populated from
    /// `AgentProvider.remote_secret_name` when the run-wide model_id is a
    /// `byop:` entry and execution_mode is Remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byop_auth_secret_name: Option<String>,
}
```

Note the addition of `#[derive(Default)]` if it's missing — the new field defaults to `None`.

- [ ] **Step 4.4: Backward-compat deserialization test**

In the sibling test file:

```rust
#[test]
fn agent_config_snapshot_deserializes_pre_5d_payload() {
    // Simulates a payload from a pre-5d server / client without the
    // BYOP and compaction fields.
    let json = r#"{
        "name": "child-1",
        "model_id": "claude-sonnet-4",
        "harness": {"type": "claude"}
    }"#;
    let snapshot: AgentConfigSnapshot =
        serde_json::from_str(json).expect("should deserialize without 5d fields");
    assert_eq!(snapshot.name.as_deref(), Some("child-1"));
    assert!(snapshot.byop_base_url.is_none());
    assert!(snapshot.byop_api_type.is_none());
    assert!(snapshot.compaction_model_provider_id.is_none());
    assert!(snapshot.compaction_model_id.is_none());
}

#[test]
fn agent_config_snapshot_round_trips_byop_fields() {
    let snapshot = AgentConfigSnapshot {
        model_id: Some("byop:prov:claude-sonnet".to_owned()),
        byop_base_url: Some("https://api.anthropic.example/v1".to_owned()),
        byop_api_type: Some("anthropic".to_owned()),
        compaction_model_provider_id: Some("prov-compact".to_owned()),
        compaction_model_id: Some("haiku".to_owned()),
        harness_auth_secrets: Some(HarnessAuthSecretsConfig {
            byop_auth_secret_name: Some("byop-prov".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: AgentConfigSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.byop_base_url.as_deref(), Some("https://api.anthropic.example/v1"));
    assert_eq!(restored.byop_api_type.as_deref(), Some("anthropic"));
    assert_eq!(restored.compaction_model_provider_id.as_deref(), Some("prov-compact"));
    assert_eq!(restored.compaction_model_id.as_deref(), Some("haiku"));
    assert_eq!(
        restored.harness_auth_secrets.unwrap().byop_auth_secret_name.as_deref(),
        Some("byop-prov")
    );
}

#[test]
fn agent_config_snapshot_is_empty_remains_true_with_default_5d_fields() {
    let snapshot = AgentConfigSnapshot::default();
    assert!(snapshot.is_empty());
}
```

- [ ] **Step 4.5: Build + tests**

```bash
cargo build -p warp 2>&1 | tail -5
cargo nextest run -p warp 'agent_config_snapshot' 2>&1 | tail -10
```

- [ ] **Step 4.6: Commit**

```
feat(ambient_agents/task): add BYOP + compaction fields to AgentConfigSnapshot

Phase 5d task 4. Wire-shape extensions for Remote BYOP orchestration:

- AgentConfigSnapshot gains four optional fields: byop_base_url,
  byop_api_type, compaction_model_provider_id, compaction_model_id.
  All Option<String>, serde-default, skip_serializing_if = "None"
  so pre-5d payloads deserialize cleanly and absent fields aren't
  serialized.

- HarnessAuthSecretsConfig gains byop_auth_secret_name for routing
  the BYOP provider's api_key through the existing managed-secret
  channel. claude_auth_secret_name / codex_auth_secret_name keep
  their semantics; the new field is the BYOP-specific siblng.

is_empty extended to check the new fields. 3 serde round-trip
tests verify backward compat + present-on-set serialization.
```

---

## Stage D — Submit-path forwarding

### Task 5: Populate the new wire fields in `launch_remote_child`

**Files:**
- Modify: `app/src/pane_group/pane/terminal_pane.rs`.

**Read these reference files FIRST:**
- `app/src/pane_group/pane/terminal_pane.rs:2047-2193` — `launch_remote_child` and the `harness_auth_secrets` mapping at line 2167.
- `app/src/ai/agent_providers/mod.rs::resolve_byop_for_local_child` — the helper to call.
- `app/src/ai/ambient_agents/task.rs` — the new fields from Task 4.
- `app/src/settings/ai.rs::AISettings` — to read `byop_compaction_model_provider_id` / `byop_compaction_model_id` for compaction inheritance.

- [ ] **Step 5.1: Resolve BYOP entry before the spawn**

In `launch_remote_child`, after the `let computer_use_enabled = ...` line (~2161) and before the `let harness_auth_secrets = ...` block (~2167), add:

```rust
    // Phase 5d. If the run-wide model_id is a BYOP entry, resolve the
    // provider + api_key from settings so we can populate the new
    // AgentConfigSnapshot fields. resolve_byop_for_local_child returns
    // None for non-BYOP ids — non-BYOP launches are unchanged.
    let byop_resolution = crate::ai::agent_providers::resolve_byop_for_local_child(ctx, &model_id);

    let (byop_base_url, byop_api_type, byop_secret_name) = match byop_resolution.as_ref() {
        Some((provider, _api_key, _byop_model_id)) => {
            let api_type_str = match provider.api_type {
                ::ai::local_provider::AgentProviderApiType::OpenAi => "open_ai",
                ::ai::local_provider::AgentProviderApiType::OpenAiResp => "open_ai_resp",
                ::ai::local_provider::AgentProviderApiType::Anthropic => "anthropic",
                ::ai::local_provider::AgentProviderApiType::Gemini => "gemini",
                ::ai::local_provider::AgentProviderApiType::Ollama => "ollama",
                ::ai::local_provider::AgentProviderApiType::DeepSeek => "deep_seek",
            };
            let secret_name = if provider.remote_secret_name.trim().is_empty() {
                None
            } else {
                Some(provider.remote_secret_name.clone())
            };
            (
                Some(provider.base_url.clone()),
                Some(api_type_str.to_string()),
                secret_name,
            )
        }
        None => (None, None, None),
    };
```

- [ ] **Step 5.2: Extend the `harness_auth_secrets` mapping to include the BYOP channel**

Replace the existing block (line ~2167):

```rust
    // Map the run-wide auth secret name into the harness-specific
    // config variant. For unsupported harnesses (Oz, OpenCode, Gemini,
    // Unknown), the secret is silently ignored — those harnesses either
    // use Warp's built-in auth (Oz) or don't currently support managed
    // secrets via this flow.
    //
    // Phase 5d: also forward the BYOP managed-secret name when a BYOP
    // resolution succeeded. The two channels coexist (a harness can
    // have both its own auth_secret and a BYOP credential channel).
    let mut harness_auth_secrets = auth_secret_name
        .as_ref()
        .filter(|name| !name.trim().is_empty())
        .and_then(|name| match orchestration_harness {
            Harness::Claude => Some(crate::ai::ambient_agents::task::HarnessAuthSecretsConfig {
                claude_auth_secret_name: Some(name.clone()),
                codex_auth_secret_name: None,
                byop_auth_secret_name: None,
            }),
            Harness::Codex => Some(crate::ai::ambient_agents::task::HarnessAuthSecretsConfig {
                claude_auth_secret_name: None,
                codex_auth_secret_name: Some(name.clone()),
                byop_auth_secret_name: None,
            }),
            Harness::Oz | Harness::OpenCode | Harness::Gemini | Harness::Unknown => None,
        });

    if let Some(byop_secret) = byop_secret_name.as_ref() {
        harness_auth_secrets = match harness_auth_secrets {
            Some(mut existing) => {
                existing.byop_auth_secret_name = Some(byop_secret.clone());
                Some(existing)
            }
            None => Some(crate::ai::ambient_agents::task::HarnessAuthSecretsConfig {
                claude_auth_secret_name: None,
                codex_auth_secret_name: None,
                byop_auth_secret_name: Some(byop_secret.clone()),
            }),
        };
    }
```

- [ ] **Step 5.3: Pull compaction inheritance from AISettings**

Before the `let spawn_request = ...` block:

```rust
    // Phase 5d. Forward Phase 4d compaction settings so a Remote worker
    // can route conversation compaction to a distinct provider/model.
    // Empty strings → None so the server picks its own.
    let (compaction_model_provider_id, compaction_model_id) = {
        let settings = crate::settings::AISettings::as_ref(ctx);
        let pid = settings.byop_compaction_model_provider_id.to_string();
        let mid = settings.byop_compaction_model_id.to_string();
        let pid_opt = if pid.is_empty() { None } else { Some(pid) };
        let mid_opt = if mid.is_empty() { None } else { Some(mid) };
        (pid_opt, mid_opt)
    };
```

(Verify the exact `byop_compaction_model_provider_id` accessor by reading `app/src/settings/ai.rs` — the type may be a `Setting<String>` so you'd use `.value()` / `.value_string()` instead of `.to_string()` on the Setting itself.)

- [ ] **Step 5.4: Populate the four new fields on `AgentConfigSnapshot`**

In the existing `SpawnAgentRequest { config: Some(AgentConfigSnapshot { … ..Default::default() }), … }` block, add the new fields:

```rust
        config: Some(AgentConfigSnapshot {
            name: agent_name,
            environment_id,
            model_id: (!model_id.is_empty()).then_some(model_id),
            worker_host: (!worker_host.is_empty()).then_some(worker_host),
            computer_use_enabled,
            harness: harness_override,
            harness_auth_secrets,
            byop_base_url,
            byop_api_type,
            compaction_model_provider_id,
            compaction_model_id,
            ..Default::default()
        }),
```

- [ ] **Step 5.5: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -10
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 5.6: Commit**

```
feat(pane_group): forward BYOP + compaction config to Remote workers

Phase 5d task 5. Submit-path wiring for Remote BYOP orchestration.
launch_remote_child now:

- Resolves the BYOP entry from settings when the run-wide model_id
  starts with byop: (via resolve_byop_for_local_child, shared with
  the Phase 5c Local path).
- Maps AgentProviderApiType to the canonical wire string
  (open_ai / open_ai_resp / anthropic / gemini / ollama / deep_seek)
  for byop_api_type on the wire.
- Routes the provider's remote_secret_name through the new
  byop_auth_secret_name channel on HarnessAuthSecretsConfig,
  coexisting with the existing per-harness auth-secret channels.
- Forwards Phase 4d compaction settings via
  compaction_model_provider_id / compaction_model_id on
  AgentConfigSnapshot when configured.

Non-BYOP Remote launches are unchanged (resolution returns None,
the four new fields stay None, harness_auth_secrets gets the same
per-harness shape as before 5d).
```

---

## Stage E — Tests

### Task 6: Submit-path integration tests

**Files:**
- Create: `app/src/pane_group/pane/terminal_pane_byop_remote_tests.rs` (or extend the existing test module — check what's there first).
- Wire via `#[cfg(test)] #[path = "..."] mod tests;` in `terminal_pane.rs`.

**Read these reference files FIRST:**
- `app/src/pane_group/pane/terminal_pane.rs` end of file — check if there's already a `mod tests` reference.
- Phase 5c's `app/src/ai/blocklist/action_model/execute/run_agents_tests.rs` for the test boilerplate pattern with `App::test`, BYOP provider setup, and `llm_id::encode`.

- [ ] **Step 6.1: Add submit-path tests**

The tests target `launch_remote_child` indirectly by checking that the constructed `SpawnAgentRequest.config` has the BYOP fields populated when expected. Because `launch_remote_child` is private and operates on a `PaneGroup` view context, the cleanest test surface is to extract the BYOP-resolution logic into a small pure helper that the test can call directly. Decide between:

**Option A:** Extract a `fn resolve_byop_for_remote_child(ctx, model_id) -> (Option<base_url>, Option<api_type>, Option<secret_name>)` helper next to `resolve_byop_for_local_child`. Test that.

**Option B:** Add a test that drives a full `App::test` through the entire `launch_remote_child` path and inspects the resulting child task's config.

**Prefer Option A** — it's cleaner, faster, and tests the actual logic without dragging in the entire view tree. Add it to `app/src/ai/agent_providers/mod.rs`:

```rust
/// Phase 5d. Wraps `resolve_byop_for_local_child` + AgentProviderApiType
/// → wire-string conversion + `remote_secret_name` extraction in the form
/// the orchestration Remote submit path consumes. Returns a triple of
/// (base_url, api_type, secret_name), each `Option<String>`. All three
/// are `None` for non-BYOP ids; secret_name is `None` for BYOP entries
/// whose provider has an empty `remote_secret_name`.
pub fn resolve_byop_for_remote_child(
    app: &AppContext,
    model_id: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some((provider, _api_key, _byop_model_id)) = resolve_byop_inner(
        app,
        &(model_id.into()),
    ) else {
        return (None, None, None);
    };

    let api_type_str = match provider.api_type {
        ::ai::local_provider::AgentProviderApiType::OpenAi => "open_ai",
        ::ai::local_provider::AgentProviderApiType::OpenAiResp => "open_ai_resp",
        ::ai::local_provider::AgentProviderApiType::Anthropic => "anthropic",
        ::ai::local_provider::AgentProviderApiType::Gemini => "gemini",
        ::ai::local_provider::AgentProviderApiType::Ollama => "ollama",
        ::ai::local_provider::AgentProviderApiType::DeepSeek => "deep_seek",
    };
    let secret_name = if provider.remote_secret_name.trim().is_empty() {
        None
    } else {
        Some(provider.remote_secret_name.clone())
    };
    (
        Some(provider.base_url.clone()),
        Some(api_type_str.to_owned()),
        secret_name,
    )
}
```

Then refactor `launch_remote_child` from Task 5 to call this helper instead of inlining the `match` (smaller diff, single source of truth).

Add tests in `mod_tests.rs`:

```rust
#[test]
fn resolve_byop_for_remote_child_returns_anthropic_wire_shape() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-a".to_owned(),
                name: "Anth".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::Anthropic,
                base_url: "https://api.anthropic.example/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("claude-sonnet".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: "byop-prov-a".to_owned(),
            };
            settings.agent_providers.set_value(vec![provider], ctx).unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-a", "sk-test".to_string(), ctx);
        });

        let encoded = llm_id::encode("prov-a", "claude-sonnet").to_string();
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));

        assert_eq!(base_url.as_deref(), Some("https://api.anthropic.example/v1"));
        assert_eq!(api_type.as_deref(), Some("anthropic"));
        assert_eq!(secret_name.as_deref(), Some("byop-prov-a"));
    });
}

#[test]
fn resolve_byop_for_remote_child_omits_secret_when_empty() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-b".to_owned(),
                name: "OAI".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.openai.example/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("gpt-4o".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings.agent_providers.set_value(vec![provider], ctx).unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-b", "sk-test".to_string(), ctx);
        });
        let encoded = llm_id::encode("prov-b", "gpt-4o").to_string();
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));
        assert!(base_url.is_some());
        assert_eq!(api_type.as_deref(), Some("open_ai"));
        // No remote_secret_name → returns None even though resolution succeeded.
        assert!(secret_name.is_none());
    });
}

#[test]
fn resolve_byop_for_remote_child_returns_all_none_for_non_byop_id() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, "claude-sonnet-4"));
        assert!(base_url.is_none());
        assert!(api_type.is_none());
        assert!(secret_name.is_none());
    });
}

#[test]
fn resolve_byop_for_remote_child_returns_all_none_for_missing_provider() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);
        let encoded = llm_id::encode("missing-prov", "m1").to_string();
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));
        assert!(base_url.is_none());
        assert!(api_type.is_none());
        assert!(secret_name.is_none());
    });
}

// All six api_types should round-trip to a stable wire string.
#[test]
fn resolve_byop_for_remote_child_maps_every_api_type() {
    let cases = [
        (AgentProviderApiType::OpenAi, "open_ai"),
        (AgentProviderApiType::OpenAiResp, "open_ai_resp"),
        (AgentProviderApiType::Anthropic, "anthropic"),
        (AgentProviderApiType::Gemini, "gemini"),
        (AgentProviderApiType::Ollama, "ollama"),
        (AgentProviderApiType::DeepSeek, "deep_seek"),
    ];
    for (api_type, expected) in cases {
        App::test((), |mut app| {
            let api_type = api_type;
            let expected = expected;
            async move {
                initialize_settings_for_tests(&mut app);
                app.add_singleton_model(AgentProviderSecrets::new);
                AISettings::handle(&app).update(&mut app, |settings, ctx| {
                    let provider = AgentProvider {
                        id: "p".to_owned(),
                        name: "P".to_owned(),
                        kind: AgentProviderKind::default(),
                        api_type,
                        base_url: "https://api.example.com".to_owned(),
                        models: vec![AgentProviderModel::from_id("m".to_owned())],
                        available_for_orchestration: true,
                        remote_secret_name: String::new(),
                    };
                    settings.agent_providers.set_value(vec![provider], ctx).unwrap();
                });
                AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
                    secrets.set("p", "sk".to_string(), ctx);
                });
                let encoded = llm_id::encode("p", "m").to_string();
                let (_base_url, api_type_str, _secret) =
                    app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));
                assert_eq!(api_type_str.as_deref(), Some(expected));
            }
        });
    }
}
```

- [ ] **Step 6.2: Run the new tests**

```bash
cargo nextest run -p warp 'resolve_byop_for_remote_child' 2>&1 | tail -15
```

Expected: 5 tests pass.

- [ ] **Step 6.3: Commit**

```
test(agent_providers): submit-path coverage for Remote BYOP wire shape

Phase 5d task 6. Extracts resolve_byop_for_remote_child as a pure
helper that wraps resolve_byop_inner + AgentProviderApiType-to-wire-
string conversion + remote_secret_name extraction. The Remote
submit path in launch_remote_child (Task 5) now calls this single
helper rather than inlining the match.

5 unit tests:
- happy path with Anthropic api_type + remote_secret_name set
- secret_name absent when remote_secret_name is empty
- all-None for non-BYOP model ids
- all-None for missing provider
- exhaustive api_type → wire-string mapping (all 6 variants)
```

---

## Stage F — Docs

### Task 7: Update `specs/multi-local-llm/README.md`

**Files:**
- Modify: `specs/multi-local-llm/README.md`.

- [ ] **Step 7.1: Append Phase 5d status block** after Phase 5c, mirror the format. List the client-side scope clearly and call out worker-side as a separate task. Final commit SHA placeholder for now; fill at commit time.

- [ ] **Step 7.2: Add status-table row** for 5d.

- [ ] **Step 7.3: Add "What landed" bullet.**

- [ ] **Step 7.4: Add "Architecture" bullet.**

- [ ] **Step 7.5: Update Future-phases section** — Phase 5 series complete on the client side; flag worker-side as a server-team follow-up.

- [ ] **Step 7.6: Commit**

```
docs(specs/multi-local-llm): record Phase 5d code-complete status

Phase 5d closes the client-side surface for Remote BYOP orchestration:
Settings UI for remote_secret_name + Auto-create button, four new
fields on AgentConfigSnapshot (byop_base_url, byop_api_type,
compaction_model_*), byop_auth_secret_name on HarnessAuthSecretsConfig,
and submit-path wiring in launch_remote_child.

Worker-side BYOP routing (the part that makes the remote child
actually talk to the user's endpoint at runtime) depends on
warp.dev / Namespace server-team work and is tracked separately.
End-to-end Remote BYOP runs gated on that integration.
```

---

## Stage G — Memory

### Task 8: Memory entry

- [ ] **Step 8.1:** Write `multi-local-llm-phase-5d.md` summarizing the same content as the README. List all implementation commits.
- [ ] **Step 8.2:** Add the one-line index entry to `MEMORY.md`.
- [ ] **Step 8.3:** No git commit (outside repo).

---

## Self-review checklist

1. **Spec coverage:** Settings UI (Tasks 2-3), wire shape (Task 4), submit path (Task 5), tests (Tasks 1 + 6), docs (Tasks 7-8). Phase 5c follow-ups closed (Task 1). Worker-side explicitly deferred.

2. **Placeholder scan:** None. Every code block is complete.

3. **Type consistency:** `AgentConfigSnapshot` field names match across Task 4 (declaration), Task 5 (population), Task 6 (test). `HarnessAuthSecretsConfig.byop_auth_secret_name` shape consistent. `AISettingsPageAction` variants match between Task 2 (declaration) and Task 3 (dispatch).

4. **Backward compatibility:** All four new wire fields are `Option<T>` with serde-default + skip-if-None. Pre-5d payloads deserialize cleanly. New `harness_auth_secrets.byop_auth_secret_name` is also Option + skip-if-None — old servers ignore it.

5. **Test coverage:**
   - Task 1: 1 new test (lookup_byop empty-key)
   - Task 4: 3 new serde tests (backward compat + round-trip + is_empty)
   - Task 6: 5 new resolution tests
   - Total: 9 new tests for Phase 5d.

6. **Phase 5c follow-ups absorbed:** Task 1 cleans up both review notes from Phase 5c (lookup_byop empty-key + DRY refactor) so the Remote bridge in Task 5 uses the same shared helper.

---

## Plan complete

Plan complete and saved to `specs/multi-local-llm/plan-phase-5d.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task with two-stage review.

**2. Inline Execution** — batched in this session.

Which approach?
