//! Compaction algorithm — ported from openwarp's
//! `app/src/ai/byop_compaction/algorithm.rs`, itself a 1:1 port of opencode
//! `packages/opencode/src/session/compaction.ts`.
//!
//! The algorithm is decoupled from concrete message types via the
//! [`MessageRef`] trait. See `wire.rs` for the local-provider implementation
//! over `warp_multi_agent_api::Message`.
//!
//! Phase A only uses [`prune_decisions`]. [`select`] / [`turns`] /
//! [`split_turn`] are kept for the Phase B head/tail summarization path and
//! exercised by the unit tests so the algorithm doesn't bit-rot before the
//! follow-up.

use std::hash::Hash;

use super::consts::{PRUNE_MINIMUM, PRUNE_PROTECT, PRUNE_PROTECTED_TOOLS};

/// Message role for turn detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// A single tool output (one entry per `Role::Tool` message in our world).
#[derive(Debug, Clone)]
pub struct ToolOutputRef<CallId> {
    pub call_id: CallId,
    pub tool_name: String,
    /// Estimated token count (opencode `Token.estimate(part.state.output)`).
    pub output_size: usize,
    pub completed: bool,
    /// Already marked compacted; encountering one signals to stop walking
    /// further back (we've reached prior compaction's frontier).
    pub already_compacted: bool,
}

/// Abstract message reference. The algorithm only interacts with this trait.
pub trait MessageRef {
    type Id: Clone + Eq + Hash;
    type CallId: Clone + Eq + Hash;

    fn id(&self) -> Self::Id;
    fn role(&self) -> Role;

    /// True if this user message was synthesized as a compaction trigger
    /// marker (opencode `parts.some(p => p.type === "compaction")`). For
    /// Phase A — which stores no compaction state — this is always false.
    fn is_compaction_marker(&self) -> bool;

    /// True if this assistant message is itself a stored summary
    /// (opencode `info.summary === true`). Phase A: always false.
    fn is_summary(&self) -> bool;

    /// Per-message token estimate.
    fn estimate_size(&self) -> usize;

    /// Tool outputs carried by this message (only `Role::Tool` will return
    /// non-empty in the warp wire format).
    fn tool_outputs(&self) -> Vec<ToolOutputRef<Self::CallId>>;
}

/// Maps to `compaction.ts:76-80`.
#[derive(Debug, Clone)]
pub struct Turn<Id> {
    pub start: usize,
    pub end: usize,
    pub id: Id,
}

/// Maps to `compaction.ts:82-85`.
#[derive(Debug, Clone)]
pub struct Tail<Id> {
    pub start: usize,
    pub id: Id,
}

/// Result of [`select`]: `head_end` is the head/tail boundary,
/// `tail_start_id` identifies the first preserved message.
#[derive(Debug, Clone)]
pub struct SelectResult<Id> {
    pub head_end: usize,
    pub tail_start_id: Option<Id>,
}

/// `compaction.ts:141-157`.
pub fn turns<M: MessageRef>(messages: &[M]) -> Vec<Turn<M::Id>> {
    let mut result: Vec<Turn<M::Id>> = Vec::new();
    let n = messages.len();
    for (i, msg) in messages.iter().enumerate() {
        if msg.role() != Role::User {
            continue;
        }
        if msg.is_compaction_marker() {
            continue;
        }
        result.push(Turn {
            start: i,
            end: n,
            id: msg.id(),
        });
    }
    let len = result.len();
    if len > 1 {
        for i in 0..len - 1 {
            result[i].end = result[i + 1].start;
        }
    }
    result
}

/// `compaction.ts:159-182` splitTurn — find the first split point inside a
/// turn that fits within `budget`.
fn split_turn<M, EstFn>(
    messages: &[M],
    turn: &Turn<M::Id>,
    budget: usize,
    estimate: &EstFn,
) -> Option<Tail<M::Id>>
where
    M: MessageRef,
    EstFn: Fn(&[M]) -> usize,
{
    if budget == 0 {
        return None;
    }
    if turn.end.saturating_sub(turn.start) <= 1 {
        return None;
    }
    let mut start = turn.start + 1;
    while start < turn.end {
        let size = estimate(&messages[start..turn.end]);
        if size > budget {
            start += 1;
            continue;
        }
        return Some(Tail {
            start,
            id: messages[start].id(),
        });
    }
    None
}

