# Multi-Local-LLM — Phase 1b-2 (Dispatch + Migration) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire BYOP dispatch end-to-end. After this PR, the user's existing single-provider config is **migrated once** into a `Vec<AgentProvider>` entry under `agents.warp_agent.providers`, the API key is moved from the `LocalProviderApiKey` keychain blob into a new `AgentProviderSecrets` map keyed by provider id, conversation `LLMId`s are rewritten from `local:<model>` → `byop:<uuid>:<model>`, and dispatch routes `byop:` IDs through the existing `local_provider::run_chat_turn` path. The settings UI is unchanged (Phase 1b-3 rebuilds the widget).

**Architecture:** Three logical stages, atomic in one PR (the data-shape change, the migration, and the dispatch wiring are mutually dependent — splitting them would leave intermediate states broken):

- **Stage A (Tasks 1-2)** — refactor `AgentProviderSecrets` from a single-key singleton to a `HashMap<provider_id, api_key>` keyed by string. Move the keychain blob from `LocalProviderApiKey` to `AgentProviderSecrets`. The legacy keychain entry is read once at load time as a transitional fallback (a stable `__legacy__` placeholder id).
- **Stage B (Tasks 3-4)** — write the migration helper that synthesizes one `AgentProvider` from existing `local_provider_*` settings, generates a UUID, moves the API key from the `__legacy__` placeholder to the UUID, rewrites persisted conversation `LLMId`s, and sets the marker `legacy_local_provider_migrated = true`. Idempotent.
- **Stage C (Tasks 5-7)** — add `app/src/ai/agent_providers/mod.rs` with `lookup_byop`, `build_byop_llm_infos`, `build_byop_models_by_feature`. Wire dispatch in `app/src/ai/agent/api/impl.rs` to branch on `byop:` prefix. Replace the `local:` picker injection with the BYOP enumeration. Add conversation-load-time `local:` → `byop:` rewrite as a safety net for any stragglers.

**Branch:** `multi-local-llm`. HEAD `9aa3644e` (end of 1b-1 + clippy fix). 13 commits ahead of `nmehta/local-llm-provider`.

**Spec references:**
- `specs/multi-local-llm/design.md` §1 (data model), §5 (migration), §6 (settings UI), §8 (naming changes), §10 (test plan)
- openwarp source verbatim:
  - `git show openwarp:app/src/ai/agent_providers/secrets.rs`
  - `git show openwarp:app/src/ai/agent_providers/mod.rs` (lookup_byop + build_byop_*)

**Estimated scope:** ~7 tasks, 7 atomic commits, ~600 lines net code, ~3-4 hours of subagent-driven work.

---

## File map

**Files modified:**
- `crates/ai/src/local_provider/agent_provider_secrets.rs` — Task 1 (struct refactor; new keychain key; legacy-fallback load).
- `app/src/ai/local_provider_config.rs` — Task 1 (call site update: `key()` → `get("__legacy__")`).
- `app/src/settings/ai.rs` — Task 3 (one new setting marker for the migration). Task 5 (no edit; consumer of new agent_providers field already in 1b-1).
- `app/src/ai/agent/api/impl.rs` — Task 6 (dispatch branch on `byop:`).
- `app/src/lib.rs` — Task 4 (wire migration kick-off + new module registration).
- `app/src/ai/local_provider_config.rs` — Task 5 (replace `inject_local_provider_choice` with `agent_providers::build_byop_models_by_feature` integration; keep legacy `is_local_llm_id` for fallback during transition).
- `app/src/ai/agent_conversations_model.rs` (or wherever conversation rows are loaded — Task 7) — Task 7 (load-time `local:` → `byop:` LLMId rewrite).

