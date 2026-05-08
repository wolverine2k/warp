//! App-level glue that snapshots `LocalProviderConfig` from `AISettings` plus
//! the `AgentProviderSecrets` singleton. Lives here (under `app/`) rather
//! than in `crates/ai/` because it depends on `AISettings`, which is defined
//! in the app crate.
//!
//! Per `specs/GH9303/tech.md` §5: callers that have `&AppContext` (the
//! controller, response_stream, passive_suggestions) call this once per turn
//! to produce an `Option<LocalProviderConfig>`, store it on `RequestParams`,
//! and the AppContext-free dispatch router consumes it.

use std::collections::HashMap;

use ai::local_provider::{AgentProviderSecrets, LocalProviderConfig};
use ai::LLMId;
use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

use crate::ai::llms::{
    LLMContextWindow, LLMInfo, LLMModelHost, LLMProvider, LLMUsageMetadata, RoutingHostConfig,
};
use crate::settings::ai::AISettings;

/// Snapshot the user's local provider config. Returns `None` when:
/// - The `LocalLlmProvider` feature flag is off, OR
/// - `local_provider_enabled` setting is false, OR
/// - The configured base URL or model id is empty / invalid (validation fails).
pub fn snapshot_from_app(ctx: &AppContext) -> Option<LocalProviderConfig> {
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
    let context_window = context_window_str
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0);

    // Capture the key from the singleton manager using the legacy placeholder id
    // during the transition window (Phase 1b-2). Task 4's migration will replace
    // "__legacy__" with the real provider UUID once the AgentProvider is created.
    let api_key = AgentProviderSecrets::as_ref(ctx)
        .get(::ai::local_provider::LEGACY_PROVIDER_PLACEHOLDER_ID)
        .map(str::to_owned);

    let cfg = LocalProviderConfig {
        display_name,
        base_url,
        model_id,
        api_key,
        supports_tools,
        context_window,
        // Legacy single-provider config has always been OpenAI-compatible.
        api_type: ::ai::local_provider::AgentProviderApiType::OpenAi,
    };

    // Honor the validation contract: invalid configs round-trip as None so
    // dispatch / picker injection cleanly skip.
    cfg.validate().ok()?;
    Some(cfg)
}

/// Build a `LocalProviderConfig` for an outgoing request, branching on the
/// LLMId prefix. The override is **scoped to the LLMIds the user explicitly
/// flagged as local-routed**, so picking a cloud-Warp model from the picker
/// keeps using the cloud-Warp dispatch path:
///
/// - `byop:<provider_id>:<model_id>` → look up via
///   `agent_providers::lookup_byop` and build a `LocalProviderConfig`
///   snapshot from the resulting `(AgentProvider, api_key, model_id)`
///   triple. The post-migration steady-state path.
/// - Legacy `local:<model_id>` → fall through to `snapshot_from_app`
///   (legacy `agents.local_provider.*` settings + the `__legacy__`
///   keychain placeholder). Keeps unmigrated/transitional `local:` IDs
///   in pre-existing conversations working.
/// - Anything else (cloud Warp model ids like `claude-3-5-sonnet`,
///   `gpt-4o`, etc.) → return `None` so dispatch falls through to the
///   cloud-Warp path. Phase 1b-2 originally cascaded these through
///   `snapshot_from_app` too, which inherited Phase B-6's
///   "intercept all requests when local is enabled" semantics — fine
///   for the single-provider era but wrong now that the user can pick
///   a `byop:` model alongside cloud ones. The user's explicit picker
///   choice is the dispatch authority.
pub fn snapshot_for_request(ctx: &AppContext, model: &LLMId) -> Option<LocalProviderConfig> {
    if !FeatureFlag::LocalLlmProvider.is_enabled() {
        return None;
    }

    // Path 1: BYOP-encoded model — multi-provider steady state.
    if let Some((provider, api_key, model_id)) = crate::ai::agent_providers::lookup_byop(ctx, model)
    {
        // The single model entry corresponding to model_id within the
        // provider's models list — fall back to first if the LLMId
        // references a model id that's no longer in the list.
        let model_entry = provider
            .models
            .iter()
            .find(|m| m.id == model_id)
            .or_else(|| provider.models.first())?;
        let context_window = if model_entry.context_window > 0 {
            Some(model_entry.context_window)
        } else {
            None
        };
        let cfg = LocalProviderConfig {
            display_name: if provider.name.is_empty() {
                model_id.clone()
            } else {
                provider.name.clone()
            },
            base_url: provider.base_url.clone(),
            model_id: model_id.clone(),
            api_key: Some(api_key),
            supports_tools: model_entry.tool_call,
            context_window,
            // Phase 2: thread the provider's wire-protocol selector through
            // the runtime config so adapter selection works at dispatch time.
            api_type: provider.api_type,
        };
        cfg.validate().ok()?;
        return Some(cfg);
    }

    // Path 2: legacy `local:<model_id>` — pre-migration / transitional.
    if is_local_llm_id(model) {
        return snapshot_from_app(ctx);
    }

    // Path 3: cloud-Warp model — don't intercept.
    None
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
        context_window: LLMContextWindow::default(),
    }
}

