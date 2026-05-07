//! BYOP (Bring Your Own Provider) `LLMId` prefix codec.
//!
//! BYOP-routed conversations identify their model with the `LLMId` string
//! prefix `byop:` so dispatch can branch between the cloud-Warp path and
//! the user-configured OpenAI-compatible path at request time.
//!
//! Encoding: `byop:<provider_id>:<model_id>`
//!   - `provider_id` is `AgentProvider::id` (a UUID v4 string, no colons).
//!   - `model_id` is `AgentProviderModel::id` (the value sent upstream as
//!     the `model` field). Some upstreams use vendor-prefixed model names
//!     like `vendor:model:variant`, so the codec splits only on the first
//!     colon after the prefix and treats the rest as the model id.
//!
//! Example: `byop:6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34:deepseek-chat`
//!
//! Phase 1b-1 only ships the codec — no caller decodes BYOP IDs yet.
//! Phase 1b-2 wires this into dispatch and the picker.

use crate::LLMId;

pub const BYOP_PREFIX: &str = "byop:";

/// Encode `(provider_id, model_id)` into a BYOP `LLMId`.
pub fn encode(provider_id: &str, model_id: &str) -> LLMId {
    LLMId::from(format!("{BYOP_PREFIX}{provider_id}:{model_id}"))
}

/// If `id` is a BYOP-encoded `LLMId`, return `(provider_id, model_id)`.
/// Returns `None` for the legacy `local:` prefix or any non-BYOP value.
pub fn decode(id: &LLMId) -> Option<(String, String)> {
    let s = id.as_str().strip_prefix(BYOP_PREFIX)?;
    let (pid, mid): (&str, &str) = s.split_once(':')?;
    if pid.is_empty() || mid.is_empty() {
        return None;
    }
    Some((pid.to_owned(), mid.to_owned()))
}

/// Quick `starts_with(BYOP_PREFIX)` check for callers that just need to
/// route between BYOP vs. cloud-Warp without splitting fields.
pub fn is_byop(id: &LLMId) -> bool {
    id.as_str().starts_with(BYOP_PREFIX)
}

#[cfg(test)]
#[path = "llm_id_tests.rs"]
mod tests;
