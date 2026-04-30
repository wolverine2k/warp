//! App-level glue that snapshots `LocalProviderConfig` from `AISettings` plus
//! the `LocalProviderKeyManager` singleton. Lives here (under `app/`) rather
//! than in `crates/ai/` because it depends on `AISettings`, which is defined
//! in the app crate.
//!
//! Per `specs/GH9303/tech.md` §5: callers that have `&AppContext` (the
//! controller, response_stream, passive_suggestions) call this once per turn
//! to produce an `Option<LocalProviderConfig>`, store it on `RequestParams`,
//! and the AppContext-free dispatch router consumes it.

use ai::local_provider::{LocalProviderConfig, LocalProviderKeyManager};
use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

use crate::settings::ai::AISettings;

/// Snapshot the user's local provider config. Returns `None` when:
#[allow(dead_code)] // Wired up by Phase 4 (picker injection) + Phase 5 (dispatch fork).
/// - The `LocalLlmProvider` feature flag is off, OR
/// - `local_provider_enabled` setting is false, OR
/// - The configured base URL or model id is empty / invalid (validation fails).
pub fn snapshot_from_app(ctx: &mut AppContext) -> Option<LocalProviderConfig> {
    if !FeatureFlag::LocalLlmProvider.is_enabled() {
        return None;
    }
    let ai_settings = AISettings::as_ref(ctx);
    if !*ai_settings.local_provider_enabled {
        return None;
    }
    let display_name = ai_settings.local_provider_display_name.to_string();
    let base_url = ai_settings.local_provider_base_url.to_string();
    let model_id = ai_settings.local_provider_model_id.to_string();
    let supports_tools = *ai_settings.local_provider_supports_tools;
    let context_window_str = ai_settings.local_provider_context_window.to_string();
    let context_window = context_window_str.trim().parse::<u32>().ok().filter(|n| *n > 0);

    // Capture the key from the singleton manager.
    let api_key = LocalProviderKeyManager::as_ref(ctx)
        .key()
        .map(str::to_string);

    let cfg = LocalProviderConfig {
        display_name,
        base_url,
        model_id,
        api_key,
        supports_tools,
        context_window,
    };

    // Honor the validation contract: invalid configs round-trip as None so
    // dispatch / picker injection cleanly skip.
    cfg.validate().ok()?;
    Some(cfg)
}
