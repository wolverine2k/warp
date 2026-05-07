# Phase B-6 — Local LLM Provider multi-turn agent loop

The single-turn dispatch + SSE adapter + compaction work landed in Phases
A through B-5 on `nmehta/local-llm-provider`. End-to-end testing against
NVIDIA's inference API (`https://inference-api.nvidia.com/v1`, model
`openai/openai/gpt-5.5`) surfaced **three structural bugs in the
multi-turn agent loop integration** that make the model look "dumb"
even though the dispatch fork itself works.

This file is a hand-off note for the next focused session.

## Reproduction

1. Configure the local provider in Settings → AI → Local LLM Provider
   with the NVIDIA endpoint above and `supports_tools = true`.
2. Launch `target/debug/warp-oss` with
   `WARP_LOCAL_PROVIDER_DEBUG_DUMP=1` (env-gated diagnostic added in
   `e55d48d6` — see `crates/ai/src/local_provider/run.rs`).
3. In Agent Mode, ask anything that requires action — e.g.
   `"compile and create an apple dmg"`.
4. After the model responds, inspect:
   - `/tmp/warp-local-provider-last-request.json` — the exact JSON body
     we sent on the most recent turn.
   - `/tmp/warp-local-provider-last-response.log` — every SSE chunk
     the upstream returned.

## What the captured request shows

The body has these properties on every follow-up turn:

1. **`"tools": null`** — the request advertises no tools to the model.
   The system prompt expansion even confirms this with the literal
   line `"No tools are currently available; respond with plain text."`
2. **Every `"role": "tool"` message has
   `"content": "(tool result not available)"`** — i.e. the placeholder
   from `compose_chat_completion_request::backfill_orphaned_tool_calls`
   is hit on every tool call. The actual shell-command output never
   reaches the model.
3. **Zero `"role": "user"` messages** in the entire `messages` array.
   The user's original prompt and every subsequent prompt are absent.
   Only system + assistant tool_calls + assistant text + (placeholder)
   tool messages.

A capable SOTA model receiving "no tools, all tool calls returned
nothing, no user query" rationally asks "what would you like me to do?"
on every response. That is not a model problem — that is exactly what
the model has been told to work with.

## Root cause synthesis

The local-provider work in Phases A through B-5 covered:
- Single-turn request translation (`compose_chat_completion_request`)
- SSE → `ResponseEvent` adapter (`OpenAiSseAdapter`)
- Compaction (overflow / select / summarize / commit / projection)

What it did **not** cover:
- The *agent loop* — the controller-side machinery that, after the
  model emits a `Message::ToolCall`, registers it as a pending action
  in `BlocklistAIActionModel`, executes the tool, captures the
  result, and threads the result back into the next turn's
  `RequestParams.input` as `AIAgentInput::ActionResult`.

Today, for warp.dev conversations, this loop runs via warp.dev's
own server-side orchestration. For local-provider conversations there
is **no analogous client-side wiring**. As a result:

- **User queries** are passed to the *first* turn's request via
  `RequestParams.input` and rendered through `extract_latest_user_query`
  / `LocalProviderInput.user_query`. They are **not** persisted into
  `task.messages`, so on the *next* turn (a follow-up triggered by an
  action result) `params.input` lacks a `UserQuery` entry, and the
  request body has no user message at all.
