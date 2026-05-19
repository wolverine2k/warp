# Phase 4c-3 — Input UI + send-time enforcement + history rendering — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Touching UI? Read `warp-ui-guidelines` first.

**Goal:** Third and final sub-phase of Phase 4c (multimodal attachments end-to-end). Surfaces attachments to the user via a 📎 file-picker button, drag-and-drop, and paste-from-clipboard; gates the Send button on the 4c-1 capability resolver; renders attachments inline in the conversation transcript. Wires the in-memory `pending_attachments` state into `LocalProviderInput.attachments` at the single dispatch site so 4c-2's already-shipping wire path carries them upstream. Persists **only metadata** (mime + display_name) into the existing `agent_conversations.conversation_data` JSON blob — bytes stay session-scoped.

**Decisions locked in (see brainstorm 2026-05-19):**

| Decision | Choice |
|---|---|
| Persistence | Bytes session-only. Metadata (mime + display_name) persisted into the existing conversation_data JSON blob. No Diesel migration. |
| Multi-attach | `MAX_ATTACHMENTS_PER_TURN = 5`. |
| Attach mechanisms | 📎 picker (mime-filtered by capability resolver) + drag-and-drop onto the input footer + paste-from-clipboard image data. |
| Capability gate | Both: block at attach-time (picker mime filter; drag/paste reject with inline toast) **and** at send-time (chip red border + Send button disabled). |
| File size cap | `MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024` (20 MiB) per file. No per-turn aggregate cap. |
| Thumbnail | Decoded once at attach-time on a background task; cached on `AgentAttachment.thumbnail_bytes` (Option<Vec<u8>>) at `THUMBNAIL_DIM = 128` px. Render path uses the pre-decoded thumbnail. |
| Reload UX | `🖼️ apple.png • not available` dimmed chip placeholder for prior-turn attachments. |

**Architecture:** A new `AttachmentInputValidator` is the single funnel that picker, drag-drop, and paste all use to validate `(mime, byte_len, current_pending_count, active_model_caps)`. On Ok, the attachment is decoded for thumbnail (image/*) and appended to `AgentInputFooter.pending_attachments: Vec<AgentAttachment>` (in-memory). The send-button enabled-state predicate calls the 4c-1 resolver per modality per pending chip — false if any chip's modality is unsupported by the active model. On submit, `pending_attachments` is moved into the `LocalProviderInput.attachments` field at `app/src/ai/agent/api/impl.rs:274` (the existing 4c-2-shipped wire path takes it upstream). Metadata-only (no bytes) is written to the persisted conversation blob alongside the user-query text so post-reload history renders a "not available" chip with the filename.

**Tech Stack:** Rust 2021, WarpUI Entity-Component-Handle framework, `serde` / `serde_json`, `image` crate for thumbnail decoding (likely already a workspace dep).

---

## Per-touchpoint reference

| Concern | Source of truth |
|---|---|
| Capability resolver | `crates/ai/src/capabilities.rs` — `resolve_image` / `resolve_pdf` / `resolve_audio` |
| Wire path (already done in 4c-2) | `LocalProviderInput.attachments: Vec<AgentAttachment>` at `crates/ai/src/local_provider/request.rs:77` |
| Dispatch site (where to thread `attachments`) | `app/src/ai/agent/api/impl.rs:274` |
| Conversation persistence shape | `AgentConversationData` (struct serialized into `agent_conversations.conversation_data`); see `app/src/persistence/agent.rs:38-52` |
| Input footer view | `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` (2735 lines) + helper modules (`chips.rs`, `toolbar_item.rs`, `editor.rs`) |
| Toolbar item kind enum | `AgentToolbarItemKind` in `app/src/ai/blocklist/agent_view/agent_input_footer/toolbar_item.rs` — extend with `AttachmentPicker` variant |
| WarpUI drag-drop primitives | `crates/warpui/src/windowing/winit/event_loop/mod.rs` (existing) |
| Image decode/thumbnail | `image` crate (workspace dep — verify in Task 4 prep) |

---

## File map

**Created:**
- `crates/ai/src/attachments.rs` — extend with `AttachmentMetadata { mime, display_name }` companion type (serializable, no bytes).
- `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_input_validator.rs` — shared validator funnel.
- `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_input_validator_tests.rs` — sibling unit tests.
- `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_chip.rs` — chip strip widget for both input area and transcript user-turn rendering.
- `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_chip_tests.rs` — sibling unit tests.

**Modified:**
- `crates/ai/src/attachments.rs` — extend `AgentAttachment` with `thumbnail_bytes: Option<Vec<u8>>` (default None; never serialized — runtime-only).
- `crates/ai/src/local_provider/request.rs` — no change (`attachments` field already wired in 4c-2).
- `app/src/ai/agent/api/impl.rs` — thread `attachments` into `LocalProviderInput { .., attachments }` at line ~274. Source: forwarded from the controller param chain.
- `app/src/ai/agent/conversation.rs` / `app/src/ai/agent/api.rs` — extend the params chain to carry `attachments: Vec<AgentAttachment>` from the input footer down to the dispatch site.
- `app/src/ai/blocklist/agent_view/agent_input_footer/toolbar_item.rs` — add `AgentToolbarItemKind::AttachmentPicker` variant + display_label + icon + default-position wiring.
- `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` — add `pending_attachments` field, picker handler, drag-drop event hook, paste-from-clipboard interception, send-button predicate extension, attachment-aware submit flow.
- `app/src/persistence/agent.rs` — no change (`AgentConversationData` is serialized verbatim; extension happens in the type def file).
- Wherever `AgentConversationData` is defined — add `#[serde(default)] attachments: Vec<AttachmentMetadata>` to the per-exchange message struct. Find via `grep -rn "pub struct AgentConversationData"`.
- Wherever user-turn rendering happens in the conversation transcript — render `attachment_chip` widgets above the user's text. Find via `grep -rn "render_user\|user_query_block\|UserMessageBlock"`.
- `specs/multi-local-llm/README.md` — append Phase 4c-3 status paragraph + table row + bullets.
- `specs/multi-local-llm/design.md` — flip §9 row for Phase 4c (4c-1 + 4c-2 + 4c-3 code complete).

---

## Stage A — Data layer

### Task 1: `AttachmentMetadata` + runtime `thumbnail_bytes` on `AgentAttachment`

**Files:**
- Modify: `crates/ai/src/attachments.rs` — add `AttachmentMetadata` type + `thumbnail_bytes` field on `AgentAttachment`.
- Modify: `crates/ai/src/attachments_tests.rs` — add 4 tests on the new type/field.

**Read these reference files FIRST:**
- `crates/ai/src/attachments.rs` (full file, ~125 lines) — current `AgentAttachment` shape.
- `crates/ai/src/attachments_tests.rs` — existing test patterns.
- `git show fb127490` — Task 1 of 4c-2 (the commit that first introduced this file) for established conventions.

- [ ] **Step 1.1: Add `AttachmentMetadata` type**

```rust
/// Phase 4c-3. Persistence-friendly companion to `AgentAttachment`.
/// Carries the user-visible name and mime type only — no bytes. Serialized
/// into the conversation history JSON blob so post-reload history rendering
/// can show a "not available" placeholder with the original filename.
///
/// `AgentAttachment` (the full runtime type) is NEVER serialized; bytes are
/// session-scoped by design (see brainstorm 2026-05-19).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttachmentMetadata {
    pub mime: String,
    pub display_name: Option<String>,
}

impl From<&AgentAttachment> for AttachmentMetadata {
    fn from(att: &AgentAttachment) -> Self {
        Self {
            mime: att.mime.clone(),
            display_name: att.display_name.clone(),
        }
    }
}
```

- [ ] **Step 1.2: Add `thumbnail_bytes` field to `AgentAttachment`**

```rust
pub struct AgentAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
    pub display_name: Option<String>,
    /// Phase 4c-3. Pre-decoded thumbnail bytes (128px PNG) for image
    /// attachments. Decoded once at attach-time on a background task so the
    /// chip-render path never blocks the UI thread. `None` for non-image
    /// modalities (pdf/audio) and for images whose decoding failed (in which
    /// case the chip falls back to the generic icon + filename rendering).
    /// Runtime-only — never serialized into the conversation blob.
    pub thumbnail_bytes: Option<Vec<u8>>,
}
```

All existing struct literals (in `crates/ai/src/local_provider/**/request_tests.rs`, `attachments_tests.rs`, etc.) need `thumbnail_bytes: None`. Find all via `cargo build -p ai` and fix the resulting compile errors.

- [ ] **Step 1.3: 4 unit tests**

```rust
#[test]
fn metadata_from_attachment_drops_bytes() {
    let att = AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![1, 2, 3, 4],
        display_name: Some("test.png".into()),
        thumbnail_bytes: None,
    };
    let md = AttachmentMetadata::from(&att);
    assert_eq!(md.mime, "image/png");
    assert_eq!(md.display_name, Some("test.png".into()));
}

