# Multi-Local-LLM — Phase 1a Implementation Plan: Mechanical Rename

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the two single-provider Rust symbols (`LocalProviderKeyManager` → `AgentProviderSecrets` and `LocalProviderWidget` → `AgentProvidersWidget`) with **zero behavior change**, so Phase 1b can extend the renamed singleton into a per-provider keychain map without bundling a confusing rename into the same diff.

**Architecture:** Pure symbol-level rename across 6 files. The keychain key string (`"LocalProviderApiKey"`), the TOML schema (`agents.local_provider.*`), the LLM ID prefix (`local:`), and the `FeatureFlag::LocalLlmProvider` flag are all **left unchanged** — those move in Phase 1b atomically with the migration helper.

**Tech Stack:** Rust, Cargo workspace, Diesel/SQLite (untouched here), `warpui` Entity/SingletonEntity pattern, `warpui_extras::secure_storage`.

**Branch:** `multi-local-llm` (already created from `nmehta/local-llm-provider` @ `64d5172a`). HEAD currently at `9a044d24` (the design spec commit).

**Spec reference:** `specs/multi-local-llm/design.md` §8 — "Phase 1a — symbol-only rename (no behavior change)" table.

**Estimated scope:** ~20-30 line touches across 6 files, two atomic commits, ~30-60 minutes.

---

## File map

**File renames:**
- `crates/ai/src/local_provider/key_manager.rs` → `crates/ai/src/local_provider/agent_provider_secrets.rs`

**Files modified (symbol references only):**
- `crates/ai/src/local_provider/mod.rs` — module declaration + re-exports
- `app/src/lib.rs` — singleton model registration (line ~1275-1277)
- `app/src/settings/ai.rs` — Task 1 doc comment only (line ~1467)
- `app/src/settings_view/ai_page.rs` — Task 1 key manager call sites (lines ~6684, ~6706, ~6811, ~6905) + Task 2 widget definition, two impls, and two instantiation sites (lines ~1515, ~1556, ~6687, ~6697, ~6941)
- `app/src/ai/local_provider_config.rs` — module-doc comment (line ~2), import (line ~13), call site (line ~47)

**Symbols UNCHANGED (Phase 1b will handle):**
- `LocalProviderConfig` (struct in `crates/ai/src/local_provider/config.rs`)
- `LocalProviderHistory`, `LocalTool` (kept by design — see spec §8)
- AISetting marker types: `LocalProviderEnabled`, `LocalProviderBaseUrl`, `LocalProviderModelId`, `LocalProviderDisplayName`, `LocalProviderContextWindow`, `LocalProviderSupportsTools`
- AISettingsPageAction variants: `ToggleLocalProviderEnabled`, `ToggleLocalProviderSupportsTools`
- Constant `SECURE_STORAGE_KEY: &str = "LocalProviderApiKey"`
- All TOML field names under `agents.local_provider.*`
- LLM ID prefix `"local:"`

---

## Task 0: Pre-flight — verify clean baseline

**Files:** none (read-only verification)

- [ ] **Step 0.1: Confirm branch and clean state**

Run:
```bash
git rev-parse --abbrev-ref HEAD
git status --short
```
Expected: branch is `multi-local-llm`; working tree shows only `.claude/scheduled_tasks.lock` and `.omc/` (both untracked, unrelated to this work).

- [ ] **Step 0.2: Baseline build (catch any pre-existing breakage before we touch code)**

Run:
```bash
cargo build -p ai 2>&1 | tail -20
```
Expected: build succeeds (`Finished ... profile`).

- [ ] **Step 0.3: Baseline tests for the local_provider module**

Run:
```bash
cargo nextest run -p ai 2>&1 | tail -15
```
Expected: all tests pass. If anything fails on a clean checkout of the branch HEAD, **stop and ask** — that's a pre-existing problem, not in scope here.

- [ ] **Step 0.4: Capture exact symbol locations (sanity check the plan)**

