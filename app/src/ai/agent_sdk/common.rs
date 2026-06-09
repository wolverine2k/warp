//! Common utilities for agent SDK commands.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::TryFutureExt;
use inquire::{InquireError, Select};
use warp_cli::agent::Harness;
use warp_cli::environment::{EnvironmentCreateArgs, EnvironmentUpdateArgs};
use warpui::r#async::FutureExt;
use warpui::{AppContext, GetSingletonModelHandle, SingletonEntity as _, UpdateModel};

use crate::ai::agent::conversation::ServerAIConversationMetadata;
use crate::ai::agent_sdk::driver::{AgentDriverError, WARP_DRIVE_SYNC_TIMEOUT};
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::cloud_environments::CloudAmbientAgentEnvironment;
use crate::ai::llms::{LLMId, LLMPreferences};
use crate::auth::auth_state::AuthStateProvider;
use crate::cloud_object::{CloudObject, CloudObjectLookup as _, Owner};
use crate::server::cloud_objects::update_manager::UpdateManager;
use crate::server::ids::{ServerId, SyncId};
use crate::server::server_api::ai::AIClient;
use crate::server::server_api::ServerApiProvider;
use crate::workspaces::update_manager::TeamUpdateManager;
use crate::workspaces::user_workspaces::UserWorkspaces;
use settings::Setting;

/// How long to wait for workspace metadata to refresh.
pub const WORKSPACE_METADATA_REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

pub fn validate_agent_mode_base_model_id(
    model_id: &str,
    ctx: &AppContext,
) -> anyhow::Result<LLMId> {
    let llm_prefs = LLMPreferences::as_ref(ctx);

    let llm_id: LLMId = model_id.into();
    let valid_ids = llm_prefs
        .get_base_llm_choices_for_agent_mode(ctx)
        .map(|info| info.id.clone())
        .collect::<Vec<_>>();

    if valid_ids.contains(&llm_id) {
        Ok(llm_id)
    } else {
        let suggestions = valid_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(anyhow::anyhow!(
            "Unknown model id '{model_id}'. Try one of: {suggestions}"
        ))
    }
}

/// Validates a model ID for orchestration use, checking it against the
/// filtered set of models available for the given harness + execution mode.
///
/// For first-party (non-BYOP) model IDs, delegates to the standard
/// `validate_agent_mode_base_model_id` check. For BYOP model IDs (prefixed
/// with `byop:`), runs the full filter pipeline including harness
/// compatibility and Remote reachability, producing structured error
/// messages explaining the specific incompatibility.
///
/// The existing `validate_agent_mode_base_model_id` is unchanged —
/// per-conversation BYOP validation continues to use it.
pub fn validate_orchestration_model_id(
    model_id: &str,
    harness_type: &str,
    execution_mode: &ai::agent::action::RunAgentsExecutionMode,
    ctx: &AppContext,
) -> anyhow::Result<LLMId> {
    use crate::ai::byop_orchestration_filter::{
        base_url_reachable_from_remote, byop_harness_compatible,
    };
    use ai::local_provider::llm_id;

    let llm_id: LLMId = model_id.into();

    // For non-BYOP IDs, delegate to the existing validator.
    if !llm_id::is_byop(&llm_id) {
        return validate_agent_mode_base_model_id(model_id, ctx);
    }

    // Decode the BYOP ID to get provider_id and model_id.
    let (provider_id, byop_model_id) = llm_id::decode(&llm_id).ok_or_else(|| {
        anyhow::anyhow!(
            "Malformed BYOP model ID '{model_id}'. Expected format: byop:<provider_id>:<model_id>"
        )
    })?;

    // Look up the provider.
    let providers = crate::settings::AISettings::as_ref(ctx)
        .agent_providers
        .value()
        .clone();
    let provider = providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "BYOP provider '{provider_id}' not found. \
                 The provider may have been deleted since the model was selected."
            )
        })?;

    // Find the model display name for error messages.
    let model_display = provider
        .models
        .iter()
        .find(|m| m.id == byop_model_id)
        .map(|m| m.name.as_str())
        .unwrap_or(&byop_model_id);
    let provider_display = if provider.name.is_empty() {
        &provider.id
    } else {
        &provider.name
    };

    // Check orchestration opt-in.
    if !provider.available_for_orchestration {
        return Err(anyhow::anyhow!(
            "BYOP model '{provider_display}/{model_display}' is not enabled for orchestration. \
             Enable 'Available for orchestration' in Settings -> AI for this provider."
        ));
    }

    // Check harness compatibility.
    if !byop_harness_compatible(provider.api_type, harness_type) {
        let compatible_harnesses = compatible_harness_names(provider.api_type);
        let api_type = provider.api_type;
        return Err(anyhow::anyhow!(
            "BYOP model '{provider_display}/{model_display}' (API type {api_type:?}) \
             is not compatible with harness '{harness_type}'. \
             Use {compatible_harnesses}, or pick a different model."
        ));
    }

    // Check reachability for Remote mode.
    if execution_mode.is_remote() && !base_url_reachable_from_remote(&provider.base_url) {
        let base_url = &provider.base_url;
        return Err(anyhow::anyhow!(
            "BYOP model '{provider_display}/{model_display}' base URL '{base_url}' \
             is not reachable from Remote execution. \
             Pick Local mode or a publicly-accessible provider."
        ));
    }

    Ok(llm_id)
}

