//! User-configured Agent providers (BYOP).
//!
//! Phase 1b-2 ships migration + dispatch routing. Phase 1b-3 brings the
//! settings widget. Phase 4 brings the models.dev catalog and native
//! adapters for non-OpenAI protocols.

pub mod migration;

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
fn build_byop_llm_infos(app: &AppContext) -> Vec<LLMInfo> {
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
#[allow(dead_code)] // Wired up by Phase 1b-2 Task 6 (dispatch).
pub fn lookup_byop(
    app: &AppContext,
    id: &ai::LLMId,
) -> Option<(AgentProvider, String, String)> {
    let (provider_id, model_id) = llm_id::decode(id)?;
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let provider = providers.into_iter().find(|p| p.id == provider_id)?;
    let api_key = AgentProviderSecrets::as_ref(app)
        .get(&provider_id)
        .map(str::to_owned)?;
    Some((provider, api_key, model_id))
}
