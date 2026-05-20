//! Phase 4d dispatcher: reads the BYOP compaction-model settings, resolves
//! them to a [`CompactionTarget`] (potentially with a different summarizer
//! config), and delegates to the lib-side [`try_compact`] orchestrator.
//!
//! Lives under `app/` because it reads `AISettings` and touches
//! `BlocklistAIController` / `BlocklistAIHistoryModel` — all app-side types.

use ai::local_provider::compaction::{
    try_compact, AutoCompactionOutcome, CompactionTarget, CompletedCompaction, TokenCounts,
};
use ai::local_provider::llm_id;
use warp_multi_agent_api::{self as api, response_event::stream_finished::TokenUsage};
use warpui::{AppContext, ModelContext, SingletonEntity};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;
use crate::ai::blocklist::BlocklistAIController;
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

    /// Auto-compaction dispatch. Mirrors the former
    /// `local_provider_compaction::dispatch_auto_compaction` but resolves a
    /// `CompactionTarget` (potentially with a dedicated summarizer) before
    /// handing off to `try_compact`.
    pub fn dispatch_auto(
        _controller: &mut BlocklistAIController,
        conversation_id: AIConversationId,
        finished_token_usage: &[TokenUsage],
        ctx: &mut ModelContext<BlocklistAIController>,
    ) {
        let Some(cfg) = local_provider_config::snapshot_from_app(ctx) else {
            return;
        };
        let compaction_cfg = local_provider_config::compaction_config_from_app(ctx);
        if !compaction_cfg.auto {
            return;
        }

        let was_local_turn = finished_token_usage
            .iter()
            .any(|u| u.model_id == cfg.model_id || u.model_id == "local");
        if !was_local_turn {
            return;
        }

        let tokens = Self::aggregate_token_counts(finished_token_usage, &cfg.model_id);

        let target = Self::resolve_target(ctx, &cfg);

        let history_model = BlocklistAIHistoryModel::handle(ctx);
        let snapshot: Option<(
            Vec<api::Message>,
            ai::local_provider::compaction::CompactionState,
        )> = {
            let history = history_model.as_ref(ctx);
            history.conversation(&conversation_id).map(|conv| {
                let messages: Vec<api::Message> = conv
                    .all_linearized_messages()
                    .iter()
                    .map(|m| (*m).clone())
                    .collect();
                (messages, conv.compaction_state().clone())
            })
        };
        let Some((messages, state_snapshot)) = snapshot else {
            return;
        };

        log::info!(
            "[compaction-auto] dispatching: conversation={} messages={} prior_completed={} tokens.count={}",
            conversation_id,
            messages.len(),
            state_snapshot.completed().len(),
            tokens.count(),
        );

        let http = reqwest::Client::new();
        ctx.spawn(
            async move {
                let mut state = state_snapshot;
                let outcome = try_compact(
                    &messages,
                    &mut state,
                    &target,
                    &compaction_cfg,
                    tokens,
                    false, // manual — the auto path is overflow-driven
                    &http,
                )
                .await;
                outcome.map(|o| (o, state))
            },
            move |_me, result, ctx| match result {
                Ok((AutoCompactionOutcome::Compacted(_), state)) => {
                    let Some(latest) = state.completed().last().cloned() else {
                        log::warn!("[compaction-auto] Compacted outcome but state.completed empty?");
                        return;
                    };
                    let history_model = BlocklistAIHistoryModel::handle(ctx);
                    history_model.update(ctx, |history_model, _ctx| {
                        let Some(conv) = history_model.conversation_mut(&conversation_id) else {
                            log::warn!(
                                "[compaction-auto] conversation gone before commit: {conversation_id}"
                            );
                            return;
                        };
                        let cc = CompletedCompaction {
                            user_msg_id: latest.user_msg_id,
                            assistant_msg_id: latest.assistant_msg_id,
                            tail_start_id: latest.tail_start_id,
                            summary_text: latest.summary_text,
                            auto: latest.auto,
                            overflow: latest.overflow,
                        };
                        conv.compaction_state_mut().push_completed(cc);
                        log::info!(
                            "[compaction-auto] committed summary onto live conversation {conversation_id}"
                        );
                    });
                }
                Ok((AutoCompactionOutcome::Skipped, _)) => {}
                Err(e) => {
                    log::warn!("[compaction-auto] summarizer call failed: {e}");
                }
            },
        );
    }

    /// Manual `/compact` dispatch. Same pipeline as [`Self::dispatch_auto`]
    /// but skips the overflow gate (`manual=true` in [`try_compact`]).
    /// Returns `true` when the dispatch fired.
    pub fn dispatch_manual(
        _controller: &mut BlocklistAIController,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<BlocklistAIController>,
    ) -> bool {
        let Some(cfg) = local_provider_config::snapshot_from_app(ctx) else {
            return false;
        };
        let compaction_cfg = local_provider_config::compaction_config_from_app(ctx);

        let target = Self::resolve_target(ctx, &cfg);

        let history_model = BlocklistAIHistoryModel::handle(ctx);
        let snapshot: Option<(
            Vec<api::Message>,
            ai::local_provider::compaction::CompactionState,
            bool,
        )> = {
            let history = history_model.as_ref(ctx);
            history.conversation(&conversation_id).map(|conv| {
                let messages: Vec<api::Message> = conv
                    .all_linearized_messages()
                    .iter()
                    .map(|m| (*m).clone())
                    .collect();
                let was_local = conv
                    .token_usage()
                    .iter()
                    .any(|u| u.model_id == cfg.model_id || u.model_id == "local");
                (messages, conv.compaction_state().clone(), was_local)
            })
        };
        let Some((messages, state_snapshot, was_local)) = snapshot else {
            return false;
        };
        if !was_local {
            return false;
        }
        if messages.is_empty() {
            log::info!("[compaction-manual] conversation has no history; nothing to compact");
            return false;
        }

        log::info!(
            "[compaction-manual] dispatching: conversation={} messages={} prior_completed={}",
            conversation_id,
            messages.len(),
            state_snapshot.completed().len(),
        );

        let http = reqwest::Client::new();
        ctx.spawn(
            async move {
                let mut state = state_snapshot;
                let outcome = try_compact(
                    &messages,
                    &mut state,
                    &target,
                    &compaction_cfg,
                    TokenCounts::default(),
                    true, // manual — skip overflow gate
                    &http,
                )
                .await;
                outcome.map(|o| (o, state))
            },
            move |_me, result, ctx| match result {
                Ok((AutoCompactionOutcome::Compacted(_), state)) => {
                    let Some(latest) = state.completed().last().cloned() else {
                        log::warn!(
                            "[compaction-manual] Compacted outcome but state.completed empty?"
                        );
                        return;
                    };
                    let history_model = BlocklistAIHistoryModel::handle(ctx);
                    history_model.update(ctx, |history_model, _ctx| {
                        let Some(conv) = history_model.conversation_mut(&conversation_id) else {
                            log::warn!(
                                "[compaction-manual] conversation gone before commit: {conversation_id}"
                            );
                            return;
                        };
                        let cc = CompletedCompaction {
                            user_msg_id: latest.user_msg_id,
                            assistant_msg_id: latest.assistant_msg_id,
                            tail_start_id: latest.tail_start_id,
                            summary_text: latest.summary_text,
                            auto: latest.auto,
                            overflow: latest.overflow,
                        };
                        conv.compaction_state_mut().push_completed(cc);
                        log::info!(
                            "[compaction-manual] committed summary onto live conversation {conversation_id}"
                        );
                    });
                }
                Ok((AutoCompactionOutcome::Skipped, _)) => {}
                Err(e) => log::warn!("[compaction-manual] summarizer call failed: {e}"),
            },
        );
        true
    }

    /// Aggregate token counts across all entries whose `model_id` matches
    /// `local_model_id` or the SSE-fallback string `"local"`.
    fn aggregate_token_counts(usage: &[TokenUsage], local_model_id: &str) -> TokenCounts {
        let mut total = TokenCounts::default();
        for u in usage {
            if u.model_id != local_model_id && u.model_id != "local" {
                continue;
            }
            total.input = total.input.saturating_add(u.total_input as usize);
            total.output = total.output.saturating_add(u.output as usize);
            total.cache_read = total.cache_read.saturating_add(u.input_cache_read as usize);
            total.cache_write = total
                .cache_write
                .saturating_add(u.input_cache_write as usize);
        }
        total
    }
}
