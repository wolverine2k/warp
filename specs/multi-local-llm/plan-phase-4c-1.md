# Phase 4c-1 — Capabilities resolver + settings toggle chips — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** First sub-phase of Phase 4c (multimodal attachments end-to-end). Builds the `capabilities::resolve_*` resolver in `crates/ai/` with the **Explicit user setting → 4b catalog → per-api_type heuristic → conservative-false** precedence chain, plus **Off / Auto / On** toggle chips per model row in settings for image, pdf, and audio. Sub-phase 4c-2 (data model + per-adapter wire shapes) and 4c-3 (input-bar UI + send-time enforcement) are separate plans.

**Architecture:** A new `crates/ai/src/capabilities.rs` exposes three resolver functions (`resolve_image`, `resolve_pdf`, `resolve_audio`) that take `(api_type, model_id, model_setting, catalog)` and return `bool` via the precedence chain. The heuristic table is a private function per modality, matched on `(api_type, lowercase model_id prefix)`. The settings page gains three new `AISettingsPageAction` variants that cycle the existing `image / pdf / audio: Option<bool>` field on `AgentProviderModel` through three states. The widget renders three new chips per model row beside the existing `tool_call` chip; MouseStateHandles live in a per-row 3-handle array initialized at build time.

**No send-path enforcement in this sub-phase.** The Send button continues to ignore capability state; the user-visible effect of 4c-1 is the chip UI and the (unused-by-dispatch-yet) resolver. Enforcement ships in 4c-3.

**Tech Stack:** Rust 2021, the existing WarpUI Element framework.

---

## File map

**Created:**
- `crates/ai/src/capabilities.rs` — public resolver fns + per-api_type heuristic tables.
- `crates/ai/src/capabilities_tests.rs` — unit tests covering all four precedence levels per modality.

**Modified:**
- `crates/ai/src/lib.rs` — `pub mod capabilities;`
- `app/src/settings_view/ai_page.rs` — 3 new action variants (`ToggleAgentProviderModelImage / Pdf / Audio`), 3 handler arms that cycle `Option<bool>` Off → Auto → On.
- `app/src/settings_view/agent_providers_widget.rs` — `ModelRowHandles` gains `capability_chip_states: [MouseStateHandle; 3]`; `render_model_row` renders the three chips beside the existing tool_call chip.
- `specs/multi-local-llm/README.md` + `specs/multi-local-llm/design.md` — Task 4 status flip to "🧪 code complete — pending live smoke."

---

## Stage A — Capabilities resolver

### Task 1: `capabilities.rs` + heuristic tables + tests

**Files:**
- Create: `crates/ai/src/capabilities.rs`
- Create: `crates/ai/src/capabilities_tests.rs`
- Modify: `crates/ai/src/lib.rs` — `pub mod capabilities;` next to the existing `pub mod catalog;`

**Read these reference files FIRST:**
- `crates/ai/src/catalog/parse.rs` — for the `CatalogModel` shape (`image`, `pdf`, `audio` booleans, `catalog_provider`, `id`). Resolver consumes catalog entries by `(api_type → catalog_provider, model_id)`.
- `crates/ai/src/catalog/mod.rs` — for `lookup_catalog_provider(api_type)` and `filter_models_for_api_type`. The resolver uses `lookup_catalog_provider` for non-Ollama api_types and the open-weights union pattern for Ollama.
- `crates/ai/src/local_provider/api_type.rs` (or wherever `AgentProviderApiType` lives) — for the exhaustive enum match.

- [ ] **Step 1.1: Create `capabilities.rs`**