/// Returns a human-readable string listing the harnesses compatible with
/// a given API type, for use in error messages.
fn compatible_harness_names(api_type: ai::local_provider::AgentProviderApiType) -> &'static str {
    use ai::local_provider::AgentProviderApiType;
    match api_type {
        AgentProviderApiType::Anthropic => "'oz' or 'claude'",
        AgentProviderApiType::OpenAi => "'oz', 'codex', or 'opencode'",
        AgentProviderApiType::OpenAiResp => "'oz' or 'codex'",
        AgentProviderApiType::DeepSeek => "'oz', 'codex', or 'opencode'",
        AgentProviderApiType::Gemini => "'oz' or 'gemini'",
        AgentProviderApiType::Ollama => "'oz'",
    }
}

pub(super) fn parse_ambient_task_id(
    run_id: &str,
    error_prefix: &str,
) -> anyhow::Result<AmbientAgentTaskId> {
    run_id
        .parse()
        .map_err(|err| anyhow::anyhow!("{error_prefix} '{run_id}': {err}"))
}

pub(super) fn set_ambient_task_context_from_run_id(
    ctx: &AppContext,
    run_id: &str,
) -> anyhow::Result<AmbientAgentTaskId> {
    let task_id = parse_ambient_task_id(run_id, "Invalid run ID")?;
    ServerApiProvider::handle(ctx)
        .as_ref(ctx)
        .get()
        .set_ambient_agent_task_id(Some(task_id));
    Ok(task_id)
}

/// Resolve the owner of a new cloud object. This resolution is based on the CLI `--team` and `--personal` flags.
///
/// If `team_flag` is true, attempts to get the current team UID (errors if not on a team).
/// If `user_flag` is true, gets the current user's UID.
/// Otherwise, defaults to team if available, falling back to user.
pub fn resolve_owner(team_flag: bool, user_flag: bool, ctx: &AppContext) -> anyhow::Result<Owner> {
    if team_flag {
        let team_id = UserWorkspaces::as_ref(ctx)
            .current_team_uid()
            .ok_or_else(|| anyhow::anyhow!("User is not on a team"))?;
        return Ok(Owner::Team { team_uid: team_id });
    }

    if user_flag {
        let user_id = AuthStateProvider::as_ref(ctx)
            .get()
            .user_id()
            .ok_or_else(|| anyhow::anyhow!("User should be logged in"))?;
        return Ok(Owner::User { user_uid: user_id });
    }

    // Default: try team first, fall back to user
    if let Some(team_uid) = UserWorkspaces::as_ref(ctx).current_team_uid() {
        return Ok(Owner::Team { team_uid });
    }

    log::warn!("Tried to default to creating team object, team could not be found.");
    let user_id = AuthStateProvider::as_ref(ctx)
        .get()
        .user_id()
        .ok_or_else(|| anyhow::anyhow!("User should be logged in"))?;
    Ok(Owner::User { user_uid: user_id })
}