#[test]
fn metadata_serde_round_trip() { /* serde_json round-trip; assert bytes are NOT present */ }

#[test]
fn metadata_deserializes_with_missing_display_name() { /* {"mime":"image/png"} → display_name: None */ }

#[test]
fn attachment_thumbnail_bytes_default_none() {
    let att = AgentAttachment {
        mime: "application/pdf".into(),
        bytes: vec![],
        display_name: None,
        thumbnail_bytes: None,
    };
    assert!(att.thumbnail_bytes.is_none());
}
```

- [ ] **Step 1.4: Build + test + clippy + commit**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai attachments 2>&1 | tail -10
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

Expect: all existing tests pass after adding `thumbnail_bytes: None` to literals. ~4 new tests.

Commit:
```
feat(ai/attachments): AttachmentMetadata + runtime thumbnail field

Phase 4c-3 task 1. Adds AttachmentMetadata { mime, display_name } —
the persistence-friendly companion to AgentAttachment serialized into
the conversation history JSON blob (bytes stay session-scoped).
AgentAttachment gains a runtime-only thumbnail_bytes field (Option<Vec<u8>>)
populated at attach-time by Task 4's thumbnail decoder so chip rendering
never blocks the UI thread.

4 new unit tests cover the From impl, serde round-trip, missing-field
deserialization, and the default-None thumbnail.
```

---

### Task 2: `AttachmentInputValidator` shared funnel

**Files:**
- Create: `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_input_validator.rs`
- Create: `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_input_validator_tests.rs`
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` — `mod attachment_input_validator;`

**Read these reference files FIRST:**
- `crates/ai/src/capabilities.rs` — full file (~120 lines). Confirm exact resolver signatures.
- `crates/ai/src/attachments.rs` — `is_image`/`is_pdf`/`is_audio` modality helpers.

- [ ] **Step 2.1: Validator module**

