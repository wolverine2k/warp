use ai::local_provider::AgentProviderApiType;

use super::*;

// ---------------------------------------------------------------
// Harness compatibility matrix tests — one per row of the matrix
// ---------------------------------------------------------------

#[test]
fn anthropic_compatible_with_native_and_claude() {
    let api = AgentProviderApiType::Anthropic;
    assert!(byop_harness_compatible(api, "oz"));
    assert!(byop_harness_compatible(api, ""));
    assert!(byop_harness_compatible(api, "claude"));
    assert!(byop_harness_compatible(api, "claude-code"));
    assert!(!byop_harness_compatible(api, "codex"));
    assert!(!byop_harness_compatible(api, "opencode"));
    assert!(!byop_harness_compatible(api, "gemini"));
}

#[test]
fn openai_compatible_with_native_codex_opencode() {
    let api = AgentProviderApiType::OpenAi;
    assert!(byop_harness_compatible(api, "oz"));
    assert!(byop_harness_compatible(api, ""));
    assert!(byop_harness_compatible(api, "codex"));
    assert!(byop_harness_compatible(api, "opencode"));
    assert!(!byop_harness_compatible(api, "claude"));
    assert!(!byop_harness_compatible(api, "gemini"));
}

#[test]
fn openai_resp_compatible_with_native_and_codex_not_opencode() {
    let api = AgentProviderApiType::OpenAiResp;
    assert!(byop_harness_compatible(api, "oz"));
    assert!(byop_harness_compatible(api, "codex"));
    assert!(!byop_harness_compatible(api, "opencode"));
    assert!(!byop_harness_compatible(api, "claude"));
    assert!(!byop_harness_compatible(api, "gemini"));
}

#[test]
fn deepseek_compatible_with_native_codex_opencode() {
    let api = AgentProviderApiType::DeepSeek;
    assert!(byop_harness_compatible(api, "oz"));
    assert!(byop_harness_compatible(api, "codex"));
    assert!(byop_harness_compatible(api, "opencode"));
    assert!(!byop_harness_compatible(api, "claude"));
    assert!(!byop_harness_compatible(api, "gemini"));
}

#[test]
fn gemini_compatible_with_native_and_gemini_cli() {
    let api = AgentProviderApiType::Gemini;
    assert!(byop_harness_compatible(api, "oz"));
    assert!(byop_harness_compatible(api, "gemini"));
    assert!(!byop_harness_compatible(api, "codex"));
    assert!(!byop_harness_compatible(api, "opencode"));
    assert!(!byop_harness_compatible(api, "claude"));
}

#[test]
fn ollama_compatible_with_native_only() {
    let api = AgentProviderApiType::Ollama;
    assert!(byop_harness_compatible(api, "oz"));
    assert!(byop_harness_compatible(api, ""));
    assert!(!byop_harness_compatible(api, "codex"));
    assert!(!byop_harness_compatible(api, "opencode"));
    assert!(!byop_harness_compatible(api, "claude"));
    assert!(!byop_harness_compatible(api, "gemini"));
}

// ---------------------------------------------------------------
// Reachability heuristic tests
// ---------------------------------------------------------------

#[test]
fn reachability_rejects_localhost() {
    assert!(!base_url_reachable_from_remote("http://localhost:11434/v1"));
    assert!(!base_url_reachable_from_remote("http://localhost/v1"));
    assert!(!base_url_reachable_from_remote("https://localhost:8443"));
}

#[test]
fn reachability_rejects_loopback_ipv4() {
    assert!(!base_url_reachable_from_remote("http://127.0.0.1:8080/v1"));
    assert!(!base_url_reachable_from_remote("http://127.0.0.2:8080"));
    assert!(!base_url_reachable_from_remote(
        "http://127.255.255.255:1234"
    ));
}

#[test]
fn reachability_rejects_loopback_ipv6() {
    assert!(!base_url_reachable_from_remote("http://[::1]:8080/v1"));
}

#[test]
fn reachability_rejects_rfc1918_10_range() {
    assert!(!base_url_reachable_from_remote("http://10.0.0.1:8080/v1"));
    assert!(!base_url_reachable_from_remote("http://10.255.255.255:443"));
}

#[test]
fn reachability_rejects_rfc1918_172_range() {
    assert!(!base_url_reachable_from_remote("http://172.16.0.1:8080"));
    assert!(!base_url_reachable_from_remote("http://172.31.255.255:443"));
    assert!(base_url_reachable_from_remote("http://172.15.0.1:8080"));
    assert!(base_url_reachable_from_remote("http://172.32.0.1:8080"));
}

#[test]
fn reachability_rejects_rfc1918_192_168_range() {
    assert!(!base_url_reachable_from_remote("http://192.168.0.1:8080"));
    assert!(!base_url_reachable_from_remote(
        "http://192.168.255.255:443"
    ));
    assert!(base_url_reachable_from_remote("http://192.169.0.1:8080"));
}

#[test]
fn reachability_rejects_local_tld() {
    assert!(!base_url_reachable_from_remote("http://myhost.local:11434"));
    assert!(!base_url_reachable_from_remote("http://llm.localhost:8080"));
}

#[test]
fn reachability_accepts_public_hostname() {
    assert!(base_url_reachable_from_remote(
        "https://api.deepseek.com/v1"
    ));
    assert!(base_url_reachable_from_remote(
        "https://my-llm.example.com:8443"
    ));
    assert!(base_url_reachable_from_remote(
        "https://api.anthropic.com/v1"
    ));
}

#[test]
fn reachability_accepts_public_ip() {
    assert!(base_url_reachable_from_remote(
        "http://203.0.113.50:8080/v1"
    ));
    assert!(base_url_reachable_from_remote("http://8.8.8.8:443"));
}

#[test]
fn reachability_rejects_empty_url() {
    assert!(!base_url_reachable_from_remote(""));
    assert!(!base_url_reachable_from_remote("   "));
}

#[test]
fn reachability_rejects_zero_address() {
    assert!(!base_url_reachable_from_remote("http://0.0.0.0:8080"));
}

// ---------------------------------------------------------------
// Harness normalization edge cases
// ---------------------------------------------------------------

#[test]
fn harness_normalize_treats_empty_as_native() {
    assert!(byop_harness_compatible(AgentProviderApiType::Ollama, ""));
    assert!(byop_harness_compatible(AgentProviderApiType::Ollama, "  "));
}

#[test]
fn harness_normalize_case_insensitive() {
    assert!(byop_harness_compatible(
        AgentProviderApiType::Anthropic,
        "Claude"
    ));
    assert!(byop_harness_compatible(
        AgentProviderApiType::Anthropic,
        "CLAUDE"
    ));
    assert!(byop_harness_compatible(
        AgentProviderApiType::OpenAi,
        "Codex"
    ));
}

#[test]
fn harness_unknown_string_treated_as_native() {
    assert!(byop_harness_compatible(
        AgentProviderApiType::Ollama,
        "future-harness"
    ));
    assert!(byop_harness_compatible(
        AgentProviderApiType::Anthropic,
        "xyzzy"
    ));
}
