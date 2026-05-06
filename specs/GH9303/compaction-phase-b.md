# Compaction Phase B — follow-up plan

Phase A landed in `crates/ai/src/local_provider/compaction/` and only runs the
read-only `prune_decisions` step. This file is the punch list for the next
follow-up so the work doesn't get forgotten.

## What Phase A already does

- `algorithm.rs` — full port of opencode `compaction.ts` (turns / split_turn /
  select / prune_decisions). `select` is `#[allow(dead_code)]` until B-3.
- `wire.rs::compute_prune_set(&[api::Task])` runs prune over the proto history
  and returns the set of `tool_call_id`s to drop.
- `wire.rs::apply_prune` mutates an OpenAI `ChatMessage` list in place,
  replacing pruned tool content with `[tool output compacted]`.
- `compose_chat_completion_request` calls the above when
  `LocalProviderInput.compaction_config.prune` is true (default).
- 5 unit tests in `algorithm_tests.rs` covering the core invariants.

## Phase B — incremental items

### B-1. `AISettings.byop_compaction_*` knobs

Today `CompactionConfig::default()` is the only constructor. Phase B should
add `from_settings(&AppContext)` mirroring openwarp:

- `byop_compaction_auto: bool` — auto-trigger summarization on overflow
- `byop_compaction_prune: bool` — toggle prune (Phase A already honours this
  via the field; just needs UI surface)
- `byop_compaction_tail_turns: u32` — tail length for `select`
- `byop_compaction_preserve_recent_tokens: u32` — override the default formula
- `byop_compaction_reserved: u32` — reserved buffer for `usable()`
- `byop_compaction_model_provider_id: String` + `..._model_id: String` —
  optional dedicated summarizer (falls back to the conversation's model)

Wire them through `app/src/settings/ai.rs` and the AI settings page (the
existing Custom Provider subpage from Phase 9 already has the layout to copy).

### B-2. `CompactionState` sidecar persistence

Port `state.rs` (markers + completed compactions) and hang it on
`AIConversation` in `app/src/ai/agent/conversation.rs`. Mirrors openwarp:

- Add `pub compaction_state: CompactionState` (default = empty = unaffected)
- Persist via the existing conversation serialization path
- `wire.rs` re-uses the real `CompactionState` instead of always-false stubs
  in the `MessageRef` impl, so prune correctly skips already-compacted output
  and `select` knows where the previous summary boundary lives

### B-3. `select`-driven head-summary path

With state in place, port openwarp's `commit.rs`:

1. Detect overflow via `overflow::is_overflow(usage, model_limit, cfg)` after
   each turn (response `StreamFinished.usage` has the token counts).
2. On overflow, call `select` to compute head/tail boundary.
3. Issue a separate summarization LLM call (use the configured
   `compaction_model` or fall back to the conversation's model).
4. On success, splice the resulting summary into the conversation as a
   synthetic `(user "Continue...", assistant <summary>)` pair, mark them in
   `CompactionState.markers`, and append a `CompletedCompaction` entry.

Open question: whether the summarizer call goes through the same SSE adapter
or a one-shot non-streaming path. Recommend non-streaming for simplicity —
we just need the final text.

#### B-3 status (delivered in this PR)

Library + projection layer is complete. Controller dispatch is the only
remaining piece — split out as B-3a so the library work can land first and
get reviewed in isolation.

Landed:

- `crates/ai/src/local_provider/compaction/overflow.rs` — `TokenCounts`,
  `ModelLimit::FALLBACK` / `from_context_window`, `is_overflow`, `usable`
  (1:1 byte-aligned with opencode `overflow.ts`).
- `crates/ai/src/local_provider/compaction/prompt.rs` — `SUMMARY_TEMPLATE`
  (byte-aligned), `build_prompt`, `build_continue_message`.
- `crates/ai/src/local_provider/compaction/commit.rs` — `commit_summarization`
  helper (decoupled from `AIConversation`; generates synthetic ids, pushes
  `CompletedCompaction`, returns the ids so the caller can splice matching
  `api::Message`s into the task list).
- `crates/ai/src/local_provider/run.rs::run_summarizer_turn` — non-streaming
  `Chat Completions` call that returns the assistant text, plus
  `build_summarizer_messages` convenience composer.
- `crates/ai/src/local_provider/wire.rs` — `ChatCompletionResponse`,
  `ResponseChoice`, `ResponseMessage` for the non-streaming response shape.
- `crates/ai/src/local_provider/request.rs` — projection step that drops
  pre-compaction history when `compaction_state.completed` is non-empty.
  Synthetic compaction pair (already in `tasks`) stands in as the new head.
- Unit tests per module (overflow: 13, prompt: 6, commit: 4) + 3 new
  integration tests (`summarizer_parses_non_streaming_json...`,
  `summarizer_surfaces_http_error_with_body_excerpt`,
  `next_turn_after_compaction_drops_pre_compaction_history`).

Deferred to B-3a:

- `StreamFinished.usage` plumbing — `OpenAiSseAdapter` doesn't yet capture
  the OpenAI-format `usage` chunk, so the controller has no token counts to
  hand to `is_overflow`. Need to (a) parse `usage` off the final `ChatCompletionChunk`
  (requires `stream_options: {"include_usage": true}` in the request body for
  servers that gate it) and (b) thread it onto `StreamFinished.usage`.
- Controller dispatch — the place that observes `Finished` for a local-provider
  conversation and decides whether to compact. Needs `&mut AIConversation`
  access to mutate `task_store` (splice synthetic `(user, assistant)` pair)
  and `compaction_state` (record the new `CompletedCompaction`). Plumbing
  the summarizer dispatch through the existing controller flow is the bulk
  of B-3a.
- End-to-end verification against a real OpenAI-compatible endpoint —
  requires the controller dispatch above to be wired before it's
  meaningful.

### B-4. `/compact` user command

Add a slash command in the input bar that triggers the same flow as B-3 but
unconditionally (`Manual` trigger instead of `Auto`). Mirrors opencode's
`/compact` and `/compact-and`.

### B-5. Integration tests

Extend `crates/ai/tests/local_provider_integration.rs` with:

- `compaction_prunes_old_tool_outputs_in_outbound_request` — drive a fake
  long history, send a turn, assert the mock server received placeholders
  for the old tool messages.
- `compaction_skipped_when_under_threshold` — sanity check that small
  conversations are unaffected.
- `compaction_summary_replaces_head` (B-3 only) — full round-trip with a
  mock summarizer endpoint.

## Estimated effort

| Item | Difficulty | Approx LOC |
| ---- | ---- | ---- |
| B-1 settings | Easy | ~150 (settings field plumbing + UI form) |
| B-2 state persistence | Moderate | ~300 (port `state.rs` + serde + AIConversation hookup) |
| B-3 summarization path | Moderate-High | ~400 (commit.rs port + summarizer LLM call + splice logic) |
| B-4 `/compact` command | Easy | ~80 (input bar slash command + dispatch) |
| B-5 integration tests | Easy | ~200 |

Total ~1,100 LOC.

## Non-goals

- Multi-protocol routing (genai integration) — staying OpenAI-only per GH9303.
- LRC + CLI subagent — separate follow-up; orthogonal to compaction.
