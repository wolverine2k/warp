//! Parse `models.dev/api.json` into a flat `Vec<CatalogModel>`.
//!
//! The catalog JSON nests models under `{provider_id}.models.{model_id}`.
//! `parse_catalog` flattens this into one entry per model, copying the
//! catalog provider id onto each entry so the api_type filter map can
//! match against it. `modalities.input` is reduced to three booleans.

use thiserror::Error;

use super::wire::WireRoot;

/// One model entry as seen by the rest of the app. Slimmed-down from the
/// raw wire shape — only the fields Warp's `AgentProviderModel` cares
/// about, plus the catalog provider id and the `open_weights` flag used
/// by the Ollama filter rule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogModel {
    /// Catalog provider id, e.g. "anthropic", "openai". Used by the
    /// api_type filter map to decide which chips to suggest.
    pub catalog_provider: String,
    /// Model id as expected by the upstream provider.
    pub id: String,
    /// Human-readable name; for many catalog entries this duplicates `id`.
    pub name: String,
    /// `limit.context`. `None` means "unknown" — chip auto-fill leaves the
    /// row's `context_window` at 0 (the existing "unknown" sentinel on
    /// `AgentProviderModel`).
    pub context_window: Option<u32>,
    /// `limit.output`. Same semantics as `context_window`.
    pub max_output_tokens: Option<u32>,
    /// Whether the model is known to advertise tool/function calling.
    /// Defaults to `true` when absent — matches `AgentProviderModel`'s
    /// existing `default_true` for `tool_call`.
    pub tool_call: bool,
    /// Whether the model emits chain-of-thought reasoning.
    pub reasoning: bool,
    /// `modalities.input` contains `"image"`.
    pub image: bool,
    /// `modalities.input` contains `"pdf"`.
    pub pdf: bool,
    /// `modalities.input` contains `"audio"`.
    pub audio: bool,
    /// `open_weights == true`. The Ollama filter rule unions across all
    /// catalog providers and shows only entries with this flag set, since
    /// Ollama hosts open-weight models regardless of upstream provider.
    pub open_weights: bool,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("HTTP fetch failed: {0}")]
    Fetch(String),
    #[error("HTTP {0}")]
    HttpStatus(u16),
    #[error("response body exceeds 5 MB cap")]
    BodyTooLarge,
    #[error("JSON parse failed: {0}")]
    Parse(String),
    #[error("I/O error: {0}")]
    Io(String),
}

pub fn parse_catalog(body: &str) -> Result<Vec<CatalogModel>, CatalogError> {
    let root: WireRoot =
        serde_json::from_str(body).map_err(|e| CatalogError::Parse(format!("{e}")))?;
    let mut out = Vec::with_capacity(root.values().map(|p| p.models.len()).sum());
    for (provider_id, provider) in root {
        for (_, wire_model) in provider.models {
            let modalities_in = wire_model
                .modalities
                .as_ref()
                .map(|m| m.input.as_slice())
                .unwrap_or(&[]);
            out.push(CatalogModel {
                catalog_provider: provider_id.clone(),
                id: wire_model.id,
                name: if wire_model.name.is_empty() {
                    provider_id.clone()
                } else {
                    wire_model.name
                },
                context_window: wire_model.limit.as_ref().and_then(|l| l.context),
                max_output_tokens: wire_model.limit.as_ref().and_then(|l| l.output),
                tool_call: wire_model.tool_call.unwrap_or(true),
                reasoning: wire_model.reasoning.unwrap_or(false),
                image: modalities_in.iter().any(|s| s == "image"),
                pdf: modalities_in.iter().any(|s| s == "pdf"),
                audio: modalities_in.iter().any(|s| s == "audio"),
                open_weights: wire_model.open_weights.unwrap_or(false),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
