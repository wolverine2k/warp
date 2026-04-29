# Product Spec: Custom Local LLM Provider

**Issue:** [warpdotdev/warp#9303](https://github.com/warpdotdev/warp/issues/9303) — *"Custom OpenAI-Compatible Provider Endpoints (BYO Endpoint for Local & Remote LLMs)"* (consolidates #6026, #8708, #4339, #5735, #4687, #3779, #9368)
**Issue status:** `triaged`, `enhancement`, `duplicate` — **not yet `ready-to-spec`** at time of writing. This spec is staged ahead of the label so it's ready to convert into `specs/GH9303/` immediately on labeling.
**Figma:** none provided

## Summary

Let a user point Warp's Agent Mode at their own OpenAI-compatible HTTP endpoint — Ollama, LM Studio, vLLM, llama.cpp server, NVIDIA NIM, or a third-party gateway like OpenRouter / Together / Groq used directly. The configured endpoint receives the chat request directly from the Warp client; no traffic is sent to `warp.dev` for that turn.

The hard truth, surfaced clearly because the issue thread already proves users will discover it the hard way (see comments from `Aeromix` and the architectural breakdown in `tbitcs`'s reply): **Warp's system prompt and tool JSON schemas live on Warp's backend and are not in the OSS client.** A purely client-side BYO-endpoint integration must therefore re-author both. Visible chat works easily; full agent autonomy (file edits, shell commands, etc.) requires the user's model to be capable of tool calling AND requires us to ship reasonable schemas for every Warp tool we want exposed. This spec commits to delivering both — a reusable generic system prompt and a JSON-schema translation table for the core tool set — and is honest about the quality gap vs. Warp's tuned models.

## Architectural choice (Path 1 of 3 raised in the issue)

The issue thread surfaced three feasible architectures. This spec picks **Path 1**:

- **Path 1 — Client-owned orchestration (this spec).** The OSS client builds the prompt, ships JSON schemas for tools, talks directly to the user's endpoint, parses tool calls, and feeds tool results back on the next turn. Pros: fully contributor-buildable, no backend dependency, works offline. Cons: we re-author Warp's system prompt and tool schemas — quality won't match Warp's tuned models out of the box; agent autonomy depends on the user's model.
- **Path 2 — Inference Delegation (`tbitcs`'s proposal).** Warp's backend builds the prompt + tool schemas as it does today, but emits a new `ClientAction::ExecuteLLMInference` carrying the fully-formulated OpenAI payload; the client forwards to the user's endpoint and streams the result back to Warp's server, which continues the agent loop. Pros: 100% reuse of Warp's "secret sauce"; the local model only sees inference-time content, not Warp's prompt engineering. Cons: requires Warp backend cooperation (a new `ClientAction` variant in `warp-proto-apis` AND a Warp server implementation); doesn't work offline; metadata still flows to Warp.
- **Path 3 — Hybrid provider adapters.** Variants of Path 1 with backend assistance for some tools.

Path 1 is in scope here because it's the only path a community contributor can land end-to-end without backend buy-in. Path 2 is the right long-term answer for parity and is enumerated in **Follow-ups** below as a recommended upstream filing for the Warp team. The two paths can coexist; Path 1 doesn't preclude Path 2.

## Problem

Today the Warp client funnels every Agent Mode request through `warp.dev`'s backend. Even with "Bring Your Own API Key" enabled, the user's key is forwarded to Warp's server, which then proxies to OpenAI/Anthropic/Google/OpenRouter — and only those four providers, only those models that Warp's server-side registry exposes. Users who want to:

- Use a local model on their own machine (privacy, offline, latency, cost)
- Test a fine-tune they host themselves
- Use a provider Warp's backend doesn't proxy (a corporate NIM endpoint, a vLLM server, a niche cloud gateway)
- Develop against a local mock server

have no path to do so without forking the closed-source backend.

## Goals

- A user can configure one custom provider (base URL + model id + optional API key) in settings and select that provider's model from the Agent Mode model picker.
- When that model is selected, the chat turn is sent **directly** from the client to the user's endpoint over HTTPS/HTTP, bypassing `warp.dev` entirely for the LLM call.
- Streaming responses (text deltas) render incrementally in the UI, identical to the existing experience.
- Tool calling works when the user's model supports the OpenAI `tools` field. The client ships a curated subset of Warp's tool catalog as JSON schemas (initial set: `read_files`, `apply_file_diffs`, `run_shell_command`, `grep`, `file_glob_v2`; expandable later) so file edits, shell commands, and search continue to function.
- Reasoning/thinking content (when emitted by the model) renders in the existing "thinking" UI region.
- A reusable, model-agnostic Warp system prompt template ships with the client; users see (and can later override) the prompt that gets prepended to their conversation.
- The feature is hidden behind a feature flag while it stabilizes; off by default.

## Known limitations users will see (called out up-front)

These are not bugs — they're design consequences of being a client-only contribution.

- **Quality gap vs. Warp's tuned models.** Warp's backend tunes its system prompt and tool schemas continuously against its model lineup. Our re-authored generic prompt won't match that. Smaller / weaker local models will visibly struggle with tool-call formatting, multi-step planning, and diff generation. Settings page documents this; first-run dialog warns users.
- **Reduced tool coverage in v1.** Out of 33 tool variants in `Message.ToolCall.tool`, v1 ships JSON schemas for the 5 listed under Goals. Other tools (MCP read/call, computer-use, web-search, web-fetch, code-review, ask-user-question, start-agent, todos, skills, upload-file-artifact, etc.) are not exposed to the local model in v1. The agent gracefully degrades — those features just don't appear as available actions on a local-model turn.
- **No Warp-side rate limiting / safety filters.** The user's prompt + Warp's tool descriptions go to the user's endpoint as-is. Endpoint trust is the user's responsibility.
- **Streaming behavior depends on the local server.** Local servers vary in tool-call streaming maturity; we test against the common ones (Ollama, LM Studio, vLLM, llama.cpp, NIM) and document quirks.

## Non-goals

- **Multiple custom providers configured simultaneously.** v1 supports one. (Open question: whether this should be N at launch — see below.)
- **Anthropic-format or Google-format endpoints.** Only OpenAI Chat Completions wire format. Users with Claude or Gemini keys go through the existing BYO-keys path or an OpenAI-compatible gateway in front of those models.
- **Cloud sync / multi-device sync of provider config.** Stays local to the device.
- **Telemetry / billing / quota tracking** for the custom provider. Warp tracks zero usage data for these turns. Users see no cost label on local models in the picker.
- **Replacing Warp's backend for non-AI features.** Code completions, command corrections, MCP, voice input, etc. continue to use the Warp backend (out of scope).
- **Auto-discovery** of running local servers (Ollama on `:11434`, LM Studio on `:1234`, etc.). v1 is fully manual.
- **Model-list discovery** from the endpoint (calling `/v1/models`). User types the model id explicitly in v1.
- **Image / vision input** to local models. The picker entry advertises `vision_supported: false`; image attachments are stripped from the request.

## User experience

### Configuration

1. The user opens **Settings → AI → Custom Providers** (a new subpage). The page is only visible when the `LocalLlmProvider` feature flag is on.
2. The page shows one form with these fields:
   - **Enabled** — checkbox; off by default.
   - **Display name** — free text, defaults to `Local`. Used in the model picker.
   - **Base URL** — required. Example: `http://localhost:11434/v1` (Ollama) or `http://localhost:1234/v1` (LM Studio). Must be a valid HTTP/HTTPS URL.
   - **Model id** — required. The string the user's endpoint expects (e.g. `qwen2.5-coder:7b`, `llama3.1:70b-instruct`, `meta/llama-3.1-70b-instruct`).
   - **API key** — optional, password-masked. Sent as `Authorization: Bearer <key>` if present.
   - **Supports tool calls** — checkbox; on by default. When off, the request omits the `tools` field and the agent runs in text-only mode.
3. Save persists non-secret fields to the user's settings TOML and the API key to the OS secure-storage keychain (mirroring how the existing BYO keys are stored).
4. The form has a **Test connection** button that does a one-shot non-streaming completion (`Hello, please respond with the word "ready".`) and shows the model's literal reply, or the HTTP status / error.

### Using the provider

1. With a custom provider configured and enabled, the Agent Mode model picker shows an additional entry: **"<Display name>: <model id>"** at the bottom of the list.
2. Selecting it sets the active model. The Agent Mode chat input behaves identically.
3. On send, the request goes to `<base URL>/chat/completions` with the user's headers. Streaming text appears incrementally.
4. If the model emits OpenAI-style `tool_calls`, the agent runs them (file edits, shell commands, etc.) just like with Warp's models.
5. If the model emits a `<think>...</think>` block or `delta.reasoning_content` (DeepSeek/Qwen-style) or OpenAI-style `reasoning` content, it renders in the "thinking" UI region.
6. Stop / cancel works.
7. The conversation persists locally as usual; conversation history is replayed on subsequent turns.

### Failure modes & messaging

- **Endpoint unreachable** (connection refused, DNS, timeout): the in-conversation error reads `Couldn't reach <Display name> at <base URL>: <reason>`. Conversation is preserved; the user can retry.
- **HTTP 401/403**: `<Display name> rejected the request (HTTP <code>): check your API key`.
- **HTTP 4xx other**: shows the status code and the response body's first ~200 chars.
- **HTTP 5xx**: `<Display name> server error (HTTP <code>); try again`.
- **Streaming error mid-response** (connection drop, malformed chunk): partial output is preserved as a finished assistant turn; an error toast says `Stream from <Display name> ended unexpectedly`.
- **Tool call returned but Supports tool calls is off**: the tool call is rendered as plain text content; agent does not execute it.
- **Provider config invalid** (empty base URL, malformed URL): the model is hidden from the picker; settings page shows inline validation.
- **Disabled / unconfigured**: the picker entry is absent; selecting a "Local" model from a previous session falls back to Warp's default model with a one-time toast `Local provider is no longer configured; falling back to <default>`.

### Edge cases

- **Switching providers mid-conversation.** Switching from Warp → Local on an existing thread starts the next turn against the local endpoint with the prior history. Switching Local → Warp does the same in reverse. No history reset.
- **Tool definitions.** When tool calls are enabled, the same tool list Warp would have sent (file edit, shell, etc.) is sent in OpenAI `tools` format. The user's model decides which it can use.
- **Slow first token.** Loading a large local model on first request can take 10s+. The existing "thinking" UI applies. No client-side timeout under 120s.
- **Cancellation.** Pressing stop drops the HTTP stream and discards in-flight chunks, matching today's behavior.
- **Long context.** No client-side context-window check; if the model rejects an oversized request the HTTP error path applies.
- **Empty `choices` from the endpoint** (model warmup glitch): treated as "stream ended without content"; the assistant turn is empty and the error toast appears.

## Success criteria

1. With `LocalLlmProvider` flag on, base URL set to `http://localhost:11434/v1`, model id `llama3.1`, the Agent Mode picker shows a `Local: llama3.1` entry.
2. Selecting that entry and sending `What is 2+2?` streams the model's reply into the conversation.
3. Network inspection confirms zero requests to `*.warp.dev` during that turn (LLM call only — non-AI traffic still goes to Warp).
4. Selecting a model with tool support enabled, then asking it to read a file, results in the file being read via Warp's existing tool runner if the model emits a valid `tool_calls` block.
5. Reasoning content from a model that emits `<think>` tags renders in the thinking UI, separate from the visible answer.
6. With the endpoint stopped, sending a message produces the documented "couldn't reach" error and does not crash.
7. With the API key field populated, the outgoing request carries `Authorization: Bearer <key>`.
8. With tool support unchecked, the outgoing request omits the `tools` field.
9. Disabling the provider and restarting Warp removes the picker entry; previously-selected local models fall back gracefully.
10. The OS keychain entry for the API key is removed when the user clears the key field.

## Validation

- **Unit tests** for the OpenAI-SSE → `ResponseEvent` adapter: text deltas, tool-call deltas, reasoning deltas, finish reasons, malformed chunks, multi-tool-call interleaving.
- **Integration test** under `crates/integration/` that runs a minimal mock OpenAI-compatible HTTP server and exercises a full agent turn end-to-end, including a tool-call round-trip.
- **Manual smoke** against Ollama, LM Studio, and a NIM endpoint (matrix of: tool calls on/off, API key on/off, HTTP vs HTTPS).
- **Network audit** via Charles/mitmproxy that confirms no warp.dev traffic during a local-provider turn.

## Open questions

1. **Single provider vs N.** v1 above defines "one provider with one model id". Should the form support a list of `(name, base URL, model id, key, tool-cap)` rows so a user can flip between Ollama and a NIM endpoint without retyping? Default proposal: ship single in v1, extend if requested.
2. **Should the model picker entry surface the host (e.g. "Local · llama3.1 · localhost")?** Default proposal: yes — the host hint reduces "which model am I talking to" confusion in screenshots and bug reports.
3. **`/v1/models` discovery for the model id field.** Skipped for v1 (manual entry). Worth a follow-up.
4. **Tool-call format negotiation.** OpenAI's `tools` field is the only supported format. Anthropic-style `tool_use` blocks (used by Claude) are out of scope; users who want Claude must go through an OpenAI-compatible gateway like LiteLLM. Confirm this is acceptable.
5. **Should the "thinking" toggle (if Warp later exposes one) be per-provider or global?** Out of scope for v1 but worth noting.
6. **Quota / "credits" display.** Warp's UI shows credit cost on each model. For local models this should render blank (not "0 credits", which implies free Warp credits). Confirm copy.
7. **Telemetry.** Should we log a coarse-grained counter (`local_provider_turn`) so we can see adoption, even though we don't log content? Default proposal: yes, opt-out via existing telemetry settings.
8. **System prompt visibility & override.** Should users see the full Warp-style system prompt we ship? Should they be allowed to override it for advanced use? Default proposal: show it read-only in the settings page; do not allow override in v1 (avoids debugging "I broke my own prompt" support load); add a follow-up for an opt-in advanced-prompt-override toggle.
9. **Tool-set coverage.** v1 ships 5 tool schemas. Should we ship more out of the gate (e.g., MCP, web-search) or keep tight and expand iteratively after dogfood? Default proposal: keep tight; each schema needs round-trip tests against the proto's `Message::ToolCall.tool` oneof.
10. **Should we file `ExecuteLLMInference` (Path 2) as a separate upstream issue against `warp-proto-apis` after Path 1 lands?** Default proposal: yes — Path 2 is the right long-term parity story; getting the proto extension on the Warp team's radar is a community-friendly follow-up.
