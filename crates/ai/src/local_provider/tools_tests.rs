//! Tests for the OpenAI ↔ proto tool-call translation table.
//! Phase 1 covers `read_files`; Phase 2.5 expands to the full v1 set.

use warp_multi_agent_api as api;

use super::tools::{tool_definitions, translate_openai_tool_call, LocalTool, ToolParseError};

#[test]
fn from_name_round_trip() {
    for t in [
        LocalTool::ReadFiles,
        LocalTool::ApplyFileDiffs,
        LocalTool::RunShellCommand,
        LocalTool::Grep,
        LocalTool::FileGlobV2,
    ] {
        assert_eq!(LocalTool::from_name(t.name()), Some(t));
    }
}

#[test]
fn from_name_unknown_returns_none() {
    assert!(LocalTool::from_name("nope").is_none());
}

#[test]
fn definitions_filtered_to_enabled_set() {
    let defs = tool_definitions(&[LocalTool::ReadFiles, LocalTool::Grep]);
    assert_eq!(defs.len(), 2);
    let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
    assert!(names.contains(&"read_files"));
    assert!(names.contains(&"grep"));
}

#[test]
fn definitions_full_v1_set() {
    let all = tool_definitions(&[
        LocalTool::ReadFiles,
        LocalTool::ApplyFileDiffs,
        LocalTool::RunShellCommand,
        LocalTool::Grep,
        LocalTool::FileGlobV2,
    ]);
    assert_eq!(all.len(), 5);
}

#[test]
fn definitions_empty_when_no_tools() {
    let defs = tool_definitions(&[]);
    assert!(defs.is_empty());
}

#[test]
fn read_files_schema_advertises_paths_array() {
    let defs = tool_definitions(&[LocalTool::ReadFiles]);
    let schema = &defs[0].function.parameters;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["paths"]["type"], "array");
    assert_eq!(schema["properties"]["paths"]["items"]["type"], "string");
    assert_eq!(schema["required"][0], "paths");
}

