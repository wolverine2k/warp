//! Commit a freshly-produced summary into [`CompactionState`].
//!
//! Adapted from openwarp's `app/src/ai/byop_compaction/commit.rs` and
//! opencode `compaction.ts processCompaction`'s tail. The openwarp version
//! pulled the summary out of an `AgentOutput` message that the streaming
//! summarizer had already deposited into the conversation; the local
//! provider takes a non-streaming summarizer path (see
//! `super::super::run::run_summarizer_turn`), so the summary text arrives
//! here directly and we generate the synthetic message ids ourselves.
//!
//! This module is conversation-free — the actual splicing of the synthetic
//! `(user, assistant)` pair into the [`warp_multi_agent_api::Task`] message
//! list lives in `app/`, where the AIConversation handle is mutable.

use uuid::Uuid;

use super::state::{CompactionState, CompletedCompaction};

/// What [`commit_summarization`] reports back so the caller can splice the
/// matching synthetic messages into the actual task list.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// Synthetic id assigned to the trigger user message that should be
    /// pushed into the task list with body
    /// [`super::prompt::build_continue_message`].
    pub user_msg_id: String,
    /// Synthetic id assigned to the assistant summary message — its content
    /// is `summary_text`.
    pub assistant_msg_id: String,
    /// The summary text itself (echoed back so the caller can build the
    /// `AgentOutput` proto without re-passing it).
    pub summary_text: String,
    /// Whether this compaction was triggered automatically (overflow path).
    pub auto: bool,
    /// Whether the trigger was a hard overflow vs. a manual `/compact`
    /// (currently always equal to `auto`; mirrored from opencode).
    pub overflow: bool,
}

/// Generate synthetic message ids for the compaction pair, push a
/// [`CompletedCompaction`] entry into `state`, and return the new ids so the
/// caller can splice matching `api::Message` entries into the task list.
///
/// `tail_start_id` is forwarded into the [`CompletedCompaction`] for
/// debug/sanity (see [`super::algorithm::select`]).
pub fn commit_summarization(
    state: &mut CompactionState,
    summary_text: String,
    tail_start_id: Option<String>,
    overflow: bool,
    manual: bool,
) -> CommitOutcome {
    let user_msg_id = format!("compaction-trigger-{}", Uuid::new_v4());
    let assistant_msg_id = format!("compaction-summary-{}", Uuid::new_v4());
    let auto = !manual;

    let completed = CompletedCompaction {
        user_msg_id: user_msg_id.clone(),
        assistant_msg_id: assistant_msg_id.clone(),
        tail_start_id,
        summary_text: Some(summary_text.clone()),
        auto,
        overflow,
    };
    state.push_completed(completed);

    log::info!(
        "[compaction] commit: user_msg={user_msg_id} assistant_msg={assistant_msg_id} \
         summary_len={} auto={auto} overflow={overflow}",
        summary_text.len(),
    );

    CommitOutcome {
        user_msg_id,
        assistant_msg_id,
        summary_text,
        auto,
        overflow,
    }
}

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;