```rust
//! Phase 4c-3. Shared input gate for the three attach mechanisms
//! (📎 picker, drag-drop, paste-from-clipboard). Lives in app/ rather than
//! crates/ai/ because capability lookup depends on the user-settings store
//! that's app-side.

use ai::attachments::AgentAttachment;
use ai::capabilities::{resolve_audio, resolve_image, resolve_pdf};

pub const MAX_ATTACHMENTS_PER_TURN: usize = 5;
pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024; // 20 MiB

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentRejection {
    /// The active model doesn't support this modality (image/pdf/audio).
    UnsupportedModality { modality: Modality, model_id: String },
    /// File exceeds MAX_ATTACHMENT_BYTES.
    FileTooLarge { actual_bytes: usize },
    /// Pending pile is already at MAX_ATTACHMENTS_PER_TURN.
    TurnLimitReached,
    /// Mime type is not image/* / application/pdf / audio/* — we don't
    /// recognize the file kind so we can't reason about it.
    UnknownMime { mime: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Image,
    Pdf,
    Audio,
}

/// Source of truth for the capability resolver inputs. Constructed by the
/// caller from the active settings store + model row.
pub struct ActiveModelCaps<'a> {
    pub model_id: &'a str,
    pub api_type: &'a str,
    pub explicit_image_setting: ai::capabilities::TriState,
    pub explicit_pdf_setting: ai::capabilities::TriState,
    pub explicit_audio_setting: ai::capabilities::TriState,
    /* + any catalog metadata the resolver consumes; mirror crate::ai::capabilities API */
}

pub fn validate(
    candidate: &AgentAttachment,
    current_pending_count: usize,
    caps: &ActiveModelCaps<'_>,
) -> Result<(), AttachmentRejection> {
    if current_pending_count >= MAX_ATTACHMENTS_PER_TURN {
        return Err(AttachmentRejection::TurnLimitReached);
    }
    if candidate.bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentRejection::FileTooLarge {
            actual_bytes: candidate.bytes.len(),
        });
    }
    if candidate.is_image() {
        if !resolve_image(caps.api_type, caps.model_id, caps.explicit_image_setting) {
            return Err(AttachmentRejection::UnsupportedModality {
                modality: Modality::Image,
                model_id: caps.model_id.to_string(),
            });
        }
    } else if candidate.is_pdf() {
        if !resolve_pdf(caps.api_type, caps.model_id, caps.explicit_pdf_setting) {
            return Err(AttachmentRejection::UnsupportedModality {
                modality: Modality::Pdf,
                model_id: caps.model_id.to_string(),
            });
        }
    } else if candidate.is_audio() {
        if !resolve_audio(caps.api_type, caps.model_id, caps.explicit_audio_setting) {
            return Err(AttachmentRejection::UnsupportedModality {
                modality: Modality::Audio,
                model_id: caps.model_id.to_string(),
            });
        }
    } else {
        return Err(AttachmentRejection::UnknownMime {
            mime: candidate.mime.clone(),
        });
    }
    Ok(())
}

/// Human-readable toast text for a rejection. UI uses this directly.
pub fn rejection_message(rej: &AttachmentRejection) -> String { /* … */ }

#[cfg(test)]
#[path = "attachment_input_validator_tests.rs"]
mod tests;
```

**Note:** The exact `ActiveModelCaps` field set must match the parameters the real 4c-1 resolver takes — read `crates/ai/src/capabilities.rs` and pull the *actual* TriState / parameter shape. The block above is illustrative; the implementer should mirror the real resolver signature exactly.

- [ ] **Step 2.2: 8 unit tests**

Cover each rejection path:
1. `valid_image_returns_ok` — happy path image/png with image_supported=true.
2. `valid_pdf_returns_ok`.
3. `valid_audio_returns_ok`.
4. `image_on_image_unsupported_model_rejects` — `UnsupportedModality { Image, .. }`.
5. `pdf_on_pdf_unsupported_model_rejects`.
6. `audio_on_audio_unsupported_model_rejects`.
7. `file_too_large_rejects_at_boundary` — bytes.len() == MAX + 1 → `FileTooLarge`; MAX exactly → Ok.
8. `turn_limit_reached_rejects_when_pile_at_max` — current_pending_count == 5 → `TurnLimitReached`.

Plus one for `unknown_mime_rejects` (e.g., `text/plain`).

- [ ] **Step 2.3: Build + test + clippy + commit**

Commit:
```
feat(agent_input_footer): AttachmentInputValidator shared gate

Phase 4c-3 task 2. Adds the single validation funnel that all three
attach mechanisms (📎 picker, drag-drop, paste-from-clipboard) route
through before appending to AgentInputFooter.pending_attachments.

Validates: turn limit (≤5), file size (≤20 MiB), and per-modality
capability against the active model via the 4c-1 resolver. Returns
typed AttachmentRejection variants with rejection_message() for UI
toast text.

9 new unit tests cover happy paths per modality, each rejection
reason, and boundary cases (max-size, max-count).
```

---

## Stage B — Input UI

### Task 3: 📎 toolbar button + native file picker

**Files:**
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/toolbar_item.rs` — add `AgentToolbarItemKind::AttachmentPicker` variant with display_label "📎", icon, default-left positioning.
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` — handle the `AttachmentPicker` action: invoke the native file picker (mime-filtered by the resolver — image/pdf/audio per active model caps), validate each pick through the Task 2 validator, append on Ok / toast on Err.
- Add `pending_attachments: Vec<AgentAttachment>` field to `AgentInputFooter` (Default = empty).

**Read these reference files FIRST:**
- `app/src/ai/blocklist/agent_view/agent_input_footer/toolbar_item.rs` (~300 lines) — `AgentToolbarItemKind` enum + `default_left()` / `default_right()` + `display_label`/`icon` impls. Add a new variant following the established pattern exactly.
- `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs:244-815` — `AgentInputFooter::new` + state setup.
- Grep for `rfd::FileDialog\|FileDialog\|open_file_picker\|pick_file` to find the existing file-picker abstraction (warp-ui-guidelines should point at it). If none exists app-side, use the `rfd` crate (likely already a workspace dep — verify in Cargo.toml).
- `warp-ui-guidelines` skill — read before writing any view code.

