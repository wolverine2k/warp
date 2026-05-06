//! Compaction configuration. Phase A: defaults only — no `AISettings` hookup.
//!
//! Phase B will add user-facing knobs (`byop_compaction_*` settings) and a
//! `from_settings(&AppContext)` constructor mirroring openwarp's.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionConfig {
    /// Whether tool-output prune fires. Default true.
    pub prune: bool,
    /// Reserved for Phase B select() — number of recent user turns to keep
    /// intact. Phase A doesn't summarize, so this is unused but tracked so
    /// the type doesn't churn between phases.
    pub tail_turns: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            prune: true,
            tail_turns: super::consts::DEFAULT_TAIL_TURNS,
        }
    }
}
