# Multi-Local-LLM — Phase 1b-4 (Legacy Cleanup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.
>
> **DO NOT EXECUTE THIS PLAN UNTIL THE GATING CONDITIONS BELOW ARE MET.** This is a deletion-only PR; running it before migration adoption is confirmed will permanently lose API keys for users who haven't yet migrated.

**Goal:** Delete all the legacy single-provider scaffolding kept as a safety net through Phases 1b-1/1b-2/1b-3. After this PR there is exactly one local-LLM dispatch path (`byop:<provider_id>:<model_id>`) with no `local:` LLMId support, no `LocalProvider*` AISetting markers, no `agents.local_provider.*` TOML schema, no V1 keychain blob, no migration helper. ~200-300 lines deleted across ~10 files.

**Architecture:** Pure deletion. Each task removes one self-contained legacy surface and verifies the build + tests stay green. The migration helper itself is removed last so its side-effects (already shipped) can't be re-triggered.

**Branch:** `multi-local-llm` (or a fresh branch off it — the implementer picks; this plan is branch-agnostic). Phases 1a, 1b-1, 1b-2, 1b-3, and the dispatch-scoping fix must already be in stable on whatever branch is used as the base.

**Spec references:**
- `specs/multi-local-llm/design.md` §6 step 3 (deprecation window) and §10 risks 3 (keychain rollback safety)
- `specs/multi-local-llm/plan-phase-1a.md` §8 ("Phase 1a — symbol-only rename")
- `specs/multi-local-llm/plan-phase-1b-2-dispatch.md` §Stage A (the V1→V2 fallback path that this PR removes)
- The TODO comment in `agent_provider_secrets.rs` at `22944c51` ("change to `return HashMap::new()` then" — that "then" is now)

**Estimated scope:** 6 tasks, 5-6 atomic commits, ~250 lines deleted (net), ~30-45 minutes to execute.

---

## ⚠️ GATING CONDITIONS — verify before starting

Phase 1b-4 must NOT ship until ALL of the following are true:

1. **Telemetry confirms migration adoption.** Count of active installs with
   `agents.warp_agent.migration.legacy_local_provider_migrated = true`
   should plateau at ≥ 99% of active local-provider users. The remaining
   < 1% are users who never had a local provider configured (so there's
   nothing to lose) OR users on stale builds (whose API keys are still in
   the V1 keychain blob and would be deleted by this PR — accept this
   risk explicitly).

2. **One full release cycle in stable** has elapsed since `v0.1.0` (the tag
   marking end of Phase 1b-3). Stable users on the prior version have
   rolled forward; their migrations have run.

3. **No open migration-failure tickets** in the support queue. If users have
   reported migration edge cases that this plan would mask (e.g. silent V2
   corruption falling through to V1 successfully), triage those first.

4. **Manual smoke test on a profile that DID NOT migrate** — to confirm
   the failure mode is "API key empty, user re-enters" not "app panics
   on load". Boot the app on a fresh user profile, do not configure
   anything, verify Settings → AI → Custom AI Providers shows an empty
   list and the picker shows the placeholder entry without crashing.

If any gating condition is unmet, **stop and surface to the user**.

---

## File map

### Files modified (deletions only)

- `app/src/settings/ai.rs` — delete 6 legacy AISetting markers + 5 `local_provider_compaction_*` markers + 2 `Toggle*` action variants. ~80 lines.
- `crates/ai/src/local_provider/agent_provider_secrets.rs` — delete `LEGACY_PROVIDER_PLACEHOLDER_ID`, `LEGACY_SECURE_STORAGE_KEY`, `LegacyStoredKey`, V1 fallback in `load_from_storage`, and the V2-corrupt fall-through. Delete the legacy V1 keychain entry on first boot after this PR. ~50 lines.
- `crates/ai/src/local_provider/mod.rs` — drop `LEGACY_PROVIDER_PLACEHOLDER_ID` from the `pub use`. ~1 line.
- `app/src/ai/local_provider_config.rs` — delete `snapshot_from_app`, `is_local_llm_id`, `synthetic_llm_info`, the legacy branches inside `snapshot_for_request` and `inject_local_provider_choice`, switch `compaction_config_from_app` from `local_provider_compaction_*` to `byop_compaction_*`. ~80 lines.
- `app/src/ai/agent/api/impl.rs` — delete the `is_local_llm_id(&params.model)` "stale local id" error-stream branch + its preamble doc comment. ~25 lines.
- `app/src/lib.rs` — delete the `migrate_legacy_local_provider_if_needed(ctx)` kick-off + its 5-line doc comment. ~7 lines.
- `app/src/ai/agent_providers/mod.rs` — drop `pub mod migration;`. 1 line.
- `app/src/ai/agent_providers/migration.rs` — delete the entire file. ~155 lines.
- `app/src/ai/agent_providers/migration_tests.rs` — delete if it exists (it doesn't in 1b-2's actual landed state — the implementer skipped tests due to scaffolding constraints — but verify before opening).
- `app/src/settings_view/agent_providers_widget.rs` — drop the `local_provider_*` AISetting type imports if any are still referenced; verify no stragglers. (Probably no edits needed — the widget was rebuilt on `agent_providers` from day one.)

