# Phase 5e -- Gemini CLI as a Local Child Harness -- Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Touching UI? Read `warp-ui-guidelines` first.

**Goal:** Enable Gemini CLI as a local child harness for orchestration, complete the BYOP env-var matrix for the Gemini+Gemini cell, lift the `normalize_local_child_harness` filter that currently strips `Harness::Gemini`, and ensure the existing Phase 5a -> 5b -> 5c pipeline (filter / picker / validator / launch) all surface Gemini-harness as a legitimate orchestration target.

**Scoping decision:** This plan addresses **Local Native + Local Gemini-CLI harness** dispatch only. Remote-Gemini dispatch inherits the same wire-field forwarding as Phase 5d (no new work there). **Per project-owner direction, both the BYOP api_key and the BYOP base_url are injected via Gemini CLI's `~/.gemini/settings.json` file — NOT via env vars.** The existing `prepare_gemini_environment_config` function already writes this file (auth type + trusted folders) and is extended in this phase to thread BYOP config in. Full endpoint redirection is supported: BYOP-Gemini providers that self-host a Gemini-compatible endpoint can redirect traffic away from `generativelanguage.googleapis.com` via the auth section in `settings.json`. The `byop_env_for_harness` matrix stays at zero entries for `Harness::Gemini` (the env-var path is the wrong injection point for this CLI).

**Out of scope:**

- Anything related to the Gemini API surface itself (the Phase 3c adapter is already shipped).
- Anything related to Remote orchestration that 5d doesn't already cover.
- Phase 5f or beyond.

**Decisions locked in:**

| Decision | Choice | Rationale |
|---|---|---|
| BYOP injection mechanism for Gemini CLI | **settings.json** (`~/.gemini/settings.json`). Both `api_key` and base-URL endpoint are written under `security.auth`. No env vars. | Project-owner direction. The Gemini CLI consumes `~/.gemini/settings.json`; the existing `prepare_gemini_environment_config` already writes this file. Writing BYOP fields there gives both auth + endpoint redirection in the same atomic write. |
| settings.json field names | `security.auth.api_key` for the API key, `security.auth.endpoint` for the BYOP base URL. Both are added to the `GeminiAuth` struct as `Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. **Field names confirmed against upstream `gemini-cli` source during implementation.** If upstream uses different names, adjust at Step 3.1. | The `GeminiSettings` struct uses `#[serde(flatten)] extra: Map<String, Value>` for forward-compat, so adding typed fields is safe. |
| Env-var matrix for `Harness::Gemini` | **Empty** — Gemini stays in the catch-all `Harness::Oz \| Harness::Gemini \| Harness::Unknown => {}` arm. settings.json is the injection point. | A single source of truth keeps the BYOP injection correct; mixing env vars + settings.json risks one path silently winning. |
| Feature flag gating | Same `LocalLlmProvider` feature flag as the rest of Phase 5 -- no new flag. The `normalize_local_child_harness` filter lift is unconditional (the harness-disabled check via `local_child_harness_disabled_message` already returns `None` for Gemini, and the `LocalClaudeCodexChildHarnesses` flag only gates Claude/Codex). | Adding a separate flag for Gemini increases the flag matrix without a concrete rollout need. |
| Plugin manager / validate_cli_installed | `GeminiHarness` already implements `ThirdPartyHarness` with the default `validate()` impl, which calls `validate_cli_installed("gemini", Some("https://geminicli.com/"))`. The `GeminiPluginManager` exists at `plugin_manager/gemini.rs` and is already gated on `FeatureFlag::GeminiNotifications && FeatureFlag::HOANotifications`. The Gemini CLI is installed via `npm install -g @anthropic/gemini-cli` or `npx @anthropic/gemini-cli` -- the validate check just confirms `gemini` is on PATH. | Existing infrastructure fully covers Gemini. No new validation or plugin-manager wiring needed. |
| `normalize_local_child_harness` filter lift | Unconditional -- add `Harness::Gemini` to the accepted set in `parse_local_child_harness`. No feature-flag gating on the filter itself. | The per-harness disable message check (`local_child_harness_disabled_message`) is the correct control point; the filter is structural. |
| Gemini child command shape | Build `gemini --yolo -i "$(cat '{prompt_path}')"` mirroring the existing `GeminiHarness` standalone pattern. Reuse the extended `prepare_gemini_environment_config` for settings/trusted-folders + BYOP injection. | Consistency with the existing cloud-agent Gemini path. |
| Phase 5c/5d review follow-ups | None outstanding for Phase 5e -- the Phase 5d plan already absorbed all Phase 5c follow-ups. |

**Architecture delta over Phases 5c/5d:**

Phase 5c built the env-var supply chain for Claude/Codex/OpenCode. Phase 5d built the Remote credential bridge. Phase 5e takes a **different mechanism** for the Gemini CLI: settings.json injection rather than env vars.

1. **Filter lift (Task 1):** `Harness::parse_local_child_harness` adds `Gemini` to the accepted set. `normalize_local_child_harness` now returns `Some(Harness::Gemini)` for `"gemini"` input.
2. **Launch path + BYOP settings.json injection (Task 2):** The `unreachable!("normalize_local_child_harness filters out Gemini")` arm in `prepare_local_harness_child_launch` becomes a real code path that mirrors the Claude pattern: resolve `harness_kind(Gemini)` -> `GeminiHarness` -> `validate()` -> extended `prepare_gemini_environment_config(working_dir, system_prompt, byop_config)` (now accepting BYOP api_key + base_url, writing them into `security.auth`) -> build command via `build_local_gemini_child_command`. `GeminiAuth` gains `api_key` and `endpoint` optional fields.
3. **Env-var matrix (Task 3):** `byop_env_for_harness` keeps `Harness::Gemini` in the catch-all empty arm. settings.json is the injection point; no env vars. Tests assert the empty behavior is intentional.
4. **Tests (Tasks 4-5):** Filter tests, launch tests, settings.json BYOP-write tests.
5. **Docs + memory (Tasks 6-7):** README status block, memory entry.

**Tech Stack:** Rust 2021, `std::collections::HashMap<OsString, OsString>`, existing `GeminiHarness`, `prepare_gemini_environment_config`, `shell_words::quote`.

---

## Per-touchpoint reference

