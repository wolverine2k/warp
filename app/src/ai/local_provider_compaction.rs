//! Phase B-3a controller-side glue: after a Local LLM Provider turn
//! finishes, dispatch the lib-side [`try_compact`] orchestrator and apply
//! the resulting [`CompletedCompaction`] onto the live `AIConversation`.
//!
//! Phase 4d: the dispatch bodies have moved to
//! [`super::compaction_dispatcher::CompactionDispatcher`]; this module is
//! now a thin delegation layer so call sites in `controller.rs` and
//! `slash_command.rs` remain unchanged.

use warp_multi_agent_api::response_event::stream_finished::TokenUsage;
use warpui::ModelContext;

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::BlocklistAIController;
use crate::ai::compaction_dispatcher::CompactionDispatcher;

/// Decide whether to fire the auto-compactor for this conversation, and if
/// so, spawn the summarizer call.
///
/// Called from `BlocklistAIController::handle_response_stream_finished`.
pub fn dispatch_auto_compaction(
    controller: &mut BlocklistAIController,
    conversation_id: AIConversationId,
    finished_token_usage: &[TokenUsage],
    ctx: &mut ModelContext<BlocklistAIController>,
) {
    CompactionDispatcher::dispatch_auto(controller, conversation_id, finished_token_usage, ctx);
}

/// Phase B-4 manual `/compact` dispatch. Returns `true` when the dispatch
/// fired (the conversation exists and the local provider is configured);
/// callers fall through to the warp.dev path on `false`.
pub fn dispatch_manual_compaction(
    controller: &mut BlocklistAIController,
    conversation_id: AIConversationId,
    ctx: &mut ModelContext<BlocklistAIController>,
) -> bool {
    CompactionDispatcher::dispatch_manual(controller, conversation_id, ctx)
}
