//! Auto-compaction orchestrator — Phase B-3a.
//!
//! Glue that ties together overflow detection, head/tail selection, the
//! summarizer LLM call, and the state commit. Lives in the `ai` crate so
//! it can be unit-tested independently of `AIConversation`; the app-side
//! wrapper at `app/src/ai/local_provider_compaction.rs` just plumbs the
//! conversation handle in and out.

use warp_multi_agent_api as api;

use crate::local_provider::compaction::algorithm::{self, MessageRef};
use crate::local_provider::compaction::commit::{commit_summarization, CommitOutcome};
use crate::local_provider::compaction::config::{CompactionConfig, CompactionTarget};
use crate::local_provider::compaction::overflow::{is_overflow, usable, ModelLimit, TokenCounts};
use crate::local_provider::compaction::prompt::build_prompt;
use crate::local_provider::compaction::state::CompactionState;
use crate::local_provider::compaction::wire::{build_tool_name_lookup, build_views};
use crate::local_provider::run::{
    build_summarizer_messages, run_summarizer_turn, SummarizerError, SummarizerInput,
};
use crate::local_provider::wire::ChatMessage;

/// Outcome of [`try_compact`].
#[derive(Debug, Clone)]
pub enum AutoCompactionOutcome {
    /// Auto-trigger disabled, no overflow, or `select` returned an empty
    /// head — nothing to do.
    Skipped,
    /// Compaction ran. The new state is already pushed onto
    /// [`CompactionState`] via [`commit_summarization`]; the caller can
    /// inspect [`CommitOutcome`] for the assigned ids.
    Compacted(CommitOutcome),
}

/// Errors encountered while attempting auto-compaction.
#[derive(Debug, thiserror::Error)]
pub enum AutoCompactionError {
    #[error("summarizer call failed: {0}")]
    Summarizer(#[from] SummarizerError),
}

/// Run the full overflow-detect → select → summarize → commit pipeline.
///
/// `messages` is the linearized conversation history (typically
/// `AIConversation::all_linearized_messages()`). `state` is mutated in
/// place — on `Compacted`, the new entry is pushed onto
/// `state.completed`. `tokens` should reflect the most recent observed
/// usage (per-model accumulator on `AIConversation`).
///
/// `manual = true` (Phase B-4 `/compact` user command) skips both the
/// `auto` and `is_overflow` gates and unconditionally proceeds when
/// there's any history — the resulting [`CompletedCompaction`] is
/// tagged `auto = false` so projection / debugging can distinguish it.
///
/// Returns `Ok(Skipped)` when `auto = false` (and `manual = false`),
/// the model isn't overflowing, or `select` couldn't find a head/tail
/// boundary. Returns `Ok(Compacted)` on success. The summarizer call is
/// the only thing that can return an error here — overflow detection
/// and message rendering are pure.
pub async fn try_compact(
    messages: &[api::Message],
    state: &mut CompactionState,
    target: &CompactionTarget,
    compaction_cfg: &CompactionConfig,
    tokens: TokenCounts,
    manual: bool,
    http: &reqwest::Client,
) -> Result<AutoCompactionOutcome, AutoCompactionError> {
    let model =
        ModelLimit::from_context_window(target.primary_cfg.context_window.map(|n| n as usize));
    if !manual && !is_overflow(compaction_cfg, tokens, model) {
        return Ok(AutoCompactionOutcome::Skipped);
    }

    // Build views over the messages so the algorithm can size each one.
    let messages_refs: Vec<&api::Message> = messages.iter().collect();
    let tool_names = build_tool_name_lookup(messages_refs.iter().copied());
    let views = build_views(&messages_refs, &tool_names, state);

    let summarizer_model =
        ModelLimit::from_context_window(target.summarizer_cfg.context_window.map(|n| n as usize));
    let usable_tokens = usable(compaction_cfg, summarizer_model);
    let preserve_budget = compaction_cfg.preserve_recent_budget(usable_tokens);
    let select_result = algorithm::select(
        &views,
        compaction_cfg.tail_turns,
        preserve_budget,
        |slice: &[crate::local_provider::compaction::wire::WireMsg<'_>]| -> usize {
            slice.iter().map(|m| m.estimate_size()).sum()
        },
    );

    if select_result.head_end == 0 {
        // Nothing in the head — the entire conversation is preserved as
        // tail. Either the conversation is short enough to fit (overflow
        // was a false positive against an unconfigured ModelLimit) or
        // `select` couldn't find a viable split point.
        log::info!("[compaction-auto] is_overflow=true but select() returned empty head; skipping");
        return Ok(AutoCompactionOutcome::Skipped);
    }

    // Render the head as ChatMessages for the summarizer body.
    let head_msgs = &messages[..select_result.head_end];
    let mut history: Vec<ChatMessage> = Vec::new();
    for m in head_msgs {
        crate::local_provider::request::push_history_messages(&mut history, m);
    }

    let user_prompt = build_prompt(state.previous_summary(), &[]);
    let summarizer_messages = build_summarizer_messages(
        Some("You are a conversation summarization assistant. Output the requested Markdown structure exactly."),
        history,
        user_prompt,
    );

    log::info!(
        "[compaction-auto] dispatching summarizer: head={} tail_start_id={:?} tokens.count={}",
        select_result.head_end,
        select_result.tail_start_id,
        tokens.count(),
    );

    let summary = run_summarizer_turn(
        SummarizerInput {
            messages: summarizer_messages,
        },
        &target.summarizer_cfg,
        http,
    )
    .await?;

    // overflow=true on the auto path so the synthesized continue prompt
    // carries the "previous request exceeded..." preamble; manual `/compact`
    // (manual=true) renders as a plain continue prompt.
    let outcome = commit_summarization(
        state,
        summary,
        select_result.tail_start_id,
        !manual, // overflow
        manual,
    );
    Ok(AutoCompactionOutcome::Compacted(outcome))
}

#[cfg(test)]
#[path = "auto_tests.rs"]
mod tests;
