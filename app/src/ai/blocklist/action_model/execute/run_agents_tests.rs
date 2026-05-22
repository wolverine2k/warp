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
