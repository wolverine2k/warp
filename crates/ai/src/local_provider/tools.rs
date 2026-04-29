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
    match tool {
        LocalTool::ReadFiles => Some(read_files_schema()),
        LocalTool::ApplyFileDiffs
        | LocalTool::RunShellCommand
        | LocalTool::Grep
        | LocalTool::FileGlobV2 => None, // Phase 2.5 fills these in
    }
}

fn build_tool_inner(
    tool: LocalTool,
    args: &Value,
) -> Result<api::message::tool_call::Tool, ToolParseError> {
    match tool {
        LocalTool::ReadFiles => build_read_files(args),
        LocalTool::ApplyFileDiffs => Err(ToolParseError::NotImplemented("apply_file_diffs")),
        LocalTool::RunShellCommand => Err(ToolParseError::NotImplemented("run_shell_command")),
        LocalTool::Grep => Err(ToolParseError::NotImplemented("grep")),
        LocalTool::FileGlobV2 => Err(ToolParseError::NotImplemented("file_glob_v2")),
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
