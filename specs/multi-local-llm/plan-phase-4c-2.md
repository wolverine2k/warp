# Phase 4c-2 — Data model + per-adapter wire shapes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Second sub-phase of Phase 4c (multimodal attachments end-to-end). Adds an `AgentAttachment` runtime type and threads it through `LocalProviderInput`; updates each of the five active adapters' request translators to carry attachments in the upstream's wire shape. **No input-bar UI yet** — 4c-3 builds the file picker, send-time enforcement, and history rendering. 4c-2 ships the wire plumbing so a programmatic / test path can build a turn with an `AgentAttachment` and confirm each adapter's request body matches that adapter's documented multimodal shape.

**Architecture:** A new `crates/ai/src/attachments.rs` exposes `AgentAttachment { mime: String, bytes: Vec<u8>, display_name: Option<String> }`. `LocalProviderInput` gains an `attachments: Vec<AgentAttachment>` field (defaults to empty — back-compat for every existing call site). Each adapter's `build_chat_request` is updated to emit the per-adapter wire shape **only when `attachments.is_empty()` is false**, so text-only turns produce the same bytes as before this phase.

**Tech Stack:** Rust 2021, `serde` + `serde_json`, `base64` for encoding image/pdf/audio bytes into wire-format strings.

---

## Per-adapter wire shapes (reference)

| Adapter | Shape (target wire) | Trade-off |
|---|---|---|
| **OpenAi** | `content` is either a plain `String` (text-only turn) or a `Vec<{type: "text"|"image_url", ...}>` (turn with attachments). Image is `{"type":"image_url","image_url":{"url":"data:image/png;base64,…"}}`. | OpenAi accepts both shapes per its API docs. Untagged serde enum keeps back-compat for text-only. |
| **Anthropic** | `content: Vec<AnthropicContentBlock>` already in place. Add variants: `{"type":"image","source":{"type":"base64","media_type":"image/png","data":"…"}}` and `{"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"…"}}`. Audio is not natively supported — translator drops audio attachments with a `log::warn!`. | Anthropic's wire shape is already array-of-blocks; this is an additive change to an existing enum. |
| **Ollama** | User message gains `images: Vec<String>` (base64-encoded **without** data-URI prefix). Image-only — Ollama's native chat shape has no pdf/document field. PDF and audio attachments are dropped at the translator with a `log::warn!`. | Smallest change; Ollama spec is image-only. |
| **Gemini** | `parts` becomes a heterogeneous `Vec<GeminiOutboundPart>` — text + function-call parts already exist; this phase adds an `InlineData` variant: `{"inline_data":{"mime_type":"image/png","data":"<base64>"}}`. | Gemini supports the most modalities natively (image, pdf, audio); InlineData carries any of them. |
| **DeepSeek** | Identical to OpenAi — content can be string or content-parts array. DeepSeek's API matches OpenAi's wire byte-for-byte for the request body. Reuses the wire types added in the OpenAi task. | No new wire types in the DeepSeek module; the translator update is the work. |

Across all adapters: the translator change is **gated on `!input.attachments.is_empty()`**. Text-only turns produce the same bytes as before, so the integration tests this phase doesn't add aren't regressed.

---

## File map

**Created:**
- `crates/ai/src/attachments.rs` — `AgentAttachment` + a small `encode_base64` helper used by the per-adapter translators.
- `crates/ai/src/attachments_tests.rs` — tests on the type itself (Debug/Clone derives, base64 helper).

**Modified:**
- `crates/ai/src/lib.rs` — `pub mod attachments;`
- `crates/ai/src/local_provider/request.rs` — add `pub attachments: Vec<AgentAttachment>` field on `LocalProviderInput`, default to empty.
- `crates/ai/src/local_provider/wire.rs` — `ChatMessage.content` becomes an untagged enum supporting Text + Parts.
- `crates/ai/src/local_provider/adapters/openai.rs` (or wherever the OpenAi request builder lives) — emit content array when attachments non-empty.
- `crates/ai/src/local_provider/adapters/anthropic/wire.rs` + `request.rs` — add Image + Document variants on `AnthropicContentBlock`; translator emits them.
- `crates/ai/src/local_provider/adapters/ollama/wire.rs` + `request.rs` — add `images: Vec<String>` on `OllamaChatMessage`; translator populates from `input.attachments`.
- `crates/ai/src/local_provider/adapters/gemini/wire.rs` + `request.rs` — add `InlineData` variant on `GeminiOutboundPart`; translator emits it.
- `crates/ai/src/local_provider/adapters/deepseek/request.rs` — uses OpenAi's types; translator emits content array when attachments non-empty.
- `crates/ai/Cargo.toml` — confirm `base64` is in deps; if not, add `base64.workspace = true`.
- `specs/multi-local-llm/README.md` + `design.md` — Task 8 status flip.

---

## Stage A — Data model

### Task 1: `AgentAttachment` + `LocalProviderInput` threading

