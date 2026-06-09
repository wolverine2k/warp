use ai::local_provider::{llm_id, AgentProviderApiType, AgentProviderSecrets};
use settings::Setting;
use warpui::{App, SingletonEntity};

use crate::settings::ai::AISettings;
use crate::settings::{AgentProvider, AgentProviderKind, AgentProviderModel};
use crate::test_util::settings::initialize_settings_for_tests;

use super::{lookup_byop, resolve_byop_for_local_child, resolve_byop_for_remote_child};

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

#[test]
fn resolve_byop_for_remote_child_returns_anthropic_wire_shape() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-a".to_owned(),
                name: "Anth".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::Anthropic,
                base_url: "https://api.anthropic.example/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("claude-sonnet".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: "byop-prov-a".to_owned(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-a", "sk-test".to_string(), ctx);
        });

        let encoded = llm_id::encode("prov-a", "claude-sonnet").to_string();
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));

        assert_eq!(
            base_url.as_deref(),
            Some("https://api.anthropic.example/v1")
        );
        assert_eq!(api_type.as_deref(), Some("anthropic"));
        assert_eq!(secret_name.as_deref(), Some("byop-prov-a"));
    });
}

#[test]
fn resolve_byop_for_remote_child_omits_secret_when_empty() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            let provider = AgentProvider {
                id: "prov-b".to_owned(),
                name: "OAI".to_owned(),
                kind: AgentProviderKind::default(),
                api_type: AgentProviderApiType::OpenAi,
                base_url: "https://api.openai.example/v1".to_owned(),
                models: vec![AgentProviderModel::from_id("gpt-4o".to_owned())],
                available_for_orchestration: true,
                remote_secret_name: String::new(),
            };
            settings
                .agent_providers
                .set_value(vec![provider], ctx)
                .unwrap();
        });
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("prov-b", "sk-test".to_string(), ctx);
        });

        let encoded = llm_id::encode("prov-b", "gpt-4o").to_string();
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));

        assert!(base_url.is_some());
        assert_eq!(api_type.as_deref(), Some("open_ai"));
        // Empty remote_secret_name → None even though resolution succeeded.
        assert!(secret_name.is_none());
    });
}

#[test]
fn resolve_byop_for_remote_child_returns_all_none_for_non_byop_id() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, "claude-sonnet-4"));

        assert!(base_url.is_none());
        assert!(api_type.is_none());
        assert!(secret_name.is_none());
    });
}

#[test]
fn resolve_byop_for_remote_child_returns_all_none_for_missing_provider() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);

        let encoded = llm_id::encode("missing-prov", "m1").to_string();
        let (base_url, api_type, secret_name) =
            app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));

        assert!(base_url.is_none());
        assert!(api_type.is_none());
        assert!(secret_name.is_none());
    });
}

#[test]
fn resolve_byop_for_remote_child_maps_every_api_type() {
    let cases = [
        (AgentProviderApiType::OpenAi, "open_ai"),
        (AgentProviderApiType::OpenAiResp, "open_ai_resp"),
        (AgentProviderApiType::Anthropic, "anthropic"),
        (AgentProviderApiType::Gemini, "gemini"),
        (AgentProviderApiType::Ollama, "ollama"),
        (AgentProviderApiType::DeepSeek, "deep_seek"),
    ];
    for (api_type, expected) in cases {
        App::test((), |mut app| async move {
            initialize_settings_for_tests(&mut app);
            app.add_singleton_model(AgentProviderSecrets::new);
            AISettings::handle(&app).update(&mut app, |settings, ctx| {
                let provider = AgentProvider {
                    id: "p".to_owned(),
                    name: "P".to_owned(),
                    kind: AgentProviderKind::default(),
                    api_type,
                    base_url: "https://api.example.com".to_owned(),
                    models: vec![AgentProviderModel::from_id("m".to_owned())],
                    available_for_orchestration: true,
                    remote_secret_name: String::new(),
                };
                settings
                    .agent_providers
                    .set_value(vec![provider], ctx)
                    .unwrap();
            });
            AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
                secrets.set("p", "sk".to_string(), ctx);
            });
            let encoded = llm_id::encode("p", "m").to_string();
            let (_base_url, api_type_str, _secret) =
                app.read(|ctx| resolve_byop_for_remote_child(ctx, &encoded));
            assert_eq!(api_type_str.as_deref(), Some(expected));
        });
    }
}
