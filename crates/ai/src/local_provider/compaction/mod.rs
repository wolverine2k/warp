//! Conversation compaction (Phase A: tool-output prune).
//!
//! Ports the algorithm from openwarp's `app/src/ai/byop_compaction/`, which
//! itself is a 1:1 port of opencode `packages/opencode/src/session/compaction.ts`.
//!
//! Phase A scope: we run [`algorithm::prune_decisions`] over the proto message
//! history each turn and replace the OpenAI tool-message content for any
//! flagged tool_call_id with a short placeholder. This bounds request-body
//! size on long, tool-heavy conversations without invoking a summarizer LLM
//! and without persisting any state across turns.
//!
//! Phase B (deferred — see `specs/GH9303/compaction-phase-b.md`):
//! - [`algorithm::select`]-driven head/tail summarization
//! - `CompactionState` sidecar persistence on `AIConversation`
//! - `byop_compaction_*` `AISettings` fields
//! - `/compact` user command in the input bar
//! - Auto-trigger on token-overflow detection (`is_overflow`)

pub mod algorithm;
pub mod config;
pub mod state;
pub mod wire;

#[cfg(test)]
#[path = "algorithm_tests.rs"]
mod algorithm_tests;

pub use config::CompactionConfig;
pub use state::{CompactionState, CompactionTrigger, CompletedCompaction, MessageMarker};

/// Constants byte-aligned with opencode `compaction.ts:33-39`.
pub mod consts {
    pub const PRUNE_MINIMUM: usize = 20_000;
    pub const PRUNE_PROTECT: usize = 40_000;
    pub const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
    pub const DEFAULT_TAIL_TURNS: usize = 2;
    pub const MIN_PRESERVE_RECENT_TOKENS: usize = 2_000;
    pub const MAX_PRESERVE_RECENT_TOKENS: usize = 8_000;
    pub const COMPACTION_BUFFER: usize = 20_000;
    pub const CHARS_PER_TOKEN: usize = 4;
    pub const PRUNE_PROTECTED_TOOLS: &[&str] = &["skill"];
}

/// Placeholder spliced in for a pruned tool result. Mirrors opencode's
/// `[tool output compacted]` substitution.
pub const PRUNED_TOOL_OUTPUT_PLACEHOLDER: &str = "[tool output compacted]";
