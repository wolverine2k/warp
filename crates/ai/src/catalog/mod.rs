//! BYOP model catalog (Phase 4b).
//!
//! Sourced from `https://models.dev/api.json` (a curated open-source
//! catalog covering every major commercial provider plus the most-common
//! open-weight models). A `CatalogCache` persists to disk with a 7-day
//! TTL; a baked-in snapshot ships in the binary as a last-resort fallback.
//!
//! Public consumers (the AgentProvidersWidget for inline chips and the
//! Browse-catalog modal) call `lookup_catalog_provider(api_type)` to map
//! a user-configured `AgentProviderApiType` onto the catalog provider id
//! used for chip filtering, then iterate `CatalogCache::all()` to render
//! the matched rows.

pub mod cache;
pub mod fetch;
pub mod parse;
pub mod snapshot;
pub mod wire;

pub use cache::CatalogCache;
pub use fetch::fetch_catalog;
pub use parse::{parse_catalog, CatalogError, CatalogModel};

use super::local_provider::AgentProviderApiType;

/// Map a Warp `AgentProviderApiType` onto the catalog provider id (the
/// top-level key in `models.dev/api.json`). Returns `None` for api_types
/// the catalog doesn't model directly (`OpenAiResp`) — those callers
/// fall through to the "All providers" filter in the Browse-catalog
/// modal and don't render inline chips.
///
/// `Ollama` is special: it has no single catalog provider key because
/// Ollama hosts open-weight models from every upstream. Callers handle
/// the Ollama case by iterating `CatalogCache::all().filter(|m| m.open_weights)`
/// instead of by provider id; this helper returns `None` so the api-type
/// table doesn't try to map it.
pub fn lookup_catalog_provider(api_type: AgentProviderApiType) -> Option<&'static str> {
    match api_type {
        AgentProviderApiType::OpenAi => Some("openai"),
        AgentProviderApiType::Anthropic => Some("anthropic"),
        AgentProviderApiType::Gemini => Some("google"),
        AgentProviderApiType::DeepSeek => Some("deepseek"),
        AgentProviderApiType::Ollama => None,
        AgentProviderApiType::OpenAiResp => None,
    }
}

/// Filter `models` to entries matching the given `api_type`. Ollama gets
/// the open-weights union; OpenAiResp gets nothing.
pub fn filter_models_for_api_type(
    api_type: AgentProviderApiType,
    models: &[CatalogModel],
) -> Vec<&CatalogModel> {
    match api_type {
        AgentProviderApiType::Ollama => {
            models.iter().filter(|m| m.open_weights).collect()
        }
        AgentProviderApiType::OpenAiResp => Vec::new(),
        other => match lookup_catalog_provider(other) {
            Some(provider) => models
                .iter()
                .filter(|m| m.catalog_provider == provider)
                .collect(),
            None => Vec::new(),
        },
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
