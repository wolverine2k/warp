use super::*;

/// Test helper: extract the plain text from a `ChatMessageContent::Text`
/// variant, mirroring the old `Option<String>::as_deref()` call sites.
fn content_text(msg: &ChatMessage) -> Option<&str> {
    msg.content.as_ref().and_then(|c| c.as_text())
}

fn cfg() -> LocalProviderConfig {
    LocalProviderConfig {
        display_name: "Local".into(),
        base_url: "http://localhost:11434/v1".into(),
        model_id: "llama3.1".into(),
        api_key: None,
        supports_tools: true,
        context_window: None,
        api_type: crate::local_provider::AgentProviderApiType::OpenAi,
    }
}

fn empty_input() -> LocalProviderInput {
    LocalProviderInput {
        user_query: None,
        tasks: vec![],
        supported_tools: vec![],
        ..Default::default()
    }
}

#[test]
fn always_emits_system_message_first() {
    let req = compose_chat_completion_request(&empty_input(), &cfg());
    assert_eq!(req.messages.len(), 1);
    assert!(matches!(req.messages[0].role, Role::System));
}

#[test]
fn appends_user_query_when_set() {
    let mut input = empty_input();
    input.user_query = Some("hi".into());
    let req = compose_chat_completion_request(&input, &cfg());
    assert_eq!(req.messages.len(), 2);
    assert!(matches!(req.messages[1].role, Role::User));
    assert_eq!(content_text(&req.messages[1]), Some("hi"));
}

#[test]
fn model_field_is_user_id_not_synthetic() {
    let req = compose_chat_completion_request(&empty_input(), &cfg());
    assert_eq!(req.model, "llama3.1");
    assert!(!req.model.starts_with("local:"));
}

#[test]
fn stream_is_always_true() {
    let req = compose_chat_completion_request(&empty_input(), &cfg());
    assert!(req.stream);
}

#[test]
fn tools_present_when_supported_and_v1_listed() {
    let mut input = empty_input();
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_chat_completion_request(&input, &cfg());
    let tools = req.tools.as_ref().expect("tools present");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].function.name, "read_files");
    assert!(matches!(req.tool_choice, Some(ToolChoice::Auto)));
}

#[test]
fn tools_absent_when_supports_tools_false() {
    let mut config = cfg();
    config.supports_tools = false;
    let mut input = empty_input();
    input.supported_tools = vec![api::ToolType::ReadFiles];
    let req = compose_chat_completion_request(&input, &config);
    assert!(req.tools.is_none());
    assert!(req.tool_choice.is_none());
}

#[test]
fn tools_absent_when_no_v1_tools_signaled() {
    let mut input = empty_input();
    // None of these are in the v1 curated set
    input.supported_tools = vec![api::ToolType::SearchCodebase, api::ToolType::CallMcpTool];
    let req = compose_chat_completion_request(&input, &cfg());
    assert!(req.tools.is_none());
}