Run:
```bash
grep -rn "LocalProviderKeyManager\|LocalProviderWidget" --include="*.rs" .
```
Expected: ~15 hits across 5 files (key_manager.rs, mod.rs, lib.rs, settings/ai.rs, settings_view/ai_page.rs, local_provider_config.rs). If a hit appears in a file not listed in the file map above, **stop and update the plan** before proceeding.

---

## Task 1: Rename `LocalProviderKeyManager` → `AgentProviderSecrets`

**Files:**
- Rename: `crates/ai/src/local_provider/key_manager.rs` → `crates/ai/src/local_provider/agent_provider_secrets.rs`
- Modify: `crates/ai/src/local_provider/mod.rs`
- Modify: `app/src/lib.rs`
- Modify: `app/src/settings/ai.rs`
- Modify: `app/src/settings_view/ai_page.rs`
- Modify: `app/src/ai/local_provider_config.rs`

This task touches all callers in one commit because Rust won't compile after a partial rename. The keychain string `"LocalProviderApiKey"` and the constant `SECURE_STORAGE_KEY` are left untouched — Phase 1b will swap those when the on-disk format changes shape.

- [ ] **Step 1.1: Rename the file with `git mv`**

Run:
```bash
git mv crates/ai/src/local_provider/key_manager.rs \
       crates/ai/src/local_provider/agent_provider_secrets.rs
```
Expected: silent success. `git status` shows `R  crates/ai/src/local_provider/key_manager.rs -> crates/ai/src/local_provider/agent_provider_secrets.rs`.

- [ ] **Step 1.2: Rename the symbols inside the new file**

Open `crates/ai/src/local_provider/agent_provider_secrets.rs`. Apply these textual replacements (all occurrences, file-scoped):

```
LocalProviderKeyManagerEvent → AgentProviderSecretsEvent
LocalProviderKeyManager      → AgentProviderSecrets
```

Do **not** change `SECURE_STORAGE_KEY`, the string `"LocalProviderApiKey"`, the `StoredKey` struct, or any `secure_storage::*` calls.

After editing, the top of the file should read (showing key landmarks):

```rust
const SECURE_STORAGE_KEY: &str = "LocalProviderApiKey";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProviderSecretsEvent {
    KeyUpdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct StoredKey {
    api_key: Option<String>,
}

pub struct AgentProviderSecrets {
    key: Option<String>,
}

impl AgentProviderSecrets {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self { ... }
    // ...
}

impl Entity for AgentProviderSecrets {
    type Event = AgentProviderSecretsEvent;
}

impl SingletonEntity for AgentProviderSecrets {}
```

The `ctx.emit(LocalProviderKeyManagerEvent::KeyUpdated)` call inside `update(...)` becomes `ctx.emit(AgentProviderSecretsEvent::KeyUpdated)`. Walk the rest of the file and replace any remaining `LocalProviderKeyManager*` identifiers.

Also update the module-level doc comment at the top of the file: any `LocalProviderKeyManager` mention becomes `AgentProviderSecrets`. Leave the literal token-payload examples that reference `"LocalProviderApiKey"` (the keychain entry) alone.

- [ ] **Step 1.3: Update the module declaration in `crates/ai/src/local_provider/mod.rs`**

Open `crates/ai/src/local_provider/mod.rs`. Find lines:

```rust
pub mod key_manager;
```
and
```rust
pub use key_manager::{LocalProviderKeyManager, LocalProviderKeyManagerEvent};
```

Change to:

```rust
pub mod agent_provider_secrets;
```
and
```rust
pub use agent_provider_secrets::{AgentProviderSecrets, AgentProviderSecretsEvent};
```

- [ ] **Step 1.4: Update `app/src/lib.rs` singleton registration**

Open `app/src/lib.rs`. Around line 1275-1277, find:

```rust
// key via LocalProviderKeyManager::as_ref(ctx). Without this, the very
// ...
ctx.add_singleton_model(::ai::local_provider::LocalProviderKeyManager::new);
```

Change to:

