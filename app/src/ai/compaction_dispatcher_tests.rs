use ai::local_provider::{AgentProviderApiType, AgentProviderSecrets};
use settings::Setting;
use warpui::{App, SingletonEntity};

use crate::ai::compaction_dispatcher::CompactionDispatcher;
use crate::settings::ai::AISettings;
use crate::test_util::settings::initialize_settings_for_tests;

fn make_primary_cfg() -> ai::local_provider::LocalProviderConfig {
    ai::local_provider::LocalProviderConfig {
        display_name: "Primary".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "test-model".into(),
        api_key: None,
        supports_tools: true,
        context_window: Some(128_000),
        api_type: AgentProviderApiType::OpenAi,
    }
}

#[test]
fn resolve_target_empty_settings_returns_same_model() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        let primary_cfg = make_primary_cfg();

        app.read(|ctx| {
            let target = CompactionDispatcher::resolve_target(ctx, &primary_cfg);
            assert_eq!(
                target.primary_cfg, target.summarizer_cfg,
                "empty settings should yield same_model (primary == summarizer)"
            );
            assert_eq!(target.primary_cfg, primary_cfg);
        });
    });
}

#[test]
fn compaction_model_available_true_when_settings_empty() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        app.read(|ctx| {
            assert!(
                CompactionDispatcher::compaction_model_available(ctx),
                "no dedicated model configured — should report available"
            );
        });
    });
}

#[test]
fn resolve_target_missing_provider_falls_back_to_same_model() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .byop_compaction_model_provider_id
                .set_value("nonexistent-uuid".to_string(), ctx)
                .unwrap();
            settings
                .byop_compaction_model_id
                .set_value("nonexistent-model".to_string(), ctx)
                .unwrap();
        });

        let primary_cfg = make_primary_cfg();

        app.read(|ctx| {
            let target = CompactionDispatcher::resolve_target(ctx, &primary_cfg);
            assert_eq!(
                target.primary_cfg, target.summarizer_cfg,
                "missing provider should fall back to same_model"
            );
            assert_eq!(target.primary_cfg, primary_cfg);
        });
    });
}

#[test]
fn compaction_model_available_false_when_provider_missing() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .byop_compaction_model_provider_id
                .set_value("nonexistent-uuid".to_string(), ctx)
                .unwrap();
            settings
                .byop_compaction_model_id
                .set_value("nonexistent-model".to_string(), ctx)
                .unwrap();
        });

        app.read(|ctx| {
            assert!(
                !CompactionDispatcher::compaction_model_available(ctx),
                "non-existent provider should report unavailable"
            );
        });
    });
}
