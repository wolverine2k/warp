//! Phase 4d dispatcher: reads the BYOP compaction-model settings, resolves
//! them to a [`CompactionTarget`] (potentially with a different summarizer
//! config), and delegates to the lib-side [`try_compact`] orchestrator.
//!
//! Lives under `app/` because it reads `AISettings` and touches
//! `BlocklistAIController` / `BlocklistAIHistoryModel` — all app-side types.

use ai::local_provider::compaction::CompactionTarget;
use ai::local_provider::llm_id;
use warpui::{AppContext, SingletonEntity};

use crate::ai::local_provider_config;
use crate::settings::ai::AISettings;

pub struct CompactionDispatcher;

impl CompactionDispatcher {
    /// Read `byop_compaction_model_provider_id` + `byop_compaction_model_id`
    /// from `AISettings`, encode to a `byop:` LLMId, and resolve via
    /// `snapshot_for_request`. Falls back to `CompactionTarget::same_model`
    /// when the settings are empty or the model is unavailable.
    pub fn resolve_target(ctx: &AppContext, primary_cfg: &ai::local_provider::LocalProviderConfig) -> CompactionTarget {
        let ai_settings = AISettings::as_ref(ctx);
        let provider_id = ai_settings.byop_compaction_model_provider_id.to_string();
        let model_id = ai_settings.byop_compaction_model_id.to_string();

        if provider_id.is_empty() || model_id.is_empty() {
            return CompactionTarget::same_model(primary_cfg.clone());
        }

        let byop_llm_id = llm_id::encode(&provider_id, &model_id);
        match local_provider_config::snapshot_for_request(ctx, &byop_llm_id) {
            Some(summarizer_cfg) => CompactionTarget {
                primary_cfg: primary_cfg.clone(),
                summarizer_cfg,
            },
            None => {
                log::warn!(
                    "[compaction-dispatcher] configured compaction model \
                     {provider_id}:{model_id} unavailable; falling back to primary"
                );
                CompactionTarget::same_model(primary_cfg.clone())
            }
        }
    }

    /// Returns `true` when the user's dedicated compaction model is reachable.
    /// Used by the settings UI for inline warnings (Task 7).
    #[allow(dead_code)] // Wired up by Phase 4d Task 7 settings-UI inline warning.
    pub fn compaction_model_available(ctx: &AppContext) -> bool {
        let ai_settings = AISettings::as_ref(ctx);
        let provider_id = ai_settings.byop_compaction_model_provider_id.to_string();
        let model_id = ai_settings.byop_compaction_model_id.to_string();

        if provider_id.is_empty() || model_id.is_empty() {
            // No dedicated model configured — the primary model is always
            // "available" for compaction, so return true.
            return true;
        }

        let byop_llm_id = llm_id::encode(&provider_id, &model_id);
        local_provider_config::snapshot_for_request(ctx, &byop_llm_id).is_some()
    }
}