### Files NOT touched

- Phase 1a renames are permanent: `AgentProviderSecrets`, `AgentProvidersWidget` stay.
- Phase 1b-1 BYOP types are permanent: `AgentProvider`, `AgentProviderModel`, `AgentProviderKind`, `AgentProviderApiType`, `byop_*` settings markers all stay.
- Phase 1b-2 dispatch is permanent: `lookup_byop`, `build_byop_models_by_feature`, `snapshot_for_request`'s BYOP branch all stay.
- Phase 1b-3 widget is permanent.
- The `legacy_local_provider_migrated` setting marker stays — it's now meaningless but cheap to leave; deleting it would require a migration step itself which defeats the cleanup's purpose. Mark it `#[allow(dead_code)]` and add a doc comment.

---

## Tasks

### Task 0: Pre-flight + gating verification

- [ ] Run `git rev-parse --abbrev-ref HEAD` and confirm working branch.
- [ ] Run `git tag --list v0.1.0` and confirm the tag exists. Run `git log --oneline v0.1.0..HEAD` to verify only acceptable changes are in flight.
- [ ] **Manually verify gating conditions 1-4 above.** Telemetry check. Stable cycle check. Support queue check. Manual smoke test on fresh profile.
- [ ] Baseline: `cargo nextest run -p ai 2>&1 | tail -3` → at least 324 (whatever the count is at the time — record it as the baseline). `cargo build -p warp` green. `cargo clippy -p ai --all-targets --all-features -- -D warnings` green.

### Task 1: Delete legacy AISetting markers

**File:** `app/src/settings/ai.rs`

Delete these markers from the `define_settings_group!(AISettings, settings: [ ... ])` block:

- `local_provider_enabled: LocalProviderEnabled`
- `local_provider_display_name: LocalProviderDisplayName`
- `local_provider_base_url: LocalProviderBaseUrl`
- `local_provider_model_id: LocalProviderModelId`
- `local_provider_supports_tools: LocalProviderSupportsTools`
- `local_provider_context_window: LocalProviderContextWindow`
- `local_provider_compaction_auto: LocalProviderCompactionAuto`
- `local_provider_compaction_prune: LocalProviderCompactionPrune`
- `local_provider_compaction_tail_turns: LocalProviderCompactionTailTurns`
- `local_provider_compaction_preserve_recent_tokens: LocalProviderCompactionPreserveRecentTokens`
- `local_provider_compaction_reserved: LocalProviderCompactionReserved`

