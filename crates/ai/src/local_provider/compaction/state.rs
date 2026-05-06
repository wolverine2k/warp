//! Compaction sidecar state — hangs off `AIConversation`, decoupled from the
//! `warp_multi_agent_api::Message` proto.
//!
//! The proto type is owned by an external dependency, so we can't add
//! `is_summary` / `compacted` flags to it. Instead this sidecar uses a
//! `message_id`-keyed map to hold per-message compaction metadata.
//!
//! [`CompactionState::VERSION`] is bumped manually on schema evolution.
//! Deserialization failures degrade to `Default` (== "never compacted").
//!
//! Ported 1:1 from openwarp's `app/src/ai/byop_compaction/state.rs`, which
//! mirrors opencode `packages/opencode/src/session/compaction.ts`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// What triggered the compaction. `Auto` fires on token-overflow, `Manual`
/// fires from a user `/compact` slash command (Phase B-4).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactionTrigger {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageMarker {
    /// This assistant message is itself a summary; its content is used to
    /// stand in for the prior history when building the request.
    #[serde(default)]
    pub is_summary: bool,
    /// This user message was synthesized as a compaction-trigger marker
    /// (opencode `parts.some(p => p.type === "compaction")`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_trigger: Option<CompactionTrigger>,
    /// This `ToolCallResult`'s output has been pruned; the request body
    /// substitutes a placeholder. Unix epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output_compacted_at: Option<u64>,
    /// Synthetic "Continue..." user message generated on the auto-resume
    /// path (opencode `metadata.compaction_continue`).
    #[serde(default)]
    pub synthetic_continue: bool,
}

/// One completed compaction interval (opencode `completedCompactions()`).
///
/// `user_msg_id` is the user message that triggered the summary (carries the
/// `compaction_trigger` marker), `assistant_msg_id` is the synthetic
/// `AgentOutput` message holding the summary text. Both appear in
/// [`CompactionState::hidden_message_ids`] — they're projected out, with the
/// summary text taking their place at the head of the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedCompaction {
    pub user_msg_id: String,
    pub assistant_msg_id: String,
    /// First message id of the preserved tail (sanity / debug).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_start_id: Option<String>,
    /// Cached summary text (also lives on the assistant message; copied
    /// here so `build_prompt` can grab `previous_summary` cheaply).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
    pub auto: bool,
    pub overflow: bool,
}

/// Sidecar state carried alongside `AIConversation`.
///
/// Default = empty = unaffected. Pure value type; safe to clone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionState {
    /// Schema version. Bump on serialization-breaking changes.
    #[serde(default = "CompactionState::current_version")]
    pub version: u32,
    #[serde(default)]
    markers: HashMap<String, MessageMarker>,
    #[serde(default)]
    completed: Vec<CompletedCompaction>,
}

impl Default for CompactionState {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            markers: HashMap::new(),
            completed: Vec::new(),
        }
    }
}

impl CompactionState {
    pub const VERSION: u32 = 1;
    fn current_version() -> u32 {
        Self::VERSION
    }

    pub fn marker(&self, msg_id: &str) -> Option<&MessageMarker> {
        self.markers.get(msg_id)
    }

    /// Merge into an existing marker (does not overwrite the whole thing).
    pub fn upsert_marker(&mut self, msg_id: impl Into<String>, f: impl FnOnce(&mut MessageMarker)) {
        let entry = self.markers.entry(msg_id.into()).or_default();
        f(entry);
    }

    /// Flag a `ToolCallResult` whose payload has been pruned.
    pub fn mark_tool_compacted(&mut self, msg_id: impl Into<String>, now_ms: u64) {
        self.upsert_marker(msg_id, |m| m.tool_output_compacted_at = Some(now_ms));
    }

    /// Push a completed compaction interval.
    pub fn push_completed(&mut self, c: CompletedCompaction) {
        // Mark both the trigger user message and the resulting summary
        // assistant message so projection can identify each individually.
        self.upsert_marker(c.user_msg_id.clone(), |m| {
            m.compaction_trigger = Some(if c.auto {
                CompactionTrigger::Auto
            } else {
                CompactionTrigger::Manual
            });
        });
        self.upsert_marker(c.assistant_msg_id.clone(), |m| m.is_summary = true);
        self.completed.push(c);
    }

    /// Flag a synthetic "Continue..." user message (auto+overflow path).
    pub fn mark_synthetic_continue(&mut self, msg_id: impl Into<String>) {
        self.upsert_marker(msg_id, |m| m.synthetic_continue = true);
    }

    /// Last completed compaction's summary text (for incremental summary
    /// anchoring in [`super::prompt::build_prompt`] — Phase B-3).
    #[allow(dead_code)]
    pub fn previous_summary(&self) -> Option<&str> {
        self.completed
            .last()
            .and_then(|c| c.summary_text.as_deref())
    }

    #[allow(dead_code)]
    pub fn completed(&self) -> &[CompletedCompaction] {
        &self.completed
    }

    /// All message ids that should be hidden from the request body
    /// (opencode `hidden`): every completed compaction's user_msg_id +
    /// assistant_msg_id. Note: the *summary content itself* is spliced back
    /// in by the projection step; this set only describes which originals
    /// to skip.
    #[allow(dead_code)]
    pub fn hidden_message_ids(&self) -> HashSet<String> {
        self.completed
            .iter()
            .flat_map(|c| [c.user_msg_id.clone(), c.assistant_msg_id.clone()])
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn marker_count(&self) -> usize {
        self.markers.len()
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn cc(uid: &str, aid: &str, auto: bool) -> CompletedCompaction {
        CompletedCompaction {
            user_msg_id: uid.to_string(),
            assistant_msg_id: aid.to_string(),
            tail_start_id: None,
            summary_text: Some(format!("summary-{aid}")),
            auto,
            overflow: false,
        }
    }

    #[test]
    fn push_completed_marks_both_messages() {
        let mut s = CompactionState::default();
        s.push_completed(cc("u1", "a1", true));
        assert_eq!(
            s.marker("u1").unwrap().compaction_trigger,
            Some(CompactionTrigger::Auto)
        );
        assert!(s.marker("a1").unwrap().is_summary);
    }

    #[test]
    fn previous_summary_returns_last() {
        let mut s = CompactionState::default();
        s.push_completed(cc("u1", "a1", false));
        s.push_completed(cc("u2", "a2", false));
        assert_eq!(s.previous_summary(), Some("summary-a2"));
    }

    #[test]
    fn hidden_message_ids_covers_all_completed() {
        let mut s = CompactionState::default();
        s.push_completed(cc("u1", "a1", false));
        s.push_completed(cc("u2", "a2", false));
        let h = s.hidden_message_ids();
        assert!(h.contains("u1"));
        assert!(h.contains("a1"));
        assert!(h.contains("u2"));
        assert!(h.contains("a2"));
        assert_eq!(h.len(), 4);
    }

    #[test]
    fn upsert_marker_merges() {
        let mut s = CompactionState::default();
        s.upsert_marker("m1", |m| m.is_summary = true);
        s.upsert_marker("m1", |m| m.synthetic_continue = true);
        let m = s.marker("m1").unwrap();
        assert!(m.is_summary);
        assert!(m.synthetic_continue);
        assert_eq!(s.marker_count(), 1);
    }

    #[test]
    fn default_serializable_roundtrip() {
        let s = CompactionState::default();
        let j = serde_json::to_string(&s).unwrap();
        let back: CompactionState = serde_json::from_str(&j).unwrap();
        assert_eq!(back.version, CompactionState::VERSION);
        assert!(back.completed.is_empty());
    }
}