- [ ] **Step 3.1: Extend `AgentToolbarItemKind`**

Add `AttachmentPicker` variant. Mirror the existing variants' positioning — likely `default_left()` so it sits near the model selector. Match the `is_available_for_agent_view` semantics: probably available in `agent_view` but not in `cli` initially (toggle later if useful).

- [ ] **Step 3.2: Wire the click handler**

When `AttachmentPicker` is invoked:
1. Look up the active model's capability state (existing footer state owns this; reuse the same source the model selector uses).
2. Build the native file picker's mime filter list from the resolver result (e.g., if image supported but not pdf/audio → filter to `*.png *.jpg *.jpeg *.webp *.gif`).
3. Open the picker. On selection, read each file's bytes (async; show a brief loading toast if any file > 5 MB).
4. For each picked file: construct an `AgentAttachment { mime, bytes, display_name, thumbnail_bytes: None }`. Run through `AttachmentInputValidator::validate(...)`. On Ok, append to `pending_attachments` and kick off thumbnail decode (Task 4). On Err, surface `rejection_message(...)` as a toast.

- [ ] **Step 3.3: Add `pending_attachments` field**

```rust
pub struct AgentInputFooter {
    // ... existing fields ...
    pending_attachments: Vec<ai::attachments::AgentAttachment>,
}
```

Default = empty Vec. No setter needed — only the picker/drag/paste handlers and the submit path mutate it.

- [ ] **Step 3.4: Build + clippy + commit**

Unit tests for the picker mime-filter derivation can live in a sibling test file. End-to-end picker is platform code (smoke-tested manually).

Commit:
```
feat(agent_input_footer): 📎 attachment picker toolbar button

Phase 4c-3 task 3. Adds AgentToolbarItemKind::AttachmentPicker with
default-left positioning. On click: opens the native file picker
mime-filtered by the active model's 4c-1 capability resolver result;
for each pick, constructs an AgentAttachment, routes through the Task 2
validator, appends to AgentInputFooter.pending_attachments on Ok or
shows a rejection toast.

AgentInputFooter gains a pending_attachments: Vec<AgentAttachment>
field (default empty) shared with Tasks 4-8.
```

---

### Task 4: Attachment chip strip widget + thumbnail decode