```rust
//! Phase 4c-1: per-model multimodal capability resolution.
//!
//! Given a model's user-set capability flag (`Option<bool>` on
//! `AgentProviderModel`), an `AgentProviderApiType`, the model's id,
//! and an optional `CatalogCache`, produce a deterministic `bool`
//! per modality (image / pdf / audio). Precedence:
//!
//! 1. **Explicit user setting** — `Some(true)` / `Some(false)`.
//! 2. **4b catalog** — `(api_type, model_id)` lookup against the
//!    cached `models.dev` snapshot.
//! 3. **Per-api_type heuristic table** — encoded constants for
//!    common model families that the catalog might miss (offline,
//!    fallback-snapshot, or pre-cache-warm cases).
//! 4. **Conservative fallback** — `false`. The user gets a clear
//!    "not supported" affordance from 4c-3's send-path gate, rather
//!    than a silent upstream 4xx.
//!
//! The resolver is dispatch-side-safe (no `app/` types, no AppContext).
//! 4c-3 wires it into the input-bar Send-button predicate; 4c-2 may
//! call it for diagnostic logging when an attachment is rejected.

use crate::catalog::{lookup_catalog_provider, CatalogModel};
use crate::local_provider::AgentProviderApiType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Image,
    Pdf,
    Audio,
}

/// Resolve `(api_type, model_id, model_setting)` against the catalog
/// and the heuristic table to produce a final `bool`. `catalog` may
/// be an empty slice — the resolver still works, just falls through
/// to the heuristic and the conservative-false default.
pub fn resolve(
    modality: Modality,
    api_type: AgentProviderApiType,
    model_id: &str,
    model_setting: Option<bool>,
    catalog: &[CatalogModel],
) -> bool {
    // 1. Explicit user setting wins.
    if let Some(value) = model_setting {
        return value;
    }
    // 2. Catalog lookup. Non-Ollama uses the api_type → catalog_provider
    //    map; Ollama is a union of open-weights entries across providers.
    let catalog_match = match api_type {
        AgentProviderApiType::Ollama => {
            catalog.iter().find(|m| m.open_weights && m.id == model_id)
        }
        other => {
            let catalog_provider = lookup_catalog_provider(other);
            match catalog_provider {
                Some(provider) => catalog
                    .iter()
                    .find(|m| m.catalog_provider == provider && m.id == model_id),
                None => None,
            }
        }
    };
    if let Some(c) = catalog_match {
        return match modality {
            Modality::Image => c.image,
            Modality::Pdf => c.pdf,
            Modality::Audio => c.audio,
        };
    }
    // 3. Heuristic fallback.
    match modality {
        Modality::Image => heuristic_image(api_type, model_id),
        Modality::Pdf => heuristic_pdf(api_type, model_id),
        Modality::Audio => heuristic_audio(api_type, model_id),
    }
    // 4. (heuristic_* returns false when nothing matches → conservative default).
}

/// Convenience: per-modality wrappers callers can spell directly.
pub fn resolve_image(
    api_type: AgentProviderApiType,
    model_id: &str,
    model_setting: Option<bool>,
    catalog: &[CatalogModel],
) -> bool {
    resolve(Modality::Image, api_type, model_id, model_setting, catalog)
}

pub fn resolve_pdf(
    api_type: AgentProviderApiType,
    model_id: &str,
    model_setting: Option<bool>,
    catalog: &[CatalogModel],
) -> bool {
    resolve(Modality::Pdf, api_type, model_id, model_setting, catalog)
}

pub fn resolve_audio(
    api_type: AgentProviderApiType,
    model_id: &str,
    model_setting: Option<bool>,
    catalog: &[CatalogModel],
) -> bool {
    resolve(Modality::Audio, api_type, model_id, model_setting, catalog)
}

// ── Heuristic tables ──────────────────────────────────────────────────────────
//
// Lowercase-prefix matches per api_type. The catalog (4b) is the primary source
// of truth; these heuristics cover offline, fallback-snapshot, and first-launch
// (pre-cache) cases. New families that ship faster than this table updates are
// caught by the catalog if models.dev has them.

fn heuristic_image(api_type: AgentProviderApiType, model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    match api_type {
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp => {
            id.starts_with("gpt-4o")
                || id.starts_with("gpt-4-turbo")
                || id.starts_with("gpt-4-vision")
                || id.starts_with("o1")
                || id.starts_with("o3")
        }
        AgentProviderApiType::Anthropic => {
            id.starts_with("claude-3")
                || id.starts_with("claude-opus-4")
                || id.starts_with("claude-sonnet-4")
                || id.starts_with("claude-haiku-4")
                || id.starts_with("claude-opus-5")
                || id.starts_with("claude-sonnet-5")
        }
        AgentProviderApiType::Gemini => {
            id.starts_with("gemini-1.5")
                || id.starts_with("gemini-2")
                || id.starts_with("gemini-pro-vision")
        }
        AgentProviderApiType::Ollama => {
            id.starts_with("llava")
                || id.starts_with("bakllava")
                || id.starts_with("qwen-vl")
                || id.starts_with("qwen2-vl")
                || id.starts_with("qwen2.5-vl")
                || id.contains("-vision")
                || id.contains("llama-3.2-vision")
        }
        AgentProviderApiType::DeepSeek => false,
    }
}

fn heuristic_pdf(api_type: AgentProviderApiType, model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    match api_type {
        // OpenAI's vision models accept image inputs but not PDFs natively.
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp => false,
        // Claude 3+ models accept PDFs via the document content block.
        AgentProviderApiType::Anthropic => {
            id.starts_with("claude-3-5")
                || id.starts_with("claude-3-7")
                || id.starts_with("claude-opus-4")
                || id.starts_with("claude-sonnet-4")
                || id.starts_with("claude-haiku-4")
                || id.starts_with("claude-opus-5")
                || id.starts_with("claude-sonnet-5")
        }
        // Gemini 1.5+ accepts PDFs.
        AgentProviderApiType::Gemini => {
            id.starts_with("gemini-1.5") || id.starts_with("gemini-2")
        }
        // Ollama doesn't have a native PDF input shape (no document field).
        AgentProviderApiType::Ollama => false,
        AgentProviderApiType::DeepSeek => false,
    }
}

fn heuristic_audio(api_type: AgentProviderApiType, model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    match api_type {
        // OpenAI's gpt-4o (non-mini) accepts audio in the realtime/chat APIs.
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp => {
            id == "gpt-4o" || id.starts_with("gpt-4o-audio")
        }
        // Anthropic does not natively accept audio inputs as of this writing.
        AgentProviderApiType::Anthropic => false,
        // Gemini 1.5+ accepts audio.
        AgentProviderApiType::Gemini => {
            id.starts_with("gemini-1.5") || id.starts_with("gemini-2")
        }
        // Ollama has no native audio shape.
        AgentProviderApiType::Ollama => false,
        AgentProviderApiType::DeepSeek => false,
    }
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
```