| Concern | Source of truth |
|---|---|
| `Harness` enum definition | `crates/warp_cli/src/agent.rs:125-149` |
| `Harness::parse_local_child_harness` (the filter) | `crates/warp_cli/src/agent.rs:157-161` |
| `normalize_local_child_harness` (wrapper) | `app/src/pane_group/pane/local_harness_launch.rs:35-37` |
| `prepare_local_harness_child_launch` (assembly site, Gemini unreachable) | `app/src/pane_group/pane/local_harness_launch.rs:92-221` (Gemini arm at line 182) |
| `local_child_task_config` (already includes Gemini) | `app/src/pane_group/pane/local_harness_launch.rs:72-89` (Gemini in the arm at line 81) |
| `local_child_harness_disabled_message` (pass-through for Gemini) | `app/src/ai/local_child_harnesses.rs:4-14` (Gemini at line 12) |
| `byop_env_for_harness` (env-var matrix, Gemini empty today) | `app/src/ai/orchestration_byop_env.rs:42-102` (Gemini arm at line 98) |
| `byop_env_for_harness` tests (Gemini empty assertion) | `app/src/ai/orchestration_byop_env_tests.rs:130-138` |
| `byop_harness_compatible` (Phase 5a matrix, Gemini+Gemini already wired) | `app/src/ai/byop_orchestration_filter.rs:34-45` (Gemini at line 42) |
| `harness_kind` (returns `ThirdParty(GeminiHarness)`) | `app/src/ai/agent_sdk/driver/harness/mod.rs:241-250` (Gemini at line 247) |
| `GeminiHarness` (full ThirdPartyHarness impl) | `app/src/ai/agent_sdk/driver/harness/gemini.rs:31-95` |
| `prepare_gemini_environment_config` (settings + trust setup) | `app/src/ai/agent_sdk/driver/harness/gemini.rs:243-268` |
| `prepare_gemini_settings` (writes `security.auth.selectedType`) | `app/src/ai/agent_sdk/driver/harness/gemini.rs:270-292` |
| `GeminiSettings` / `GeminiSecurity` / `GeminiAuth` (settings.json schema) | `app/src/ai/agent_sdk/driver/harness/gemini.rs:318-345` |
| `gemini_command` (builds shell command) | `app/src/ai/agent_sdk/driver/harness/gemini.rs:101-103` |
| `validate_cli_installed` (checks PATH) | `app/src/ai/agent_sdk/driver/harness/mod.rs:271-286` |
| `harness_model_env_vars` (Gemini returns empty -- correct) | `app/src/ai/agent_sdk/driver/harness/mod.rs:414-434` (Gemini at line 430) |
| `plugin_manager_for` (Gemini already handled) | `app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs:214-271` (Gemini at line 247) |
| `GeminiPluginManager` (auto-install support) | `app/src/terminal/cli_agent_sessions/plugin_manager/gemini.rs:22-47` |
| `launch_local_harness_child` (call site in terminal_pane) | `app/src/pane_group/pane/terminal_pane.rs:1845-2009` |
| BYOP env resolve call site in `launch_local_harness_child` | `app/src/pane_group/pane/terminal_pane.rs:1880-1902` |
| `GEMINI_API_KEY_AUTH_TYPE` constant | `app/src/ai/agent_sdk/driver/harness/gemini.rs:313` |
| `launch_local_harness_child` tests | `app/src/pane_group/pane/local_harness_launch_tests.rs:1-469` |

---

## File map

**Created:**

- None. All changes are modifications to existing files.

**Modified:**

- `crates/warp_cli/src/agent.rs` -- add `Self::Gemini` to the accepted set in `parse_local_child_harness`.
- `app/src/ai/agent_sdk/driver/harness/gemini.rs` -- extend `prepare_gemini_environment_config` signature to accept optional BYOP `(api_key, base_url)`; add `api_key` + `endpoint` fields to `GeminiAuth`; thread the BYOP fields into the settings.json write. Make the function `pub(crate)`.
- `app/src/ai/agent_sdk/driver/harness/gemini_tests.rs` (or sibling test file) -- add tests that confirm `prepare_gemini_environment_config` writes BYOP `api_key` + `endpoint` to `~/.gemini/settings.json` when the BYOP config is `Some(...)`; round-trip the file through `read_json_file_or_default` to verify.
- `app/src/pane_group/pane/local_harness_launch.rs` -- replace the `Harness::Gemini => unreachable!(...)` arm with a real launch path mirroring the Claude pattern; resolve the BYOP entry via `resolve_byop_for_local_child`; pass the api_key + base_url to `prepare_gemini_environment_config`; add `build_local_gemini_child_command` helper.
- `app/src/ai/orchestration_byop_env.rs` -- **no code changes**. Gemini stays in the catch-all empty arm. The module doc comment is updated to note that Gemini uses settings.json injection instead of env vars.
- `app/src/ai/orchestration_byop_env_tests.rs` -- rename `gemini_harness_returns_empty_today` to `gemini_harness_uses_settings_json_not_env_vars`; the assertion stays the same (empty bag) but the test name reflects the intentional design.
- `app/src/pane_group/pane/local_harness_launch_tests.rs` -- update filter rejection test; add Gemini-specific launch test that confirms (a) the command starts with `gemini`, (b) settings.json was written, (c) BYOP api_key + endpoint are in the settings.json content.
- `crates/warp_cli/src/agent_tests.rs` -- add/update test for `parse_local_child_harness` accepting Gemini.
- `specs/multi-local-llm/README.md` -- add Phase 5e status block + table row + What landed + Architecture bullets.

---

## Stage A -- Lift the filter

### Task 1: Enable `Harness::Gemini` in `parse_local_child_harness`

**Files:**
- Modify: `crates/warp_cli/src/agent.rs`
- Modify: `crates/warp_cli/src/agent_tests.rs` (if tests exist for parse_local_child_harness)
- Modify: `app/src/pane_group/pane/local_harness_launch_tests.rs`

**Read these reference files FIRST:**
- `crates/warp_cli/src/agent.rs:157-161` -- `parse_local_child_harness` match that currently rejects Gemini.
- `app/src/pane_group/pane/local_harness_launch_tests.rs:64-98` -- `normalize_local_child_harness_accepts_supported_aliases` and `normalize_local_child_harness_rejects_unsupported_values` tests.