**Files:**
- Create: `crates/ai/src/attachments.rs`
- Create: `crates/ai/src/attachments_tests.rs`
- Modify: `crates/ai/src/lib.rs` — `pub mod attachments;`
- Modify: `crates/ai/src/local_provider/request.rs` — add `attachments: Vec<AgentAttachment>` field.
- Modify: `crates/ai/Cargo.toml` — add `base64.workspace = true` if not already present.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/request.rs` — current shape of `LocalProviderInput`. Add the new field; verify `Default` derive still works (the field's `Default` is empty `Vec`).
- `crates/ai/src/catalog/parse.rs` (for the `#[serde(default)]` pattern this codebase uses — applies to wire-type changes in Tasks 2-6 but not to the runtime `AgentAttachment` type which isn't serialized).

- [ ] **Step 1.1: Create `attachments.rs`**

```rust
//! Phase 4c-2: runtime attachment type carried through `LocalProviderInput`.
//!
//! `AgentAttachment` is the in-memory shape a turn's attached file takes
//! while dispatch is composing the upstream request. Each adapter's
//! translator reads `LocalProviderInput.attachments` and emits the
//! upstream's per-modality wire shape (see plan-phase-4c-2.md preamble).
//!
//! The persistence question (does an attachment survive a conversation
//! reload?) is deferred to Phase 4c-3 — `AgentAttachment` is session-
//! scoped today and is NOT serialized into the conversation history DB.

use base64::Engine;

/// One attached file. `bytes` holds the raw file content; adapters
/// base64-encode it when their wire shape requires (OpenAi's data-URI,
/// Anthropic's `source.data`, Gemini's `inline_data.data`, Ollama's
/// `images` array element). The `mime` string is the canonical IANA
/// media type — e.g., `"image/png"`, `"application/pdf"`, `"audio/wav"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
    /// User-visible name for UI history rendering (4c-3). `None` is
    /// acceptable — the file picker may generate one if missing.
    pub display_name: Option<String>,
}

impl AgentAttachment {
    /// Returns true when the mime type is `image/*`. Used by the
    /// per-adapter translators to decide whether to emit an image
    /// content block.
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// Returns true when the mime type is `application/pdf`.
    pub fn is_pdf(&self) -> bool {
        self.mime == "application/pdf"
    }

    /// Returns true when the mime type is `audio/*`.
    pub fn is_audio(&self) -> bool {
        self.mime.starts_with("audio/")
    }
}

/// Base64-encode the attachment bytes for adapters whose wire shape
/// takes a raw base64 string (Anthropic `source.data`, Gemini
/// `inline_data.data`, Ollama `images[i]`).
pub fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Build an OpenAi-style data URI for an attachment: `data:{mime};base64,{b64}`.
pub fn encode_data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", encode_base64(bytes))
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
```

- [ ] **Step 1.2: Create `attachments_tests.rs`**

```rust
use super::{encode_base64, encode_data_uri, AgentAttachment};

fn png_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![0x89, 0x50, 0x4e, 0x47],
        display_name: Some("test.png".into()),
    }
}

#[test]
fn is_image_recognizes_image_mimes() {
    assert!(png_attachment().is_image());
    assert!(AgentAttachment {
        mime: "image/jpeg".into(),
        ..png_attachment()
    }
    .is_image());
    assert!(!AgentAttachment {
        mime: "application/pdf".into(),
        ..png_attachment()
    }
    .is_image());
}

#[test]
fn is_pdf_matches_exact_mime() {
    assert!(AgentAttachment {
        mime: "application/pdf".into(),
        ..png_attachment()
    }
    .is_pdf());
    assert!(!png_attachment().is_pdf());
}

#[test]
fn is_audio_recognizes_audio_mimes() {
    assert!(AgentAttachment {
        mime: "audio/wav".into(),
        ..png_attachment()
    }
    .is_audio());
    assert!(AgentAttachment {
        mime: "audio/mpeg".into(),
        ..png_attachment()
    }
    .is_audio());
    assert!(!png_attachment().is_audio());
}

#[test]
fn encode_base64_round_trip() {
    let encoded = encode_base64(&[0x89, 0x50, 0x4e, 0x47]);
    assert_eq!(encoded, "iVBORw==");
}

#[test]
fn encode_data_uri_format() {
    let uri = encode_data_uri("image/png", &[0x89, 0x50, 0x4e, 0x47]);
    assert_eq!(uri, "data:image/png;base64,iVBORw==");
}
```

- [ ] **Step 1.3: Thread `attachments` onto `LocalProviderInput`**

In `crates/ai/src/local_provider/request.rs`, find the existing `pub struct LocalProviderInput { … }` (currently has `user_query`, `tasks`, `supported_tools`, `conversation_id`, `task_id`, `needs_create_task`, `action_results`, and synthetic injection fields).

Add immediately after `action_results`:

```rust
    /// Phase 4c-2. Attachments carried alongside the user query. Empty
    /// `Vec` is the default — every existing call site builds one without
    /// touching this field. Each adapter's request translator reads
    /// `attachments` and emits the upstream's per-modality wire shape;
    /// when empty, the translator emits the same text-only request body
    /// as before Phase 4c-2 (back-compat).
    pub attachments: Vec<crate::attachments::AgentAttachment>,
