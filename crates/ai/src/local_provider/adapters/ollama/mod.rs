//! Ollama native API adapter. Phase 3b.
//!
//! Submodule layout:
//! - `wire`: serde types for the native `/api/chat` request and NDJSON
//!   streaming response.
//! - `request` (Task 2): translator from `LocalProviderInput` to an
//!   `OllamaChatRequest`.
//! - `response` (Task 5): NDJSON stream decoder.
//!
//! Wire-format differences from OpenAi handled here:
//! - **NDJSON streaming** instead of SSE (`Accept: application/x-ndjson`,
//!   each response-body line is a complete `OllamaChatChunk`).
//! - Optional Bearer auth — most local Ollama instances are unauthed.
//! - Native tool_call shape: `tool_calls[].function.arguments` is a JSON
//!   object, not a stringified-JSON string; no `id` / `type:function`
//!   fields. The decoder synthesizes UUID ids since Ollama doesn't send
//!   any.
//! - `options.num_ctx` threaded from `cfg.context_window` to size the KV
//!   cache appropriately for large-context models.
//!
//! The full `ProviderAdapter` impl arrives in Task 7 of the phase-3b plan;
//! today this file just registers submodules so the wire types compile.

pub mod request;
pub mod wire;

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
