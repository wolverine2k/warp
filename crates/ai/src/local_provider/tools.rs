//! Tool definitions + bidirectional translation between OpenAI's
//! `{name, arguments_json}` and the proto's typed `Message::ToolCall.tool::*` variants.
//!
//! Per `specs/GH9303/tech.md` §6.5, v1 ships 5 tools: `read_files`, `apply_file_diffs`,
//! `run_shell_command`, `grep`, `file_glob_v2`. **Phase 1 of the implementation lands
//! the framework and `read_files` only**, to keep the initial commit reviewable.
//! The remaining four arrive in Phase 2.5.
//!
//! Each tool has:
//! - a JSON-schema parameter description sent to the model in the `tools` array,
//! - a `parse_args` function that takes the model's stringified-JSON `arguments`
//!   and produces the typed proto variant, returning `ToolParseError` on bad input.

use serde_json::Value;
use thiserror::Error;
use warp_multi_agent_api as api;

use crate::local_provider::wire::{ToolDefinition, ToolFunction};

/// Variants the v1 tool set exposes. Names match `Message::ToolCall.tool::*` in the proto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTool {
    ReadFiles,
    ApplyFileDiffs,
    RunShellCommand,
    Grep,
    FileGlobV2,
}

impl LocalTool {
    pub fn name(self) -> &'static str {
        match self {
            LocalTool::ReadFiles => "read_files",
            LocalTool::ApplyFileDiffs => "apply_file_diffs",
            LocalTool::RunShellCommand => "run_shell_command",
            LocalTool::Grep => "grep",
            LocalTool::FileGlobV2 => "file_glob_v2",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "read_files" => Some(LocalTool::ReadFiles),
            "apply_file_diffs" => Some(LocalTool::ApplyFileDiffs),
            "run_shell_command" => Some(LocalTool::RunShellCommand),
            "grep" => Some(LocalTool::Grep),
            "file_glob_v2" => Some(LocalTool::FileGlobV2),
            _ => None,
        }
    }

    /// Description shown in the system prompt's tool list.
    pub fn description(self) -> &'static str {
        match self {
            LocalTool::ReadFiles => "read_files: read one or more text files from the user's filesystem.",
            LocalTool::ApplyFileDiffs => "apply_file_diffs: apply search/replace edits to files.",
            LocalTool::RunShellCommand => "run_shell_command: run a single shell command in the user's terminal.",
            LocalTool::Grep => "grep: search files for a regex pattern.",
            LocalTool::FileGlobV2 => "file_glob_v2: list files matching a glob pattern.",
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ToolParseError {
    #[error("unknown tool name `{0}`")]
    UnknownTool(String),
    #[error("arguments are not valid JSON: {0}")]
    InvalidJson(String),
    #[error("missing required field `{0}`")]
    MissingField(&'static str),
    #[error("field `{field}` has wrong type: {detail}")]
    TypeMismatch { field: &'static str, detail: String },
    #[error("not yet implemented in this build: {0}")]
    NotImplemented(&'static str),
}

/// Build the OpenAI `tools` array for the request.
/// Only tools in `enabled` are exposed to the model.
pub fn tool_definitions(enabled: &[LocalTool]) -> Vec<ToolDefinition> {
    enabled
        .iter()
        .filter_map(|t| {
            schema_for(*t).map(|parameters| ToolDefinition {
                kind: "function",
                function: ToolFunction {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters,
                },
            })
        })
        .collect()
}

/// Translate an OpenAI tool_call (name + stringified-JSON arguments) into the
/// proto's typed `Message::ToolCall` variant. Returns `ToolParseError` on
/// unknown tool name, malformed JSON, or schema violations.
pub fn translate_openai_tool_call(
    tool_call_id: &str,
    name: &str,
    arguments_json: &str,
) -> Result<api::message::ToolCall, ToolParseError> {
    let tool = LocalTool::from_name(name).ok_or_else(|| ToolParseError::UnknownTool(name.into()))?;
    let args: Value = serde_json::from_str(arguments_json)
        .map_err(|e| ToolParseError::InvalidJson(e.to_string()))?;
    let inner = build_tool_inner(tool, &args)?;
    Ok(api::message::ToolCall {
        tool_call_id: tool_call_id.to_string(),
        tool: Some(inner),
        ..Default::default()
    })
}

fn schema_for(tool: LocalTool) -> Option<Value> {
    Some(match tool {
        LocalTool::ReadFiles => read_files_schema(),
        LocalTool::ApplyFileDiffs => apply_file_diffs_schema(),
        LocalTool::RunShellCommand => run_shell_command_schema(),
        LocalTool::Grep => grep_schema(),
        LocalTool::FileGlobV2 => file_glob_v2_schema(),
    })
}

fn build_tool_inner(
    tool: LocalTool,
    args: &Value,
) -> Result<api::message::tool_call::Tool, ToolParseError> {
    match tool {
        LocalTool::ReadFiles => build_read_files(args),
        LocalTool::ApplyFileDiffs => build_apply_file_diffs(args),
        LocalTool::RunShellCommand => build_run_shell_command(args),
        LocalTool::Grep => build_grep(args),
        LocalTool::FileGlobV2 => build_file_glob_v2(args),
    }
}

// ---------- read_files ----------
//
// The proto's `ReadFiles` is `repeated File files`, where each `File` is
// `{ name: string, line_ranges: repeated FileContentLineRange }`. We expose a
// model-friendly schema with a flat `paths: string[]` array (omitting line
// ranges in v1; the model just lists the files it wants whole) and translate
// it into the typed proto on the way in.

fn read_files_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "paths": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Absolute or repository-relative paths to read in full."
            }
        },
        "required": ["paths"]
    })
}