/// Refresh workspace metadata before executing an operation.
///
/// This ensures that team state is up-to-date before creating cloud objects or performing
/// other operations that depend on team membership.
pub fn refresh_workspace_metadata<C>(
    ctx: &mut C,
) -> impl Future<Output = anyhow::Result<()>> + Send + 'static
where
    C: GetSingletonModelHandle + UpdateModel,
{
    let refresh_future = TeamUpdateManager::handle(ctx).update(ctx, |manager, ctx| {
        manager
            .refresh_workspace_metadata(ctx)
            .with_timeout(WORKSPACE_METADATA_REFRESH_TIMEOUT)
    });

    async move {
        let _ = refresh_future
            .await
            .map_err(|_| anyhow::anyhow!("Timed out refreshing team metadata"))?;
        Ok(())
    }
}

/// Refresh Warp Drive before executing an operation.
pub fn refresh_warp_drive(
    ctx: &AppContext,
) -> impl Future<Output = anyhow::Result<()>> + Send + 'static {
    UpdateManager::as_ref(ctx)
        .initial_load_complete()
        .with_timeout(WARP_DRIVE_SYNC_TIMEOUT)
        .map_err(|_| anyhow::anyhow!("Timed out waiting for Warp Drive to sync"))
}

/// Fetch the conversation's server metadata and validate that its harness matches the caller's
/// `--harness` choice. Returns the metadata on success so the caller can reuse it (e.g. for the
/// server conversation token).
///
/// Called up-front before any task/config-build logic consumes `args.harness`, so a mismatch
/// error surfaces before side effects like task creation. We deliberately do NOT auto-upgrade
/// the harness: `Harness::Oz` default with a Claude conversation id is treated as a mismatch
/// and errors out.
pub(super) async fn fetch_and_validate_conversation_harness(
    ai_client: Arc<dyn AIClient>,
    conversation_id: &str,
    args_harness: Harness,
) -> Result<ServerAIConversationMetadata, AgentDriverError> {
    let metadata = ai_client
        .list_ai_conversation_metadata(Some(vec![conversation_id.to_string()]))
        .await
        .map_err(|e| AgentDriverError::ConversationLoadFailed(format!("{e:#}")))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            AgentDriverError::ConversationLoadFailed(format!(
                "conversation {conversation_id} not found or not accessible"
            ))
        })?;

    if metadata.harness != args_harness {
        return Err(AgentDriverError::ConversationHarnessMismatch {
            conversation_id: conversation_id.to_string(),
            expected: Harness::from(metadata.harness).to_string(),
            got: args_harness.to_string(),
        });
    }

    Ok(metadata)
}

/// Format an object owner for display in the CLI.
pub fn format_owner(owner: &Owner) -> &'static str {
    // TODO: For potentially-shared objects, consider looking up the particular user/team name.
    match owner {
        Owner::User { .. } => "Personal",
        Owner::Team { .. } => "Team",
    }
}

