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
