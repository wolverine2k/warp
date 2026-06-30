//! Summarizer prompt — byte-identical to opencode
//! `packages/opencode/src/session/compaction.ts:40-75, 121-132`.
//!
//! Don't "improve" the template wording — it's a portability contract with
//! opencode and any change here needs a paired change there.

/// Maps directly to `compaction.ts:40-75 SUMMARY_TEMPLATE`.
pub const SUMMARY_TEMPLATE: &str = "Output exactly the Markdown structure shown inside <template> and keep the section order unchanged. Do not include the <template> tags in your response.\n<template>\n## Goal\n- [single-sentence task summary]\n\n## Constraints & Preferences\n- [user constraints, preferences, specs, or \"(none)\"]\n\n## Progress\n### Done\n- [completed work or \"(none)\"]\n\n### In Progress\n- [current work or \"(none)\"]\n\n### Blocked\n- [blockers or \"(none)\"]\n\n## Key Decisions\n- [decision and why, or \"(none)\"]\n\n## Next Steps\n- [ordered next actions or \"(none)\"]\n\n## Critical Context\n- [important technical facts, errors, open questions, or \"(none)\"]\n\n## Relevant Files\n- [file or directory path: why it matters, or \"(none)\"]\n</template>\n\nRules:\n- Keep every section, even when empty.\n- Use terse bullets, not prose paragraphs.\n- Preserve exact file paths, commands, error strings, and identifiers when known.\n- Do not mention the summary process or that context was compacted.";

/// Build the final user prompt sent to the summarizer LLM. Maps to
/// `compaction.ts:121-132 buildPrompt`.
///
/// `previous_summary = Some(...)` triggers the "update" branch — the prior
/// summary is anchored in via `<previous-summary>` and the model is asked to
/// merge the new conversation history into it.
/// `previous_summary = None` triggers the "fresh" branch.
///
/// `context` is plugin-supplied free-form text (none in our local
/// implementation; pass `&[]`).
pub fn build_prompt(previous_summary: Option<&str>, context: &[String]) -> String {
    let anchor = match previous_summary {
        Some(prev) => format!(
            "Update the anchored summary below using the conversation history above.\n\
             Preserve still-true details, remove stale details, and merge in the new facts.\n\
             <previous-summary>\n{prev}\n</previous-summary>"
        ),
        None => "Create a new anchored summary from the conversation history above.".to_string(),
    };
    let mut parts: Vec<String> = Vec::with_capacity(2 + context.len());
    parts.push(anchor);
    parts.push(SUMMARY_TEMPLATE.to_string());
    parts.extend(context.iter().cloned());
    parts.join("\n\n")
}

/// Synthetic user "Continue..." message inserted on the auto-resume path
/// (replay=false + auto=true). Byte-aligned with opencode
/// `compaction.ts:533-537`.
///
/// `overflow=true` prepends an extra explanation about media attachments
/// being trimmed.
pub fn build_continue_message(overflow: bool) -> String {
    let prefix = if overflow {
        "The previous request exceeded the provider's size limit due to large media attachments. \
         The conversation was compacted and media files were removed from context. \
         If the user was asking about attached images or files, explain that the attachments were too large to process and suggest they try again with smaller or fewer files.\n\n"
    } else {
        ""
    };
    format!(
        "{prefix}Continue if you have next steps, or stop and ask for clarification if you are unsure how to proceed."
    )
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
