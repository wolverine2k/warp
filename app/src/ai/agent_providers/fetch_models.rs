//! Per-provider model-list discovery used by the "Fetch models" button in
//! `AgentProvidersWidget` (Phase 4a). Each call selects an adapter for the
//! provider's `api_type`, pre-flights the API-key requirement, builds and
//! sends the model-list request (paginating until exhausted or a 200-entry
//! cap is hit), dedupes by `id`, and returns a structured outcome.
//!
//! The helper is wire-protocol-agnostic — new adapters get fetch support
//! automatically as soon as their `build_list_models_request` /
//! `parse_list_models_response` overrides return something other than
//! `Err(UnsupportedApiType(...))`.
//!
//! Production callers land in Task 8 when `AISettingsPageAction::
//! FetchAgentProviderModels` wires the settings UI button to this helper.
//! Until then, the public symbols here are only referenced from the unit
//! tests; the module-level `dead_code` allow is removed in that task.
#![allow(dead_code)]

use std::time::Duration;

use ai::local_provider::{
    adapters::{DiscoveredModel, ListModelsPage},
    api_type::AgentProviderApiType,
    config::LocalProviderConfig,
    select_adapter,
    ProviderAdapterError as AdapterError,
};

/// Hard caps for the pagination loop. The entry cap bounds the modal
/// size; the page cap bounds the time spent on a misbehaving cursor.
/// `MAX_ENTRIES` is `pub` so the settings handler can use it to flag
/// `truncated: true` in telemetry without duplicating the constant.
pub const MAX_ENTRIES: usize = 200;
const MAX_PAGES: usize = 10;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome of a single `fetch_models` call. `Failed` carries a one-line
/// user-visible reason (first ~120 chars), matching `ProbeOutcome::Failed`.
#[derive(Debug, Clone)]
pub enum FetchModelsOutcome {
    Ok(Vec<DiscoveredModel>),
    Failed(String),
}

impl FetchModelsOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
}

/// Run the full fetch flow for one provider. Selects the adapter,
/// pre-flights API-key requirement, builds + sends the request (with
/// pagination), dedupes by `id`, and returns a structured outcome.
pub async fn fetch_models(
    cfg: LocalProviderConfig,
    http: reqwest::Client,
) -> FetchModelsOutcome {
    match tokio::time::timeout(FETCH_TIMEOUT, fetch_models_inner(cfg, http)).await {
        Ok(outcome) => outcome,
        Err(_) => FetchModelsOutcome::Failed(format!(
            "Request timed out after {}s",
            FETCH_TIMEOUT.as_secs()
        )),
    }
}

async fn fetch_models_inner(
    cfg: LocalProviderConfig,
    http: reqwest::Client,
) -> FetchModelsOutcome {
    let adapter = match select_adapter(cfg.api_type) {
        Ok(a) => a,
        Err(AdapterError::UnsupportedApiType(t)) => {
            return FetchModelsOutcome::Failed(format!(
                "Fetch models not supported for api_type {t:?}"
            ));
        }
        Err(e) => return FetchModelsOutcome::Failed(format!("{e}")),
    };

    // Pre-flight: every adapter except Ollama requires an API key.
    if cfg.api_type != AgentProviderApiType::Ollama
        && cfg.api_key.as_deref().unwrap_or("").is_empty()
    {
        return FetchModelsOutcome::Failed("API key required".into());
    }

    let mut accumulator: Vec<DiscoveredModel> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let req = match adapter.build_list_models_request(&cfg, &http, cursor.as_deref()) {
            Ok(r) => r,
            Err(AdapterError::UnsupportedApiType(t)) => {
                return FetchModelsOutcome::Failed(format!(
                    "Fetch models not supported for api_type {t:?}"
                ));
            }
            Err(e) => return FetchModelsOutcome::Failed(format!("{e}")),
        };
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return FetchModelsOutcome::Failed(truncate_to_120(&format!("{e}"))),
        };
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let body = body.chars().take(120).collect::<String>();
            if body.is_empty() {
                return FetchModelsOutcome::Failed(format!("HTTP {status}"));
            }
            return FetchModelsOutcome::Failed(format!("HTTP {status}: {body}"));
        }
        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => return FetchModelsOutcome::Failed(truncate_to_120(&format!("{e}"))),
        };
        let ListModelsPage {
            mut models,
            next_cursor,
        } = match adapter.parse_list_models_response(&body) {
            Ok(p) => p,
            Err(e) => return FetchModelsOutcome::Failed(format!("Parse error: {e}")),
        };
        accumulator.append(&mut models);
        if accumulator.len() >= MAX_ENTRIES {
            accumulator.truncate(MAX_ENTRIES);
            break;
        }
        match next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    // Dedupe by `id`, keeping first occurrence. Handles overlapping pages.
    let mut seen = std::collections::HashSet::<String>::with_capacity(accumulator.len());
    accumulator.retain(|m| seen.insert(m.id.clone()));

    FetchModelsOutcome::Ok(accumulator)
}

fn truncate_to_120(s: &str) -> String {
    s.chars().take(120).collect()
}

/// Phase 4b cross-phase enrichment. After a successful fetch_models()
/// call returns a Vec<DiscoveredModel> with whatever metadata the live
/// upstream returned (typically just `id`), this helper cross-references
/// each entry against the catalog and fills in missing `display_name`,
/// `context_window`, and `max_output_tokens`. Multimodal capability
/// flags are NOT lifted here — they end up on AgentProviderModel only
/// when the user commits the row via the modal.
///
/// Catalog lookup is opt-in: the caller passes the catalog slice (or
/// an empty slice to skip enrichment entirely). Existing fields on
/// `DiscoveredModel` always win — the catalog only fills `None`s.
pub fn enrich_with_catalog(
    mut models: Vec<DiscoveredModel>,
    api_type: AgentProviderApiType,
    catalog: &[ai::catalog::CatalogModel],
) -> Vec<DiscoveredModel> {
    if catalog.is_empty() {
        return models;
    }
    let candidate_set = ai::catalog::filter_models_for_api_type(api_type, catalog);
    for d in &mut models {
        let Some(c) = candidate_set.iter().find(|c| c.id == d.id) else {
            continue;
        };
        if d.display_name.is_none() && !c.name.is_empty() {
            d.display_name = Some(c.name.clone());
        }
        if d.context_window.is_none() {
            d.context_window = c.context_window;
        }
        if d.max_output_tokens.is_none() {
            d.max_output_tokens = c.max_output_tokens;
        }
    }
    models
}

#[cfg(test)]
#[path = "fetch_models_tests.rs"]
mod tests;
