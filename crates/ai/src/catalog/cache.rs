//! On-disk cache for the parsed catalog.
//!
//! Lifecycle:
//!   1. `CatalogCache::load_or_default()` reads the on-disk JSON. If the
//!      file doesn't exist or fails to parse, returns a `CatalogCache`
//!      backed by the baked-in snapshot with `fetched_at = None` so the
//!      next `needs_refresh()` call returns `true`.
//!   2. The settings page checks `needs_refresh()` on open and kicks off
//!      a background `fetch_catalog()` if stale; the UI renders against
//!      the in-memory copy in the meantime.
//!   3. When fetch completes, `replace_with_fresh(models)` rotates the
//!      in-memory state and calls `save()` to persist.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::parse::{CatalogError, CatalogModel};
use super::snapshot::BAKED_IN_SNAPSHOT;

const TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const CACHE_FILE_NAME: &str = "byop_catalog.json";

#[derive(Debug, Clone)]
pub struct CatalogCache {
    models: Vec<CatalogModel>,
    /// Unix seconds when the catalog was last successfully fetched.
    /// `None` when backed by the baked-in snapshot (forces a refresh).
    fetched_at: Option<u64>,
    /// Whether the in-memory copy is the baked-in snapshot (signals the
    /// settings UI to show a "using built-in fallback" caption).
    is_snapshot: bool,
}

/// On-disk serialization format. Versioned to allow future schema bumps
/// without invalidating existing caches silently.
#[derive(Debug, Serialize, Deserialize)]
struct OnDisk {
    version: u8,
    fetched_at: u64,
    models: Vec<CatalogModel>,
}

const ON_DISK_VERSION: u8 = 1;

impl CatalogCache {
    /// Load from `<config_dir>/<CACHE_FILE_NAME>` if present and valid;
    /// otherwise fall back to the baked-in snapshot.
    pub fn load_or_default() -> Self {
        match cache_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(body) => match serde_json::from_str::<OnDisk>(&body) {
                Ok(d) if d.version == ON_DISK_VERSION => Self {
                    models: d.models,
                    fetched_at: Some(d.fetched_at),
                    is_snapshot: false,
                },
                _ => Self::from_snapshot(),
            },
            None => Self::from_snapshot(),
        }
    }

    fn from_snapshot() -> Self {
        Self {
            models: BAKED_IN_SNAPSHOT.clone(),
            fetched_at: None,
            is_snapshot: true,
        }
    }

    pub fn all(&self) -> &[CatalogModel] {
        &self.models
    }

    pub fn is_snapshot(&self) -> bool {
        self.is_snapshot
    }

    pub fn fetched_at(&self) -> Option<u64> {
        self.fetched_at
    }

    /// Returns `true` when the cache is older than `TTL_SECONDS`, when no
    /// fetch has happened yet, or when the current copy is the snapshot.
    pub fn needs_refresh(&self) -> bool {
        if self.is_snapshot {
            return true;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        match self.fetched_at {
            None => true,
            Some(t) => {
                if now < t {
                    return true; // clock went backwards; force refresh
                }
                now - t > TTL_SECONDS
            }
        }
    }

    /// Replace the in-memory state with a fresh fetch and persist to disk.
    /// The persistence error is logged but not propagated — a failed save
    /// shouldn't break the user's session.
    pub fn replace_with_fresh(&mut self, models: Vec<CatalogModel>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.models = models;
        self.fetched_at = Some(now);
        self.is_snapshot = false;
        if let Err(e) = self.save() {
            log::warn!("catalog cache save failed: {e}");
        }
    }

    fn save(&self) -> Result<(), CatalogError> {
        let path = cache_path().ok_or_else(|| {
            CatalogError::Io("config dir unavailable".to_string())
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CatalogError::Io(format!("{e}")))?;
        }
        let on_disk = OnDisk {
            version: ON_DISK_VERSION,
            fetched_at: self.fetched_at.unwrap_or(0),
            models: self.models.clone(),
        };
        let body = serde_json::to_string_pretty(&on_disk)
            .map_err(|e| CatalogError::Parse(format!("{e}")))?;
        atomic_write(&path, &body).map_err(|e| CatalogError::Io(format!("{e}")))
    }

    /// Lookup by `(catalog_provider, model_id)`. Returns the first match.
    pub fn lookup(&self, catalog_provider: &str, model_id: &str) -> Option<&CatalogModel> {
        self.models
            .iter()
            .find(|m| m.catalog_provider == catalog_provider && m.id == model_id)
    }
}

fn cache_path() -> Option<PathBuf> {
    // In tests, callers may set WARP_CATALOG_CACHE_DIR to redirect I/O to a
    // tempdir so tests don't share the real config directory.
    if let Ok(dir) = std::env::var("WARP_CATALOG_CACHE_DIR") {
        return Some(PathBuf::from(dir).join(CACHE_FILE_NAME));
    }
    dirs::config_dir().map(|d| d.join("warp").join(CACHE_FILE_NAME))
}

/// Write `body` to `path` via a temp-file + rename so a crashed save
/// can't leave a half-written cache that fails to parse on the next boot.
/// Uses `tempfile::NamedTempFile` so concurrent writers each get a unique
/// temp path and don't clobber one another.
fn atomic_write(path: &Path, body: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent dir"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    tmp.write_all(body.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