- [ ] **Step 1.2: Create `capabilities_tests.rs`**

```rust
//! Phase 4c-1 tests for the capability resolver. Cover each precedence
//! level (explicit, catalog, heuristic, conservative-fallback) for each
//! modality (image / pdf / audio).

use crate::catalog::CatalogModel;
use crate::local_provider::AgentProviderApiType;

use super::{resolve_audio, resolve_image, resolve_pdf};

fn catalog_entry(
    provider: &str,
    id: &str,
    image: bool,
    pdf: bool,
    audio: bool,
) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: id.to_string(),
        context_window: Some(8000),
        max_output_tokens: Some(4000),
        tool_call: true,
        reasoning: false,
        image,
        pdf,
        audio,
        open_weights: false,
    }
}

// ── Level 1: explicit user setting short-circuits ──────────────────────────

#[test]
fn explicit_some_true_wins_over_catalog_and_heuristic() {
    let catalog = vec![catalog_entry("openai", "gpt-4o", false, false, false)];
    assert!(resolve_image(
        AgentProviderApiType::OpenAi,
        "gpt-4o",
        Some(true),
        &catalog,
    ));
}

#[test]
fn explicit_some_false_wins_over_catalog_and_heuristic() {
    let catalog = vec![catalog_entry("openai", "gpt-4o", true, true, true)];
    assert!(!resolve_image(
        AgentProviderApiType::OpenAi,
        "gpt-4o",
        Some(false),
        &catalog,
    ));
    assert!(!resolve_pdf(
        AgentProviderApiType::Anthropic,
        "claude-3-5-sonnet-20241022",
        Some(false),
        &catalog,
    ));
}

// ── Level 2: catalog lookup ────────────────────────────────────────────────

#[test]
fn catalog_lookup_resolves_image() {
    let catalog = vec![catalog_entry("anthropic", "claude-opus-4-7", true, true, false)];
    assert!(resolve_image(
        AgentProviderApiType::Anthropic,
        "claude-opus-4-7",
        None,
        &catalog,
    ));
}

#[test]
fn catalog_lookup_resolves_pdf_and_audio_independently() {
    let catalog = vec![catalog_entry("google", "gemini-2-pro", true, true, true)];
    assert!(resolve_image(AgentProviderApiType::Gemini, "gemini-2-pro", None, &catalog));
    assert!(resolve_pdf(AgentProviderApiType::Gemini, "gemini-2-pro", None, &catalog));
    assert!(resolve_audio(AgentProviderApiType::Gemini, "gemini-2-pro", None, &catalog));
}

#[test]
fn catalog_lookup_can_return_false_explicitly() {
    // A catalog entry that says image:false should override the heuristic.
    let catalog = vec![catalog_entry("openai", "gpt-4o", false, false, false)];
    assert!(!resolve_image(AgentProviderApiType::OpenAi, "gpt-4o", None, &catalog));
}

#[test]
fn ollama_catalog_lookup_uses_open_weights_union() {
    // Ollama models live under various catalog_providers (meta, alibaba, etc.)
    // but the resolver matches by id within the open_weights subset.
    let mut llama = catalog_entry("meta", "llama-3.2-vision-11b", true, false, false);
    llama.open_weights = true;
    let mut qwen = catalog_entry("alibaba", "qwen2-vl-72b", true, false, false);
    qwen.open_weights = true;
    let catalog = vec![llama, qwen];
    assert!(resolve_image(
        AgentProviderApiType::Ollama,
        "llama-3.2-vision-11b",
        None,
        &catalog,
    ));
    assert!(resolve_image(
        AgentProviderApiType::Ollama,
        "qwen2-vl-72b",
        None,
        &catalog,
    ));
}

// ── Level 3: heuristic table (no catalog match) ────────────────────────────

#[test]
fn heuristic_resolves_openai_gpt4o_image_true() {
    assert!(resolve_image(AgentProviderApiType::OpenAi, "gpt-4o", None, &[]));
    assert!(resolve_image(AgentProviderApiType::OpenAi, "gpt-4o-mini", None, &[]));
    assert!(resolve_image(AgentProviderApiType::OpenAi, "gpt-4-turbo", None, &[]));
    assert!(resolve_image(AgentProviderApiType::OpenAi, "o1", None, &[]));
}

#[test]
fn heuristic_resolves_anthropic_claude_3_image_true_pdf_false() {
    // Claude 3 (no -5/-7 suffix) gets image but not pdf in the heuristic.
    assert!(resolve_image(
        AgentProviderApiType::Anthropic,
        "claude-3-opus-20240229",
        None,
        &[],
    ));
    assert!(!resolve_pdf(
        AgentProviderApiType::Anthropic,
        "claude-3-opus-20240229",
        None,
        &[],
    ));
}

#[test]
fn heuristic_resolves_claude_3_5_pdf_true() {
    assert!(resolve_pdf(
        AgentProviderApiType::Anthropic,
        "claude-3-5-sonnet-20241022",
        None,
        &[],
    ));
}

#[test]
fn heuristic_resolves_gemini_all_modalities() {
    assert!(resolve_image(AgentProviderApiType::Gemini, "gemini-1.5-pro", None, &[]));
    assert!(resolve_pdf(AgentProviderApiType::Gemini, "gemini-1.5-pro", None, &[]));
    assert!(resolve_audio(AgentProviderApiType::Gemini, "gemini-1.5-pro", None, &[]));
}

#[test]
fn heuristic_resolves_ollama_llava_image_only() {
    assert!(resolve_image(AgentProviderApiType::Ollama, "llava:latest", None, &[]));
    assert!(!resolve_pdf(AgentProviderApiType::Ollama, "llava:latest", None, &[]));
    assert!(!resolve_audio(AgentProviderApiType::Ollama, "llava:latest", None, &[]));
}

#[test]
fn heuristic_deepseek_all_false() {
    assert!(!resolve_image(AgentProviderApiType::DeepSeek, "deepseek-chat", None, &[]));
    assert!(!resolve_pdf(AgentProviderApiType::DeepSeek, "deepseek-chat", None, &[]));
    assert!(!resolve_audio(AgentProviderApiType::DeepSeek, "deepseek-chat", None, &[]));
}

// ── Level 4: conservative fallback ─────────────────────────────────────────

#[test]
fn unknown_model_returns_false() {
    // No catalog entry, no heuristic match — defaults to false.
    assert!(!resolve_image(
        AgentProviderApiType::OpenAi,
        "completely-made-up-model-id",
        None,
        &[],
    ));
    assert!(!resolve_image(
        AgentProviderApiType::Anthropic,
        "claude-2-old-model",
        None,
        &[],
    ));
    assert!(!resolve_image(
        AgentProviderApiType::Ollama,
        "mistral-text-only",
        None,
        &[],
    ));
}

// ── Case-insensitivity ──────────────────────────────────────────────────────

#[test]
fn heuristic_match_is_case_insensitive() {
    assert!(resolve_image(AgentProviderApiType::OpenAi, "GPT-4O", None, &[]));
    assert!(resolve_image(AgentProviderApiType::Anthropic, "Claude-3-Opus", None, &[]));
}
```

