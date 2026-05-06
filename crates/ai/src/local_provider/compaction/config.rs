//! Compaction configuration. Phase A consumes only `prune`; the remaining
//! fields are populated by Phase B's settings hookup and used by `select`
//! once the summarization path lands.
//!
//! The library type is AppContext-free; the app-side
//! `local_provider_config::compaction_config_from_app` reads
//! `AISettings.local_provider_compaction_*` and produces this struct.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionConfig {
    /// Auto-trigger summarization on token-overflow. Phase B (B-3) only.
    /// Phase A ignores this field.
    pub auto: bool,
    /// Whether tool-output prune fires. Default true.
    pub prune: bool,
    /// Number of recent user turns to keep verbatim in the tail. Used by
    /// `select` (Phase B-3). 0 disables tail splitting.
    pub tail_turns: usize,
    /// Force-override the `preserve_recent_budget` formula. `None` means
    /// `min(MAX_PRESERVE_RECENT_TOKENS, max(MIN_PRESERVE_RECENT_TOKENS, usable / 4))`.
    pub preserve_recent_tokens: Option<usize>,
    /// Force-override the reserved buffer used by `usable()`. `None` means
    /// `min(20_000, max_output)`.
    pub reserved: Option<usize>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto: true,
            prune: true,
            tail_turns: super::consts::DEFAULT_TAIL_TURNS,
            preserve_recent_tokens: None,
            reserved: None,
        }
    }
}

impl CompactionConfig {
    /// Mirrors opencode `compaction.ts:134-139`:
    /// `cfg.preserve_recent_tokens ?? min(MAX, max(MIN, floor(usable / 4)))`.
    /// Used by [`super::algorithm::select`] in Phase B-3.
    #[allow(dead_code)]
    pub fn preserve_recent_budget(&self, usable_tokens: usize) -> usize {
        use super::consts::{MAX_PRESERVE_RECENT_TOKENS, MIN_PRESERVE_RECENT_TOKENS};
        self.preserve_recent_tokens.unwrap_or_else(|| {
            MAX_PRESERVE_RECENT_TOKENS.min(MIN_PRESERVE_RECENT_TOKENS.max(usable_tokens / 4))
        })
    }
}