/// An error resolving an agent option, which we may have prompted the user for.
#[derive(Debug, thiserror::Error)]
pub enum ResolveConfigurationError {
    /// The user canceled the operation, and we should exit.
    #[error("Operation canceled")]
    Canceled,
    #[error("{id} is not a valid {kind} identifier")]
    InvalidId { id: String, kind: &'static str },
    #[error("{kind} {id} not found")]
    ObjectNotFound { id: String, kind: &'static str },
    #[error(transparent)]
    Other(anyhow::Error),
}

#[derive(Clone, Debug, PartialEq)]
pub enum EnvironmentChoice {
    /// The user explicitly chose not to use an environment.
    None,
    /// The user chose a specific environment.
    Environment { id: String, name: String },
}

impl EnvironmentChoice {
    /// Resolve the environment to use when creating an agent integration.
    /// Warp Drive *must* have been synced first.
    pub fn resolve_for_create(
        args: EnvironmentCreateArgs,
        ctx: &AppContext,
    ) -> Result<Self, ResolveConfigurationError> {
        if args.no_environment {
            Ok(EnvironmentChoice::None)
        } else if let Some(id) = args.environment {
            Self::get_by_id(id, ctx)
        } else {
            let all_environments = CloudAmbientAgentEnvironment::get_all(ctx);
            let mut synced_environments: Vec<(ServerId, &CloudAmbientAgentEnvironment)> =
                all_environments
                    .iter()
                    .filter_map(|env| {
                        if let SyncId::ServerId(server_id) = env.sync_id() {
                            Some((server_id, env))
                        } else {
                            None
                        }
                    })
                    .collect();

            synced_environments
                .sort_by_key(|(_, env)| env.model().string_model.name.to_lowercase());

            let environments: Vec<EnvironmentChoice> = synced_environments
                .into_iter()
                .map(|(server_id, env)| EnvironmentChoice::Environment {
                    id: server_id.to_string(),
                    name: env.model().string_model.name.clone(),
                })
                .collect();

            let mut options = vec![EnvironmentChoice::None];
            options.extend(environments);

            // If there are no synced environments, require the user to create one or use --no-environment.
            if options.len() == 1 {
                let cli_name = warp_cli::binary_name().unwrap_or_else(|| "warp".to_string());
                return Err(ResolveConfigurationError::Other(anyhow::anyhow!(
                    "No environments are configured for this account.\n\
You can create an environment with `{cli_name} environment create`.\n\
Or, re-run this command with `--no-environment` to not use an environment.\n\
Without an environment, the agent will not be able to access private repositories or create pull requests.",
                )));
            }

            let prompt = "Select an environment to run the agent in (or 'No environment'):";

            let choice = Select::new(prompt, options).prompt();

            match choice {
                Ok(choice) => Ok(choice),
                Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                    Err(ResolveConfigurationError::Canceled)
                }
                Err(err) => Err(ResolveConfigurationError::Other(anyhow::anyhow!(
                    "Error selecting environment: {err}"
                ))),
            }
        }
    }

    /// Resolve the environment to use when updating an agent integration. If the user did not
    /// request any changes to the environment, this returns `Ok(None)`.
    /// Warp Drive *must* have been synced first.
    pub fn resolve_for_update(
        args: EnvironmentUpdateArgs,
        ctx: &AppContext,
    ) -> Result<Option<Self>, ResolveConfigurationError> {
        if args.remove_environment {
            Ok(Some(EnvironmentChoice::None))
        } else if let Some(id) = args.environment {
            Self::get_by_id(id, ctx).map(Some)
        } else {
            Ok(None)
        }
    }

    fn get_by_id(id: String, ctx: &AppContext) -> Result<Self, ResolveConfigurationError> {
        let sync_id = SyncId::ServerId(ServerId::try_from(id.as_str()).map_err(|_| {
            ResolveConfigurationError::InvalidId {
                id: id.clone(),
                kind: "environment",
            }
        })?);

        let environment =
            CloudAmbientAgentEnvironment::get_by_id(&sync_id, ctx).ok_or_else(|| {
                ResolveConfigurationError::ObjectNotFound {
                    id: id.clone(),
                    kind: "environment",
                }
            })?;

        Ok(EnvironmentChoice::Environment {
            id,
            name: environment.model().string_model.name.clone(),
        })
    }
}

impl fmt::Display for EnvironmentChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvironmentChoice::None => write!(
                f,
                "No environment (agent will not be able to access private repositories or create pull requests)",
            ),
            EnvironmentChoice::Environment { id, name } => write!(f, "{name} ({id})"),
        }
    }
}

#[cfg(test)]
#[path = "common_tests.rs"]
mod tests;