#[test]
fn history_walks_in_proto_order() {
    // Build one task with three messages: user_query, agent_output, user_query.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            api::Message {
                id: "m1".into(),
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "first".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            api::Message {
                id: "m2".into(),
                message: Some(api::message::Message::AgentOutput(
                    api::message::AgentOutput { text: "ok".into() },
                )),
                ..Default::default()
            },
            api::Message {
                id: "m3".into(),
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "second".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        user_query: None,
        tasks: vec![task],
        supported_tools: vec![],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // system + 3 history messages
    assert_eq!(req.messages.len(), 4);
    assert!(matches!(req.messages[0].role, Role::System));
    assert!(matches!(req.messages[1].role, Role::User));
    assert_eq!(content_text(&req.messages[1]), Some("first"));
    assert!(matches!(req.messages[2].role, Role::Assistant));
    assert_eq!(content_text(&req.messages[2]), Some("ok"));
    assert!(matches!(req.messages[3].role, Role::User));
    assert_eq!(content_text(&req.messages[3]), Some("second"));
}

#[test]
fn reasoning_messages_dropped_from_history() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::AgentReasoning(
                api::message::AgentReasoning {
                    reasoning: "hidden".into(),
                    finished_duration: None,
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let input = LocalProviderInput {
        user_query: None,
        tasks: vec![task],
        supported_tools: vec![],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    assert_eq!(req.messages.len(), 1, "only system; reasoning is dropped");
}

#[test]
fn tool_call_history_translated_to_openai_format() {
    let tool = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
        files: vec![api::message::tool_call::read_files::File {
            name: "src/main.rs".into(),
            line_ranges: vec![],
        }],
    });
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                tool_call_id: "call_xyz".into(),
                tool: Some(tool),
            })),
            ..Default::default()
        }],
        ..Default::default()
    };
    let input = LocalProviderInput {
        user_query: None,
        tasks: vec![task],
        supported_tools: vec![],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // system + assistant(tool_calls) + synthetic tool message backfilled by
    // `backfill_orphaned_tool_calls` so the upstream's strict-ordering
    // validator doesn't reject the request with HTTP 400.
    assert_eq!(req.messages.len(), 3);
    let assistant = &req.messages[1];
    assert!(matches!(assistant.role, Role::Assistant));
    assert!(
        assistant.content.is_none(),
        "tool-call assistant has no text content"
    );
    let tcs = assistant.tool_calls.as_ref().expect("tool_calls present");
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].id, "call_xyz");
    assert_eq!(tcs[0].function.name, "read_files");
    assert!(tcs[0].function.arguments.contains("src/main.rs"));
    let tool_followup = &req.messages[2];
    assert!(matches!(tool_followup.role, Role::Tool));
    assert_eq!(tool_followup.tool_call_id.as_deref(), Some("call_xyz"));
}

#[test]
fn orphaned_tool_call_gets_backfilled_from_action_results() {
    let tool = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
        files: vec![api::message::tool_call::read_files::File {
            name: "Cargo.toml".into(),
            line_ranges: vec![],
        }],
    });
    let task = api::Task {
        id: "t1".into(),
        messages: vec![api::Message {
            id: "m1".into(),
            message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                tool_call_id: "call_real".into(),
                tool: Some(tool),
            })),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut action_results = std::collections::HashMap::new();
    action_results.insert(
        "call_real".to_string(),
        "[package]\nname = \"foo\"".to_string(),
    );
    let input = LocalProviderInput {
        user_query: Some("what's in Cargo.toml?".into()),
        tasks: vec![task],
        supported_tools: vec![],
        action_results,
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // system + assistant(tool_calls) + tool(result) + user
    assert_eq!(req.messages.len(), 4);
    let tool_msg = &req.messages[2];
    assert!(matches!(tool_msg.role, Role::Tool));
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_real"));
    assert_eq!(content_text(tool_msg), Some("[package]\nname = \"foo\""));
}

#[test]
fn context_window_threads_into_prompt() {
    let mut config = cfg();
    config.context_window = Some(4096);
    let req = compose_chat_completion_request(&empty_input(), &config);
    let sys_content = content_text(&req.messages[0]).unwrap();
    assert!(sys_content.contains("4096"));
}

// ---- summarize_tool_result ----

fn tool_result(inner: api::message::tool_call_result::Result) -> api::message::ToolCallResult {
    api::message::ToolCallResult {
        tool_call_id: "tc_1".into(),
        result: Some(inner),
        ..Default::default()
    }
}

#[test]
fn run_shell_command_finished_renders_command_output_and_exit() {
    let r = tool_result(api::message::tool_call_result::Result::RunShellCommand(
        api::RunShellCommandResult {
            command: "ls -la".into(),
            result: Some(api::run_shell_command_result::Result::CommandFinished(
                api::ShellCommandFinished {
                    output: "total 0\ndrwx 1 user 0 .\n".into(),
                    exit_code: 0,
                    ..Default::default()
                },
            )),
            ..Default::default()
        },
    ));
    let s = summarize_tool_result(&r);
    assert!(s.contains("$ ls -la"));
    assert!(s.contains("total 0"));
    assert!(s.contains("[exit 0]"));
}

