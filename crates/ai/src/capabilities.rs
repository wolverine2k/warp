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
