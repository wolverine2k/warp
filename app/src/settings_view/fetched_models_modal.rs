//! Pure state for the "Fetched models" modal opened by the Fetch button
//! in `AgentProvidersWidget` (Phase 4a). Created when `fetch_models()`
//! resolves with `Ok(...)`; consumed when the user clicks Commit or
//! Cancel.
//!
//! The state and its transitions live in this module so they can be
//! unit-tested without spinning up an `AISettingsPageView` (which owns
//! ~20 `ViewHandle`s and requires a full GPUI App). The handler arms in
//! `ai_page.rs` are thin glue over these helpers.

use std::collections::HashSet;

use ai::local_provider::adapters::DiscoveredModel;

use crate::settings::AgentProviderModel;

#[derive(Debug, Clone)]
pub struct FetchedModelsModalState {
    pub provider_index: usize,
    /// Captured at fetch-start. The resolve callback compares the
    /// captured copy in its closure against the provider's current id
    /// and drops the resolve on mismatch; this field is kept on the
    /// state for forensics and for the Task 9 modal header/subtitle.
    #[allow(dead_code)]
    pub provider_id: String,
    pub fetched: Vec<DiscoveredModel>,
    /// Model ids currently checked. Defaults to "all rows not in
    /// `already_added`" — the user only has to uncheck what they
    /// don't want.
    pub checked: HashSet<String>,
    /// Model ids that are already on the provider — rendered as
    /// disabled rows in the modal.
    pub already_added: HashSet<String>,
}

impl FetchedModelsModalState {
    pub fn new_from_fetched(
        provider_index: usize,
        provider_id: String,
        fetched: Vec<DiscoveredModel>,
        already_added: HashSet<String>,
    ) -> Self {
        let checked: HashSet<String> = fetched
            .iter()
            .filter(|m| !already_added.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
        Self {
            provider_index,
            provider_id,
            fetched,
            checked,
            already_added,
        }
    }

    /// No-op on rows in `already_added` — those render disabled and the
    /// UI shouldn't be able to dispatch a toggle for them, but guard
    /// defensively in case a stale action lands.
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

    pub fn set_all_checked(&mut self, checked: bool) {
        if checked {
            self.checked = self
                .fetched
                .iter()
                .filter(|m| !self.already_added.contains(&m.id))
                .map(|m| m.id.clone())
                .collect();
        } else {
            self.checked.clear();
        }
    }

    /// Build the `AgentProviderModel` rows to append on Commit. Filters
    /// unchecked rows and (defensively) `already_added` rows even if
    /// they somehow appear in `checked`.
    pub fn committed_rows(&self) -> Vec<AgentProviderModel> {
        self.fetched
            .iter()
            .filter(|m| self.checked.contains(&m.id))
            .filter(|m| !self.already_added.contains(&m.id))
            .map(|d| AgentProviderModel {
                name: d.display_name.clone().unwrap_or_else(|| d.id.clone()),
                id: d.id.clone(),
                context_window: d.context_window.unwrap_or(0),
                max_output_tokens: d.max_output_tokens.unwrap_or(0),
                reasoning: false,
                tool_call: true,
                image: None,
                pdf: None,
                audio: None,
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "fetched_models_modal_tests.rs"]
mod tests;