If the migration marker `legacy_local_provider_migrated: LegacyLocalProviderMigrated` is referenced elsewhere (it shouldn't be, post-1b-4), **leave it** with an `#[allow(dead_code)]` and a comment: `// Kept for telemetry / forensics only — never read by code post-1b-4.`

Delete from `AISettingsPageAction` enum:
- `ToggleLocalProviderEnabled`
- `ToggleLocalProviderSupportsTools`

Delete the matching arms from the `match action { ... }` dispatch site.

If anything else in `ai.rs` references the deleted markers (e.g. `*ai_settings.local_provider_enabled`), the build will fail and surface the call site — fix or delete those too. Likely candidates: nothing post-1b-3 because the dispatch fix scoped everything to `byop:` / `local:` prefixes, but verify with a final `cargo build`.

**Test:** `cargo build -p warp` green. Test count unchanged.

**Commit:**
```
refactor(settings/ai): delete legacy local_provider_* setting markers

Phase 1b-4. Removes the 6 user-facing markers + 5 compaction markers +
2 toggle-action variants from the AISettings group. The migration
marker `legacy_local_provider_migrated` is kept (with #[allow(dead_code)])
for telemetry / forensics only.

agents.local_provider.* TOML schema is now fully obsolete. Existing user
TOML files with this section are silently ignored on load (settings
crate skips unrecognized keys); deleting it from settings.toml is a
manual cleanup the user can do at leisure.
```

### Task 2: Delete legacy V1 keychain fallback

**File:** `crates/ai/src/local_provider/agent_provider_secrets.rs`

Delete:
- `pub const LEGACY_PROVIDER_PLACEHOLDER_ID: &str = "__legacy__";`
- `const LEGACY_SECURE_STORAGE_KEY: &str = "LocalProviderApiKey";`
- The `LegacyStoredKey` struct.
- The V1 fallback branch in `load_from_storage` (the section after the V2 read).
- The "fall through to V1 fallback" comment block.

Switch the V2-corrupt-blob handler:

```rust
Err(e) => {
    log::error!("Failed to deserialize AgentProviderSecrets V2 blob: {e:#}");
    // Per the Phase 1b-2 → 1b-4 plan: V1 has been removed, so a corrupt
    // V2 blob now surfaces as an empty map. The user re-enters their
    // API key via Settings → AI rather than having the corruption
    // silently masked.
    return HashMap::new();
}
```

Add a one-time cleanup of the legacy keychain entry on first boot after this PR. New private fn `delete_legacy_v1_keychain_entry_if_present(ctx)` called once from `new()`:

```rust
fn delete_legacy_v1_keychain_entry_if_present(ctx: &mut ModelContext<Self>) {
    // V1 blob (LocalProviderApiKey) was kept around through Phase 1b-2/1b-3
    // for rollback safety. By Phase 1b-4 the migration adoption rate is
    // confirmed and this entry is dead weight — delete on first launch
    // post-1b-4 so we don't leak a stale credential indefinitely.
    if let Err(e) = ctx.secure_storage().remove_value("LocalProviderApiKey") {
        match e {
            secure_storage::Error::NotFound => {} // Already gone — fine.
            _ => log::warn!("Failed to clean up legacy LocalProviderApiKey keychain entry: {e:#}"),
        }
    }
}
```

Wire the call from `new()` after `load_from_storage`.

**File:** `crates/ai/src/local_provider/mod.rs` — drop `LEGACY_PROVIDER_PLACEHOLDER_ID` from the `pub use agent_provider_secrets::{...}` re-export.

**Test:** `cargo build -p ai` green. `cargo nextest run -p ai` test count unchanged.

**Commit:**
```
refactor(ai/local_provider): delete V1 keychain fallback path

Phase 1b-4. Drops LEGACY_PROVIDER_PLACEHOLDER_ID, LegacyStoredKey, and
the V1 fallback branch in load_from_storage. V2-corrupt blob now
returns an empty HashMap (per the TODO landed at 22944c51) rather than
masking corruption with stale V1 data — the user re-enters their key
via Settings → AI on next launch.

A one-shot cleanup deletes the legacy `LocalProviderApiKey` keychain
entry on first launch post-1b-4 so we don't leak a stale credential
indefinitely.

mod.rs no longer re-exports LEGACY_PROVIDER_PLACEHOLDER_ID.
```

### Task 3: Delete legacy dispatch + picker branches

**File:** `app/src/ai/local_provider_config.rs`

Delete:
- `pub fn snapshot_from_app` — entire function.
- `pub fn is_local_llm_id` — entire function.
- `pub fn synthetic_llm_info` — entire function.
- The legacy `local:` branch from `snapshot_for_request` (Path 2 in the dispatch).
- The legacy `synthetic_llm_info(&cfg)` branch from `inject_local_provider_choice` (the `if let Some(cfg) = snapshot_from_app(ctx) { ... }` block).
- Update the `purge` closure inside `inject_local_provider_choice` to drop the `is_local_llm_id` check (only `byop:` purge remains).

Replace `compaction_config_from_app` to read from `byop_compaction_*` markers exclusively:

```rust
pub fn compaction_config_from_app(
    ctx: &AppContext,
) -> ai::local_provider::compaction::CompactionConfig {
    use ai::local_provider::compaction::CompactionConfig;
    let s = AISettings::as_ref(ctx);
    let parse_optional = |raw: &str| -> Option<usize> {
        let n = raw.trim().parse::<u32>().ok()?;
        (n > 0).then_some(n as usize)
    };
    // No String<u32> wrappers on the byop_compaction_* settings — they're
    // already u32 markers. Use the raw integer values directly.
    CompactionConfig {
        auto: *s.byop_compaction_auto,
        prune: *s.byop_compaction_prune,
        tail_turns: (*s.byop_compaction_tail_turns as usize)
            .max(ai::local_provider::compaction::consts::DEFAULT_TAIL_TURNS),
        preserve_recent_tokens: parse_optional(&s.byop_compaction_preserve_recent_tokens.to_string()),
        reserved: parse_optional(&s.byop_compaction_reserved.to_string()),
    }
}
```

(Verify the actual `byop_compaction_*` field types from `app/src/settings/ai.rs` — they were defined as `u32` directly in 1b-1, no String wrappers. Adapt the closure accordingly.)

**File:** `app/src/ai/agent/api/impl.rs` — delete:

```rust
if crate::ai::local_provider_config::is_local_llm_id(&params.model) {
    // Stale local id but no active config (user disabled the provider
    // but their saved profile still references it). Surface a
    // one-shot error stream so the controller's existing toast path
    // fires; the user can re-select a Warp model.
    let (tx, rx) = async_channel::unbounded();
    let err = AIApiError::Other(anyhow::anyhow!(
        "Local provider is no longer configured. Re-enable it in settings, or pick a Warp model."
    ));
    let _ = tx.send(Err(Arc::new(err))).await;
    return Ok(Box::pin(rx));
}
```

The doc comment block above (`// ---- Custom Local LLM Provider fork (specs/GH9303/) ----`) can also be cleaned up — keep the `params.local_provider_config.take()` branch and its tighter doc; drop the now-stale "we retain the `local:` model-id signal as a separate diagnostic" sentence.

**Test:** `cargo build -p warp` green. `cargo nextest run -p ai` test count unchanged. `cargo clippy -p warp --lib --tests -- -D warnings` green.

**Commit:**
```
refactor(ai/local_provider_config): delete legacy local: dispatch path

Phase 1b-4. Drops snapshot_from_app, is_local_llm_id, synthetic_llm_info,
the local: branch from snapshot_for_request, and the local: purge from
inject_local_provider_choice. compaction_config_from_app now reads
exclusively from byop_compaction_* settings.

agent/api/impl.rs loses the "stale local: id" error-stream branch —
post-migration, no conversation should hold a local: LLMId; if any
straggler exists it falls through to the cloud-Warp path which surfaces
its own "model not found" error.
```

### Task 4: Delete migration helper + lib.rs kick-off

**Files:**
- Delete: `app/src/ai/agent_providers/migration.rs`
- Delete: `app/src/ai/agent_providers/migration_tests.rs` (if present)
- Modify: `app/src/ai/agent_providers/mod.rs` — drop `pub mod migration;`
- Modify: `app/src/lib.rs` — delete the kick-off line + its 5-line doc comment

```bash
git rm app/src/ai/agent_providers/migration.rs
[ -f app/src/ai/agent_providers/migration_tests.rs ] && git rm app/src/ai/agent_providers/migration_tests.rs
```

In `app/src/ai/agent_providers/mod.rs`, delete:
```rust
pub mod migration;
```

In `app/src/lib.rs`, delete:
```rust
    // Phase 1b-2 stage B: one-time migration of legacy `agents.local_provider.*`
    // config into the BYOP shape. Must run AFTER both AISettings (registered
    // earlier via settings::init) and AgentProviderSecrets are present — the
    // helper reads from both and writes to both. Idempotent — re-runs no-op
    // once the marker is set.
    crate::ai::agent_providers::migration::migrate_legacy_local_provider_if_needed(ctx);
```

**Test:** `cargo build -p warp` green. `cargo nextest run -p ai` test count unchanged.

**Commit:**
```
refactor(ai/agent_providers): delete migration helper + lib.rs kick-off

Phase 1b-4. The one-time legacy → BYOP config migration completed for
all active installs by the time this PR ships (see plan-phase-1b-4
gating conditions). Deletes the helper module + the kick-off in
app/src/lib.rs.

The marker setting `legacy_local_provider_migrated` is preserved in
ai.rs with #[allow(dead_code)] for telemetry/forensics only.
```

### Task 5: Final verification + sweep

- [ ] Sweep checks (all should return zero matches):

  ```bash
  grep -rn "snapshot_from_app\|is_local_llm_id\|synthetic_llm_info\|LEGACY_PROVIDER_PLACEHOLDER_ID\|LegacyStoredKey\|migrate_legacy_local_provider\|local_provider_compaction_\|LocalProviderEnabled\|LocalProviderBaseUrl\|LocalProviderModelId\|LocalProviderDisplayName\|LocalProviderContextWindow\|LocalProviderSupportsTools\|ToggleLocalProvider" --include="*.rs" .
  ```

  Expected: zero hits across the workspace. Any hits are either:
  - Legitimate forensics references in doc comments (acceptable).
  - Real call sites that should have been deleted (fix or stop and report).
  - The `legacy_local_provider_migrated` setting (preserved as dead_code; this should NOT be flagged by the above sweep since the regex doesn't match it).

- [ ] Sweep for deleted symbols showing up in error messages or tests:

  ```bash
  grep -rn '"LocalProviderApiKey"' --include="*.rs" .
  ```

  Expected: zero hits in `crates/ai/`. Any references in `app/` should be in the cleanup helper from Task 2. Verify and accept.

- [ ] `cargo build -p warp` green.
- [ ] `cargo nextest run -p ai` — 324/324 (or whatever the final baseline was) passing.
- [ ] `cargo clippy -p ai --all-targets --all-features -- -D warnings` green.
- [ ] `cargo clippy -p warp --lib --tests -- -D warnings` green.
- [ ] `cargo fmt --check` green (no drift left over).

### Task 6: Push + tag

- [ ] `git log --oneline v0.1.0..HEAD` — verify the 4-5 1b-4 commits are clean.
- [ ] `git push origin <branch>`.
- [ ] After merge to main / stable: `git tag -a v0.2.0 -m "Phase 1b-4 cleanup — single-provider scaffolding removed"` and `git push origin v0.2.0`. The tag marks the codebase's transition to BYOP-only.

---

## Risks & rollback

1. **A user who never migrated upgrades to 1b-4.** Their V1 keychain entry gets deleted by Task 2's cleanup. Their `agents.local_provider.*` settings are still in TOML but no longer read. Net effect: API key gone, settings inert; the user re-enters via the (1b-3) widget. **Acceptable** — the gating conditions ensure this is < 1% of users.

2. **A test relies on a deleted symbol.** The build will fail at compile time and surface the file:line. Fix the test (delete or update to use BYOP types).

3. **Rollback needed mid-cycle.** If 1b-4 ships and a major migration-failure issue surfaces, rollback restores the V1 fallback path but the user's V1 keychain entry has been deleted. Re-migration would require manual API key re-entry. Set expectations accordingly in the PR description.

4. **`legacy_local_provider_migrated` setting churn.** Leaving the marker as dead code is a permanent footprint in the TOML schema. Acceptable; deleting it would require its own migration step which defeats the purpose. If a future release wants to clean it up, it can ship a no-op migration that just removes the key.

---

## Rollback patches (for emergency)

If 1b-4 needs to be reverted within hours of shipping, the cleanest rollback is:

```bash
git revert <task1>..<task5>
git push origin <branch>
```

This puts the legacy paths back. Users whose V1 keychain entries were already deleted by the brief 1b-4 window won't recover the keys — they re-enter once via the widget. Sustained 1b-4 in stable for ≥ 1 week before declaring it permanent makes rollback unnecessary.

---

## After 1b-4 ships

There is no Phase 1b-5. Phase 1 of the multi-provider work is complete.

Future phases per `specs/multi-local-llm/design.md` §9:
- Phase 2: ProviderAdapter trait
- Phase 3a-d: native Anthropic / Ollama / Gemini / DeepSeek adapters
- Phase 4a-d: /models fetch, models.dev catalog, multimodal capabilities, dedicated compaction model

Each is independently scoped with its own design + plan.