#[test]
fn read_files_text_success_renders_each_file() {
    let r = tool_result(api::message::tool_call_result::Result::ReadFiles(
        api::ReadFilesResult {
            result: Some(api::read_files_result::Result::TextFilesSuccess(
                api::read_files_result::TextFilesSuccess {
                    files: vec![
                        api::FileContent {
                            file_path: "src/main.rs".into(),
                            content: "fn main() {}".into(),
                            ..Default::default()
                        },
                        api::FileContent {
                            file_path: "Cargo.toml".into(),
                            content: "[package]\nname = \"foo\"".into(),
                            ..Default::default()
                        },
                    ],
                },
            )),
        },
    ));
    let s = summarize_tool_result(&r);
    assert!(s.contains("--- src/main.rs ---"));
    assert!(s.contains("fn main() {}"));
    assert!(s.contains("--- Cargo.toml ---"));
}

#[test]
fn read_files_error_renders_message() {
    let r = tool_result(api::message::tool_call_result::Result::ReadFiles(
        api::ReadFilesResult {
            result: Some(api::read_files_result::Result::Error(
                api::read_files_result::Error {
                    message: "permission denied".into(),
                },
            )),
        },
    ));
    let s = summarize_tool_result(&r);
    assert!(s.contains("read failed"));
    assert!(s.contains("permission denied"));
}

#[test]
fn grep_success_with_matches_renders_paths_and_lines() {
    let r = tool_result(api::message::tool_call_result::Result::Grep(
        api::GrepResult {
            result: Some(api::grep_result::Result::Success(
                api::grep_result::Success {
                    matched_files: vec![api::grep_result::success::GrepFileMatch {
                        file_path: "src/lib.rs".into(),
                        matched_lines: vec![
                            api::grep_result::success::grep_file_match::GrepLineMatch {
                                line_number: 12,
                            },
                            api::grep_result::success::grep_file_match::GrepLineMatch {
                                line_number: 47,
                            },
                        ],
                    }],
                },
            )),
        },
    ));
    let s = summarize_tool_result(&r);
    assert!(s.contains("src/lib.rs"));
    assert!(s.contains("12"));
    assert!(s.contains("47"));
}

#[test]
fn grep_no_matches_says_no_matches() {
    let r = tool_result(api::message::tool_call_result::Result::Grep(
        api::GrepResult {
            result: Some(api::grep_result::Result::Success(
                api::grep_result::Success {
                    matched_files: vec![],
                },
            )),
        },
    ));
    assert_eq!(summarize_tool_result(&r), "<no matches>");
}

#[test]
fn file_glob_v2_renders_paths() {
    let r = tool_result(api::message::tool_call_result::Result::FileGlobV2(
        api::FileGlobV2Result {
            result: Some(api::file_glob_v2_result::Result::Success(
                api::file_glob_v2_result::Success {
                    matched_files: vec![
                        api::file_glob_v2_result::success::FileGlobMatch {
                            file_path: "a.rs".into(),
                        },
                        api::file_glob_v2_result::success::FileGlobMatch {
                            file_path: "b.rs".into(),
                        },
                    ],
                    warnings: String::new(),
                },
            )),
        },
    ));
    let s = summarize_tool_result(&r);
    assert!(s.contains("a.rs"));
    assert!(s.contains("b.rs"));
}

#[test]
fn apply_file_diffs_success_lists_updates_and_deletes() {
    let r = tool_result(api::message::tool_call_result::Result::ApplyFileDiffs(
        api::ApplyFileDiffsResult {
            result: Some(api::apply_file_diffs_result::Result::Success(
                api::apply_file_diffs_result::Success {
                    updated_files_v2: vec![
                        api::apply_file_diffs_result::success::UpdatedFileContent {
                            file: Some(api::FileContent {
                                file_path: "edited.rs".into(),
                                ..Default::default()
                            }),
                            was_edited_by_user: false,
                        },
                    ],
                    deleted_files: vec![api::apply_file_diffs_result::success::DeletedFile {
                        file_path: "removed.rs".into(),
                    }],
                    ..Default::default()
                },
            )),
        },
    ));
    let s = summarize_tool_result(&r);
    assert!(s.contains("updated: edited.rs"));
    assert!(s.contains("deleted: removed.rs"));
}