#[test]
fn read_files_translate_minimal_valid() {
    let result =
        translate_openai_tool_call("call_abc", "read_files", r#"{"paths":["src/main.rs"]}"#)
            .unwrap();
    assert_eq!(result.tool_call_id, "call_abc");
    let inner = result.tool.as_ref().expect("tool variant present");
    match inner {
        api::message::tool_call::Tool::ReadFiles(rf) => {
            assert_eq!(rf.files.len(), 1);
            assert_eq!(rf.files[0].name, "src/main.rs");
            assert!(rf.files[0].line_ranges.is_empty());
        }
        _ => panic!("expected ReadFiles variant"),
    }
}

#[test]
fn read_files_translate_multiple_paths() {
    let result = translate_openai_tool_call(
        "call_xyz",
        "read_files",
        r#"{"paths":["a.rs","b.rs","c.rs"]}"#,
    )
    .unwrap();
    let inner = result.tool.as_ref().unwrap();
    if let api::message::tool_call::Tool::ReadFiles(rf) = inner {
        let names: Vec<_> = rf.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["a.rs", "b.rs", "c.rs"]);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn read_files_missing_paths_field() {
    let err = translate_openai_tool_call("id", "read_files", r#"{"foo":"bar"}"#).unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("paths"));
}

#[test]
fn read_files_paths_wrong_type() {
    let err =
        translate_openai_tool_call("id", "read_files", r#"{"paths":"not-array"}"#).unwrap_err();
    assert!(matches!(
        err,
        ToolParseError::TypeMismatch { field: "paths", .. }
    ));
}

#[test]
fn read_files_paths_element_wrong_type() {
    let err = translate_openai_tool_call("id", "read_files", r#"{"paths":["ok",42]}"#).unwrap_err();
    assert!(matches!(
        err,
        ToolParseError::TypeMismatch { field: "paths", .. }
    ));
}

#[test]
fn read_files_extra_fields_ignored() {
    // Hallucinated `purpose` field should not be an error.
    let result = translate_openai_tool_call(
        "id",
        "read_files",
        r#"{"paths":["x.rs"],"purpose":"because"}"#,
    );
    assert!(result.is_ok());
}

#[test]
fn invalid_json_arguments_surface_as_error() {
    let err = translate_openai_tool_call("id", "read_files", "{not json").unwrap_err();
    assert!(matches!(err, ToolParseError::InvalidJson(_)));
}

#[test]
fn unknown_tool_name_surfaces_as_error() {
    let err = translate_openai_tool_call("id", "magic_tool", r#"{}"#).unwrap_err();
    assert_eq!(err, ToolParseError::UnknownTool("magic_tool".into()));
}

// ---------- run_shell_command ----------

#[test]
fn run_shell_command_minimal_valid() {
    let tc =
        translate_openai_tool_call("id", "run_shell_command", r#"{"command":"ls -la"}"#).unwrap();
    if let api::message::tool_call::Tool::RunShellCommand(rsc) = tc.tool.unwrap() {
        assert_eq!(rsc.command, "ls -la");
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn run_shell_command_missing_command() {
    let err = translate_openai_tool_call("id", "run_shell_command", r#"{}"#).unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("command"));
}

// ---------- grep ----------

#[test]
fn grep_minimal_valid() {
    let tc =
        translate_openai_tool_call("id", "grep", r#"{"queries":["TODO","FIXME"],"path":"src"}"#)
            .unwrap();
    if let api::message::tool_call::Tool::Grep(g) = tc.tool.unwrap() {
        assert_eq!(g.queries, vec!["TODO", "FIXME"]);
        assert_eq!(g.path, "src");
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn grep_path_optional_defaults_empty() {
    let tc = translate_openai_tool_call("id", "grep", r#"{"queries":["x"]}"#).unwrap();
    if let api::message::tool_call::Tool::Grep(g) = tc.tool.unwrap() {
        assert_eq!(g.queries, vec!["x"]);
        assert_eq!(g.path, "");
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn grep_queries_required() {
    let err = translate_openai_tool_call("id", "grep", r#"{"path":"src"}"#).unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("queries"));
}

#[test]
fn grep_special_chars_pass_through_unaltered() {
    let tc =
        translate_openai_tool_call("id", "grep", r#"{"queries":["fn\\s+main\\(\\)"]}"#).unwrap();
    if let api::message::tool_call::Tool::Grep(g) = tc.tool.unwrap() {
        assert_eq!(g.queries, vec!["fn\\s+main\\(\\)"]);
    } else {
        panic!("wrong variant");
    }
}

// ---------- file_glob_v2 ----------

#[test]
fn file_glob_v2_minimal_valid() {
    let tc =
        translate_openai_tool_call("id", "file_glob_v2", r#"{"patterns":["**/*.rs"]}"#).unwrap();
    if let api::message::tool_call::Tool::FileGlobV2(g) = tc.tool.unwrap() {
        assert_eq!(g.patterns, vec!["**/*.rs"]);
        assert_eq!(g.search_dir, "");
        assert_eq!(g.max_matches, 0);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn file_glob_v2_full_args() {
    let tc = translate_openai_tool_call(
        "id",
        "file_glob_v2",
        r#"{"patterns":["*.rs","*.toml"],"search_dir":"crates","max_matches":50,"max_depth":3,"min_depth":1}"#,
    )
    .unwrap();
    if let api::message::tool_call::Tool::FileGlobV2(g) = tc.tool.unwrap() {
        assert_eq!(g.patterns, vec!["*.rs", "*.toml"]);
        assert_eq!(g.search_dir, "crates");
        assert_eq!(g.max_matches, 50);
        assert_eq!(g.max_depth, 3);
        assert_eq!(g.min_depth, 1);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn file_glob_v2_patterns_required() {
    let err =
        translate_openai_tool_call("id", "file_glob_v2", r#"{"search_dir":"."}"#).unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("patterns"));
}

// ---------- apply_file_diffs ----------

#[test]
fn apply_file_diffs_minimal_valid() {
    let tc = translate_openai_tool_call(
        "id",
        "apply_file_diffs",
        r#"{
            "summary": "rename foo to bar",
            "diffs": [{
                "file_path": "src/main.rs",
                "search": "fn foo()",
                "replace": "fn bar()"
            }]
        }"#,
    )
    .unwrap();
    if let api::message::tool_call::Tool::ApplyFileDiffs(afd) = tc.tool.unwrap() {
        assert_eq!(afd.summary, "rename foo to bar");
        assert_eq!(afd.diffs.len(), 1);
        assert_eq!(afd.diffs[0].file_path, "src/main.rs");
        assert_eq!(afd.diffs[0].search, "fn foo()");
        assert_eq!(afd.diffs[0].replace, "fn bar()");
        assert!(afd.v4a_updates.is_empty(), "v1 ships search/replace only");
        assert!(afd.new_files.is_empty());
        assert!(afd.deleted_files.is_empty());
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn apply_file_diffs_diffs_required() {
    let err =
        translate_openai_tool_call("id", "apply_file_diffs", r#"{"summary":"empty"}"#).unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("diffs"));
}

#[test]
fn apply_file_diffs_diff_missing_search() {
    let err = translate_openai_tool_call(
        "id",
        "apply_file_diffs",
        r#"{"diffs":[{"file_path":"a.rs","replace":"x"}]}"#,
    )
    .unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("search"));
}

#[test]
fn apply_file_diffs_summary_optional() {
    let tc = translate_openai_tool_call(
        "id",
        "apply_file_diffs",
        r#"{"diffs":[{"file_path":"a.rs","search":"x","replace":"y"}]}"#,
    )
    .unwrap();
    if let api::message::tool_call::Tool::ApplyFileDiffs(afd) = tc.tool.unwrap() {
        assert_eq!(afd.summary, "");
        assert_eq!(afd.diffs.len(), 1);
    } else {
        panic!("wrong variant");
    }
}