```

The struct already derives `Default`, so the new field's `Default::default()` is `Vec::new()` — no existing call site needs changes.

- [ ] **Step 1.4: Wire into `crates/ai/src/lib.rs`**

Add `pub mod attachments;` at the alphabetically-correct position (between `api_keys` and `aws_credentials`, or wherever the existing ordering puts it).

- [ ] **Step 1.5: Confirm `base64` dependency**

```bash
cd /Users/nmehta/Documents/code/github/warp
grep -n "^base64" crates/ai/Cargo.toml Cargo.toml | head -5
```

If `base64` isn't in `crates/ai/Cargo.toml`'s `[dependencies]`, add `base64.workspace = true`. (It's already used elsewhere in the repo for secrets/keys — should be a workspace dep already.)

- [ ] **Step 1.6: Build + test + clippy**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai attachments 2>&1 | tail -10   # 5/5 passed
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 1.7: Commit**

```bash
git add crates/ai/src/attachments.rs crates/ai/src/attachments_tests.rs \
        crates/ai/src/lib.rs crates/ai/src/local_provider/request.rs \
        crates/ai/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(ai/attachments): runtime AgentAttachment type + LocalProviderInput field

Phase 4c-2 task 1. Adds AgentAttachment { mime, bytes, display_name }
in the new crates/ai/src/attachments.rs module, plus is_image / is_pdf
/ is_audio helpers and encode_base64 / encode_data_uri encoders used
by the per-adapter wire translators in Tasks 2-6.

LocalProviderInput gains `attachments: Vec<AgentAttachment>` (default
empty Vec, so every existing call site keeps working unchanged). The
field is read by each adapter's translator in subsequent tasks; when
empty, translators emit the same text-only request body as before
4c-2.

5 unit tests cover the modality helpers and the two encoders.
EOF
)"
```

---

## Stage B — Per-adapter wire shapes

Each Stage-B task follows the same shape: locate the adapter's wire types, add the new variant(s) needed for attachments, update the translator's user-message construction to emit them when `input.attachments` is non-empty, add 3-4 unit tests with fixture request bodies.

### Task 2: OpenAi attachments

**Files:**
- Modify: `crates/ai/src/local_provider/wire.rs` — `ChatMessage.content` becomes an untagged enum.
- Modify: `crates/ai/src/local_provider/request.rs` (or `crates/ai/src/local_provider/adapters/openai.rs` — wherever the OpenAi request translator lives) — emit content array when attachments non-empty.
- Modify: existing `*_tests.rs` for the OpenAi translator — add 4 fixture-based tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/wire.rs` lines 40-65 — current `ChatMessage` struct.
- The OpenAi request translator (likely in `crates/ai/src/local_provider/request.rs` since OpenAi is the default wire format).
- The corresponding tests file for the existing OpenAi translator.

- [ ] **Step 2.1: Change `ChatMessage.content` to an untagged enum**

In `crates/ai/src/local_provider/wire.rs`, replace:

```rust
pub struct ChatMessage {
    pub role: Role,
    pub content: Option<String>,
    // … other fields
}
```

…with:

```rust
pub struct ChatMessage {
    pub role: Role,
    pub content: Option<ChatMessageContent>,
    // … other fields
}

/// Phase 4c-2. OpenAi accepts either a plain string `content` (text-only
/// turn) or an array of typed parts (turn with attachments). Untagged
/// serde keeps the wire shape identical to before 4c-2 for text-only
/// turns — `Text(String)` serializes as a JSON string, not as a
/// tagged object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ChatMessageContent {
    Text(String),
    Parts(Vec<ChatContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrlSpec },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageUrlSpec {
    /// `data:image/png;base64,<base64-payload>` for inline images.
    pub url: String,
}
```

All existing call sites that built `content: Some("...".to_string())` need updating to `content: Some(ChatMessageContent::Text("...".to_string()))`. A helper on `ChatMessage` makes this less verbose:

```rust
impl ChatMessage {
    /// Convenience for text-only messages.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(ChatMessageContent::Text(text.into())),
            // … fill in other field defaults
        }
    }
}
```

Update every existing constructor in the translator to use `ChatMessage::text(...)`. Run `cargo build -p ai` after this step to surface every call site that needs the change.

- [ ] **Step 2.2: Update the translator to emit the array when attachments are present**

In the OpenAi user-message construction (find by grepping for `Role::User` or `role: Role::User` in the request module), branch on `input.attachments.is_empty()`:

