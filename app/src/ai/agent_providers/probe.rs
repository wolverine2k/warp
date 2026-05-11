//! Per-provider connection probe used by the "Test connection" button in
//! `AgentProvidersWidget` (Phase 2). Each call selects an adapter for the
//! provider's `api_type`, builds a probe request (typically `GET /v1/models`
//! for OpenAI-compatible endpoints), fires it, and returns a one-line
//! user-visible status.
//!
//! The helper is wire-protocol-agnostic — Phase 3 adapters get probe
//! support automatically as soon as their `build_probe_request` returns
//! something other than `Err(UnsupportedApiType(...))`.

use ai::local_provider::{
    config::LocalProviderConfig, select_adapter, ProviderAdapterError as AdapterError,
};

/// Outcome of a single probe attempt.
#[derive(Debug, Clone)]
pub enum ProbeOutcome {
    /// Adapter built a probe request that returned HTTP 2xx.
    Ok,
    /// Anything else: unsupported api_type, validation failure, transport
    /// error, or a non-2xx HTTP response. The string is user-visible (first
    /// ~120 chars of the underlying reason).
    Failed(String),
}

/// Run the connection probe and return a structured outcome. The body of
/// any HTTP response is not parsed; success is HTTP 2xx, anything else is a
/// failure. This keeps the probe wire-protocol-agnostic — different
/// upstreams return different model-list shapes.
pub async fn probe(cfg: LocalProviderConfig, http: reqwest::Client) -> ProbeOutcome {
    let adapter = match select_adapter(cfg.api_type) {
        Ok(a) => a,
        Err(AdapterError::UnsupportedApiType(t)) => {
            return ProbeOutcome::Failed(format!("api_type {t:?} is not implemented yet"));
        }
        Err(e) => return ProbeOutcome::Failed(format!("{e}")),
    };
    let req = match adapter.build_probe_request(&cfg, &http) {
        Ok(r) => r,
        Err(e) => return ProbeOutcome::Failed(format!("{e}")),
    };
    match req.send().await {
        Ok(resp) if resp.status().is_success() => ProbeOutcome::Ok,
        Ok(resp) => {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect::<String>();
            if body.is_empty() {
                ProbeOutcome::Failed(format!("HTTP {status}"))
            } else {
                ProbeOutcome::Failed(format!("HTTP {status}: {body}"))
            }
        }
        Err(e) => ProbeOutcome::Failed(format!("{e}")),
    }
}
