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
mod tests {
    use super::super::state::CompactionTrigger;
    use super::*;

    #[test]
    fn commit_pushes_completed_and_marks_messages() {
        let mut state = CompactionState::default();
        let outcome = commit_summarization(
            &mut state,
            "## Goal\n- summary".to_string(),
            Some("tail-1".into()),
            true,
            false,
        );
        assert!(outcome.user_msg_id.starts_with("compaction-trigger-"));
        assert!(outcome.assistant_msg_id.starts_with("compaction-summary-"));
        assert!(outcome.auto);
        assert!(outcome.overflow);

        let completed = state.completed();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].user_msg_id, outcome.user_msg_id);
        assert_eq!(completed[0].assistant_msg_id, outcome.assistant_msg_id);
        assert_eq!(completed[0].summary_text.as_deref(), Some("## Goal\n- summary"));
        assert_eq!(completed[0].tail_start_id.as_deref(), Some("tail-1"));

        // The trigger user message was tagged Auto, the assistant message
        // was tagged is_summary.
        let user_marker = state.marker(&outcome.user_msg_id).expect("user marker");
        assert_eq!(user_marker.compaction_trigger, Some(CompactionTrigger::Auto));
        let assistant_marker = state
            .marker(&outcome.assistant_msg_id)
            .expect("assistant marker");
        assert!(assistant_marker.is_summary);
    }

    #[test]
    fn manual_trigger_marks_as_manual_not_auto() {
        let mut state = CompactionState::default();
        let outcome =
            commit_summarization(&mut state, "summary".into(), None, false, true);
        assert!(!outcome.auto);
        assert!(!outcome.overflow);
        let marker = state.marker(&outcome.user_msg_id).unwrap();
        assert_eq!(marker.compaction_trigger, Some(CompactionTrigger::Manual));
    }

    #[test]
    fn previous_summary_returns_most_recently_committed() {
        let mut state = CompactionState::default();
        let _ = commit_summarization(&mut state, "first".into(), None, true, false);
        let _ = commit_summarization(&mut state, "second".into(), None, true, false);
        assert_eq!(state.previous_summary(), Some("second"));
    }

    #[test]
    fn ids_are_unique_per_call() {
        let mut state = CompactionState::default();
        let a = commit_summarization(&mut state, "x".into(), None, true, false);
        let b = commit_summarization(&mut state, "y".into(), None, true, false);
        assert_ne!(a.user_msg_id, b.user_msg_id);
        assert_ne!(a.assistant_msg_id, b.assistant_msg_id);
    }
}
