//! Tests for the OpenAI ↔ proto tool-call translation table.
//! Phase 1 covers `read_files`; Phase 2.5 expands to the full v1 set.

use warp_multi_agent_api as api;

use super::tools::{
    tool_definitions, translate_openai_tool_call, LocalTool, ToolParseError,
};

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
    // Only ReadFiles has a schema in Phase 1; the rest are skipped silently.
    let defs = tool_definitions(&[LocalTool::ReadFiles, LocalTool::Grep]);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].function.name, "read_files");
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
    let result = translate_openai_tool_call(
        "call_abc",
        "read_files",
        r#"{"paths":["src/main.rs"]}"#,
    )
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
    let err =
        translate_openai_tool_call("id", "read_files", r#"{"foo":"bar"}"#).unwrap_err();
    assert_eq!(err, ToolParseError::MissingField("paths"));
}

#[test]
fn read_files_paths_wrong_type() {
    let err = translate_openai_tool_call("id", "read_files", r#"{"paths":"not-array"}"#)
        .unwrap_err();
    assert!(matches!(err, ToolParseError::TypeMismatch { field: "paths", .. }));
}

#[test]
fn read_files_paths_element_wrong_type() {
    let err = translate_openai_tool_call("id", "read_files", r#"{"paths":["ok",42]}"#)
        .unwrap_err();
    assert!(matches!(err, ToolParseError::TypeMismatch { field: "paths", .. }));
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

#[test]
fn unimplemented_tool_returns_not_implemented() {
    // Phase 2.5 will implement these; for now we expect a clean NotImplemented.
    let err =
        translate_openai_tool_call("id", "run_shell_command", r#"{"command":"ls"}"#)
            .unwrap_err();
    assert!(matches!(err, ToolParseError::NotImplemented("run_shell_command")));
}
