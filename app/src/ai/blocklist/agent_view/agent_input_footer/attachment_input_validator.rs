//! Phase 4c-3. Shared input gate for the three attach mechanisms
//! (📎 picker, drag-drop, paste-from-clipboard). Lives in app/ rather than
//! crates/ai/ because capability lookup depends on the user-settings store
//! that's app-side.

// Phase 4c-3 task 2 ships this module with no callers — the callers (📎
// picker, drag-drop, paste) ship in Tasks 3, 5, 6 respectively. Remove this
// allow when Task 3 wires the first caller.
#![allow(dead_code)]

use ai::attachments::AgentAttachment;
use ai::capabilities::{resolve_audio, resolve_image, resolve_pdf};
use ai::catalog::CatalogModel;
use ai::local_provider::AgentProviderApiType;

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
/// caller from the active settings store + model row. Fields mirror
/// `crates/ai/src/capabilities.rs::resolve_image/pdf/audio` exactly:
/// `(api_type, model_id, model_setting, catalog)`.
pub struct ActiveModelCaps<'a> {
    pub api_type: AgentProviderApiType,
    pub model_id: &'a str,
    /// Per-user explicit override for image capability (`None` = not set).
    pub image_setting: Option<bool>,
    /// Per-user explicit override for PDF capability (`None` = not set).
    pub pdf_setting: Option<bool>,
    /// Per-user explicit override for audio capability (`None` = not set).
    pub audio_setting: Option<bool>,
    /// Catalog snapshot used for capability lookup; may be empty — the
    /// resolver falls through to heuristics and conservative-false default.
    pub catalog: &'a [CatalogModel],
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
        if !resolve_image(caps.api_type, caps.model_id, caps.image_setting, caps.catalog) {
            return Err(AttachmentRejection::UnsupportedModality {
                modality: Modality::Image,
                model_id: caps.model_id.to_string(),
            });
        }
    } else if candidate.is_pdf() {
        if !resolve_pdf(caps.api_type, caps.model_id, caps.pdf_setting, caps.catalog) {
            return Err(AttachmentRejection::UnsupportedModality {
                modality: Modality::Pdf,
                model_id: caps.model_id.to_string(),
            });
        }
    } else if candidate.is_audio() {
        if !resolve_audio(caps.api_type, caps.model_id, caps.audio_setting, caps.catalog) {
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

/// Human-readable toast text for each rejection variant.
pub fn rejection_message(rej: &AttachmentRejection) -> String {
    match rej {
        AttachmentRejection::UnsupportedModality { modality, model_id } => {
            let modality_str = match modality {
                Modality::Image => "images",
                Modality::Pdf => "PDFs",
                Modality::Audio => "audio",
            };
            format!("Model '{model_id}' doesn't accept {modality_str}.")
        }
        AttachmentRejection::FileTooLarge { actual_bytes } => {
            let mb = actual_bytes / (1024 * 1024);
            format!("File too large (max 20 MB). Got {mb} MB.")
        }
        AttachmentRejection::TurnLimitReached => {
            "Max 5 attachments per turn. Remove one to add another.".to_string()
        }
        AttachmentRejection::UnknownMime { mime } => {
            format!("Unsupported file type: {mime}.")
        }
    }
}

#[cfg(test)]
#[path = "attachment_input_validator_tests.rs"]
mod tests;
