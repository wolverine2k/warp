# Product Spec: Custom Local LLM Provider

**Issue:** none yet (this spec is being authored speculatively; will be linked once an issue exists)
**Figma:** none provided

## Summary

Let a user point Warp's Agent Mode at their own OpenAI-compatible HTTP endpoint — for example a local Ollama, LM Studio, vLLM, llama.cpp server, NVIDIA NIM deployment, or a third-party gateway like OpenRouter / Together / Groq used directly. The configured endpoint receives the chat request directly from the Warp client; no traffic is sent to `warp.dev` for that turn. Existing Agent Mode features (conversation history, tool calls, reasoning content) work to the extent the user's model supports them.

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
- Tool calling works when the user's model supports the OpenAI `tools` field (so file edits, shell commands, etc. continue to function).
- Reasoning/thinking content (when emitted by the model) renders in the existing "thinking" UI region.
- The feature is hidden behind a feature flag while it stabilizes; off by default.

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
