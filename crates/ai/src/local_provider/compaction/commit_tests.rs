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
    assert_eq!(
        completed[0].summary_text.as_deref(),
        Some("## Goal\n- summary")
    );
    assert_eq!(completed[0].tail_start_id.as_deref(), Some("tail-1"));

    // The trigger user message was tagged Auto, the assistant message
    // was tagged is_summary.
    let user_marker = state.marker(&outcome.user_msg_id).expect("user marker");
    assert_eq!(
        user_marker.compaction_trigger,
        Some(CompactionTrigger::Auto)
    );
    let assistant_marker = state
        .marker(&outcome.assistant_msg_id)
        .expect("assistant marker");
    assert!(assistant_marker.is_summary);
}

#[test]
fn manual_trigger_marks_as_manual_not_auto() {
    let mut state = CompactionState::default();
    let outcome = commit_summarization(&mut state, "summary".into(), None, false, true);
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
