//! Phase B-3a controller-side glue: after a Local LLM Provider turn
//! finishes, dispatch the lib-side [`try_compact`] orchestrator and apply
//! the resulting [`CompletedCompaction`] onto the live `AIConversation`.
//!
//! Lives here (under `app/`) rather than in `crates/ai/` because it touches
//! `BlocklistAIController` / `BlocklistAIHistoryModel` / `AIConversation` —
//! all app-side types. The actual algorithm + summarizer call lives in the
//! `ai` crate so it stays unit-testable independently.

use ai::local_provider::compaction::{
    try_compact, AutoCompactionOutcome, CompletedCompaction, TokenCounts,
};
use warp_multi_agent_api::{self as api, response_event::stream_finished::TokenUsage};
use warpui::{ModelContext, SingletonEntity};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;
use crate::ai::blocklist::BlocklistAIController;

/// Decide whether to fire the auto-compactor for this conversation, and if
/// so, spawn the summarizer call. The actual mutation of
/// `AIConversation::compaction_state` happens in the spawn callback after
/// the network call resolves — no race coordination with the next turn:
/// if the user submits before the summarizer finishes, the next turn just
/// goes uncompacted, and the turn after benefits.
///
/// Called from `BlocklistAIController::handle_response_stream_finished`
/// after the existing usage-update step, so `finished_token_usage` reflects
/// the accumulated counts for the just-finished turn.
pub fn dispatch_auto_compaction(
    _controller: &mut BlocklistAIController,
    conversation_id: AIConversationId,
    finished_token_usage: &[TokenUsage],
    ctx: &mut ModelContext<BlocklistAIController>,
) {
    let Some(cfg) = crate::ai::local_provider_config::snapshot_from_app(ctx) else {
        // Local provider not enabled / configured — nothing to do.
        return;
    };
    let compaction_cfg = crate::ai::local_provider_config::compaction_config_from_app(ctx);
    if !compaction_cfg.auto {
        return;
    }

    // Was this just-finished turn actually a local-provider turn?
    // SSE adapter sets `token_usage[*].model_id` to either the upstream-
    // echoed model name (typically the user's `cfg.model_id`) or the
    // string "local" when the upstream omitted the field. warp.dev turns
    // emit names like "claude-sonnet-4-6" that match neither.
    let was_local_turn = finished_token_usage
        .iter()
        .any(|u| u.model_id == cfg.model_id || u.model_id == "local");
    if !was_local_turn {
        return;
    }

    // Compute observed TokenCounts from the just-finished usage entries
    // (sum across all model_ids that matched, defensively).
    let tokens = aggregate_token_counts(finished_token_usage, &cfg.model_id);

    // Snapshot messages + compaction_state from the conversation.
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
            let outcome =
                try_compact(&messages, &mut state, &cfg, &compaction_cfg, tokens, &http).await;
            // Hand back both the outcome and the (possibly mutated) state
            // so the callback can re-attach the new CompletedCompaction to
            // the live conversation.
            outcome.map(|o| (o, state))
        },
        move |_me, result, ctx| match result {
            Ok((AutoCompactionOutcome::Compacted(_), state)) => {
                // Re-attach the freshly-pushed CompletedCompaction onto the
                // live conversation. We push the entry that try_compact
                // produced in the cloned state — `completed.last()` is the
                // one it just appended.
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
            Ok((AutoCompactionOutcome::Skipped, _)) => {
                // No-op — overflow check was false, or select returned an
                // empty head. Logged inside try_compact at info level.
            }
            Err(e) => {
                log::warn!("[compaction-auto] summarizer call failed: {e}");
            }
        },
    );
}

/// Aggregate token counts across all entries whose `model_id` matches
/// `local_model_id` or the SSE-fallback string `"local"`. Cache reads /
/// writes are folded in too so `is_overflow`'s check matches what the model
/// actually saw.
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
