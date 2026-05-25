//! User-configured Agent providers (BYOP).
//!
//! Phase 1b-2 ships migration + dispatch routing. Phase 1b-3 brings the
//! settings widget. Phase 4 brings the models.dev catalog and native
//! adapters for non-OpenAI protocols.

pub mod fetch_models;
pub mod migration;
pub mod probe;

use std::collections::HashMap;

use ai::local_provider::{llm_id, AgentProviderSecrets};
use settings::Setting;
use warpui::{AppContext, SingletonEntity};

use crate::ai::llms::{
    AvailableLLMs, DisableReason, LLMContextWindow, LLMInfo, LLMProvider, LLMUsageMetadata,
    ModelsByFeature,
};
use crate::settings::{AISettings, AgentProvider};

/// Build the list of valid `(provider, model)` LLMInfos for the picker.
///
/// "Valid" = provider has a non-empty `base_url`, at least one model, and
/// has an API key in `AgentProviderSecrets`. Invalid providers are silently
/// skipped — the user can spot them via the (Phase 1b-3) settings widget.
pub fn build_byop_llm_infos(app: &AppContext) -> Vec<LLMInfo> {
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let secrets = AgentProviderSecrets::as_ref(app);
    let mut out = Vec::new();

    for provider in providers {
        if provider.base_url.trim().is_empty() {
            continue;
        }
        if provider.models.is_empty() {
            continue;
        }
        let has_key = secrets
            .get(&provider.id)
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if !has_key {
            continue;
        }

        let provider_label = if provider.name.trim().is_empty() {
            provider.id.clone()
        } else {
            provider.name.clone()
        };

        for model in &provider.models {
            if model.id.trim().is_empty() {
                continue;
            }
            let display_name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            out.push(LLMInfo {
                display_name: format!("{provider_label} / {display_name}"),
                base_model_name: format!("{provider_label} / {display_name}"),
                id: llm_id::encode(&provider.id, &model.id),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: None,
                disable_reason: None,
                // Phase 4c will resolve the per-model multimodal capability
                // (image/pdf/audio) here. Phase 1b-2 ships text-only.
                vision_supported: false,
                spec: None,
                provider: LLMProvider::Unknown,
                host_configs: HashMap::new(),
                discount_percentage: None,
                context_window: LLMContextWindow::default(),
            });
        }
    }

    out
}

/// Build the list of BYOP `LLMInfo`s eligible for orchestration pickers.
///
/// Identical to [`build_byop_llm_infos`] but additionally requires
/// `provider.available_for_orchestration == true`. This keeps the
/// orchestration picker scoped to providers the user has explicitly opted
/// in, without affecting the main-conversation picker which uses
/// `build_byop_llm_infos` directly.
///
/// Gated on `FeatureFlag::LocalLlmProvider` at the call site.
pub fn build_byop_orchestration_llm_infos(app: &AppContext) -> Vec<LLMInfo> {
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let secrets = AgentProviderSecrets::as_ref(app);
    let mut out = Vec::new();

    for provider in providers {
        if !provider.available_for_orchestration {
            continue;
        }
        if provider.base_url.trim().is_empty() {
            continue;
        }
        if provider.models.is_empty() {
            continue;
        }
        let has_key = secrets
            .get(&provider.id)
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if !has_key {
            continue;
        }

        let provider_label = if provider.name.trim().is_empty() {
            provider.id.clone()
        } else {
            provider.name.clone()
        };

        for model in &provider.models {
            if model.id.trim().is_empty() {
                continue;
            }
            let display_name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            out.push(LLMInfo {
                display_name: format!("{provider_label} / {display_name}"),
                base_model_name: format!("{provider_label} / {display_name}"),
                id: llm_id::encode(&provider.id, &model.id),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: None,
                disable_reason: None,
                // Phase 4c will resolve the per-model multimodal capability
                // (image/pdf/audio) here. Phase 1b-2 ships text-only.
                vision_supported: false,
                spec: None,
                provider: LLMProvider::Unknown,
                host_configs: HashMap::new(),
                discount_percentage: None,
                context_window: LLMContextWindow::default(),
            });
        }
    }

    out
}