**Files created:**
- `app/src/ai/agent_providers/mod.rs` (Task 5) — `lookup_byop`, `build_byop_llm_infos`, `build_byop_models_by_feature`. Verbatim port from openwarp with the `attachment_caps` reference dropped (Phase 4c brings that back).
- `app/src/ai/agent_providers/migration.rs` (Task 4) — `migrate_legacy_local_provider_if_needed(app)`. Idempotent.
- `crates/ai/src/local_provider/agent_provider_secrets_tests.rs` (Task 2, sibling test file).
- `app/src/ai/agent_providers/migration_tests.rs` (Task 4, sibling test file).
- `app/src/ai/agent_providers/lookup_tests.rs` (Task 6 — tests for `lookup_byop` failure modes).

**Cargo deps:** `serde_json` is already in both crates' `Cargo.toml` (used elsewhere). No new deps.

**Out of Phase 1b-2 (deferred):**
- Settings UI rebuild (Phase 1b-3 — `AgentProvidersWidget` body still renders a single-provider form).
- Removal of legacy `agents.local_provider.*` settings + `LocalProviderApiKey` keychain entry (Phase 1b-4).
- Native Anthropic / Gemini / Ollama / DeepSeek adapters (Phase 3).
- `models.dev` catalog (Phase 4b).
- Multimodal capability fields wiring (Phase 4c).

---

## Stage A: Secrets HashMap refactor

### Task 0: Pre-flight

- [ ] **Step 0.1: Confirm clean baseline**

```bash
git rev-parse --abbrev-ref HEAD          # multi-local-llm
git status --short                       # only .claude/ and .omc/ untracked
git log --oneline -1                     # 9aa3644e
cargo nextest run -p ai 2>&1 | tail -3   # 320 tests run: 320 passed
```

If anything diverges, STOP and report.

### Task 1: Refactor `AgentProviderSecrets` to HashMap

**Files:**
- Modify: `crates/ai/src/local_provider/agent_provider_secrets.rs`
- Modify: `app/src/ai/local_provider_config.rs` (call-site update)

**Reference:** `git show openwarp:app/src/ai/agent_providers/secrets.rs` — adopt verbatim, English comments substituted, with one transitional difference: openwarp starts fresh (HashMap from day 1, no legacy keychain to migrate). We need a load-time fallback that reads the legacy `LocalProviderApiKey` blob once when the new `AgentProviderSecrets` blob is absent.

- [ ] **Step 1.1: Replace the struct + persistence layer**

Open `crates/ai/src/local_provider/agent_provider_secrets.rs`. Current shape (post-1a rename): single `key: Option<String>`, keychain key `"LocalProviderApiKey"`, JSON-encoded as `StoredKey { api_key: Option<String> }`.

Target shape: `keys: HashMap<String, String>`, keychain key `"AgentProviderSecrets"`, JSON-encoded as `HashMap<String, String>`. On load:
1. Try the NEW keychain key (`"AgentProviderSecrets"`). If found, deserialize as `HashMap<String, String>`.
2. Else, try the OLD keychain key (`"LocalProviderApiKey"`). If found, deserialize as `StoredKey { api_key: Option<String> }`. If `api_key` is `Some(k)` and non-empty, populate the map with one entry: `{ "__legacy__": k }`. Persist immediately under the new key (so subsequent loads use the V2 path).
3. Else, return an empty map.

Constants:
- `pub const LEGACY_PROVIDER_PLACEHOLDER_ID: &str = "__legacy__";` — exposed for `LocalProviderConfig::snapshot_from_app` to look up the legacy entry.
- `const SECURE_STORAGE_KEY: &str = "AgentProviderSecrets";`
- `const LEGACY_SECURE_STORAGE_KEY: &str = "LocalProviderApiKey";`

Public API (replace existing `key()`):
- `pub fn get(&self, provider_id: &str) -> Option<&str>`
- `pub fn set(&mut self, provider_id: &str, api_key: String, ctx: &mut ModelContext<Self>)` — empty `api_key` removes the entry.
- `pub fn remove(&mut self, provider_id: &str, ctx: &mut ModelContext<Self>)`
- `pub fn provider_ids(&self) -> impl Iterator<Item = &str>` — used by future consumers (1b-3 widget).