- [ ] **Step 1.1: Update `parse_local_child_harness` to accept Gemini**

In `crates/warp_cli/src/agent.rs`, change the match in `parse_local_child_harness`:

```rust
    pub fn parse_local_child_harness(value: &str) -> Option<Self> {
        match Self::parse_orchestration_harness(value) {
            Some(harness @ (Self::Claude | Self::OpenCode | Self::Codex | Self::Gemini)) => Some(harness),
            Some(Self::Oz) | Some(Self::Unknown) | None => None,
        }
    }
```

Note: `Self::Gemini` moves from the rejection arm to the acceptance arm. The exhaustive match (no `_` wildcard) is preserved.

- [ ] **Step 1.2: Update `local_harness_launch_tests.rs` filter tests**

In `app/src/pane_group/pane/local_harness_launch_tests.rs`, update `normalize_local_child_harness_accepts_supported_aliases` to include Gemini:

```rust
    assert_eq!(
        normalize_local_child_harness("gemini"),
        Some(Harness::Gemini)
    );
```

And update `normalize_local_child_harness_rejects_unsupported_values` to remove the Gemini assertion (line 96):

Remove:
```rust
    assert_eq!(normalize_local_child_harness("gemini"), None);
```

The remaining rejections (`"oz"`, `""`) stay.

- [ ] **Step 1.3: Update `crates/warp_cli/src/agent_tests.rs`** if it contains parallel filter tests. Read the file first; if it has tests for `parse_local_child_harness`, add the Gemini acceptance assertion and remove any Gemini rejection assertion. If no such tests exist, skip this step.

- [ ] **Step 1.4: Build + clippy + tests**

```bash
cargo build -p warp_cli 2>&1 | tail -5
cargo clippy -p warp_cli --lib --tests -- -D warnings 2>&1 | tail -5
cargo nextest run -p warp_cli parse_local_child_harness 2>&1 | tail -10
cargo nextest run -p warp normalize_local_child_harness 2>&1 | tail -10
```

Note: `cargo build -p warp` will fail here because `local_harness_launch.rs` still has the `unreachable!()` for Gemini. That's expected -- Task 2 fixes it. Verify the `warp_cli` crate builds and its tests pass independently.

- [ ] **Step 1.5: Commit**

```
feat(warp_cli): accept Harness::Gemini in parse_local_child_harness

Phase 5e task 1. Moves Gemini from the rejection arm to the
acceptance arm in parse_local_child_harness, lifting the filter
that prevented Gemini CLI from being used as a local child harness.

The exhaustive match (no _ wildcard) is preserved. Downstream
prepare_local_harness_child_launch gains the real Gemini code path
in the next task.
```

---

## Stage B -- Launch path

### Task 2: Wire the Gemini launch path in `prepare_local_harness_child_launch`

**Files:**
- Modify: `app/src/pane_group/pane/local_harness_launch.rs`

**Read these reference files FIRST:**
- `app/src/pane_group/pane/local_harness_launch.rs:122-183` -- the per-harness match in `prepare_local_harness_child_launch`. Read the `Harness::Claude` arm (lines 125-159) as the pattern to mirror.
- `app/src/ai/agent_sdk/driver/harness/gemini.rs:31-103` -- `GeminiHarness` struct, `validate()` default impl, `gemini_command()`, `prepare_gemini_environment_config()`. These are the building blocks.
- `app/src/ai/agent_sdk/driver/harness/mod.rs:241-250` -- `harness_kind(Harness::Gemini)` returns `ThirdParty(GeminiHarness)`.

- [ ] **Step 2.1: Add `build_local_gemini_child_command` helper**

After `build_local_codex_child_command` (around line 70), add:

```rust
pub(super) fn build_local_gemini_child_command(prompt: &str) -> Result<String, String> {
    // Write the prompt to a temp file so the shell command can
    // `cat` it without quoting issues. The temp file is NOT held
    // by the caller -- it must be persisted at a stable path.
    // Use the same pattern as the standalone GeminiHarnessRunner:
    // write to a NamedTempFile, then persist it to a known location
    // so the shell command can reference it. But for local child
    // launches, the temp file's path is baked into the command string
    // and the file gets cleaned up when the pane exits.
    let temp_file = tempfile::Builder::new()
        .prefix("oz_prompt_")
        .suffix(".txt")
        .tempfile()
        .map_err(|error| format!("Failed to create temp prompt file for Gemini child: {error}"))?;
    std::io::Write::write_all(&mut temp_file.as_file(), prompt.as_bytes())
        .map_err(|error| format!("Failed to write prompt to temp file for Gemini child: {error}"))?;
    let prompt_path = temp_file.path().display().to_string();
    // Persist the temp file so it outlives this function -- the shell
    // command reads it when the pane opens. The OS will clean it up
    // eventually, or when the pane exits.
    let _ = temp_file.keep();
    Ok(format!("gemini --yolo -i \"$(cat '{prompt_path}')\""))
}
```

Add the required import near the top of the file (if not already present):

```rust
use std::io::Write;
```

- [ ] **Step 2.2: Replace the `unreachable!` arm with a real Gemini launch path**

In `prepare_local_harness_child_launch`, replace line 182:

```rust
        Harness::Gemini => unreachable!("normalize_local_child_harness filters out Gemini"),
```

With a real code path mirroring the Claude arm. **This task assumes Task 3 (settings.json BYOP injection) has been completed first** — `prepare_gemini_environment_config` already accepts the `byop_config` parameter:

```rust
        Harness::Gemini => {
            let working_dir = startup_directory
                .or_else(|| std::env::current_dir().ok())
                .ok_or_else(|| {
                    format!(
                        "Could not resolve a working directory for the local {} child.",
                        harness.display_name()
                    )
                })?;
            let HarnessKind::ThirdParty(third_party_harness) =
                harness_kind(harness).map_err(|error: AgentDriverError| error.to_string())?
            else {
                unreachable!("Gemini resolves to a third-party harness")
            };
            third_party_harness
                .validate()
                .map_err(|error: AgentDriverError| error.to_string())?;
            // Phase 5e: resolve BYOP config from the run-wide model_id (passed
            // in alongside the existing byop_env: HashMap parameter — for
            // Gemini we ignore the env-var bag because settings.json is the
            // injection point, and instead unpack the byop_config sibling
            // parameter that launch_local_harness_child threads in).
            let byop_config_for_gemini = byop_config_for_gemini.clone();
            // Prepare Gemini environment config: writes settings.json
            // (auth type, trusted folders, and BYOP api_key + endpoint
            // when byop_config_for_gemini is Some(...)). No system_prompt
            // for local child launches -- the orchestrator's prompt is
            // the effective system prompt, passed as the -i argument.
            crate::ai::agent_sdk::driver::harness::gemini::prepare_gemini_environment_config(
                &working_dir,
                None,
                byop_config_for_gemini.as_ref(),
            )
            .map_err(|error| error.to_string())?;
            if let Some(manager) = plugin_manager_for(third_party_harness.cli_agent()) {
                if let Err(error) = manager.install().await {
                    log::warn!("Gemini plugin installation failed for child harness: {error}");
                }
            }

            build_local_gemini_child_command(&prompt)?
        }
```