/// Placeholder picker entry shown when no valid BYOP provider is configured.
/// `AvailableLLMs::new` rejects an empty choices list, so we always need at
/// least one entry; this one is greyed out and prompts the user to add a
/// provider in settings.
#[allow(dead_code)] // Wired up by Phase 1b-2 Task 7 (picker swap).
fn placeholder_llm_info() -> LLMInfo {
    LLMInfo {
        display_name: "No custom providers configured — add one in Settings → AI".to_owned(),
        base_model_name: "Not configured".to_owned(),
        id: ai::LLMId::from("byop-placeholder"),
        reasoning_level: None,
        usage_metadata: LLMUsageMetadata {
            request_multiplier: 1,
            credit_multiplier: None,
        },
        description: None,
        disable_reason: Some(DisableReason::Unavailable),
        vision_supported: false,
        spec: None,
        provider: LLMProvider::Unknown,
        host_configs: HashMap::new(),
        discount_percentage: None,
        context_window: LLMContextWindow::default(),
    }
}

/// Build a `ModelsByFeature` populated entirely from the BYOP provider list.
/// All four features (agent_mode / coding / cli_agent / computer_use) get
/// the same model set — custom providers don't differentiate by capability
/// in Phase 1b-2.
#[allow(dead_code)] // Wired up by Phase 1b-2 Task 7 (picker swap).
pub fn build_byop_models_by_feature(app: &AppContext) -> ModelsByFeature {
    let mut choices = build_byop_llm_infos(app);
    if choices.is_empty() {
        choices.push(placeholder_llm_info());
    }

    let default_id = choices[0].id.clone();
    let make = || {
        AvailableLLMs::new(default_id.clone(), choices.clone(), None)
            .expect("choices is non-empty by construction")
    };

    ModelsByFeature {
        agent_mode: make(),
        coding: make(),
        cli_agent: Some(make()),
        computer_use: Some(make()),
    }
}

/// Resolve a `byop:<provider_id>:<model_id>` `LLMId` to its
/// `(provider, api_key, model_id)` triple. Returns `None` if the LLMId is
/// not BYOP-encoded, the provider has been deleted, or no API key is
/// configured. Callers should map `None` to a structured "provider
/// unavailable" error so the conversation pane can surface a recoverable
/// banner instead of a hard crash.
pub fn lookup_byop(app: &AppContext, id: &ai::LLMId) -> Option<(AgentProvider, String, String)> {
    let (provider_id, model_id) = llm_id::decode(id)?;
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let provider = providers.into_iter().find(|p| p.id == provider_id)?;
    let api_key = AgentProviderSecrets::as_ref(app)
        .get(&provider_id)
        .map(str::to_owned)?;
    Some((provider, api_key, model_id))
}

/// Phase 5c. Resolves a `byop:<provider_id>:<model_id>` LLMId to the
/// `(provider, api_key, model_id)` triple a local child-harness spawn site
/// needs to assemble its env-var bag. Returns `None` for:
/// - Non-BYOP model IDs (caller should treat this as "no env vars to inject").
/// - BYOP IDs that don't decode (malformed).
/// - Provider IDs that aren't in settings.
/// - Providers missing an API key in `AgentProviderSecrets`.
///
/// The model_id returned is the user-side model id (the part after the
/// `byop:<provider_id>:` prefix), suitable for passing as `OPENAI_MODEL` /
/// `ANTHROPIC_MODEL` to the harness CLI.
pub fn resolve_byop_for_local_child(
    app: &AppContext,
    model_id: &str,
) -> Option<(AgentProvider, String, String)> {
    let llm_id: ai::LLMId = model_id.into();
    // `llm_id::decode` already returns `None` for non-BYOP ids via its
    // `strip_prefix(BYOP_PREFIX)` guard, so no separate `is_byop` check.
    let (provider_id, byop_model_id) = llm_id::decode(&llm_id)?;

    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let provider = providers.into_iter().find(|p| p.id == provider_id)?;

    let api_key = AgentProviderSecrets::as_ref(app)
        .get(&provider.id)?
        .to_string();
    if api_key.is_empty() {
        return None;
    }

    Some((provider, api_key, byop_model_id))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
