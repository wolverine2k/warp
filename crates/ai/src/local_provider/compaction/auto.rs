//! Auto-compaction orchestrator — Phase B-3a.
//!
//! Glue that ties together overflow detection, head/tail selection, the
//! summarizer LLM call, and the state commit. Lives in the `ai` crate so
//! it can be unit-tested independently of `AIConversation`; the app-side
//! wrapper at `app/src/ai/local_provider_compaction.rs` just plumbs the
//! conversation handle in and out.

use warp_multi_agent_api as api;

use crate::local_provider::{
    compaction::{
        algorithm::{self, MessageRef},
        commit::{commit_summarization, CommitOutcome},
        config::CompactionConfig,
        overflow::{is_overflow, usable, ModelLimit, TokenCounts},
        prompt::build_prompt,
        state::CompactionState,
        wire::{build_tool_name_lookup, build_views},
    },
    config::LocalProviderConfig,
    run::{build_summarizer_messages, run_summarizer_turn, SummarizerError, SummarizerInput},
    wire::ChatMessage,
};

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
/// Returns `Ok(Skipped)` when `auto = false`, the model isn't overflowing,
/// or `select` couldn't find a head/tail boundary. Returns `Ok(Compacted)`
/// on success. The summarizer call is the only thing that can return an
/// error here — overflow detection and message rendering are pure.
pub async fn try_compact(
    messages: &[api::Message],
    state: &mut CompactionState,
    cfg: &LocalProviderConfig,
    compaction_cfg: &CompactionConfig,
    tokens: TokenCounts,
    http: &reqwest::Client,
) -> Result<AutoCompactionOutcome, AutoCompactionError> {
    let model = ModelLimit::from_context_window(cfg.context_window.map(|n| n as usize));
    if !is_overflow(compaction_cfg, tokens, model) {
        return Ok(AutoCompactionOutcome::Skipped);
    }

    // Build views over the messages so the algorithm can size each one.
    let messages_refs: Vec<&api::Message> = messages.iter().collect();
    let tool_names = build_tool_name_lookup(messages_refs.iter().copied());
    let views = build_views(&messages_refs, &tool_names, state);

    let usable_tokens = usable(compaction_cfg, model);
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
        cfg,
        http,
    )
    .await?;

    let outcome = commit_summarization(
        state,
        summary,
        select_result.tail_start_id,
        true,  // overflow
        false, // not manual
    );
    Ok(AutoCompactionOutcome::Compacted(outcome))
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use super::*;
    use crate::local_provider::compaction::CompactionConfig;
    use crate::local_provider::config::LocalProviderConfig;

    /// reqwest's default rustls feature requires a crypto provider before
    /// any TLS use. Installing it here lets these unit tests construct a
    /// `reqwest::Client` without panicking — even though the Skipped paths
    /// never actually call out to the network.
    fn init_crypto_provider() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        });
    }

    fn cfg() -> LocalProviderConfig {
        LocalProviderConfig {
            display_name: "Test".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            model_id: "test-model".into(),
            api_key: None,
            supports_tools: true,
            // Tiny context window so even a single small turn overflows.
            context_window: Some(64),
        }
    }

    fn user_msg(id: &str, q: &str) -> api::Message {
        api::Message {
            id: id.into(),
            message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                query: q.into(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn skipped_when_auto_disabled() {
        init_crypto_provider();
        let mut state = CompactionState::default();
        let compaction_cfg = CompactionConfig {
            auto: false,
            ..CompactionConfig::default()
        };
        let messages: Vec<api::Message> = vec![user_msg("u1", "hi")];
        let http = reqwest::Client::new();
        let r = try_compact(
            &messages,
            &mut state,
            &cfg(),
            &compaction_cfg,
            TokenCounts {
                total: 1_000_000,
                ..Default::default()
            },
            &http,
        )
        .await
        .expect("ok");
        assert!(matches!(r, AutoCompactionOutcome::Skipped));
        assert!(state.completed().is_empty());
    }

    #[tokio::test]
    async fn skipped_when_below_overflow_threshold() {
        init_crypto_provider();
        let mut state = CompactionState::default();
        let compaction_cfg = CompactionConfig::default();
        let messages: Vec<api::Message> = vec![user_msg("u1", "hi")];
        let mut large_window_cfg = cfg();
        large_window_cfg.context_window = Some(200_000);
        let http = reqwest::Client::new();
        let r = try_compact(
            &messages,
            &mut state,
            &large_window_cfg,
            &compaction_cfg,
            TokenCounts {
                total: 100, // way under usable budget
                ..Default::default()
            },
            &http,
        )
        .await
        .expect("ok");
        assert!(matches!(r, AutoCompactionOutcome::Skipped));
        assert!(state.completed().is_empty());
    }

    // The "happy path" case where try_compact actually fires the
    // summarizer is exercised in `crates/ai/tests/local_provider_integration.rs`
    // (auto_compaction_round_trip), which boots a JSON mock server.
}