#[test]
fn cancel_result_renders_clearly() {
    let r = tool_result(api::message::tool_call_result::Result::Cancel(()));
    let s = summarize_tool_result(&r);
    assert!(s.to_lowercase().contains("cancel"));
}

#[test]
fn empty_result_is_handled_safely() {
    let r = api::message::ToolCallResult {
        tool_call_id: "tc_x".into(),
        result: None,
        ..Default::default()
    };
    let s = summarize_tool_result(&r);
    assert!(s.contains("empty"));
}

// ---- Phase B-3 projection ----

use crate::local_provider::compaction::{CompactionState, CompletedCompaction};

fn user_msg(id: &str, body: &str) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: body.into(),
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn agent_msg(id: &str, body: &str) -> api::Message {
    api::Message {
        id: id.into(),
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput { text: body.into() },
        )),
        ..Default::default()
    }
}

#[test]
fn projection_no_op_when_compaction_state_empty() {
    let task = api::Task {
        id: "t1".into(),
        messages: vec![user_msg("u1", "hi"), agent_msg("a1", "hello")],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // system + user + assistant
    assert_eq!(req.messages.len(), 3);
}

#[test]
fn projection_synthesizes_head_and_drops_pre_tail_history() {
    // The synthetic compaction pair is NOT in `tasks` — the projection
    // synthesizes it from `compaction_state`.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            user_msg("u_old1", "old turn 1"),
            agent_msg("a_old1", "old reply 1"),
            user_msg("u_old2", "old turn 2"),
            agent_msg("a_old2", "old reply 2"),
            user_msg("u_new", "post-compact ask"),
        ],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "compaction-trigger-X".into(),
        assistant_msg_id: "compaction-summary-X".into(),
        tail_start_id: Some("u_new".into()),
        summary_text: Some("## Goal\n- summary".into()),
        auto: true,
        overflow: true,
    });
    let input = LocalProviderInput {
        tasks: vec![task],
        compaction_state: state,
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());

    // Expect: system + synthetic continue user + synthetic summary
    // assistant + tail user. The four pre-tail messages are dropped.
    assert_eq!(
        req.messages.len(),
        4,
        "wrong msg count: {:?}",
        req.messages
            .iter()
            .map(|m| (m.role, m.content.clone()))
            .collect::<Vec<_>>()
    );
    // Continue prompt has the overflow=true preamble.
    assert!(content_text(&req.messages[1]).unwrap().contains("Continue"));
    assert_eq!(content_text(&req.messages[2]), Some("## Goal\n- summary"));
    assert_eq!(content_text(&req.messages[3]), Some("post-compact ask"));
}

#[test]
fn projection_drops_all_history_when_tail_start_id_is_none() {
    // Manual `/compact` with no preserved tail: the synthetic pair is
    // the entire head, every task message gets dropped.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![user_msg("u1", "hi"), agent_msg("a1", "hello")],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "compaction-trigger-Y".into(),
        assistant_msg_id: "compaction-summary-Y".into(),
        tail_start_id: None,
        summary_text: Some("manual digest".into()),
        auto: false,
        overflow: false,
    });
    let input = LocalProviderInput {
        tasks: vec![task],
        compaction_state: state,
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // system + synthetic user + synthetic assistant — that's it.
    assert_eq!(req.messages.len(), 3);
    assert_eq!(content_text(&req.messages[2]), Some("manual digest"));
}

#[test]
fn projection_no_op_when_summary_text_missing() {
    // Defensive: completed entry without cached summary_text. We
    // can't reconstruct the head, so we render the original messages
    // rather than silently lose context.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![user_msg("u1", "hi")],
        ..Default::default()
    };
    let mut state = CompactionState::default();
    state.push_completed(CompletedCompaction {
        user_msg_id: "compaction-trigger-Z".into(),
        assistant_msg_id: "compaction-summary-Z".into(),
        tail_start_id: None,
        summary_text: None,
        auto: true,
        overflow: true,
    });
    let input = LocalProviderInput {
        tasks: vec![task],
        compaction_state: state,
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    assert_eq!(req.messages.len(), 2);
    assert_eq!(content_text(&req.messages[1]), Some("hi"));
}

