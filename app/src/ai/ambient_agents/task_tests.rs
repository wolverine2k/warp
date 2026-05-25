use chrono::Utc;

use super::{
    AgentConfigSnapshot, AmbientAgentTask, AmbientAgentTaskState, HarnessAuthSecretsConfig,
    TaskStatusErrorCode, TaskStatusMessage,
};

fn make_task(snapshot_name: Option<&str>, title: &str) -> AmbientAgentTask {
    let now = Utc::now();
    let agent_config_snapshot = snapshot_name.map(|name| AgentConfigSnapshot {
        name: Some(name.to_string()),
        ..Default::default()
    });
    AmbientAgentTask {
        task_id: "11111111-1111-1111-1111-111111111111".parse().unwrap(),
        parent_run_id: None,
        title: title.to_string(),
        state: AmbientAgentTaskState::InProgress,
        prompt: String::new(),
        created_at: now,
        started_at: Some(now),
        updated_at: now,
        status_message: None,
        source: None,
        session_id: None,
        session_link: None,
        creator: None,
        executor: None,
        conversation_id: None,
        request_usage: None,
        is_sandbox_running: false,
        agent_config_snapshot,
        artifacts: vec![],
        last_event_sequence: None,
        children: vec![],
    }
}

#[test]
fn display_name_prefers_agent_config_snapshot_name_over_title() {
    let task = make_task(Some("frontend-tests"), "Long descriptive task title");
    assert_eq!(task.display_name(), "frontend-tests");
}

#[test]
fn display_name_falls_back_to_title_when_snapshot_name_is_missing() {
    let task = make_task(None, "Long descriptive task title");
    assert_eq!(task.display_name(), "Long descriptive task title");
}

#[test]
fn display_name_falls_back_to_title_when_snapshot_name_is_whitespace() {
    let task = make_task(Some("   "), "Long descriptive task title");
    assert_eq!(task.display_name(), "Long descriptive task title");
}

#[test]
fn display_name_returns_literal_agent_when_both_sources_are_empty() {
    let task = make_task(None, "");
    assert_eq!(task.display_name(), "Agent");
}

#[test]
fn display_name_returns_literal_agent_for_whitespace_only_title() {
    let task = make_task(None, "   \t\n  ");
    assert_eq!(task.display_name(), "Agent");
}

#[test]
fn display_name_trims_whitespace_at_each_layer() {
    let task = make_task(Some("  frontend-tests  "), "  Long descriptive title  ");
    assert_eq!(task.display_name(), "frontend-tests");

    let task = make_task(None, "  Long descriptive title  ");
    assert_eq!(task.display_name(), "Long descriptive title");
}

#[test]
fn task_status_error_code_deserializes_public_api_casing() {
    let message: TaskStatusMessage = serde_json::from_str(
        "{\"message\":\"setup failed\",\"error_code\":\"environment_setup_failed\"}",
    )
    .unwrap();

    assert_eq!(
        message.error_code,
        Some(TaskStatusErrorCode::EnvironmentSetupFailed)
    );
    assert!(message.is_environment_setup_failure());
}

#[test]
fn task_status_error_code_deserializes_graphql_casing() {
    let message: TaskStatusMessage = serde_json::from_str(
        "{\"message\":\"setup failed\",\"errorCode\":\"ENVIRONMENT_SETUP_FAILED\"}",
    )
    .unwrap();

    assert_eq!(
        message.error_code,
        Some(TaskStatusErrorCode::EnvironmentSetupFailed)
    );
    assert!(message.is_environment_setup_failure());
}

#[test]
fn task_status_error_code_deserializes_unknown_codes() {
    let message: TaskStatusMessage =
        serde_json::from_str("{\"message\":\"failed\",\"error_code\":\"new_error\"}").unwrap();

    assert_eq!(message.error_code, Some(TaskStatusErrorCode::Unknown));
    assert!(!message.is_environment_setup_failure());
}

#[test]
fn agent_config_snapshot_deserializes_pre_5d_payload() {
    // Simulates a payload from a pre-5d server / client without the
    // BYOP and compaction fields.
    let json = r#"{
        "name": "child-1",
        "model_id": "claude-sonnet-4",
        "harness": {"type": "claude"}
    }"#;
    let snapshot: AgentConfigSnapshot =
        serde_json::from_str(json).expect("should deserialize without 5d fields");
    assert_eq!(snapshot.name.as_deref(), Some("child-1"));
    assert!(snapshot.byop_base_url.is_none());
    assert!(snapshot.byop_api_type.is_none());
    assert!(snapshot.compaction_model_provider_id.is_none());
    assert!(snapshot.compaction_model_id.is_none());
}

#[test]
fn agent_config_snapshot_round_trips_byop_fields() {
    let snapshot = AgentConfigSnapshot {
        model_id: Some("byop:prov:claude-sonnet".to_owned()),
        byop_base_url: Some("https://api.anthropic.example/v1".to_owned()),
        byop_api_type: Some("anthropic".to_owned()),
        compaction_model_provider_id: Some("prov-compact".to_owned()),
        compaction_model_id: Some("haiku".to_owned()),
        harness_auth_secrets: Some(HarnessAuthSecretsConfig {
            byop_auth_secret_name: Some("byop-prov".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: AgentConfigSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(
        restored.byop_base_url.as_deref(),
        Some("https://api.anthropic.example/v1")
    );
    assert_eq!(restored.byop_api_type.as_deref(), Some("anthropic"));
    assert_eq!(
        restored.compaction_model_provider_id.as_deref(),
        Some("prov-compact")
    );
    assert_eq!(restored.compaction_model_id.as_deref(), Some("haiku"));
    assert_eq!(
        restored
            .harness_auth_secrets
            .unwrap()
            .byop_auth_secret_name
            .as_deref(),
        Some("byop-prov")
    );
}

#[test]
fn agent_config_snapshot_is_empty_remains_true_with_default_5d_fields() {
    let snapshot = AgentConfigSnapshot::default();
    assert!(snapshot.is_empty());
}