fn build_read_files(args: &Value) -> Result<api::message::tool_call::Tool, ToolParseError> {
    let paths_value = args
        .get("paths")
        .ok_or(ToolParseError::MissingField("paths"))?;
    let arr = paths_value.as_array().ok_or(ToolParseError::TypeMismatch {
        field: "paths",
        detail: format!("expected array, got {paths_value}"),
    })?;
    let mut files = Vec::with_capacity(arr.len());
    for (i, p) in arr.iter().enumerate() {
        let name = p
            .as_str()
            .ok_or(ToolParseError::TypeMismatch {
                field: "paths",
                detail: format!("element {i} is not a string: {p}"),
            })?
            .to_string();
        files.push(api::message::tool_call::read_files::File {
            name,
            line_ranges: vec![],
        });
    }
    Ok(api::message::tool_call::Tool::ReadFiles(
        api::message::tool_call::ReadFiles { files },
    ))
}

// ---------- apply_file_diffs ----------
//
// v1 commits to the simpler search/replace `FileDiff` shape per spec §6.4.
// V4A hunks (`V4AFileUpdate`) are deferred behind `supports_v4a_file_diffs`.
// `NewFile` / `DeleteFile` are also follow-ups.

fn apply_file_diffs_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "One-line summary of the changes."
            },
            "diffs": {
                "type": "array",
                "description": "Search/replace edits. `search` must match exactly one location in the file.",
                "items": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "search":    { "type": "string", "description": "Existing text to replace; empty to create a new file." },
                        "replace":   { "type": "string", "description": "Replacement text." }
                    },
                    "required": ["file_path", "search", "replace"]
                }
            }
        },
        "required": ["diffs"]
    })
}

fn build_apply_file_diffs(args: &Value) -> Result<api::message::tool_call::Tool, ToolParseError> {
    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let diffs_value = args
        .get("diffs")
        .ok_or(ToolParseError::MissingField("diffs"))?;
    let arr = diffs_value.as_array().ok_or(ToolParseError::TypeMismatch {
        field: "diffs",
        detail: format!("expected array, got {diffs_value}"),
    })?;
    let mut diffs = Vec::with_capacity(arr.len());
    for (i, d) in arr.iter().enumerate() {
        let obj = d.as_object().ok_or(ToolParseError::TypeMismatch {
            field: "diffs",
            detail: format!("element {i} is not an object"),
        })?;
        let file_path = obj
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or(ToolParseError::MissingField("file_path"))?
            .to_string();
        let search = obj
            .get("search")
            .and_then(|v| v.as_str())
            .ok_or(ToolParseError::MissingField("search"))?
            .to_string();
        let replace = obj
            .get("replace")
            .and_then(|v| v.as_str())
            .ok_or(ToolParseError::MissingField("replace"))?
            .to_string();
        diffs.push(api::message::tool_call::apply_file_diffs::FileDiff {
            file_path,
            search,
            replace,
        });
    }
    Ok(api::message::tool_call::Tool::ApplyFileDiffs(
        api::message::tool_call::ApplyFileDiffs {
            summary,
            diffs,
            new_files: vec![],
            deleted_files: vec![],
            v4a_updates: vec![],
        },
    ))
}