`byop_config_for_gemini` is a new `Option<GeminiByopConfig>` parameter on `prepare_local_harness_child_launch` (type defined in `gemini.rs` as a pub(crate) struct with `api_key: String` + `base_url: String` fields). For non-Gemini harnesses it is `None`; for `Harness::Gemini` the caller resolves it via `resolve_byop_for_local_child` and constructs the struct from the resolved provider's `base_url` + the resolved api_key.

The `byop_env: HashMap<OsString, OsString>` parameter from Phase 5c is **still threaded through** but stays empty for the Gemini arm (the env-var matrix's catch-all guarantees an empty bag for Gemini). Other harness arms remain unchanged.

- [ ] **Step 2.3: Add `byop_config: Option<GeminiByopConfig>` parameter to `prepare_local_harness_child_launch`**

Update the function signature in `app/src/pane_group/pane/local_harness_launch.rs` to accept `Option<GeminiByopConfig>` (defined in Task 3). Plumb it from the call site in `launch_local_harness_child` (`app/src/pane_group/pane/terminal_pane.rs:~1880`) by checking if the resolved BYOP entry's `api_type == Gemini` and the chosen harness is Gemini — if so, build the struct; otherwise pass `None`.

- [ ] **Step 2.4: Build + clippy + tests**

```bash
cargo build -p warp 2>&1 | tail -10
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
cargo nextest run -p warp prepare_local_harness_child_launch 2>&1 | tail -15
```

Expected: clean build and clippy. Existing tests pass (they don't exercise the Gemini path yet).

- [ ] **Step 2.5: Commit**

```
feat(pane_group): wire Gemini CLI launch path with settings.json BYOP injection

Phase 5e task 2. Replaces the unreachable!() arm for Harness::Gemini
with a real code path that:
- resolves harness_kind(Gemini) -> GeminiHarness
- validates the gemini CLI is on PATH
- prepares Gemini environment config via the extended
  prepare_gemini_environment_config(working_dir, system_prompt,
  byop_config), threading BYOP api_key + base_url into settings.json
  when the run-wide model_id is a byop: entry with api_type==Gemini
- installs the Warp notification plugin if available
- builds the command via build_local_gemini_child_command

prepare_local_harness_child_launch gains an Option<GeminiByopConfig>
parameter (None for non-Gemini harnesses). launch_local_harness_child
resolves it from resolve_byop_for_local_child + harness check. The
env-var bag from Phase 5c is unchanged; Gemini stays at zero env vars.
```

---

## Stage C -- Settings.json BYOP injection

### Task 3: Extend `prepare_gemini_environment_config` to write BYOP api_key + endpoint to settings.json

**Files:**
- Modify: `app/src/ai/agent_sdk/driver/harness/gemini.rs`
- Modify: `app/src/ai/agent_sdk/driver/harness/gemini_tests.rs` (or sibling test file)
- Modify: `app/src/ai/orchestration_byop_env.rs` (doc-comment + test rename only — no functional code change)
- Modify: `app/src/ai/orchestration_byop_env_tests.rs` (rename only)

**Read these reference files FIRST:**
- `app/src/ai/agent_sdk/driver/harness/gemini.rs:243-292` -- existing `prepare_gemini_environment_config` + `prepare_gemini_settings`.
- `app/src/ai/agent_sdk/driver/harness/gemini.rs:318-345` -- `GeminiSettings`, `GeminiSecurity`, `GeminiAuth` struct definitions. The `extra: Map<String, Value>` flatten field preserves forward-compat for any field we don't model typed.
- Upstream `gemini-cli` repo schema for `~/.gemini/settings.json` -- confirm field names for `api_key` and base-URL endpoint before adding typed fields. If upstream uses different names (e.g. `apiKey` via camelCase, `endpoint` vs `baseUrl`), adjust the new field names accordingly. The `#[serde(rename_all = "camelCase")]` on `GeminiAuth` handles snake_case-to-camelCase mapping for declared fields, but the on-disk key must match upstream.
- `app/src/ai/orchestration_byop_env.rs:42-102` -- the existing matrix. **No code change** to the match; only the doc comment + a renamed test.

- [ ] **Step 3.1: Add `api_key` + `endpoint` fields to `GeminiAuth`**

In `app/src/ai/agent_sdk/driver/harness/gemini.rs`, extend the `GeminiAuth` struct:

```rust
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected_type: Option<String>,
    /// Phase 5e. BYOP api_key written to settings.json when the user has
    /// configured a BYOP-Gemini provider for local-child orchestration.
    /// Confirm the exact wire-key against upstream gemini-cli; adjust if
    /// upstream uses a different name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    /// Phase 5e. BYOP base-URL endpoint override. When set, Gemini CLI
    /// routes traffic here instead of generativelanguage.googleapis.com.
    /// Confirm the exact wire-key against upstream gemini-cli.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}
```

- [ ] **Step 3.2: Add a `GeminiByopConfig` pub(crate) struct**

Above `prepare_gemini_environment_config`, add:

```rust
/// Phase 5e. BYOP overrides written into `security.auth` of
/// `~/.gemini/settings.json` when the user has picked a BYOP-Gemini provider
/// + Gemini-CLI child harness. `base_url` is written as `endpoint`; the api
/// key is written as `api_key`. Both empty-after-trim values are treated as
/// "no override" by the writer.
#[derive(Clone, Debug)]
pub(crate) struct GeminiByopConfig {
    pub api_key: String,
    pub base_url: String,
}
```

- [ ] **Step 3.3: Extend `prepare_gemini_environment_config` signature + body**

```rust
pub(crate) fn prepare_gemini_environment_config(
    working_dir: &Path,
    system_prompt: Option<&str>,
    byop_config: Option<&GeminiByopConfig>,
) -> Result<()> {
    let home_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    let gemini_dir = home_dir.join(GEMINI_CONFIG_DIR);
    prepare_gemini_settings(
        &gemini_dir.join(GEMINI_SETTINGS_FILE_NAME),
        system_prompt.is_some(),
        byop_config,
    )?;
    prepare_gemini_trusted_folders(
        &gemini_dir.join(GEMINI_TRUSTED_FOLDERS_FILE_NAME),
        working_dir,
    )?;
    if let Some(prompt) = system_prompt {
        let prompt_path = gemini_dir.join(GEMINI_SYSTEM_PROMPT_FILE_NAME);
        std::fs::write(&prompt_path, prompt).with_context(|| {
            format!(
                "Failed to write Gemini system prompt to {}",
                prompt_path.display()
            )
        })?;
    }
    Ok(())
}
```

- [ ] **Step 3.4: Extend `prepare_gemini_settings` to write BYOP fields**

```rust
fn prepare_gemini_settings(
    settings_path: &Path,
    has_system_prompt: bool,
    byop_config: Option<&GeminiByopConfig>,
) -> Result<()> {
    let mut settings: GeminiSettings = read_json_file_or_default(settings_path)?;
    let auth = settings
        .security
        .get_or_insert_with(GeminiSecurity::default)
        .auth
        .get_or_insert_with(GeminiAuth::default);
    auth.selected_type = Some(GEMINI_API_KEY_AUTH_TYPE.to_owned());

    if let Some(byop) = byop_config {
        let trimmed_key = byop.api_key.trim();
        let trimmed_url = byop.base_url.trim();
        auth.api_key = (!trimmed_key.is_empty()).then(|| trimmed_key.to_owned());
        auth.endpoint = (!trimmed_url.is_empty()).then(|| trimmed_url.to_owned());
    } else {
        // Clear any previously-written BYOP fields so non-BYOP runs don't
        // inherit stale values from a prior BYOP session.
        auth.api_key = None;
        auth.endpoint = None;
    }

    if has_system_prompt {
        let context = settings.context.get_or_insert_with(GeminiContext::default);
        let file_name = GEMINI_SYSTEM_PROMPT_FILE_NAME.to_owned();
        if !context.file_name.contains(&file_name) {
            context.file_name.push(file_name);
        }
    }

    write_json_file(
        settings_path,
        &settings,
        "Failed to serialize Gemini settings",
    )
}
```

The clear-on-`None` branch is important: a user who runs a BYOP-Gemini child once and then runs a non-BYOP Gemini child would otherwise leak the BYOP api_key into the second run. This is the same hygiene the trusted-folders writer does implicitly (overwrites with current working dir).

- [ ] **Step 3.5: Update existing call site in `GeminiHarness::build_runner` / wherever `prepare_gemini_environment_config` is called**

Pass `None` at all existing call sites — they are non-BYOP cloud paths. Search for `prepare_gemini_environment_config(` and add the third argument.

- [ ] **Step 3.6: Add tests in `gemini_tests.rs`**

```rust
#[test]
fn prepare_gemini_settings_writes_byop_api_key_and_endpoint() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let byop = GeminiByopConfig {
        api_key: "AIza-byop-test".to_owned(),
        base_url: "https://my-gemini-proxy.example.com/v1beta".to_owned(),
    };
    prepare_gemini_settings(&settings_path, false, Some(&byop)).unwrap();

    let written: GeminiSettings = read_json_file_or_default(&settings_path).unwrap();
    let auth = written.security.unwrap().auth.unwrap();
    assert_eq!(auth.selected_type.as_deref(), Some(GEMINI_API_KEY_AUTH_TYPE));
    assert_eq!(auth.api_key.as_deref(), Some("AIza-byop-test"));
    assert_eq!(
        auth.endpoint.as_deref(),
        Some("https://my-gemini-proxy.example.com/v1beta")
    );
}

#[test]
fn prepare_gemini_settings_clears_byop_fields_when_none() {
    // First, write a BYOP session.
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let byop = GeminiByopConfig {
        api_key: "leak".to_owned(),
        base_url: "https://leak.example".to_owned(),
    };
    prepare_gemini_settings(&settings_path, false, Some(&byop)).unwrap();

    // Now, simulate a non-BYOP run that re-uses the same settings file.
    prepare_gemini_settings(&settings_path, false, None).unwrap();

    let written: GeminiSettings = read_json_file_or_default(&settings_path).unwrap();
    let auth = written.security.unwrap().auth.unwrap();
    assert!(auth.api_key.is_none(), "api_key must be cleared on non-BYOP run");
    assert!(auth.endpoint.is_none(), "endpoint must be cleared on non-BYOP run");
}

#[test]
fn prepare_gemini_settings_treats_empty_trim_as_none() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let byop = GeminiByopConfig {
        api_key: "   ".to_owned(),
        base_url: "\t\n".to_owned(),
    };
    prepare_gemini_settings(&settings_path, false, Some(&byop)).unwrap();

    let written: GeminiSettings = read_json_file_or_default(&settings_path).unwrap();
    let auth = written.security.unwrap().auth.unwrap();
    assert!(auth.api_key.is_none());
    assert!(auth.endpoint.is_none());
}
```

- [ ] **Step 3.7: Rename the env-var matrix Gemini test to reflect intentional design**

In `orchestration_byop_env_tests.rs`, rename `gemini_harness_returns_empty_today` to `gemini_harness_uses_settings_json_not_env_vars`. The assertion body stays the same (the bag must be empty for Gemini). Update the test comment to explain Phase 5e injects via settings.json.

```rust
#[test]
fn gemini_harness_uses_settings_json_not_env_vars() {
    // Phase 5e: Gemini CLI BYOP routing goes through ~/.gemini/settings.json
    // (security.auth.api_key + security.auth.endpoint), not env vars.
    // byop_env_for_harness intentionally returns an empty bag for any
    // (provider, Harness::Gemini) combination. The settings.json write
    // happens in app/src/ai/agent_sdk/driver/harness/gemini.rs via
    // prepare_gemini_environment_config(byop_config).
    let provider = provider_with(
        AgentProviderApiType::Gemini,
        "https://generativelanguage.googleapis.com/v1beta",
    );
    let env = byop_env_for_harness(&provider, "AIza-test-key", "gemini-2.5-pro", Harness::Gemini);
    assert!(env.is_empty(), "Gemini uses settings.json — env bag must stay empty");
}
```

- [ ] **Step 3.8: Update the module doc comment**

In `orchestration_byop_env.rs`, update the matrix table:

Change:
```
//! | gemini     | (deferred -- Gemini CLI is not enabled as a local child harness today) |
```

To:
```
//! | gemini     | (settings.json injection — Gemini CLI BYOP via ~/.gemini/settings.json; see gemini.rs::prepare_gemini_environment_config) |
```

- [ ] **Step 3.9: Build + clippy + tests**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
cargo nextest run -p warp prepare_gemini_settings 2>&1 | tail -15
cargo nextest run -p warp orchestration_byop_env 2>&1 | tail -10
```

Expected: 3 new gemini_tests + the renamed Gemini env-var test all green; existing tests unchanged.

- [ ] **Step 3.10: Commit**

```
feat(ai/gemini): write BYOP api_key + endpoint into settings.json

Phase 5e task 3. Gemini CLI BYOP routing uses ~/.gemini/settings.json
instead of env vars (project-owner direction). The Gemini CLI's
existing settings.json schema is extended with two new optional
fields under security.auth: api_key and endpoint.

- GeminiAuth gains api_key + endpoint, both Option<String> with
  serde default + skip-if-None so non-BYOP runs produce the same
  file content as before.
- New pub(crate) GeminiByopConfig struct carries the resolved
  api_key + base_url through prepare_gemini_environment_config.
- prepare_gemini_settings clears prior BYOP fields when byop_config
  is None so non-BYOP runs don't inherit stale values.
- The byop_env_for_harness matrix stays empty for Harness::Gemini;
  the test is renamed to reflect that the empty bag is intentional.

3 new gemini_tests covering: BYOP write, clear-on-None, empty-trim
treated as None. Existing orchestration_byop_env tests unchanged
beyond the rename.

Field names (api_key, endpoint) match the most common upstream
gemini-cli conventions; if upstream uses different names, adjust
the typed field declarations on GeminiAuth and the test JSON.
```

---

## Stage D -- Verify existing coverage

### Task 4: Confirm Phase 5a's `byop_harness_compatible` matrix already covers Gemini+Gemini

**Files:**
- Read-only: `app/src/ai/byop_orchestration_filter.rs:42`
- Read-only: `app/src/ai/byop_orchestration_filter_tests.rs:51-59`

**Read these reference files FIRST:**
- `app/src/ai/byop_orchestration_filter.rs:42` -- `AgentProviderApiType::Gemini => matches!(harness, "oz" | "gemini")`.
- `app/src/ai/byop_orchestration_filter_tests.rs:51-59` -- `gemini_compatible_with_native_and_gemini_cli` test.

- [ ] **Step 4.1: Verify the matrix is already wired**

Read `app/src/ai/byop_orchestration_filter.rs:42`. Confirm the Gemini+Gemini cell returns `true`:

```rust
AgentProviderApiType::Gemini => matches!(harness, "oz" | "gemini"),
```

This is already correct -- Phase 5a wired it.

- [ ] **Step 4.2: Verify the test exists**

Read `app/src/ai/byop_orchestration_filter_tests.rs:51-59`. Confirm `gemini_compatible_with_native_and_gemini_cli` asserts:

```rust
assert!(byop_harness_compatible(api, "gemini"));
```

This is already correct -- no changes needed.

- [ ] **Step 4.3: No commit needed** -- this is a read-only verification step.

---

## Stage E -- Launch tests

### Task 5: Add Gemini-specific tests to `local_harness_launch_tests.rs`

**Files:**
- Modify: `app/src/pane_group/pane/local_harness_launch_tests.rs`

**Read these reference files FIRST:**
- `app/src/pane_group/pane/local_harness_launch_tests.rs:149-199` -- `local_child_task_config_records_supported_third_party_harnesses` and friends.
- `app/src/pane_group/pane/local_harness_launch_tests.rs:217-254` -- `prepare_local_codex_child_launch_does_not_rewrite_global_codex_state` (the test boilerplate pattern).
- `app/src/pane_group/pane/local_harness_launch_tests.rs:359-417` -- `prepare_local_harness_child_launch_merges_byop_env_into_env_vars` (the BYOP test pattern).

- [ ] **Step 5.1: Update `local_child_task_config_records_supported_third_party_harnesses`**

Add `Harness::Gemini` to the `for harness in [...]` list (line 151):

```rust
    for harness in [Harness::Claude, Harness::OpenCode, Harness::Codex, Harness::Gemini] {
```

This is already correct in the existing code (Gemini is at line 81 in the `local_child_task_config` function), but the test only loops over Claude/OpenCode/Codex. Adding Gemini to the test confirms the existing behavior.

- [ ] **Step 5.2: Update `local_child_task_config_stamps_orchestrator_name`**

Add `Harness::Gemini` to the loop (line 164):

```rust
    for harness in [Harness::Claude, Harness::OpenCode, Harness::Codex, Harness::Gemini] {
```

- [ ] **Step 5.3: Add Gemini BYOP settings.json write test**

Append a new test that confirms the BYOP api_key + endpoint reach `~/.gemini/settings.json` (via `HOME` redirected to the test's TempDir) and that the launched command starts with `gemini`:

```rust
#[tokio::test]
#[serial_test::serial]
async fn prepare_local_gemini_child_writes_byop_to_settings_json() {
    let fake_home = TempDir::new().unwrap();
    let fake_bin_dir = TempDir::new().unwrap();
    let working_dir = fake_home.path().join("workspace");
    fs::create_dir_all(&working_dir).unwrap();
    write_fake_cli(fake_bin_dir.path(), "gemini");

    let _home = EnvVarGuard::set("HOME", fake_home.path().as_os_str().to_os_string());
    let _path = EnvVarGuard::set("PATH", fake_bin_dir.path().as_os_str().to_os_string());

    let mut ai_client = MockAIClient::new();
    ai_client
        .expect_create_agent_task()
        .times(1)
        .returning(|_, _, _, _| Ok("550e8400-e29b-41d4-a716-446655440000".parse().unwrap()));

    // Gemini does NOT use the byop_env bag — settings.json is the
    // injection point. Pass an empty bag and assert the file content.
    let byop_env: HashMap<OsString, OsString> = HashMap::new();
    let byop_config = Some(GeminiByopConfig {
        api_key: "AIza-byop-test".to_string(),
        base_url: "https://my-gemini-proxy.example.com/v1beta".to_string(),
    });

    let prepared = prepare_local_harness_child_launch(
        "go".to_string(),
        "gemini".to_string(),
        Some("byop:prov:gemini-2.5-pro".to_string()),
        Some("parent-run-1".to_string()),
        Some("agent-a".to_string()),
        Some(ShellType::Zsh),
        Some(working_dir),
        Arc::new(ai_client),
        byop_env,
        byop_config,
    )
    .await
    .unwrap();

    // env_vars from prepare_local_harness_child_launch should NOT
    // contain GEMINI_API_KEY — Gemini uses settings.json injection.
    assert!(
        !prepared.env_vars.contains_key(&OsString::from("GEMINI_API_KEY")),
        "Gemini env bag must stay empty; settings.json carries BYOP"
    );

    // Confirm settings.json was written with the BYOP fields.
    let settings_path = fake_home.path().join(".gemini").join("settings.json");
    assert!(settings_path.exists(), "expected settings.json at {}", settings_path.display());
    let written: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&settings_path).unwrap(),
    )
    .unwrap();
    assert_eq!(
        written["security"]["auth"]["apiKey"].as_str(),
        Some("AIza-byop-test"),
    );
    assert_eq!(
        written["security"]["auth"]["endpoint"].as_str(),
        Some("https://my-gemini-proxy.example.com/v1beta"),
    );
    assert_eq!(
        written["security"]["auth"]["selectedType"].as_str(),
        Some("gemini-api-key"),
    );

    // Command starts with "gemini".
    assert!(
        prepared.command.starts_with("gemini"),
        "expected gemini command, got: {}",
        prepared.command
    );
}
```

- [ ] **Step 5.4: Build + clippy + tests**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -5
cargo nextest run -p warp local_harness_launch 2>&1 | tail -20
```

- [ ] **Step 5.5: Commit**

```
test(pane_group): add Gemini launch + settings.json BYOP write test

Phase 5e task 5. Extends local_harness_launch_tests with:
- Gemini added to local_child_task_config loop coverage
- New test: prepare_local_gemini_child_writes_byop_to_settings_json
  verifies that:
  - the prepared env_vars do NOT contain GEMINI_API_KEY (Gemini
    uses settings.json injection, not env vars)
  - ~/.gemini/settings.json is written under HOME=TempDir with
    security.auth.apiKey + security.auth.endpoint matching the
    BYOP config
  - selectedType remains "gemini-api-key"
  - the launched command starts with "gemini"
```

---

## Stage F -- Docs

### Task 6: Update `specs/multi-local-llm/README.md`

**Files:**
- Modify: `specs/multi-local-llm/README.md`

**Read these reference files FIRST:**
- `specs/multi-local-llm/README.md:72-91` -- Phase 5d status block (the most recent; mirror its format).

- [ ] **Step 6.1: Append Phase 5e status block**

After the existing Phase 5d status block, add:

```markdown
**Phase 5e (BYOP orchestration -- Gemini CLI as local child harness)** code is complete on `multi-local-llm` (final commit `<FILL IN>`). Lifts the `normalize_local_child_harness` filter that previously stripped `Harness::Gemini`, wires the real launch path in `prepare_local_harness_child_launch`, and injects BYOP credentials + base-URL into `~/.gemini/settings.json` (not env vars — the Gemini CLI is configured via its settings file):

- `Harness::parse_local_child_harness` (`crates/warp_cli/src/agent.rs`) now accepts `Harness::Gemini` in its output set. `normalize_local_child_harness` returns `Some(Harness::Gemini)` for `"gemini"` input.
- `prepare_local_harness_child_launch` in `pane_group/pane/local_harness_launch.rs` gains a `Harness::Gemini` arm that resolves `GeminiHarness`, validates `gemini` is on PATH, prepares Gemini environment config (settings.json auth type, trusted folders, **plus BYOP api_key + endpoint when applicable**), installs the Warp notification plugin if available, and builds the command via `build_local_gemini_child_command`.
- `prepare_gemini_environment_config` (`app/src/ai/agent_sdk/driver/harness/gemini.rs`) gains a third parameter `Option<&GeminiByopConfig>`. `GeminiAuth` gains `api_key` and `endpoint` optional fields. When the user picks a BYOP-Gemini provider + Gemini-CLI child harness, the settings.json write threads the api_key + base_url under `security.auth`. Empty/whitespace-only values are written as absent so non-BYOP runs don't inherit stale values.
- `byop_env_for_harness` in `orchestration_byop_env.rs` stays unchanged — `Harness::Gemini` remains in the catch-all empty arm. Settings.json is the single source of truth for Gemini BYOP routing.
- Phase 5a's `byop_harness_compatible` matrix already covers the Gemini+Gemini cell (`matches!(harness, "oz" | "gemini")`). No changes needed there.

> **Verification gate:** live-test smoke against the `gemini` CLI with a Gemini-API BYOP provider configured with a real `AIza...` API key and a BYOP base_url. Pick the `gemini` harness + `Local` execution + the BYOP model. Confirm `~/.gemini/settings.json` contains the BYOP `apiKey` + `endpoint` under `security.auth` after the child launches, and that subsequent traffic hits the configured endpoint with the user's key. Once smoke passes, Phase 5e row flips to checkmark.
```

- [ ] **Step 6.2: Add status-table row**

```markdown
| 5e -- BYOP orchestration Gemini CLI as local child harness | [`plan-phase-5e-gemini-cli.md`](plan-phase-5e-gemini-cli.md) | code complete -- pending live smoke |
```

- [ ] **Step 6.3: Add "What landed" bullet**

```markdown
- **Phase 5e (BYOP orchestration Gemini CLI):** Gemini CLI is now a supported local child harness for orchestration. Pick a BYOP-Gemini model + Gemini harness -> the spawned `gemini` CLI authenticates with the user's BYOP api_key and routes traffic to the user's BYOP base_url. Both are written into `~/.gemini/settings.json` under `security.auth` (`apiKey` + `endpoint`), so BYOP-Gemini providers that self-host a Gemini-compatible endpoint get full redirection — not just authentication.
```

- [ ] **Step 6.4: Add "Architecture" bullet**

```markdown
- **Phase 5e:** `Harness::parse_local_child_harness` now accepts `Gemini`. `prepare_local_harness_child_launch` gains a real `Harness::Gemini` arm (replacing `unreachable!()`) that resolves `GeminiHarness`, validates, prepares environment config, and builds the command. `prepare_gemini_environment_config` is made `pub(crate)` and gains a third `Option<&GeminiByopConfig>` parameter; `GeminiAuth` gains `api_key` + `endpoint` optional fields, written under `security.auth` when BYOP is active and cleared otherwise. `byop_env_for_harness` stays unchanged — `Harness::Gemini` deliberately returns an empty env bag because settings.json is the injection point.
```

- [ ] **Step 6.5: Update Future-phases section**

Update the Phase 5 entry to reflect 5e completion:

```markdown
- **Phase 5a--e** -- BYOP in agent orchestration. **5a--5e are all code complete on `multi-local-llm`.** 5a (foundation), 5b (Local Native path), 5c (external-CLI env-var injection: Claude Code / Codex / OpenCode), and 5e (Gemini CLI enablement, settings.json BYOP injection for api_key + endpoint) are pending live smoke. 5d (Remote credential bridge) is client-side complete; worker-side server integration is a separate server-team task.
```

- [ ] **Step 6.6: Commit**

```
docs(specs/multi-local-llm): record Phase 5e code-complete status

Phase 5e enables Gemini CLI as a local child harness for
orchestration. Lifts the normalize_local_child_harness filter,
wires the launch path with GeminiHarness validation + environment
config, and adds GEMINI_API_KEY to the BYOP env-var matrix.

Base-URL redirection is not supported (Gemini CLI limitation).
```

---

## Stage G -- Memory

### Task 7: Memory entry

**Files:**
- Create: `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/multi-local-llm-phase-5e.md`
- Modify: `/Users/nmehta/.claude/projects/-Users-nmehta-Documents-code-github-warp/memory/MEMORY.md` -- add index line.

- [ ] **Step 7.1: Write memory file** following the Phase 5d template, summarizing the same content as the README status block. List the implementation commits in order.

- [ ] **Step 7.2: Append the one-line index entry** to `MEMORY.md`:

```markdown
- [Phase 5e code complete, Gemini CLI enabled](multi-local-llm-phase-5e.md) -- Gemini CLI enabled as local child harness; BYOP GEMINI_API_KEY injection wired; base-URL redirection blocked on upstream
```

- [ ] **Step 7.3: No git commit needed** -- outside the repo.

---

## Open questions

| Question | Why it matters | Status |
|---|---|---|
| Does the Gemini CLI honor `GEMINI_API_KEY` as an env var, or only via `settings.json`? | Determines whether we inject via env var or settings.json. | **Resolved (project-owner direction):** use settings.json. Write the api_key into `security.auth.api_key` (or upstream-confirmed equivalent). No env-var injection. |
| Does the Gemini CLI support a base-URL override env var (`GEMINI_BASE_URL`, `GOOGLE_API_BASE`, or similar)? | If not, BYOP-Gemini providers that self-host an endpoint can't redirect traffic via env vars. | **Resolved (project-owner direction):** use settings.json. Write the base URL into `security.auth.endpoint` (or upstream-confirmed equivalent). Full endpoint redirection is supported via settings.json. |
| Should the `build_local_gemini_child_command` use `--sandbox=false` or `--yolo` for auto-approval in child panes? | The existing `GeminiHarnessRunner` uses `--yolo`. Consistency says use the same flag. | **Resolved:** use `--yolo` to match the existing pattern. |
| Exact upstream field names for api_key + endpoint in `~/.gemini/settings.json`. | The plan assumes `security.auth.api_key` and `security.auth.endpoint` (snake_case via `#[serde(rename_all = "camelCase")]` → `apiKey` / `endpoint` on disk). If upstream uses different keys (e.g. `apiKey` + `baseUrl`, or nests them under a different path), the typed fields on `GeminiAuth` must match. | **Verify during Task 3 implementation** by inspecting the upstream `gemini-cli` source. If different, adjust the field names in Task 3 Step 3.1 and the assertion strings in Task 3 Step 3.6 + Task 5 Step 5.3 accordingly. The strategy (settings.json injection) and architecture stay unchanged. |

---

## Self-review checklist

After implementation:

1. **Spec coverage:** Every behavior Phase 5e needs is wired: filter lift, launch path, env-var matrix, BYOP harness compatibility (confirmed existing), tests. Base-URL limitation documented.

2. **Placeholder scan:** No `TODO` / `TBD` / "handle edge cases later" in any task. The base-URL limitation is explicitly documented as an upstream dependency, not a TODO.

3. **Type consistency:** `Harness::Gemini` is the same variant across the filter (`parse_local_child_harness`), the launch path (`prepare_local_harness_child_launch`), the env-var matrix (`byop_env_for_harness`), and the tests.

4. **Backward compat:** Existing local-harness launches for Claude/Codex/OpenCode are unchanged. The filter lift is additive (Gemini was previously rejected; now it's accepted). No existing tests break.

5. **Test coverage:** Task 1 filter test update (1 assertion added, 1 removed) + Task 3 env-var tests (3 new/updated) + Task 5 launch tests (1 new integration test + 2 loop expansions) = ~6 new test assertions / 1 new integration test function.

6. **No `_` wildcards in matches:** All match arms are exhaustive throughout.

7. **Inline format args:** All `format!()` calls use inline args per `CLAUDE.md` convention.

8. **`ctx` parameter naming:** No new functions take `AppContext` -- the env-var matrix and launch path are pure functions.

---

## Plan complete

Plan complete and saved to `specs/multi-local-llm/plan-phase-5e-gemini-cli.md`. Two execution options:

**1. Subagent-Driven (recommended)** -- fresh subagent per task with two-stage review.

**2. Inline Execution** -- batched in this session.

Which approach?
