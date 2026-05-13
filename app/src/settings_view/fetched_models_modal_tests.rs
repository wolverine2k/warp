//! Phase 4a tests for the pure `FetchedModelsModalState` transitions.

use std::collections::HashSet;

use ai::local_provider::adapters::DiscoveredModel;

use super::FetchedModelsModalState;

fn discovered(id: &str) -> DiscoveredModel {
    DiscoveredModel {
        id: id.into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
    }
}

fn discovered_full(id: &str, display: &str, ctx: u32, max: u32) -> DiscoveredModel {
    DiscoveredModel {
        id: id.into(),
        display_name: Some(display.into()),
        context_window: Some(ctx),
        max_output_tokens: Some(max),
    }
}

#[test]
fn new_from_fetched_checks_all_when_no_already_added() {
    let state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2"), discovered("m3")],
        HashSet::new(),
    );
    assert_eq!(state.checked.len(), 3);
    assert!(state.checked.contains("m1"));
    assert!(state.checked.contains("m2"));
    assert!(state.checked.contains("m3"));
}

#[test]
fn new_from_fetched_excludes_already_added_from_default_checked() {
    let already: HashSet<String> = ["m1".to_string()].into_iter().collect();
    let state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2")],
        already,
    );
    assert!(!state.checked.contains("m1"));
    assert!(state.checked.contains("m2"));
}

#[test]
fn toggle_flips_checked_state() {
    let mut state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2")],
        HashSet::new(),
    );
    state.toggle("m1", false);
    assert!(!state.checked.contains("m1"));
    assert!(state.checked.contains("m2"));
    state.toggle("m1", true);
    assert!(state.checked.contains("m1"));
}

#[test]
fn toggle_is_no_op_for_already_added() {
    let already: HashSet<String> = ["m1".to_string()].into_iter().collect();
    let mut state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1")],
        already,
    );
    state.toggle("m1", true);
    assert!(!state.checked.contains("m1"));
}

#[test]
fn set_all_checked_true_excludes_already_added() {
    let already: HashSet<String> = ["m1".to_string()].into_iter().collect();
    let mut state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2")],
        already,
    );
    state.checked.clear();
    state.set_all_checked(true);
    assert!(!state.checked.contains("m1"));
    assert!(state.checked.contains("m2"));
}

#[test]
fn set_all_checked_false_clears_all() {
    let mut state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2")],
        HashSet::new(),
    );
    state.set_all_checked(false);
    assert!(state.checked.is_empty());
}

#[test]
fn committed_rows_uses_display_name_when_present_else_id() {
    let state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![
            discovered_full("m1", "Model 1", 8000, 4000),
            discovered("m2"),
        ],
        HashSet::new(),
    );
    let rows = state.committed_rows();
    assert_eq!(rows.len(), 2);
    let m1 = rows.iter().find(|r| r.id == "m1").unwrap();
    assert_eq!(m1.name, "Model 1");
    assert_eq!(m1.context_window, 8000);
    assert_eq!(m1.max_output_tokens, 4000);
    let m2 = rows.iter().find(|r| r.id == "m2").unwrap();
    assert_eq!(m2.name, "m2");
    assert_eq!(m2.context_window, 0);
    assert_eq!(m2.max_output_tokens, 0);
}

#[test]
fn committed_rows_filters_unchecked() {
    let mut state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2")],
        HashSet::new(),
    );
    state.toggle("m1", false);
    let rows = state.committed_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "m2");
}

#[test]
fn committed_rows_skips_already_added_even_if_checked() {
    let already: HashSet<String> = ["m1".to_string()].into_iter().collect();
    let mut state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1"), discovered("m2")],
        already,
    );
    state.checked.insert("m1".to_string());
    let rows = state.committed_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "m2");
}

#[test]
fn committed_rows_default_capability_flags() {
    let state = FetchedModelsModalState::new_from_fetched(
        0,
        "prov-1".into(),
        vec![discovered("m1")],
        HashSet::new(),
    );
    let rows = state.committed_rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].tool_call);
    assert!(!rows[0].reasoning);
    assert!(rows[0].image.is_none());
    assert!(rows[0].pdf.is_none());
    assert!(rows[0].audio.is_none());
}