- [ ] **Step 1.3: Wire into `crates/ai/src/lib.rs`**

Find the existing `pub mod catalog;` line and add adjacent:

```rust
pub mod capabilities;
```

(Pick the alphabetical position — between `aws_credentials` and `catalog`, or whichever spot the lib.rs ordering convention dictates. The Phase 4b review flagged out-of-order placements as a [MEDIUM]; following that guidance, `capabilities` sorts before `catalog`.)

- [ ] **Step 1.4: Build + test + clippy**

```bash
cargo build -p ai 2>&1 | tail -5            # clean
cargo nextest run -p ai capabilities 2>&1 | tail -10   # 14/14 passed
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5   # clean
```

- [ ] **Step 1.5: Commit**

```bash
git add crates/ai/src/capabilities.rs crates/ai/src/capabilities_tests.rs crates/ai/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(ai/capabilities): per-model multimodal resolver

Phase 4c-1 task 1. Adds resolve_image/resolve_pdf/resolve_audio with
the Explicit user setting → 4b catalog → per-api_type heuristic →
conservative-false precedence chain. The resolver is dispatch-side-
safe (no app/ types) so 4c-3's send-path gate and any future
runtime-side logging can call the same chain.

Heuristic tables encode the well-known families per api_type:
  - OpenAi: gpt-4o*, gpt-4-turbo*, o1*, o3* → image; gpt-4o → audio.
  - Anthropic: claude-3+ → image; claude-3-5+ / claude-{opus,sonnet,
    haiku}-4+ → image+pdf.
  - Gemini: gemini-1.5+ / gemini-2+ → image+pdf+audio.
  - Ollama: llava*, bakllava*, qwen-vl*, *-vision* → image only.
  - DeepSeek: text-only.

14 unit tests cover each precedence level for each modality:
explicit-true/false short-circuit, catalog hits for all four
modalities, Ollama open-weights union, heuristic positives and
negatives per family, case-insensitive matching, and the
unknown-model conservative-false fallback.

No send-path enforcement yet — that lands in 4c-3. 4c-1 ships the
resolver alone so 4c-2 (data model + per-adapter wire) can be
implemented without waiting on the UI gate.
EOF
)"
```

---

## Stage B — Settings UI

### Task 2: Action variants + handler arms

**Files:**
- Modify: `app/src/settings_view/ai_page.rs` — 3 new action variants + 3 handler arms.

**Read these reference files FIRST:**
- `app/src/settings_view/ai_page.rs` around `ToggleAgentProviderModelToolCall` — the existing per-model toggle action. Phase 4c-1's action variants mirror its shape but cycle a three-state `Option<bool>` instead of flipping a `bool`.

- [ ] **Step 2.1: Add 3 action variants**

Append to `AISettingsPageAction`, immediately after `ToggleAgentProviderModelToolCall`:

