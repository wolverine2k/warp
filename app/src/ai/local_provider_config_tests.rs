use ai::local_provider::{llm_id, AgentProviderApiType, AgentProviderSecrets};
use settings::Setting;
use warp_core::features::FeatureFlag;
use warpui::{App, SingletonEntity};

use super::{is_local_provider_llm_id, snapshot_for_request};
use crate::settings::{AISettings, AgentProvider, AgentProviderKind, AgentProviderModel};
use crate::test_util::settings::initialize_settings_for_tests;

#[test]
fn snapshot_for_request_resolves_keyless_ollama_byop_to_local_config() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::LocalLlmProvider.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .agent_providers
                .set_value(
                    vec![AgentProvider {
                        id: "ollama-local".to_string(),
                        name: "Local Ollama".to_string(),
                        kind: AgentProviderKind::default(),
                        api_type: AgentProviderApiType::Ollama,
                        base_url: "http://localhost:11434".to_string(),
                        models: vec![AgentProviderModel::from_id("llama3.2".to_string())],
                        available_for_orchestration: true,
                        remote_secret_name: String::new(),
                    }],
                    ctx,
                )
                .unwrap();
        });

        let model_id = llm_id::encode("ollama-local", "llama3.2");
        let cfg = app
            .read(|ctx| snapshot_for_request(ctx, &model_id))
            .expect("keyless Ollama BYOP should still route locally");

        assert_eq!(cfg.api_type, AgentProviderApiType::Ollama);
        assert_eq!(cfg.base_url, "http://localhost:11434");
        assert_eq!(cfg.model_id, "llama3.2");
        assert_eq!(cfg.api_key, None);
    });
}

#[test]
fn is_local_provider_llm_id_matches_legacy_local_and_byop_ids() {
    let byop_id = llm_id::encode("provider-1", "model-a");

    assert!(is_local_provider_llm_id(&"local:model-a".into()));
    assert!(is_local_provider_llm_id(&byop_id));
    assert!(!is_local_provider_llm_id(&"claude-sonnet-4".into()));
}