**Files:**
- Create: `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_chip.rs` — chip strip widget (used by input footer AND by Task 9's transcript renderer).
- Create: `app/src/ai/blocklist/agent_view/agent_input_footer/attachment_chip_tests.rs`
- Modify: `agent_input_footer/mod.rs` — render the chip strip above the editor; spawn background thumbnail-decode tasks when attachments are appended.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/agent_view/agent_input_footer/chips.rs` (~197 lines) — existing chip widget patterns. Match the visual style.
- `warp-ui-guidelines` skill — chips, icons, theming.
- `image` crate docs (workspace dep) for decode/resize. Verify presence via `grep -n "image\\b" Cargo.toml`.

- [ ] **Step 4.1: Build the chip widget**

Per-chip render:
- Image attachment with `thumbnail_bytes: Some(...)`: render the thumbnail at 32×32 (or whatever WarpUI considers a chip-sized image).
- Image attachment with `thumbnail_bytes: None`: render the generic 🖼️ icon (decode still in flight or failed).
- PDF: 📄 icon.
- Audio: 🎙️ icon.
- All chips: display the filename next to the icon (truncate long names mid-string), and a × remove button on hover.
- Red border state: derived from validator result against current active model. If `validate(...)` would fail for this chip RIGHT NOW (model has been changed since attach), border is red; tooltip says "Active model doesn't accept <modality>; remove or switch model."

- [ ] **Step 4.2: Background thumbnail decode**

When an image is appended to `pending_attachments`, spawn a background task:
1. Decode the bytes with the `image` crate.
2. Resize to fit `THUMBNAIL_DIM × THUMBNAIL_DIM` (128 px) preserving aspect ratio.
3. Re-encode as PNG.
4. Stash the resulting bytes on the corresponding `AgentAttachment.thumbnail_bytes`.
5. Trigger a footer re-render.

For attachments that arrive without bytes (post-reload metadata-only — Task 10), `thumbnail_bytes` stays None and the chip renders the generic icon.

- [ ] **Step 4.3: Wire into footer render**

Above the editor (or wherever WarpUI's typical "chips above text input" pattern goes — confirm with `warp-ui-guidelines`). Empty `pending_attachments` → strip is hidden entirely (zero vertical real estate cost when unused).

- [ ] **Step 4.4: 5 unit tests**

1. `image_chip_with_thumbnail_renders_thumbnail` — synthetic 1x1 PNG attachment with thumbnail_bytes, asserts thumbnail element is in the render tree.
2. `image_chip_without_thumbnail_renders_generic_icon`.
3. `pdf_chip_renders_pdf_icon`.
4. `audio_chip_renders_audio_icon`.
5. `red_border_when_active_model_drops_capability` — chip rendered against model_caps where image=false → red border element present.

- [ ] **Step 4.5: Build + test + clippy + commit**

Commit:
```
feat(agent_input_footer): attachment chip strip + thumbnail decode

Phase 4c-3 task 4. Adds attachment_chip.rs — a chip strip widget
rendered above the editor (when pending_attachments is non-empty)
and reused by Task 9's transcript renderer.

Image chips show a 32px pre-decoded thumbnail; pdf/audio show icon +
filename. The thumbnail is decoded once at attach-time on a background
task and cached on AgentAttachment.thumbnail_bytes so the render path
never blocks the UI thread. Chips render with a red border when the
active model's capability resolver flips false for this modality
(reactive to model-selector changes).

5 new unit tests on render shape; thumbnail decode logic exercised
indirectly via fixtures.
```

---

### Task 5: Drag-and-drop onto input footer

**Files:**
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` — hook WarpUI's drag-drop primitives on the footer view bounds. On drop: iterate file paths, read bytes, validate, append.

**Read these reference files FIRST:**
- `crates/warpui/src/windowing/winit/event_loop/mod.rs` — existing drag-drop event primitives.
- `crates/warpui/src/windowing/winit/event_loop/drag_drop_tests.rs` — how drag-drop is exercised in tests.
- `warp-ui-guidelines` — view-level event handling patterns.

- [ ] **Step 5.1: Register a drop handler on the footer view**

When files are dropped anywhere in the `AgentInputFooter` view bounds:
1. For each dropped file path: read bytes (warn-and-skip on read errors), infer mime from extension.
2. Construct `AgentAttachment { mime, bytes, display_name: Some(filename), thumbnail_bytes: None }`.
3. Route each through `AttachmentInputValidator::validate(...)`.
4. On Ok: append + kick off thumbnail decode (Task 4 machinery).
5. On Err: emit a toast with `rejection_message(...)` — one toast per rejection, or one consolidated toast if multiple rejections of the same kind (UX nicety; ship one-per-rejection in MVP).

- [ ] **Step 5.2: Visual feedback during drag-over (optional polish)**

If WarpUI exposes drag-over events: change the footer's border color to indicate "drop target active" while files are being dragged over it. Skip if drag-over isn't easy to wire.

- [ ] **Step 5.3: 3 unit tests**

Drive the drop handler with synthetic file path lists (real disk reads via tempfile):
1. `drop_image_appends_to_pending`.
2. `drop_too_large_image_toasts_and_doesnt_append`.
3. `drop_multiple_with_one_rejection_appends_valid_only`.

- [ ] **Step 5.4: Build + test + clippy + commit**

Commit:
```
feat(agent_input_footer): drag-and-drop file attachment

Phase 4c-3 task 5. Hooks WarpUI's drag-drop primitive on the
AgentInputFooter view bounds. Dropped files are read from disk, mime
is inferred from extension, and each is routed through the Task 2
validator. Valid files append to pending_attachments and trigger
thumbnail decode; rejected files surface a toast via
rejection_message().

3 new unit tests against synthetic drop events.
```

---

### Task 6: Paste-from-clipboard image

**Files:**
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/editor.rs` (or wherever paste lands) — intercept Cmd/Ctrl+V when clipboard holds image data and route to the validator instead of pasting text.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/agent_view/agent_input_footer/editor.rs` (~391 lines) — editor paste handling.
- Grep for `clipboard\|paste\|read_clipboard\|set_clipboard` in `crates/warpui/src/` and `app/src/` for the existing clipboard abstraction.
- **Scout warning:** if no `read_image_from_clipboard()` abstraction exists, this task may grow to also build that primitive. **STOP and report** before writing a new clipboard primitive — that's a scope expansion the user should approve.

- [ ] **Step 6.1: Detect image in clipboard on paste**

When Cmd/Ctrl+V is pressed on the editor:
1. Check whether the system clipboard holds an image (PNG bytes via NSPasteboard / Windows CF_DIB / Linux selection).
2. If yes: construct an `AgentAttachment { mime: "image/png", bytes, display_name: Some("pasted-image.png"), thumbnail_bytes: None }` and route through the validator.
3. If no: fall through to the existing text-paste behavior.

- [ ] **Step 6.2: 2 unit tests**

End-to-end paste-from-real-clipboard is manual-smoke. At the unit-test layer:
1. `paste_with_image_clipboard_appends_to_pending` — inject a mock clipboard that returns image bytes; verify append.
2. `paste_with_text_clipboard_falls_through_to_editor` — inject a mock clipboard with no image; verify editor paste happens.

- [ ] **Step 6.3: Build + test + clippy + commit**

Commit:
```
feat(agent_input_footer): paste-from-clipboard image attachment

Phase 4c-3 task 6. Intercepts Cmd/Ctrl+V on the agent input editor
when the system clipboard holds image data; routes the pasted image
through the Task 2 validator instead of pasting as text. Falls through
to normal text-paste when the clipboard has no image.

2 new unit tests with injected mock clipboard cover the
image-clipboard append path and the text-clipboard fall-through.
```

---

## Stage C — Send-path wire

### Task 7: Send button enabled-state predicate

**Files:**
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` — extend the existing send-button enabled-state computation to call the resolver per pending chip; disable Send when any chip's modality is unsupported by the active model. Wire the per-chip red-border state from the same predicate.

**Read these reference files FIRST:**
- Grep `agent_input_footer/mod.rs` for `send_button\|submit_enabled\|on_send` to find the existing predicate. (The earlier scout came up empty — the predicate likely lives in the `View` impl or as an inline closure in the send-button render code.)

- [ ] **Step 7.1: Define `send_enabled_for_attachments`**

```rust
/// Returns false when any pending attachment's modality is not supported
/// by the active model. Returns true when pending_attachments is empty
/// (no attachments → no attachment-based gating). Existing non-attachment
/// reasons to disable Send (empty editor, in-flight request) still apply
/// and short-circuit before this check.
fn send_enabled_for_attachments(
    pending: &[AgentAttachment],
    caps: &ActiveModelCaps<'_>,
) -> bool {
    pending.iter().all(|att| {
        attachment_input_validator::validate(att, 0, caps).is_ok()
        // Pass current_pending_count=0 because TurnLimitReached is irrelevant
        // when checking already-attached items; we only care about modality
        // and size.
    })
}
```

Subtle: when checking already-attached items, we don't want `TurnLimitReached` to falsely fire. Pass `0` for `current_pending_count` since we're not adding, we're validating in place.

Cleaner alternative: factor out a `validate_for_dispatch(att, caps)` that skips the count check entirely. Up to the implementer — both work.

- [ ] **Step 7.2: Wire per-chip red-border state**

The chip widget (Task 4) takes a `chip_capability_state: ChipCapabilityState` enum:
- `Ok` — normal border.
- `UnsupportedByActiveModel(modality)` — red border + tooltip.

Compute this per chip in the footer render path using the same per-chip validator. Pass into the chip widget.

- [ ] **Step 7.3: Tests**

Add 3 tests in `agent_input_footer` test file (or wherever the send-button predicate is tested today):
1. `send_enabled_when_no_attachments`.
2. `send_enabled_when_all_attachments_supported`.
3. `send_disabled_when_any_attachment_unsupported_by_active_model`.

- [ ] **Step 7.4: Build + test + clippy + commit**

Commit:
```
feat(agent_input_footer): send-button gate on attachment capabilities

Phase 4c-3 task 7. Extends the send-button enabled-state predicate to
check each pending attachment's modality against the active model via
the 4c-1 capability resolver. Send is disabled when any pending
attachment is unsupported; the same predicate drives the chip's red-
border state per attachment for visual consistency.

3 new unit tests cover the no-attachments, all-supported, and any-
unsupported paths.
```

---

### Task 8: Dispatch + persistence wire

**Files:**
- Modify: `app/src/ai/blocklist/agent_view/agent_input_footer/mod.rs` — on submit, drain `pending_attachments` and pass through the controller chain to the dispatch site.
- Modify: `app/src/ai/agent/conversation.rs` and/or `app/src/ai/agent/api.rs` — extend the params chain to carry `attachments: Vec<AgentAttachment>` from the footer submit down to `impl.rs:274`.
- Modify: `app/src/ai/agent/api/impl.rs:274` — populate `LocalProviderInput.attachments` from the params chain.
- Modify: Wherever `AgentConversationData` is defined — add `#[serde(default)] attachments: Vec<AttachmentMetadata>` to the per-exchange / per-user-turn message struct. Find via `grep -rn "pub struct AgentConversationData" app/`.

**Read these reference files FIRST:**
- `app/src/ai/agent/api/impl.rs:250-285` — the dispatch site (current `LocalProviderInput` construction lacks `attachments`).
- `app/src/persistence/agent.rs:38-52` — serialization point.
- `git show fb127490` and `git show 47e13ca4` — for the established 4c-2 pattern of attachments flowing through the system.

- [ ] **Step 8.1: Thread attachments through the controller param chain**

The footer's submit path currently builds a request that lands as `params` at `impl.rs:274`. Add `attachments: Vec<AgentAttachment>` to whatever struct carries `user_query` / `tasks` / etc., and propagate from footer submit → controller → API params → dispatch site.

When in doubt: grep upward from the dispatch site at `impl.rs:274` for `user_query` — the existing `user_query` field traces the exact chain `attachments` needs to follow.

- [ ] **Step 8.2: Populate at the dispatch site**

```rust
let input = local_provider::request::LocalProviderInput {
    user_query,
    tasks,
    // … existing fields …
    action_results,
    synthetic_user_queries,
    compaction_config: params.local_provider_compaction_config.clone(),
    compaction_state: params.local_provider_compaction_state.clone(),
    attachments: params.attachments.clone(),  // Phase 4c-3
};
```

- [ ] **Step 8.3: Persist metadata-only**

When the user's turn is recorded into `AgentConversationData`, alongside the user-query text, record `Vec<AttachmentMetadata>` derived from `pending_attachments.iter().map(AttachmentMetadata::from).collect()`. Bytes are NOT serialized.

Wherever the user turn is added to the persisted conversation, look for the existing call site that takes `user_query: String` and extend it to also take `attachments: Vec<AttachmentMetadata>`.

- [ ] **Step 8.4: Clear pending_attachments on submit**

After the request kicks off, `AgentInputFooter::pending_attachments` is drained (`mem::take`) into the request. The chip strip immediately re-renders as empty.

- [ ] **Step 8.5: 4 unit tests**

1. `submit_drains_pending_attachments_into_request`.
2. `submit_clears_pending_attachments_after_dispatch`.
3. `persisted_conversation_blob_carries_metadata_only_no_bytes`.
4. `deserialize_old_conversation_blob_without_attachments_field_succeeds` — back-compat for existing rows.

- [ ] **Step 8.6: Build + test + clippy + commit**

Commit:
```
feat(agent): wire pending_attachments through dispatch + persistence

Phase 4c-3 task 8. On submit, AgentInputFooter drains its
pending_attachments into the controller param chain; the dispatch site
at app/src/ai/agent/api/impl.rs:274 populates LocalProviderInput.
attachments from there (the 4c-2 wire path then carries them upstream).

The persisted AgentConversationData JSON blob gains a #[serde(default)]
attachments: Vec<AttachmentMetadata> field per user turn — metadata only
(mime + display_name); bytes stay session-scoped per the persistence
decision. Existing rows without the field deserialize as empty Vec.

4 new unit tests cover dispatch population, post-submit clear,
metadata-only persistence, and back-compat deserialization.
```

---

## Stage D — History rendering

### Task 9: In-session transcript renders attachment chips on user turns

**Files:**
- Modify: Wherever the conversation transcript renders user turns. Find via `grep -rn "render_user\|user_query_block\|UserMessage\|render_message" app/src/ai/blocklist/agent_view/`.

**Read these reference files FIRST:**
- `app/src/ai/blocklist/agent_view/agent_view_block.rs` — the per-block renderer.
- `app/src/ai/blocklist/agent_view/mod.rs` — overall transcript structure.

- [ ] **Step 9.1: Find the user-turn render path**

Locate where the user's text bubble is built. This is the insertion point: above the text bubble (or wherever 'context chips on a message' would naturally live in the existing layout), render `attachment_chip` widgets.

- [ ] **Step 9.2: Render attachments from the in-memory exchange state**

When the user turn was just sent (in-session, bytes still in memory): render full chips with thumbnails (for images) or icons (for pdf/audio) + filenames.

This requires the per-turn in-memory exchange state to carry attachments somewhere the renderer can reach. The cleanest path: when the footer drains `pending_attachments`, it stashes them onto the in-memory `AgentExchange` (or whatever the per-turn struct is called) — that struct is already what the renderer reads from.

- [ ] **Step 9.3: 3 unit tests**

1. `user_turn_with_image_renders_thumbnail_chip`.
2. `user_turn_with_pdf_renders_pdf_icon_chip`.
3. `user_turn_with_no_attachments_renders_no_chip_strip`.

- [ ] **Step 9.4: Build + test + clippy + commit**

Commit:
```
feat(agent_view): render attachment chips on user turns

Phase 4c-3 task 9. The conversation transcript renders attachment
chips above each user turn whose exchange carries attachments —
thumbnails for images, icon + filename for pdf/audio. Reuses the
Task 4 attachment_chip widget so the input-area and transcript
rendering are visually identical.

3 new unit tests on fixture user turns.
```

---

### Task 10: Post-reload "not available" chip placeholder

**Files:**
- Modify: the transcript renderer (same files as Task 9) — when an exchange's `attachments` came from a deserialized `AgentConversationData` (no bytes), render a dimmed chip with the filename + "not available" subtitle.

- [ ] **Step 10.1: Distinguish in-session attachments from reload-only metadata**

After reload, the per-turn struct has `Vec<AttachmentMetadata>` (deserialized from the blob) but no `Vec<AgentAttachment>` (bytes were never persisted). The renderer needs to handle both:
- In-session, just sent: full chip with thumbnail (`AgentAttachment` available).
- After reload: dimmed placeholder chip from `AttachmentMetadata` only.

The simplest model: the per-turn struct carries `Option<Vec<AgentAttachment>>` (Some when bytes are live, None when metadata-only). Render branches on `Option`:
- `Some(att)`: full chip (Task 9 path).
- `None`: dimmed metadata placeholder.

- [ ] **Step 10.2: Dimmed placeholder render**

Each `AttachmentMetadata` renders as: `[modality_icon] {display_name or "attachment"} • not available` in a dimmed style. No thumbnail (no bytes to decode).

- [ ] **Step 10.3: 3 unit tests**

1. `metadata_only_chip_renders_not_available_label`.
2. `metadata_only_chip_uses_modality_icon_from_mime` — image/png → 🖼️, application/pdf → 📄, audio/wav → 🎙️.
3. `reload_path_picks_metadata_over_full_attachment_when_only_metadata_present` — verifies the Option branch.

- [ ] **Step 10.4: Build + test + clippy + commit**

Commit:
```
feat(agent_view): "not available" placeholder for reloaded attachments

Phase 4c-3 task 10. After conversation reload, attachments restored
from the persisted JSON blob have metadata only (mime + display_name)
with no bytes — by design, per the 4c-3 persistence decision.

The transcript renderer detects metadata-only attachments and renders
a dimmed chip with the modality icon + filename + "not available"
subtitle. Provides context for the assistant's prior response without
storing image bytes in SQLite.

3 new unit tests cover the placeholder shape, mime → icon mapping, and
the in-session vs. reload branch.
```

---

## Stage E — Docs

### Task 11: Spec docs + status flip

**Files:**
- Modify: `specs/multi-local-llm/README.md` — append Phase 4c-3 status paragraph, status-table row, user-visible bullet, architecture bullet.
- Modify: `specs/multi-local-llm/design.md` — flip §9 row to "4c-1 + 4c-2 + 4c-3 code complete; live smoke pending".

- [ ] **Step 11.1: Update README.md**

Status paragraph (use the final implementation commit SHA as `<TBD>` placeholder — fill in at commit time):

```markdown
**Phase 4c-3 (input UI + send-time enforcement + history rendering)** code is complete on `multi-local-llm` (final commit `<TBD>`). Third and final sub-phase for Phase 4c. Adds the 📎 file-picker toolbar button, drag-and-drop onto the input footer, paste-from-clipboard for image data, and the Send-button capability gate. Bytes are session-only; metadata (mime + display_name) persists into the existing `agent_conversations.conversation_data` JSON blob so post-reload history shows a "🖼️ filename • not available" placeholder. The per-chip red border and Send-button gate use the 4c-1 capability resolver against the active model. ~35 new unit tests across the metadata type (4), validator (9), chip widget (8 across Tasks 4 + 10), drag-drop (3), paste (2), send predicate (3), dispatch (4), transcript render (3).

> **Verification gate:** the 7-smoke manual checklist (see plan-phase-4c-3.md). Covers 📎 picker per api_type, drag-drop, paste, model-switch chip-red-border, 5-attachment limit, 20-MB size cap, and reload showing the "not available" placeholder. Once all seven smokes pass, Phase 4c (all three sub-phases) flips to ✅.
```

Status table row:
```markdown
| 4c-3 — Input UI + send-time enforcement + history rendering | [`plan-phase-4c-3.md`](plan-phase-4c-3.md) | 🧪 code complete — pending live smoke |
```

User-visible bullet:
```markdown
- **Phase 4c-3 (full multimodal UX):** 📎 button + drag-drop + Cmd/Ctrl+V paste attach images, PDFs, and audio (per active-model capabilities). Send is disabled when an attachment's modality isn't supported by the active model. After reload, prior turns show a dimmed "[filename] • not available" placeholder so the assistant's response still has context.
```

Architecture bullet:
```markdown
- **Phase 4c-3:** New `AttachmentInputValidator` shared funnel (`app/src/ai/blocklist/agent_view/agent_input_footer/attachment_input_validator.rs`) gates picker/drag/paste on modality (4c-1 resolver), file size (≤20 MiB), and turn limit (≤5). `AgentInputFooter.pending_attachments: Vec<AgentAttachment>` lives in memory; drained on submit into `LocalProviderInput.attachments` at the dispatch site (`app/src/ai/agent/api/impl.rs:274`). New `attachment_chip` widget renders thumbnails (pre-decoded at attach-time on a background task) for images and icon+filename for pdf/audio — used by both the input strip and the transcript user-turn render. Persisted conversation blob gains `#[serde(default)] attachments: Vec<AttachmentMetadata>` per user-turn — mime + display_name only; bytes stay session-scoped.
```

- [ ] **Step 11.2: Update design.md §9 row**

Change "4c-1 + 4c-2 code complete; 4c-3 pending" (or similar) to "4c-1 + 4c-2 + 4c-3 code complete; live smoke pending across all three".

- [ ] **Step 11.3: Commit**

```bash
git add specs/multi-local-llm/README.md specs/multi-local-llm/design.md
git commit -m "docs(specs/multi-local-llm): record Phase 4c-3 code-complete status"
```

---

## Final verification

- [ ] **Verification 1: Sweeps** — text-only turns produce the same wire bytes as before 4c-3 (the dispatch site only populates `attachments` when `pending_attachments` is non-empty; otherwise the 4c-2-shipping `attachments: Vec::new()` default applies). Existing tests stay green. Conversation blobs written before 4c-3 deserialize cleanly via `#[serde(default)]`.
- [ ] **Verification 2: Build + tests + clippy** — `cargo build -p ai && cargo build -p warp` clean. `cargo nextest run -p ai && cargo nextest run -p warp` shows ~30 new tests added; no regressions in existing ~1000+ tests. `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` clean.
- [ ] **Verification 3: Manual smoke** — 7/7 smokes per the README verification gate.
- [ ] **Verification 4: Final reviewer + push** — dispatch `oh-my-claudecode:code-reviewer` for the full Phase 4c-3 diff. Stop before push.

---

## Risks & open questions

1. **Paste-from-clipboard is platform-heavy.** macOS NSPasteboard, Windows Clipboard API, Linux X11/Wayland — three different code paths to read image data. **Mitigation:** Task 6's "Scout warning" tells the implementer to STOP and report if no existing `read_image_from_clipboard()` abstraction is found, so the user can decide whether to expand scope or punt B4 (paste) to a follow-up sub-phase. Drag-drop (Task 5) uses an existing WarpUI primitive; safer.
2. **Native file picker mime filter on Linux is fiddly.** macOS / Windows mime filters are robust; some Linux desktops show "all files" anyway. **Mitigation:** validate post-pick with the AttachmentInputValidator regardless of the filter result. The filter is best-effort UX, not security.
3. **Drag-drop event ownership.** The agent input footer is one view inside a larger layout; dropping onto the editor vs. chip area vs. toolbar — which sub-view claims the drop? **Mitigation:** claim drops anywhere in the footer view bounds; matches most chat apps.
4. **Synchronous image decode would block the UI thread on large images.** A 20 MB photo decoded inline at chip-render time would hitch. **Mitigation:** Task 4 decodes once at attach-time on a background task, caches `thumbnail_bytes` on the in-memory `AgentAttachment`, and renders the cached thumbnail from then on.
5. **Capability resolver lookup needs the active model id.** The footer already owns the active model (via the model selector); resolver lookup is a function call + cache hit. The chip's red-border state has to update reactively when the user changes the model — this is the existing WarpUI reactive-render pattern; just make sure the chip widget re-renders on model-selector state changes.
6. **Conversation blob JSON shape evolution.** Adding `attachments: Vec<AttachmentMetadata>` with `#[serde(default)]` is forward+backward compatible — older code reads new rows without seeing the field (lossy but non-fatal), newer code reads older rows as empty Vec. No migration needed.
7. **Drained-attachment ownership.** `pending_attachments` is drained into the controller param chain on submit. If the dispatch fails (e.g., model returns a 5xx), the attachments are gone from the input area — the user would have to re-attach to retry. **Mitigation:** acceptable for first ship; if user feedback flags this, a future improvement is to stash a copy and restore on dispatch failure.

---

## Next plan

Phase 4c-3 is the final sub-phase of Phase 4c. There is no Phase 4c-4 planned. After live smoke flips all three sub-phases to ✅, Phase 4c is complete and the next macro-phase (TBD per `specs/multi-local-llm/design.md`) takes over.
