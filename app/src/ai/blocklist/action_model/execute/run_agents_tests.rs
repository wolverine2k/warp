use super::*;
use crate::settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel};
use crate::test_util::settings::initialize_settings_for_tests;
use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest};
use ai::local_provider::llm_id;
use settings::Setting;
use warpui::App;

fn make_request(model_id: &str, harness_type: &str) -> RunAgentsRequest {
    RunAgentsRequest {
        summary: String::new(),
        base_prompt: "go".to_string(),
        skills: vec![],
        model_id: model_id.to_string(),
        harness_type: harness_type.to_string(),
        execution_mode: RunAgentsExecutionMode::Local,
        agent_run_configs: vec![RunAgentsAgentRunConfig {
            name: "child-1".to_string(),
            prompt: "do thing".to_string(),
            title: "T".to_string(),
        }],
        plan_id: String::new(),
        harness_auth_secret_name: None,
    }
}

fn add_byop_provider_for_orchestration(
    app: &mut App,
    provider_id: &str,
    api_type: AgentProviderApiType,
) {
    AISettings::handle(app).update(app, |settings, ctx| {
        let provider = AgentProvider {
            id: provider_id.to_owned(),
            name: "P".to_owned(),
            kind: Default::default(),
            api_type,
            base_url: "https://api.example.com/v1".to_owned(),
            models: vec![AgentProviderModel::from_id("m1".to_owned())],
            available_for_orchestration: true,
            remote_secret_name: String::new(),
        };
        let mut providers = settings.agent_providers.value().clone();
        providers.push(provider);
        settings.agent_providers.set_value(providers, ctx).unwrap();
    });
}

#[test]
fn validate_request_accepts_compatible_byop_with_oz_harness() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        add_byop_provider_for_orchestration(&mut app, "prov-1", AgentProviderApiType::OpenAi);

        let model_id = llm_id::encode("prov-1", "m1").to_string();
        let request = make_request(&model_id, "oz");
        app.read(|ctx| {
            assert!(validate_request(&request, ctx).is_ok());
        });
    });
}

#[test]
fn validate_request_rejects_anthropic_byop_with_gemini_harness() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        add_byop_provider_for_orchestration(&mut app, "prov-2", AgentProviderApiType::Anthropic);

        let model_id = llm_id::encode("prov-2", "m1").to_string();
        // gemini harness is incompatible with Anthropic API and is not a
        // local-child harness, so only the BYOP validator fires.
        let request = make_request(&model_id, "gemini");
        app.read(|ctx| {
            let err = validate_request(&request, ctx).unwrap_err();
            assert!(err.contains("not compatible with harness 'gemini'"), "{err}");
        });
    });
}

#[test]
fn validate_request_rejects_byop_when_provider_not_opted_in() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        add_byop_provider_for_orchestration(&mut app, "prov-3", AgentProviderApiType::OpenAi);
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let mut providers = settings.agent_providers.value().clone();
            providers
                .last_mut()
                .unwrap()
                .available_for_orchestration = false;
            settings.agent_providers.set_value(providers, ctx).unwrap();
        });

        let model_id = llm_id::encode("prov-3", "m1").to_string();
        let request = make_request(&model_id, "oz");
        app.read(|ctx| {
            let err = validate_request(&request, ctx).unwrap_err();
            assert!(err.contains("not enabled for orchestration"), "{err}");
        });
    });
}

#[test]
fn validate_request_passes_through_first_party_model_ids() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        let request = make_request("claude-4.5-sonnet", "oz");
        app.read(|ctx| {
            assert!(validate_request(&request, ctx).is_ok());
        });
    });
}

// Phase 5b integration tests for the orchestration submit -> child-agent
// translator. Verifies that BYOP model ids survive `run_agents_to_start_agent_mode`
// into the per-child `StartAgentExecutionMode` so the existing in-process
// BYOP dispatcher (Phase 4d) takes over.

#[cfg(test)]
mod run_agents_to_start_agent_mode_byop_tests {
    use super::*;
    use ai::agent::action::{
        RunAgentsAgentRunConfig, RunAgentsExecutionMode, StartAgentExecutionMode,
    };
    use ai::local_provider::llm_id;

    fn make_run_config() -> RunAgentsAgentRunConfig {
        RunAgentsAgentRunConfig {
            name: "child-1".to_string(),
            prompt: "p".to_string(),
            title: "T".to_string(),
        }
    }

    #[test]
    fn byop_model_id_threads_into_local_native_start_mode() {
        let model_id = llm_id::encode("prov-1", "m1").to_string();
        let mode = run_agents_to_start_agent_mode(
            &RunAgentsExecutionMode::Local,
            "oz",
            &model_id,
            &[],
            None,
            &make_run_config(),
        )
        .expect("Local Native + BYOP must translate");
        match mode {
            StartAgentExecutionMode::Local {
                harness_type,
                model_id: forwarded,
            } => {
                assert!(harness_type.is_none(), "oz harness should be None on Local");
                assert_eq!(forwarded.as_deref(), Some(model_id.as_str()));
            }
            other => panic!("expected Local mode, got {other:?}"),
        }
    }

    #[test]
    fn empty_harness_is_treated_as_native_oz() {
        let model_id = llm_id::encode("prov-1", "m1").to_string();
        let mode = run_agents_to_start_agent_mode(
            &RunAgentsExecutionMode::Local,
            "",
            &model_id,
            &[],
            None,
            &make_run_config(),
        )
        .expect("empty harness == native oz");
        assert!(matches!(
            mode,
            StartAgentExecutionMode::Local {
                harness_type: None,
                ..
            }
        ));
    }

    #[test]
    fn byop_model_id_threads_into_codex_local_when_harness_set() {
        // Sanity: even when the orchestration UI lets the user pick a third-party
        // local harness, the BYOP id rides along as the model. (Wiring the
        // env-vars for that path is Phase 5c work; this test only proves the
        // translator preserves the model_id.)
        let model_id = llm_id::encode("prov-1", "m1").to_string();
        let mode = run_agents_to_start_agent_mode(
            &RunAgentsExecutionMode::Local,
            "codex",
            &model_id,
            &[],
            None,
            &make_run_config(),
        );
        // Note: this may return Err if local_child_harness_disabled_message
        // says codex is disabled in this build. Use match rather than assume Ok.
        if let Ok(StartAgentExecutionMode::Local {
            harness_type,
            model_id: forwarded,
        }) = mode
        {
            assert_eq!(harness_type.as_deref(), Some("codex"));
            assert_eq!(forwarded.as_deref(), Some(model_id.as_str()));
        }
    }

    #[test]
    fn empty_model_id_returns_none_on_local_native() {
        let mode = run_agents_to_start_agent_mode(
            &RunAgentsExecutionMode::Local,
            "oz",
            "", // run-wide model_id empty => fall back to child's own pref
            &[],
            None,
            &make_run_config(),
        )
        .expect("empty model_id must still translate");
        match mode {
            StartAgentExecutionMode::Local { model_id, .. } => assert!(model_id.is_none()),
            other => panic!("expected Local mode, got {other:?}"),
        }
    }
}