```rust
// key via AgentProviderSecrets::as_ref(ctx). Without this, the very
// ...
ctx.add_singleton_model(::ai::local_provider::AgentProviderSecrets::new);
```

- [ ] **Step 1.5: Update `app/src/settings/ai.rs` doc comment**

Open `app/src/settings/ai.rs`. Around line 1467, find:

```rust
// stored separately via LocalProviderKeyManager (secure storage).
```

Change to:

```rust
// stored separately via AgentProviderSecrets (secure storage).
```

(This is just a comment; no code path runs through it.)

- [ ] **Step 1.6: Update `app/src/settings_view/ai_page.rs` call sites**

Open `app/src/settings_view/ai_page.rs`. Around lines 6684, 6706, 6811, 6905 there are mentions of `LocalProviderKeyManager`. Replace each:

Line ~6684 (doc comment):
```rust
/// The API key is stored separately via `LocalProviderKeyManager` (OS secure
```
→
```rust
/// The API key is stored separately via `AgentProviderSecrets` (OS secure
```

Line ~6706 (load on widget construction):
```rust
let api_key_initial: String = ::ai::local_provider::LocalProviderKeyManager::as_ref(ctx)
```
→
```rust
let api_key_initial: String = ::ai::local_provider::AgentProviderSecrets::as_ref(ctx)
```

Line ~6811 (write on input change):
```rust
::ai::local_provider::LocalProviderKeyManager::handle(ctx).update(
```
→
```rust
::ai::local_provider::AgentProviderSecrets::handle(ctx).update(
```

Line ~6905 (doc comment):
```rust
/// in `LocalProviderKeyManager` (not an AISetting), so it has no
```
→
```rust
/// in `AgentProviderSecrets` (not an AISetting), so it has no
```

- [ ] **Step 1.7: Update `app/src/ai/local_provider_config.rs`**

Open `app/src/ai/local_provider_config.rs`. Three changes:

Line ~2 (module doc):
```rust
//! the `LocalProviderKeyManager` singleton. Lives here (under `app/`) rather
```
→
```rust
//! the `AgentProviderSecrets` singleton. Lives here (under `app/`) rather
```

Line ~13 (import):
```rust
use ai::local_provider::{LocalProviderConfig, LocalProviderKeyManager};
```
→
```rust
use ai::local_provider::{LocalProviderConfig, AgentProviderSecrets};
```

(`LocalProviderConfig` stays; only the second item changes.)

Line ~47 (call):
```rust
let api_key = LocalProviderKeyManager::as_ref(ctx)
```
→
```rust
let api_key = AgentProviderSecrets::as_ref(ctx)
```

- [ ] **Step 1.8: Sweep for any leftover references**

Run:
```bash
grep -rn "LocalProviderKeyManager" --include="*.rs" .
```
Expected: **zero matches**. If any remain (e.g. another file the file map missed), fix them before continuing.

```bash
grep -rn "key_manager" --include="*.rs" crates/ai/ app/
```
Expected: zero matches under those paths. (The module is now `agent_provider_secrets`.)

- [ ] **Step 1.9: Build the `ai` crate**

Run:
```bash
cargo build -p ai 2>&1 | tail -20
```
Expected: `Finished ... profile`. If errors mention `LocalProviderKeyManager` or `key_manager`, return to Step 1.8 and re-sweep.

- [ ] **Step 1.10: Build the full workspace (catches `app/` references)**