```rust
/// Phase 4c-1. Cycle the model's `image: Option<bool>` field through
/// Off (`Some(false)`) → Auto (`None`) → On (`Some(true)`) → Off.
ToggleAgentProviderModelImage {
    provider_index: usize,
    model_index: usize,
},

/// Phase 4c-1. Same as `ToggleAgentProviderModelImage` for `pdf`.
ToggleAgentProviderModelPdf {
    provider_index: usize,
    model_index: usize,
},

/// Phase 4c-1. Same as `ToggleAgentProviderModelImage` for `audio`.
ToggleAgentProviderModelAudio {
    provider_index: usize,
    model_index: usize,
},
```

- [ ] **Step 2.2: Add a `cycle_option_bool` helper at module-private scope**

Above (or near) the handler match, before the `impl TypedActionView` block:

```rust
/// Phase 4c-1. Three-state cycle for capability chips:
///   None (Auto) → Some(true) (On) → Some(false) (Off) → None (Auto) …
///
/// The ordering matches the chip UX: clicking through is
/// "indeterminate" → "explicit on" → "explicit off" → "indeterminate."
fn cycle_capability_state(current: Option<bool>) -> Option<bool> {
    match current {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}
```

- [ ] **Step 2.3: Add 3 handler arms**

Insert inside the `handle_action` match, immediately after the `ToggleAgentProviderModelToolCall` arm:

```rust
AISettingsPageAction::ToggleAgentProviderModelImage {
    provider_index,
    model_index,
} => {
    let provider_index = *provider_index;
    let model_index = *model_index;
    AISettings::handle(ctx).update(ctx, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        if let Some(p) = providers.get_mut(provider_index) {
            if let Some(m) = p.models.get_mut(model_index) {
                m.image = cycle_capability_state(m.image);
                report_if_error!(settings.agent_providers.set_value(providers, ctx));
            }
        }
    });
    ctx.notify();
}

AISettingsPageAction::ToggleAgentProviderModelPdf {
    provider_index,
    model_index,
} => {
    let provider_index = *provider_index;
    let model_index = *model_index;
    AISettings::handle(ctx).update(ctx, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        if let Some(p) = providers.get_mut(provider_index) {
            if let Some(m) = p.models.get_mut(model_index) {
                m.pdf = cycle_capability_state(m.pdf);
                report_if_error!(settings.agent_providers.set_value(providers, ctx));
            }
        }
    });
    ctx.notify();
}

AISettingsPageAction::ToggleAgentProviderModelAudio {
    provider_index,
    model_index,
} => {
    let provider_index = *provider_index;
    let model_index = *model_index;
    AISettings::handle(ctx).update(ctx, |settings, ctx| {
        let mut providers = settings.agent_providers.value().clone();
        if let Some(p) = providers.get_mut(provider_index) {
            if let Some(m) = p.models.get_mut(model_index) {
                m.audio = cycle_capability_state(m.audio);
                report_if_error!(settings.agent_providers.set_value(providers, ctx));
            }
        }
    });
    ctx.notify();
}
```

- [ ] **Step 2.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 2.5: Commit**

```bash
git add app/src/settings_view/ai_page.rs
git commit -m "$(cat <<'EOF'
feat(app/settings_view/ai_page): wire 4c-1 capability toggle actions

Phase 4c-1 task 2. Adds three new AISettingsPageAction variants
(ToggleAgentProviderModelImage / Pdf / Audio) + their handler arms
plus a module-private cycle_capability_state(Option<bool>) helper
that walks the three-state cycle (Auto → On → Off → Auto).

Each handler mutates the AgentProviderModel's image / pdf / audio
field via the existing AISettings::handle(ctx).update path so the
change persists to settings.toml automatically (mirrors how
ToggleAgentProviderModelToolCall works for the bool field).

No chip rendering yet — that lands in Task 3.
EOF
)"
```

---

### Task 3: Widget — 3 capability chips per model row

**Files:**
- Modify: `app/src/settings_view/agent_providers_widget.rs` — `ModelRowHandles` gains a chip-state array; `render_model_row` renders the chips.

**Read these reference files FIRST:**
- `agent_providers_widget.rs::ModelRowHandles` — existing struct, already holds `tool_call_chip_state`. You add a `[MouseStateHandle; 3]` array alongside it.
- `agent_providers_widget.rs::build_model_row` — where the new chip-state array is initialized at construction (avoiding the inline `MouseStateHandle::default()` pitfall from CLAUDE.md).
- `agent_providers_widget.rs::render_model_row` — where the tool_call chip renders today; the three new chips slot in next to it.
- `crates/ai/src/capabilities.rs` (from Task 1) — the resolver the chip-label code calls to compute the dim "Auto (on/off)" hint.

- [ ] **Step 3.1: Add chip-state pool to `ModelRowHandles`**

```rust
struct ModelRowHandles {
    name_editor: ViewHandle<EditorView>,
    id_editor: ViewHandle<EditorView>,
    context_editor: ViewHandle<EditorView>,
    tool_call_chip_state: MouseStateHandle,
    remove_button_state: MouseStateHandle,
    quick_add_chip_states: [MouseStateHandle; 5],  // Phase 4b, unchanged
    /// Phase 4c-1. Mouse-state handles for the image / pdf / audio
    /// capability chips. Allocated at row-build time so render never
    /// builds MouseStateHandle::default() inline.
    /// Index 0 = image, 1 = pdf, 2 = audio.
    capability_chip_states: [MouseStateHandle; 3],
}
```

