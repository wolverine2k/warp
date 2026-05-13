//! Catalog module — Phase 4b.
//!
//! Public API is stabilized in Task 5; until then we just expose the
//! parse types so the lower-level tests can build.

pub mod parse;
pub mod wire;

pub use parse::{parse_catalog, CatalogError, CatalogModel};
