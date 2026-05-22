//! Phase 5a. Pure helpers for filtering BYOP models in orchestration pickers.
//!
//! Two concerns:
//! 1. **Harness compatibility** — which API-type + harness combinations are
//!    valid. The matrix is maintained inline; changes to external CLIs may
//!    require updates here.
//! 2. **Remote reachability** — whether a provider's `base_url` is likely
//!    reachable from a Remote worker host. Best-effort string-based heuristic;
//!    see the doc comment on `base_url_reachable_from_remote` for known
//!    limitations.
//!
//! Note: functions here are suppressed for dead-code lint until Task 4 wires
//! them into the orchestration model picker.
#![allow(dead_code)]

use ai::local_provider::AgentProviderApiType;

/// Returns `true` when `api_type` can drive agents under `harness_type`.
///
/// The matrix (from spec-phase-5.md):
///
/// | API type   | Compatible harnesses                       |
/// |------------|--------------------------------------------|
/// | Anthropic  | Native (oz / empty), claude-code (claude)  |
/// | OpenAI     | Native, codex, opencode                    |
/// | OpenAIResp | Native, codex (NOT opencode)               |
/// | DeepSeek   | Native, codex, opencode                    |
/// | Gemini     | Native, gemini                             |
/// | Ollama     | Native only                                |
///
/// `harness_type` uses the canonical config-name strings from
/// `Harness::config_name()`: `"oz"`, `"claude"`, `"opencode"`, `"gemini"`,
/// `"codex"`. An empty string is treated as Native (oz).
pub fn byop_harness_compatible(api_type: AgentProviderApiType, harness_type: &str) -> bool {
    let harness = normalize_harness(harness_type);

    match api_type {
        AgentProviderApiType::Anthropic => matches!(harness, "oz" | "claude"),
        AgentProviderApiType::OpenAi => matches!(harness, "oz" | "codex" | "opencode"),
        AgentProviderApiType::OpenAiResp => matches!(harness, "oz" | "codex"),
        AgentProviderApiType::DeepSeek => matches!(harness, "oz" | "codex" | "opencode"),
        AgentProviderApiType::Gemini => matches!(harness, "oz" | "gemini"),
        AgentProviderApiType::Ollama => matches!(harness, "oz"),
    }
}

/// Normalize harness_type to the canonical config-name. Empty / "oz" / unknown
/// all map to `"oz"` (Native).
fn normalize_harness(harness_type: &str) -> &'static str {
    let trimmed = harness_type.trim();
    if trimmed.is_empty() {
        return "oz";
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "claude" | "claude-code" => "claude",
        "opencode" => "opencode",
        "gemini" => "gemini",
        "codex" => "codex",
        "oz" => "oz",
        _ => "oz",
    }
}

/// Returns `false` when `base_url` points at an address that a Remote worker
/// host almost certainly cannot reach: localhost, loopback (127.x.x.x / ::1),
/// RFC1918 private ranges (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16),
/// and `.local` / `.localhost` TLDs.
///
/// **Known limitations (best-effort heuristic):**
/// - False negatives: a publicly-resolvable hostname pointing at a private IP
///   (e.g. `home.example.com -> 192.168.1.10`) will pass this check even
///   though the Remote worker cannot reach it. The heuristic operates on the
///   URL string, not the resolved address.
/// - False positives: a Tailscale `.ts.net` address or a VPN hostname is
///   technically reachable from a worker on the same network, but this
///   heuristic has no way to know that. Users on private overlays can work
///   around this by using Local execution mode.
pub fn base_url_reachable_from_remote(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return false;
    }

    let host = match url::Url::parse(trimmed) {
        Ok(parsed) => match parsed.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return false,
        },
        Err(_) => return false,
    };

    // `url::Url::parse` keeps the surrounding brackets on an IPv6 host (so
    // `host_str()` returns `"[::1]"`). Match both forms so a future caller
    // passing an already-unwrapped host string still gets rejected.
    if host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
        || host == "0.0.0.0"
    {
        return false;
    }

    if host.ends_with(".local") || host.ends_with(".localhost") {
        return false;
    }

    // Single IPv4 parse covering loopback 127/8 and RFC1918 (10/8, 172.16/12, 192.168/16).
    if let Ok(addr) = host.parse::<std::net::Ipv4Addr>() {
        let octets = addr.octets();
        if octets[0] == 127 {
            return false;
        }
        if octets[0] == 10 {
            return false;
        }
        if octets[0] == 172 && (16..=31).contains(&octets[1]) {
            return false;
        }
        if octets[0] == 192 && octets[1] == 168 {
            return false;
        }
    }

    true
}

#[cfg(test)]
#[path = "byop_orchestration_filter_tests.rs"]
mod tests;
