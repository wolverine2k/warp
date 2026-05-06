use std::{collections::HashMap, sync::Arc};

use crate::{ai::agent::redaction, terminal::model::session::SessionType};
use ai::local_provider;
use futures_util::StreamExt;
use warp_core::features::FeatureFlag;
use warp_multi_agent_api as api;

use crate::server::server_api::{AIApiError, ServerApi};

use super::{convert_to::convert_input, ConvertToAPITypeError, RequestParams, ResponseStream};

pub async fn generate_multi_agent_output(
    server_api: Arc<ServerApi>,
    mut params: RequestParams,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    // ---- Custom Local LLM Provider fork (specs/GH9303/) ----
    //
    // If a local provider is configured + enabled, route this request to
    // the user's endpoint instead of warp.dev — regardless of which model
    // the conversation has selected. Configuring + enabling the local
    // provider is the user's explicit opt-in, and forcing every Agent
    // Mode request through it lets users on free / analytics-disabled
    // plans (where warp.dev's MultiAgent endpoint rejects with HTTP 400
    // "App analytics must be enabled") use Agent Mode at all.
    //
    // We retain the `local:` model-id signal as a separate diagnostic:
    // if the user explicitly selected a local model but no provider is
    // configured, surface a clear error rather than silently routing to
    // warp.dev (which would then fail in a less-obvious way).
    //
    // The snapshot was populated at `RequestParams::new` time from
    // `&AppContext`; this code path stays AppContext-free per tech.md §5.
    if let Some(cfg) = params.local_provider_config.take() {
        return route_to_local_provider(params, cfg, cancellation_rx).await;
    }
    if crate::ai::local_provider_config::is_local_llm_id(&params.model) {
        // Stale local id but no active config (user disabled the provider
        // but their saved profile still references it). Surface a
        // one-shot error stream so the controller's existing toast path
        // fires; the user can re-select a Warp model.
        let (tx, rx) = async_channel::unbounded();
        let err = AIApiError::Other(anyhow::anyhow!(
            "Local provider is no longer configured. Re-enable it in settings, or pick a Warp model."
        ));
        let _ = tx.send(Err(Arc::new(err))).await;
        return Ok(Box::pin(rx));
    }

    let supported_tools = params
        .supported_tools_override
        .take()
        .unwrap_or_else(|| get_supported_tools(&params));
    let supported_cli_agent_tools = get_supported_cli_agent_tools(&params);
    let mut logging_metadata = HashMap::new();
    if let Some(metadata) = params.metadata {
        logging_metadata.insert(
            "is_autodetected_user_query".to_owned(),
            prost_types::Value {
                kind: Some(prost_types::value::Kind::BoolValue(
                    metadata.is_autodetected_user_query,
                )),
            },
        );
        logging_metadata.insert(
            "entrypoint".to_owned(),
            prost_types::Value {
                kind: Some(prost_types::value::Kind::StringValue(
                    metadata.entrypoint.entrypoint(),
                )),
            },
        );
        logging_metadata.insert(
            "is_auto_resume_after_error".to_owned(),
            prost_types::Value {
                kind: Some(prost_types::value::Kind::BoolValue(
                    metadata.is_auto_resume_after_error,
                )),
            },
        );
    }

    if params.should_redact_secrets {
        redaction::redact_inputs(&mut params.input);
    }

    let mut api_keys = params.api_keys;
    if let Some(api_keys) = &mut api_keys {
        api_keys.allow_use_of_warp_credits = params.allow_use_of_warp_credits_with_byok;
    }

    let request = api::Request {
        task_context: Some(api::request::TaskContext {
            tasks: params.tasks,
        }),
        input: Some(convert_input(params.input)?),
        settings: Some(api::request::Settings {
            model_config: Some(api::request::settings::ModelConfig {
                base: params.model.into(),
                cli_agent: params.cli_agent_model.into(),
                computer_use_agent: params.computer_use_model.into(),
                base_model_context_window_limit: if FeatureFlag::ConfigurableContextWindow
                    .is_enabled()
                {
                    params.context_window_limit.unwrap_or(0)
                } else {
                    0
                },
                ..Default::default()
            }),
            rules_enabled: params.is_memory_enabled,
            warp_drive_context_enabled: params.warp_drive_context_enabled,
            web_context_retrieval_enabled: true,
            supports_parallel_tool_calls: true,
            use_anthropic_text_editor_tools: false,
            planning_enabled: params.planning_enabled,
            supports_create_files: true,
            supported_tools: supported_tools.into_iter().map(Into::into).collect(),
            supports_long_running_commands: true,
            should_preserve_file_content_in_history: true,
            supports_todos_ui: true,
            supports_linked_code_blocks: FeatureFlag::LinkedCodeBlocks.is_enabled(),
            supports_started_child_task_message: true,
            supports_suggest_prompt: true,
            supports_read_image_files: FeatureFlag::ReadImageFiles.is_enabled(),
            supports_reasoning_message: true,
            api_keys,
            autonomy_level: params.autonomy_level.into(),
            isolation_level: params.isolation_level.into(),
            web_search_enabled: params.web_search_enabled,
            supported_cli_agent_tools: supported_cli_agent_tools
                .into_iter()
                .map(Into::into)
                .collect(),
            supports_v4a_file_diffs: FeatureFlag::V4AFileDiffs.is_enabled(),
            supports_summarization_via_message_replacement:
                FeatureFlag::SummarizationViaMessageReplacement.is_enabled(),
            supports_bundled_skills: FeatureFlag::BundledSkills.is_enabled(),
            supports_research_agent: params.research_agent_enabled,
            supports_orchestration_v2: FeatureFlag::OrchestrationV2.is_enabled(),
        }),
        metadata: Some(api::request::Metadata {
            logging: logging_metadata,
            conversation_id: params
                .conversation_token
                .as_ref()
                .map(|token| token.as_str().to_string())
                .unwrap_or_default(),
            ambient_agent_task_id: params
                .ambient_agent_task_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            forked_from_conversation_id: if params.conversation_token.is_none() {
                // We only include this param on our initial request to the server
                // (when the forked conversation has not been assigned a new id yet).
                params
                    .forked_from_conversation_token
                    .map(|token| token.as_str().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            parent_agent_id: params.parent_agent_id.unwrap_or_default(),
            agent_name: params.agent_name.unwrap_or_default(),
        }),
        existing_suggestions: params
            .existing_suggestions
            .map(|suggestions| suggestions.into()),
        mcp_context: params.mcp_context.map(Into::into),
    };

    let response_stream = server_api.generate_multi_agent_output(&request).await;
    match response_stream {
        Ok(stream) => {
            let output_stream = stream.take_until(cancellation_rx);
            Ok(Box::pin(output_stream))
        }
        Err(e) => {
            let (tx, rx) = async_channel::unbounded();
            let _ = tx.send(Err(e)).await;
            Ok(Box::pin(rx))
        }
    }
}

/// Custom Local LLM Provider routing — see `specs/GH9303/`.
///
/// Translates the in-memory RequestParams into the local provider's input shape
/// and invokes `run_chat_turn`. Errors that prevent producing a stream
/// (config validation, transport setup) are surfaced through the same one-shot
/// channel pattern the server path uses on `Err(...)`.
async fn route_to_local_provider(
    mut params: RequestParams,
    cfg: ai::local_provider::LocalProviderConfig,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    let supported_tools = params
        .supported_tools_override
        .take()
        .unwrap_or_else(|| get_supported_tools(&params));

    let user_query = extract_latest_user_query(&params.input);
    let tasks = std::mem::take(&mut params.tasks);

    // Plumb the controller's existing conversation + task ids through to
    // the SSE adapter so emitted events match the AIConversation the
    // controller is driving. Without this, OpenAiSseAdapter generates
    // fresh `local:<uuid>` ids that don't exist in the conversation, every
    // event triggers `UpdateConversationError::TaskNotFound`, and the user
    // sees no output. The conversation token may be absent on the very
    // first turn (before the server-side conversation token is assigned);
    // the task id must come from the latest task in `tasks` because that's
    // the one the controller is actively writing into.
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_string());
    // Prefer the controller's root task id (always set for normal turns), falling
    // back to the most recent task in `tasks` for paths that pre-populate it.
    // `params.tasks` is empty for local-only conversations because
    // `compute_active_tasks()` filters out optimistic tasks and no
    // server-driven `Action::CreateTask` ever upgrades the root.
    let task_id = params
        .root_task_id
        .clone()
        .or_else(|| tasks.last().map(|t| t.id.clone()));

    // Emit CreateTask only on the very first turn — when no server-created
    // tasks exist yet (compute_active_tasks() returned empty). On subsequent
    // turns the optimistic root has already been upgraded; emitting CreateTask
    // again triggers UnexpectedUpgrade and corrupts the task store.
    let needs_create_task = tasks.is_empty();

    // Tool-call results never make it into `task.messages` for local-provider
    // conversations — the controller carries them through `request.input` as
    // `AIAgentInput::ActionResult` instead. Pull them out here so the request
    // translator can pair each assistant `tool_calls` entry with a matching
    // `role:"tool"` follower (OpenAI rejects with HTTP 400 otherwise).
    let action_results = collect_action_results(&params.input);

    let input = local_provider::request::LocalProviderInput {
        user_query,
        tasks,
        supported_tools,
        conversation_id,
        task_id,
        needs_create_task,
        action_results,
        compaction_config: params.local_provider_compaction_config.clone(),
        compaction_state: params.local_provider_compaction_state.clone(),
    };

    let http = reqwest::Client::new();
    match local_provider::run_chat_turn(input, cfg, cancellation_rx, http).await {
        Ok(stream) => {
            // The local-provider stream yields `ResponseEvent` directly (errors
            // are encoded as `Finished{InternalError}` events). Wrap each event
            // in `Ok(...)` so the type matches `ResponseStream`.
            let wrapped = stream.map(Ok::<_, Arc<AIApiError>>);
            Ok(Box::pin(wrapped))
        }
        Err(e) => {
            let (tx, rx) = async_channel::unbounded();
            let err = AIApiError::Other(anyhow::Error::from(e));
            let _ = tx.send(Err(Arc::new(err))).await;
            Ok(Box::pin(rx))
        }
    }
}

/// Walk the AIAgentInput list in reverse and return the most recent UserQuery
/// query string, if any. Used by the local-provider routing to pick the latest
/// turn the user typed.
fn extract_latest_user_query(input: &[crate::ai::agent::AIAgentInput]) -> Option<String> {
    for entry in input.iter().rev() {
        if let crate::ai::agent::AIAgentInput::UserQuery { query, .. } = entry {
            return Some(query.clone());
        }
    }
    None
}

/// Build a `tool_call_id -> rendered_result` map from the request's
/// `AIAgentInput::ActionResult` entries. The action id is the same string the
/// model used as the `tool_call_id` when it issued the call, so the request
/// translator can use this map to splice in the missing `role:"tool"` messages.
fn collect_action_results(
    input: &[crate::ai::agent::AIAgentInput],
) -> std::collections::HashMap<String, String> {
    input
        .iter()
        .filter_map(|entry| match entry {
            crate::ai::agent::AIAgentInput::ActionResult { result, .. } => {
                Some((result.id.to_string(), format!("{}", result.result)))
            }
            _ => None,
        })
        .collect()
}

fn get_supported_tools(params: &RequestParams) -> Vec<api::ToolType> {
    let mut supported_tools = vec![
        api::ToolType::Grep,
        api::ToolType::FileGlob,
        api::ToolType::FileGlobV2,
        api::ToolType::ReadMcpResource,
        api::ToolType::CallMcpTool,
        api::ToolType::InitProject,
        api::ToolType::OpenCodeReview,
        api::ToolType::RunShellCommand,
        api::ToolType::SuggestNewConversation,
        api::ToolType::Subagent,
        api::ToolType::WriteToLongRunningShellCommand,
        api::ToolType::ReadShellCommandOutput,
        api::ToolType::ReadDocuments,
        api::ToolType::CreateDocuments,
        api::ToolType::EditDocuments,
        api::ToolType::SuggestPrompt,
    ];

    if FeatureFlag::ConversationsAsContext.is_enabled() {
        supported_tools.push(api::ToolType::FetchConversation);
    }

    match params.session_context.session_type() {
        None | Some(SessionType::Local) => {
            supported_tools.extend(&[
                api::ToolType::ReadFiles,
                api::ToolType::ApplyFileDiffs,
                api::ToolType::SearchCodebase,
            ]);

            if FeatureFlag::ArtifactCommand.is_enabled() {
                supported_tools.push(api::ToolType::UploadFileArtifact);
            }
        }
        Some(SessionType::WarpifiedRemote { host_id: Some(_) }) => {
            // Remote session with a known host — enable tools that route
            // through RemoteServerClient. The host_id is only populated
            // after a successful connection handshake, so its presence is a
            // sufficient proxy for client availability.
            // SearchCodebase remains disabled (follow-up work).
            supported_tools.extend(&[api::ToolType::ReadFiles, api::ToolType::ApplyFileDiffs]);
        }
        Some(SessionType::WarpifiedRemote { host_id: None }) => {
            // Feature flag off or not yet connected — no remote tools.
        }
    }

    if FeatureFlag::AgentModeComputerUse.is_enabled() && params.computer_use_enabled {
        supported_tools.extend(&[api::ToolType::UseComputer]);
        supported_tools.extend(&[api::ToolType::RequestComputerUse])
    }

    if FeatureFlag::PRCommentsSlashCommand.is_enabled() {
        supported_tools.push(api::ToolType::InsertReviewComments);
    }

    if FeatureFlag::ListSkills.is_enabled() {
        supported_tools.push(api::ToolType::ReadSkill);
    }

    if params.orchestration_enabled {
        // Always advertise the legacy start-agent tool so the server
        // can fall back to it when its own orchestrate flag is off.
        // When RunAgents is also enabled, advertise it alongside.
        supported_tools.push(if FeatureFlag::OrchestrationV2.is_enabled() {
            api::ToolType::StartAgentV2
        } else {
            api::ToolType::StartAgent
        });
        if FeatureFlag::RunAgentsTool.is_enabled() && FeatureFlag::OrchestrationV2.is_enabled() {
            supported_tools.push(api::ToolType::RunAgents);
        }
        supported_tools.push(api::ToolType::SendMessageToAgent);
    }

    if FeatureFlag::AskUserQuestion.is_enabled() && params.ask_user_question_enabled {
        supported_tools.push(api::ToolType::AskUserQuestion);
    }

    supported_tools
}

fn get_supported_cli_agent_tools(params: &RequestParams) -> Vec<api::ToolType> {
    let mut supported_cli_agent_tools = vec![
        api::ToolType::WriteToLongRunningShellCommand,
        api::ToolType::ReadShellCommandOutput,
        api::ToolType::Grep,
        api::ToolType::FileGlob,
        api::ToolType::FileGlobV2,
    ];

    if FeatureFlag::TransferControlTool.is_enabled() {
        supported_cli_agent_tools.push(api::ToolType::TransferShellCommandControlToUser);
    }

    match params.session_context.session_type() {
        None | Some(SessionType::Local) => {
            supported_cli_agent_tools
                .extend(&[api::ToolType::ReadFiles, api::ToolType::SearchCodebase]);
        }
        Some(SessionType::WarpifiedRemote { host_id: Some(_) }) => {
            supported_cli_agent_tools.push(api::ToolType::ReadFiles);
        }
        Some(SessionType::WarpifiedRemote { host_id: None }) => {}
    }

    supported_cli_agent_tools
}

#[cfg(test)]
#[path = "impl_tests.rs"]
mod tests;
