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
