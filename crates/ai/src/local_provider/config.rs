//! Configuration for the local provider — see `specs/GH9303/tech.md` §3.
//!
//! `LocalProviderConfig` is a frozen snapshot of the user's settings + secret,
//! captured at the AppContext-owning call site and threaded into the dispatch
//! function as part of `RequestParams`. Everything downstream of dispatch
//! reads from this snapshot, so the runtime path stays AppContext-free.

use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalProviderConfig {
    /// User-facing name for the picker entry.
    pub display_name: String,
    /// Base URL of the OpenAI-compatible endpoint (e.g. `http://localhost:11434/v1`).
    /// Validated to parse as a URL with an `http` or `https` scheme.
    pub base_url: String,
    /// The model id the user's endpoint expects.
    pub model_id: String,
    /// Optional bearer token. When `Some(...)`, sent as `Authorization: Bearer <key>`.
    pub api_key: Option<String>,
    /// Whether to send the `tools` field on outbound requests.
    pub supports_tools: bool,
    /// Optional context-window size in tokens. When `Some(n)`, surfaced in the
    /// system prompt; `None` means "omit and let the model handle context limits".
    pub context_window: Option<u32>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LocalProviderConfigError {
    #[error("base URL is empty")]
    EmptyBaseUrl,
    #[error("base URL is not a valid HTTP(S) URL: {0}")]
    InvalidBaseUrl(String),
    #[error("model id is empty")]
    EmptyModelId,
    #[error("display name is empty")]
    EmptyDisplayName,
}

impl LocalProviderConfig {
    /// Validate the snapshot. Returns `Ok(())` when the config is sendable, an
    /// error otherwise. The settings UI uses this for inline validation; the
    /// dispatch router rejects invalid configs by treating them as "unconfigured".
    pub fn validate(&self) -> Result<(), LocalProviderConfigError> {
        if self.display_name.trim().is_empty() {
            return Err(LocalProviderConfigError::EmptyDisplayName);
        }
        if self.base_url.trim().is_empty() {
            return Err(LocalProviderConfigError::EmptyBaseUrl);
        }
        let parsed = Url::parse(&self.base_url)
            .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(LocalProviderConfigError::InvalidBaseUrl(format!(
                "scheme `{}` is not http(s)",
                parsed.scheme()
            )));
        }
        if self.model_id.trim().is_empty() {
            return Err(LocalProviderConfigError::EmptyModelId);
        }
        Ok(())
    }

    /// The synthetic LLMId used to identify this provider in the model picker.
    /// Format: `local:{model_id}`. The `local:` prefix is what the dispatch
    /// router checks to decide between server and local paths.
    pub fn synthetic_llm_id(&self) -> String {
        format!("local:{}", self.model_id)
    }

    /// The chat-completions endpoint URL: `{base_url}/chat/completions`,
    /// joined defensively even when the base URL has or omits a trailing slash.
    pub fn chat_completions_url(&self) -> Result<Url, LocalProviderConfigError> {
        let mut base = Url::parse(&self.base_url)
            .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))?;
        // Ensure the path ends with `/` so `join` appends instead of replacing the
        // last segment.
        if !base.path().ends_with('/') {
            let new_path = format!("{}/", base.path());
            base.set_path(&new_path);
        }
        base.join("chat/completions")
            .map_err(|e| LocalProviderConfigError::InvalidBaseUrl(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(base: &str, model: &str) -> LocalProviderConfig {
        LocalProviderConfig {
            display_name: "Local".into(),
            base_url: base.into(),
            model_id: model.into(),
            api_key: None,
            supports_tools: true,
            context_window: None,
        }
    }

    #[test]
    fn validate_happy_path() {
        assert!(cfg("http://localhost:11434/v1", "llama3.1").validate().is_ok());
        assert!(cfg("https://api.example.com/v1", "gpt-4").validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_url() {
        assert_eq!(
            cfg("", "llama3.1").validate(),
            Err(LocalProviderConfigError::EmptyBaseUrl)
        );
    }

    #[test]
    fn validate_rejects_non_http_scheme() {
        let result = cfg("ftp://nope/v1", "llama3.1").validate();
        assert!(matches!(result, Err(LocalProviderConfigError::InvalidBaseUrl(_))));
    }

    #[test]
    fn validate_rejects_unparseable_url() {
        let result = cfg("not a url", "llama3.1").validate();
        assert!(matches!(result, Err(LocalProviderConfigError::InvalidBaseUrl(_))));
    }

    #[test]
    fn validate_rejects_empty_model() {
        assert_eq!(
            cfg("http://localhost:11434/v1", "").validate(),
            Err(LocalProviderConfigError::EmptyModelId)
        );
    }

    #[test]
    fn synthetic_llm_id_format() {
        assert_eq!(cfg("http://x/v1", "llama3.1").synthetic_llm_id(), "local:llama3.1");
        assert_eq!(cfg("http://x/v1", "qwen2.5-coder:7b").synthetic_llm_id(), "local:qwen2.5-coder:7b");
    }

    #[test]
    fn chat_completions_url_joins_path() {
        let url = cfg("http://localhost:11434/v1", "llama3.1")
            .chat_completions_url()
            .unwrap();
        assert_eq!(url.as_str(), "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn chat_completions_url_with_trailing_slash() {
        let url = cfg("http://localhost:11434/v1/", "llama3.1")
            .chat_completions_url()
            .unwrap();
        assert_eq!(url.as_str(), "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn chat_completions_url_no_path() {
        let url = cfg("http://localhost:11434", "llama3.1")
            .chat_completions_url()
            .unwrap();
        assert_eq!(url.as_str(), "http://localhost:11434/chat/completions");
    }
}