/// Returns true when the given LLMId belongs to a custom local provider.
/// The dispatch router uses this to decide between server and local paths.
pub fn is_local_llm_id(id: &LLMId) -> bool {
    id.as_str().starts_with("local:")
}

/// Snapshot the local-provider compaction config from `AISettings`.
/// Phase A only consumes the `prune` field; the rest are wired through so
/// Phase B-3 (summarization) doesn't have to revisit this glue layer.
pub fn compaction_config_from_app(
    ctx: &AppContext,
) -> ai::local_provider::compaction::CompactionConfig {
    use ai::local_provider::compaction::CompactionConfig;
    let s = AISettings::as_ref(ctx);
    let parse_optional = |raw: &str| -> Option<usize> {
        let n = raw.trim().parse::<u32>().ok()?;
        (n > 0).then_some(n as usize)
    };
    let tail_turns_raw = s.local_provider_compaction_tail_turns.to_string();
    let preserve_raw = s
        .local_provider_compaction_preserve_recent_tokens
        .to_string();
    let reserved_raw = s.local_provider_compaction_reserved.to_string();
    CompactionConfig {
        auto: *s.local_provider_compaction_auto,
        prune: *s.local_provider_compaction_prune,
        tail_turns: parse_optional(&tail_turns_raw)
            .unwrap_or(ai::local_provider::compaction::consts::DEFAULT_TAIL_TURNS),
        preserve_recent_tokens: parse_optional(&preserve_raw),
        reserved: parse_optional(&reserved_raw),
    }
}

/// Inject (or refresh) custom-provider entries across every feature list in
/// `ModelsByFeature`. Called after a model-list refresh so the picker shows
/// the user's BYOP models alongside server-provided ones.
///
/// Idempotent: any prior `local:*` (legacy single-provider) and `byop:*`
/// (Phase 1b-2 multi-provider) entries are purged before the latest set is
/// re-pushed. Both prefixes are purged regardless of which one this function
/// ends up adding so the picker doesn't accumulate duplicates across the
/// migration boundary.
///
/// Wires:
/// - During the brief pre-migration transition window, `snapshot_from_app`
///   returns the legacy single-provider config and `synthetic_llm_info`
///   adds one `local:` entry.
/// - Post-migration (the steady state), `agent_providers::build_byop_llm_infos`
///   enumerates the user's `Vec<AgentProvider>` and emits one entry per
///   `(provider, model)` pair using the `byop:<id>:<model>` LLMId format.
/// - Both paths are tried; whichever produces entries wins. In practice
///   only one will be active at a time.
pub fn inject_local_provider_choice(
    models: &mut crate::ai::llms::ModelsByFeature,
    ctx: &AppContext,
) {
    fn is_byop_llm_id(id: &LLMId) -> bool {
        id.as_str().starts_with("byop:")
    }
    fn purge(choices: &mut Vec<LLMInfo>) {
        choices.retain(|info| !is_local_llm_id(&info.id) && !is_byop_llm_id(&info.id));
    }
    purge(models.agent_mode.choices_mut());
    purge(models.coding.choices_mut());
    if let Some(cli) = models.cli_agent.as_mut() {
        purge(cli.choices_mut());
    }
    if let Some(cu) = models.computer_use.as_mut() {
        purge(cu.choices_mut());
    }

    // BYOP path: enumerate every valid (provider, model) pair from
    // agents.warp_agent.providers + AgentProviderSecrets.
    let byop_infos = crate::ai::agent_providers::build_byop_llm_infos(ctx);
    for info in &byop_infos {
        models.agent_mode.choices_mut().push(info.clone());
        models.coding.choices_mut().push(info.clone());
    }

    // Legacy local: path — only active during the pre-migration transition
    // window. After Phase 1b-2's migration helper runs, snapshot_from_app
    // returns None (no api_key under the __legacy__ placeholder), so this
    // branch becomes a no-op.
    if let Some(cfg) = snapshot_from_app(ctx) {
        let info = synthetic_llm_info(&cfg);
        // Append to agent_mode and coding (the two features we expect a
        // local model to participate in). cli_agent and computer_use stay
        // server-only because the local provider's tool catalog (5 v1
        // tools) doesn't include long-running shell or computer-use
        // variants.
        models.agent_mode.choices_mut().push(info.clone());
        models.coding.choices_mut().push(info);
    }
}