```rust
let user_content = if input.attachments.is_empty() {
    ChatMessageContent::Text(input.user_query.clone().unwrap_or_default())
} else {
    let mut parts: Vec<ChatContentPart> = Vec::new();
    if let Some(text) = input.user_query.as_ref() {
        if !text.is_empty() {
            parts.push(ChatContentPart::Text { text: text.clone() });
        }
    }
    for attachment in &input.attachments {
        if attachment.is_image() {
            parts.push(ChatContentPart::ImageUrl {
                image_url: ImageUrlSpec {
                    url: crate::attachments::encode_data_uri(
                        &attachment.mime,
                        &attachment.bytes,
                    ),
                },
            });
        } else {
            log::warn!(
                "OpenAi adapter: dropping unsupported attachment mime {} \
                 (only image/* is supported on this api_type)",
                attachment.mime
            );
        }
    }
    ChatMessageContent::Parts(parts)
};
// ... use user_content when building the user ChatMessage.
```

- [ ] **Step 2.3: Add 4 fixture-based unit tests**

In the existing OpenAi translator's test file (find it by grepping for `#[test]` near the translator), add:

```rust
use crate::attachments::AgentAttachment;
use super::wire::{ChatContentPart, ChatMessageContent};

fn png_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![0x89, 0x50, 0x4e, 0x47],
        display_name: Some("test.png".into()),
    }
}

#[test]
fn text_only_turn_emits_string_content() {
    let input = LocalProviderInput {
        user_query: Some("hello".into()),
        attachments: Vec::new(),
        ..Default::default()
    };
    let req = build_request(&input, &test_cfg()).unwrap();
    let user_msg = req.messages.iter().find(|m| m.role == Role::User).unwrap();
    assert!(matches!(
        &user_msg.content,
        Some(ChatMessageContent::Text(t)) if t == "hello"
    ));
}

#[test]
fn turn_with_image_emits_parts_array() {
    let input = LocalProviderInput {
        user_query: Some("what is this".into()),
        attachments: vec![png_attachment()],
        ..Default::default()
    };
    let req = build_request(&input, &test_cfg()).unwrap();
    let user_msg = req.messages.iter().find(|m| m.role == Role::User).unwrap();
    let parts = match &user_msg.content {
        Some(ChatMessageContent::Parts(p)) => p,
        _ => panic!("expected Parts"),
    };
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[0], ChatContentPart::Text { text } if text == "what is this"));
    assert!(matches!(
        &parts[1],
        ChatContentPart::ImageUrl { image_url } if image_url.url.starts_with("data:image/png;base64,")
    ));
}

#[test]
fn pdf_attachment_is_dropped_with_warning() {
    let input = LocalProviderInput {
        user_query: Some("read this".into()),
        attachments: vec![AgentAttachment {
            mime: "application/pdf".into(),
            bytes: vec![1, 2, 3],
            display_name: None,
        }],
        ..Default::default()
    };
    let req = build_request(&input, &test_cfg()).unwrap();
    let user_msg = req.messages.iter().find(|m| m.role == Role::User).unwrap();
    let parts = match &user_msg.content {
        Some(ChatMessageContent::Parts(p)) => p,
        _ => panic!("expected Parts"),
    };
    // PDF is dropped; only the text part remains.
    assert_eq!(parts.len(), 1);
}

#[test]
fn empty_user_query_with_image_still_emits_array() {
    let input = LocalProviderInput {
        user_query: Some("".into()),
        attachments: vec![png_attachment()],
        ..Default::default()
    };
    let req = build_request(&input, &test_cfg()).unwrap();
    let user_msg = req.messages.iter().find(|m| m.role == Role::User).unwrap();
    let parts = match &user_msg.content {
        Some(ChatMessageContent::Parts(p)) => p,
        _ => panic!("expected Parts"),
    };
    // Empty text is filtered out; only the image remains.
    assert_eq!(parts.len(), 1);
    assert!(matches!(&parts[0], ChatContentPart::ImageUrl { .. }));
}
```

(Adjust `build_request` / `test_cfg` / etc. to whatever the existing OpenAi translator test harness uses.)

- [ ] **Step 2.4: Build + test + clippy + commit**

```bash
cargo build -p ai 2>&1 | tail -5
cargo nextest run -p ai openai 2>&1 | tail -10
cargo clippy -p ai --lib --tests -- -D warnings 2>&1 | tail -5
git add crates/ai/src/local_provider/wire.rs crates/ai/src/local_provider/request.rs <other-touched-test-files>
git commit -m "feat(ai/local_provider/adapters/openai): wire attachments

Phase 4c-2 task 2. ChatMessage.content becomes an untagged
ChatMessageContent enum supporting Text(String) (back-compat for
text-only turns — serializes as a plain JSON string) and
Parts(Vec<ChatContentPart>) for turns carrying attachments. New
ChatContentPart enum has Text + ImageUrl variants; ImageUrlSpec
carries the data:image/png;base64,... URL.

Translator branches on input.attachments.is_empty(): empty produces
the same wire bytes as before 4c-2; non-empty produces a content
array. Non-image attachments (pdf, audio) are dropped at the
translator with a log::warn! — OpenAi's content-array shape is
image-only.

4 new unit tests cover text-only round-trip, image-attachment array,
pdf-dropped-with-warning, and empty-text-with-image."
```

