//! Glue between the framework-agnostic [`super::algorithm`] and our
//! `warp_multi_agent_api::Message` proto messages.
//!
//! Phase A path: see [`apply_prune`].

use std::collections::{HashMap, HashSet};

use warp_multi_agent_api as api;

use super::algorithm::{prune_decisions, MessageRef, Role, ToolOutputRef};
use super::consts::CHARS_PER_TOKEN;
use super::PRUNED_TOOL_OUTPUT_PLACEHOLDER;

/// `tool_call_id → tool_name` index used by the prune decision to skip
/// protected tools (e.g. `skill`).
pub type ToolNameLookup = HashMap<String, String>;

/// Build a `tool_call_id → tool_name` map from the conversation history.
/// Mirrors openwarp's `build_tool_name_lookup`.
pub fn build_tool_name_lookup<'a, I>(messages: I) -> ToolNameLookup
where
    I: IntoIterator<Item = &'a api::Message>,
{
    let mut out = ToolNameLookup::new();
    for msg in messages {
        if let Some(api::message::Message::ToolCall(tc)) = &msg.message {
            let name = match tc.tool.as_ref() {
                Some(api::message::tool_call::Tool::ReadSkill(_)) => "skill",
                _ => "",
            };
            out.insert(tc.tool_call_id.clone(), name.to_string());
        }
    }
    out
}

/// Per-message [`MessageRef`] view over a borrowed `api::Message`. Phase A
/// has no `CompactionState`, so the trait methods that consult it always
/// return false.
#[derive(Clone, Copy)]
struct WireMsg<'a> {
    msg: &'a api::Message,
    tool_names: &'a ToolNameLookup,
}

fn estimate_size_chars(msg: &api::Message) -> usize {
    use api::message::Message as M;
    let chars = msg
        .message
        .as_ref()
        .map(|inner| match inner {
            M::UserQuery(u) => u.query.chars().count(),
            M::AgentOutput(a) => a.text.chars().count(),
            M::AgentReasoning(r) => r.reasoning.chars().count(),
            M::ToolCall(_) => msg.server_message_data.chars().count().max(64),
            M::ToolCallResult(tcr) => {
                let from_oneof = tcr
                    .result
                    .as_ref()
                    .map(|r| format!("{r:?}").chars().count())
                    .unwrap_or(0);
                from_oneof
                    .max(msg.server_message_data.chars().count())
                    .max(32)
            }
            _ => 0,
        })
        .unwrap_or(0);
    (chars + CHARS_PER_TOKEN / 2) / CHARS_PER_TOKEN
}

impl<'a> MessageRef for WireMsg<'a> {
    type Id = String;
    type CallId = String;

    fn id(&self) -> String {
        self.msg.id.clone()
    }

    fn role(&self) -> Role {
        use api::message::Message as M;
        match &self.msg.message {
            Some(M::UserQuery(_)) => Role::User,
            Some(M::ToolCallResult(_)) => Role::Tool,
            _ => Role::Assistant,
        }
    }

    fn is_compaction_marker(&self) -> bool {
        false
    }

    fn is_summary(&self) -> bool {
        false
    }

    fn estimate_size(&self) -> usize {
        estimate_size_chars(self.msg)
    }

    fn tool_outputs(&self) -> Vec<ToolOutputRef<String>> {
        let Some(api::message::Message::ToolCallResult(tcr)) = &self.msg.message else {
            return Vec::new();
        };
        let tool_name = self
            .tool_names
            .get(&tcr.tool_call_id)
            .cloned()
            .unwrap_or_default();
        let output_size = estimate_size_chars(self.msg);
        vec![ToolOutputRef {
            call_id: tcr.tool_call_id.clone(),
            tool_name,
            output_size,
            completed: tcr.result.is_some() || !self.msg.server_message_data.is_empty(),
            already_compacted: false,
        }]
    }
}

/// Run [`prune_decisions`] over a slice of conversation tasks (each task's
/// proto messages, in order) and return the set of `tool_call_id`s whose
/// content should be replaced with a placeholder when the OpenAI body is
/// built.
pub fn compute_prune_set(tasks: &[api::Task]) -> HashSet<String> {
    let flat: Vec<&api::Message> = tasks.iter().flat_map(|t| t.messages.iter()).collect();
    if flat.is_empty() {
        return HashSet::new();
    }
    let tool_names = build_tool_name_lookup(flat.iter().copied());
    let views: Vec<WireMsg<'_>> = flat
        .iter()
        .map(|m| WireMsg {
            msg: *m,
            tool_names: &tool_names,
        })
        .collect();
    prune_decisions(&views)
        .into_iter()
        .map(|(_msg_id, call_id)| call_id)
        .collect()
}

/// In-place: replace the `content` of every Tool-role `ChatMessage` whose
/// `tool_call_id` is in `prune_set` with [`PRUNED_TOOL_OUTPUT_PLACEHOLDER`].
/// No-op when `prune_set` is empty.
pub fn apply_prune(messages: &mut [crate::local_provider::wire::ChatMessage], prune_set: &HashSet<String>) {
    if prune_set.is_empty() {
        return;
    }
    use crate::local_provider::wire::Role as ChatRole;
    for m in messages.iter_mut() {
        if !matches!(m.role, ChatRole::Tool) {
            continue;
        }
        let id = match &m.tool_call_id {
            Some(id) => id,
            None => continue,
        };
        if prune_set.contains(id) {
            m.content = Some(PRUNED_TOOL_OUTPUT_PLACEHOLDER.to_string());
        }
    }
}