// ---- Phase B-6 multi-turn agent loop ----

#[test]
fn synthetic_user_query_is_injected_before_anchor_message() {
    // The local-provider path doesn't have a server echoing
    // `Message::UserQuery` back into `task.messages`. The controller
    // surfaces historical user queries as `(anchor_id, query)` pairs;
    // the translator emits `role:"user"` immediately before the
    // anchor's task message during history rendering.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            agent_msg("a_old", "first answer"),
            agent_msg("a_new", "second answer"),
        ],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        // Anchor "a_old" -> "what is X?", anchor "a_new" -> "and Y?"
        synthetic_user_queries: vec![
            ("a_old".into(), "what is X?".into()),
            ("a_new".into(), "and Y?".into()),
        ],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // [system, user("what is X?"), assistant("first answer"),
    //  user("and Y?"), assistant("second answer")]
    assert_eq!(req.messages.len(), 5);
    assert!(matches!(req.messages[0].role, Role::System));
    assert!(matches!(req.messages[1].role, Role::User));
    assert_eq!(content_text(&req.messages[1]), Some("what is X?"));
    assert!(matches!(req.messages[2].role, Role::Assistant));
    assert_eq!(content_text(&req.messages[2]), Some("first answer"));
    assert!(matches!(req.messages[3].role, Role::User));
    assert_eq!(content_text(&req.messages[3]), Some("and Y?"));
    assert!(matches!(req.messages[4].role, Role::Assistant));
    assert_eq!(content_text(&req.messages[4]), Some("second answer"));
}

#[test]
fn synthetic_user_query_with_unmatched_anchor_is_dropped_silently() {
    // Defensive: if the anchor id no longer exists in task.messages
    // (e.g. compaction dropped it), we silently skip emitting the
    // synthetic user message rather than appending it at an arbitrary
    // position. Anchored injection is a hint, not a hard guarantee —
    // the translator must stay correct even when the anchor is gone.
    let task = api::Task {
        id: "t1".into(),
        messages: vec![agent_msg("a1", "answer")],
        ..Default::default()
    };
    let input = LocalProviderInput {
        tasks: vec![task],
        synthetic_user_queries: vec![("missing".into(), "ghost".into())],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    // [system, assistant("answer")] — no ghost user message.
    assert_eq!(req.messages.len(), 2);
    assert!(matches!(req.messages[0].role, Role::System));
    assert!(matches!(req.messages[1].role, Role::Assistant));
    assert_eq!(content_text(&req.messages[1]), Some("answer"));
}

#[test]
fn historical_action_results_resolve_orphan_tool_calls_across_turns() {
    // Simulates a multi-turn agent loop: turn 1 produced a tool call
    // (whose result lives in `action_results`), turn 2 produced
    // another tool call (whose result is also in `action_results`),
    // and turn 3 is the current user query. The translator must pair
    // each historical assistant `tool_calls` entry with its real
    // `role:"tool"` follower instead of the
    // `"(tool result not available)"` placeholder.
    let tool_1 = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
        files: vec![api::message::tool_call::read_files::File {
            name: "Cargo.toml".into(),
            line_ranges: vec![],
        }],
    });
    let tool_2 = api::message::tool_call::Tool::Grep(api::message::tool_call::Grep {
        queries: vec!["fn main".into()],
        path: ".".into(),
    });
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            api::Message {
                id: "m_call_1".into(),
                message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                    tool_call_id: "call_alpha".into(),
                    tool: Some(tool_1),
                })),
                ..Default::default()
            },
            api::Message {
                id: "m_call_2".into(),
                message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                    tool_call_id: "call_beta".into(),
                    tool: Some(tool_2),
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let mut action_results = std::collections::HashMap::new();
    action_results.insert("call_alpha".into(), "[package]\nname = \"foo\"".into());
    action_results.insert("call_beta".into(), "src/main.rs: lines 1\n".into());

    let input = LocalProviderInput {
        user_query: Some("now what?".into()),
        tasks: vec![task],
        supported_tools: vec![api::ToolType::ReadFiles, api::ToolType::Grep],
        action_results,
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());

    // [system, assistant(tool_calls call_alpha), tool(real result for alpha),
    //  assistant(tool_calls call_beta), tool(real result for beta),
    //  user("now what?")]
    assert_eq!(req.messages.len(), 6, "{:#?}", req.messages);
    assert!(matches!(req.messages[1].role, Role::Assistant));
    assert!(matches!(req.messages[2].role, Role::Tool));
    assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_alpha"));
    assert_eq!(
        content_text(&req.messages[2]),
        Some("[package]\nname = \"foo\"")
    );
    assert!(matches!(req.messages[3].role, Role::Assistant));
    assert!(matches!(req.messages[4].role, Role::Tool));
    assert_eq!(req.messages[4].tool_call_id.as_deref(), Some("call_beta"));
    // No placeholder anywhere.
    for m in &req.messages {
        assert_ne!(
            content_text(m),
            Some("(tool result not available)"),
            "placeholder leaked: {:?}",
            m
        );
    }
    assert!(matches!(req.messages[5].role, Role::User));
    assert_eq!(content_text(&req.messages[5]), Some("now what?"));
}