---

### Task 3: Anthropic attachments

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/anthropic/wire.rs` — add `Image` + `Document` variants to `AnthropicContentBlock`.
- Modify: `crates/ai/src/local_provider/adapters/anthropic/request.rs` — translator emits the new variants.
- Modify: existing anthropic test file(s) — add 4 fixture-based tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/anthropic/wire.rs` lines 45-130 — current `AnthropicMessage` + `AnthropicContentBlock`. `content: Vec<AnthropicContentBlock>` is already the shape; this task adds enum variants.
- `crates/ai/src/local_provider/adapters/anthropic/request.rs` — translator. Find the user-message construction.

- [ ] **Step 3.1: Add Image + Document variants to `AnthropicContentBlock`**

```rust
// Within AnthropicContentBlock — add alongside existing Text / ToolUse / ToolResult / etc.:

#[serde(rename = "image")]
Image {
    source: AnthropicMediaSource,
},

#[serde(rename = "document")]
Document {
    source: AnthropicMediaSource,
},
```

And add the shared source type:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicMediaSource {
    #[serde(rename = "type")]
    pub source_type: AnthropicSourceType,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicSourceType {
    Base64,
}
```

- [ ] **Step 3.2: Update the Anthropic translator**

In the user-message construction in `request.rs`, after building the existing text content blocks, append per-attachment blocks:

```rust
for attachment in &input.attachments {
    if attachment.is_image() {
        blocks.push(AnthropicContentBlock::Image {
            source: AnthropicMediaSource {
                source_type: AnthropicSourceType::Base64,
                media_type: attachment.mime.clone(),
                data: crate::attachments::encode_base64(&attachment.bytes),
            },
        });
    } else if attachment.is_pdf() {
        blocks.push(AnthropicContentBlock::Document {
            source: AnthropicMediaSource {
                source_type: AnthropicSourceType::Base64,
                media_type: "application/pdf".into(),
                data: crate::attachments::encode_base64(&attachment.bytes),
            },
        });
    } else {
        log::warn!(
            "Anthropic adapter: dropping unsupported attachment mime {} \
             (only image/* and application/pdf are supported on this api_type)",
            attachment.mime
        );
    }
}
```

- [ ] **Step 3.3: Add 4 fixture-based unit tests**

In the existing Anthropic translator test file:

```rust
use crate::attachments::AgentAttachment;

fn png() -> AgentAttachment { /* same as Task 2 */ }
fn pdf_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "application/pdf".into(),
        bytes: vec![0x25, 0x50, 0x44, 0x46],
        display_name: Some("test.pdf".into()),
    }
}

#[test]
fn text_only_turn_emits_just_text_block() { /* … */ }

#[test]
fn image_attachment_appends_image_block() {
    let input = LocalProviderInput { attachments: vec![png()], ..Default::default() };
    let req = build_request(&input, &test_cfg()).unwrap();
    let user_msg = req.messages.iter().find(|m| m.role == AnthropicRole::User).unwrap();
    assert!(user_msg.content.iter().any(|b| matches!(
        b,
        AnthropicContentBlock::Image { source } if source.media_type == "image/png"
    )));
}

#[test]
fn pdf_attachment_emits_document_block() { /* … similar … */ }

#[test]
fn audio_attachment_dropped_with_warning() { /* … similar … */ }
```

- [ ] **Step 3.4: Build + test + clippy + commit**

Same pattern as Task 2. Commit message:

```
feat(ai/local_provider/adapters/anthropic): wire attachments