The `Event` enum changes from `KeyUpdated` to `KeysUpdated` (matches openwarp). No-arg variant — listeners re-pull the full map.

Drop the `StoredKey` struct.

**Don't** add the `set_legacy(api_key: String)` method that openwarp doesn't have — we want callers to be explicit about which provider id they're touching.

- [ ] **Step 1.2: Update `LocalProviderConfig::snapshot_from_app`**

Open `app/src/ai/local_provider_config.rs`. Around line 47 the current call is:

```rust
let api_key = AgentProviderSecrets::as_ref(ctx)
    .key()
    .map(str::to_string);
```

Change to:

```rust
let api_key = AgentProviderSecrets::as_ref(ctx)
    .get(::ai::local_provider::LEGACY_PROVIDER_PLACEHOLDER_ID)
    .map(str::to_owned);
```

Add the `pub use` for `LEGACY_PROVIDER_PLACEHOLDER_ID` in `crates/ai/src/local_provider/mod.rs`.

**Why this works:** During the transition window (between 1b-2 shipping and the user's first launch where migration runs), `LocalProviderConfig::snapshot_from_app` continues to return a valid snapshot for the legacy single-provider config. After Stage B's migration runs, the placeholder entry is replaced with a UUID-keyed entry — `snapshot_from_app` then returns `api_key = None`, but by that point all conversation `LLMId`s have been rewritten to `byop:`, so the `local:` dispatch path is no longer hit.

- [ ] **Step 1.3: Build + tests + commit**

```bash
cargo build -p ai 2>&1 | tail -5
cargo build -p warp 2>&1 | tail -5
cargo nextest run -p ai 2>&1 | tail -3       # still 320/320
```

Commit:

```
refactor(ai/local_provider): AgentProviderSecrets becomes HashMap<id, key>

Phase 1b-2 stage A. Refactors the single-key singleton into a
HashMap<provider_id, api_key> backed by a new keychain blob
"AgentProviderSecrets" (matching openwarp). Legacy "LocalProviderApiKey"
blob is read once on first load and migrated into the new map under a
stable placeholder id LEGACY_PROVIDER_PLACEHOLDER_ID = "__legacy__"; the
LocalProviderConfig::snapshot_from_app call site updated to look up by
that placeholder. The legacy keychain entry is left in place for
rollback safety; Phase 1b-4 cleanup removes it.

Public API: get/set/remove/provider_ids. Event renamed
KeyUpdated -> KeysUpdated.
```

### Task 2: Tests for the HashMap refactor

**File:**
- Create: `crates/ai/src/local_provider/agent_provider_secrets_tests.rs`

- [ ] **Step 2.1: Write the test file**

Use a sibling-test pattern (per CLAUDE.md). Tests cover:
1. Empty load — fresh keychain, returns empty map.
2. V2 load — pre-existing `AgentProviderSecrets` blob deserialized correctly.
3. V1 fallback — pre-existing `LocalProviderApiKey` blob (StoredKey shape) loaded as `__legacy__` entry; immediately persisted under V2 key.
4. V1 fallback when `api_key` is `None` — empty map returned, no V2 write.
5. Both V1 and V2 present — V2 wins, V1 ignored.
6. `set` / `get` / `remove` round-trip.
7. `set` with empty string removes the entry.
8. Event emitted on `set` and `remove`.

These tests need an in-memory `secure_storage` mock. The existing `crates/ai/src/api_keys.rs` `ApiKeyManager` test pattern is the model — see how it's tested today and mirror.

If a real `secure_storage` test harness doesn't exist yet, document this as a `BLOCKED` and request guidance — do not invent a half-working mock.

- [ ] **Step 2.2: Wire `#[cfg(test)] #[path = "..."] mod tests;` into the production file**

In `agent_provider_secrets.rs`, append:

```rust
#[cfg(test)]
#[path = "agent_provider_secrets_tests.rs"]
mod tests;
```

- [ ] **Step 2.3: Run + commit**

```bash
cargo nextest run -p ai 2>&1 | tail -3       # 328/328 (320 + 8 new)
```

Commit:

```
test(ai/local_provider): cover AgentProviderSecrets HashMap + V1 fallback

8 unit tests covering: empty load, V2 deserialization, V1 fallback (with
and without api_key), V1+V2 coexistence, set/get/remove round trips,
event emission.

Phase 1b-2 stage A.
```

---

## Stage B: Provider migration

### Task 3: Add migration marker setting

**File:**
- Modify: `app/src/settings/ai.rs`

- [ ] **Step 3.1: Add the marker**

Locate the `define_settings_group!(AISettings, settings: [ ... ])` block. After the 9 BYOP markers added in 1b-1, append:

```rust
    // Set to `true` after the one-time migration of legacy
    // agents.local_provider.* config into agents.warp_agent.providers
    // has run. Stops migration from re-running on subsequent launches.
    legacy_local_provider_migrated: LegacyLocalProviderMigrated {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.migration.legacy_local_provider_migrated",
        description: "Set after the one-time legacy local-provider config migration runs.",
    }
```

- [ ] **Step 3.2: Build + commit**

```bash
cargo build -p warp 2>&1 | tail -5
cargo nextest run -p ai 2>&1 | tail -3
```

Commit:

```
feat(settings/ai): add legacy_local_provider_migrated marker (Phase 1b-2)

One bool setting at agents.warp_agent.migration.legacy_local_provider_migrated.
Set by Stage B's migration helper after the one-time conversion of
agents.local_provider.* into the BYOP shape. Defaults to false so
migration runs once on first launch with this build; idempotent
re-runs no-op when set.
```

### Task 4: Migration helper

**Files:**
- Create: `app/src/ai/agent_providers/mod.rs` (skeleton — full body lands in Task 5).
- Create: `app/src/ai/agent_providers/migration.rs`
- Create: `app/src/ai/agent_providers/migration_tests.rs`
- Modify: `app/src/lib.rs` (kick off migration after singletons are registered).

- [ ] **Step 4.1: Skeleton `mod.rs`**

Create `app/src/ai/agent_providers/mod.rs` with just:

```rust
//! User-configured Agent providers (BYOP).
//!
//! Phase 1b-2 ships migration + dispatch routing. Phase 1b-3 brings the
//! settings widget. Phase 4 brings the models.dev catalog and native
//! adapters for non-OpenAI protocols.

pub mod migration;
```

Wire `pub mod agent_providers;` into `app/src/ai/mod.rs` in alphabetical order (between `agent` and `agent_sdk` — verify with `grep -n "pub mod " app/src/ai/mod.rs`).

- [ ] **Step 4.2: Migration helper**

Create `app/src/ai/agent_providers/migration.rs`. The function `migrate_legacy_local_provider_if_needed(app: &mut AppContext)` does:

1. Check marker `LegacyLocalProviderMigrated`. If true → no-op return.
2. Read legacy fields:
   - `local_provider_enabled` — if false AND `local_provider_base_url` is empty AND `agent_providers` is empty → set marker (nothing to migrate) and return.
   - `local_provider_display_name`, `local_provider_base_url`, `local_provider_model_id`, `local_provider_supports_tools`, `local_provider_context_window` (parse to u32, 0 if empty/invalid).
3. If `agent_providers` already has entries → set marker (user already has BYOP config) and return without overwriting.
4. Read API key from `AgentProviderSecrets`'s `__legacy__` entry. If absent, log warning and continue (provider entry is created without a key; user will be prompted to re-enter via Phase 1b-3 widget).
5. Generate UUID v4: `let provider_id = uuid::Uuid::new_v4().to_string();`.
6. Build `AgentProvider`:

   ```rust
   AgentProvider {
       id: provider_id.clone(),
       name: if display_name.is_empty() { "Local".to_owned() } else { display_name },
       kind: AgentProviderKind::OpenAiCompatible,
       api_type: AgentProviderApiType::OpenAi,
       base_url,
       models: vec![AgentProviderModel {
           id: model_id.clone(),
           name: model_id.clone(),
           context_window,
           max_output_tokens: 0,
           reasoning: false,
           tool_call: supports_tools,
           image: None,
           pdf: None,
           audio: None,
       }],
   }
   ```
7. Append to `agent_providers` Vec setting (use `Setting::set`).
8. Move the keychain entry: `secrets.remove("__legacy__")` + `secrets.set(&provider_id, api_key)`.
9. Set `byop_last_used_model_id` to `byop:<provider_id>:<model_id>` (so the picker shows the migrated provider as the default for new conversations).
10. Set marker `LegacyLocalProviderMigrated = true`.
11. Log: `log::info!("Migrated legacy local-provider config into BYOP entry {provider_id}");`.

**Note on conversation rewrite:** Migration of conversation `LLMId`s (`local:<model>` → `byop:<provider_id>:<model>`) is handled at conversation-load time in Task 7, not here. Reason: doing it here requires walking the SQLite-persisted conversation rows, which is risky and out of scope for a settings-side migration. Doing it lazily on load is safer and idempotent.

- [ ] **Step 4.3: Tests for migration**

Sibling `migration_tests.rs`. Cover:
1. Marker already set → no-op (idempotent).
2. Empty legacy config → marker set, no provider added.
3. Populated legacy config → exactly one `AgentProvider` added with expected fields, secret moved from `__legacy__` to UUID, marker set, `byop_last_used_model_id` populated.
4. `agent_providers` non-empty (user already has BYOP) → no overwrite, marker set.
5. Re-run after success → no-op.
6. Re-run after partial failure (e.g., marker not yet set but provider already added) → no duplicate.

These tests need an `AppContext` test harness with `AISettings` + `AgentProviderSecrets` singletons registered, plus the legacy `LocalProviderApiKey` keychain entry seeded. Look at `app/src/workspace/view_test.rs::initialize_app` for the singleton registration pattern.

- [ ] **Step 4.4: Wire migration kick-off in `app/src/lib.rs`**

Find where `AgentProviderSecrets` is registered as a singleton (Phase 1a put this around line 1275-1277). After both `AISettings` and `AgentProviderSecrets` are registered, call:

```rust
crate::ai::agent_providers::migration::migrate_legacy_local_provider_if_needed(ctx);
```

Verify the call ordering doesn't deadlock or trigger recursive singleton registrations.

- [ ] **Step 4.5: Build + commit**

```bash
cargo build -p warp 2>&1 | tail -5
cargo nextest run -p ai -p warp 2>&1 | tail -10
```

Commit:

```
feat(ai/agent_providers): one-time migration of legacy local-provider config

Phase 1b-2 stage B. Adds the migration helper + sibling tests + lib.rs
kick-off. On first launch with this build, if
agents.local_provider.enabled was true (or base_url non-empty) AND no
existing agent_providers entries, synthesize one AgentProvider with a
fresh UUID, copy its API key from the __legacy__ keychain placeholder
to the UUID, set byop_last_used_model_id, set the
legacy_local_provider_migrated marker. Idempotent — re-runs no-op on
the marker.

Conversation LLMId rewrite (local:<model> -> byop:<uuid>:<model>) is
deferred to Task 7 (load-time fallback).
```

---

## Stage C: Dispatch + lookup + picker

### Task 5: Add `lookup_byop` + `build_byop_*` to `agent_providers` module

**File:**
- Modify: `app/src/ai/agent_providers/mod.rs` (replace the skeleton from Task 4 with the full body).

**Reference:** `git show openwarp:app/src/ai/agent_providers/mod.rs` — copy `lookup_byop`, `build_byop_llm_infos`, `build_byop_models_by_feature`, `placeholder_llm_info` verbatim, with these differences:
- Drop the `attachment_caps::resolve_for_model` call inside `build_byop_llm_infos`. Phase 4c brings that back. Replace the `vision_supported` field with `false` for now (multimodal off until Phase 4c).
- English comments substituted for the Chinese ones.
- The placeholder text `"未配置自定义提供商 — 请到 设置 → AI 添加"` becomes `"No custom providers configured — add one in Settings → AI"`.

- [ ] **Step 5.1: Replace `mod.rs`**

The full body covers `build_byop_llm_infos`, `placeholder_llm_info`, `build_byop_models_by_feature`, and `lookup_byop`. Imports: `std::collections::HashMap`, `settings::Setting`, `warpui::{AppContext, SingletonEntity}`, the local_provider's `llm_id`, and types from `crate::ai::llms` and `crate::settings::{AISettings, AgentProvider}`.

The `LLMInfo` shape needs every field it had before in the openwarp source. Verify against `crate::ai::llms::LLMInfo` definition that the field names match — they should, since we adopted openwarp's data shape verbatim in 1b-1.

`lookup_byop`:

```rust
pub fn lookup_byop(app: &AppContext, id: &ai::LLMId)
    -> Option<(AgentProvider, String, String)>
{
    let (provider_id, model_id) = local_provider::llm_id::decode(id)?;
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let provider = providers.into_iter().find(|p| p.id == provider_id)?;
    let api_key = AgentProviderSecrets::as_ref(app)
        .get(&provider_id)
        .map(str::to_owned)?;
    Some((provider, api_key, model_id))
}
```

Note `AgentProviderSecrets` is now imported from `crate::ai::local_provider::AgentProviderSecrets` (it lives in the `ai` crate, not in `app/src/ai/agent_providers/`).

- [ ] **Step 5.2: Tests for `lookup_byop`**

Sibling `lookup_tests.rs`. Cover:
1. Success — populated provider + secret.
2. Missing provider — returns `None`.
3. Missing model match (provider exists but model id doesn't) — `lookup_byop` per its current shape doesn't validate model_id, just splits the LLMId. So this case returns `Some(...)` with a `model_id` the upstream will reject. Document this as the design choice and don't test it.
4. Missing key — returns `None`.
5. Malformed `LLMId` (legacy `local:` or non-byop) — returns `None`.

- [ ] **Step 5.3: Build + commit**

Commit:

```
feat(ai/agent_providers): add lookup_byop + build_byop_models_by_feature

Phase 1b-2 stage C. Ports openwarp's BYOP picker enumeration and the
lookup function used by dispatch:

  lookup_byop(app, &llm_id) -> Option<(AgentProvider, api_key, model_id)>

Drops the multimodal attachment_caps reference (Phase 4c brings that
back). Picker entries get vision_supported = false unconditionally until
4c lands. The placeholder LLMInfo shows when no providers are
configured.
```

### Task 6: Wire dispatch in `agent/api/impl.rs`

**File:**
- Modify: `app/src/ai/agent/api/impl.rs`

- [ ] **Step 6.1: Add the `byop:` branch**

Around the existing dispatch logic (top of `generate_multi_agent_output` or equivalent — grep for `is_local_llm_id` and `local_provider_config`). Currently:

```rust
if let Some(cfg) = params.local_provider_config.take() {
    return route_to_local_provider(params, cfg, cancellation_rx).await;
}
if crate::ai::local_provider_config::is_local_llm_id(&params.model) {
    /* fallback safety check */
}
// fallthrough: cloud path
```

Add a new branch BEFORE both existing checks:

```rust
if crate::ai::local_provider::llm_id::is_byop(&params.model) {
    let Some((provider, api_key, model_id)) =
        crate::ai::agent_providers::lookup_byop(ctx_or_app, &params.model)
    else {
        // Provider/model/key missing — surface a structured error so the
        // conversation pane shows "Provider unavailable, pick another".
        return Err(ApiError::InvalidApiKey);
    };
    // Build a runtime config snapshot from (provider, model_id, api_key)
    // and route through existing local_provider::run_chat_turn.
    let runtime_cfg = build_runtime_config(provider, &model_id, api_key);
    params.local_provider_config = Some(runtime_cfg);
    return route_to_local_provider(params, runtime_cfg, cancellation_rx).await;
}
```

`build_runtime_config` is a small adapter — likely 10-15 lines — that maps `(AgentProvider, AgentProviderModel, api_key)` to the existing `LocalProviderConfig` shape. Define it in `app/src/ai/local_provider_config.rs` or inline in `impl.rs`. Reuse the existing `LocalProviderConfig` struct verbatim — no new type.

- [ ] **Step 6.2: Build + tests + commit**

Tests for this dispatch routing live in the existing `app/src/ai/agent/api/impl_tests.rs` — extend with one or two new cases (byop dispatch routes correctly, missing provider returns InvalidApiKey).

Commit:

```
feat(ai/agent/api): dispatch byop: LLMIds through local_provider runtime

Phase 1b-2 stage C. Adds a new branch in generate_multi_agent_output:
when params.model is a `byop:<provider_id>:<model_id>` LLMId, look up
the provider+key via agent_providers::lookup_byop, build a
LocalProviderConfig snapshot, and route through the existing
local_provider::run_chat_turn path. Missing provider/key returns
InvalidApiKey for the conversation pane to surface.

The legacy `local:` branch is preserved during the transition window —
unmigrated users (i.e., before migration runs) continue routing through
LocalProviderConfig::snapshot_from_app.
```

### Task 7: Picker injection swap + conversation-load LLMId rewrite

**Files:**
- Modify: `app/src/ai/local_provider_config.rs` — replace `inject_local_provider_choice` callers to use `agent_providers::build_byop_models_by_feature`. Keep `is_local_llm_id` as a small helper for the safety net.
- Modify: `app/src/ai/agent_conversations_model.rs` (or wherever conversation rows are deserialized — grep first to confirm location).

- [ ] **Step 7.1: Swap picker injection**

Find every caller of `inject_local_provider_choice`. They're in the conversation/picker path. Replace with `agent_providers::build_byop_models_by_feature(app)` integration. The legacy `synthetic_llm_info` becomes dead code — delete the function.

`is_local_llm_id` stays for one purpose only: detecting unmigrated `LLMId`s during conversation load (Step 7.2).

- [ ] **Step 7.2: Conversation-load `local:` → `byop:` rewrite**

When loading a persisted conversation:
1. Decode the `LLMId`.
2. If `is_local_llm_id(&llm_id)`:
   a. If `legacy_local_provider_migrated == true`, look up the migrated provider's UUID. Find it by reading `agent_providers` and matching name + base_url against the legacy `local_provider_*` settings (or simpler: pick the first provider, since migration creates exactly one).
   b. Re-encode as `byop:<uuid>:<rest>` where `<rest>` is the stripped `local:` model id.
   c. Persist the rewritten LLMId back to the conversation row.
3. Else (already `byop:` or cloud), no-op.

If migration has not yet run, leave the `local:` ID intact — the next launch will run migration and the rewrite happens on subsequent loads.

- [ ] **Step 7.3: Tests**

Conversation-load rewrite tests in `agent_conversations_model_tests.rs` (sibling pattern):
1. Pre-migration: conversation has `local:<model>` ID → loaded as-is.
2. Post-migration: conversation has `local:<model>` ID → rewritten to `byop:<uuid>:<model>`, persisted.
3. Already-byop: no rewrite, no persist.

- [ ] **Step 7.4: Build + commit**

Commit:

```
feat(ai/agent_providers): swap picker + rewrite legacy LLMIds on load

Phase 1b-2 stage C completion. Picker enumeration now goes through
build_byop_models_by_feature instead of inject_local_provider_choice.
Conversations whose persisted LLMId still uses the legacy `local:`
prefix are rewritten to `byop:<uuid>:<rest>` at load time after
migration has run. Migration runs lazily — unmigrated users keep their
`local:` IDs intact and the next launch fixes them.

End of Phase 1b-2 — BYOP dispatch is fully wired. Phase 1b-3 will
rebuild the settings widget to render the providers list (currently
the user can only have one provider, the migrated one).
```

---

## Final verification

- [ ] **Verification 1: Sweeps**

```bash
echo "=== byop: routing types in place ==="
grep -rn "lookup_byop\|build_byop_llm_infos\|build_byop_models_by_feature\|is_byop" --include="*.rs" .

echo "=== AgentProviderSecrets HashMap shape ==="
grep -n "keys: HashMap" crates/ai/src/local_provider/agent_provider_secrets.rs

echo "=== keychain key change ==="
grep -n '"AgentProviderSecrets"\|"LocalProviderApiKey"' crates/ai/src/local_provider/agent_provider_secrets.rs

echo "=== migration kick-off ==="
grep -n "migrate_legacy_local_provider_if_needed" app/src/lib.rs
```

- [ ] **Verification 2: Build + tests + clippy**

```bash
cargo build -p ai && cargo build -p warp
cargo nextest run -p ai 2>&1 | tail -3                       # 320 + new tests
cargo clippy -p ai --all-targets --all-features -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

(Workspace clippy with bin targets has the stale-build-hash issue noted in Phase 1b-1; CI presubmit uses a clean build and avoids it.)

- [ ] **Verification 3: Final reviewer + push**

Dispatch the final code reviewer (`oh-my-claudecode:code-reviewer`) for the full 7-commit Phase 1b-2 diff. Stop before push; user reviews, then pushes manually.

---

## Risks & open questions

1. **`secure_storage` test harness availability.** Task 2's tests need an in-memory `secure_storage` mock. If one doesn't exist, the implementer should report BLOCKED so we can decide between (a) building the mock as a side step, or (b) deferring secrets persistence tests to a manual smoke-test pass.

2. **Migration on a corrupted state.** If the user has `agents.local_provider.*` populated but no `LocalProviderApiKey` in keychain (e.g., they wiped it manually), migration creates an `AgentProvider` entry with no associated key. The widget in 1b-3 should show this clearly — but since 1b-3 hasn't shipped yet, the user briefly sees a provider that can't dispatch. Acceptable for the transition window.

3. **Conversation-load rewrite scope.** Task 7's rewrite logic runs on every conversation load, not just once. Performance impact should be negligible (one regex check + occasional rewrite + DB write), but if profiling shows it as hot, gate behind a "migration in progress" flag.

4. **`local:` cloud-fallback.** If a user has a stored `local:<model>` LLMId AND migration hasn't run AND `LocalProviderConfig::snapshot_from_app` returns `None` (no API key found at `__legacy__` placeholder — possibly because Task 1 cleared the legacy keychain), dispatch falls through to the cloud path with an unrecognized model id, which will fail. This window is small (between upgrade and first successful migration kick-off) but real. Mitigation: ensure migration runs eagerly during app boot (Task 4.4), before any conversation can be opened.

5. **Two simultaneous providers, same `(name, base_url)`.** Migration synthesizes a UUID, so duplicates are byte-distinct in the providers Vec. Conversation-rewrite logic that matches on `(name, base_url)` would tie-break arbitrarily. Acceptable since duplicates only arise after a second user-managed provider is added (post-1b-3).

---

## Next plan (Phase 1b-3)

After 1b-2 ships green, Phase 1b-3 will cover:
- Rebuild `AgentProvidersWidget` body in `app/src/settings_view/ai_page.rs` to render a Vec of provider cards instead of the single-provider form.
- Per-card: name, base_url, api_key, api_type chips, models table.
- Per-model row: name, id, context_window, tool_call.
- Add/remove provider, add/remove model.
- Wire the existing `AgentProviderSecrets::set/remove` API for keychain writes.
- Remove the legacy `local_provider_*` AISetting markers from the widget rendering (the markers themselves stay until 1b-4).

That plan will be written after 1b-2 is approved + executed.