// ---------- run_shell_command ----------

fn run_shell_command_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The shell command to run. Single command, no shell pipelines unless intentional."
            }
        },
        "required": ["command"]
    })
}

fn build_run_shell_command(
    args: &Value,
) -> Result<api::message::tool_call::Tool, ToolParseError> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or(ToolParseError::MissingField("command"))?
        .to_string();
    Ok(api::message::tool_call::Tool::RunShellCommand(
        api::message::tool_call::RunShellCommand {
            command,
            // The remaining fields take their proto-default zero values; the
            // harness applies its own risk policy when wait_until_complete_value
            // is unset (None).
            ..Default::default()
        },
    ))
}

// ---------- grep ----------

fn grep_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "queries": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Search terms or regex patterns. The proto field is `repeated string queries`."
            },
            "path": {
                "type": "string",
                "description": "Relative path to the file or directory to search."
            }
        },
        "required": ["queries"]
    })
}

fn build_grep(args: &Value) -> Result<api::message::tool_call::Tool, ToolParseError> {
    let queries_value = args
        .get("queries")
        .ok_or(ToolParseError::MissingField("queries"))?;
    let arr = queries_value.as_array().ok_or(ToolParseError::TypeMismatch {
        field: "queries",
        detail: format!("expected array, got {queries_value}"),
    })?;
    let mut queries = Vec::with_capacity(arr.len());
    for (i, q) in arr.iter().enumerate() {
        let s = q.as_str().ok_or(ToolParseError::TypeMismatch {
            field: "queries",
            detail: format!("element {i} is not a string"),
        })?;
        queries.push(s.to_string());
    }
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(api::message::tool_call::Tool::Grep(
        api::message::tool_call::Grep { queries, path },
    ))
}

// ---------- file_glob_v2 ----------

fn file_glob_v2_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "patterns": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Glob patterns. Supports ?, *, []."
            },
            "search_dir": {
                "type": "string",
                "description": "Relative path to the directory to search."
            },
            "max_matches": { "type": "integer", "minimum": 0, "description": "0 means no limit." },
            "max_depth":   { "type": "integer", "minimum": 0, "description": "0 means no limit; 1 means children only." },
            "min_depth":   { "type": "integer", "minimum": 0 }
        },
        "required": ["patterns"]
    })
}

fn build_file_glob_v2(args: &Value) -> Result<api::message::tool_call::Tool, ToolParseError> {
    let patterns_value = args
        .get("patterns")
        .ok_or(ToolParseError::MissingField("patterns"))?;
    let arr = patterns_value.as_array().ok_or(ToolParseError::TypeMismatch {
        field: "patterns",
        detail: format!("expected array, got {patterns_value}"),
    })?;
    let mut patterns = Vec::with_capacity(arr.len());
    for (i, p) in arr.iter().enumerate() {
        let s = p.as_str().ok_or(ToolParseError::TypeMismatch {
            field: "patterns",
            detail: format!("element {i} is not a string"),
        })?;
        patterns.push(s.to_string());
    }
    let search_dir = args
        .get("search_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let max_matches = args.get("max_matches").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let max_depth = args.get("max_depth").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let min_depth = args.get("min_depth").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    Ok(api::message::tool_call::Tool::FileGlobV2(
        api::message::tool_call::FileGlobV2 {
            patterns,
            search_dir,
            max_matches,
            max_depth,
            min_depth,
        },
    ))
}