Phase 4c-2 task 3. Adds Image + Document variants to
AnthropicContentBlock plus the shared AnthropicMediaSource type
(`{type: "base64", media_type, data}`). Translator emits an Image
block for image/* mime types and a Document block for
application/pdf; audio/* is dropped with a log::warn! (Anthropic's
API doesn't natively accept audio at this writing).

4 new unit tests cover text-only round-trip, image block emission,
document block emission, and audio drop.
```

---

### Task 4: Ollama attachments

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/ollama/wire.rs` — add `images: Vec<String>` to `OllamaChatMessage`.
- Modify: `crates/ai/src/local_provider/adapters/ollama/request.rs` — translator populates it.
- Modify: existing ollama test files — add 3 fixture-based tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/ollama/wire.rs` lines 30-50 — current `OllamaChatMessage`.

- [ ] **Step 4.1: Add `images` to `OllamaChatMessage`**

```rust
// Find:
pub struct OllamaChatMessage {
    pub role: OllamaRole,
    pub content: String,
    // … other fields
}

// Add:
    /// Phase 4c-2. Base64-encoded image attachments (no data-URI prefix —
    /// raw base64 only). Empty Vec is the default; serialized only when
    /// non-empty so text-only turns produce the same wire bytes as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
```

- [ ] **Step 4.2: Update the Ollama translator**

In the user-message construction:

```rust
let mut images: Vec<String> = Vec::new();
for attachment in &input.attachments {
    if attachment.is_image() {
        images.push(crate::attachments::encode_base64(&attachment.bytes));
    } else {
        log::warn!(
            "Ollama adapter: dropping unsupported attachment mime {} \
             (Ollama's chat API natively accepts images only)",
            attachment.mime
        );
    }
}
// … construct user message with `images` field …
```

- [ ] **Step 4.3: Add 3 fixture-based unit tests**

`text_only_turn_omits_images_field` (verify `images` is omitted from the JSON via `skip_serializing_if`), `image_attachment_appends_base64`, `pdf_attachment_dropped_with_warning`.

- [ ] **Step 4.4: Build + test + clippy + commit**

```
feat(ai/local_provider/adapters/ollama): wire attachments

Phase 4c-2 task 4. OllamaChatMessage gains an `images: Vec<String>`
field with #[serde(default, skip_serializing_if = "Vec::is_empty")]
so text-only turns produce the same wire bytes as before. Translator
base64-encodes each image attachment without the data-URI prefix
(Ollama's native shape is raw base64). PDF and audio attachments
are dropped with a log::warn! — Ollama's chat API is image-only.

3 new unit tests cover the no-images path, the with-images path,
and the pdf-drop case.
```

---

### Task 5: Gemini attachments

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/gemini/wire.rs` — add `InlineData` variant to `GeminiOutboundPart`.
- Modify: `crates/ai/src/local_provider/adapters/gemini/request.rs` — translator emits it.
- Modify: existing gemini test files — add 3 fixture-based tests.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/gemini/wire.rs` lines 70-110 — current `GeminiOutboundPart` enum.

- [ ] **Step 5.1: Add `InlineData` variant**

```rust
// In GeminiOutboundPart, alongside existing Text / FunctionCall / FunctionResponse:

#[serde(rename = "inline_data")]
InlineData { inline_data: GeminiInlineData },

// New supporting type:
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeminiInlineData {
    pub mime_type: String,
    pub data: String,  // raw base64, no data-URI prefix
}
```

(Match the existing enum's serde tagging convention. Gemini's wire uses snake_case keys.)

- [ ] **Step 5.2: Update the Gemini translator**

In the user-content construction:

```rust
for attachment in &input.attachments {
    if attachment.is_image() || attachment.is_pdf() || attachment.is_audio() {
        parts.push(GeminiOutboundPart::InlineData {
            inline_data: GeminiInlineData {
                mime_type: attachment.mime.clone(),
                data: crate::attachments::encode_base64(&attachment.bytes),
            },
        });
    } else {
        log::warn!(
            "Gemini adapter: dropping attachment with unrecognized mime {}",
            attachment.mime
        );
    }
}
```

- [ ] **Step 5.3: Add 3 fixture-based unit tests**

`text_only_turn_emits_just_text_part`, `image_attachment_appends_inline_data_part`, `audio_attachment_emits_inline_data_with_audio_mime`.

- [ ] **Step 5.4: Build + test + clippy + commit**

```
feat(ai/local_provider/adapters/gemini): wire attachments

Phase 4c-2 task 5. GeminiOutboundPart gains an InlineData variant
{inline_data:{mime_type,data}} that carries any of image/* /
application/pdf / audio/* — Gemini's native shape supports all three
modalities via the same content-part. Translator base64-encodes the
bytes without the data-URI prefix (matches the spec). Unknown mimes
are dropped with a log::warn!.

3 new unit tests cover text-only round-trip, image part emission,
and audio part emission.
```

---

### Task 6: DeepSeek attachments

**Files:**
- Modify: `crates/ai/src/local_provider/adapters/deepseek/request.rs` — translator emits OpenAi-shape content array.
- Modify: existing deepseek test files — add 2 tests confirming the wire shape matches OpenAi.

**Read these reference files FIRST:**
- `crates/ai/src/local_provider/adapters/deepseek/{request,wire}.rs` — current translator. Phase 3d's commit log notes the adapter reuses OpenAi types; verify whether it reuses the wire types directly or has its own thin re-exports.

- [ ] **Step 6.1: Update the DeepSeek translator**

Same as OpenAi (Task 2 Step 2.2). DeepSeek's API accepts the content-array shape byte-for-byte. If the DeepSeek translator currently constructs `ChatMessage { content: Some("...".to_string()), .. }`, update it to use the new untagged-enum constructor `ChatMessageContent::Text(...)` for text-only turns and `Parts(vec![…])` for attachment turns. The branching logic is identical to OpenAi's.

If the DeepSeek module has its own copy of `ChatMessage` (rather than reusing `crate::local_provider::wire::ChatMessage`), the same enum + part types need to be added there.

- [ ] **Step 6.2: Add 2 unit tests**

```rust
#[test]
fn deepseek_text_only_matches_openai_shape() { /* same fixture body as OpenAi text-only */ }

#[test]
fn deepseek_image_attachment_emits_content_array() { /* same shape as OpenAi */ }
```

- [ ] **Step 6.3: Build + test + clippy + commit**

```
feat(ai/local_provider/adapters/deepseek): wire attachments