/// `compaction.ts:244-293` select. Returns the head/tail boundary used by the
/// summarization path (Phase B). Phase A doesn't call this in production.
#[allow(dead_code)]
pub fn select<M, EstFn>(
    messages: &[M],
    tail_turns: usize,
    preserve_recent_budget: usize,
    estimate_slice: EstFn,
) -> SelectResult<M::Id>
where
    M: MessageRef,
    EstFn: Fn(&[M]) -> usize,
{
    if tail_turns == 0 {
        return SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        };
    }
    let all = turns(messages);
    if all.is_empty() {
        return SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        };
    }
    let recent_start = all.len().saturating_sub(tail_turns);
    let recent: Vec<&Turn<M::Id>> = all[recent_start..].iter().collect();
    let sizes: Vec<usize> = recent
        .iter()
        .map(|t| estimate_slice(&messages[t.start..t.end]))
        .collect();

    let mut total: usize = 0;
    let mut keep: Option<Tail<M::Id>> = None;
    for i in (0..recent.len()).rev() {
        let turn = recent[i];
        let size = sizes[i];
        if total + size <= preserve_recent_budget {
            total += size;
            keep = Some(Tail {
                start: turn.start,
                id: turn.id.clone(),
            });
            continue;
        }
        let remaining = preserve_recent_budget.saturating_sub(total);
        let split = split_turn(messages, turn, remaining, &estimate_slice);
        if split.is_some() {
            keep = split;
        }
        // opencode: once a turn overshoots, don't try earlier turns even if
        // splitTurn failed.
        break;
    }

    match keep {
        None => SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        },
        Some(t) if t.start == 0 => SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        },
        Some(t) => SelectResult {
            head_end: t.start,
            tail_start_id: Some(t.id),
        },
    }
}

/// `compaction.ts:297-341` prune. Returns the `(message_id, tool_call_id)`
/// pairs whose tool output should be replaced with a short placeholder.
///
/// Walks the message list back-to-front. Skips the two most recent user
/// turns, accumulates total token estimates of completed tool outputs, and
/// once the accumulator passes [`PRUNE_PROTECT`] starts marking older outputs
/// for prune. If the total prunable bytes don't exceed [`PRUNE_MINIMUM`] the
/// caller gets an empty Vec back (don't bother rewriting the request).
pub fn prune_decisions<M: MessageRef>(messages: &[M]) -> Vec<(M::Id, M::CallId)> {
    let mut total: usize = 0;
    let mut pruned: usize = 0;
    let mut to_prune: Vec<(M::Id, M::CallId)> = Vec::new();
    let mut user_turns_seen: usize = 0;

    'outer: for msg in messages.iter().rev() {
        if msg.role() == Role::User {
            user_turns_seen += 1;
        }
        // Always keep the latest two user turns intact (opencode `if (turns < 2) continue`).
        if user_turns_seen < 2 {
            continue;
        }
        // Reached a prior summary — earlier history was already replaced.
        if msg.role() == Role::Assistant && msg.is_summary() {
            break 'outer;
        }
        let outputs = msg.tool_outputs();
        for tp in outputs.into_iter().rev() {
            if !tp.completed {
                continue;
            }
            if PRUNE_PROTECTED_TOOLS.contains(&tp.tool_name.as_str()) {
                continue;
            }
            if tp.already_compacted {
                break 'outer;
            }
            let estimate = tp.output_size;
            total += estimate;
            if total <= PRUNE_PROTECT {
                continue;
            }
            pruned += estimate;
            to_prune.push((msg.id(), tp.call_id));
        }
    }

    if pruned > PRUNE_MINIMUM {
        to_prune
    } else {
        Vec::new()
    }
}
