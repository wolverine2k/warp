//! Hand-authored system prompt for the local provider.
//!
//! Per `specs/GH9303/tech.md` §6.4: Warp's tuned system prompt is server-only
//! and does not ship in the OSS client. We author a generic agent system prompt
//! here. Quality is intentionally conservative — the prompt is the largest lever
//! for local-model quality and is expected to iterate post-launch.
//!
//! The template uses simple `str::replace` substitution rather than a format-style
//! parser, so brace characters in tool names or model output don't corrupt it.

const TEMPLATE: &str = include_str!("system_prompt.md");

const TOOLS_PLACEHOLDER: &str = "{{tools}}";
const CONTEXT_WINDOW_PLACEHOLDER: &str = "{{context_window}}";
const DIFF_GUIDE_PLACEHOLDER: &str = "{{diff_guide}}";

/// Compose the system prompt for a given turn.
///
/// - `tool_descriptions`: short prose lines, one per tool the model can call.
///   Already filtered to the v1 tool set; an empty slice means "tools disabled".
/// - `context_window`: optional context-window hint in tokens. `None` omits.
/// - `apply_file_diffs_enabled`: when true, the prompt includes the search/replace
///   diff format guide so the model produces diffs we can deterministically parse.
pub fn compose_system_prompt(
    tool_descriptions: &[&str],
    context_window: Option<u32>,
    apply_file_diffs_enabled: bool,
) -> String {
    let tools_block = if tool_descriptions.is_empty() {
        "No tools are currently available; respond with plain text.".to_string()
    } else {
        let mut s = String::from("You have access to the following tools:\n");
        for line in tool_descriptions {
            s.push_str("- ");
            s.push_str(line);
            s.push('\n');
        }
        s
    };

    let context_block = match context_window {
        Some(n) if n > 0 => format!("You have approximately {n} tokens of context to work with."),
        _ => String::new(),
    };

    let diff_block = if apply_file_diffs_enabled {
        DIFF_FORMAT_GUIDE.to_string()
    } else {
        String::new()
    };

    TEMPLATE
        .replace(TOOLS_PLACEHOLDER, &tools_block)
        .replace(CONTEXT_WINDOW_PLACEHOLDER, &context_block)
        .replace(DIFF_GUIDE_PLACEHOLDER, &diff_block)
}

/// V4A-vs-search/replace decision (per spec §6.4 / M1): v1 commits to the
/// simpler search/replace shape because smaller local models produce it
/// more reliably. V4A is a follow-up gated on the `supports_v4a_file_diffs`
/// capability bit.
const DIFF_FORMAT_GUIDE: &str = r#"
When you need to edit a file, emit an `apply_file_diffs` tool call with a list
of search/replace blocks. Each block has:
  - `file_path`: the path being edited
  - `search`: the EXACT existing text to replace, character-for-character
  - `replace`: the new text

Search text must be unique within the file. Include enough surrounding lines for
the search to match exactly one location. To create a new file, use an empty
`search` value.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tools_says_so() {
        let p = compose_system_prompt(&[], None, false);
        assert!(p.contains("No tools are currently available"));
    }

    #[test]
    fn lists_supplied_tools() {
        let p = compose_system_prompt(
            &[
                "read_files: read text files from disk",
                "grep: search files for a regex",
            ],
            None,
            false,
        );
        assert!(p.contains("read_files"));
        assert!(p.contains("grep"));
    }

    #[test]
    fn context_window_present_when_set() {
        let p = compose_system_prompt(&[], Some(8192), false);
        assert!(p.contains("8192 tokens"));
    }

    #[test]
    fn context_window_omitted_when_none() {
        let p = compose_system_prompt(&[], None, false);
        assert!(!p.contains("tokens of context"));
    }

    #[test]
    fn context_window_omitted_when_zero() {
        let p = compose_system_prompt(&[], Some(0), false);
        assert!(!p.contains("tokens of context"));
    }

    #[test]
    fn diff_guide_present_iff_enabled() {
        assert!(compose_system_prompt(&[], None, true).contains("apply_file_diffs"));
        assert!(!compose_system_prompt(&[], None, false).contains("search/replace blocks"));
    }

    #[test]
    fn template_substitution_is_brace_safe() {
        // Tool name with literal braces should not corrupt the output.
        let p = compose_system_prompt(&["weird{tool}name: does things"], None, false);
        assert!(p.contains("weird{tool}name"));
    }

    #[test]
    fn deterministic_across_calls() {
        let a = compose_system_prompt(&["x: y"], Some(4096), true);
        let b = compose_system_prompt(&["x: y"], Some(4096), true);
        assert_eq!(a, b);
    }
}
