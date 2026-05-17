use tempfile::TempDir;

use super::{CatalogCache, CatalogModel};

fn sample(id: &str, provider: &str) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: id.to_string(),
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

/// Redirect `cache_path()` to a fresh tempdir for the duration of this test.
/// Returns the `TempDir` guard — dropping it deletes the directory.
fn isolated_cache() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    std::env::set_var("WARP_CATALOG_CACHE_DIR", dir.path());
    dir
}

#[serial_test::serial]
#[test]
fn snapshot_default_signals_needs_refresh() {
    let _dir = isolated_cache();
    let cache = CatalogCache::load_or_default();
    // In a clean test env there is no on-disk cache, so this should be
    // the snapshot — which always needs a refresh.
    assert!(cache.needs_refresh());
    // The snapshot exposes at least one model — see Task 4 / snapshot.json.
    assert!(!cache.all().is_empty());
}

#[serial_test::serial]
#[test]
fn replace_with_fresh_marks_not_snapshot_and_not_stale() {
    let _dir = isolated_cache();
    let mut cache = CatalogCache::load_or_default();
    cache.replace_with_fresh(vec![sample("m1", "openai")]);
    assert!(!cache.is_snapshot());
    assert!(!cache.needs_refresh(), "fresh cache shouldn't need refresh");
    assert_eq!(cache.all().len(), 1);
    assert_eq!(cache.lookup("openai", "m1").unwrap().id, "m1");
}

#[serial_test::serial]
#[test]
fn lookup_returns_none_for_unknown_provider_or_id() {
    let _dir = isolated_cache();
    let mut cache = CatalogCache::load_or_default();
    cache.replace_with_fresh(vec![sample("m1", "openai")]);
    assert!(cache.lookup("openai", "missing").is_none());
    assert!(cache.lookup("missing", "m1").is_none());
}

#[serial_test::serial]
#[test]
fn baked_in_snapshot_covers_all_active_api_types() {
    let _dir = isolated_cache();
    let cache = CatalogCache::load_or_default();
    let models = cache.all();
    assert!(models.iter().any(|m| m.catalog_provider == "openai"));
    assert!(models.iter().any(|m| m.catalog_provider == "anthropic"));
    assert!(models.iter().any(|m| m.catalog_provider == "google"));
    assert!(models.iter().any(|m| m.catalog_provider == "deepseek"));
    assert!(models.iter().any(|m| m.open_weights));
}