- [ ] **Step 3.2: Initialize in `build_model_row`**

In the `ModelRowHandles { … }` construction at the bottom of `build_model_row`:

```rust
capability_chip_states: [
    MouseStateHandle::default(),
    MouseStateHandle::default(),
    MouseStateHandle::default(),
],
```

- [ ] **Step 3.3: Render the three chips in `render_model_row`**

`render_model_row` currently takes:

```rust
fn render_model_row(
    provider_index: usize,
    provider_api_type: AgentProviderApiType,
    model_index: usize,
    model: &AgentProviderModel,
    row: &ModelRowHandles,
    view: &AISettingsPageView,
    appearance: &Appearance,
) -> Box<dyn Element>
```

(`provider_api_type` and `view` were added in Phase 4b Task 8 for the chips.)

Add a helper `render_capability_chip(label_glyph, state, current, resolved, action, appearance)` near `render_card_button` that produces a Secondary-themed button with a tri-state label:

```rust
/// Phase 4c-1. Renders a three-state capability chip (Off / Auto / On).
/// The label combines a modality glyph (🖼️ / 📄 / 🎙️) with the current
/// user state and (when current is `None`) the resolver-inferred value
/// in dim "Auto (on)" / "Auto (off)" form.
fn render_capability_chip(
    glyph: &str,
    mouse_state: MouseStateHandle,
    current: Option<bool>,
    resolved: bool,
    action: AISettingsPageAction,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let label = match current {
        Some(true) => format!("{glyph} On"),
        Some(false) => format!("{glyph} Off"),
        None => {
            if resolved {
                format!("{glyph} Auto (on)")
            } else {
                format!("{glyph} Auto (off)")
            }
        }
    };
    Self::render_card_button(label, mouse_state, action, appearance)
}
```

Inside `render_model_row`, near where `tool_call_chip` is rendered, build the three chip elements and append them to the same Flex::row:

```rust
let catalog_slice: &[ai::catalog::CatalogModel] = view
    .catalog_cache
    .as_ref()
    .map(|c| c.all())
    .unwrap_or(&[]);

let resolved_image = ai::capabilities::resolve_image(
    provider_api_type,
    &model.id,
    model.image,
    catalog_slice,
);
let resolved_pdf = ai::capabilities::resolve_pdf(
    provider_api_type,
    &model.id,
    model.pdf,
    catalog_slice,
);
let resolved_audio = ai::capabilities::resolve_audio(
    provider_api_type,
    &model.id,
    model.audio,
    catalog_slice,
);

let image_chip = Self::render_capability_chip(
    "🖼️",
    row.capability_chip_states[0].clone(),
    model.image,
    resolved_image,
    AISettingsPageAction::ToggleAgentProviderModelImage {
        provider_index,
        model_index,
    },
    appearance,
);
let pdf_chip = Self::render_capability_chip(
    "📄",
    row.capability_chip_states[1].clone(),
    model.pdf,
    resolved_pdf,
    AISettingsPageAction::ToggleAgentProviderModelPdf {
        provider_index,
        model_index,
    },
    appearance,
);
let audio_chip = Self::render_capability_chip(
    "🎙️",
    row.capability_chip_states[2].clone(),
    model.audio,
    resolved_audio,
    AISettingsPageAction::ToggleAgentProviderModelAudio {
        provider_index,
        model_index,
    },
    appearance,
);
```

Append these to the chip row that already holds `tool_call_chip`. Each chip needs a `Container::with_margin_left(6.)` wrapper (matching the existing chip spacing pattern in `render_api_type_field`):

```rust
let chip_row = Flex::row()
    .with_cross_axis_alignment(CrossAxisAlignment::Center)
    .with_child(tool_call_chip)
    .with_child(Container::new(image_chip).with_margin_left(6.).finish())
    .with_child(Container::new(pdf_chip).with_margin_left(6.).finish())
    .with_child(Container::new(audio_chip).with_margin_left(6.).finish())
    .finish();
```

(Adapt to whatever the current chip-row construction in `render_model_row` looks like — the exact lines may differ. The point is: three new chips slot in beside the existing tool_call chip with 6px spacing.)

- [ ] **Step 3.4: Build + clippy**

```bash
cargo build -p warp 2>&1 | tail -5
cargo clippy -p warp --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 3.5: Commit**

```bash
git add app/src/settings_view/agent_providers_widget.rs
git commit -m "$(cat <<'EOF'
feat(app/settings_view/agent_providers_widget): capability chips per model row

Phase 4c-1 task 3. Renders three new tri-state capability chips
(🖼️ image · 📄 pdf · 🎙️ audio) beside the existing tool_call chip
on each model row. Each chip is a Secondary button that cycles
the user setting Off (Some(false)) → Auto (None) → On (Some(true))
on click.

