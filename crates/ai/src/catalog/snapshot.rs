//! Baked-in catalog snapshot — last-resort fallback when both the
//! on-disk cache and the live fetch fail. Source: `snapshot.json` in
//! this directory, parsed at startup via the same `parse_catalog`
//! helper that processes a fresh fetch.

use std::sync::LazyLock;

use super::parse::{parse_catalog, CatalogModel};

const SNAPSHOT_JSON: &str = include_str!("snapshot.json");

pub static BAKED_IN_SNAPSHOT: LazyLock<Vec<CatalogModel>> = LazyLock::new(|| {
    parse_catalog(SNAPSHOT_JSON)
        .expect("baked-in catalog snapshot must parse — fix snapshot.json")
});
