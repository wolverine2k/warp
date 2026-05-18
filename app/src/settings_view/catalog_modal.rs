//! Pure state for the "Browse catalog" modal opened by the Browse
//! catalog button in `AgentProvidersWidget` (Phase 4b). Mirrors the
//! Phase 4a `FetchedModelsModalState` pattern: the modal-state
//! transitions live in this module so they can be unit-tested without
//! a GPUI `ViewContext`, and the handler arms in `ai_page.rs` are thin
//! glue over the helpers here.
//!
//! Task 7 in `plan-phase-4b.md` wires this state into action handlers;
//! until then the module is referenced only by its own tests. The
//! module-level `dead_code` allow is removed in that task.
#![allow(dead_code)]

use std::collections::HashSet;

use ai::catalog::CatalogModel;

use crate::settings::AgentProviderModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogFilter {
    /// Filter to entries matching the provider card's `api_type`
    /// (default — shows ~10 relevant rows out of the box).
    ThisProvider,
    /// Show every catalog entry, ordered by catalog_provider then id.
    /// Used for OpenAiResp providers (which have no chip suggestions)
    /// and for users who want to add a model from an unexpected source.
    AllProviders,
}

#[derive(Debug, Clone)]
pub struct CatalogModalState {
    pub provider_index: usize,
    /// Captured at open time; Task 7's commit/resolve handlers compare
    /// this against the provider's current id and drop stale actions
    /// when the provider was removed or replaced mid-modal.
    pub provider_id: String,
    pub filter: CatalogFilter,
    /// Free-form search; matched case-insensitively against `id` + `name`.
    pub search: String,
    /// Model ids currently checked. Default: empty (catalog browsing is
    /// opt-in row-by-row; default-checking would surprise users).
    pub checked: HashSet<String>,
    /// Model ids already on the provider; rendered dimmed and not
    /// commitable. Captured at modal-open time so the user sees a stable
    /// view even if they navigate.
    pub already_added: HashSet<String>,
}

impl CatalogModalState {
    pub fn new(provider_index: usize, provider_id: String, already_added: HashSet<String>) -> Self {
        Self {
            provider_index,
            provider_id,
            filter: CatalogFilter::ThisProvider,
            search: String::new(),
            checked: HashSet::new(),
            already_added,
        }
    }

    pub fn toggle(&mut self, model_id: &str, checked: bool) {
        if self.already_added.contains(model_id) {
            return;
        }
        if checked {
            self.checked.insert(model_id.to_owned());
        } else {
            self.checked.remove(model_id);
        }
    }

    pub fn set_filter(&mut self, filter: CatalogFilter) {
        self.filter = filter;
    }

    pub fn set_search(&mut self, search: String) {
        self.search = search;
    }

    /// Returns a sub-slice of `available` matching the current filter +
    /// search. The caller (the widget render path) does the filtering;
    /// this helper just encodes the predicates so the test surface is
    /// the same logic the widget uses.
    pub fn matches_search(&self, model: &CatalogModel) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let needle = self.search.to_ascii_lowercase();
        model.id.to_ascii_lowercase().contains(&needle)
            || model.name.to_ascii_lowercase().contains(&needle)
    }

    /// Build the AgentProviderModel rows to commit. Filters unchecked
    /// rows and (defensively) already-added rows.
    pub fn committed_rows<'a, I>(&self, available: I) -> Vec<AgentProviderModel>
    where
        I: IntoIterator<Item = &'a CatalogModel>,
    {
        available
            .into_iter()
            .filter(|m| self.checked.contains(&m.id))
            .filter(|m| !self.already_added.contains(&m.id))
            .map(|c| AgentProviderModel {
                name: c.name.clone(),
                id: c.id.clone(),
                context_window: c.context_window.unwrap_or(0),
                max_output_tokens: c.max_output_tokens.unwrap_or(0),
                reasoning: c.reasoning,
                tool_call: c.tool_call,
                image: if c.image { Some(true) } else { None },
                pdf: if c.pdf { Some(true) } else { None },
                audio: if c.audio { Some(true) } else { None },
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "catalog_modal_tests.rs"]
mod tests;
