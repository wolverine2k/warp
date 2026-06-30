use std::sync::Once;

use super::*;
use crate::local_provider::compaction::config::CompactionTarget;
use crate::local_provider::compaction::CompactionConfig;
use crate::local_provider::config::LocalProviderConfig;

/// reqwest's default rustls feature requires a crypto provider before
/// any TLS use. Installing it here lets these unit tests construct a
/// `reqwest::Client` without panicking — even though the Skipped paths
/// never actually call out to the network.
fn init_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Test".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "test-model".into(),
        api_key: None,
        supports_tools: true,
        // Tiny context window so even a single small turn overflows.
        context_window: Some(64),
        api_type: crate::local_provider::AgentProviderApiType::OpenAi,
    }
}

fn user_msg(id: &str, q: &str) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: q.into(),
            ..Default::default()
        })),
        ..Default::default()
    }
}

#[tokio::test]
async fn skipped_when_auto_disabled() {
    init_crypto_provider();
    let mut state = CompactionState::default();
    let compaction_cfg = CompactionConfig {
        auto: false,
        ..CompactionConfig::default()
    };
    let messages: Vec<api::Message> = vec![user_msg("u1", "hi")];
    let http = reqwest::Client::new();
    let r = try_compact(
        &messages,
        &mut state,
        &CompactionTarget::same_model(cfg()),
        &compaction_cfg,
        TokenCounts {
            total: 1_000_000,
            ..Default::default()
        },
        false, // manual
        &http,
    )
    .await
    .expect("ok");
    assert!(matches!(r, AutoCompactionOutcome::Skipped));
    assert!(state.completed().is_empty());
}

#[tokio::test]
async fn skipped_when_below_overflow_threshold() {
    init_crypto_provider();
    let mut state = CompactionState::default();
    let compaction_cfg = CompactionConfig::default();
    let messages: Vec<api::Message> = vec![user_msg("u1", "hi")];
    let mut large_window_cfg = cfg();
    large_window_cfg.context_window = Some(200_000);
    let http = reqwest::Client::new();
    let r = try_compact(
        &messages,
        &mut state,
        &CompactionTarget::same_model(large_window_cfg),
        &compaction_cfg,
        TokenCounts {
            total: 100, // way under usable budget
            ..Default::default()
        },
        false, // manual
        &http,
    )
    .await
    .expect("ok");
    assert!(matches!(r, AutoCompactionOutcome::Skipped));
    assert!(state.completed().is_empty());
}

// The "happy path" case where try_compact actually fires the
// summarizer is exercised in `crates/ai/tests/local_provider_integration.rs`
// (auto_compaction_round_trip), which boots a JSON mock server.

#[test]
fn compaction_target_same_model_has_identical_configs() {
    let cfg = LocalProviderConfig {
        display_name: "Test".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "test-model".into(),
        api_key: None,
        supports_tools: true,
        context_window: Some(128_000),
        api_type: crate::local_provider::AgentProviderApiType::OpenAi,
    };
    let target = CompactionTarget::same_model(cfg.clone());
    assert_eq!(target.primary_cfg, cfg);
    assert_eq!(target.summarizer_cfg, cfg);
}

#[test]
fn compaction_target_split_has_different_configs() {
    let primary = LocalProviderConfig {
        display_name: "Primary".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "big-model".into(),
        api_key: None,
        supports_tools: true,
        context_window: Some(128_000),
        api_type: crate::local_provider::AgentProviderApiType::OpenAi,
    };
    let summarizer = LocalProviderConfig {
        display_name: "Summarizer".into(),
        base_url: "http://127.0.0.1:2/v1".into(),
        model_id: "small-model".into(),
        api_key: None,
        supports_tools: false,
        context_window: Some(32_000),
        api_type: crate::local_provider::AgentProviderApiType::Ollama,
    };
    let target = CompactionTarget {
        primary_cfg: primary.clone(),
        summarizer_cfg: summarizer.clone(),
    };
    assert_eq!(target.primary_cfg.model_id, "big-model");
    assert_eq!(target.summarizer_cfg.model_id, "small-model");
    assert_ne!(target.primary_cfg, target.summarizer_cfg);
}
