//! Tests for the compaction algorithm. Phase A focuses on `prune_decisions`;
//! `select` / `turns` are also exercised lightly so they don't bit-rot before
//! Phase B picks them up.

use std::collections::HashMap;

use super::algorithm::{prune_decisions, turns, MessageRef, Role, ToolOutputRef};
use super::consts::PRUNE_PROTECT;

/// Tiny in-memory message used by these tests so we don't have to construct
/// proto messages.
#[derive(Clone)]
struct FakeMsg {
    id: &'static str,
    role: Role,
    summary: bool,
    compaction_marker: bool,
    tool_outputs: Vec<ToolOutputRef<&'static str>>,
}

impl MessageRef for FakeMsg {
    type Id = &'static str;
    type CallId = &'static str;
    fn id(&self) -> Self::Id {
        self.id
    }
    fn role(&self) -> Role {
        self.role
    }
    fn is_compaction_marker(&self) -> bool {
        self.compaction_marker
    }
    fn is_summary(&self) -> bool {
        self.summary
    }
    fn estimate_size(&self) -> usize {
        self.tool_outputs.iter().map(|t| t.output_size).sum()
    }
    fn tool_outputs(&self) -> Vec<ToolOutputRef<Self::CallId>> {
        self.tool_outputs.clone()
    }
}

fn user(id: &'static str) -> FakeMsg {
    FakeMsg {
        id,
        role: Role::User,
        summary: false,
        compaction_marker: false,
        tool_outputs: vec![],
    }
}

fn assistant(id: &'static str) -> FakeMsg {
    FakeMsg {
        id,
        role: Role::Assistant,
        summary: false,
        compaction_marker: false,
        tool_outputs: vec![],
    }
}

fn tool(id: &'static str, call_id: &'static str, output_size: usize) -> FakeMsg {
    FakeMsg {
        id,
        role: Role::Tool,
        summary: false,
        compaction_marker: false,
        tool_outputs: vec![ToolOutputRef {
            call_id,
            tool_name: "run_shell_command".to_string(),
            output_size,
            completed: true,
            already_compacted: false,
        }],
    }
}

#[test]
fn prune_skips_when_history_below_protect() {
    // 1 user turn + tool output well under PRUNE_PROTECT — nothing to do.
    let msgs = vec![user("u1"), assistant("a1"), tool("t1", "c1", 1_000)];
    let decisions = prune_decisions(&msgs);
    assert!(decisions.is_empty(), "expected no prune, got {decisions:?}");
}

#[test]
fn prune_keeps_two_most_recent_user_turns_intact() {
    // 3 user turns. The latest two must remain untouched even if their tool
    // output is huge.
    let msgs = vec![
        user("u1"),
        tool("t1", "c1", PRUNE_PROTECT * 2),
        user("u2"),
        tool("t2", "c2", PRUNE_PROTECT * 2),
        user("u3"),
        tool("t3", "c3", PRUNE_PROTECT * 2),
    ];
    let decisions = prune_decisions(&msgs);
    let pruned: HashMap<&'static str, &'static str> =
        decisions.into_iter().map(|(m, c)| (m, c)).collect();
    assert!(!pruned.contains_key("t2"), "u2's tool must not be pruned");
    assert!(!pruned.contains_key("t3"), "u3's tool must not be pruned");
    assert!(
        pruned.contains_key("t1"),
        "u1's tool should be pruned once we exceed PRUNE_PROTECT"
    );
}

#[test]
fn prune_stops_at_summary_boundary() {
    // Walking back, a summary marker should halt the search — anything older
    // is considered already-compacted history.
    let mut summary = assistant("a-summary");
    summary.summary = true;
    let msgs = vec![
        user("u-old"),
        tool("t-old", "c-old", PRUNE_PROTECT * 5),
        summary,
        user("u1"),
        tool("t1", "c1", PRUNE_PROTECT * 2),
        user("u2"),
        tool("t2", "c2", 100),
        user("u3"),
    ];
    let decisions = prune_decisions(&msgs);
    let ids: Vec<&'static str> = decisions.iter().map(|(_, c)| *c).collect();
    assert!(
        !ids.contains(&"c-old"),
        "must not prune past the summary, got {ids:?}"
    );
}

#[test]
fn prune_skips_protected_tools() {
    let mut t = tool("t1", "c1", PRUNE_PROTECT * 5);
    t.tool_outputs[0].tool_name = "skill".to_string();
    let msgs = vec![user("u1"), t, user("u2"), user("u3")];
    let decisions = prune_decisions(&msgs);
    assert!(
        decisions.is_empty(),
        "skill tool is protected; got {decisions:?}"
    );
}

#[test]
fn turns_collapses_consecutive_non_user_messages() {
    let msgs = vec![
        user("u1"),
        assistant("a1"),
        tool("t1", "c1", 10),
        user("u2"),
        assistant("a2"),
    ];
    let ts = turns(&msgs);
    assert_eq!(ts.len(), 2);
    assert_eq!(ts[0].id, "u1");
    assert_eq!(ts[0].start, 0);
    assert_eq!(ts[0].end, 3, "first turn ends where second user starts");
    assert_eq!(ts[1].id, "u2");
    assert_eq!(ts[1].start, 3);
    assert_eq!(ts[1].end, msgs.len());
}