Phase 4c-2 task 6. DeepSeek's wire shape is OpenAi-compatible
(confirmed in Phase 3d) so the attachment translation reuses the
OpenAi ChatMessageContent / ChatContentPart enums. Translator
branches on input.attachments.is_empty(): empty produces
text-string content; non-empty produces a content-array. Same
image-only constraint as OpenAi — pdf and audio are dropped with
a log::warn!.

2 new unit tests confirm the wire shape matches OpenAi byte-for-byte.
```

---

## Stage C — Cross-adapter integration smoke

### Task 7: Cross-adapter integration test

**Files:**
- Modify: `crates/ai/tests/local_provider_integration.rs` (or create a new `attachment_integration.rs` if the existing file is too crowded).

This task adds **one integration test per active api_type** that builds a `LocalProviderInput` with one `AgentAttachment` and asserts the resulting upstream request body matches the expected per-adapter wire shape. Catches cross-cutting issues that per-adapter unit tests miss (e.g., a translator that builds the right user message but corrupts the system-prompt path).

**Read these reference files FIRST:**
- `crates/ai/tests/local_provider_integration.rs` — existing integration test pattern, especially the mock-HTTP-server setup if any.

- [ ] **Step 7.1: Add 5 integration tests (one per adapter)**

For each of OpenAi / Anthropic / Ollama / Gemini / DeepSeek:

```rust
#[tokio::test]
async fn openai_attachment_turn_emits_content_array_in_outbound_body() {
    // Build a LocalProviderInput with one image attachment.
    // Drive the OpenAi adapter's build_chat_request.
    // Assert the serialized JSON body contains a `content` array with
    // an `image_url` part whose URL starts with "data:image/png;base64,".
}
```

Similar shape for the other four adapters with their adapter-specific assertions.

- [ ] **Step 7.2: Build + test + clippy + commit**

```bash
cargo nextest run -p ai attachment_integration 2>&1 | tail -10   # 5/5 passed
git add crates/ai/tests/local_provider_integration.rs
git commit -m "test(ai/local_provider): cross-adapter attachment smoke

Phase 4c-2 task 7. Adds 5 integration tests (one per active api_type)
that build a LocalProviderInput with a single image attachment and
assert the resulting upstream request body matches each adapter's
documented multimodal wire shape:

- OpenAi: content-array with image_url part (data: URI)
- Anthropic: AnthropicContentBlock::Image with base64 source
- Ollama: images: Vec<String> on the user message
- Gemini: GeminiOutboundPart::InlineData
- DeepSeek: same shape as OpenAi

Catches cross-cutting issues that per-adapter unit tests miss
(e.g., system-prompt corruption when the user-message construction
path forks).
"
```

---

## Stage D — Docs status flip

### Task 8: Spec docs + status flip

**Files:**
- Modify: `specs/multi-local-llm/README.md` — append Phase 4c-2 status paragraph + status-table row + bullets.
- Modify: `specs/multi-local-llm/design.md` — flag the §9 row to note 4c-2 is now code-complete (4c-1 + 4c-2 done; 4c-3 pending).

- [ ] **Step 8.1: Update README.md**

Append a status paragraph after the Phase 4c-1 paragraph:

```markdown
**Phase 4c-2 (data model + per-adapter wire shapes)** code is complete on `multi-local-llm` (final commit `<TBD>`). Second of three sub-phases for Phase 4c. Adds `AgentAttachment { mime, bytes, display_name }` in `crates/ai/src/attachments.rs` and threads `attachments: Vec<AgentAttachment>` onto `LocalProviderInput`. Each of the five active adapters' request translators is updated to emit the per-modality wire shape: OpenAi/DeepSeek content-array with `image_url` parts, Anthropic Image + Document content blocks, Ollama `images: Vec<base64>` field, Gemini `InlineData` parts. Translators gate on `attachments.is_empty()` so text-only turns produce the same wire bytes as before 4c-2 (back-compat). **~17 new unit tests** (5 on `AgentAttachment` + 4 per OpenAi/Anthropic + 3 per Ollama/Gemini + 2 DeepSeek) plus 5 integration tests covering the cross-adapter outbound shape.

