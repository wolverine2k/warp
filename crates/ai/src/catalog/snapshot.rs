//! Task 4 fills this in. For now, a one-entry placeholder keeps `cache.rs`
//! buildable and satisfies the `!is_empty()` assertion in cache_tests.rs;
//! the placeholder is replaced in Task 4 with the full baked-in catalog
//! from snapshot.json.
use std::sync::LazyLock;
use super::parse::CatalogModel;
pub static BAKED_IN_SNAPSHOT: LazyLock<Vec<CatalogModel>> = LazyLock::new(|| {
    vec![CatalogModel {
        catalog_provider: "openai".to_string(),
        id: "gpt-4o".to_string(),
        name: "GPT-4o".to_string(),
        context_window: Some(128_000),
        max_output_tokens: Some(16_384),
        tool_call: true,
        reasoning: false,
        image: true,
        pdf: false,
        audio: true,
        open_weights: false,
    }]
});
