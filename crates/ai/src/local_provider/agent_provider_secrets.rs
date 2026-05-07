//! `AgentProviderSecrets`: per-provider API keys stored in OS secure storage.
//!
//! Data shape: `HashMap<provider_id, api_key>` serialized as JSON under the
//! `AgentProviderSecrets` keychain key.
//!
//! Phase 1b-2: introduced as a refactor of the prior single-key singleton.
//! On first load, if no V2 blob exists at `AgentProviderSecrets`, the legacy
//! V1 blob at `LocalProviderApiKey` is read; its single api_key (if any) is
//! ported into the new map under the stable placeholder id
//! `LEGACY_PROVIDER_PLACEHOLDER_ID = "__legacy__"`. Phase 1b-2 Task 4's
//! migration helper later moves that entry to a UUID. Phase 1b-4 cleanup
//! removes the legacy keychain entry.

use std::collections::HashMap;

use serde::Deserialize;
use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "AgentProviderSecrets";
const LEGACY_SECURE_STORAGE_KEY: &str = "LocalProviderApiKey";

/// Stable placeholder id for the legacy single-provider api key carried over
/// from V1. `LocalProviderConfig::snapshot_from_app` looks up the legacy
/// secret under this id during the transition window. Phase 1b-2 Task 4's
/// migration replaces this with a fresh UUID.
pub const LEGACY_PROVIDER_PLACEHOLDER_ID: &str = "__legacy__";

/// Emitted when any provider's api key changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProviderSecretsEvent {
    KeysUpdated,
}

/// V1 (legacy) on-disk shape — single api_key wrapped in a tagged struct so
/// future additions don't break old clients. Read-only here; we never write
/// the V1 shape after Phase 1b-2.
#[derive(Debug, Default, Deserialize)]
struct LegacyStoredKey {
    api_key: Option<String>,
}

/// Singleton: per-provider API keys.
pub struct AgentProviderSecrets {
    keys: HashMap<String, String>,
}

impl AgentProviderSecrets {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let keys = Self::load_from_storage(ctx);
        Self { keys }
    }

    pub fn get(&self, provider_id: &str) -> Option<&str> {
        self.keys.get(provider_id).map(String::as_str)
    }

    pub fn set(&mut self, provider_id: &str, api_key: String, ctx: &mut ModelContext<Self>) {
        if api_key.is_empty() {
            if self.keys.remove(provider_id).is_none() {
                return;
            }
        } else {
            self.keys.insert(provider_id.to_owned(), api_key);
        }
        ctx.emit(AgentProviderSecretsEvent::KeysUpdated);
        self.persist(ctx);
    }

    pub fn remove(&mut self, provider_id: &str, ctx: &mut ModelContext<Self>) {
        if self.keys.remove(provider_id).is_some() {
            ctx.emit(AgentProviderSecretsEvent::KeysUpdated);
            self.persist(ctx);
        }
    }

    pub fn provider_ids(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    fn load_from_storage(ctx: &mut ModelContext<Self>) -> HashMap<String, String> {
        // Try the V2 keychain blob first.
        match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(json) => match serde_json::from_str::<HashMap<String, String>>(&json) {
                Ok(map) => return map,
                Err(e) => {
                    log::error!("Failed to deserialize AgentProviderSecrets V2 blob: {e:#}");
                    // Fall through to V1 fallback as a recovery path.
                }
            },
            Err(secure_storage::Error::NotFound) => { /* fall through to V1 */ }
            Err(e) => {
                log::error!("Failed to read AgentProviderSecrets: {e:#}");
                return HashMap::new();
            }
        }

        // V1 fallback: legacy single-key blob.
        let legacy_raw = match ctx.secure_storage().read_value(LEGACY_SECURE_STORAGE_KEY) {
            Ok(json) => json,
            Err(secure_storage::Error::NotFound) => return HashMap::new(),
            Err(e) => {
                log::error!("Failed to read legacy LocalProviderApiKey blob: {e:#}");
                return HashMap::new();
            }
        };
        let legacy: LegacyStoredKey = serde_json::from_str(&legacy_raw).unwrap_or_else(|e| {
            log::error!("Failed to deserialize legacy LocalProviderApiKey blob: {e:#}");
            LegacyStoredKey::default()
        });

        let mut map = HashMap::new();
        if let Some(k) = legacy.api_key.filter(|k| !k.is_empty()) {
            map.insert(LEGACY_PROVIDER_PLACEHOLDER_ID.to_owned(), k);
            // Persist immediately under V2 so the next launch skips the V1 path.
            let json =
                serde_json::to_string(&map).expect("HashMap<String,String> always serializes");
            if let Err(e) = ctx.secure_storage().write_value(SECURE_STORAGE_KEY, &json) {
                log::error!("Failed to write V2 AgentProviderSecrets after V1 migration: {e:#}");
            }
        }
        map
    }

    fn persist(&self, ctx: &mut ModelContext<Self>) {
        let json = match serde_json::to_string(&self.keys) {
            Ok(json) => json,
            Err(e) => {
                log::error!("Failed to serialize AgentProviderSecrets: {e:#}");
                return;
            }
        };
        if let Err(e) = ctx.secure_storage().write_value(SECURE_STORAGE_KEY, &json) {
            log::error!("Failed to write AgentProviderSecrets: {e:#}");
        }
    }
}

impl Entity for AgentProviderSecrets {
    type Event = AgentProviderSecretsEvent;
}

impl SingletonEntity for AgentProviderSecrets {}