When in Auto mode, the chip label shows the resolver-inferred value
in dim text — e.g., "🖼️ Auto (on)" if the model's catalog entry
or per-api_type heuristic says the modality is supported. This
makes the implicit state inspectable without forcing every user
to set every flag.

ModelRowHandles gains a [MouseStateHandle; 3] pool pre-allocated at
build time per the repeated-init pitfall in CLAUDE.md.

The chips have no send-path effect yet — 4c-3 wires the resolver
into the Send button's enabled-state predicate.
EOF
)"
```

---

## Stage C — Verification

### Task 4: Manual smoke + spec docs status flip

**Files:**
- Modify: `specs/multi-local-llm/README.md` — add Phase 4c-1 status paragraph, add status-table row, add user-visible bullet, add architecture bullet.
- Modify: `specs/multi-local-llm/design.md` — flag the §9 row for 4c with "🧪 code complete (4c-1)" (or similar partial-completion marker).

- [ ] **Step 4.1: Manual smoke**

```text
[ ] Open Settings → AI → Custom AI Providers. Add a provider card
    (api_type = Anthropic for an interesting capability mix; Ollama
    for the open-weights union test). Add one model row with id
    `claude-opus-4-7` (or `llava` for Ollama).
[ ] Verify the three new chips render: 🖼️ Auto (on), 📄 Auto (on),
    🎙️ Auto (off) for Claude; 🖼️ Auto (on), 📄 Auto (off), 🎙️ Auto
    (off) for Ollama llava.
[ ] Click each chip. Verify it cycles On → Off → Auto → On and the
    label updates in place each click. Verify settings.toml persists
    the new value (the file should contain image = true / pdf = false
    / etc.).
[ ] Change the api_type chip to a different family (e.g., Anthropic
    → Gemini). Confirm the chip labels update to reflect the new
    resolver result (Gemini's heuristic says audio is supported, so
    the audio chip should flip from "Auto (off)" to "Auto (on)").
[ ] Restart the app. Confirm the user-set chips (On/Off, not Auto)
    persist; the Auto chips re-resolve against the current catalog +
    heuristic.
```

Pass criterion: all five checkpoints succeed. If any fails, file in this plan's §Risks; block the sub-phase only if it's a 4c-1 regression (not a Phase 4b catalog gap or a UI layering issue from earlier phases).

- [ ] **Step 4.2: Update `specs/multi-local-llm/README.md`**

Add a status paragraph after Phase 4b's paragraph:

```markdown
**Phase 4c-1 (capabilities resolver + settings toggle chips)** code is complete on `multi-local-llm` (final commit `<TBD>`). First of three sub-phases for Phase 4c (multimodal attachments end-to-end). Adds a `crates/ai/src/capabilities.rs` resolver with the **Explicit user setting → 4b catalog → per-api_type heuristic → conservative-false** precedence chain, plus three Off/Auto/On toggle chips per model row in settings (🖼️ image · 📄 pdf · 🎙️ audio). **No send-path enforcement yet** — 4c-3 wires the resolver into the Send button's enabled-state predicate; 4c-2 builds the data model and per-adapter wire shapes in between. **14 new unit tests** on the resolver (one per precedence level per modality plus case-insensitivity); existing test suites stay green.

> **Verification gate:** manual settings-UI smoke against each of the five active api_types (OpenAI, Anthropic, Ollama, Gemini, DeepSeek). Confirm the chips render with the correct Auto-resolved state, cycle Off/Auto/On on click, and persist across restarts. Once smoke passes, the Phase 4c row in the status table flips to "🧪 4c-1 shipped, 4c-2/4c-3 pending" until all three sub-phases land.
```

Add a row to the status table (after the Phase 4b row):

```markdown
| 4c-1 — Capabilities resolver + Off/Auto/On chips per model row | [`plan-phase-4c-1.md`](plan-phase-4c-1.md) | 🧪 code complete — pending live smoke |
```

Add a What-landed bullet under "User-visible":

```markdown
- **Phase 4c-1 (pending live smoke):** three new tri-state capability chips per model row in Settings → AI (🖼️ image · 📄 pdf · 🎙️ audio). Cycles Off / Auto / On on click. The Auto state shows the resolver-inferred value in dim text. No send-path effect yet — 4c-3 wires these in.
```

Add an Architecture bullet:

```markdown
- **Phase 4c-1:** New `crates/ai/src/capabilities.rs` resolver with `resolve_image / resolve_pdf / resolve_audio(api_type, model_id, model_setting, catalog) -> bool`. Precedence chain: explicit user setting > 4b catalog > per-api_type heuristic table (`gpt-4o*` / `claude-3+` / `gemini-1.5+` / `llava*` etc.) > conservative-false. Three new `AISettingsPageAction` variants cycle the existing `Option<bool>` fields; widget renders the chips with a `[MouseStateHandle; 3]` pool on `ModelRowHandles`.
```

- [ ] **Step 4.3: Update `specs/multi-local-llm/design.md` §9 row**

Change the existing 4c row from no status flag to a partial-completion marker:

```markdown
| **4c. Multimodal attachments end-to-end** | (existing description; 4c-1 shipped, 4c-2 + 4c-3 pending) | (existing files) | (existing gate) |
```

Or, if you prefer the rolling-update format from prior phases, add `🧪 4c-1 code complete` to the row title.

- [ ] **Step 4.4: Commit**

```bash
git add specs/multi-local-llm/README.md specs/multi-local-llm/design.md
git commit -m "$(cat <<'EOF'
docs(specs/multi-local-llm): record Phase 4c-1 code-complete status

