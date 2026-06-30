use super::*;

#[test]
fn summary_template_contains_all_required_sections() {
    let required = [
        "## Goal",
        "## Constraints & Preferences",
        "## Progress",
        "### Done",
        "### In Progress",
        "### Blocked",
        "## Key Decisions",
        "## Next Steps",
        "## Critical Context",
        "## Relevant Files",
    ];
    for section in required {
        assert!(
            SUMMARY_TEMPLATE.contains(section),
            "SUMMARY_TEMPLATE missing required section: {section}"
        );
    }
}

#[test]
fn build_prompt_fresh_branch_when_no_previous_summary() {
    let prompt = build_prompt(None, &[]);
    assert!(prompt.contains("Create a new anchored summary"));
    assert!(!prompt.contains("<previous-summary>"));
    assert!(prompt.contains(SUMMARY_TEMPLATE));
}

#[test]
fn build_prompt_update_branch_anchors_previous_summary() {
    let prev = "## Goal\n- old goal";
    let prompt = build_prompt(Some(prev), &[]);
    assert!(prompt.contains("Update the anchored summary"));
    assert!(prompt.contains("<previous-summary>"));
    assert!(prompt.contains("old goal"));
    assert!(prompt.contains(SUMMARY_TEMPLATE));
}

#[test]
fn build_prompt_appends_plugin_context_in_order() {
    let ctx = vec!["plugin-a".to_string(), "plugin-b".to_string()];
    let prompt = build_prompt(None, &ctx);
    let a_pos = prompt.find("plugin-a").expect("plugin-a in prompt");
    let b_pos = prompt.find("plugin-b").expect("plugin-b in prompt");
    assert!(a_pos < b_pos);
    // Plugins land after the template body.
    let tpl_pos = prompt.find(SUMMARY_TEMPLATE).expect("template in prompt");
    assert!(a_pos > tpl_pos);
}

#[test]
fn build_continue_message_default_no_overflow_prefix() {
    let msg = build_continue_message(false);
    assert!(!msg.contains("size limit"));
    assert!(msg.contains("Continue"));
}

#[test]
fn build_continue_message_overflow_includes_media_explanation() {
    let msg = build_continue_message(true);
    assert!(msg.contains("size limit"));
    assert!(msg.contains("attachments"));
    assert!(msg.contains("Continue"));
}