> **Verification gate:** live-test smoke against each of the five upstreams with a real attachment — image to OpenAi/Anthropic/Ollama/Gemini, pdf to Anthropic/Gemini, audio to Gemini. Confirm the upstream accepts the request (no 400 due to malformed content shape). Once all six smokes pass, the 4c-2 row in the status table flips to ✅. 4c-3 (input-bar UI + send-time enforcement + history rendering) is the third and final sub-phase.
```

Status table row:

```markdown
| 4c-2 — AgentAttachment data model + per-adapter wire shapes | [`plan-phase-4c-2.md`](plan-phase-4c-2.md) | 🧪 code complete — pending live smoke |
```

User-visible bullet (note: no UI yet):

```markdown
- **Phase 4c-2 (programmatic only, no UI yet):** the BYOP wire path now carries attachments. Programmatic / test callers can populate `LocalProviderInput.attachments` and each adapter emits the upstream's multimodal shape. User-facing file picker + send-time enforcement land in 4c-3.
```

Architecture bullet:

```markdown
- **Phase 4c-2:** New `crates/ai/src/attachments.rs` with `AgentAttachment { mime, bytes, display_name }` + `encode_base64` / `encode_data_uri` helpers. `LocalProviderInput` gains `attachments: Vec<AgentAttachment>` (default empty). Per-adapter wire-type extensions: OpenAi/DeepSeek `ChatMessageContent` becomes an untagged enum supporting Text(String) | Parts(Vec<ChatContentPart>); Anthropic `AnthropicContentBlock` gains Image + Document variants with a shared `AnthropicMediaSource`; Ollama `OllamaChatMessage` gains `images: Vec<String>` (raw base64); Gemini `GeminiOutboundPart` gains an `InlineData` variant. Each translator gates the new shape on `!attachments.is_empty()` so text-only turns are bit-for-bit unchanged.
```

- [ ] **Step 8.2: Update design.md §9 row**

Append "4c-1 + 4c-2 code complete; 4c-3 pending" to the existing 4c row's status flag.

- [ ] **Step 8.3: Commit**

```bash
git add specs/multi-local-llm/README.md specs/multi-local-llm/design.md
git commit -m "docs(specs/multi-local-llm): record Phase 4c-2 code-complete status"
```

---

## Final verification

- [ ] **Verification 1: Sweeps** — `crates/ai/src/attachments.rs` is self-contained; `LocalProviderInput` thread-through doesn't touch any non-attachment field; each adapter's wire-types extension is additive (existing variants intact); text-only turns produce the same wire bytes as before 4c-2 in every adapter (verified by the unmodified existing tests staying green).
- [ ] **Verification 2: Build + tests + clippy** — `cargo build -p ai && cargo build -p warp` clean; `cargo nextest run -p ai` shows all existing tests still green plus ~22 new (17 unit + 5 integration); `cargo clippy -p ai --all-targets --all-features -- -D warnings` clean.
- [ ] **Verification 3: Manual smoke** — 6/6 per-modality per-adapter smokes pass (see §Task 8.1 README paragraph for the matrix).
- [ ] **Verification 4: Final reviewer + push** — dispatch `oh-my-claudecode:code-reviewer` for the full Phase 4c-2 diff. Stop before push.

---

## Risks & open questions

1. **`ChatMessageContent` untagged enum back-compat.** Phase 4c-2's biggest semantic risk is the `content: Option<String>` → `content: Option<ChatMessageContent>` change in the shared OpenAi wire types. The untagged serde derive makes the JSON shape identical for text-only turns, but every Rust call site that constructs a `ChatMessage` needs the new constructor. Mitigation: the `ChatMessage::text(...)` helper minimizes churn. Existing tests will compile-error against the old field type, surfacing every call site.
2. **Audio support is genuinely sparse.** Only OpenAi (`gpt-4o`) and Gemini accept audio natively; Anthropic, DeepSeek, and Ollama drop it at the translator with a `log::warn!`. 4c-3's input UI hides the audio file-picker option for incapable api_types (via the 4c-1 resolver) rather than letting users attach audio that gets silently dropped.
3. **Ollama's `images` shape is image-only.** No pdf or audio support in Ollama's native chat API. Translator drops both with a `log::warn!`; 4c-3's UI gate prevents the user from attaching them in the first place.
4. **Adapter content-array migration breaks `local_provider_integration.rs` fixtures.** Existing integration tests that compare exact request-body strings against fixtures may need fixture updates if the JSON shape changed (e.g., for OpenAi, the user message's `content` was a string; now it's still a string for text-only via untagged-enum, so fixtures should stay valid — but verify).
5. **Anthropic's `document` block accepts PDF but not arbitrary files.** Translator restricts the Document block to `application/pdf` mime; other document mimes (`.docx`, `.txt`) get dropped with the unsupported warning. Acceptable for first ship; 4c-3 hides the file picker for non-supported mimes.
6. **Persistence deferral.** `AgentAttachment` is session-scoped in 4c-2 — not persisted to the conversation DB. If the user closes the conversation tab, the attachments are gone from history. 4c-3 decides whether to add blob storage for persistence.

---

## Next plan (Phase 4c-3 — Input UI + send-time enforcement + history rendering)

Phase 4c-3 builds the input-bar attachment UI: a 📎 file-picker button + drag-drop target on the agent input editor. Attached files render as removable chips above the input. The Send button's enabled-state predicate calls the 4c-1 resolver per modality against the active model; disables Send + renders an inline error when any attached modality is unsupported. Conversation transcript renders image attachments as inline thumbnails, pdf as 📄 + filename, audio as 🎙️ + filename. The persistence decision (session-scoped vs. blob-storage-backed) is committed in this plan.