Phase 4c-1 (capabilities resolver + settings toggle chips) shipped
end-to-end on multi-local-llm. First of three sub-phases for
Phase 4c (multimodal attachments end-to-end); 4c-2 (data model +
per-adapter wire shapes) and 4c-3 (input UI + send-time
enforcement) are queued.

Status table row flips from queued to "🧪 4c-1 code complete —
pending live smoke." README adds the status paragraph mirroring
4a/4b shape; design.md §9 row gets the partial-completion marker.
EOF
)"
```

---

## Final verification

- [ ] **Verification 1: Sweeps** — `crates/ai/src/capabilities.rs` is self-contained; no churn outside the listed files. `crates/ai/src/lib.rs` gains exactly one `pub mod capabilities;`. No new feature flag introduced (gated by the existing `LocalLlmProvider` via the parent widget). No persistence schema change (`image / pdf / audio: Option<bool>` were added in Phase 1b-1).
- [ ] **Verification 2: Build + tests + clippy** — `cargo build -p ai && cargo build -p warp` clean; `cargo nextest run -p ai capabilities` shows 14/14 passing; `cargo clippy -p ai --all-targets --all-features -- -D warnings` clean; `cargo clippy -p warp --lib --tests -- -D warnings` clean.
- [ ] **Verification 3: Manual smoke** — 5/5 checkpoints in Task 4.1 pass.
- [ ] **Verification 4: Final reviewer + push** — dispatch `oh-my-claudecode:code-reviewer` for the full Phase 4c-1 diff. Stop before push; user reviews, then pushes manually.

```bash
git log --oneline c2fdc61c..HEAD
# Expected: 4 task commits + 1 design commit (1058b90a, already committed):
#   <sha> docs(specs/multi-local-llm): record Phase 4c-1 code-complete status
#   <sha> feat(app/settings_view/agent_providers_widget): capability chips per model row
#   <sha> feat(app/settings_view/ai_page): wire 4c-1 capability toggle actions
#   <sha> feat(ai/capabilities): per-model multimodal resolver
```

---

## Risks & open questions

1. **Heuristic-table churn.** The heuristic constants encode current model families; new families (gpt-5*, claude-opus-6-*, gemini-3-*) ship faster than this table updates. **Mitigation:** the catalog (Phase 4b) takes precedence over the heuristic — as long as models.dev tracks the new family, the heuristic doesn't need to. The heuristic is a fallback for the offline / fallback-snapshot / pre-cache-warm cases only.
2. **Catalog-vs-heuristic divergence.** A model could resolve to one value from the catalog and a different value from the heuristic (e.g., user is offline, snapshot says false, heuristic prefix-match says true). The resolver always prefers the catalog when present; that's the designed behavior, but it means a fresh-snapshot-only user could see different Auto resolutions than someone with a warm catalog. **Acceptable** — the chip's "Auto (on/off)" label always reflects the *current* resolution, so the user sees what dispatch will use.
3. **Three chips per row crowds the layout.** With tool_call + 3 capability chips + the existing remove button + the per-row inputs, a narrow window may overflow horizontally. **Mitigation:** the existing widget uses horizontal scroll for chip rows; defer dedicated layout work until 4c-3's input UI surfaces the real overflow signal.
4. **Three-state UX is inherently confusing.** Off / Auto / On is one more state than most settings have. **Mitigation:** the "Auto (on/off)" label suffix is the disambiguation — clicking the chip walks all three states linearly so the user can always observe what each state means. A future polish step could add a hover tooltip explaining the cycle.
5. **`audio: gpt-4o == true` is approximate.** OpenAI's audio support varies by API (realtime vs. completions). The heuristic flips `audio = true` for `gpt-4o`-exact, which is correct for the Realtime API but not for `gpt-4o-mini`. 4c-3's send-path gate will surface upstream rejections for the user to override via the Off chip. **Acceptable trade-off** for first ship.

---

## Next plan (Phase 4c-2 — Data model + per-adapter wire shapes)

Phase 4c-2 introduces the `AgentAttachment { mime, bytes, display_name }` struct in `crates/ai/src/attachments.rs`, threads an `attachments: Vec<AgentAttachment>` field onto `LocalProviderInput`, and updates each of the five active adapters' request translators to carry attachments in the upstream's wire shape:

- OpenAi / DeepSeek: `content` becomes an array with `image_url` parts (base64 data-URI).
- Anthropic: `content` blocks with `image` source (`type: "base64"`) for image; `document` block for pdf.
- Gemini: `parts` with `inline_data` (base64 + mime).
- Ollama: user message gains `images: Vec<base64>` (image-only).

Per-adapter unit tests against fixtures of each upstream's documented request shape. **No input-bar UI yet** — 4c-3 builds the file picker, ties the resolver into the Send button's enabled-state predicate, and decides on attachment persistence.
