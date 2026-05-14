//! Catalog module — Phase 4b.
//!
//! Public API is stabilized in Task 5; until then we just expose the
//! parse types so the lower-level tests can build.

pub mod cache;
pub mod fetch;
pub mod parse;
pub mod snapshot;
pub mod wire;

pub use cache::CatalogCache;
pub use fetch::fetch_catalog;
pub use parse::{parse_catalog, CatalogError, CatalogModel};
