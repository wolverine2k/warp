//! One-time migration of legacy `agents.local_provider.*` config into the
//! BYOP shape under `agents.warp_agent.providers`.
//!
//! Trigger: `legacy_local_provider_migrated` setting is `false` AND the
//! providers list is empty AND legacy fields are populated. Idempotent —
//! re-runs no-op once the marker is set or the providers list is non-empty.
//!
//! What this does (Phase 1b-2 stage B per the plan):
//!   1. Synthesize one `AgentProvider` with a fresh UUID, kind = OpenAiCompatible,
//!      api_type = OpenAi, base_url + display_name + one model from the
//!      legacy fields.
//!   2. Move the secret: read `__legacy__` from `AgentProviderSecrets`, write
//!      it under the new UUID, remove the placeholder entry.
//!   3. Append the synthesized provider to `agent_providers` Vec.
//!   4. Set `byop_last_used_model_id = byop:<uuid>:<model_id>`.
//!   5. Set `legacy_local_provider_migrated = true`.
//!
//! The legacy `agents.local_provider.*` settings stay populated for one
//! release as a deprecation window (Phase 1b-4 cleanup removes them). The
//! V1 keychain entry `LocalProviderApiKey` also stays for rollback safety
//! (1b-4 removes that too).
//!
//! Conversation `LLMId` rewrite (`local:` → `byop:`) is handled at
//! conversation-load time in Phase 1b-2 Task 7, not here.

use ai::local_provider::{AgentProviderSecrets, LEGACY_PROVIDER_PLACEHOLDER_ID};
use settings::Setting;
use warpui::{AppContext, SingletonEntity};

use crate::settings::{
    AISettings, AgentProvider, AgentProviderApiType, AgentProviderKind, AgentProviderModel,
};

/// Run the one-time migration if needed. Idempotent.
pub fn migrate_legacy_local_provider_if_needed(ctx: &mut AppContext) {
    // Snapshot all the inputs we need under a single immutable borrow, then
    // drop it before doing any mutations.
    let snapshot: MigrationInputs = {
        let settings = AISettings::as_ref(ctx);
        MigrationInputs {
            already_migrated: *settings.legacy_local_provider_migrated.value(),
            providers_already_present: !settings.agent_providers.value().is_empty(),
            legacy_enabled: *settings.local_provider_enabled.value(),
            legacy_display_name: settings.local_provider_display_name.value().clone(),
            legacy_base_url: settings.local_provider_base_url.value().clone(),
            legacy_model_id: settings.local_provider_model_id.value().clone(),
            legacy_supports_tools: *settings.local_provider_supports_tools.value(),
            legacy_context_window: settings
                .local_provider_context_window
                .value()
                .trim()
                .parse()
                .unwrap_or(0),
            existing_providers: settings.agent_providers.value().clone(),
        }
    };

    if snapshot.already_migrated {
        return;
    }

    // User already has BYOP providers configured (e.g., synced from another
    // device). Don't overwrite; just mark migration done.
    if snapshot.providers_already_present {
        set_marker(ctx);
        return;
    }

    // Legacy provider was never configured — nothing to migrate, just mark
    // the migration done so we don't re-run this check on every launch.
    if !snapshot.legacy_enabled && snapshot.legacy_base_url.trim().is_empty() {
        set_marker(ctx);
        return;
    }

    // Without a model id we can't form a usable BYOP entry. Mark migration
    // done so we don't re-attempt; user will see "no models" in the picker
    // once the Phase 1b-3 widget lands and re-enter via that UI.
    if snapshot.legacy_model_id.trim().is_empty() {
        log::warn!(
            "Skipping legacy local-provider migration: no model id in agents.local_provider.model_id"
        );
        set_marker(ctx);
        return;
    }

    // Generate stable identifiers for the migrated entry.
    let provider_id = uuid::Uuid::new_v4().to_string();
    let provider_name = if snapshot.legacy_display_name.trim().is_empty() {
        "Local".to_owned()
    } else {
        snapshot.legacy_display_name
    };
    let model_id = snapshot.legacy_model_id;

    // Build the AgentProvider.
    let provider = AgentProvider {
        id: provider_id.clone(),
        name: provider_name,
        kind: AgentProviderKind::OpenAiCompatible,
        api_type: AgentProviderApiType::OpenAi,
        base_url: snapshot.legacy_base_url,
        models: vec![AgentProviderModel {
            id: model_id.clone(),
            name: model_id.clone(),
            context_window: snapshot.legacy_context_window,
            max_output_tokens: 0,
            reasoning: false,
            tool_call: snapshot.legacy_supports_tools,
            image: None,
            pdf: None,
            audio: None,
        }],
    };

    // Move the secret: __legacy__ -> <uuid>.
    {
        let secrets_handle = AgentProviderSecrets::handle(ctx);
        secrets_handle.update(ctx, |secrets, ctx| {
            let api_key = secrets
                .get(LEGACY_PROVIDER_PLACEHOLDER_ID)
                .map(str::to_owned);
            if let Some(api_key) = api_key {
                if !api_key.is_empty() {
                    secrets.set(&provider_id, api_key, ctx);
                }
                secrets.remove(LEGACY_PROVIDER_PLACEHOLDER_ID, ctx);
            }
        });
    }

    // Append to providers list + write picker default + set marker — one
    // settings update so listeners see all three changes coherently.
    let mut providers = snapshot.existing_providers;
    providers.push(provider);
    let last_used = format!("byop:{provider_id}:{model_id}");

    AISettings::handle(ctx).update(ctx, |s, ctx| {
        if let Err(e) = s.agent_providers.set_value(providers, ctx) {
            log::error!("Failed to persist migrated agent_providers list: {e:#}");
        }
        if let Err(e) = s.byop_last_used_model_id.set_value(last_used, ctx) {
            log::error!("Failed to persist byop_last_used_model_id after migration: {e:#}");
        }
        if let Err(e) = s.legacy_local_provider_migrated.set_value(true, ctx) {
            log::error!("Failed to persist legacy_local_provider_migrated marker: {e:#}");
        }
    });

    log::info!(
        "Migrated legacy local-provider config into BYOP entry {provider_id} (model {model_id})"
    );
}

fn set_marker(ctx: &mut AppContext) {
    AISettings::handle(ctx).update(ctx, |s, ctx| {
        if let Err(e) = s.legacy_local_provider_migrated.set_value(true, ctx) {
            log::error!("Failed to persist legacy_local_provider_migrated marker: {e:#}");
        }
    });
}

struct MigrationInputs {
    already_migrated: bool,
    providers_already_present: bool,
    legacy_enabled: bool,
    legacy_display_name: String,
    legacy_base_url: String,
    legacy_model_id: String,
    legacy_supports_tools: bool,
    legacy_context_window: u32,
    existing_providers: Vec<AgentProvider>,
}
