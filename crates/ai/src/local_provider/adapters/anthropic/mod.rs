//! Anthropic Messages API adapter. Phase 3a.
//!
//! Submodule layout:
//! - `wire`: serde types for the Messages API (request, streaming events,
//!   non-streaming response).
//! - `request` (Task 2): translator from `LocalProviderInput` to an
//!   `AnthropicMessagesRequest`.
//! - `response` (Task 5): SSE stream decoder.
//!
//! The full `ProviderAdapter` impl arrives in Task 6 of the phase-3a plan;
//! today this file just registers submodules so the wire types compile.

pub mod wire;
