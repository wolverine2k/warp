//! Singleton manager for the local provider's optional API key.
//!
//! Mirrors `crate::api_keys::ApiKeyManager` so the same secure-storage and
//! event-emission patterns are reused. We keep this as a SEPARATE manager
//! from `ApiKeyManager` because:
//!
//! - `ApiKeyManager`'s on-the-wire `ApiKeys` struct mirrors the proto type
//!   `warp_multi_agent_api::request::settings::ApiKeys`. Adding a non-wire
//!   field there would confuse readers.
//! - The local provider key has no relationship with Warp's BYO-keys
//!   serialization; a separate manager has a clean blast radius and can be
//!   removed without touching the BYO-keys path.
//!
//! Per `specs/GH9303/tech.md` §2-3, the secure-storage key is
//! `LocalProviderApiKey` and the value is the bearer token to send as
//! `Authorization: Bearer <key>` on outgoing requests.

use serde::{Deserialize, Serialize};
use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "LocalProviderApiKey";

/// Emitted when the user-provided local-provider API key is updated in-memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalProviderKeyManagerEvent {
    KeyUpdated,
}

/// On-disk shape (JSON-encoded) of the secure-storage payload. Wraps the key
/// so future additions (e.g. per-provider expiration) don't break old clients.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct StoredKey {
    api_key: Option<String>,
}

/// Singleton holding the optional local-provider API key.
pub struct LocalProviderKeyManager {
    key: Option<String>,
}

impl LocalProviderKeyManager {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let key = Self::load_from_secure_storage(ctx);
        Self { key }
    }

    /// The current API key, if set. Returns `None` when no key is configured.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Update the key (or clear it by passing `None`). Persists to secure
    /// storage and emits `KeyUpdated`.
    pub fn set_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        // Treat empty strings as "clear" so the UI doesn't accidentally
        // serialize an Authorization header with an empty token.
        self.key = key.filter(|s| !s.is_empty());
        ctx.emit(LocalProviderKeyManagerEvent::KeyUpdated);
        self.write_to_secure_storage(ctx);
    }

    fn load_from_secure_storage(ctx: &mut ModelContext<Self>) -> Option<String> {
        let key_json = match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(json) => json,
            Err(e) => {
                if !matches!(e, secure_storage::Error::NotFound) {
                    log::error!("Failed to read local provider key from secure storage: {e:#}");
                }
                return None;
            }
        };
        let stored: StoredKey = match serde_json::from_str(&key_json) {
            Ok(v) => v,
            Err(e) => {
                log::error!("Failed to deserialize local provider key: {e:#}");
                return None;
            }
        };
        stored.api_key.filter(|s| !s.is_empty())
    }

    fn write_to_secure_storage(&self, ctx: &mut ModelContext<Self>) {
        let stored = StoredKey {
            api_key: self.key.clone(),
        };
        let json = match serde_json::to_string(&stored) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to serialize local provider key: {e:#}");
                return;
            }
        };
        if let Err(e) = ctx.secure_storage().write_value(SECURE_STORAGE_KEY, &json) {
            log::error!("Failed to write local provider key to secure storage: {e:#}");
        }
    }
}

impl Entity for LocalProviderKeyManager {
    type Event = LocalProviderKeyManagerEvent;
}

impl SingletonEntity for LocalProviderKeyManager {}
