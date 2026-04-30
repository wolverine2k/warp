//! App-level glue that snapshots `LocalProviderConfig` from `AISettings` plus
//! the `LocalProviderKeyManager` singleton. Lives here (under `app/`) rather
//! than in `crates/ai/` because it depends on `AISettings`, which is defined
//! in the app crate.
//!
//! Per `specs/GH9303/tech.md` §5: callers that have `&AppContext` (the
//! controller, response_stream, passive_suggestions) call this once per turn
//! to produce an `Option<LocalProviderConfig>`, store it on `RequestParams`,
//! and the AppContext-free dispatch router consumes it.

use std::collections::HashMap;

use ai::local_provider::{LocalProviderConfig, LocalProviderKeyManager};
use ai::LLMId;
use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

use crate::ai::llms::{
    LLMInfo, LLMModelHost, LLMProvider, LLMUsageMetadata, RoutingHostConfig,
};
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

/// Build a synthetic `LLMInfo` for the local provider so it appears in the
/// model picker alongside server-provided models. The synthetic LLMId carries
/// the `local:` prefix the dispatch router checks (per tech.md §5).
#[allow(dead_code)] // Wired up by Phase 5 dispatch fork / Phase 4 injection site.
pub fn synthetic_llm_info(cfg: &LocalProviderConfig) -> LLMInfo {
    let synthetic_id: LLMId = cfg.synthetic_llm_id().into();
    let mut host_configs = HashMap::new();
    host_configs.insert(
        LLMModelHost::Local,
        RoutingHostConfig {
            enabled: true,
            model_routing_host: LLMModelHost::Local,
        },
    );
    LLMInfo {
        display_name: format!("{}: {}", cfg.display_name, cfg.model_id),
        base_model_name: cfg.model_id.clone(),
        id: synthetic_id,
        reasoning_level: None,
        usage_metadata: LLMUsageMetadata {
            request_multiplier: 0,
            credit_multiplier: None,
        },
        description: Some("Custom local provider".to_string()),
        disable_reason: None,
        vision_supported: false,
        spec: None,
        provider: LLMProvider::Unknown,
        host_configs,
        discount_percentage: None,
    }
}

/// Returns true when the given LLMId belongs to a custom local provider.
/// The dispatch router uses this to decide between server and local paths.
#[allow(dead_code)] // Wired up by Phase 5 dispatch fork.
pub fn is_local_llm_id(id: &LLMId) -> bool {
    id.as_str().starts_with("local:")
}