#[test]
fn full_multi_turn_loop_round_trip() {
    // End-to-end shape of a 3-turn local-provider conversation:
    //
    //   Turn 1 — user asks "read Cargo.toml"
    //     assistant emits tool_call call_alpha (ReadFiles)
    //   Turn 2 — controller threads call_alpha's result back
    //     assistant emits tool_call call_beta (Grep)
    //   Turn 3 — controller threads call_beta's result back; new
    //     user query "summarize"
    //
    // The captured-bug scenario from `phase-b-6-multi-turn-agent-loop.md`:
    // 0 user messages, all tool messages "(tool result not available)".
    // After Phase B-6 the request body must contain BOTH historical
    // user messages (anchored) plus the current one, and BOTH tool
    // results' real content.
    let tool_1 = api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
        files: vec![api::message::tool_call::read_files::File {
            name: "Cargo.toml".into(),
            line_ranges: vec![],
        }],
    });
    let tool_2 = api::message::tool_call::Tool::Grep(api::message::tool_call::Grep {
        queries: vec!["fn main".into()],
        path: ".".into(),
    });
    let task = api::Task {
        id: "t1".into(),
        messages: vec![
            api::Message {
                id: "m_t1_call".into(),
                message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                    tool_call_id: "call_alpha".into(),
                    tool: Some(tool_1),
                })),
                ..Default::default()
            },
            api::Message {
                id: "m_t2_call".into(),
                message: Some(api::message::Message::ToolCall(api::message::ToolCall {
                    tool_call_id: "call_beta".into(),
                    tool: Some(tool_2),
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let mut action_results = std::collections::HashMap::new();
    action_results.insert("call_alpha".into(), "Cargo.toml body".into());
    action_results.insert("call_beta".into(), "src/main.rs hits".into());

    let input = LocalProviderInput {
        user_query: Some("summarize".into()),
        tasks: vec![task],
        supported_tools: vec![api::ToolType::ReadFiles, api::ToolType::Grep],
        action_results,
        // Turn-1 user query is anchored to the first task message
        // ("m_t1_call"). Turn-2 had no fresh user query (auto follow-
        // up from action result).
        synthetic_user_queries: vec![("m_t1_call".into(), "read Cargo.toml".into())],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());

    // Expected (in order):
    //   0: system
    //   1: user "read Cargo.toml"
    //   2: assistant tool_calls call_alpha
    //   3: tool call_alpha "Cargo.toml body"
    //   4: assistant tool_calls call_beta
    //   5: tool call_beta "src/main.rs hits"
    //   6: user "summarize"
    assert_eq!(req.messages.len(), 7);
    assert!(matches!(req.messages[0].role, Role::System));
    assert!(matches!(req.messages[1].role, Role::User));
    assert_eq!(content_text(&req.messages[1]), Some("read Cargo.toml"));
    assert!(matches!(req.messages[2].role, Role::Assistant));
    assert!(matches!(req.messages[3].role, Role::Tool));
    assert_eq!(content_text(&req.messages[3]), Some("Cargo.toml body"));
    assert!(matches!(req.messages[4].role, Role::Assistant));
    assert!(matches!(req.messages[5].role, Role::Tool));
    assert_eq!(content_text(&req.messages[5]), Some("src/main.rs hits"));
    assert!(matches!(req.messages[6].role, Role::User));
    assert_eq!(content_text(&req.messages[6]), Some("summarize"));

    // Tools advertised non-null on every multi-turn body.
    let tools = req.tools.expect("tools should be advertised");
    assert!(!tools.is_empty());
}

// ---- Phase 4c-2 attachment tests ----

use crate::attachments::AgentAttachment;

fn png_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![0x89, 0x50, 0x4e, 0x47],
        display_name: Some("test.png".into()),
        thumbnail_bytes: None,
    }
}

#[test]
fn text_only_turn_emits_string_content() {
    let input = LocalProviderInput {
        user_query: Some("hello".into()),
        attachments: Vec::new(),
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    let user_msg = req
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::User))
        .unwrap();
    assert!(
        matches!(&user_msg.content, Some(ChatMessageContent::Text(t)) if t == "hello"),
        "expected Text(\"hello\"), got {:?}",
        user_msg.content
    );
}

