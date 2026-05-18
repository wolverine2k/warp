use std::collections::HashSet;

use ai::catalog::CatalogModel;

use super::{CatalogFilter, CatalogModalState};

fn catalog(id: &str, provider: &str) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: format!("Display {id}"),
        context_window: Some(8000),
        max_output_tokens: Some(4000),
        tool_call: true,
        reasoning: false,
        image: false,
        pdf: false,
        audio: false,
        open_weights: false,
    }
}

#[test]
fn new_state_has_empty_checked_and_this_provider_filter() {
    let s = CatalogModalState::new(0, "prov-1".into(), HashSet::new());
    assert!(s.checked.is_empty());
    assert_eq!(s.filter, CatalogFilter::ThisProvider);
    assert!(s.search.is_empty());
}

#[test]
fn toggle_flips_state_and_skips_already_added() {
    let mut already = HashSet::new();
    already.insert("m1".to_string());
    let mut s = CatalogModalState::new(0, "prov-1".into(), already);
    s.toggle("m1", true); // already-added, ignored
    assert!(!s.checked.contains("m1"));
    s.toggle("m2", true);
    assert!(s.checked.contains("m2"));
    s.toggle("m2", false);
    assert!(!s.checked.contains("m2"));
}

#[test]
fn set_filter_switches_between_modes() {
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    s.set_filter(CatalogFilter::AllProviders);
    assert_eq!(s.filter, CatalogFilter::AllProviders);
}

#[test]
fn search_empty_matches_all() {
    let s = CatalogModalState::new(0, "p".into(), HashSet::new());
    assert!(s.matches_search(&catalog("anything", "openai")));
}

#[test]
fn search_matches_case_insensitive_id_or_name() {
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    s.set_search("OPUS".into());
    assert!(s.matches_search(&catalog("claude-opus-4-7", "anthropic")));
    assert!(s.matches_search(&CatalogModel {
        name: "Claude Opus".into(),
        ..catalog("x", "anthropic")
    }));
    assert!(!s.matches_search(&catalog("gpt-4o", "openai")));
}

#[test]
fn committed_rows_lifts_capability_flags() {
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    let c = CatalogModel {
        image: true,
        pdf: false,
        audio: true,
        ..catalog("m1", "openai")
    };
    s.toggle("m1", true);
    let rows = s.committed_rows(std::iter::once(&c));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].image, Some(true));
    assert_eq!(rows[0].pdf, None);
    assert_eq!(rows[0].audio, Some(true));
}

#[test]
fn committed_rows_skips_already_added_even_if_checked() {
    let mut already = HashSet::new();
    already.insert("m1".to_string());
    let mut s = CatalogModalState::new(0, "p".into(), already);
    s.checked.insert("m1".to_string()); // defensive: shouldn't happen via toggle
    let c = catalog("m1", "openai");
    let rows = s.committed_rows(std::iter::once(&c));
    assert!(rows.is_empty());
}

#[test]
fn committed_rows_fills_zero_when_metadata_is_none() {
    let c = CatalogModel {
        context_window: None,
        max_output_tokens: None,
        ..catalog("m1", "openai")
    };
    let mut s = CatalogModalState::new(0, "p".into(), HashSet::new());
    s.toggle("m1", true);
    let rows = s.committed_rows(std::iter::once(&c));
    assert_eq!(rows[0].context_window, 0);
    assert_eq!(rows[0].max_output_tokens, 0);
}
