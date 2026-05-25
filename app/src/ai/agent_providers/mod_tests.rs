use ai::local_provider::{llm_id, AgentProviderApiType, AgentProviderSecrets};
use settings::Setting;
use warpui::{App, SingletonEntity};

use crate::settings::ai::AISettings;
use crate::settings::{AgentProvider, AgentProviderKind, AgentProviderModel};
use crate::test_util::settings::initialize_settings_for_tests;

use super::{lookup_byop, resolve_byop_for_local_child};

#[test]
fn resolve_byop_for_local_child_returns_provider_api_key_and_user_model_id() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-xyz".to_owned(),
                name: "P".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::Anthropic,
                base_url: "https://api.anthropic.example/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("claude-sonnet-4".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-xyz", "sk-test-key".to_string(), ctx);
        });

        let encoded_id = llm_id::encode("prov-xyz", "claude-sonnet-4").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &encoded_id));

        let (provider, api_key, model_id) = resolved.expect("BYOP entry must resolve");
        assert_eq!(provider.id, "prov-xyz");
        assert_eq!(provider.api_type, AgentProviderApiType::Anthropic);
        assert_eq!(api_key, "sk-test-key");
        assert_eq!(model_id, "claude-sonnet-4");
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_for_non_byop_id() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, "claude-sonnet-4"));
        assert!(resolved.is_none());
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_for_missing_provider() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        // No provider added; even a well-formed byop id can't resolve.
        let encoded_id = llm_id::encode("missing-prov", "m1").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &encoded_id));
        assert!(resolved.is_none());
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_when_api_key_missing() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-no-key".to_owned(),
                name: "P".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.example.com/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("m1".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        // No secret set for "prov-no-key".
        let encoded_id = llm_id::encode("prov-no-key", "m1").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &encoded_id));
        assert!(resolved.is_none());
    });
}

#[test]
fn resolve_byop_for_local_child_returns_none_when_api_key_is_empty_string() {
    // Distinct from the "missing-key" path: a secret entry exists but its
    // value is the empty string. The function must reject this so the
    // caller doesn't forward `Authorization: Bearer ` to the upstream.
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-empty-key".to_owned(),
                name: "P".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.example.com/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("m1".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-empty-key", String::new(), ctx);
        });

        let encoded_id = llm_id::encode("prov-empty-key", "m1").to_string();
        let resolved = app.read(|ctx| resolve_byop_for_local_child(ctx, &encoded_id));
        assert!(resolved.is_none());
    });
}

#[test]
fn lookup_byop_returns_none_when_api_key_is_empty_string() {
    // Mirrors resolve_byop_for_local_child_returns_none_when_api_key_is_empty_string
    // but exercises lookup_byop (which takes &LLMId rather than &str). Before
    // Phase 5d, lookup_byop could return Some(provider, "", model_id); the
    // new resolve_byop_inner guard closes that gap.
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-empty-key".to_owned(),
                name: "P".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.example.com/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("m1".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-empty-key", String::new(), ctx);
        });

        let llm_id = llm_id::encode("prov-empty-key", "m1");
        let resolved = app.read(|ctx| lookup_byop(ctx, &llm_id));
        assert!(resolved.is_none(), "empty api_key must produce None");
    });
}