#[test]
fn turn_with_image_emits_parts_array() {
    let input = LocalProviderInput {
        user_query: Some("what is this".into()),
        attachments: vec![png_attachment()],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    let user_msg = req
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::User))
        .unwrap();
    let parts = match &user_msg.content {
        Some(ChatMessageContent::Parts(p)) => p,
        other => panic!("expected Parts, got {other:?}"),
    };
    assert_eq!(parts.len(), 2);
    assert!(
        matches!(&parts[0], ChatContentPart::Text { text } if text == "what is this"),
        "unexpected first part: {:?}",
        parts[0]
    );
    assert!(
        matches!(&parts[1], ChatContentPart::ImageUrl { image_url } if image_url.url.starts_with("data:image/png;base64,")),
        "unexpected second part: {:?}",
        parts[1]
    );
}

#[test]
fn pdf_attachment_is_dropped_and_only_text_part_remains() {
    let input = LocalProviderInput {
        user_query: Some("read this".into()),
        attachments: vec![AgentAttachment {
            mime: "application/pdf".into(),
            bytes: vec![1, 2, 3],
            display_name: None,
            thumbnail_bytes: None,
        }],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    let user_msg = req
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::User))
        .unwrap();
    let parts = match &user_msg.content {
        Some(ChatMessageContent::Parts(p)) => p,
        other => panic!("expected Parts, got {other:?}"),
    };
    // PDF is dropped; only the text part remains.
    assert_eq!(parts.len(), 1, "expected 1 part (text only), got {parts:?}");
    assert!(matches!(&parts[0], ChatContentPart::Text { .. }));
}

#[test]
fn empty_user_query_with_image_emits_only_image_part() {
    let input = LocalProviderInput {
        user_query: Some("".into()),
        attachments: vec![png_attachment()],
        ..Default::default()
    };
    let req = compose_chat_completion_request(&input, &cfg());
    let user_msg = req
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::User))
        .unwrap();
    let parts = match &user_msg.content {
        Some(ChatMessageContent::Parts(p)) => p,
        other => panic!("expected Parts, got {other:?}"),
    };
    // Empty text is filtered out; only the image remains.
    assert_eq!(
        parts.len(),
        1,
        "expected 1 part (image only), got {parts:?}"
    );
    assert!(matches!(&parts[0], ChatContentPart::ImageUrl { .. }));
}