Run (in background — it's a few minutes):
```bash
cargo build 2>&1 | tail -30
```
Expected: `Finished`. Any errors point to a missed call site in `app/`.

- [ ] **Step 1.11: Run the relevant test suites**

Run:
```bash
cargo nextest run -p ai 2>&1 | tail -20
```
Expected: same green count as the baseline in Step 0.3.

- [ ] **Step 1.12: Stage and commit**

Run:
```bash
git add crates/ai/src/local_provider/ \
        app/src/lib.rs \
        app/src/settings/ai.rs \
        app/src/settings_view/ai_page.rs \
        app/src/ai/local_provider_config.rs
git status --short
```
Expected `git status` shows: one rename (`R`), four modifies (`M`).

Then:
```bash
git commit -m "$(cat <<'EOF'
refactor(ai/local_provider): rename LocalProviderKeyManager to AgentProviderSecrets

Phase 1a of multi-provider work — pure symbol rename, no behavior change.
Renames the singleton + Event enum + file. Keychain key string
("LocalProviderApiKey") and the agents.local_provider.* TOML schema are
left untouched; those move in Phase 1b atomically with the migration helper.

See specs/multi-local-llm/design.md §8 (Phase 1a table).
EOF
)"
```

Expected: commit succeeds.

---

## Task 2: Rename `LocalProviderWidget` → `AgentProvidersWidget`

**Files:**
- Modify: `app/src/settings_view/ai_page.rs`

All 5 occurrences of `LocalProviderWidget` (struct definition, two impls, **and** the two `widgets.push(Box::new(...))` instantiation sites) live in `ai_page.rs`. The earlier draft of this plan listed `settings/ai.rs` as a second file — that was wrong. Verify with `grep -n "LocalProviderWidget" app/src/settings_view/ai_page.rs` before editing; you should see exactly 5 lines.

- [ ] **Step 2.1: Rename struct definition in `ai_page.rs`**

Open `app/src/settings_view/ai_page.rs`. Around line 6687:

```rust
struct LocalProviderWidget {
```
→
```rust
struct AgentProvidersWidget {
```

Around line 6697 (the inherent `impl`):

```rust
impl LocalProviderWidget {
```
→
```rust
impl AgentProvidersWidget {
```

Around line 6941 (the trait `impl`):

```rust
impl SettingsWidget for LocalProviderWidget {
```
→
```rust
impl SettingsWidget for AgentProvidersWidget {
```

- [ ] **Step 2.2: Sweep `ai_page.rs` for any remaining references**

Run:
```bash
grep -n "LocalProviderWidget" app/src/settings_view/ai_page.rs
```
Expected: zero matches. (The widget body uses `Self`, so no other call sites should exist inside `ai_page.rs`.)

- [ ] **Step 2.3: Update instantiation sites in `app/src/settings_view/ai_page.rs`**

Same file as Steps 2.1 — the widget is instantiated nearby in the page's branch logic. Around lines 1515 and 1556:

```rust
widgets.push(Box::new(LocalProviderWidget::new(ctx)));
```
→
```rust
widgets.push(Box::new(AgentProvidersWidget::new(ctx)));
```

There should be exactly two such sites. Use `grep -n "LocalProviderWidget::new" app/src/settings_view/ai_page.rs` to verify.

- [ ] **Step 2.4: Confirm no other references exist**

Run:
```bash
grep -rn "LocalProviderWidget" --include="*.rs" .
```
Expected: zero matches.

- [ ] **Step 2.5: Build the workspace**

Run (in background):
```bash
cargo build 2>&1 | tail -20
```
Expected: `Finished`. Any errors point to a missed reference.

- [ ] **Step 2.6: Run AI-crate tests**

Run:
```bash
cargo nextest run -p ai 2>&1 | tail -10
```
Expected: same green count as before.

- [ ] **Step 2.7: Stage and commit**

Run:
```bash
git add app/src/settings_view/ai_page.rs
git status --short
```
Expected: one `M` line for `ai_page.rs` (plus the ignorable untracked `.claude/` and `.omc/` entries).

Then:
```bash
git commit -m "$(cat <<'EOF'
refactor(settings_view/ai_page): rename LocalProviderWidget to AgentProvidersWidget

Phase 1a of multi-provider work — pure symbol rename, no behavior change.
The widget body, settings keys, and AISettingsPageAction variants are
all left as-is; Phase 1b replaces this widget with one that renders a list
of providers.

Note: all 5 occurrences live in ai_page.rs; an earlier draft of this plan
incorrectly listed settings/ai.rs as also touched.

See specs/multi-local-llm/design.md §8 (Phase 1a table).
EOF
)"
```

Expected: commit succeeds.

---

## Task 3: Final verification

**Files:** none (verification only)

- [ ] **Step 3.1: Confirm no `LocalProvider*` symbols renamed in Phase 1a still exist**

Run:
```bash
grep -rn "LocalProviderKeyManager\|LocalProviderWidget" --include="*.rs" .
```
Expected: zero matches.

- [ ] **Step 3.2: Confirm all `LocalProvider*` symbols *not* in Phase 1a scope still exist**

Run:
```bash
grep -rn "LocalProviderConfig\|LocalProviderHistory\|LocalTool" --include="*.rs" . | wc -l
```
Expected: a positive integer (≥ 50) — these symbols are intentionally preserved for Phase 1b.

- [ ] **Step 3.3: Run presubmit (full required gate per `CLAUDE.md`)**

Run (in background — this can take 15+ minutes):
```bash
./script/presubmit 2>&1 | tail -40
```
Expected: all phases pass (fmt, clippy, tests). If anything fails, fix before continuing — do not push a broken branch.

- [ ] **Step 3.4: Confirm both commits look right**

Run:
```bash
git log --oneline multi-local-llm ^nmehta/local-llm-provider
```
Expected:
```
<sha-2> refactor(settings_view/ai_page): rename LocalProviderWidget to AgentProvidersWidget
<sha-1> refactor(ai/local_provider): rename LocalProviderKeyManager to AgentProviderSecrets
9a044d24 docs(specs/multi-local-llm): design for multi-provider local LLM support
```
(Three commits ahead of `nmehta/local-llm-provider`.)

- [ ] **Step 3.5: Push and verify origin tracks the branch**

Run:
```bash
git push -u origin multi-local-llm
```
Expected: branch is created on origin and tracking is set. The user will open a PR manually (or via `gh pr create`) once they've eyeballed the diff.

---

## Self-review checklist (writer's note)

This plan was written against `specs/multi-local-llm/design.md` §8 Phase 1a row. Spot checks before handoff:

- [x] **Scope:** every change here is a symbol rename — no logic changes, no schema changes, no LLM-ID format changes, no keychain-key changes. Matches design §8.
- [x] **Coverage:** both renamed symbols (`LocalProviderKeyManager`, `LocalProviderWidget`) are explicitly handled with all known call sites enumerated.
- [x] **Symbols intentionally NOT renamed in Phase 1a:** `LocalProviderConfig`, `LocalProviderHistory`, `LocalTool`, AISetting marker types, `ToggleLocalProvider*` actions, the `"LocalProviderApiKey"` keychain string. Each of these is called out in the file map's "UNCHANGED" list.
- [x] **No placeholders:** every step shows the exact text-before and text-after for code edits.
- [x] **No TBDs.**
- [x] **Type consistency:** `AgentProviderSecrets` is the singular form used everywhere; `AgentProvidersWidget` (plural) follows the openwarp naming convention from the spec.
- [x] **Verification gates:** Step 0 baseline, Step 1.9-1.11 build+test after rename 1, Step 2.5-2.6 build+test after rename 2, Step 3.3 full presubmit before push.

---

## Next plan

Once Phase 1a is merged (or at least pushed and reviewed), the Phase 1b plan will cover:
- New types `AgentProvider`, `AgentProviderModel`, `AgentProviderKind`, `AgentProviderApiType` in `app/src/settings/ai.rs`
- Rename `AgentProviderSecrets` from a singleton-with-one-key into a `HashMap<provider_id, api_key>`
- Migration helper (one-time, idempotent, fixture-tested)
- New `byop:<uuid>:<model>` LLM ID encoder/decoder + legacy `local:` parser
- Multi-provider settings UI (list with add/remove + per-model rows)
- `lookup_byop` dispatch in `agent/api/impl.rs`
- Conversation-load-time LLMId rewrite
- Move `agents.local_provider.compaction_*` to `agents.byop_compaction.*`

That plan will be written after this one is approved and Phase 1a code is in.