- **Tool calls** emitted by the local SSE adapter land in
  `task.messages` as `Message::ToolCall` (so they appear in the next
  request's history), but the controller does **not** register them in
  `action_model` as pending actions. The execution path that would
  produce an `AIAgentActionResult` is never engaged.
- **Tool results** therefore never enter
  `RequestInput::for_actions_results` →
  `AIAgentInput::ActionResult` → `params.input` →
  `collect_action_results` map. `compose_chat_completion_request`
  calls `backfill_orphaned_tool_calls` and emits the
  `"(tool result not available)"` placeholder on every assistant
  `tool_calls` message.
- The empty `tools` array is a separate, smaller issue: on follow-up
  turns the controller's `RequestInput::for_actions_results` path
  does not preserve `supported_tools`, so `enabled_local_tools`
  filters an empty list and `tools` is `None`. (The first turn does
  populate it, but no first-turn request exists in the user's
  captured body because the relevant turn is a follow-up.)

## Fix plan — five ordered steps

### Step 1: persist user queries in `task.messages`

When `BlocklistAIController::send_request_input` accepts a
`RequestInput` containing `AIAgentInput::UserQuery`, the standard
flow records it through `BlocklistAIHistoryModel::update_conversation_for_new_request_input`
which delegates to `AIConversation::update_for_new_request_input`. We
need to verify that path actually produces a `Message::UserQuery` in
`task.messages` for local-provider conversations. It probably does —
the captured-body absence is then a downstream filter problem.

If it does, the bug is more likely that
`api/impl.rs::route_to_local_provider`'s `extract_latest_user_query`
is the *only* place we surface user content, and it pulls from the
ephemeral `params.input` (which only has the *latest* `UserQuery`),
ignoring the `task.messages` history that already has older ones.
The fix is to **drop `extract_latest_user_query` and rely entirely on
`task.messages` history rendering** — the user's queries are already
in there as `Message::UserQuery` (after Step 1 verification), and
`compose_chat_completion_request::push_history_messages` already
handles the rendering.

Concretely:
- `api/impl.rs::route_to_local_provider`: remove `user_query` plumbing.
- `compose_chat_completion_request`: drop the `if let Some(q) =
  input.user_query { … push at end }` block.
- `LocalProviderInput.user_query`: remove the field.

### Step 2: register `Message::ToolCall` as a pending action

Find where warp.dev's path registers `BlocklistAIActionModel` pending
actions when the response stream emits a `ToolCall`. Mirror that for
the local-provider path so the controller knows to execute the tool
and produce an `AIAgentActionResult`.

This likely lives in
`app/src/ai/blocklist/controller/response_stream.rs` or
`controller.rs`. The relevant entrypoint is the
`ResponseEvent::ClientActions` handler that sees
`Action::AddMessagesToTask` containing a
`Message::ToolCall`. For warp.dev that path probably already
registers a pending action; if so, our local-provider events go
through the same handler and the question becomes "why doesn't it
fire" — possibly because the `tool_call_id` shape differs, or the
`Message::ToolCall.tool` proto variant the local adapter emits is
unrecognized.

Check the variant: today the SSE adapter in
`crates/ai/src/local_provider/response.rs` emits typed tool variants
via parser logic in the same crate. Confirm those variants are the
ones `action_model::preprocess` recognizes; if not, add the missing
mapping.

### Step 3: thread action results back

Once Step 2 produces `AIAgentActionResult`s, the existing
`send_follow_up_for_conversation` →
`RequestInput::for_actions_results` →
`RequestParams.input` →
`collect_action_results` chain should work without changes. The
captured-body proof: the request body already contains assistant
`tool_calls` followed by `role: "tool"` placeholders — meaning
`backfill_orphaned_tool_calls` is firing because no real result is
present. With Step 2 producing real results, the
`action_results` map populates, and the placeholder path no longer
triggers.

### Step 4: preserve `supported_tools` on follow-up turns

`RequestInput::for_actions_results` does not set
`supported_tools_override`, so `RequestParams.supported_tools_override`
is `None` and `route_to_local_provider` falls back to
`get_supported_tools(&params)`. That should work — but the captured
request shows `"tools": null`. So either:
- `cfg.supports_tools` is `false` (the user confirmed it is `true` in
  settings, so unlikely)
- `enabled_local_tools(get_supported_tools(&params), cfg)` returns
  empty for some reason

Add a `log::debug!` at request build time that prints
`supported_tools.len()` and `local_tools.len()`. If
`supported_tools.len() > 0` but `local_tools.len() == 0`, the bug is
in `LocalTool::from_name` not matching the `ToolType` proto variants
emitted by `get_supported_tools` for a follow-up turn.

### Step 5: tests

- **Unit**: extend `compose_chat_completion_request` tests with a
  multi-turn task fixture: `[user query, assistant tool_call, tool
  result]` × N, plus a fresh user query. Assert all user messages
  appear, no `"(tool result not available)"` placeholders are
  emitted, and `tools` is non-`None`.
- **Integration**: in
  `crates/ai/tests/local_provider_integration.rs`, add a multi-turn
  test that drives `run_chat_turn` against a mock OpenAI server
  scripted to return a tool call on turn 1 and a final answer on
  turn 2. Assert the request body for turn 2 contains:
  - The user's original query as a `role: "user"` message
  - The actual tool result (not the placeholder) as a `role: "tool"`
    message
  - A non-null `tools` array

## Useful breadcrumbs from this session

- `crates/ai/src/local_provider/run.rs` — `WARP_LOCAL_PROVIDER_DEBUG_DUMP`
  env writes the outbound JSON body to
  `/tmp/warp-local-provider-last-request.json` per turn (overwrites)
  and each inbound SSE chunk to
  `/tmp/warp-local-provider-last-response.log` (truncated per turn).
- `app/src/ai/agent/api/impl.rs` — `generate_multi_agent_output`
  dispatch fork; `route_to_local_provider` translates
  `RequestParams` → `LocalProviderInput`.
- `app/src/ai/blocklist/controller.rs::send_follow_up_for_conversation`
  is the agent-loop trigger (line 1411). It pulls finished results
  via `action_model.drain_finished_action_results`. If no results
  drain, no follow-up fires.
- `app/src/ai/blocklist/action_model/execute.rs` is where pending
  actions actually run; `preprocess.rs` is where they're recognized.
- The captured request body the user observed had:
  - 1 `system`, 16 `assistant`, 9 `tool`, **0 `user`** messages
  - `tools: null`
  - All 9 `tool` messages: `"content": "(tool result not available)"`
  - `model: "openai/openai/gpt-5.5"`
  - `usage: { prompt_tokens: 1643, completion_tokens: 332,
    total_tokens: 1975 }` (small — well under the upstream's limits;
    no token-budget pressure)

## Out of scope for B-6 (defer to later phases)

- Cross-restart persistence of `compaction_state` already landed in
  B-2a; nothing to revisit there.
- Smaller-window auto-compaction tuning (the FALLBACK 200K context
  is generous; users with explicitly small `context_window` settings
  get the right behaviour).
- Token-counts on `StreamFinished` for warp.dev conversations —
  that's separately tracked.

## Definition of done for B-6

1. A multi-turn local-provider conversation:
   - User asks a question requiring tools (e.g. "what's in
     `Cargo.toml`?")
   - Model emits `read_files` tool call → controller executes → result
     appears in the model's next turn → model gives a real answer
     using the file content
2. Request body for every follow-up turn contains:
   - A non-empty `tools` array
   - The user's original query as a `role: "user"` message
   - All prior tool results as `role: "tool"` messages with their
     real content (not the `"(tool result not available)"`
     placeholder)
3. No regression in the 309 ai-crate tests or the existing 18
   integration tests.
4. New unit + integration tests per Step 5 pass.

## Branch state at hand-off

Branch `nmehta/local-llm-provider` is at:

```
e55d48d6 feat(ai/local_provider): add env-gated request/response body dump for diagnosis
c3443eb6 fix(server_api): suppress UpdateAgentTask "task owner not found" log noise
f6b608de fix(ai/local_provider): always route Agent Mode through local provider when configured
7ec7666e fix(ai/local_provider): tighten anti-acknowledge prompt + sync gate also catches mid-conversation switches
6ec39dcb fix(ai/blocklist): skip TaskStatusSyncModel sync for local-provider conversations
41756955 feat(ai/local_provider): persist compaction_state across restarts (Phase B-2a)
80a48a8d feat(ai/local_provider): /compact slash command + prune integration tests (Phase B-4 + B-5)
43ac0aad feat(ai/local_provider): wire auto-compaction into controller (Phase B-3a step 3)
5337ade0 feat(ai/local_provider): auto-compaction orchestrator + usage plumbing (Phase B-3a steps 1-2)
be1e0b09 feat(ai/local_provider): port head-summary compaction libraries (Phase B-3)
```

All pushed to `wolverine2k/warp`. Tests + clippy + fmt clean.
